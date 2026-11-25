use std::path::Path;

use vidcut_core::analysis::Found;

use super::{CLASSES, ROW, SAME, STRIDES};
use crate::compute::Gpu;
use crate::compute::ops::{self, Act, Carve, Conv, Layer, Ops, Res, Shape, View};
use crate::error::{Error, Result};
use crate::subjects::net::{self, Onnx, Runner, Seen, plane, saw, within};

const WHAT: &str = "object detection";
const SILU: Act = Act::Silu;
const WIDTHS: [u32; 5] = [32, 64, 128, 256, 512];
const DEPTHS: [usize; 4] = [1, 3, 3, 1];
const POOLS: [u32; 3] = [5, 9, 13];
const HIDDEN: u32 = 128;
const PLACE: u32 = 4;

struct Csp {
    one: Conv,
    two: Conv,
    blocks: Vec<(Conv, Layer)>,
    three: Conv,
    short: bool,
}

struct Head {
    stem: Conv,
    class: [Layer; 2],
    place: [Layer; 2],
    classes: Conv,
    boxes: Conv,
    sure: Conv,
}

struct Weights {
    stem: Conv,
    down: [Conv; 4],
    dark: [Csp; 4],
    spp: [Conv; 2],
    lateral: Conv,
    p4: Csp,
    reduce: Conv,
    p3: Csp,
    bu2: Conv,
    n3: Csp,
    bu1: Conv,
    n4: Csp,
    heads: [Head; 3],
}

struct Convs<'a> {
    onnx: &'a Onnx,
    gpu: &'a Gpu,
    at: usize,
}

impl Convs<'_> {
    fn next(&mut self, s: &Shape) -> Result<(Vec<f32>, Vec<f32>)> {
        let node = self.onnx.order().get(self.at);
        let node = node.ok_or_else(|| self.onnx.wrong("it has too few convolutions"))?;
        self.at += 1;
        self.onnx.parts(node, s)
    }

    fn conv(&mut self, s: Shape, act: Act) -> Result<Conv> {
        let (w, b) = self.next(&s)?;
        Ok(ops::conv(self.gpu, s, &w, Some(&b), act, None))
    }

    fn layer(&mut self, s: Shape, act: Act) -> Result<Layer> {
        let (w, b) = self.next(&s)?;
        Ok(ops::layer(self.gpu, s, &w, &b, act))
    }

    fn focus(&mut self, cout: u32) -> Result<Conv> {
        let (w, b) = self.next(&Shape::square(12, cout, 3, 1, 1))?;
        let mut six = vec![0f32; cout as usize * 3 * 36];
        for o in 0..cout as usize {
            for g in 0..4 {
                let (dy, dx) = (g % 2, g / 2);
                for c in 0..3 {
                    for ky in 0..3 {
                        for kx in 0..3 {
                            let to = ((o * 3 + c) * 6 + 2 * ky + dy) * 6 + 2 * kx + dx;
                            six[to] = w[((o * 12 + g * 3 + c) * 3 + ky) * 3 + kx];
                        }
                    }
                }
            }
        }
        let s = Shape::square(3, cout, 6, 2, 2);
        Ok(ops::conv(self.gpu, s, &six, Some(&b), SILU, None))
    }

    fn down(&mut self, cin: u32, cout: u32) -> Result<Conv> {
        self.conv(Shape::square(cin, cout, 3, 2, 1), SILU)
    }

    fn one(&mut self, cin: u32, cout: u32, act: Act) -> Result<Conv> {
        self.conv(Shape::square(cin, cout, 1, 1, 0), act)
    }

    fn three(&mut self, c: u32) -> Result<Layer> {
        self.layer(Shape::square(c, c, 3, 1, 1), SILU)
    }

    fn csp(&mut self, cin: u32, cout: u32, depth: usize, short: bool) -> Result<Csp> {
        let h = cout / 2;
        let one = self.one(cin, h, SILU)?;
        let two = self.one(cin, h, SILU)?;
        let mut blocks = Vec::new();
        for _ in 0..depth {
            blocks.push((self.one(h, h, SILU)?, self.three(h)?));
        }
        Ok(Csp {
            one,
            two,
            blocks,
            three: self.one(2 * h, cout, SILU)?,
            short,
        })
    }

    fn head(&mut self, cin: u32) -> Result<Head> {
        let stem = self.one(cin, HIDDEN, SILU)?;
        let class = [self.three(HIDDEN)?, self.three(HIDDEN)?];
        let classes = self.one(HIDDEN, CLASSES as u32, Act::Sigmoid)?;
        let place = [self.three(HIDDEN)?, self.three(HIDDEN)?];
        Ok(Head {
            stem,
            class,
            place,
            classes,
            boxes: self.one(HIDDEN, PLACE, Act::None)?,
            sure: self.one(HIDDEN, 1, Act::Sigmoid)?,
        })
    }
}

