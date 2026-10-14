//! Makes a proxy of each barcode fixture with every encoder proxies can be
//! made with here, on every GPU, decoding on the CPU and on each GPU decoder there is.
//! Says how each proxy's frames went, the frames copied to and from the
//! GPU, and where a proxy's frames differ from the source's: frame `k` must
//! show barcode `k`, as the fixture's does, at the source frame's time.
//!
//! `cargo run --release -p vidcut-av --example proxy_check -- [--side N] FILE...`
//!
//! `--side`: the proxy's longest side (default 960, as vidcut makes them).

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::ffmpeg;
use vidcut_av::hw::GpuDecoder;
use vidcut_av::proxy::{ProxyEncoder, encoders};
use vidcut_core::preferences::ProxyCodec;

fn secs(pts: i64, tb: ffmpeg::Rational) -> f64 {
    pts as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
}

/// `(barcode, timestamp seconds)` of every frame, decoded on the CPU.
fn frames(input: &Input) -> Vec<(Option<u32>, f64)> {
    let mut reader = VideoReader::open_input(input).unwrap();
    let tb = reader.time_base();
    let mut out = Vec::new();
    while let Some(frame) = reader.next_frame().unwrap() {
        let pts = frame.pts().unwrap_or(i64::MIN);
        out.push((read_barcode(&frame).unwrap(), secs(pts, tb)));
    }
    out
}

/// The proxy size for `input`: its picture as shown, its longest side
/// `side` (or its own, when smaller), both sides even.
fn proxy_size(input: &Input, side: u32) -> (u32, u32) {
    let reader = VideoReader::open_input(input).unwrap();
    let first = reader.peek().expect("a frame");
    let (w, h) = (first.width(), first.height());
    let (w, h) = if reader.rotation() % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    };
    let long = w.max(h).min(side);
    let even = |x: u32| (x / 2 * 2).max(2);
    if w >= h {
        (even(long), even(long * h / w))
    } else {
        (even(long * w / h), even(long))
    }
}

fn main() {
    let mut side = 960;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--side" {
            side = args.next().and_then(|s| s.parse().ok()).expect("--side N");
        } else {
            files.push(arg);
        }
    }
    println!("FFmpeg {}", vidcut_av::ffmpeg_version());
    let available = vidcut_av::export::encode::detect(true);
    // Each GPU's encoders, then the CPU's.
    let mut on = vec![available.clone()];
    on.extend(vidcut_av::gpus::list().iter().map(|g| available.on(g)));
    let mut all: Vec<ProxyEncoder> = Vec::new();
    for a in &on {
        for codec in [
            ProxyCodec::Av1,
            ProxyCodec::H264,
            ProxyCodec::Prores,
            ProxyCodec::Dnxhr,
        ] {
            for e in encoders(codec, a, &[]) {
                if !all.iter().any(|x| x.name == e.name && x.device == e.device) {
                    all.push(e);
                }
            }
        }
    }
    let mut decoders: Vec<(String, Option<GpuDecoder>)> = vec![("CPU".into(), None)];
    for gpu in GpuDecoder::all() {
        decoders.push((gpu.to_string(), Some(gpu)));
    }
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-check-")
        .tempdir()
        .unwrap();
    let mut wrong = 0;
    for path in &files {
        println!("{path}");
        let input = Input::File(path.into());
        // The source's timestamps; its barcodes read only where its
        // pictures are stored upright in SDR.
        let truth: Vec<(Option<u32>, f64)> = frames(&input)
            .into_iter()
            .enumerate()
            .map(|(k, (_, t))| (Some(k as u32), t))
            .collect();
        let (w, h) = proxy_size(&input, side);
        for (name, decoder) in &decoders {
            for enc in &all {
                let dest = tmp.path().join("proxy.mkv");
                let what = format!(
                    "{name} -> {} on {}",
                    enc.name,
                    enc.device.as_deref().unwrap_or("the CPU")
                );
                let written = match vidcut_av::proxy::write(
                    &input,
                    w,
                    h,
                    &dest,
                    None,
                    std::slice::from_ref(enc),
                    decoder.as_ref(),
                ) {
                    Ok(written) => written,
                    Err(e) => {
                        println!("  {what}: no proxy: {e}");
                        continue;
                    }
                };
                let got = frames(&Input::File(dest.clone()));
                // Matroska stores milliseconds.
                let differ: Vec<usize> = (0..truth.len().max(got.len()))
                    .filter(|&n| match (truth.get(n), got.get(n)) {
                        (Some((want, want_t)), Some((code, t))) => {
                            code != want || (t - want_t).abs() > 1.5e-3
                        }
                        _ => true,
                    })
                    .collect();
                let went = written
                    .went
                    .and_then(|w| w.label())
                    .unwrap_or_else(|| "decoded on the CPU".into());
                let c = written.copies;
                println!(
                    "  {what}: {}x{}, {went}; down {} ({:.0} MB), up {} ({:.0} MB); {}",
                    written.width,
                    written.height,
                    c.down,
                    c.down_bytes as f64 / 1e6,
                    c.up,
                    c.up_bytes as f64 / 1e6,
                    match differ.first() {
                        None => format!("{} frames right", truth.len()),
                        Some(&n) => {
                            wrong += 1;
                            format!(
                                "WRONG: {} of {} frames, the first {n}: {:?} where the source has {:?}",
                                differ.len(),
                                truth.len(),
                                got.get(n),
                                truth.get(n)
                            )
                        }
                    }
                );
                let _ = std::fs::remove_file(&dest);
            }
        }
    }
    println!("{wrong} proxies wrong");
    if wrong > 0 {
        std::process::exit(1);
    }
}
