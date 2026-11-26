use std::sync::Mutex;

use super::{
    Block, FOUR, FRAMES, Gru, HIDDEN, LAST, Make, Net, PRIORS, Read, SMOOTH, Skip, TAPS, TWO,
    priors,
};
use crate::compute::ops::{Act, Shape};
use crate::error::Result;
use crate::weights::Torch;

pub(super) struct Cpu;

pub(super) struct Conv {
    cin: usize,
    cout: usize,
    k: usize,
    stride: usize,
    pad: usize,
    w: Vec<f32>,
    b: Vec<f32>,
    act: Act,
}

pub(super) struct Dw {
    w: Vec<f32>,
    b: Vec<f32>,
}

impl Make for Cpu {
    type Conv = Conv;
    type Dw = Dw;
    type Taps = Vec<f32>;

    fn conv(&self, s: Shape, w: &[f32], b: &[f32], act: Act) -> Conv {
        let (k, cin, cout) = (s.k.0 as usize, s.cin as usize, s.cout as usize);
        let mut laid = vec![0.0; w.len()];
        for (from, v) in w.iter().enumerate() {
            let (kx, rest) = (from % k, from / k);
            let (ky, rest) = (rest % k, rest / k);
            let (ci, co) = (rest % cin, rest / cin);
            laid[((ky * k + kx) * cin + ci) * cout + co] = *v;
        }
        Conv {
            cin,
            cout,
            k,
            stride: s.stride as usize,
            pad: s.pad.0 as usize,
            w: laid,
            b: b.to_vec(),
            act,
        }
    }

    fn dw(&self, w: &[f32], b: &[f32]) -> Dw {
        let c = b.len();
        let mut laid = vec![0.0; w.len()];
        for (from, v) in w.iter().enumerate() {
            laid[(from % 9) * c + from / 9] = *v;
        }
        Dw {
            w: laid,
            b: b.to_vec(),
        }
    }

    fn taps(&self, w: &[f32]) -> Vec<f32> {
        w.to_vec()
    }
}

struct Tensor {
    n: usize,
    h: usize,
    w: usize,
    c: usize,
    v: Vec<f32>,
}

impl Tensor {
    fn new(n: usize, h: usize, w: usize, c: usize) -> Self {
        Self {
            n,
            h,
            w,
            c,
            v: vec![0.0; n * h * w * c],
        }
    }

    fn at(&self, b: usize, y: usize, x: usize) -> &[f32] {
        &self.v[((b * self.h + y) * self.w + x) * self.c..][..self.c]
    }
}

fn rows(out: &mut [f32], len: usize, job: &(dyn Fn(usize, &mut [f32]) + Sync)) {
    let parts: Vec<Mutex<&mut [f32]>> = out.chunks_mut(len).map(Mutex::new).collect();
    crate::pool::run(parts.len(), &|i| job(i, &mut crate::pool::lock(&parts[i])));
}

fn act(v: f32, a: Act) -> f32 {
    match a {
        Act::Relu6 => v.clamp(0.0, 6.0),
        Act::Sigmoid => 1.0 / (1.0 + (-v).exp()),
        _ => v,
    }
}

fn inside(at: usize, pad: usize, size: usize) -> Option<usize> {
    at.checked_sub(pad).filter(|&v| v < size)
}

fn conv_into(c: &Conv, x: &Tensor, res: Option<&Tensor>, (dst, at): (&mut Tensor, usize)) {
    debug_assert_eq!(x.c, c.cin);
    let (oh, ow, dc) = (dst.h, dst.w, dst.c);
    let used = x.n * oh * ow * dc;
    rows(&mut dst.v[..used], ow * dc, &|r, row| {
        let (b, oy) = (r / oh, r % oh);
        let mut acc = vec![0.0; c.cout];
        for ox in 0..ow {
            acc.copy_from_slice(&c.b);
            for ky in 0..c.k {
                let Some(iy) = inside(oy * c.stride + ky, c.pad, x.h) else {
                    continue;
                };
                for kx in 0..c.k {
                    let Some(ix) = inside(ox * c.stride + kx, c.pad, x.w) else {
                        continue;
                    };
                    let w = &c.w[(ky * c.k + kx) * c.cin * c.cout..][..c.cin * c.cout];
                    for (v, ws) in x.at(b, iy, ix).iter().zip(w.chunks_exact(c.cout)) {
                        for (a, wv) in acc.iter_mut().zip(ws) {
                            *a += v * wv;
                        }
                    }
                }
            }
            if let Some(res) = res {
                for (a, r) in acc.iter_mut().zip(res.at(b, oy, ox)) {
                    *a += r;
                }
            }
            for (o, a) in row[ox * dc + at..][..c.cout].iter_mut().zip(&acc) {
                *o = act(*a, c.act);
            }
        }
    });
}