fn exactly<T, const N: usize>(onnx: &Onnx, v: Vec<T>) -> Result<[T; N]> {
    v.try_into().map_err(|_| onnx.wrong("it is not YOLOX-S"))
}

fn weights(gpu: &Gpu, onnx: &Onnx) -> Result<Weights> {
    let mut c = Convs { onnx, gpu, at: 0 };
    let stem = c.focus(WIDTHS[0])?;
    let mut down = Vec::new();
    let mut dark = Vec::new();
    let mut spp = Vec::new();
    for (i, &depth) in DEPTHS.iter().enumerate() {
        let (cin, cout) = (WIDTHS[i], WIDTHS[i + 1]);
        down.push(c.down(cin, cout)?);
        if i == 3 {
            spp.push(c.one(cout, cout / 2, SILU)?);
            spp.push(c.one(cout * 2, cout, SILU)?);
        }
        dark.push(c.csp(cout, cout, depth, i < 3)?);
    }
    let [c3, c4, c5] = [WIDTHS[2], WIDTHS[3], WIDTHS[4]];
    let lateral = c.one(c5, c4, SILU)?;
    let p4 = c.csp(c5, c4, 1, false)?;
    let reduce = c.one(c4, c3, SILU)?;
    let p3 = c.csp(c4, c3, 1, false)?;
    let bu2 = c.down(c3, c3)?;
    let n3 = c.csp(c4, c4, 1, false)?;
    let bu1 = c.down(c4, c4)?;
    let n4 = c.csp(c5, c5, 1, false)?;
    let heads = [c.head(c3)?, c.head(c4)?, c.head(c5)?];
    if c.at != onnx.order().len() {
        return Err(onnx.wrong("it has more convolutions than YOLOX-S"));
    }
    Ok(Weights {
        stem,
        down: exactly(onnx, down)?,
        dark: exactly(onnx, dark)?,
        spp: exactly(onnx, spp)?,
        lateral,
        p4,
        reduce,
        p3,
        bu2,
        n3,
        bu1,
        n4,
        heads,
    })
}

struct Work {
    wide: View,
    input: View,
    level: [View; 2],
    cat: View,
    part: [View; 3],
    cat8: View,
    cat16: [View; 2],
    cat32: View,
    spp: View,
    pans: [View; 3],
    head: [View; 3],
    heads: View,
    scratch: [View; 2],
}

fn plan(c: &mut Carve, s: u32, n: u32) -> Work {
    let cells: u32 = STRIDES.iter().map(|&d| (s / d as u32).pow(2)).sum();
    let tiles = |d: u32, ch: u32| ops::WINO as u32 * n * (s / d).div_ceil(4).pow(2) * ch;
    let wino = [(4, 32), (8, 64), (16, 128), (32, 256), (8, 128), (16, 128)];
    let most = wino.iter().map(|&(d, ch)| tiles(d, ch)).max().unwrap_or(0);
    let [c3, c4, c5] = [WIDTHS[2], WIDTHS[3], WIDTHS[4]];
    Work {
        wide: c.view(n, s / 2, s / 2, WIDTHS[0]),
        input: c.view(n, s, s, 3),
        level: [(); 2].map(|_| c.view(n, s / 4, s / 4, WIDTHS[1])),
        cat: c.view(n, s / 4, s / 4, WIDTHS[1]),
        part: [(); 3].map(|_| c.view(n, s / 4, s / 4, WIDTHS[1] / 2)),
        cat8: c.view(n, s / 8, s / 8, 2 * c3),
        cat16: [
            c.view(n, s / 16, s / 16, 2 * c4),
            c.view(n, s / 16, s / 16, 2 * c3),
        ],
        cat32: c.view(n, s / 32, s / 32, 2 * c4),
        spp: c.view(n, s / 32, s / 32, 2 * c5),
        pans: [
            c.view(n, s / 8, s / 8, c3),
            c.view(n, s / 16, s / 16, c4),
            c.view(n, s / 32, s / 32, c5),
        ],
        head: [(); 3].map(|_| c.view(n, s / 8, s / 8, HIDDEN)),
        heads: c.view(n, 1, cells, ROW as u32),
        scratch: [(); 2].map(|_| c.view(1, 1, 1, most)),
    }
}

