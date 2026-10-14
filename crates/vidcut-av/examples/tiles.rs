//! Times timeline filmstrip tiles: frames of FILE at a fixed spacing, read
//! in order as the timeline asks for them; `gpu` decodes on the GPU the
//! viewer decodes on (`VIDCUT_HWDEC` picks another).
//!
//! `cargo run --release -p vidcut-av --example tiles -- FILE [HEIGHT] [gpu]`
//! (`gpu` may come without a height).

use std::path::PathBuf;
use std::time::Instant;

use vidcut_av::export::renderer::FrameReader;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("FILE"));
    let rest: Vec<String> = args.collect();
    let gpu = rest
        .iter()
        .any(|a| a == "gpu")
        .then(|| vidcut_av::hw::GpuDecoder::best().expect("a GPU decoder"));
    let height: u32 = match rest.iter().find(|a| *a != "gpu") {
        Some(h) => h.parse().expect("HEIGHT"),
        None => 112,
    };
    let info = vidcut_av::probe::probe(&path, gpu.as_ref()).expect("probe");
    let video = info.video.expect("a video stream");
    let duration = video.duration_secs.unwrap_or(0.0);
    let origin = video.first_frame_time.unwrap_or(0.0);
    let fps = Rational::new(30, 1).expect("valid");
    let width = (f64::from(height) * 16.0 / 9.0).round() as u32;
    let frames = (duration * 30.0) as i64;
    println!(
        "{} ({:.0} s), tiles {width}x{height}, decoding on {}",
        path.display(),
        duration,
        gpu.as_ref().map_or("the CPU".to_string(), |d| format!(
            "{} {}",
            d.kind().name(),
            d.on()
        ))
    );
    for spacing in [0.25, 1.0, 2.0, 4.0, 8.0, 30.0] {
        let step = (spacing * 30.0) as i64;
        // Twenty tiles from a third of the way in.
        let from = frames / 3;
        let mut reader = gpu
            .clone()
            .map_or_else(FrameReader::default, FrameReader::decoding_on);
        let started = Instant::now();
        let mut first = None;
        let mut n = 0;
        for i in 0..20 {
            let at = from + i * step;
            if at >= frames {
                break;
            }
            let piece = VideoPiece::Source {
                pace: Default::default(),
                media: "m".into(),
                path: path.clone(),
                origin,
                source_frame: at,
                frames: 1,
                image: None,
            };
            let t = Instant::now();
            reader.rgba(&piece, fps, width, height).expect("tile");
            first.get_or_insert(t.elapsed());
            n += 1;
        }
        let all = started.elapsed();
        println!(
            "  every {spacing:>5} s: first {:>6.1} ms, then {:>6.1} ms a tile",
            first.unwrap_or_default().as_secs_f64() * 1e3,
            (all - first.unwrap_or_default()).as_secs_f64() * 1e3 / (n - 1).max(1) as f64
        );
    }
}
