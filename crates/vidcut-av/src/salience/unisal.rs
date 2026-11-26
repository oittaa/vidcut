use std::path::Path;

use vidcut_core::analysis::Found;

use crate::compute::ops::{self, Act, Conv, Kernels, Ops, Res, Shape, View, tensor};
use crate::compute::{self, Gpu};
use crate::error::{Error, Result};
use crate::weights::Torch;

pub(super) const WHAT: &str = "saliency";
pub(super) const FRAMES: u32 = 5;
const EPS: f32 = 1e-5;
const STEP: u32 = 32;
const STAGES: [[u32; 4]; 7] = [
    [1, 16, 1, 1],
    [6, 24, 2, 2],
    [6, 32, 3, 2],
    [6, 64, 4, 2],
    [6, 96, 3, 1],
    [6, 160, 3, 2],
    [6, 320, 1, 1],
];
const STEM: u32 = 32;
const LAST: u32 = 1280;
const PRIORS: u32 = 16;
const PRIOR: f64 = 6.0;
const HIDDEN: u32 = 256;
const TWO: u32 = 128;
const FOUR: u32 = 64;
const TAPS: (usize, usize) = (7, 14);
const TAPPED: (u32, u32) = (64, 160);
const SMOOTH: u32 = 41;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const SPREAD: [f32; 3] = [0.229, 0.224, 0.225];
const SOURCE: &str = "DHF1K";
const CELL: &str = "rnn.cell_list.0";

const GRU: &str = r#"
@group(0) @binding(1) var<storage, read> gates: array<f32>;
@group(0) @binding(2) var<storage, read> xs: array<f32>;
@group(0) @binding(3) var<storage, read_write> h: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let c = u(1u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= u(0u)) {
        return;
    }
    let g = (i / c) * 3u * c + i % c;
    let r = gates[g];
    let z = gates[g + c];
    let fresh = tanh(clamp(xs[g + 2u * c] + r * gates[g + 2u * c], -15.0, 15.0));
    h[i] = h[i] + z * (fresh - h[i]);
}
"#;

const SMOOTHING: &str = r#"
@group(0) @binding(1) var<storage, read> wts: array<f32>;
@group(0) @binding(2) var<storage, read> src: array<f32>;
@group(0) @binding(3) var<storage, read_write> dst: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = u(0u); let sh = u(1u); let sw = u(2u); let h = u(3u); let w = u(4u); let k = u(5u);
    let i = gid(wg, nw) * 256u + li;
    if (i >= n * h * w) {
        return;
    }
    let x = i % w;
    let y = (i / w) % h;
    let b = i / (w * h);
    let half = i32(k / 2u);
    var v = 0.0;
    for (var ky = 0u; ky < k; ky++) {
        let yy = u32(clamp(i32(y + ky) - half, 0, i32(h) - 1));
        let row = (b * sh + yy * sh / h) * sw;
        for (var kx = 0u; kx < k; kx++) {
            let xx = u32(clamp(i32(x + kx) - half, 0, i32(w) - 1));
            v = fma(src[row + xx * sw / w], wts[ky * k + kx], v);
        }
    }
    dst[i] = v;
}
"#;

pub(super) fn sized(w: u32, h: u32) -> (u32, u32) {
    let aspect = f64::from(h) / f64::from(w.max(1));
    let mut best = (0.0, (8, 13));
    for rows in 7..14u32 {
        for cols in 7..14u32 {
            if !(100..=120).contains(&(rows * cols)) {
                continue;
            }
            let this = f64::from(rows) / f64::from(cols);
            let near = aspect.min(this) / aspect.max(this);
            if near > best.0 {
                best = (near, (rows, cols));
            }
        }
    }
    let (rows, cols) = best.1;
    (cols * STEP, rows * STEP)
}

pub(super) fn input(planes: &[f32], (w, h): (u32, u32)) -> Vec<f32> {
    let plane = (w * h) as usize;
    let mut out = vec![0.0; 3 * plane];
    for (p, rgb) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        for (c, v) in rgb.iter_mut().enumerate() {
            *v = (planes[(2 - c) * plane + p] / 255.0 - MEAN[c]) / SPREAD[c];
        }
    }
    out
}

