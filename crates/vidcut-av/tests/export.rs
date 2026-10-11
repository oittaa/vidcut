//! Export engine tests: render real fixtures and verify the output frame by
//! frame (barcodes) and sample by sample (beep onsets).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::export::{self, encode::Availability};
use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, VideoCodec};
use vidcut_core::export::{
    AudioCodec as ACodec, AudioSettings, Container as OutContainer, EncoderChoice, ExportSettings,
    VideoCodec as VCodec, VideoSettings,
};
use vidcut_core::render::{AudioPiece, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

fn out_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vidcut-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Src {
    path: PathBuf,
    origin: f64,
}

fn src(spec: FixtureSpec) -> Src {
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    Src { path, origin }
}

fn v(s: &Src, source_frame: i64, frames: i64) -> VideoPiece {
    VideoPiece::Source {
        media: "m".into(),
        path: s.path.clone(),
        origin: s.origin,
        source_frame,
        frames,
    }
}

fn a(s: &Src, fps: Rational, rate: u32, source_frame: i64, frames: i64) -> AudioPiece {
    AudioPiece::Source {
        media: "m".into(),
        path: s.path.clone(),
        stream: 0,
        origin: s.origin,
        source_sample: fps.frame_to_sample(source_frame, rate),
        samples: fps.frame_to_sample(frames, rate),
        gain_db: 0.0,
    }
}

fn plan(
    fps: Rational,
    w: u32,
    h: u32,
    video: Vec<VideoPiece>,
    audio: Vec<Vec<AudioPiece>>,
) -> RenderPlan {
    let frames = video.iter().map(VideoPiece::frames).sum();
    RenderPlan {
        fps,
        width: w,
        height: h,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video,
        audio,
        range: (0, frames),
        name: "t".into(),
    }
}

/// Lossless-enough settings so barcodes and beeps survive exactly: FFV1 + PCM in MKV.
fn lossless() -> ExportSettings {
    ExportSettings {
        container: OutContainer::Mkv,
        video: Some(VideoSettings {
            codec: VCodec::H264,
            encoder: EncoderChoice::Cpu,
            quality: Some(0),
            bitrate: None,
            speed: Some("ultrafast".into()),
            profile: None,
            bit_depth: None,
            width: None,
            height: None,
        }),
        audio: Some(AudioSettings {
            codec: ACodec::PcmS16,
            bitrate: None,
            sample_rate: None,
            channels: None,
        }),
        declick: false,
    }
}

fn render(plan: &RenderPlan, settings: &ExportSettings, out: &Path) -> export::Report {
    export::run(
        plan,
        settings,
        out,
        &Availability::default(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_or_else(|e| panic!("export: {e}"))
}

/// Barcodes of every output frame.
fn barcodes(path: &Path) -> Vec<Option<u32>> {
    let mut r = VideoReader::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        out.push(read_barcode(&f).unwrap());
    }
    out
}

/// Decodes the first audio stream to mono f32 at its own rate.
fn audio_mono(path: &Path) -> Vec<f32> {
    use vidcut_av::ffmpeg::{self, codec, format, frame, media};
    vidcut_av::init();
    let mut input = format::input(path).unwrap();
    let stream = input.streams().best(media::Type::Audio).unwrap();
    let index = stream.index();
    let mut dec = codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .audio()
        .unwrap();
    let mut out = Vec::new();
    // Tests export 48 kHz 16-bit PCM, so no resampling is needed here.
    let take = |dec: &mut ffmpeg::decoder::Audio, out: &mut Vec<f32>| {
        let mut f = frame::Audio::empty();
        while dec.receive_frame(&mut f).is_ok() {
            let ch = f.channels() as usize;
            let n = f.samples();
            match f.format() {
                format::Sample::I16(format::sample::Type::Packed) => {
                    let d = f.data(0);
                    for i in 0..n {
                        let v = i16::from_le_bytes([d[i * ch * 2], d[i * ch * 2 + 1]]);
                        out.push(f32::from(v) / 32768.0);
                    }
                }
                other => panic!("unexpected sample format {other:?}"),
            }
        }
    };
    for (s, p) in input.packets() {
        if s.index() == index {
            dec.send_packet(&p).unwrap();
            take(&mut dec, &mut out);
        }
    }
    dec.send_eof().unwrap();
    take(&mut dec, &mut out);
    out
}

#[test]
fn same_rate_cuts_are_frame_and_sample_exact() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::PcmS16,
            48_000,
            2,
        ),
    );
    let b1 = src(
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::Flac,
            48_000,
            2,
        ),
    );
    let ts = src(
        FixtureSpec::new(VideoCodec::H264, Container::MpegTs, 320, 180, (30, 1), 90)
            .start_offset_ms(1400),
    );
    let np = src(FixtureSpec::new(
        VideoCodec::Vp9,
        Container::Webm,
        320,
        180,
        (30, 1),
        60,
    ));
    // Segments: a1 10..40, b1 45..75, ts 31..61, black 5, np 3..33.
    let pieces = vec![
        v(&a1, 10, 30),
        v(&b1, 45, 30),
        v(&ts, 31, 30),
        VideoPiece::Black { frames: 5 },
        v(&np, 3, 30),
    ];
    let lane = vec![
        a(&a1, fps, 48_000, 10, 30),
        a(&b1, fps, 48_000, 45, 30),
        AudioPiece::Silence {
            samples: fps.frame_to_sample(35, 48_000),
        },
        a(&np, fps, 48_000, 3, 30),
    ];
    let p = plan(fps, 320, 180, pieces, vec![lane]);
    let out = out_dir("exact").join("out.mkv");
    let report = render(&p, &lossless(), &out);
    assert_eq!(report.frames, 125);

    let codes = barcodes(&out);
    let mut want: Vec<Option<u32>> = Vec::new();
    want.extend((10..40).map(Some));
    want.extend((45..75).map(Some));
    want.extend((31..61).map(Some));
    want.extend(std::iter::repeat_n(None, 5));
    want.extend((3..33).map(Some));
    assert_eq!(codes.len(), want.len(), "frame count");
    let bad: Vec<_> = codes
        .iter()
        .zip(&want)
        .enumerate()
        .filter(|(_, (g, w))| g != w)
        .collect();
    assert!(bad.is_empty(), "frames differing from the plan: {bad:?}");

    // Audio: exact length; beeps where the sources had them.
    let mono = audio_mono(&out);
    assert_eq!(mono.len() as i64, p.samples(), "sample count");
    let onsets = vidcut_av::beep::onsets(&mono, 0.1, 4800);
    // a1 10..40 (0.333..1.333 s) has its 1 s beep at output 0.667 s;
    // b1 45..75 (1.5..2.5 s) has its 2 s beep at output 1.0 + 0.5 = 1.5 s;
    // np 3..33 (0.1..1.1 s) starts at output 3.1667 s and beeps at 1.0 s -> 4.0667 s.
    let expect = [0.6667, 1.5, 4.0667];
    assert_eq!(onsets.len(), expect.len(), "beeps at {onsets:?}");
    for (got, want) in onsets.iter().zip(expect) {
        let t = *got as f64 / 48_000.0;
        assert!(
            (t - want).abs() < 0.001,
            "beep at {t:.4}, want {want:.4} (A/V offset > 1 ms)"
        );
    }
}

