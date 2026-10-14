//! Makes a proxy of FILE with CODEC (`h264`, `av1`, `prores`, `dnxhr`),
//! decoding on the GPU in use, and prints the time it took, the cores it
//! kept busy, what made it and whether its frames stayed on the GPU.
//!
//! `cargo run --release -p vidcut-av --example proxy_speed -- FILE CODEC`

use std::time::Instant;

use vidcut_av::decode::Input;
use vidcut_core::preferences::ProxyCodec;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [file, codec] = &args[..] else {
        eprintln!("usage: proxy_speed FILE CODEC");
        std::process::exit(2);
    };
    let codec = match codec.as_str() {
        "h264" => ProxyCodec::H264,
        "av1" => ProxyCodec::Av1,
        "prores" => ProxyCodec::Prores,
        "dnxhr" => ProxyCodec::Dnxhr,
        other => panic!("no proxy codec {other}"),
    };
    let path = std::path::PathBuf::from(file);
    let video = vidcut_av::probe::probe(&path, None)
        .expect("probe")
        .video
        .expect("a video stream");
    let (width, height) = (960, (960 * video.height / video.width) & !1);
    let encoders = vidcut_av::proxy::encoders(codec, &vidcut_av::export::encode::detect());
    let gpu = vidcut_av::hw::GpuDecoder::best();
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
    let decoded = written.decoder.map_or("the CPU", |k| k.label());
    let stayed = if written.on_gpu {
        "stayed on the GPU"
    } else {
        "came down to the CPU"
    };
    println!("Source:  {} {}x{}", video.codec, video.width, video.height);
    println!("Proxy:   {}", written.encoder.label());
    println!("Decoded: on {decoded}");
    println!("Frames:  {stayed}");
    println!(
        "Speed:   {} frames in {seconds:.1} s, {:.0} frames a second",
        written.frames,
        written.frames as f64 / seconds
    );
    println!("CPU:     {cores:.1} cores busy");
    let _ = std::fs::remove_dir_all(&dir);
}
