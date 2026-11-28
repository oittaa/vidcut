use std::path::{Path, PathBuf};

use vidcut_av::ffmpeg::{self, codec, filter, frame, media};

const REFERENCE: &str = "bwdif=mode=send_field:parity=auto:deint=all,scale=flags=bicubic+accurate_rnd+full_chroma_int:in_color_matrix=bt709:in_range=limited:out_range=full,format=rgb48le";

fn graph(f: &frame::Video, tb: ffmpeg::Rational, spec: &str) -> filter::Graph {
    let mut g = filter::Graph::new();
    let args = format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect=1/1",
        f.width(),
        f.height(),
        ffmpeg::ffi::AVPixelFormat::from(f.format()) as i32,
        tb.numerator(),
        tb.denominator().max(1),
    );
    g.add(&filter::find("buffer").expect("buffer"), "in", &args)
        .expect("source");
    g.add(&filter::find("buffersink").expect("buffersink"), "out", "")
        .expect("sink");
    g.output("in", 0)
        .and_then(|o| o.input("out", 0))
        .and_then(|o| o.parse(spec))
        .expect("filters");
    g.validate().expect("graph");
    g
}

fn drain(g: &mut filter::Graph, out: &mut Vec<frame::Video>) {
    let mut f = frame::Video::empty();
    while g.get("out").expect("sink").sink().frame(&mut f).is_ok() {
        out.push(f.clone());
    }
}

fn pull(
    dec: &mut ffmpeg::decoder::Video,
    g: &mut Option<filter::Graph>,
    out: &mut Vec<frame::Video>,
    (tb, spec): (ffmpeg::Rational, &str),
) {
    let mut f = frame::Video::empty();
    while dec.receive_frame(&mut f).is_ok() {
        let graph = g.get_or_insert_with(|| graph(&f, tb, spec));
        graph
            .get("in")
            .expect("source")
            .source()
            .add(&f)
            .expect("push");
        drain(graph, out);
    }
}

fn frames(path: &Path, spec: &str) -> Vec<frame::Video> {
    let mut input = ffmpeg::format::input(path).expect("open");
    let stream = input.streams().best(media::Type::Video).expect("video");
    let (index, tb) = (stream.index(), stream.time_base());
    let mut dec = codec::context::Context::from_parameters(stream.parameters())
        .and_then(|c| c.decoder().video())
        .expect("decoder");
    let mut g: Option<filter::Graph> = None;
    let mut out = Vec::new();
    for (s, packet) in input.packets() {
        if s.index() == index {
            dec.send_packet(&packet).expect("packet");
            pull(&mut dec, &mut g, &mut out, (tb, spec));
        }
    }
    dec.send_eof().expect("eof");
    pull(&mut dec, &mut g, &mut out, (tb, spec));
    if let Some(graph) = g.as_mut() {
        graph
            .get("in")
            .expect("source")
            .source()
            .flush()
            .expect("flush");
        drain(graph, &mut out);
    }
    out
}

fn sample(f: &frame::Video, x: usize, y: usize, c: usize) -> f64 {
    let at = y * f.stride(0) + (x * 3 + c) * 2;
    let d = f.data(0);
    f64::from(u16::from_le_bytes([d[at], d[at + 1]])) / 257.0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [source, dir] = args.as_slice() else {
        eprintln!("usage: field_check SOURCE FRAMES_DIR");
        std::process::exit(2);
    };
    ffmpeg::init().expect("ffmpeg");
    let reference = frames(Path::new(source), REFERENCE);
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    println!(
        "reference frames: {}, exported files: {}",
        reference.len(),
        files.len()
    );
    if let (Some(a), Some(b)) = (files.first(), files.last()) {
        println!("first file: {}, last file: {}", a.display(), b.display());
    }
    let (mut most, mut most_luma) = ((0.0, 0, 0, 0, 0), (0.0, 0, 0, 0));
    let (mut sum, mut count, mut over) = (0.0, 0u64, 0u64);
    for (i, (r, file)) in reference.iter().zip(&files).enumerate() {
        let made = frames(file, "format=rgb48le");
        let m = made.first().expect("a frame");
        if (m.width(), m.height()) != (r.width(), r.height()) {
            println!(
                "frame {i}: size {}x{} against {}x{}",
                m.width(),
                m.height(),
                r.width(),
                r.height()
            );
            continue;
        }
        let (mut frame_most, mut frame_at, mut frame_sum) = (0.0f64, (0, 0, 0), 0.0);
        for y in 0..r.height() as usize {
            for x in 0..r.width() as usize {
                let (mut lr, mut lm) = (0.0, 0.0);
                for (c, w) in [0.2126, 0.7152, 0.0722].into_iter().enumerate() {
                    let (a, b) = (sample(r, x, y, c), sample(m, x, y, c));
                    let d = (a - b).abs();
                    lr += w * a;
                    lm += w * b;
                    frame_sum += d;
                    if d > frame_most {
                        (frame_most, frame_at) = (d, (x, y, c));
                    }
                    if d > 2.0 {
                        over += 1;
                    }
                    if d > most.0 {
                        most = (d, i, x, y, c);
                    }
                }
                let dl = (lr - lm).abs();
                if dl > most_luma.0 {
                    most_luma = (dl, i, x, y);
                }
            }
        }
        let n = u64::from(r.width()) * u64::from(r.height()) * 3;
        sum += frame_sum;
        count += n;
        println!(
            "frame {i}: largest {frame_most:.2} at x {}, y {}, channel {}; mean {:.4}",
            frame_at.0,
            frame_at.1,
            frame_at.2,
            frame_sum / n as f64
        );
    }
    println!("8-bit code values; channel 0 is red");
    println!(
        "largest: {:.2} at frame {}, x {}, y {}, channel {}",
        most.0, most.1, most.2, most.3, most.4
    );
    println!(
        "largest in luma: {:.2} at frame {}, x {}, y {}",
        most_luma.0, most_luma.1, most_luma.2, most_luma.3
    );
    println!(
        "mean: {:.4}; samples over 2: {over} of {count}",
        sum / count.max(1) as f64
    );
}
