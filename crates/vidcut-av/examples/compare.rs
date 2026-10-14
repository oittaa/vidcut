//! Compares two videos picture by picture, both decoded on the CPU and made
//! 8-bit RGB at the first one's size: the mean difference in levels, each
//! channel's mean of the second minus the first, and the largest
//! difference. Writes picture `K` of each as PNG into OUTDIR, for looking.
//!
//! `cargo run --release -p vidcut-av --example compare -- A B [OUTDIR K]`

use vidcut_av::decode::VideoReader;
use vidcut_av::ffmpeg::{format, frame, software::scaling};

/// Every picture of `path` as packed RGB at `size` (its own when `None`).
fn pictures(path: &str, size: Option<(u32, u32)>) -> Vec<(u32, u32, Vec<u8>)> {
    let mut reader = VideoReader::open(std::path::Path::new(path)).unwrap();
    let mut out = Vec::new();
    let mut scaler: Option<scaling::Context> = None;
    while let Some(f) = reader.next_frame().unwrap() {
        let (w, h) = size.unwrap_or((f.width(), f.height()));
        let s = scaler.get_or_insert_with(|| {
            scaling::Context::get(
                f.format(),
                f.width(),
                f.height(),
                format::Pixel::RGB24,
                w,
                h,
                scaling::Flags::BICUBIC
                    | scaling::Flags::ACCURATE_RND
                    | scaling::Flags::FULL_CHR_H_INT,
            )
            .unwrap()
        });
        let mut rgb = frame::Video::empty();
        s.run(&f, &mut rgb).unwrap();
        let stride = rgb.stride(0);
        let mut packed = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h as usize {
            packed.extend_from_slice(&rgb.data(0)[y * stride..][..w as usize * 3]);
        }
        out.push((w, h, packed));
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [a, b, rest @ ..] = args.as_slice() else {
        panic!("compare A B [OUTDIR K]");
    };
    let first = pictures(a, None);
    let size = first.first().map(|(w, h, _)| (*w, *h));
    let second = pictures(b, size);
    let n = first.len().min(second.len());
    let (mut sum, mut count, mut largest) = (0u64, 0u64, 0u8);
    let mut bias = [0i64; 3];
    for ((_, _, x), (_, _, y)) in first.iter().zip(&second).take(n) {
        for (k, (p, q)) in x.iter().zip(y).enumerate() {
            sum += u64::from(p.abs_diff(*q));
            largest = largest.max(p.abs_diff(*q));
            bias[k % 3] += i64::from(*q) - i64::from(*p);
        }
        count += x.len() as u64;
    }
    let per = (count / 3).max(1) as f64;
    println!(
        "{n} pictures ({} and {}): mean difference {:.2} levels, B minus A R {:+.2} G {:+.2} B {:+.2}, largest {largest}",
        first.len(),
        second.len(),
        sum as f64 / count.max(1) as f64,
        bias[0] as f64 / per,
        bias[1] as f64 / per,
        bias[2] as f64 / per,
    );
    if let [dir, k] = rest {
        let k: usize = k.parse().expect("K");
        std::fs::create_dir_all(dir).unwrap();
        for (name, list) in [("a", &first), ("b", &second)] {
            let (w, h, rgb) = &list[k];
            let rgba: Vec<u8> = rgb
                .chunks(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect();
            let png = vidcut_av::export::still::png_rgba(*w, *h, &rgba).unwrap();
            std::fs::write(format!("{dir}/{name}-{k}.png"), png).unwrap();
        }
    }
}
