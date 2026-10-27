use std::time::Instant;

use vidcut_av::decode::{Input, VideoReader};
use vidcut_av::gpus::Gpu;
use vidcut_av::hw::GpuDecoder;

struct Run {
    frames: usize,
    secs: f64,
    cpu: f64,
    peak: Option<u64>,
}

fn decode(reader: &mut VideoReader, frames: usize, down: bool, gpu: Option<&Gpu>) -> Run {
    let used = || gpu.and_then(vidcut_av::gpus::used_by_process);
    let (cpu, start) = (vidcut_av::cpu_seconds(), Instant::now());
    let mut peak = used();
    let mut n = 0;
    while n < frames {
        let Some(f) = reader.next_frame().unwrap() else {
            break;
        };
        if down && vidcut_av::hw::on_gpu(&f) {
            vidcut_av::hw::download(&f).unwrap();
        }
        n += 1;
        if n % 10 == 0 {
            peak = peak.max(used());
        }
    }
    Run {
        frames: n,
        secs: start.elapsed().as_secs_f64(),
        cpu: vidcut_av::cpu_seconds() - cpu,
        peak: peak.max(used()),
    }
}

fn mb(bytes: u64) -> u64 {
    bytes >> 20
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("FILE"));
    let frames: usize = args.next().map_or(300, |n| n.parse().expect("FRAMES"));
    let start: f64 = args.next().map_or(0.0, |s| s.parse().expect("START_S"));
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
        let device = gpu.as_ref().and_then(|g| g.gpu());
        let before = device.and_then(vidcut_av::gpus::used_by_process);
        let open = |input: &Input| -> vidcut_av::error::Result<VideoReader> {
            let mut reader = match gpu {
                None => VideoReader::open_input(input),
                Some(g) => VideoReader::open_on(input, g),
            }?;
            if start > 0.0 {
                let tb = reader.time_base();
                let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
                reader.seek((start / tick) as i64)?;
            }
            Ok(reader)
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
        let name = match (gpu, reader.gpu()) {
            (Some(g), Some(kind)) => name.replacen(g.kind().name(), kind.name(), 1),
            _ => name.clone(),
        };
        let run = decode(&mut reader, frames, false, device);
        let mut line = format!(
            "{name}: opened in {:.0} ms; {} frames in {:.2} s ({:.0} fps, {:.1} cores)",
            opened.as_secs_f64() * 1e3,
            run.frames,
            run.secs,
            run.frames as f64 / run.secs,
            run.cpu / run.secs,
        );
        if let (Some(before), Some(peak)) = (before, run.peak) {
            line += &format!(
                ", GPU memory {} MB over the {} MB before",
                mb(peak.saturating_sub(before)),
                mb(before)
            );
        }
        if gpu.is_some() {
            let down = decode(&mut open(&input).unwrap(), frames, true, None);
            line += &format!(
                ", copied down {:.0} fps ({:.1} cores)",
                down.frames as f64 / down.secs,
                down.cpu / down.secs
            );
        }
        let tb = reader.time_base();
        let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
        let mut seeks = Vec::new();
        for k in 1..=6 {
            let t = start + k as f64 * 2.0 + 1.0;
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
