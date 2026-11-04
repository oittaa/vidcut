use crate::model::{
    Border, CellShape, CompositeMode, Effect, EffectKind, MosaicBlur, Picture, VideoInfo,
};
use crate::render::Placed;

const LEAST_ZOOM: f64 = 1e-9;
const LEAST_BAR: f64 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub back: [f64; 8],
    pub shown: [f64; 4],
    pub edge: [f64; 4],
    pub soft: Soft,
    pub opacity: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Soft {
    pub inner: [f64; 4],
    pub reach: [f64; 2],
}

impl Soft {
    const KNEE: f64 = 0.5;
    const WIDTH: f64 = 7.0 / 8.0;

    fn of([u0, v0, u1, v1]: [f64; 4], softness: f64, [fw, fh]: [f64; 2]) -> ([f64; 4], Self) {
        let ramp = (softness.abs() * fw / 400.0).floor();
        if ramp <= 0.0 || fw <= 0.0 || fh <= 0.0 {
            return ([u0, v0, u1, v1], Self::default());
        }
        let (du, dv) = (1.0 / fw, 1.0 / fh);
        let reach = [ramp * du, ramp * dv];
        let inset = if softness > 0.0 { 0.5 } else { ramp - 0.5 };
        let inner = [
            u0 + inset * du,
            v0 + inset * dv,
            u1 - inset * du,
            v1 - inset * dv,
        ];
        if softness < 0.0 {
            return ([u0, v0, u1, v1], Self { inner, reach });
        }
        let (ou, ov) = ((ramp + 1.0) * du, (ramp + 1.0) * dv);
        let shown = [
            (u0 - ou).max(0.0),
            (v0 - ov).max(0.0),
            (u1 + ou).min(1.0),
            (v1 + ov).min(1.0),
        ];
        (shown, Self { inner, reach })
    }