pub(super) fn centroid(map: &[f32], (w, h): (u32, u32)) -> Found {
    let top = map.iter().copied().fold(f32::MIN, f32::max);
    let (mut sum, mut sx, mut sy) = (0f64, 0f64, 0f64);
    for (i, &v) in map.iter().enumerate() {
        let e = f64::from(v - top).exp();
        sum += e;
        sx += e * (f64::from(i as u32 % w) + 0.5);
        sy += e * (f64::from(i as u32 / w) + 0.5);
    }
    Found {
        center: [
            (sx / sum / f64::from(w)) as f32,
            (sy / sum / f64::from(h)) as f32,
        ],
        size: [0.0; 2],
        rotation: 0.0,
        score: top,
    }
}

struct Dw {
    w: wgpu::Buffer,
    b: wgpu::Buffer,
}

struct Block {
    expand: Option<Conv>,
    dw: Dw,
    project: Conv,
    res: bool,
    down: bool,
    hidden: u32,
    cout: u32,
}

struct Skip {
    expand: Conv,
    reduce: Conv,
    wide: u32,
}

struct Gru {
    x: [Dw; 3],
    h: [Dw; 3],
    xs: [Conv; 3],
    hs: [Conv; 3],
}

struct Net {
    stem: Conv,
    blocks: Vec<Block>,
    last: Conv,
    post: Block,
    gru: Gru,
    after: Conv,
    skip2: Skip,
    skip4: Skip,
    up2: Block,
    post2: Block,
    adapt: Conv,
    smooth: wgpu::Buffer,
    gaussians: Vec<f32>,
}

fn scaled(w: &mut [f32], b: &mut [f32], scale: &[f32], shift: &[f32]) {
    let each = w.len() / scale.len();
    for (co, (s, t)) in scale.iter().zip(shift).enumerate() {
        w[co * each..(co + 1) * each]
            .iter_mut()
            .for_each(|v| *v *= s);
        b[co] = b[co] * s + t;
    }
}

struct Read<'a> {
    t: &'a Torch,
    gpu: &'a Gpu,
}