pub(crate) struct Things {
    weights: Weights,
    work: Work,
    side: u32,
    batch: u32,
    classes: &'static [usize],
}

pub(super) fn load(path: &Path, classes: &'static [usize], side: u32) -> Result<Runner<Things>> {
    let onnx = Onnx::open(path)?;
    let gpu = Gpu::start(WHAT, ops::SHARED).map_err(|why| {
        Error::Invalid(format!(
            "{why}; with gpu.acceleration off, the CPU finds people and vehicles"
        ))
    })?;
    let batch = net::batch((side, side));
    Runner::new(gpu, |gpu| {
        let weights = weights(gpu, &onnx)?;
        let (_, work) = ops::arena(gpu, |c| plan(c, side, batch))?;
        Ok(Things {
            weights,
            work,
            side,
            batch,
            classes,
        })
    })
}

struct Pass<'o, 'a, 'p, 's> {
    o: &'o mut Ops<'a, 'p>,
    n: u32,
    side: u32,
    work: &'o Work,
    seen: Option<&'s mut Vec<Seen>>,
}

impl Pass<'_, '_, '_, '_> {
    fn at(&self, v: &View, d: u32, c: u32) -> View {
        within(v, 0, self.n, (self.side / d, self.side / d, c))
    }

    fn saw(&mut self, name: &str, v: &View) {
        saw(&mut self.seen, self.o.gpu, || name.into(), v);
    }

    fn conv(&mut self, name: &str, c: &Conv, from: &View, to: &View) {
        self.o.gpu.mark(|| name.into());
        self.o.conv(c, from, to, Res::None);
        self.saw(name, to);
    }

    fn layer(&mut self, l: &Layer, from: &View, to: &View, res: Res) {
        let w = self.work;
        self.o
            .layer(l, from, to, res, (&w.scratch[0], &w.scratch[1]));
    }

    fn csp(&mut self, name: &str, c: &Csp, from: &View, to: &View, d: u32) {
        let w = self.work;
        let h = c.three.cin / 2;
        let cat = self.at(&w.cat, d, 2 * h);
        let [x, y, t] = w.part.each_ref().map(|v| self.at(v, d, h));
        self.o.gpu.mark(|| format!("{name} in"));
        self.o.conv(&c.one, from, &x, Res::None);
        self.o.conv(&c.two, from, &cat.part(h, h), Res::None);
        let last = c.blocks.len().saturating_sub(1);
        let (mut now, mut spare) = (x, y);
        for (i, (a, b)) in c.blocks.iter().enumerate() {
            self.o.gpu.mark(|| format!("{name} block{i}"));
            self.o.conv(a, &now, &t, Res::None);
            let out = if i == last { cat.part(0, h) } else { spare };
            let res = if c.short { Res::After(&now) } else { Res::None };
            self.layer(b, &t, &out, res);
            (now, spare) = (out, now);
        }
        self.conv(name, &c.three, &cat, to);
    }

    fn head(&mut self, i: usize, h: &Head, from: &View, out: &View) {
        let d = STRIDES[i] as u32;
        let [stem, one, two] = self.work.head.each_ref().map(|v| self.at(v, d, HIDDEN));
        self.o.gpu.mark(|| format!("head{d} class"));
        self.o.conv(&h.stem, from, &stem, Res::None);
        self.layer(&h.class[0], &stem, &one, Res::None);
        self.layer(&h.class[1], &one, &two, Res::None);
        self.o.conv(
            &h.classes,
            &two,
            &out.part(PLACE + 1, CLASSES as u32),
            Res::None,
        );
        self.o.gpu.mark(|| format!("head{d} box"));
        self.layer(&h.place[0], &stem, &one, Res::None);
        self.layer(&h.place[1], &one, &two, Res::None);
        self.o.conv(&h.boxes, &two, &out.part(0, PLACE), Res::None);
        self.o.conv(&h.sure, &two, &out.part(PLACE, 1), Res::None);
        self.saw(&format!("head{d}"), out);
    }
}

impl Things {
    fn levels(&self) -> [(u32, u32); 3] {
        let mut base = 0;
        STRIDES.map(|d| {
            let cols = self.side / d as u32;
            let level = (base, cols);
            base += cols * cols * ROW as u32 * self.batch;
            level
        })
    }
}

impl net::Net for Things {
    fn size(&self) -> (u32, u32) {
        (self.side, self.side)
    }

    fn batch(&self) -> u32 {
        self.batch
    }

    fn wide(&self) -> &View {
        &self.work.wide
    }

    fn heads(&self) -> &View {
        &self.work.heads
    }

