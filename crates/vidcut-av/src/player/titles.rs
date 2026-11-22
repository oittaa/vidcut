use std::sync::Arc;

use vidcut_core::model::{Hdr, Title, TitlePart};
use vidcut_core::render::{Run, ShownTitle};

use super::frame::TitlePicture;
use super::store::{self, TitleKey, TitlePictures};
use crate::ffmpeg::{format, frame};

pub(crate) type Key = (Title, (u32, u32), u32);

pub(crate) type Placing = (Key, Option<Hdr>, Option<Run>);

const PIXEL: usize = 16;

const PLANES: [usize; 4] = [1, 2, 0, 3];

const WIDE: [[f32; 3]; 3] = [
    [0.6274, 0.3293, 0.0433],
    [0.0691, 0.9195, 0.0114],
    [0.0164, 0.0880, 0.8956],
];

const PQ_M1: f32 = 0.159_301_76;
const PQ_M2: f32 = 78.843_75;
const PQ_C1: f32 = 0.835_937_5;
const PQ_C2: f32 = 18.851_563;
const PQ_C3: f32 = 18.6875;
const HLG_A: f32 = 0.178_832_77;
const HLG_B: f32 = 0.284_668_92;
const HLG_C: f32 = 0.559_910_7;
const WHITE_NITS: f32 = 203.0;

fn pictures(key: &Key) -> TitlePictures {
    let whole = TitleKey::Title(Box::new(key.clone()));
    let global = store::global();
    if let Some(pictures) = global.title_pictures(&whole) {
        return pictures;
    }
    let pictures = drawn(key);
    global.keep_titles(whole, pictures.clone());
    pictures
}

pub(crate) fn placing(
    shown: &ShownTitle,
    size: (u32, u32),
    timeline: u32,
    colors: Option<Hdr>,
) -> Placing {
    let title = shown.title.sampled(shown.frame());
    let run = title.moves().then_some(shown.run);
    ((title, size, timeline), colors, run)
}

fn places(pictures: &TitlePictures, (t, (_, h), _): &Key, run: Option<Run>) -> Vec<[i32; 2]> {
    pictures
        .iter()
        .zip(t.blocks())
        .filter_map(|(p, b)| Some((p.as_ref()?, b.part)))
        .map(|(p, part)| match run.filter(|_| part != TitlePart::Shape) {
            Some(run) => {
                let top = run.top(0, f64::from(*h), f64::from(p.height)) + f64::from(p.lift);
                [p.left, top.round() as i32]
            }
            None => [p.left, p.top],
        })
        .collect()
}

fn encoded(c: f32, hdr: Hdr) -> f32 {
    let c = c.max(0.0);
    match hdr {
        Hdr::Pq => {
            let y = (c * WHITE_NITS / 10000.0).powf(PQ_M1);
            ((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y)).powf(PQ_M2)
        }
        Hdr::Hlg => {
            let l = (c * WHITE_NITS / 1000.0).powf(1.0 / 1.2);
            match l <= 1.0 / 12.0 {
                true => (3.0 * l).sqrt(),
                false => HLG_A * (12.0 * l - HLG_B).max(1e-30).ln() + HLG_C,
            }
        }
    }
}

fn widened(rgb: [f32; 3], hdr: Hdr) -> [f32; 3] {
    let lin = rgb.map(|s| {
        let s = s.clamp(0.0, 1.0);
        match s <= 0.04045 {
            true => s / 12.92,
            false => ((s + 0.055) / 1.055).powf(2.4),
        }
    });
    WIDE.map(|row| {
        encoded(row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2], hdr).clamp(0.0, 1.0)
    })
}

fn color(p: &TitlePicture, i: usize) -> [f32; 4] {
    match p.float {
        true => read(&p.rgba.as_chunks::<PIXEL>().0[i]),
        false => p.rgba.as_chunks::<4>().0[i].map(|v| f32::from(v) / 255.0),
    }
}

fn over(under: [f32; 4], top: [f32; 4]) -> [f32; 4] {
    let keep = 1.0 - top[3];
    std::array::from_fn(|c| top[c] + under[c] * keep)
}

fn read(cell: &[u8; PIXEL]) -> [f32; 4] {
    let (floats, _) = cell.as_chunks::<4>();
    std::array::from_fn(|c| f32::from_le_bytes(floats[c]))
}

pub(crate) fn frame((key, colors, run): &Placing) -> frame::Video {
    let pictures = pictures(key);
    let at = places(&pictures, key, *run);
    let (w, h) = key.1;
    let shown: Vec<_> = pictures.iter().flatten().zip(at).collect();
    let mut f = frame::Video::new(format::Pixel::GBRAPF32LE, w, h);
    let mut row = vec![[0.0f32; 4]; w as usize];
    for y in 0..h as i32 {
        row.fill([0.0; 4]);
        for (p, [left, top]) in &shown {
            if y < *top || y >= top + p.height as i32 {
                continue;
            }
            let from = (y - top) as usize * p.width as usize;
            for x in (*left).max(0)..(left + p.width as i32).min(w as i32) {
                let cell = &mut row[x as usize];
                *cell = over(*cell, color(p, from + (x - left) as usize));
            }
        }
        if let Some(hdr) = *colors {
            for cell in row.iter_mut().filter(|c| c[3] > 0.0) {
                let [r, g, b, a] = *cell;
                let [r, g, b] = widened([r / a, g / a, b / a], hdr);
                *cell = [r * a, g * a, b * a, a];
            }
        }
        for (plane, c) in PLANES.into_iter().enumerate() {
            let start = y as usize * f.stride(plane);
            let data = &mut f.data_mut(plane)[start..start + w as usize * 4];
            for (cell, v) in data.as_chunks_mut::<4>().0.iter_mut().zip(&row) {
                *cell = v[c].to_le_bytes();
            }
        }
    }
    crate::screen::held::tag(&mut f, *colors);
    f
}

fn drawn((t, (width, height), timeline): &Key) -> TitlePictures {
    crate::burn::title_blocks(t, *width, *height, *timeline)
        .into_iter()
        .map(|sprite| {
            sprite.map(|sprite| {
                Arc::new(TitlePicture {
                    left: sprite.x,
                    top: sprite.y,
                    width: sprite.width,
                    height: sprite.height,
                    lift: sprite.lift,
                    frame: (*width, *height),
                    float: sprite.float,
                    rgba: sprite.premultiplied(),
                })
            })
        })
        .collect()
}
