use std::sync::Arc;

use vidcut_core::model::{Hdr, Picture, Title, TitleLine, TitlePart};
use vidcut_core::render::{Placed, Run, ShownTitle};

use super::frame::TitlePicture;
use super::store::{self, TitleKey, TitlePictures};
use crate::ffmpeg::{format, frame};

pub(crate) type Key = (Title, (u32, u32), u32);

pub(crate) type Placing = (Key, Option<Hdr>, Option<Run>, bool);

const PIXEL: usize = 16;

const LARGEST: u32 = 8192;

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Move {
    by: [i32; 2],
    pivot: [f64; 2],
    turn: f64,
}

impl Move {
    const STILL: Self = Self {
        by: [0, 0],
        pivot: [0.0, 0.0],
        turn: 0.0,
    };

    fn joins(&self, other: &Self) -> bool {
        self.by == other.by
            && self.turn == other.turn
            && (self.turn == 0.0 || self.pivot == other.pivot)
    }
}

fn steady(t: &Title, size: (u32, u32)) -> Title {
    let bare = Title {
        position: None,
        rotation: None,
        upper: t.upper.clone().map(|u| TitleLine {
            position: None,
            rotation: None,
            ..u
        }),
        ..t.clone()
    };
    let cut = t.shape_past_edge(size) || bare.shape_past_edge(size);
    Title {
        position: t.position.filter(|_| cut),
        ..bare
    }
}

fn moves(
    from: &Title,
    to: &Title,
    pictures: &TitlePictures,
    run: Run,
    size: (u32, u32),
) -> Option<Move> {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let mut each = pictures
        .iter()
        .zip(from.blocks())
        .zip(to.blocks())
        .filter_map(|((p, from_block), now)| {
            let p = p.as_ref()?;
            let [x0, y0] = from_block.pivot;
            let [x, y] = now.pivot;
            let boxes = (now.part == TitlePart::Shape)
                .then(|| from.shape_pixels(from_block.pivot, (w, h)))
                .flatten()
                .zip(to.shape_pixels(now.pivot, (w, h)));
            let by = match boxes {
                Some((a, b)) => [(b[0] - a[0]) as i32, (b[1] - a[1]) as i32],
                None if to.moves() => {
                    let top = run.top(0, h, f64::from(p.height)) + f64::from(p.lift);
                    [((x - x0) * w).round() as i32, top.round() as i32 - p.top]
                }
                None => [
                    ((x * w).round() - (x0 * w).round()) as i32,
                    ((y * h).round() - (y0 * h).round()) as i32,
                ],
            };
            Some(Move {
                by,
                pivot: [x * w, y * h],
                turn: now.turn - from_block.turn,
            })
        });
    let Some(first) = each.next() else {
        return Some(Move::STILL);
    };
    each.all(|m| m.joins(&first)).then_some(first)
}

fn bounds(pictures: &TitlePictures, at: &[[i32; 2]]) -> [i32; 4] {
    let shown: Vec<_> = pictures.iter().flatten().zip(at).collect();
    let x0 = shown.iter().map(|(_, [l, _])| *l).min().unwrap_or(0);
    let y0 = shown.iter().map(|(_, [_, t])| *t).min().unwrap_or(0);
    let x1 = shown.iter().map(|(p, [l, _])| l + p.width as i32).max();
    let y1 = shown.iter().map(|(p, [_, t])| t + p.height as i32).max();
    [
        x0,
        y0,
        x1.unwrap_or(x0 + 1).max(x0 + 1),
        y1.unwrap_or(y0 + 1).max(y0 + 1),
    ]
}

pub(crate) fn laid(
    shown: &ShownTitle,
    size: (u32, u32),
    timeline: u32,
    colors: Option<Hdr>,
) -> (Placing, Move) {
    let to = shown.title.sampled(shown.frame());
    let key = (steady(&to, size), size, timeline);
    let pictures = pictures(&key);
    let [x0, y0, x1, y1] = bounds(&pictures, &places(&pictures, &key, None));
    let small = (x1 - x0) as u32 <= LARGEST && (y1 - y0) as u32 <= LARGEST;
    match moves(&key.0, &to, &pictures, shown.run, size).filter(|_| small) {
        Some(m) => ((key, colors, None, false), m),
        None => {
            let run = to.moves().then_some(shown.run);
            (((to, size, timeline), colors, run, true), Move::STILL)
        }
    }
}

pub(crate) fn placed(origin: [i32; 2], made: (u32, u32), size: (u32, u32), m: Move) -> Placed {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let (bw, bh) = (f64::from(made.0), f64::from(made.1));
    let [fx, fy, fw, fh] = vidcut_core::place::fitted(made, (1, 1), 0, size);
    let [bx, by] = origin.map(f64::from);
    let [dx, dy] = m.by.map(f64::from);
    let [px, py] = match m.turn == 0.0 {
        true => [bx + dx, by + dy],
        false => m.pivot,
    };
    let anchor = [(px - dx - bx) / bw, (py - dy - by) / bh];
    let picture = Picture {
        position: Some([
            0.5 + (px - fx - anchor[0] * fw) / w,
            0.5 + (py - fy - anchor[1] * fh) / h,
        ]),
        zoom: Some([bw / fw, bh / fh]),
        rotation: (m.turn != 0.0).then_some(m.turn),
        anchor: Some(anchor),
        ..Picture::default()
    };
    Placed {
        clip: String::new(),
        picture,
        run: None,
        source: 0,
        frame: 0,
        start: 0,
        ready: true,
        ramp: None,
        crossing: false,
        moving: None,
        clock: Default::default(),
        adjustment: false,
    }
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

pub(crate) fn frame((key, colors, run, within): &Placing) -> (frame::Video, [i32; 2]) {
    let pictures = pictures(key);
    let at = places(&pictures, key, *run);
    let [mut x0, mut y0, mut x1, mut y1] = bounds(&pictures, &at);
    if *within {
        let (fw, fh) = (key.1.0 as i32, key.1.1 as i32);
        (x0, y0) = (x0.clamp(0, fw - 1), y0.clamp(0, fh - 1));
        (x1, y1) = (x1.clamp(x0 + 1, fw), y1.clamp(y0 + 1, fh));
    }
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let shown: Vec<_> = pictures.iter().flatten().zip(at).collect();
    let mut f = frame::Video::new(format::Pixel::GBRAPF32LE, w, h);
    let mut row = vec![[0.0f32; 4]; w as usize];
    for y in 0..h as i32 {
        row.fill([0.0; 4]);
        for (p, [left, top]) in &shown {
            let (left, top) = (left - x0, top - y0);
            if y < top || y >= top + p.height as i32 {
                continue;
            }
            let from = (y - top) as usize * p.width as usize;
            for x in left.max(0)..(left + p.width as i32).min(w as i32) {
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
    (f, [x0, y0])
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