    fn forward(&self, o: &mut Ops, n: u32, seen: Option<&mut Vec<Seen>>) {
        let (k, w) = (&self.work, &self.weights);
        let s = self.side;
        let mut p = Pass {
            o,
            n,
            side: s,
            work: k,
            seen,
        };
        let input = k.input.shaped(n, s, s, 3);
        p.o.gpu.mark(|| "planes".into());
        for ch in 0..3 {
            let from = plane(&k.wide, ch, self.batch, n, (s, s));
            p.o.copy(&from, &input.part(ch, 1), 1.0, 0.0, Act::None);
        }
        p.saw("input", &input);
        let stem = p.at(&k.wide, 2, WIDTHS[0]);
        p.conv("stem", &w.stem, &input, &stem);
        let a = p.at(&k.level[0], 4, WIDTHS[1]);
        p.conv("dark2 down", &w.down[0], &stem, &a);
        let d2 = p.at(&k.level[1], 4, WIDTHS[1]);
        p.csp("dark2", &w.dark[0], &a, &d2, 4);
        let [c3, c4, c5] = [WIDTHS[2], WIDTHS[3], WIDTHS[4]];
        let a = p.at(&k.level[0], 8, c3);
        p.conv("dark3 down", &w.down[1], &d2, &a);
        let cat8 = p.at(&k.cat8, 8, 2 * c3);
        let d3 = cat8.part(c3, c3);
        p.csp("dark3", &w.dark[1], &a, &d3, 8);
        let a = p.at(&k.level[0], 16, c4);
        p.conv("dark4 down", &w.down[2], &d3, &a);
        let cat16 = p.at(&k.cat16[0], 16, 2 * c4);
        let d4 = cat16.part(c4, c4);
        p.csp("dark4", &w.dark[2], &a, &d4, 16);
        let a = p.at(&k.level[0], 32, c5);
        p.conv("dark5 down", &w.down[3], &d4, &a);
        let pooled = p.at(&k.level[1], 32, c4);
        p.conv("spp in", &w.spp[0], &a, &pooled);
        let spp = p.at(&k.spp, 32, 2 * c5);
        p.o.gpu.mark(|| "spp pools".into());
        p.o.copy(&pooled, &spp.part(0, c4), 1.0, 0.0, Act::None);
        for (i, size) in POOLS.into_iter().enumerate() {
            let to = spp.part((i as u32 + 1) * c4, c4);
            p.o.pool(&pooled, &to, size, 1, (size / 2) as i32);
        }
        p.conv("spp out", &w.spp[1], &spp, &a);
        let d5 = p.at(&k.level[1], 32, c5);
        p.csp("dark5", &w.dark[3], &a, &d5, 32);
        let cat32 = p.at(&k.cat32, 32, 2 * c4);
        let top = cat32.part(c4, c4);
        p.conv("lateral", &w.lateral, &d5, &top);
        p.o.nearest(&top, &cat16.part(0, c4));
        let f0 = p.at(&k.level[0], 16, c4);
        p.csp("p4", &w.p4, &cat16, &f0, 16);
        let cat16b = p.at(&k.cat16[1], 16, 2 * c3);
        let mid = cat16b.part(c3, c3);
        p.conv("reduce", &w.reduce, &f0, &mid);
        p.o.nearest(&mid, &cat8.part(0, c3));
        let pans = [
            p.at(&k.pans[0], 8, c3),
            p.at(&k.pans[1], 16, c4),
            p.at(&k.pans[2], 32, c5),
        ];
        p.csp("p3", &w.p3, &cat8, &pans[0], 8);
        p.conv("bu2", &w.bu2, &pans[0], &cat16b.part(0, c3));
        p.csp("n3", &w.n3, &cat16b, &pans[1], 16);
        p.conv("bu1", &w.bu1, &pans[1], &cat32.part(0, c4));
        p.csp("n4", &w.n4, &cat32, &pans[2], 32);
        for (i, (base, cols)) in self.levels().into_iter().enumerate() {
            let out = within(&k.heads, base, n, (cols, cols, ROW as u32));
            p.head(i, &w.heads[i], &pans[i], &out);
        }
    }

    fn found(&self, values: &[f32], b: u32) -> Vec<Found> {
        let mut found = Vec::new();
        for (i, (base, cols)) in self.levels().into_iter().enumerate() {
            let cells = (cols * cols) as usize * ROW;
            let first = base as usize + b as usize * cells;
            let level = (self.side as usize, STRIDES[i]);
            super::boxes(self.classes, &values[first..][..cells], level, &mut found);
        }
        crate::subjects::apart(found, SAME)
    }
}
