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

fn luma(f: &frame::Video, x: usize, y: usize) -> f64 {
    [0.2126, 0.7152, 0.0722]
        .into_iter()
        .enumerate()
        .map(|(c, w)| w * sample(f, x, y, c))
        .sum()
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    all.sort();
    all
}

fn picture(path: &Path) -> frame::Video {
    frames(path, "format=rgb48le")
        .into_iter()
        .next()
        .expect("a frame")
}

const PARTS: [&str; 3] = ["top and bottom 4 rows", "moving", "still"];

#[derive(Clone, Copy, Default)]
struct Part {
    most: f64,
    at: (usize, usize, usize, usize),
    sum: f64,
    count: u64,
    over: u64,
}

impl Part {
    fn add(&mut self, d: f64, at: (usize, usize, usize, usize)) {
        if d > self.most {
            (self.most, self.at) = (d, at);
        }
        self.sum += d;
        self.count += 1;
        self.over += u64::from(d > 2.0);
    }

    fn join(&mut self, o: &Part) {
        if o.most > self.most {
            (self.most, self.at) = (o.most, o.at);
        }
        self.sum += o.sum;
        self.count += o.count;
        self.over += o.over;
    }

    fn line(&self) -> String {
        let (i, x, y, c) = self.at;
        format!(
            "largest {:.2} at frame {i}, x {x}, y {y}, channel {c}; mean {:.4}; over 2: {} of {}",
            self.most,
            self.sum / self.count.max(1) as f64,
            self.over,
            self.count
        )
    }
}

fn moving(made: &[&frame::Video], k: usize, x: usize, y: usize) -> bool {
    let here = luma(made[k], x, y);
    [k.checked_sub(1), Some(k + 1)]
        .into_iter()
        .flatten()
        .filter_map(|j| made.get(j))
        .any(|o| (luma(o, x, y) - here).abs() > 2.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [reference, dir] = args.as_slice() else {
        eprintln!("usage: field_check SOURCE|FOLDER FRAMES_DIR");
        std::process::exit(2);
    };
    ffmpeg::init().expect("ffmpeg");
    let reference = Path::new(reference);
    let reference: Vec<frame::Video> = if reference.is_dir() {
        files(reference).iter().map(|f| picture(f)).collect()
    } else {
        frames(reference, REFERENCE)
    };
    let files = files(Path::new(dir));
    println!(
        "reference frames: {}, exported files: {}",
        reference.len(),
        files.len()
    );
    if let (Some(a), Some(b)) = (files.first(), files.last()) {
        println!("first file: {}, last file: {}", a.display(), b.display());
    }
    let made: Vec<frame::Video> = files.iter().map(|f| picture(f)).collect();
    let made: Vec<&frame::Video> = made.iter().collect();
    let mut all = [Part::default(); 3];
    for (i, r) in reference.iter().enumerate().take(made.len()) {
        let m = made[i];
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
        let h = r.height() as usize;
        let mut parts = [Part::default(); 3];
        for y in 0..h {
            for x in 0..r.width() as usize {
                let part = if y < 4 || y + 4 >= h {
                    0
                } else if moving(&made, i, x, y) {
                    1
                } else {
                    2
                };
                for c in 0..3 {
                    let d = (sample(r, x, y, c) - sample(m, x, y, c)).abs();
                    parts[part].add(d, (i, x, y, c));
                }
            }
        }
        let mut frame = Part::default();
        for (a, p) in all.iter_mut().zip(&parts) {
            a.join(p);
            frame.join(p);
        }
        let (_, x, y, c) = frame.at;
        let mean = |p: &Part| p.sum / p.count.max(1) as f64;
        println!(
            "frame {i}: largest {:.2} at x {x}, y {y}, channel {c}; mean {:.4}; rows {:.2}/{:.4}, moving {:.2}/{:.4}, still {:.2}/{:.4}",
            frame.most,
            mean(&frame),
            parts[0].most,
            mean(&parts[0]),
            parts[1].most,
            mean(&parts[1]),
            parts[2].most,
            mean(&parts[2]),
        );
    }
    println!("8-bit code values; channel 0 is red; largest/mean for each part");
    let mut total = Part::default();
    for (name, p) in PARTS.iter().zip(&all) {
        println!("{name}: {}", p.line());
        total.join(p);
    }
    println!("all: {}", total.line());
}
