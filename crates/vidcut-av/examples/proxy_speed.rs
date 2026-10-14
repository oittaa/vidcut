//! Makes a proxy of FILE with CODEC (`h264`, `av1`, `prores`, `dnxhr`),
//! decoding on the GPU in use (`cpu`: on the CPU), and prints the time it
//! took, the cores it kept busy and what made it.
//!
//! `cargo run --release -p vidcut-av --example proxy_speed -- FILE CODEC [cpu] [WIDTH]`

use std::time::Instant;

use vidcut_av::decode::Input;
use vidcut_core::preferences::ProxyCodec;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [file, codec, rest @ ..] = &args[..] else {
        eprintln!("usage: proxy_speed FILE CODEC [cpu] [WIDTH]");
        std::process::exit(2);
    };
    let codec = match codec.as_str() {
        "h264" => ProxyCodec::H264,
        "av1" => ProxyCodec::Av1,
        "prores" => ProxyCodec::Prores,
        "dnxhr" => ProxyCodec::Dnxhr,
        other => panic!("no proxy codec {other}"),
    };
    let on_cpu = rest.iter().any(|a| a == "cpu");
    let width: u32 = rest.iter().find_map(|a| a.parse().ok()).unwrap_or(960);
    let path = std::path::PathBuf::from(file);
    let video = vidcut_av::probe::probe(&path, None)
        .expect("probe")
        .video
        .expect("a video stream");
    let height = (width * video.height / video.width) & !1;
    let available = vidcut_av::export::encode::detect();
    let encoders = vidcut_av::proxy::encoders(codec, &available);
    let gpu = if on_cpu {
        None
    } else {
        vidcut_av::hw::GpuDecoder::best()
    };
    let dir = std::env::temp_dir().join(format!("vidcut-proxy-speed-{}", std::process::id()));
    let (clock, cpu) = (Instant::now(), vidcut_av::cpu_seconds());
    let written = vidcut_av::proxy::write(
        &Input::File(path),
        width,
        height,
        &dir.join("proxy.mkv"),
        None,
        &encoders,
        gpu.as_ref(),
    )
    .expect("proxy");
    let seconds = clock.elapsed().as_secs_f64();
    let cores = (vidcut_av::cpu_seconds() - cpu) / seconds;
    println!(
        "{} {}x{} {}: {width}x{height} {}, {} frames in {seconds:.1} s ({:.0} fps on {cores:.1} cores), decoded on {}",
        video.codec,
        video.width,
        video.height,
        video.pix_fmt,
        written.encoder.name,
        written.frames,
        written.frames as f64 / seconds,
        written.decoder.map_or("the CPU", |k| k.name()),
    );
    let _ = std::fs::remove_dir_all(&dir);
}