impl Read<'_> {
    fn floats(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        self.t.floats(name, shape)
    }

    fn norm(&self, name: &str, c: u32) -> Result<(Vec<f32>, Vec<f32>)> {
        let c = c as usize;
        let read = |key: &str| self.floats(&format!("{name}.{key}"), &[c]);
        let (g, b) = (read("weight")?, read("bias")?);
        let (m, v) = (read("running_mean")?, read("running_var")?);
        let scale: Vec<f32> = g
            .iter()
            .zip(&v)
            .map(|(g, v)| g / (v + EPS).sqrt())
            .collect();
        let shift = b.iter().zip(&m).zip(&scale).map(|((b, m), s)| b - m * s);
        let shift = shift.collect();
        Ok((scale, shift))
    }

    fn conv(&self, conv: &str, norm: Option<&str>, s: Shape, bias: bool, act: Act) -> Result<Conv> {
        let (kh, kw) = s.k;
        let cout = s.cout as usize;
        let shape = [cout, s.cin as usize, kh as usize, kw as usize];
        let mut w = self.floats(&format!("{conv}.weight"), &shape)?;
        let mut b = match bias {
            true => self.floats(&format!("{conv}.bias"), &[cout])?,
            false => vec![0.0; cout],
        };
        if let Some(norm) = norm {
            let (scale, shift) = self.norm(norm, s.cout)?;
            scaled(&mut w, &mut b, &scale, &shift);
        }
        Ok(ops::conv(self.gpu, s, &w, Some(&b), act, None))
    }

    fn dw(&self, conv: &str, norm: &str, c: u32) -> Result<Dw> {
        let mut w = self.floats(&format!("{conv}.weight"), &[c as usize, 1, 3, 3])?;
        let mut b = vec![0.0; c as usize];
        let (scale, shift) = self.norm(norm, c)?;
        scaled(&mut w, &mut b, &scale, &shift);
        Ok(Dw {
            w: self.gpu.upload(&compute::bytes(&w)),
            b: self.gpu.upload(&compute::bytes(&b)),
        })
    }

    fn block(
        &self,
        at: &str,
        (cin, cout, t): (u32, u32, u32),
        domain: bool,
        (res, down): (bool, bool),
    ) -> Result<Block> {
        let hidden = cin * t;
        let conv = |i: usize| format!("{at}.conv.{i}");
        let norm = |i: usize| match domain {
            true => format!("{at}.conv.{i}.bn_{SOURCE}"),
            false => format!("{at}.conv.{i}"),
        };
        let pw =
            |i: usize, s: Shape, act: Act| self.conv(&conv(i), Some(&norm(i + 1)), s, false, act);
        let (expand, d, p) = match t {
            1 => (None, 0, 3),
            _ => {
                let s = Shape::square(cin, hidden, 1, 1, 0);
                (Some(pw(0, s, Act::Relu6)?), 3, 6)
            }
        };
        Ok(Block {
            expand,
            dw: self.dw(&conv(d), &norm(d + 1), hidden)?,
            project: pw(p, Shape::square(hidden, cout, 1, 1, 0), Act::None)?,
            res,
            down,
            hidden,
            cout,
        })
    }

    fn skip(&self, at: &str, cin: u32, cout: u32) -> Result<Skip> {
        let wide = cin * 2;
        let norm = |part: &str| format!("{at}.{part}.1.bn_{SOURCE}");
        let s = Shape::square(cin, wide, 1, 1, 0);
        let expand = self.conv(
            &format!("{at}.expansion.0"),
            Some(&norm("expansion")),
            s,
            false,
            Act::Relu6,
        )?;
        let s = Shape::square(wide, cout, 1, 1, 0);
        let reduce = self.conv(
            &format!("{at}.reduction.0"),
            Some(&norm("reduction")),
            s,
            true,
            Act::None,
        )?;
        Ok(Skip {
            expand,
            reduce,
            wide,
        })
    }

    fn gate(
        &self,
        (branch, norm, affine): (&str, &str, &str),
        bias: Option<&str>,
        act: Act,
    ) -> Result<Conv> {
        let c = HIDDEN as usize;
        let mut w = self.floats(&format!("{CELL}.{branch}.conv_sep.weight"), &[c, c, 1, 1])?;
        let mut b = vec![0.0; c];
        let (scale, shift) = self.norm(&format!("{CELL}.norm_{norm}.bn_{SOURCE}"), HIDDEN)?;
        let a = self.floats(&format!("{CELL}.a_{affine}"), &[c, 1, 1])?;
        let scale: Vec<f32> = scale.iter().zip(&a).map(|(s, a)| s * a).collect();
        let shift: Vec<f32> = shift.iter().zip(&a).map(|(t, a)| t * a).collect();
        scaled(&mut w, &mut b, &scale, &shift);
        if let Some(bias) = bias {
            let extra = self.floats(&format!("{CELL}.{bias}"), &[c, 1, 1])?;
            b.iter_mut().zip(extra).for_each(|(b, e)| *b += e);
        }
        let s = Shape::square(HIDDEN, HIDDEN, 1, 1, 0);
        Ok(ops::conv(self.gpu, s, &w, Some(&b), act, None))
    }

    fn gru(&self) -> Result<Gru> {
        let dw = |branch: &str| {
            let norm = format!("{CELL}.{branch}.sep_bn.bn_{SOURCE}");
            self.dw(&format!("{CELL}.{branch}.conv_dw"), &norm, HIDDEN)
        };
        Ok(Gru {
            x: [dw("w_r")?, dw("w_z")?, dw("w")?],
            h: [dw("u_r")?, dw("u_z")?, dw("u")?],
            xs: [
                self.gate(("w_r", "r_x", "r_x"), Some("b_r"), Act::None)?,
                self.gate(("w_z", "z_x", "z_x"), Some("b_z"), Act::None)?,
                self.gate(("w", "out_x", "h_x"), Some("b_h"), Act::None)?,
            ],
            hs: [
                self.gate(("u_r", "r_h", "r_h"), None, Act::Sigmoid)?,
                self.gate(("u_z", "z_h", "z_h"), None, Act::Sigmoid)?,
                self.gate(("u", "out_h", "h_h"), None, Act::None)?,
            ],
        })
    }

    fn net(&self) -> Result<Net> {
        let s = Shape::square(3, STEM, 3, 2, 1);
        let stem = self.conv(
            "cnn.features.0.0",
            Some("cnn.features.0.1"),
            s,
            false,
            Act::Relu6,
        )?;
        let mut blocks = Vec::new();
        let mut cin = STEM;
        for [t, c, n, s] in STAGES {
            for i in 0..n {
                let at = format!("cnn.features.{}", blocks.len() + 1);
                let res = (i > 0 || s == 1) && cin == c;
                blocks.push(self.block(&at, (cin, c, t), false, (res, i == 0 && s == 2))?);
                cin = c;
            }
        }
        let at = format!("cnn.features.{}", blocks.len() + 1);
        let s = Shape::square(cin, LAST, 1, 1, 0);
        let last = self.conv(
            &format!("{at}.0"),
            Some(&format!("{at}.1")),
            s,
            false,
            Act::Relu6,
        )?;
        let s = Shape::square(HIDDEN, HIDDEN, 1, 1, 0);
        let norm = format!("post_rnn.1.bn_{SOURCE}");
        let both = (false, false);
        Ok(Net {
            stem,
            blocks,
            last,
            post: self.block("post_cnn.inv_res", (LAST + PRIORS, HIDDEN, 1), false, both)?,
            gru: self.gru()?,
            after: self.conv("post_rnn.0", Some(&norm), s, false, Act::Relu6)?,
            skip2: self.skip("skip_2x", TAPPED.1, TWO)?,
            skip4: self.skip("skip_4x", TAPPED.0, FOUR)?,
            up2: self.block("upsampling_2.inv_res", (HIDDEN + TWO, TWO, 2), true, both)?,
            post2: self.block(
                "post_upsampling_2.inv_res",
                (TWO + FOUR, FOUR, 2),
                true,
                both,
            )?,
            adapt: self.conv(
                &format!("adaptation_{}.0", SOURCE.to_lowercase()),
                None,
                Shape::square(FOUR, 1, 1, 1, 0),
                true,
                Act::None,
            )?,
            smooth: self.gpu.upload(&compute::bytes(&self.floats(
                &format!("smoothing_{}.weight", SOURCE.to_lowercase()),
                &[1, 1, SMOOTH as usize, SMOOTH as usize],
            )?)),
            gaussians: self.floats(
                &format!("coarse_gaussians_{}", SOURCE.to_lowercase()),
                &[PRIORS as usize, 2, 2],
            )?,
        })
    }
}

