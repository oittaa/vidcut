//! How fast a file's video decodes on the CPU and on each GPU decoder there
//! is (NVDEC, VA-API, Direct3D 11): frames a second decoding on from the
//! start, the frames left on the GPU and again copied down to memory (as the
//! player takes VA-API's and Direct3D 11's), with the CPU it took; and the
//! time a seek takes to a frame in the middle of a GOP.
//!
//! `cargo run -p vidcut-av --example decode_speed -- FILE [FRAMES]`

use std::time::Instant;

use vidcut_av::decode::{Input, VideoReader};
use vidcut_av::hw::GpuDecoder;

/// Decodes up to `frames` from the start, copying each down when `down`:
/// the frames, the seconds and the CPU-seconds it took.
fn decode(reader: &mut VideoReader, frames: usize, down: bool) -> (usize, f64, f64) {
    let (cpu, start) = (vidcut_av::cpu_seconds(), Instant::now());
    let mut n = 0;
    while n < frames {
        let Some(f) = reader.next_frame().unwrap() else {
            break;
        };
        // Interlaced frames of VA-API and Direct3D 11 came down already,
        // for the CPU to deinterlace.
        if down && vidcut_av::hw::on_gpu(&f) {
            vidcut_av::hw::download(&f).unwrap();
        }
        n += 1;
    }
    (
        n,
        start.elapsed().as_secs_f64(),
        vidcut_av::cpu_seconds() - cpu,
    )
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("FILE"));
    let frames: usize = args.next().map_or(300, |n| n.parse().expect("FRAMES"));
    let input = Input::File(path.clone());
    let mut decoders: Vec<(String, Option<GpuDecoder>)> = vec![("CPU".into(), None)];
    for gpu in GpuDecoder::all() {
        let on = match gpu.on() {
            "" => String::new(),
            on => format!(" on {on}"),
        };
        decoders.push((format!("{}{on}", gpu.kind().name()), Some(gpu)));
    }
    for (name, gpu) in &decoders {
        let open = |input: &Input| match gpu {
            None => VideoReader::open_input(input),
            Some(g) => VideoReader::open_on(input, g),
        };
        let started = Instant::now();
        let mut reader = match open(&input) {
            Ok(r) => r,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let opened = started.elapsed();
        let (n, secs, cpu) = decode(&mut reader, frames, false);
        let mut line = format!(
            "{name}: opened in {:.0} ms; {n} frames in {secs:.2} s ({:.0} fps, {:.1} cores)",
            opened.as_secs_f64() * 1e3,
            n as f64 / secs,
            cpu / secs,
        );
        if gpu.is_some() {
            let (n, secs, cpu) = decode(&mut open(&input).unwrap(), frames, true);
            line += &format!(
                ", copied down {:.0} fps ({:.1} cores)",
                n as f64 / secs,
                cpu / secs
            );
        }
        let tb = reader.time_base();
        let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
        // Seeks to the middle of each of the first GOPs and decodes to there.
        let mut seeks = Vec::new();
        for k in 1..=6 {
            let t = k as f64 * 2.0 + 1.0;
            let s = Instant::now();
            reader.seek((t / tick) as i64).unwrap();
            while let Some(f) = reader.next_frame().unwrap() {
                if f.pts().unwrap_or(0) as f64 * tick >= t {
                    break;
                }
            }
            seeks.push(s.elapsed().as_secs_f64() * 1e3);
        }
        println!("{line}; seeks {seeks:.0?} ms");
    }
}
