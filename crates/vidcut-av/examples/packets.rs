//! Prints a file's video packets in decode order (pts, dts, keyframe):
//! `cargo run -p vidcut-av --example packets -- FILE [FROM_S TO_S]`.

use vidcut_av::ffmpeg;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("FILE");
    let from: f64 = args.next().map_or(0.0, |s| s.parse().unwrap());
    let to: f64 = args.next().map_or(f64::MAX, |s| s.parse().unwrap());
    vidcut_av::init();
    let mut input = ffmpeg::format::input(&path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    let (index, tb) = (stream.index(), f64::from(stream.time_base()));
    for (s, p) in input.packets() {
        if s.index() != index {
            continue;
        }
        let pts = p.pts().map(|t| t as f64 * tb);
        let dts = p.dts().map(|t| t as f64 * tb);
        if pts.is_some_and(|t| t >= from && t <= to) || dts.is_some_and(|t| t >= from && t <= to) {
            println!(
                "pts {:>8.4} (frame {:>4})  dts {:>8.4}  {}",
                pts.unwrap_or(f64::NAN),
                pts.map_or(-1, |t| (t * 30.0).round() as i64),
                dts.unwrap_or(f64::NAN),
                if p.is_key() { "KEY" } else { "" }
            );
        }
    }
}