fn priors(gaussians: &[f32], (fw, fh): (u32, u32)) -> Vec<f32> {
    let grid = |n: u32, i: u32| match n > 1 {
        true => f64::from(i) / f64::from(n - 1),
        false => 0.0,
    };
    let along = |v: f64, mu: f32, spread: f32| {
        let d = (v - f64::from(mu)) / f64::from(spread).exp();
        (-d * d / 2.0).exp()
    };
    let wide = (LAST + PRIORS) as usize;
    let mut out = vec![0f32; (FRAMES * fh * fw) as usize * wide];
    for b in 0..FRAMES {
        for y in 0..fh {
            for x in 0..fw {
                let at = ((b * fh + y) * fw + x) as usize * wide + LAST as usize;
                for (k, g) in gaussians.as_chunks::<4>().0.iter().enumerate() {
                    let v = along(grid(fh, y), g[0], g[1]) * along(grid(fw, x), g[2], g[3]);
                    out[at + k] = (v * PRIOR) as f32;
                }
            }
        }
    }
    out
}

struct Work {
    input: View,
    p: View,
    q: View,
    e: View,
    d: View,
    feat: View,
    im: View,
    rf: View,
    tx: View,
    th: View,
    xs: View,
    gates: View,
    state: View,
    cat2: View,
    u2: View,
    cat4: View,
    p4: View,
    adapt: View,
    map: View,
}

