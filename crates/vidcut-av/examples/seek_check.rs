//! Seeks to every frame of a barcode fixture, on the CPU and on each GPU
//! decoder there is, and says where a seek shows other frames than the
//! file's own from there (three read on).
//!
//! `cargo run -p vidcut-av --example seek_check -- FILE...`

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::ffmpeg::frame;
use vidcut_av::hw::GpuDecoder;

/// The barcode of `f`, brought down first when it is on the GPU.
fn barcode(f: &frame::Video) -> Option<u32> {
    if vidcut_av::hw::on_gpu(f) {
        read_barcode(&vidcut_av::hw::download(f).unwrap()).unwrap()
    } else {
        read_barcode(f).unwrap()
    }
}

/// Up to `n` frames on from where `reader` is: timestamps and barcodes.
fn read_on(reader: &mut VideoReader, n: usize) -> Vec<(i64, Option<u32>)> {
    let mut out = Vec::new();
    while out.len() < n {
        let Some(f) = reader.next_frame().unwrap() else {
            break;
        };
        out.push((f.pts().unwrap_or(i64::MIN), barcode(&f)));
    }
    out
}

fn main() {
    println!("FFmpeg {}", vidcut_av::ffmpeg_version());
    for path in std::env::args().skip(1) {
        println!("{path}");
        check(&Input::File(path.into()));
    }
}

/// Every seek into `input` on the CPU and on each GPU decoder.
fn check(input: &Input) {
    // The CPU's frames, read through: what every seek should show.
    let truth = read_on(&mut VideoReader::open_input(input).unwrap(), usize::MAX);
    let mut decoders: Vec<(String, Option<GpuDecoder>)> = vec![("CPU".into(), None)];
    for gpu in GpuDecoder::all() {
        decoders.push((gpu.to_string(), Some(gpu)));
    }
    for (name, gpu) in &decoders {
        let open = || match gpu {
            None => VideoReader::open_input(input),
            Some(g) => VideoReader::open_on(input, g),
        };
        let mut reader = match open() {
            Ok(r) => r,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let through = read_on(&mut reader, usize::MAX);
        let mut wrong = Vec::new();
        for (k, (pts, _)) in truth.iter().enumerate() {
            reader.seek(*pts).unwrap();
            let got = read_on(&mut reader, 3);
            let want = &truth[k..(k + 3).min(truth.len())];
            if got != want {
                wrong.push((k, got));
            }
        }
        println!(
            "{name}: read through {}, {} of {} seeks wrong",
            if through == truth { "right" } else { "WRONG" },
            wrong.len(),
            truth.len()
        );
        for (k, got) in wrong.iter().take(8) {
            let codes: Vec<String> = got
                .iter()
                .map(|(pts, code)| match code {
                    Some(c) => format!("{c}@{pts}"),
                    None => format!("?@{pts}"),
                })
                .collect();
            println!("  seek to frame {k}: {}", codes.join(" "));
        }
    }
}