#[test]
fn other_rates_are_resampled_by_nearest_frame() {
    // A 25 fps source on a 30 fps timeline: 5 source frames per 6 output frames.
    let fps = Rational::new(30, 1).unwrap();
    let s25 = src(FixtureSpec::new(
        VideoCodec::ProRes422Hq,
        Container::Mov,
        320,
        180,
        (25, 1),
        50,
    )
    .no_audio());
    let p = plan(fps, 320, 180, vec![v(&s25, 0, 12)], vec![]);
    let out = out_dir("rate").join("out.mkv");
    let mut settings = lossless();
    settings.audio = None;
    render(&p, &settings, &out);
    let codes: Vec<u32> = barcodes(&out).into_iter().map(Option::unwrap).collect();
    // Output frame j at j/30 s, minus half a frame: first 25 fps frame at or after.
    let want: Vec<u32> = (0..12)
        .map(|j| (((j as f64 / 30.0 - 1.0 / 60.0) * 25.0).ceil().max(0.0)) as u32)
        .collect();
    assert_eq!(codes, want);
    assert_eq!(codes, vec![0, 1, 2, 3, 3, 4, 5, 6, 7, 8, 8, 9]);
}

#[test]
fn scaling_padding_and_presets() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let p = plan(
        fps,
        320,
        180,
        vec![v(&a1, 0, 60)],
        vec![vec![a(&a1, fps, 48_000, 0, 60)]],
    );
    let dir = out_dir("presets");
    for preset in vidcut_core::export::builtin_presets() {
        let mut settings = preset.settings.clone();
        if let Some(v) = settings.video.as_mut() {
            v.encoder = EncoderChoice::Cpu;
            // Keep the test quick: small frames even for the 4K preset.
            if v.width.is_some() {
                v.width = Some((v.width.unwrap() / 4) & !1);
                v.height = Some((v.height.unwrap() / 4) & !1);
            }
            if v.codec == VCodec::Av1 {
                v.speed = Some("12".into());
            }
        }
        let mut p = p.clone();
        if let Some(a) = &settings.audio
            && a.codec == ACodec::Opus
        {
            p.sample_rate = 48_000;
        }
        let out = dir.join(format!(
            "{}.{}",
            preset.name,
            settings.container.extension()
        ));
        let report = render(&p, &settings, &out);
        let info = vidcut_av::probe::probe(&out).unwrap_or_else(|e| panic!("{}: {e}", preset.name));
        if let Some(vs) = &settings.video {
            let vi = info
                .video
                .as_ref()
                .unwrap_or_else(|| panic!("{}: no video", preset.name));
            let (w, h) = (vs.width.unwrap_or(320), vs.height.unwrap_or(180));
            assert_eq!((vi.width, vi.height), (w, h), "{}", preset.name);
            // Decodable, and the barcode survives scaling and lossy coding.
            let codes = barcodes(&out);
            assert_eq!(codes.len(), 60, "{}: frames", preset.name);
            if w * 180 == h * 320 {
                assert_eq!(codes[10], Some(10), "{}: barcode", preset.name);
            }
        } else {
            assert!(info.video.is_none(), "{}", preset.name);
        }
        assert_eq!(info.audio.len(), 1, "{}: audio", preset.name);
        assert!(report.audio_encoder.is_some());
    }
}

