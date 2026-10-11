//! Generates (or finds cached) a barcode test fixture and prints its path.
//!
//! ```text
//! cargo run -p vidcut-av --example make_fixture -- CODEC CONTAINER WIDTHxHEIGHT FPS FRAMES [no-audio] [tc=HH:MM:SS:FF]
//! cargo run -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
//! cargo run -p vidcut-av --example make_fixture -- h264 mov 320x180 25 50 tc=10:00:00:00
//! ```
//! CODEC: h264 hevc10 prores vp9 ffv1; CONTAINER: mp4 mov mkv ts webm; FPS like 30 or 30000/1001.

use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage =
        "usage: make_fixture CODEC CONTAINER WIDTHxHEIGHT FPS FRAMES [no-audio] [tc=HH:MM:SS:FF]";
    let [codec, container, size, fps, frames, rest @ ..] = args.as_slice() else {
        panic!("{usage}")
    };
    let codec = match codec.as_str() {
        "h264" => VideoCodec::H264,
        "hevc10" => VideoCodec::Hevc10,
        "prores" => VideoCodec::ProRes422Hq,
        "vp9" => VideoCodec::Vp9,
        "ffv1" => VideoCodec::Ffv1,
        other => panic!("unknown codec {other}; {usage}"),
    };
    let container = match container.as_str() {
        "mp4" => Container::Mp4,
        "mov" => Container::Mov,
        "mkv" => Container::Mkv,
        "ts" => Container::MpegTs,
        "webm" => Container::Webm,
        other => panic!("unknown container {other}; {usage}"),
    };
    let (w, h) = size.split_once('x').expect(usage);
    let (num, den) = fps.split_once('/').unwrap_or((fps, "1"));
    let mut spec = FixtureSpec::new(
        codec,
        container,
        w.parse().expect(usage),
        h.parse().expect(usage),
        (num.parse().expect(usage), den.parse().expect(usage)),
        frames.parse().expect(usage),
    );
    if rest.iter().any(|a| a == "no-audio") {
        spec = spec.no_audio();
    }
    if let Some(tc) = rest.iter().find_map(|a| a.strip_prefix("tc=")) {
        spec = spec.timecode(tc);
    }
    let started = std::time::Instant::now();
    let path = fixture::ensure(&spec).expect("generating fixture");
    eprintln!("ready in {:.1} s", started.elapsed().as_secs_f64());
    println!("{}", path.display());
}