fn conv(c: &Conv, x: &Tensor, res: Option<&Tensor>) -> Tensor {
    let side = |v: usize| (v + 2 * c.pad - c.k) / c.stride + 1;
    let mut y = Tensor::new(x.n, side(x.h), side(x.w), c.cout);
    conv_into(c, x, res, (&mut y, 0));
    y
}

fn dw(d: &Dw, x: &Tensor) -> Tensor {
    let (h, w, c) = (x.h, x.w, x.c);
    let mut y = Tensor::new(x.n, h, w, c);
    rows(&mut y.v, w * c, &|r, row| {
        let (b, oy) = (r / h, r % h);
        for ox in 0..w {
            let out = &mut row[ox * c..][..c];
            out.copy_from_slice(&d.b);
            for ky in 0..3 {
                let Some(iy) = inside(oy + ky, 1, h) else {
                    continue;
                };
                for kx in 0..3 {
                    let Some(ix) = inside(ox + kx, 1, w) else {
                        continue;
                    };
                    let taps = &d.w[(ky * 3 + kx) * c..][..c];
                    for ((o, s), t) in out.iter_mut().zip(x.at(b, iy, ix)).zip(taps) {
                        *o += s * t;
                    }
                }
            }
            out.iter_mut().for_each(|v| *v = v.clamp(0.0, 6.0));
        }
    });
    y
}

fn halved(x: &Tensor) -> Tensor {
    let (h, w, c) = (x.h / 2, x.w / 2, x.c);
    let mut y = Tensor::new(x.n, h, w, c);
    for (r, row) in y.v.chunks_mut(w * c).enumerate() {
        let (b, oy) = (r / h, r % h);
        for (ox, out) in row.chunks_mut(c).enumerate() {
            out.copy_from_slice(x.at(b, 2 * oy, 2 * ox));
        }
    }
    y
}

fn up_into(x: &Tensor, (dst, at): (&mut Tensor, usize)) {
    let (oh, ow, dc) = (dst.h, dst.w, dst.c);
    let near = |o: usize, size: usize| {
        let f = ((o as f32 + 0.5) * 0.5 - 0.5).max(0.0);
        let i0 = (f.floor() as usize).min(size - 1);
        (i0, (i0 + 1).min(size - 1), f - i0 as f32)
    };
    for (r, row) in dst.v[..x.n * oh * ow * dc].chunks_mut(ow * dc).enumerate() {
        let (b, oy) = (r / oh, r % oh);
        let (y0, y1, ly) = near(oy, x.h);
        for ox in 0..ow {
            let (x0, x1, lx) = near(ox, x.w);
            let top = x.at(b, y0, x0).iter().zip(x.at(b, y0, x1));
            let low = x.at(b, y1, x0).iter().zip(x.at(b, y1, x1));
            for (o, ((a, bb), (cc, d))) in row[ox * dc + at..][..x.c].iter_mut().zip(top.zip(low)) {
                *o = (1.0 - ly) * ((1.0 - lx) * a + lx * bb) + ly * ((1.0 - lx) * cc + lx * d);
            }
        }
    }
}

fn block(b: &Block<Cpu>, x: &Tensor) -> Tensor {
    let d = match &b.expand {
        Some(e) => dw(&b.dw, &conv(e, x, None)),
        None => dw(&b.dw, x),
    };
    conv(&b.project, &d, b.res.then_some(x))
}

fn skip_into(s: &Skip<Cpu>, x: &Tensor, dst: (&mut Tensor, usize)) {
    conv_into(&s.reduce, &conv(&s.expand, x, None), None, dst);
}

fn gru(g: &Gru<Cpu>, im: &Tensor, state: &mut Tensor) {
    let tx: Vec<Tensor> = g.x.iter().map(|d| dw(d, im)).collect();
    let th: Vec<Tensor> = g.h.iter().map(|d| dw(d, state)).collect();
    let xs: Vec<Tensor> =
        g.xs.iter()
            .zip(&tx)
            .map(|(c, t)| conv(c, t, None))
            .collect();
    let r = conv(&g.hs[0], &th[0], Some(&xs[0]));
    let z = conv(&g.hs[1], &th[1], Some(&xs[1]));
    let gh = conv(&g.hs[2], &th[2], None);
    let parts = r.v.iter().zip(&z.v).zip(gh.v.iter().zip(&xs[2].v));
    for (h, ((r, z), (gh, x))) in state.v.iter_mut().zip(parts) {
        let fresh = (x + r * gh).clamp(-15.0, 15.0).tanh();
        *h += z * (fresh - *h);
    }
}

