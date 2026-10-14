//! Makes a proxy of FILE with CODEC (`h264`, `av1`, `prores`, `dnxhr`),
//! decoding on the GPU in use, and prints the time it took, the cores it
//! kept busy, what made it and whether its frames stayed on the GPU.
//! `compare` makes it on the CPU too, and saves the middle frame of each
//! as a PNG in the temp folder's `vidcut-compare`.
//!
//! `cargo run --release -p vidcut-av --example proxy_speed -- FILE CODEC [compare]`

use std::path::{Path, PathBuf};
use std::time::Instant;

use vidcut_av::decode::Input;
use vidcut_core::preferences::ProxyCodec;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (file, codec, compare) = match &args[..] {
        [file, codec] => (file, codec, false),
        [file, codec, c] if c == "compare" => (file, codec, true),
        _ => {
            eprintln!("usage: proxy_speed FILE CODEC [compare]");
            std::process::exit(2);
        }
    };
    let codec = match codec.as_str() {
        "h264" => ProxyCodec::H264,
        "av1" => ProxyCodec::Av1,
        "prores" => ProxyCodec::Prores,
        "dnxhr" => ProxyCodec::Dnxhr,
        other => panic!("no proxy codec {other}"),
    };
    let path = PathBuf::from(file);
    let video = vidcut_av::probe::probe(&path, None)
        .expect("probe")
        .video
        .expect("a video stream");
    println!("Source:  {} {}x{}", video.codec, video.width, video.height);
    let size = (960, (960 * video.height / video.width) & !1);
    let dir = std::env::temp_dir().join(format!("vidcut-proxy-speed-{}", std::process::id()));
    let gpu = vidcut_av::hw::GpuDecoder::best();
    let made = make(&path, codec, size, gpu.as_ref(), &dir.join("gpu.mkv"));
    if compare {
        println!();
        make(&path, codec, size, None, &dir.join("cpu.mkv"));
        let out = std::env::temp_dir().join("vidcut-compare");
        std::fs::create_dir_all(&out).expect("temp folder");
        let name = path
            .file_stem()
            .map_or("proxy".into(), |s| s.to_string_lossy());
        for on in ["gpu", "cpu"] {
            let png = out.join(format!("{name}-{on}.png"));
            middle_png(&dir.join(format!("{on}.mkv")), made / 2, size, &png);
            println!("\nSaved {}", png.display());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Makes the proxy at `dest`, decoding on `gpu`, and prints how: the
/// frames it wrote.
fn make(
    path: &Path,
    codec: ProxyCodec,
    (width, height): (u32, u32),
    gpu: Option<&vidcut_av::hw::GpuDecoder>,
    dest: &Path,
) -> u64 {
    let encoders = vidcut_av::proxy::encoders(codec, &vidcut_av::export::encode::detect());
    let (clock, cpu) = (Instant::now(), vidcut_av::cpu_seconds());
    let written = vidcut_av::proxy::write(
        &Input::File(path.to_path_buf()),
        width,
        height,
        dest,
        None,
        &encoders,
        gpu,
    )
    .expect("proxy");
    let seconds = clock.elapsed().as_secs_f64();
    let cores = (vidcut_av::cpu_seconds() - cpu) / seconds;
    let decoded = written.decoder.map_or("the CPU", |k| k.label());
    let stayed = if written.on_gpu {
        "stayed on the GPU"
    } else {
        "came down to the CPU"
    };
    println!("Proxy:   {}", written.encoder.label());
    println!("Decoded: on {decoded}");
    println!("Frames:  {stayed}");
    println!(
        "Speed:   {} frames in {seconds:.1} s, {:.0} frames a second",
        written.frames,
        written.frames as f64 / seconds
    );
    println!("CPU:     {cores:.1} cores busy");
    written.frames
}

/// Frame `k` of the proxy at `proxy`, decoded on the CPU, as a PNG at `png`.
fn middle_png(proxy: &Path, k: u64, (width, height): (u32, u32), png: &Path) {
    let video = vidcut_av::probe::probe(proxy, None)
        .expect("probe")
        .video
        .expect("a video stream");
    let (num, den) = video.avg_frame_rate;
    let fps = Rational::new(i64::from(num), i64::from(den)).expect("a frame rate");
    let piece = VideoPiece::Source {
        pace: Default::default(),
        media: "proxy".into(),
        path: proxy.to_path_buf(),
        origin: video.first_frame_time.unwrap_or(0.0),
        source_frame: k as i64,
        frames: 1,
        image: None,
    };
    let bytes = vidcut_av::export::still::png(&piece, fps, width, height, None).expect("the frame");
    std::fs::write(png, bytes).expect("saving the PNG");
}