#[test]
fn still_frames_follow_the_sampling_rule() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let png = vidcut_av::export::still::png(&v(&a1, 37, 1), fps, 320, 180).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let path = out_dir("still").join("f.png");
    std::fs::write(&path, &png).unwrap();
    let mut r = VideoReader::open(&path).unwrap();
    let f = r.next_frame().unwrap().unwrap();
    assert_eq!(read_barcode(&f).unwrap(), Some(37));
}

#[test]
fn cancelling_leaves_no_files() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let p = plan(fps, 320, 180, vec![v(&a1, 0, 90)], vec![]);
    let dir = out_dir("cancel");
    let out = dir.join("out.mkv");
    let cancel = AtomicBool::new(false);
    let mut settings = lossless();
    settings.audio = None;
    let err = export::run(
        &p,
        &settings,
        &out,
        &Availability::default(),
        &cancel,
        &mut |pr| {
            if pr.frames == 10 {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        },
    )
    .unwrap_err();
    assert!(export::is_cancelled(&err), "{err}");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "no output or .part left"
    );
}

#[test]
fn many_segments_keep_memory_flat() {
    // 400 segments alternating between two sources: memory must not grow with
    // the number of segments (one decoder open at a time).
    let fps = Rational::new(30, 1).unwrap();
    let x = src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 360, (30, 1), 300).gop(60));
    let y = src(FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        640,
        360,
        (30, 1),
        300,
    ));
    let mut video = Vec::new();
    let mut lane = Vec::new();
    for i in 0..400i64 {
        let (s, at) = if i % 2 == 0 {
            (&x, (i * 37) % 290)
        } else {
            (&y, (i * 53) % 290)
        };
        video.push(v(s, at, 2));
        lane.push(a(s, fps, 48_000, at, 2));
    }
    let p = plan(fps, 640, 360, video, vec![lane]);
    let out = out_dir("many").join("out.mkv");
    let before = peak_rss_mb();
    render(&p, &lossless(), &out);
    let grew = peak_rss_mb() - before;
    eprintln!("400 segments: peak RSS grew by {grew} MB");
    assert!(grew < 500, "peak RSS grew by {grew} MB");
    assert_eq!(barcodes(&out).len(), 800);
}

fn peak_rss_mb() -> i64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l.split_whitespace().nth(1).unwrap().parse::<i64>().unwrap() / 1024)
        })
        .unwrap_or(0)
}

#[cfg(feature = "gpu-tests")]
#[test]
fn gpu_encoders_produce_valid_frame_exact_output() {
    let available = vidcut_av::export::encode::detect();
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        640,
        360,
        (30, 1),
        90,
    ));
    let p = plan(
        fps,
        640,
        360,
        vec![v(&a1, 15, 45)],
        vec![vec![a(&a1, fps, 48_000, 15, 45)]],
    );
    let dir = out_dir("gpu");
    let mut tested = 0;
    for status in available.encoders.iter().filter(|e| e.ok) {
        let codec = match status.encoder.split('_').next().unwrap() {
            "h264" => VCodec::H264,
            "hevc" => VCodec::Hevc,
            "av1" => VCodec::Av1,
            other => panic!("unexpected {other}"),
        };
        let ten = status.pixel == "p010le";
        let settings = ExportSettings {
            container: OutContainer::Mp4,
            video: Some(VideoSettings {
                codec,
                encoder: status.kind,
                quality: Some(18),
                bitrate: None,
                speed: None,
                profile: None,
                bit_depth: Some(if ten { 10 } else { 8 }),
                width: None,
                height: None,
            }),
            audio: Some(AudioSettings {
                codec: ACodec::Aac,
                bitrate: None,
                sample_rate: None,
                channels: None,
            }),
            declick: true,
        };
        let out = dir.join(format!("{}-{}.mp4", status.encoder, status.pixel));
        let report = export::run(
            &p,
            &settings,
            &out,
            &available,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_or_else(|e| panic!("{} {}: {e}", status.encoder, status.pixel));
        assert_eq!(
            report.video_encoder.as_deref(),
            Some(status.encoder.as_str())
        );
        let codes = barcodes(&out);
        assert_eq!(codes.len(), 45, "{}", status.encoder);
        let want: Vec<Option<u32>> = (15..60).map(Some).collect();
        assert_eq!(codes, want, "{} {}: frames", status.encoder, status.pixel);
        tested += 1;
    }
    eprintln!("verified {tested} GPU encoder configurations");
    assert!(tested > 0, "no GPU encoder available");
}
