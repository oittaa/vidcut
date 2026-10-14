//! Measures passes (decision D45): exports a stretch of a video at a target
//! bitrate and prints the bitrate reached, the time and the luma PSNR
//! against the source.
//!
//! `passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]`, e.g.
//! `passes bbb.mp4 60 20 h264 cpu 4 passes=2 first_pass=full`, where the
//! keys are `video.*` settings (`passes`, `first_pass`, `multipass`,
//! `lookahead`, `speed`); `q20` in place of MBPS is constant quality 20.
//! The output is 8-bit at the source's size, or `size=WxH`. `gpu` decodes
//! on the GPU in use, as the app exports, and says how many frames stayed
//! on it.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use vidcut_av::decode::VideoReader;
use vidcut_av::export::{self, video::VideoFrames};
use vidcut_av::ffmpeg::format::Pixel;
use vidcut_av::ffmpeg::frame;
use vidcut_core::export::{ExportSettings, apply_overrides, builtin_presets};
use vidcut_core::render::{RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

fn psnr_y(a: &frame::Video, b: &frame::Video) -> f64 {
    let (w, h) = (a.width() as usize, a.height() as usize);
    let (da, db) = (a.data(0), b.data(0));
    let (sa, sb) = (a.stride(0), b.stride(0));
    let mut se = 0u64;
    for y in 0..h {
        for x in 0..w {
            let d = i64::from(da[y * sa + x]) - i64::from(db[y * sb + x]);
            se += (d * d) as u64;
        }
    }
    let mse = se as f64 / (w * h) as f64;
    if mse == 0.0 {
        99.0
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [file, start, secs, codec, encoder, mbps, rest @ ..] = &args[..] else {
        eprintln!("usage: passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]");
        std::process::exit(2);
    };
    let path = PathBuf::from(file);
    let gpu = rest
        .iter()
        .any(|s| s == "gpu")
        .then(|| vidcut_av::hw::GpuDecoder::best().expect("a GPU decoder"));
    let size = rest.iter().find_map(|s| s.strip_prefix("size=")).map(|s| {
        let (w, h) = s.split_once('x').expect("size=WxH");
        (
            w.parse::<u32>().expect("width"),
            h.parse::<u32>().expect("height"),
        )
    });
    let rest: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|s| *s != "gpu" && !s.starts_with("size="))
        .collect();
    let info = vidcut_av::probe::probe(&path, None).expect("probe");
    let v = info.video.expect("video");
    let (width, height) = size.unwrap_or((v.width, v.height));
    let fps = Rational::new(i64::from(v.avg_frame_rate.0), i64::from(v.avg_frame_rate.1)).unwrap();
    let start = (start.parse::<f64>().unwrap() * fps.as_f64()).round() as i64;
    let frames = (secs.parse::<f64>().unwrap() * fps.as_f64()).round() as i64;
    let piece = VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: path.clone(),
        origin: v.first_frame_time.unwrap_or(0.0),
        source_frame: start,
        frames,
        image: None,
    };
    let plan = RenderPlan {
        fps,
        width,
        height,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video: vec![piece.clone()],
        audio: vec![],
        range: (0, frames),
        name: "passes".into(),
    };
    let mut settings: ExportSettings = builtin_presets()
        .into_iter()
        .find(|p| p.name == "h264-master")
        .unwrap()
        .settings;
    settings.container = vidcut_core::export::Container::Mkv;
    settings.audio = None;
    // `qN` instead of a bitrate: constant quality N.
    let rate = match mbps.strip_prefix('q') {
        Some(q) => vec![format!("video.quality={q}"), "video.bitrate=null".into()],
        None => vec![
            format!(
                "video.bitrate={}",
                (mbps.parse::<f64>().unwrap() * 1e6) as u64
            ),
            "video.quality=null".into(),
        ],
    };
    let mut sets = vec![
        format!("video.codec={codec}"),
        format!("video.encoder={encoder}"),
        "video.hdr=sdr".into(),
        "video.bit_depth=8".into(),
    ];
    sets.extend(rate);
    sets.extend(rest.iter().map(|s| format!("video.{s}")));
    apply_overrides(&mut settings, &sets).expect("settings");
    let dir = std::env::temp_dir().join(format!("vidcut-passes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.mkv");
    let available = vidcut_av::export::encode::detect();
    let clock = Instant::now();
    let report = export::run_decoding_on(
        &plan,
        &settings,
        &out,
        &available,
        gpu.as_ref(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .expect("export");
    let seconds = clock.elapsed().as_secs_f64();
    let bytes = std::fs::metadata(&out).unwrap().len();
    let got = bytes as f64 * 8.0 / (frames as f64 / fps.as_f64()) / 1e6;
    let target: f64 = mbps.parse().unwrap_or(got);
    // Luma PSNR against the source frames as the export reads them.
    let mut source = VideoFrames::new(&[piece], fps, width, height, Pixel::YUV420P, None);
    let mut reader = VideoReader::open(&out).unwrap();
    let (mut sum, mut worst, mut n) = (0.0, f64::MAX, 0);
    while let Some(f) = reader.next_frame().unwrap() {
        let s = source.next_frame().unwrap().unwrap();
        let p = psnr_y(&s, &f);
        sum += p;
        worst = worst.min(p);
        n += 1;
    }
    println!(
        "{codec} {encoder} {} {}: {got:.3} Mb/s for {target} ({:+.1}%), {seconds:.1} s, \
         PSNR {:.2} dB (worst frame {worst:.2}), passes {:?}",
        report.video_encoder.as_deref().unwrap_or("?"),
        rest.join(" "),
        (got / target - 1.0) * 100.0,
        sum / f64::from(n),
        report.passes,
    );
    if let Some(gpu) = &gpu {
        println!(
            "  {} of {} frames decoded on {}, {} of them kept on the GPU to the encoder",
            report.gpu_decoded,
            report.frames,
            gpu.kind().name(),
            report.gpu_frames
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
