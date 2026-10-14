//! Times timeline filmstrip tiles: frames of FILE at a fixed spacing, read
//! in order as the timeline asks for them; `gpu` decodes on the GPU the
//! viewer decodes on (`VIDCUT_HWDEC` picks another). `compare` makes twenty
//! tiles both on that GPU and on the CPU, and says how far apart they are.
//!
//! `cargo run --release -p vidcut-av --example tiles -- FILE [HEIGHT] [gpu|compare]`
//! (`gpu` or `compare` may come without a height).

use std::path::PathBuf;
use std::time::Instant;

use vidcut_av::export::renderer::FrameReader;
use vidcut_av::hw::GpuDecoder;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("FILE"));
    let rest: Vec<String> = args.collect();
    let comparing = rest.iter().any(|a| a == "compare");
    let gpu = (comparing || rest.iter().any(|a| a == "gpu"))
        .then(|| GpuDecoder::best().expect("a GPU decoder"));
    let height: u32 = match rest.iter().find(|a| *a != "gpu" && *a != "compare") {
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
    let piece = |at| VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: path.clone(),
        origin,
        source_frame: at,
        frames: 1,
        image: None,
    };
    if let (true, Some(gpu)) = (comparing, gpu.clone()) {
        compare(&piece, frames, fps, (width, height), gpu);
        return;
    }
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
            let t = Instant::now();
            reader.rgba(&piece(at), fps, width, height).expect("tile");
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

/// Twenty tiles a second apart from a third of the way in, made on `gpu`
/// and on the CPU: how many the GPU decoded, and how far its red, green and
/// blue are from the CPU's.
fn compare(
    piece: &dyn Fn(i64) -> VideoPiece,
    frames: i64,
    fps: Rational,
    (width, height): (u32, u32),
    gpu: GpuDecoder,
) {
    let mut on_gpu = FrameReader::decoding_on(gpu);
    let mut on_cpu = FrameReader::default();
    let (mut tiles, mut decoded) = (0, 0);
    let (mut sum, mut abs, mut most, mut values) = ([0i64; 3], 0u64, 0u16, 0u64);
    for i in 0..20 {
        let at = frames / 3 + i * 30;
        if at >= frames {
            break;
        }
        let a = on_gpu
            .rgba(&piece(at), fps, width, height)
            .expect("GPU tile");
        decoded += usize::from(on_gpu.decoded_on().is_some());
        let b = on_cpu
            .rgba(&piece(at), fps, width, height)
            .expect("CPU tile");
        for (p, q) in a
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.rgba.as_chunks::<4>().0)
        {
            for c in 0..3 {
                let d = i16::from(p[c]) - i16::from(q[c]);
                sum[c] += i64::from(d);
                abs += u64::from(d.unsigned_abs());
                most = most.max(d.unsigned_abs());
            }
            values += 3;
        }
        tiles += 1;
    }
    let pixels = (values / 3).max(1) as f64;
    println!("  {decoded} of {tiles} tiles decoded on the GPU");
    println!(
        "  GPU minus CPU on average: red {:+.2}, green {:+.2}, blue {:+.2}",
        sum[0] as f64 / pixels,
        sum[1] as f64 / pixels,
        sum[2] as f64 / pixels
    );
    println!(
        "  apart by {:.2} on average, {most} at most (of 255)",
        abs as f64 / values.max(1) as f64
    );
}
