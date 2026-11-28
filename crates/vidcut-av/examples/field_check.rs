use std::path::{Path, PathBuf};

use vidcut_av::ffmpeg::{self, codec, filter, frame, media};

const REFERENCE: &str = "bwdif=mode=send_field:parity=auto:deint=all,scale=flags=bicubic+accurate_rnd+full_chroma_int:in_color_matrix=bt709:in_range=limited:out_range=full,format=gbrpf32le";

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

struct Picture {
    width: usize,
    height: usize,
    rgb: Vec<[f32; 3]>,
    luma: Vec<f32>,
}

impl Picture {
    fn of(f: &frame::Video) -> Self {
        let (width, height) = (f.width() as usize, f.height() as usize);
        let planes: Vec<Vec<f32>> = [2, 0, 1]
            .into_iter()
            .map(|p| {
                let (stride, data) = (f.stride(p), f.data(p));
                (0..height)
                    .flat_map(|y| data[y * stride..y * stride + width * 4].as_chunks::<4>().0)
                    .map(|b| f32::from_le_bytes(*b) * 255.0)
                    .collect()
            })
            .collect();
        let rgb: Vec<[f32; 3]> = (0..width * height)
            .map(|i| [planes[0][i], planes[1][i], planes[2][i]])
            .collect();
        let luma = rgb
            .iter()
            .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
            .collect();
        Picture {
            width,
            height,
            rgb,
            luma,
        }
    }
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

fn picture(path: &Path) -> Picture {
    Picture::of(frames(path, "format=gbrpf32le").first().expect("a frame"))
}

enum Reference {
    Made(Vec<frame::Video>),
    Files(Vec<PathBuf>),
}

impl Reference {
    fn len(&self) -> usize {
        match self {
            Reference::Made(v) => v.len(),
            Reference::Files(v) => v.len(),
        }
    }

    fn get(&self, i: usize) -> Picture {
        match self {
            Reference::Made(v) => Picture::of(&v[i]),
            Reference::Files(v) => picture(&v[i]),
        }
    }
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

    fn mean(&self) -> f64 {
        self.sum / self.count.max(1) as f64
    }

    fn line(&self) -> String {
        let (i, x, y, c) = self.at;
        format!(
            "largest {:.2} at frame {i}, x {x}, y {y}, channel {c}; mean {:.4}; over 2: {} of {}",
            self.most,
            self.mean(),
            self.over,
            self.count
        )
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [reference, dir] = args.as_slice() else {
        eprintln!("usage: field_check SOURCE|FOLDER FRAMES_DIR");
        std::process::exit(2);
    };
    ffmpeg::init().expect("ffmpeg");
    let reference = Path::new(reference);
    let reference = if reference.is_dir() {
        Reference::Files(files(reference))
    } else {
        Reference::Made(frames(reference, REFERENCE))
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
    let read = |i: usize| files.get(i).map(|f| picture(f));
    let mut window = [None, read(0), read(1)];
    let mut all = [Part::default(); 3];
    let mut span = [(f32::MAX, f32::MIN); 2];
    for i in 0..reference.len().min(files.len()) {
        let r = reference.get(i);
        let m = window[1].as_ref().expect("a picture");
        if (m.width, m.height) != (r.width, r.height) {
            println!(
                "frame {i}: size {}x{} against {}x{}",
                m.width, m.height, r.width, r.height
            );
        } else {
            for (k, p) in [&r, m].into_iter().enumerate() {
                for v in p.rgb.iter().flatten() {
                    span[k] = (span[k].0.min(*v), span[k].1.max(*v));
                }
            }
            let (w, h) = (r.width, r.height);
            let near = [window[0].as_ref(), window[2].as_ref()];
            let mut parts = [Part::default(); 3];
            for at in 0..w * h {
                let (x, y) = (at % w, at / w);
                let part = if y < 4 || y + 4 >= h {
                    0
                } else if near
                    .iter()
                    .flatten()
                    .any(|o| (o.luma[at] - m.luma[at]).abs() > 2.0)
                {
                    1
                } else {
                    2
                };
                for c in 0..3 {
                    let d = f64::from((r.rgb[at][c] - m.rgb[at][c]).abs());
                    parts[part].add(d, (i, x, y, c));
                }
            }
            let mut frame = Part::default();
            for (a, p) in all.iter_mut().zip(&parts) {
                a.join(p);
                frame.join(p);
            }
            let (_, x, y, c) = frame.at;
            println!(
                "frame {i}: largest {:.2} at x {x}, y {y}, channel {c}; mean {:.4}; rows {:.2}/{:.4}, moving {:.2}/{:.4}, still {:.2}/{:.4}",
                frame.most,
                frame.mean(),
                parts[0].most,
                parts[0].mean(),
                parts[1].most,
                parts[1].mean(),
                parts[2].most,
                parts[2].mean(),
            );
        }
        window.rotate_left(1);
        window[2] = read(i + 2);
    }
    println!("8-bit code values, not clipped; channel 0 is red; largest/mean for each part");
    println!(
        "values: reference {:.2} to {:.2}, {dir} {:.2} to {:.2}",
        span[0].0, span[0].1, span[1].0, span[1].1
    );
    let mut total = Part::default();
    for (name, p) in PARTS.iter().zip(&all) {
        println!("{name}: {}", p.line());
        total.join(p);
    }
    println!("all: {}", total.line());
}