fn smoothed(taps: &[f32], x: &Tensor, (w, h): (usize, usize)) -> Vec<f32> {
    let (k, half) = (SMOOTH as usize, SMOOTH as usize / 2);
    let (sh, sw) = (x.h, x.w);
    let cols: Vec<usize> = (0..w + k)
        .map(|j| j.saturating_sub(half).min(w - 1) * sw / w)
        .collect();
    let lines: Vec<usize> = (0..h + k)
        .map(|j| j.saturating_sub(half).min(h - 1) * sh / h)
        .collect();
    let mut out = vec![0.0; x.n * h * w];
    rows(&mut out, w, &|r, row| {
        let (b, y) = (r / h, r % h);
        for (xx, o) in row.iter_mut().enumerate() {
            let mut v = 0.0;
            for (ky, line) in lines[y..y + k].iter().enumerate() {
                let src = &x.v[(b * sh + line) * sw..][..sw];
                let ws = &taps[ky * k..][..k];
                for (c, t) in cols[xx..xx + k].iter().zip(ws) {
                    v += src[*c] * t;
                }
            }
            *o = v;
        }
    });
    out
}

pub(super) struct Model {
    net: Net<Cpu>,
    size: (usize, usize),
    feat: Vec<f32>,
    state: Tensor,
}

impl Model {
    pub(super) fn load(t: &Torch, (w, h): (u32, u32)) -> Result<Self> {
        let net = Read { t, make: &Cpu }.net()?;
        let feat = priors(&net.gaussians, (w / 32, h / 32));
        let (w, h) = (w as usize, h as usize);
        let state = Tensor::new(FRAMES as usize, h / 32, w / 32, HIDDEN as usize);
        Ok(Self {
            net,
            size: (w, h),
            feat,
            state,
        })
    }

    pub(super) fn maps(&mut self, pictures: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let (w, h) = self.size;
        let plane = w * h;
        let mut out = Vec::new();
        for run in pictures.chunks(FRAMES as usize) {
            let mut input = Tensor::new(run.len(), h, w, 3);
            for (slot, picture) in input.v.chunks_mut(plane * 3).zip(run) {
                slot.copy_from_slice(picture);
            }
            let maps = self.forward(&input);
            out.extend(maps.chunks(plane).map(<[f32]>::to_vec));
        }
        out
    }

    fn forward(&mut self, input: &Tensor) -> Vec<f32> {
        let (net, n) = (&self.net, input.n);
        let (w, h) = self.size;
        let wide = (LAST + PRIORS) as usize;
        let (hidden, two, four) = (HIDDEN as usize, TWO as usize, FOUR as usize);
        let mut cat2 = Tensor::new(n, h / 16, w / 16, hidden + two);
        let mut cat4 = Tensor::new(n, h / 8, w / 8, two + four);
        let mut x = conv(&net.stem, input, None);
        for (i, b) in net.blocks.iter().enumerate() {
            x = block(b, &x);
            if i + 1 == TAPS.0 {
                skip_into(&net.skip4, &x, (&mut cat4, two));
            }
            if i + 1 == TAPS.1 {
                skip_into(&net.skip2, &x, (&mut cat2, hidden));
            }
            if b.down {
                x = halved(&x);
            }
        }
        let (fh, fw) = (h / 32, w / 32);
        let mut feat = Tensor {
            n,
            h: fh,
            w: fw,
            c: wide,
            v: self.feat[..n * fh * fw * wide].to_vec(),
        };
        conv_into(&net.last, &x, None, (&mut feat, 0));
        let mut im = block(&net.post, &feat);
        let cells = fh * fw * hidden;
        let mut state = Tensor {
            n,
            h: fh,
            w: fw,
            c: hidden,
            v: self.state.v[..n * cells].to_vec(),
        };
        gru(&net.gru, &im, &mut state);
        self.state.v[..n * cells].copy_from_slice(&state.v);
        let rf = conv(&net.after, &state, None);
        im.v.iter_mut().zip(&rf.v).for_each(|(a, b)| *a += b);
        up_into(&im, (&mut cat2, 0));
        let u2 = block(&net.up2, &cat2);
        up_into(&u2, (&mut cat4, 0));
        let p4 = block(&net.post2, &cat4);
        let adapt = conv(&net.adapt, &p4, None);
        smoothed(&net.smooth, &adapt, (w, h))
    }
}
