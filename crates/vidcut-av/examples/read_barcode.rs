//! Prints the barcode frame index in each image or video frame file:
//! `cargo run -p vidcut-av --example read_barcode -- [--all] FILE...`
//! (e.g. to compare `vidcut gui screenshot` with `vidcut frame render`).
//! A picture placed on black is read inside the black. `--all` reads every
//! frame of a video and prints the indices as runs: `0-299 0-149` for a
//! timeline of two clips, `?x3` for three frames without a barcode.

use std::error::Error;
use std::path::Path;
use vidcut_av::decode::VideoReader;
use vidcut_av::ffmpeg::{format::Pixel, frame, software::scaling};

/// Luma above this is picture, not the black around it: every column of a
/// barcode has a white cell in one of its bands, or the gray margin.
const PICTURE: u8 = 64;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let all = args.first().is_some_and(|a| a == "--all");
    if all {
        args.remove(0);
    }
    for path in &args {
        match read(Path::new(path), all) {
            Ok(codes) if all => println!("{path}: {}", runs(&codes)),
            Ok(codes) => match codes.first() {
                Some(Some(i)) => println!("{path}: {i}"),
                _ => println!("{path}: no barcode"),
            },
            Err(e) => println!("{path}: error: {e}"),
        }
    }
}

/// The barcodes of the file's first frame, or of every frame with `all`.
fn read(path: &Path, all: bool) -> Result<Vec<Option<u32>>, Box<dyn Error>> {
    let mut reader = VideoReader::open(path)?;
    let mut codes = Vec::new();
    while let Some(f) = reader.next_frame()? {
        codes.push(barcode(&f)?);
        if !all {
            break;
        }
    }
    Ok(codes)
}

/// The barcode of a frame of any pixel format, inside any black around it.
fn barcode(f: &frame::Video) -> Result<Option<u32>, Box<dyn Error>> {
    let (w, h) = (f.width() as usize, f.height() as usize);
    let gray;
    let luma = match f.format() {
        // 8-bit formats whose plane 0 is luma.
        Pixel::YUV420P
        | Pixel::YUV422P
        | Pixel::YUV444P
        | Pixel::YUVJ420P
        | Pixel::YUVJ422P
        | Pixel::YUVJ444P
        | Pixel::NV12
        | Pixel::GRAY8 => f,
        other => {
            let (fw, fh) = (f.width(), f.height());
            let mut scaler =
                scaling::Context::get(other, fw, fh, Pixel::GRAY8, fw, fh, scaling::Flags::POINT)?;
            let mut out = frame::Video::empty();
            scaler.run(f, &mut out)?;
            gray = out;
            &gray
        }
    };
    let (data, stride) = (luma.data(0), luma.stride(0));
    let px = |x: usize, y: usize| data[y * stride + x];
    let columns: Vec<usize> = (0..w)
        .filter(|&x| (0..h).step_by(4).any(|y| px(x, y) > PICTURE))
        .collect();
    let rows: Vec<usize> = (0..h)
        .filter(|&y| (0..w).step_by(4).any(|x| px(x, y) > PICTURE))
        .collect();
    let (Some(&left), Some(&right), Some(&top), Some(&bottom)) =
        (columns.first(), columns.last(), rows.first(), rows.last())
    else {
        return Ok(None);
    };
    // The picture is centered: a band all black (index 0's top) is still
    // in it.
    let x0 = left.min(w - 1 - right);
    let y0 = top.min(h - 1 - bottom);
    Ok(vidcut_av::barcode::read_with(
        w - 2 * x0,
        h - 2 * y0,
        |x, y| px(x0 + x, y0 + y),
    ))
}

/// The indices as runs counting up by one: `0-299 0-149 ?x3 7`.
fn runs(codes: &[Option<u32>]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < codes.len() {
        let mut j = i + 1;
        match codes[i] {
            Some(first) => {
                while j < codes.len() && codes[j] == Some(first + (j - i) as u32) {
                    j += 1;
                }
                let last = first + (j - 1 - i) as u32;
                out.push(if j - i == 1 {
                    format!("{first}")
                } else {
                    format!("{first}-{last}")
                });
            }
            None => {
                while j < codes.len() && codes[j].is_none() {
                    j += 1;
                }
                out.push(format!("?x{}", j - i));
            }
        }
        i = j;
    }
    if out.is_empty() {
        "no frames".into()
    } else {
        out.join(" ")
    }
}