fn work(gpu: &Gpu, net: &Net, (w, h): (u32, u32)) -> Work {
    let px = |div: u32| (h / div) * (w / div);
    let (mut io, mut hidden, mut div) = (STEM * px(2), 0, 2);
    for b in &net.blocks {
        hidden = hidden.max(b.hidden * px(div));
        io = io.max(b.cout * px(div));
        if b.down {
            div *= 2;
        }
    }
    let hidden = [
        hidden,
        net.post.hidden * px(32),
        net.up2.hidden * px(16),
        net.post2.hidden * px(8),
        net.skip2.wide * px(16),
        net.skip4.wide * px(8),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    let flat = |size: u32| tensor(gpu, FRAMES, 1, 1, size);
    let at = |div: u32, c: u32| tensor(gpu, FRAMES, h / div, w / div, c);
    let (fw, fh) = (w / 32, h / 32);
    let wide = LAST + PRIORS;
    let feat = View {
        buf: gpu.upload(&compute::bytes(&priors(&net.gaussians, (fw, fh)))),
        base: 0,
        n: FRAMES,
        h: fh,
        w: fw,
        c: wide,
        stride: wide,
        at: 0,
    };
    Work {
        input: at(1, 3),
        p: flat(io),
        q: flat(io),
        e: flat(hidden),
        d: flat(hidden),
        feat,
        im: at(32, HIDDEN),
        rf: at(32, HIDDEN),
        tx: at(32, 3 * HIDDEN),
        th: at(32, 3 * HIDDEN),
        xs: at(32, 3 * HIDDEN),
        gates: at(32, 3 * HIDDEN),
        state: at(32, HIDDEN),
        cat2: at(16, HIDDEN + TWO),
        u2: at(16, TWO),
        cat4: at(8, TWO + FOUR),
        p4: at(8, FOUR),
        adapt: at(8, 1),
        map: at(1, 1),
    }
}

pub(super) struct Model {
    gpu: Gpu,
    kernels: Kernels,
    net: Net,
    work: Work,
    size: (u32, u32),
    gru: wgpu::ComputePipeline,
    smooth: wgpu::ComputePipeline,
}

impl Model {
    pub(super) fn load(path: &Path, title: &'static str, size: (u32, u32)) -> Result<Self> {
        let t = Torch::open(path, title)?;
        let gpu = Gpu::start(WHAT, ops::SHARED).map_err(Error::Invalid)?;
        let made = compute::checked(&gpu.device, WHAT, || {
            let net = Read { t: &t, gpu: &gpu }.net()?;
            let work = work(&gpu, &net, size);
            let gru = gpu.pipe("unisal-gru", || format!("{}{GRU}", ops::HEADER));
            let smooth = gpu.pipe("unisal-smooth", || format!("{}{SMOOTHING}", ops::HEADER));
            Ok((Kernels::new(&gpu), net, work, gru, smooth))
        })?;
        let (kernels, net, work, gru, smooth) = made;
        Ok(Self {
            gpu,
            kernels,
            net,
            work,
            size,
            gru,
            smooth,
        })
    }

    pub(super) fn size(&self) -> (u32, u32) {
        self.size
    }

    fn shaped(&self, v: &View, n: u32, div: u32, c: u32) -> View {
        let (w, h) = self.size;
        v.shaped(n, h / div, w / div, c)
    }

    fn block(&self, o: &mut Ops, b: &Block, (x, y): (&View, &View), div: u32) {
        let d = self.shaped(&self.work.d, x.n, div, b.hidden);
        match &b.expand {
            Some(expand) => {
                let e = self.shaped(&self.work.e, x.n, div, b.hidden);
                o.conv(expand, x, &e, Res::None);
                o.dwconv(&b.dw.w, &b.dw.b, 3, Act::Relu6, &e, &d);
            }
            None => o.dwconv(&b.dw.w, &b.dw.b, 3, Act::Relu6, x, &d),
        }
        let res = match b.res {
            true => Res::Before(x),
            false => Res::None,
        };
        o.conv(&b.project, &d, y, res);
    }

    fn skip(&self, o: &mut Ops, s: &Skip, (x, y): (&View, &View), div: u32) {
        let e = self.shaped(&self.work.e, x.n, div, s.wide);
        o.conv(&s.expand, x, &e, Res::None);
        o.conv(&s.reduce, &e, y, Res::None);
    }

    fn gru(&self, o: &mut Ops, n: u32) {
        let (g, k) = (&self.net.gru, &self.work);
        let im = self.shaped(&k.im, n, 32, HIDDEN);
        let state = self.shaped(&k.state, n, 32, HIDDEN);
        let wide = |v: &View| self.shaped(v, n, 32, 3 * HIDDEN);
        let (tx, th, xs, gates) = (wide(&k.tx), wide(&k.th), wide(&k.xs), wide(&k.gates));
        let part = |v: &View, i: usize| v.part(i as u32 * HIDDEN, HIDDEN);
        for i in 0..3 {
            o.dwconv(&g.x[i].w, &g.x[i].b, 3, Act::Relu6, &im, &part(&tx, i));
            o.dwconv(&g.h[i].w, &g.h[i].b, 3, Act::Relu6, &state, &part(&th, i));
            o.conv(&g.xs[i], &part(&tx, i), &part(&xs, i), Res::None);
        }
        for (i, hs) in g.hs[..2].iter().enumerate() {
            let res = part(&xs, i);
            o.conv(hs, &part(&th, i), &part(&gates, i), Res::Before(&res));
        }
        o.conv(&g.hs[2], &part(&th, 2), &part(&gates, 2), Res::None);
        let count = n * im.h * im.w * HIDDEN;
        let param = o.gpu.uniform(&[count, HIDDEN]);
        let bufs = [&gates.buf, &xs.buf, &state.buf];
        o.gpu
            .dispatch(o.pass, &self.gru, param, &bufs, count.div_ceil(256));
    }

    fn forward(&self, o: &mut Ops, n: u32) {
        let (net, k) = (&self.net, &self.work);
        let input = self.shaped(&k.input, n, 1, 3);
        let (mut cur, mut other) = (&k.p, &k.q);
        let mut x = self.shaped(cur, n, 2, STEM);
        o.conv(&net.stem, &input, &x, Res::None);
        let mut div = 2;
        for (i, b) in net.blocks.iter().enumerate() {
            let y = self.shaped(other, n, div, b.cout);
            self.block(o, b, (&x, &y), div);
            (cur, other, x) = (other, cur, y);
            if i + 1 == TAPS.0 {
                let cat4 = self.shaped(&k.cat4, n, 8, TWO + FOUR);
                self.skip(o, &net.skip4, (&x, &cat4.part(TWO, FOUR)), div);
            }
            if i + 1 == TAPS.1 {
                let cat2 = self.shaped(&k.cat2, n, 16, HIDDEN + TWO);
                self.skip(o, &net.skip2, (&x, &cat2.part(HIDDEN, TWO)), div);
            }
            if b.down {
                div *= 2;
                let y = self.shaped(other, n, div, b.cout);
                o.nearest(&x, &y);
                (cur, other, x) = (other, cur, y);
            }
        }
        let feat = self.shaped(&k.feat, n, 32, LAST + PRIORS);
        o.conv(&net.last, &x, &feat.part(0, LAST), Res::None);
        let im = self.shaped(&k.im, n, 32, HIDDEN);
        self.block(o, &net.post, (&feat, &im), 32);
        self.gru(o, n);
        let rf = self.shaped(&k.rf, n, 32, HIDDEN);
        o.conv(
            &net.after,
            &self.shaped(&k.state, n, 32, HIDDEN),
            &rf,
            Res::None,
        );
        o.add(&rf, &im);
        let cat2 = self.shaped(&k.cat2, n, 16, HIDDEN + TWO);
        o.resize(&im, &cat2.part(0, HIDDEN), (0.5, 0.5), 1.0, 0);
        let u2 = self.shaped(&k.u2, n, 16, TWO);
        self.block(o, &net.up2, (&cat2, &u2), 16);
        let cat4 = self.shaped(&k.cat4, n, 8, TWO + FOUR);
        o.resize(&u2, &cat4.part(0, TWO), (0.5, 0.5), 1.0, 0);
        let p4 = self.shaped(&k.p4, n, 8, FOUR);
        self.block(o, &net.post2, (&cat4, &p4), 8);
        let adapt = self.shaped(&k.adapt, n, 8, 1);
        o.conv(&net.adapt, &p4, &adapt, Res::None);
        let (w, h) = self.size;
        let param = o.gpu.uniform(&[n, adapt.h, adapt.w, h, w, SMOOTH]);
        let bufs = [&net.smooth, &adapt.buf, &k.map.buf];
        o.gpu.dispatch(
            o.pass,
            &self.smooth,
            param,
            &bufs,
            (n * h * w).div_ceil(256),
        );
    }

    pub(super) fn maps(&self, pictures: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        let gpu = &self.gpu;
        let (w, h) = self.size;
        let plane = (w * h) as usize;
        let mut out = Vec::new();
        compute::checked(&gpu.device, WHAT, || {
            for run in pictures.chunks(FRAMES as usize) {
                for (b, picture) in run.iter().enumerate() {
                    let at = (b * plane * 3 * 4) as u64;
                    gpu.queue
                        .write_buffer(&self.work.input.buf, at, &compute::bytes(picture));
                }
                let size = (run.len() * plane * 4) as u64;
                let staging = gpu.staging(size);
                let mut enc = gpu.device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_compute_pass(&Default::default());
                    let mut o = Ops {
                        gpu,
                        k: &self.kernels,
                        pass: &mut pass,
                    };
                    self.forward(&mut o, run.len() as u32);
                }
                enc.copy_buffer_to_buffer(&self.work.map.buf, 0, &staging, 0, size);
                gpu.flush();
                gpu.queue.submit([enc.finish()]);
                let values = compute::floats(&gpu.fetch(&staging, size, None)?);
                out.extend(values.chunks(plane).map(<[f32]>::to_vec));
            }
            Ok(())
        })?;
        Ok(out)
    }
}