    pub fn alpha(&self, [u, v]: [f64; 2]) -> f64 {
        let [ru, rv] = self.reach;
        if ru <= 0.0 || rv <= 0.0 {
            return 1.0;
        }
        let [u0, v0, u1, v1] = self.inner;
        let a = (u0 - u).max(u - u1).max(0.0) / ru;
        let b = (v0 - v).max(v - v1).max(0.0) / rv;
        let h = (Self::KNEE - (a - b).abs()).max(0.0) / Self::KNEE;
        let m = a.max(b) + h * h * Self::KNEE / 4.0;
        let t = ((1.0 - m) / Self::WIDTH).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placing {
    pub levels: Vec<Level>,
    pub mode: CompositeMode,
    pub effects: Vec<Working>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Working {
    pub work: Work,
    pub blend: f64,
    pub use_alpha: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Work {
    Blur(Blurring),
    Mosaic(Tiling),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blurring {
    pub across: [f64; 4],
    pub down: [f64; 4],
    pub border: Border,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tiling {
    pub shape: CellShape,
    pub cell: [f64; 2],
    pub step: f64,
    pub origin: [f64; 2],
    pub reach: [u32; 2],
    pub samples: u32,
    pub blur_alpha: bool,
    pub whole: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OnFrame {
    width: f64,
    size: [f64; 2],
    middle: [f64; 2],
}

impl Tiling {
    const LEAST: f64 = 1e-3;
    const MOST_REACH: f64 = 1e9;

    fn of(m: &MosaicBlur, on: &OnFrame) -> Self {
        let samples = if m.antialias { 2.0 } else { 1.0 };
        let [w, h] = on.size;
        let [mx, my] = on.middle;
        let high = (samples * MosaicBlur::CELL * (w * h).sqrt() / m.frequency()).max(Self::LEAST);
        let cell = [(high * m.stretch()).max(Self::LEAST), high];
        let step = match m.shape() {
            CellShape::Square => cell[0],
            CellShape::Hexagon => (cell[0] * 3.0_f64.sqrt() / 2.0).max(Self::LEAST),
            CellShape::Triangle => (cell[0] / 3.0_f64.sqrt()).max(Self::LEAST),
        };
        let [px, py] = m.place();
        let [dx, dy] = m.offsets();
        let origin = [
            (samples * (mx + (px - 0.5) * w) + dx * 0.5 * cell[0]).rem_euclid(2.0 * step),
            (samples * (my + (0.5 - py) * h) - dy * 0.5 * cell[1]).rem_euclid(2.0 * cell[1]),
        ];
        let reach = cell.map(|c| (m.box_size() * c / 2.0 + 1e-9).min(Self::MOST_REACH) as u32);
        Self {
            shape: m.shape(),
            cell,
            step,
            origin,
            reach,
            samples: samples as u32,
            blur_alpha: m.blurs_alpha(),
            whole: m.whole_cells,
        }
    }
}

impl Working {
    fn of(e: &Effect, on: &OnFrame) -> Option<Self> {
        if !e.on || e.mixed() <= 0.0 {
            return None;
        }
        let work = match &e.kind {
            EffectKind::GaussianBlur(b) => {
                let (across, down) = b.spreads(on.width);
                let still = across.iter().chain(&down).all(|s| *s <= 0.0);
                if still && e.use_alpha {
                    return None;
                }
                Work::Blur(Blurring {
                    across,
                    down,
                    border: b.edge(),
                })
            }
            EffectKind::MosaicBlur(m) => Work::Mosaic(Tiling::of(m, on)),
        };
        Some(Self {
            work,
            blend: e.mixed(),
            use_alpha: e.use_alpha,
        })
    }
}

pub fn flipped(p: &Picture, [x, y]: [f64; 2]) -> [f64; 2] {
    let sx = if p.flip_horizontal { -1.0 } else { 1.0 };
    let sy = if p.flip_vertical { -1.0 } else { 1.0 };
    [0.5 + sx * (x - 0.5), 0.5 + sy * (y - 0.5)]
}

pub fn anchored(p: &Picture, (w, h): (f64, f64), [fx, fy, fw, fh]: [f64; 4]) -> [f64; 2] {
    let ([px, py], [ax, ay]) = (p.place(), p.pivot());
    [fx + ax * fw + (px - 0.5) * w, fy + ay * fh + (py - 0.5) * h]
}

fn whole_pixels(part: f64, pixels: f64) -> f64 {
    (part * pixels + 1e-6).floor() / pixels.max(1.0)
}

fn level(placed: &Placed, frame: (f64, f64), fitted: [f64; 4], (pw, ph): (f64, f64)) -> Level {
    let sampled = placed.picture.sampled(placed.source as f64);
    let p = &sampled;
    let [_, _, fw, fh] = fitted;
    let [zx, zy] = p.scale().map(|z| z.max(LEAST_ZOOM));
    let [ax, ay] = p.pivot();
    let (sin, cos) = p.turn().to_radians().sin_cos();
    let sx = if p.flip_horizontal { -1.0 } else { 1.0 };
    let sy = if p.flip_vertical { -1.0 } else { 1.0 };
    let [qx, qy] = anchored(p, frame, fitted);
    let (dx, dy) = (zx * (ax - 0.5) * fw, zy * (ay - 0.5) * fh);
    let (ox, oy) = (qx - cos * dx - sin * dy, qy + sin * dx - cos * dy);
    let (k, [u0, v0]) = placed
        .zoomed()
        .map_or((1.0, Picture::MIDDLE), |b| (b.size, flipped(p, b.center)));
    let (a, b) = (k * sx * cos / (zx * fw), -k * sx * sin / (zx * fw));
    let (c, d) = (k * sy * sin / (zy * fh), k * sy * cos / (zy * fh));
    let c0 = p.crop;
    let crop = [
        whole_pixels(c0.left, pw),
        whole_pixels(c0.top, ph),
        1.0 - whole_pixels(c0.right, pw),
        1.0 - whole_pixels(c0.bottom, ph),
    ];
    let gone = p.scale().contains(&0.0) || crop[0] >= crop[2] || crop[1] >= crop[3];
    let (edge, soft) = Soft::of(crop, p.softness(), [fw, fh]);
    let affine = [
        a,
        b,
        u0 - a * ox - b * oy,
        c,
        d,
        v0 - c * ox - d * oy,
        0.0,
        0.0,
    ];
    Level {
        back: leaned(affine, [u0, v0], [k * sx, k * sy], p.pitch(), p.yaw()),
        shown: crop,
        edge,
        soft,
        opacity: if gone { 0.0 } else { p.opaque() },
    }
}

fn leaned(
    affine: [f64; 8],
    [u0, v0]: [f64; 2],
    [su, sv]: [f64; 2],
    pitch: f64,
    yaw: f64,
) -> [f64; 8] {
    if pitch == 0.0 && yaw == 0.0 {
        return affine;
    }
    let (g, h) = (-2.0 * yaw / su, 2.0 * pitch / sv);
    let i = 1.0 - g * u0 - h * v0;
    let lean = unit([
        1.0 + u0 * g,
        u0 * h,
        u0 * (i - 1.0),
        v0 * g,
        1.0 + v0 * h,
        v0 * (i - 1.0),
        g,
        h,
        i,
    ]);
    chain(&affine, &lean)
}

fn unit([a, b, e, c, d, f, g, h, i]: [f64; 9]) -> [f64; 8] {
    let s = if i.abs() < 1e-8 {
        if i.is_sign_negative() { -1e-8 } else { 1e-8 }
    } else {
        i
    };
    [a / s, b / s, e / s, c / s, d / s, f / s, g / s, h / s]
}

pub const SAME: [f64; 8] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

pub fn chain(first: &[f64; 8], next: &[f64; 8]) -> [f64; 8] {
    let [a1, b1, e1, c1, d1, f1, g1, h1] = *first;
    let [a2, b2, e2, c2, d2, f2, g2, h2] = *next;
    unit([
        a2 * a1 + b2 * c1 + e2 * g1,
        a2 * b1 + b2 * d1 + e2 * h1,
        a2 * e1 + b2 * f1 + e2,
        c2 * a1 + d2 * c1 + f2 * g1,
        c2 * b1 + d2 * d1 + f2 * h1,
        c2 * e1 + d2 * f1 + f2,
        g2 * a1 + h2 * c1 + g1,
        g2 * b1 + h2 * d1 + h1,
        g2 * e1 + h2 * f1 + 1.0,
    ])
}

fn scaled(m: &[f64; 8], (w, h): (f64, f64)) -> [f64; 8] {
    let [a, b, e, c, d, f, g, k] = *m;
    [a * w, b * w, e * w, c * h, d * h, f * h, g, k]
}

fn then(outer: &[f64; 8], inner: &[f64; 8], frame: (f64, f64)) -> [f64; 8] {
    chain(&scaled(outer, frame), inner)
}

pub fn apply(m: &[f64; 8], [x, y]: [f64; 2]) -> [f64; 2] {
    let [a, b, e, c, d, f, g, h] = *m;
    let w = g * x + h * y + 1.0;
    [(a * x + b * y + e) / w, (c * x + d * y + f) / w]
}

pub fn ahead(m: &[f64; 8], [u, v]: [f64; 2]) -> [f64; 2] {
    let [a, b, e, c, d, f, g, h] = *m;
    let (a, b, c, d) = (a - u * g, b - u * h, c - v * g, d - v * h);
    let det = a * d - b * c;
    let (u, v) = (u - e, v - f);
    [(d * u - b * v) / det, (a * v - c * u) / det]
}

pub fn lettered([px, py]: [f64; 2], zoom: f64, radians: f64, [ox, oy]: [f64; 2]) -> [f64; 8] {
    let (sin, cos) = radians.sin_cos();
    let (a, b, c, d) = (cos / zoom, -sin / zoom, sin / zoom, cos / zoom);
    [
        a,
        b,
        px - a * px - b * py - ox,
        c,
        d,
        py - c * px - d * py - oy,
        0.0,
        0.0,
    ]
}

impl Level {
    pub fn forward(&self, uv: [f64; 2]) -> [f64; 2] {
        ahead(&self.back, uv)
    }

    pub fn centered(from: (u32, u32), to: (u32, u32)) -> Self {
        let (fw, fh) = (f64::from(from.0), f64::from(from.1));
        let (tw, th) = (f64::from(to.0), f64::from(to.1));
        let by = (tw / fw).min(th / fh);
        let (w, h) = (fw * by, fh * by);
        let (x, y) = ((tw - w) / 2.0, (th - h) / 2.0);
        Self {
            back: [1.0 / w, 0.0, -x / w, 0.0, 1.0 / h, -y / h, 0.0, 0.0],
            shown: [0.0, 0.0, 1.0, 1.0],
            edge: [0.0, 0.0, 1.0, 1.0],
            soft: Soft::default(),
            opacity: 1.0,
        }
    }
}

impl Placing {
    pub fn new(placed: &[Placed], frame: (f64, f64), fitted: [f64; 4], pixels: (f64, f64)) -> Self {
        let whole = [0.0, 0.0, frame.0, frame.1];
        let mut levels: Vec<Level> = Vec::with_capacity(placed.len());
        let mut effects = Vec::new();
        let mut stretch = [1.0, 1.0];
        for (i, p) in placed.iter().enumerate().rev() {
            let [zx, zy] = p.picture.scale_at(p.source as f64);
            stretch = [stretch[0] * zx.abs(), stretch[1] * zy.abs()];
            let (at, size) = if i == 0 {
                (fitted, pixels)
            } else {
                (whole, frame)
            };
            let own = level(p, frame, at, size);
            let back = match levels.last() {
                Some(outer) => then(&outer.back, &own.back, frame),
                None => own.back,
            };
            let unzoomed = p
                .zoomed()
                .map_or(Picture::MIDDLE, |b| flipped(&p.picture, b.center));
            let on = OnFrame {
                width: frame.0 * (stretch[0] * stretch[1]).sqrt(),
                size: [at[2] * stretch[0], at[3] * stretch[1]],
                middle: ahead(&back, unzoomed),
            };
            let mut made: Vec<Working> = p
                .picture
                .effects
                .iter()
                .filter_map(|e| Working::of(e, &on))
                .collect();
            made.append(&mut effects);
            effects = made;
            if i == 0 && !levels.is_empty() && at == whole && p.picture.is_usual() {
                continue;
            }
            levels.push(Level { back, ..own });
        }
        for l in &mut levels {
            let [u0, v0, u1, v1] = l.edge;
            let quad = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|c| ahead(&l.back, c));
            if l.soft == Soft::default() && no_bars(quad, frame) {
                l.edge = [-1.0, -1.0, 2.0, 2.0];
            }
        }
        Self {
            levels,
            mode: first_mode(placed),
            effects,
        }
    }

    pub fn whole(placed: &[Placed], frame: (f64, f64)) -> Self {
        Self::new(placed, frame, [0.0, 0.0, frame.0, frame.1], frame)
    }

    pub fn inside(mut self, placed: &[Placed], around: usize) -> Self {
        if around > 0 {
            for l in self.levels.iter_mut().take(around) {
                l.opacity = 1.0;
            }
            self.mode = first_mode(&placed[..placed.len().saturating_sub(around)]);
        }
        self
    }

    pub fn inner(&self, frame: (f64, f64)) -> [f64; 8] {
        self.levels.last().map_or(SAME, |l| scaled(&l.back, frame))
    }

    pub fn at(&self, x: f64, y: f64) -> Option<([f64; 2], f64)> {
        let mut cover = 1.0;
        let mut uv = [0.0, 0.0];
        for l in &self.levels {
            let [a, b, e, c, d, f, g, h] = l.back;
            let w = g * x + h * y + 1.0;
            if !w.is_finite() || w.abs() < 1e-12 {
                return None;
            }
            let (u, v) = ((a * x + b * y + e) / w, (c * x + d * y + f) / w);
            if !u.is_finite() || !v.is_finite() {
                return None;
            }
            let (du, dv) = (
                (a - u * g).hypot(b - u * h) / w.abs(),
                (c - v * g).hypot(d - v * h) / w.abs(),
            );
            let [u0, v0, u1, v1] = l.edge;
            for edge in [(u - u0) / du, (u1 - u) / du, (v - v0) / dv, (v1 - v) / dv] {
                cover *= (edge + 0.5).clamp(0.0, 1.0);
            }
            cover *= l.soft.alpha([u, v]) * l.opacity;
            if cover <= 0.0 {
                return None;
            }
            uv = [u, v];
        }
        Some((uv, cover))
    }

    pub fn step(&self) -> [f64; 4] {
        self.levels.last().map_or([0.0; 4], |l| {
            let [a, b, _, c, d, _, _, _] = l.back;
            [a, b, c, d]
        })
    }

    pub fn share(&self, (w, h): (f64, f64)) -> f64 {
        let Some(l) = self.levels.last() else {
            return 0.0;
        };
        let [u0, v0, u1, v1] = l.edge;
        let [a, b, _, c, d, _, g, k] = l.back;
        let corners = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|p| ahead(&l.back, p));
        let quad = if g == 0.0 && k == 0.0 {
            (u1 - u0) * (v1 - v0) / (a * d - b * c).abs()
        } else {
            quad_area(corners).unwrap_or(0.0)
        };
        let boxed = self.bounds((w, h)).map_or(0.0, |[x0, y0, x1, y1]| {
            f64::from(x1 - x0) * f64::from(y1 - y0)
        });
        (quad.min(boxed) / (w * h)).clamp(0.0, 1.0)
    }

    pub fn bounds(&self, (w, h): (f64, f64)) -> Option<[u32; 4]> {
        let mut out = [0.0, 0.0, w, h];
        for l in &self.levels {
            let [u0, v0, u1, v1] = l.edge;
            let corners = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|c| ahead(&l.back, c));
            if corners
                .iter()
                .any(|c| !c[0].is_finite() || !c[1].is_finite())
            {
                continue;
            }
            let xs = corners.map(|c| c[0]);
            let ys = corners.map(|c| c[1]);
            let lo = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min) - 1.0;
            let hi = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max) + 1.0;
            out = [
                out[0].max(lo(xs)),
                out[1].max(lo(ys)),
                out[2].min(hi(xs)),
                out[3].min(hi(ys)),
            ];
        }
        let [x0, y0, x1, y1] = out.map(|v| v.clamp(0.0, w.max(h)));
        let (x0, y0) = (x0.floor() as u32, y0.floor() as u32);
        let (x1, y1) = (x1.ceil().min(w) as u32, y1.ceil().min(h) as u32);
        (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
    }
}

fn first_mode(placed: &[Placed]) -> CompositeMode {
    placed
        .iter()
        .map(|p| p.picture.mode())
        .find(|m| *m != CompositeMode::Normal)
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub enum MixPart {
    Layer(usize),
    Title(usize),
    Mix(Mix),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mix {
    pub parts: Vec<MixPart>,
    pub opacity: f64,
    pub mode: CompositeMode,
    around: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OuterMix {
    pub mix: Mix,
    pub layers: (usize, usize),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mixes {
    pub outer: Vec<OuterMix>,
    pub follows: Vec<Option<usize>>,
    pub deepest: usize,
    pub moded: bool,
    layers: Vec<usize>,
    titles: Vec<usize>,
}

struct Member<'a> {
    index: usize,
    title: bool,
    placed: &'a [Placed],
}

impl Member<'_> {
    fn around(&self, depth: usize) -> Option<&Placed> {
        let chain = match self.title {
            true => self.placed,
            false => self.placed.get(1..).unwrap_or_default(),
        };
        chain.iter().rev().nth(depth)
    }

    fn part(&self) -> MixPart {
        match self.title {
            true => MixPart::Title(self.index),
            false => MixPart::Layer(self.index),
        }
    }

    fn mixes_by_itself(&self, depth: usize) -> bool {
        let inner = self.placed.len().saturating_sub(depth + 1);
        !self.title && first_mode(&self.placed[..inner]) == CompositeMode::Normal
    }
}

fn parts_of(members: &[Member<'_>], depth: usize, frame: (f64, f64)) -> Vec<MixPart> {
    let whole = [0.0, 0.0, frame.0, frame.1];
    let mut parts = Vec::new();
    let mut rest = members;
    while let Some(first) = rest.first() {
        let Some(outer) = first.around(depth) else {
            parts.push(first.part());
            rest = &rest[1..];
            continue;
        };
        let n = rest
            .iter()
            .take_while(|m| m.around(depth) == Some(outer))
            .count();
        let (run, after) = rest.split_at(n);
        let inner = parts_of(run, depth + 1, frame);
        let opacity = level(outer, frame, whole, frame).opacity;
        let mode = outer.picture.mode();
        let normal = mode == CompositeMode::Normal;
        let mixes = n > 1
            || inner.iter().any(|p| matches!(p, MixPart::Mix(_)))
            || !(normal || first.mixes_by_itself(depth));
        if mixes && (opacity != 1.0 || !normal) {
            parts.push(MixPart::Mix(Mix {
                parts: inner,
                opacity,
                mode,
                around: depth + 1,
            }));
        } else {
            parts.extend(inner);
        }
        rest = after;
    }
    parts
}

fn layers_in(parts: &[MixPart]) -> usize {
    parts
        .iter()
        .map(|p| match p {
            MixPart::Layer(_) => 1,
            MixPart::Title(_) => 0,
            MixPart::Mix(m) => layers_in(&m.parts),
        })
        .sum()
}

impl Mixes {
    pub fn of(layers: &[&[Placed]], titles: &[(usize, &[Placed])], frame: (f64, f64)) -> Self {
        let n = layers.len();
        let plain = layers.iter().all(|p| p.len() < 2);
        if n == 0 || plain && titles.iter().all(|(a, p)| *a > n || p.is_empty()) {
            return Self::default();
        }
        let titled = |k: usize| {
            titles
                .iter()
                .enumerate()
                .filter(move |(_, (above, _))| (*above).min(n) == k)
                .map(move |(index, (above, placed))| Member {
                    index,
                    title: true,
                    placed: if *above > n { &[] } else { placed },
                })
        };
        let mut members: Vec<Member<'_>> = titled(0).collect();
        for (index, placed) in layers.iter().enumerate() {
            members.push(Member {
                index,
                title: false,
                placed,
            });
            members.extend(titled(index + 1));
        }
        let parts = parts_of(&members, 0, frame);
        if !parts.iter().any(|p| matches!(p, MixPart::Mix(_))) {
            return Self::default();
        }
        let mut out = Self {
            follows: vec![None; titles.len()],
            layers: vec![0; n],
            titles: vec![0; titles.len()],
            ..Self::default()
        };
        let (mut next, mut last) = (0, None);
        for part in parts {
            match part {
                MixPart::Layer(i) => {
                    next = i + 1;
                    last = None;
                }
                MixPart::Title(j) => out.follows[j] = last,
                MixPart::Mix(mix) => {
                    out.mark(&mix, 1);
                    let to = next + layers_in(&mix.parts);
                    out.outer.push(OuterMix {
                        mix,
                        layers: (next, to),
                    });
                    next = to;
                    last = Some(out.outer.len() - 1);
                }
            }
        }
        out
    }

    fn mark(&mut self, mix: &Mix, depth: usize) {
        self.deepest = self.deepest.max(depth);
        self.moded |= mix.mode != CompositeMode::Normal;
        for part in &mix.parts {
            match part {
                MixPart::Layer(i) => self.layers[*i] = mix.around,
                MixPart::Title(j) => self.titles[*j] = mix.around,
                MixPart::Mix(m) => self.mark(m, depth + 1),
            }
        }
    }

    pub fn around_layer(&self, i: usize) -> usize {
        self.layers.get(i).copied().unwrap_or(0)
    }

    pub fn around_title(&self, j: usize) -> usize {
        self.titles.get(j).copied().unwrap_or(0)
    }

    pub fn starts_at(&self, i: usize) -> bool {
        self.outer.iter().any(|m| m.layers.0 == i)
    }
}

fn quad_area(c: [[f64; 2]; 4]) -> Option<f64> {
    if c.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return None;
    }
    let mut s = 0.0;
    for i in 0..4 {
        let [x0, y0] = c[i];
        let [x1, y1] = c[(i + 1) % 4];
        s += x0 * y1 - x1 * y0;
    }
    Some((s * 0.5).abs())
}

pub fn fit(w: u32, h: u32, sar: (i32, i32), out_w: u32, out_h: u32) -> (u32, u32) {
    let sar = if sar.0 > 0 && sar.1 > 0 {
        f64::from(sar.0) / f64::from(sar.1)
    } else {
        1.0
    };
    let dar = f64::from(w) * sar / f64::from(h);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if dar > f64::from(out_w) / f64::from(out_h) {
        (out_w, even(f64::from(out_w) / dar).min(out_h))
    } else {
        (even(f64::from(out_h) * dar).min(out_w), out_h)
    }
}

pub fn upright_fit(
    (w, h): (u32, u32),
    sar: (i32, i32),
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> (u32, u32) {
    if rotation % 180 == 90 {
        fit(h, w, (sar.1, sar.0), out_w, out_h)
    } else {
        fit(w, h, sar, out_w, out_h)
    }
}

pub fn upright_pixels((w, h): (u32, u32), rotation: u16) -> (f64, f64) {
    let (w, h) = (f64::from(w), f64::from(h));
    if rotation % 180 == 90 { (h, w) } else { (w, h) }
}

pub fn rounded_place(
    size: (u32, u32),
    sar: (i32, i32),
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> [f64; 4] {
    let (w, h) = upright_fit(size, sar, rotation, (out_w, out_h));
    [
        f64::from((out_w - w.min(out_w)) / 2),
        f64::from((out_h - h.min(out_h)) / 2),
        f64::from(w),
        f64::from(h),
    ]
}

pub fn fitted(
    (w, h): (u32, u32),
    sar: (i32, i32),
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> [f64; 4] {
    let square = sar.0 == sar.1 || sar.0 <= 0 || sar.1 <= 0;
    if !square || rotation != 0 || w == 0 || h == 0 {
        return rounded_place((w, h), sar, rotation, (out_w, out_h));
    }
    let (pw, ph) = (f64::from(w), f64::from(h));
    let (fw, fh) = (f64::from(out_w), f64::from(out_h));
    let (sw, sh) = if pw * fh >= ph * fw {
        (fw, fw * ph / pw)
    } else {
        (fh * pw / ph, fh)
    };
    [(fw - sw) / 2.0, (fh - sh) / 2.0, sw, sh]
}

fn uncovered(quad: &[[f64; 2]; 4], [x, y]: [f64; 2]) -> f64 {
    let (mut left, mut right, mut near) = (false, false, f64::INFINITY);
    for i in 0..4 {
        let [ax, ay] = quad[i];
        let [bx, by] = quad[(i + 1) % 4];
        let (dx, dy) = (bx - ax, by - ay);
        let (px, py) = (x - ax, y - ay);
        let side = dx * py - dy * px;
        left |= side > 0.0;
        right |= side < 0.0;
        let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        near = near.min((px - t * dx).hypot(py - t * dy));
    }
    if left && right { near } else { 0.0 }
}

pub fn no_bars(quad: [[f64; 2]; 4], (w, h): (f64, f64)) -> bool {
    quad.iter().flatten().all(|v| v.is_finite())
        && [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
            .iter()
            .all(|c| uncovered(&quad, *c) < LEAST_BAR - 1e-9)
}

pub fn no_bars_at([x, y, w, h]: [f64; 4], frame: (f64, f64)) -> bool {
    no_bars([[x, y], [x + w, y], [x + w, y + h], [x, y + h]], frame)
}

pub fn fills(v: &VideoInfo, frame: (u32, u32)) -> bool {
    let sar = (v.sar[0], v.sar[1]);
    let place = fitted((v.width, v.height), sar, v.rotation, frame);
    !v.alpha && no_bars_at(place, (f64::from(frame.0), f64::from(frame.1)))
}

pub fn detail(placed: &[Placed]) -> f64 {
    placed
        .iter()
        .map(|p| {
            let near = p.picture.dynamic_zoom.map_or(1.0, |z| z.closest());
            p.picture.largest_zoom() / near
        })
        .product()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing(picture: Picture, uv: [f64; 2]) -> [f64; 2] {
        let placed = Placed {
            clip: "c1".into(),
            picture,
            run: None,
            source: 0,
            start: 0,
            ready: true,
        };
        let frame = (1920.0, 1080.0);
        let level = Placing::whole(&[placed], frame).levels[0];
        level.forward(uv).map(|v| (v * 1e6).round() / 1e6)
    }

    #[test]
    fn the_largest_zoom_key_sets_the_detail() {
        let picture = Picture {
            zoom_keys: vec![
                crate::model::Keyframe {
                    frame: 0,
                    value: 0.1,
                    in_handle: None,
                    out_handle: None,
                },
                crate::model::Keyframe {
                    frame: 30,
                    value: 0.4,
                    in_handle: None,
                    out_handle: None,
                },
            ],
            zoom_y: vec![0.1, 0.4],
            ..Picture::default()
        };
        let placed = Placed {
            clip: "c1".into(),
            picture,
            run: None,
            source: 0,
            start: 0,
            ready: true,
        };
        assert_eq!(detail(&[placed]), 0.4);
    }

    #[test]
    fn a_zoom_works_about_the_anchor() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.5, 0.5]), [480.0, 540.0]);
    }

    #[test]
    fn a_rotation_works_about_the_anchor() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            rotation: Some(90.0),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.5, 0.5]), [1440.0, 1020.0]);
    }

    #[test]
    fn the_anchor_stays_where_the_position_puts_it() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(90.0),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.75, 0.5]), [1440.0, 540.0]);
    }

    #[test]
    fn the_anchor_is_a_point_of_the_flipped_picture() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            flip_horizontal: true,
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.25, 0.5]), [1440.0, 540.0]);
    }

    #[test]
    fn a_yaw_of_one_half_lands_the_top_left_corner() {
        let p = Picture {
            yaw: Some(0.5),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.0, 0.0]), [-960.0, -540.0]);
    }

    #[test]
    fn a_pitch_of_one_half_lands_the_bottom_right_corner() {
        let p = Picture {
            pitch: Some(0.5),
            ..Picture::default()
        };
        assert_eq!(landing(p, [1.0, 1.0]), [2880.0, 1620.0]);
    }
}
