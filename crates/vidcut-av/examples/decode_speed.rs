//! How fast a file's video decodes on the CPU and on NVDEC: frames a second
//! decoding on from the start, and the time a seek takes to a frame in the
//! middle of a GOP.
//!
//! `cargo run -p vidcut-av --example decode_speed -- FILE [FRAMES]`

use std::time::Instant;

use vidcut_av::cuda::CudaDevice;
use vidcut_av::decode::{Input, VideoReader};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("FILE"));
    let frames: usize = args.next().map_or(300, |n| n.parse().expect("FRAMES"));
    let input = Input::File(path.clone());
    let device = CudaDevice::open().ok();
    for gpu in [false, true] {
        let open = Instant::now();
        let mut reader = match (gpu, &device) {
            (false, _) => VideoReader::open_input(&input).unwrap(),
            (true, Some(d)) => match VideoReader::open_cuda(&input, d) {
                Ok(r) => r,
                Err(e) => {
                    println!("NVDEC: {e}");
                    continue;
                }
            },
            (true, None) => {
                println!("NVDEC: no CUDA device");
                continue;
            }
        };
        let opened = open.elapsed();
        let start = Instant::now();
        let mut n = 0;
        while n < frames && reader.next_frame().unwrap().is_some() {
            n += 1;
        }
        let secs = start.elapsed().as_secs_f64();
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
        println!(
            "{}: opened in {:.0} ms; {n} frames in {secs:.2} s ({:.0} fps); seeks {:.0?} ms",
            if gpu { "NVDEC" } else { "CPU" },
            opened.as_secs_f64() * 1e3,
            n as f64 / secs,
            seeks
        );
    }
}
