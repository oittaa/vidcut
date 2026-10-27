use std::path::Path;

use super::ops::{self, Act, Carve, Conv, Deconv, Grid, Ops, Res, Shape, View, tensor};
use crate::compute::{Gpu, bytes};
use crate::error::Result;
use crate::weights::Torch;

const DEPTH: usize = 4;
const HALO: u32 = 96;
const BAND: u64 = 1 << 29;

pub(super) struct Norm {
    pub(super) gamma: wgpu::Buffer,
    pub(super) beta: wgpu::Buffer,
    pub(super) eps: f32,
}

struct Former {
    heads: u32,
    shift: u32,
    norm1: Norm,
    q: Conv,
    kv: Conv,
    cor: (wgpu::Buffer, wgpu::Buffer),
    proj: Conv,
    motion: Conv,
    norm2: Norm,
    fc1: Conv,
    dw: (wgpu::Buffer, wgpu::Buffer),
    fc2: Conv,
}

pub(super) struct Unet {
    down: Vec<(Conv, Conv)>,
    up: Vec<Deconv>,
    last: Conv,
}

pub(super) struct Head {
    pub(super) convs: Vec<Conv>,
}

pub(super) struct Ema {
    stem: Vec<Conv>,
    embed2: Conv,
    block2: Vec<Conv>,
    embed3: Conv,
    block3: Vec<Conv>,
    cross: Vec<Conv>,
    cross_proj: Conv,
    cross_norm: Norm,
    stage4: Vec<Former>,
    norm4: Norm,
    embed5: Conv,
    embed5_norm: Norm,
    stage5: Vec<Former>,
    norm5: Norm,
    heads: Vec<Head>,
    unet: Unet,
}

pub(super) struct Loader<'a> {
    pub(super) gpu: &'a Gpu,
    pub(super) t: Torch,
    pub(super) prefix: &'static str,
}

impl Loader<'_> {
    pub(super) fn floats(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        self.t.floats(&format!("{}{name}", self.prefix), shape)
    }

    pub(super) fn buffer(&self, name: &str, shape: &[usize]) -> Result<wgpu::Buffer> {
        Ok(self.gpu.upload(&bytes(&self.floats(name, shape)?)))
    }

    pub(super) fn conv(&self, name: &str, s: Shape, act: Act, prelu: Option<&str>) -> Result<Conv> {
        let (kh, kw) = s.k;
        let w = self.floats(
            &format!("{name}.weight"),
            &[s.cout as usize, s.cin as usize, kh as usize, kw as usize],
        )?;
        let b = self.floats(&format!("{name}.bias"), &[s.cout as usize])?;
        let a = prelu
            .map(|p| self.floats(&format!("{p}.weight"), &[s.cout as usize]))
            .transpose()?;
        let act = if a.is_some() { Act::Prelu } else { act };
        Ok(ops::conv(self.gpu, s, &w, Some(&b), act, a.as_deref()))
    }

    pub(super) fn linear(
        &self,
        name: &str,
        cin: u32,
        cout: u32,
        bias: bool,
        act: Act,
    ) -> Result<Conv> {
        let w = self.floats(&format!("{name}.weight"), &[cout as usize, cin as usize])?;
        let b = if bias {
            Some(self.floats(&format!("{name}.bias"), &[cout as usize])?)
        } else {
            None
        };
        Ok(ops::conv(
            self.gpu,
            Shape::square(cin, cout, 1, 1, 0),
            &w,
            b.as_deref(),
            act,
            None,
        ))
    }

    pub(super) fn deconv(
        &self,
        name: &str,
        cin: u32,
        cout: u32,
        prelu: Option<&str>,
    ) -> Result<Deconv> {
        let w = self.floats(
            &format!("{name}.weight"),
            &[cin as usize, cout as usize, 4, 4],
        )?;
        let b = self.floats(&format!("{name}.bias"), &[cout as usize])?;
        let a = prelu
            .map(|p| self.floats(&format!("{p}.weight"), &[cout as usize]))
            .transpose()?;
        let act = if a.is_some() { Act::Prelu } else { Act::None };
        Ok(ops::deconv(
            self.gpu,
            cin,
            cout,
            &w,
            Some(&b),
            act,
            a.as_deref(),
        ))
    }

    pub(super) fn norm(&self, name: &str, c: u32, eps: f32) -> Result<Norm> {
        Ok(Norm {
            gamma: self.buffer(&format!("{name}.weight"), &[c as usize])?,
            beta: self.buffer(&format!("{name}.bias"), &[c as usize])?,
            eps,
        })
    }

    pub(super) fn convs(&self, name: &str, cin: u32, c: u32) -> Result<Vec<Conv>> {
        Ok(vec![
            self.conv(
                &format!("{name}.0"),
                Shape::square(cin, c, 3, 1, 1),
                Act::None,
                Some(&format!("{name}.1")),
            )?,
            self.conv(
                &format!("{name}.2"),
                Shape::square(c, c, 3, 1, 1),
                Act::None,
                Some(&format!("{name}.3")),
            )?,
        ])
    }

    pub(super) fn unet(&self, c: u32) -> Result<Unet> {
        let mut down = Vec::new();
        for (i, (cin, cout)) in [
            (17 + c, 2 * c),
            (4 * c, 4 * c),
            (8 * c, 8 * c),
            (16 * c, 16 * c),
        ]
        .into_iter()
        .enumerate()
        {
            let at = format!("unet.down{i}");
            down.push((
                self.conv(
                    &format!("{at}.conv1.0"),
                    Shape::square(cin, cout, 3, 2, 1),
                    Act::None,
                    Some(&format!("{at}.conv1.1")),
                )?,
                self.conv(
                    &format!("{at}.conv2.0"),
                    Shape::square(cout, cout, 3, 1, 1),
                    Act::None,
                    Some(&format!("{at}.conv2.1")),
                )?,
            ));
        }
        let mut up = Vec::new();
        for (i, (cin, cout)) in [(32 * c, 8 * c), (16 * c, 4 * c), (8 * c, 2 * c), (4 * c, c)]
            .into_iter()
            .enumerate()
        {
            up.push(self.deconv(
                &format!("unet.up{i}.0"),
                cin,
                cout,
                Some(&format!("unet.up{i}.1")),
            )?);
        }
        let last = self.conv(
            "unet.conv",
            Shape::square(c, 3, 3, 1, 1),
            Act::Sigmoid,
            None,
        )?;
        Ok(Unet { down, up, last })
    }

    pub(super) fn head(&self, i: usize, cin: u32, c: u32) -> Result<Head> {
        let mut convs = Vec::new();
        for (j, (a, b)) in [(cin, c), (c, c), (c, 5)].into_iter().enumerate() {
            convs.push(self.conv(
                &format!("block.{i}.conv.{j}.0"),
                Shape::square(a, b, 3, 1, 1),
                Act::None,
                Some(&format!("block.{i}.conv.{j}.1")),
            )?);
        }
        Ok(Head { convs })
    }
}

fn former(l: &Loader, name: &str, dim: u32, motion: u32, heads: u32, shift: u32) -> Result<Former> {
    let at = |s: &str| format!("{name}.{s}");
    Ok(Former {
        heads,
        shift,
        norm1: l.norm(&at("norm1"), dim, 1e-6)?,
        q: l.linear(&at("attn.q"), dim, dim, true, Act::None)?,
        kv: l.linear(&at("attn.kv"), dim, 2 * dim, true, Act::None)?,
        cor: (
            l.buffer(&at("attn.cor_embed.weight"), &[motion as usize, 2])?,
            l.buffer(&at("attn.cor_embed.bias"), &[motion as usize])?,
        ),
        proj: l.linear(&at("attn.proj"), dim, dim, true, Act::None)?,
        motion: l.linear(&at("attn.motion_proj"), motion, motion, true, Act::None)?,
        norm2: l.norm(&at("norm2"), dim, 1e-6)?,
        fc1: l.linear(&at("mlp.fc1"), dim, 4 * dim, true, Act::None)?,
        dw: (
            l.buffer(
                &at("mlp.dwconv.dwconv.weight"),
                &[4 * dim as usize, 1, 3, 3],
            )?,
            l.buffer(&at("mlp.dwconv.dwconv.bias"), &[4 * dim as usize])?,
        ),
        fc2: l.linear(&at("mlp.fc2"), 4 * dim, dim, true, Act::None)?,
    })
}

pub(super) fn load(gpu: &Gpu, path: &Path, title: &'static str) -> Result<Ema> {
    let l = Loader {
        gpu,
        t: Torch::open(path, title)?,
        prefix: "module.",
    };
    let fb = |s: &str| format!("feature_bone.{s}");
    let prelu_conv = |name: &str, cin: u32, cout: u32| {
        l.conv(
            &fb(&format!("{name}.0")),
            Shape::square(cin, cout, 3, 2, 1),
            Act::None,
            Some(&fb(&format!("{name}.1"))),
        )
    };
    let mut cross = Vec::new();
    for (i, cin) in [128u32, 64, 32].into_iter().enumerate() {
        for j in 0..(1u32 << i) {
            let k = cross.len();
            cross.push(l.conv(
                &fb(&format!("patch_embed4.layers.{k}")),
                Shape {
                    cin,
                    cout: 32,
                    k: (3, 3),
                    stride: 1 << (i + 1),
                    pad: (1 + j as i32, 1 + j as i32),
                    dil: 1 + j,
                },
                Act::None,
                None,
            )?);
        }
    }
    let stage = |name: &str, dim: u32, motion: u32, heads: u32| -> Result<Vec<Former>> {
        (0..DEPTH)
            .map(|j| {
                former(
                    &l,
                    &fb(&format!("{name}.{j}")),
                    dim,
                    motion,
                    heads,
                    if j % 2 == 0 { 0 } else { 3 },
                )
            })
            .collect()
    };
    Ok(Ema {
        stem: l.convs(&fb("block1.conv"), 3, 32)?,
        embed2: prelu_conv("patch_embed2", 32, 64)?,
        block2: l.convs(&fb("block2.conv"), 64, 64)?,
        embed3: prelu_conv("patch_embed3", 64, 128)?,
        block3: l.convs(&fb("block3.conv"), 128, 128)?,
        cross,
        cross_proj: l.conv(
            &fb("patch_embed4.proj"),
            Shape::square(224, 256, 1, 1, 0),
            Act::None,
            None,
        )?,
        cross_norm: l.norm(&fb("patch_embed4.norm"), 256, 1e-5)?,
        stage4: stage("block4", 256, 64, 8)?,
        norm4: l.norm(&fb("norm4"), 256, 1e-6)?,
        embed5: l.conv(
            &fb("patch_embed5.proj"),
            Shape::square(256, 512, 3, 2, 1),
            Act::None,
            None,
        )?,
        embed5_norm: l.norm(&fb("patch_embed5.norm"), 512, 1e-5)?,
        stage5: stage("block5", 512, 128, 16)?,
        norm5: l.norm(&fb("norm5"), 512, 1e-6)?,
        heads: vec![l.head(0, 134, 128)?, l.head(1, 81, 128)?],
        unet: l.unet(64)?,
    })
}

struct Tokens {
    win: View,
    normed: View,
    q: View,
    kv: View,
    ce: View,
    xo: View,
    cr: View,
    mo: View,
    xa: View,
    x1: View,
    n2: View,
    h1: View,
    h2: View,
    ping: View,
}

fn tokens(c: &mut Carve, grid: &Grid, (w, h): (u32, u32), ch: u32, cm: u32) -> Tokens {
    let n = 2 * grid.tokens();
    Tokens {
        win: c.view(1, 1, n, ch),
        normed: c.view(1, 1, n, ch),
        q: c.view(1, 1, n, ch),
        kv: c.view(1, 1, n, 2 * ch),
        ce: c.view(1, 1, n, cm),
        xo: c.view(1, 1, n, ch),
        cr: c.view(1, 1, n, cm),
        mo: c.view(1, 1, n, cm),
        xa: c.view(1, 1, n, ch),
        x1: c.view(2, h, w, ch),
        n2: c.view(2, h, w, ch),
        h1: c.view(2, h, w, 4 * ch),
        h2: c.view(2, h, w, 4 * ch),
        ping: c.view(2, h, w, ch),
    }
}

struct Scale {
    size: (u32, u32),
    g4: [Grid; 2],
    g5: [Grid; 2],
}

impl Scale {
    fn new(gpu: &Gpu, (w, h): (u32, u32)) -> Self {
        let grids = |s: u32| {
            [
                Grid::new(gpu, h / s, w / s, 0),
                Grid::new(gpu, h / s, w / s, 3),
            ]
        };
        Self {
            size: (w, h),
            g4: grids(8),
            g5: grids(16),
        }
    }
}

pub(super) struct Shallow {
    af: [View; 3],
}

fn shallow_for(c: &mut Carve, (w, h): (u32, u32)) -> Shallow {
    Shallow {
        af: [
            c.view(1, h, w, 32),
            c.view(1, h / 2, w / 2, 64),
            c.view(1, h / 4, w / 4, 128),
        ],
    }
}

pub(super) struct Deep {
    af3: View,
    af4: View,
    mf: Option<[View; 2]>,
}

fn deep_for(gpu: &Gpu, (w, h): (u32, u32), motion: bool) -> Deep {
    let at = |s: u32, c: u32| tensor(gpu, 2, h / s, w / s, c);
    Deep {
        af3: at(8, 256),
        af4: at(16, 512),
        mf: motion.then(|| [at(8, 256), at(16, 512)]),
    }
}

struct Backbone {
    cross: View,
    tmp3: View,
    tmp4: View,
    shallow: [Shallow; 2],
    temp: View,
    t4: Tokens,
    t5: Tokens,
}

fn backbone_for(c: &mut Carve, s: &Scale) -> Backbone {
    let (w, h) = s.size;
    let cross = c.view(2, h / 8, w / 8, 224);
    let tmp3 = c.view(2, h / 8, w / 8, 256);
    let tmp4 = c.view(2, h / 16, w / 16, 512);
    let m = c.mark();
    let shallow = [shallow_for(c, s.size), shallow_for(c, s.size)];
    let temp = c.view(1, h, w, 32);
    c.reset(m);
    let t4 = tokens(c, &s.g4[0], (w / 8, h / 8), 256, 64);
    c.reset(m);
    let t5 = tokens(c, &s.g5[0], (w / 16, h / 16), 512, 128);
    c.reset(m);
    Backbone {
        cross,
        tmp3,
        tmp4,
        shallow,
        temp,
        t4,
        t5,
    }
}

pub(super) struct Flow {
    pub(super) flow: View,
    pub(super) mask: View,
    m4: View,
    m3: View,
    up4: View,
    x4: View,
    c4: (View, View),
    out4: View,
    big: View,
    up2: View,
    x2: View,
    c2: (View, View),
    out2: View,
    warped: View,
}

fn flow_for(c: &mut Carve, (w, h): (u32, u32)) -> Flow {
    let mut one = |s: u32, ch: u32| c.view(1, h / s, w / s, ch);
    Flow {
        flow: one(1, 4),
        mask: one(1, 1),
        m4: one(16, 2048),
        m3: one(8, 1024),
        up4: one(8, 512),
        x4: one(4, 134),
        c4: (one(4, 128), one(4, 128)),
        out4: one(4, 5),
        big: one(1, 5),
        up2: one(4, 256),
        x2: one(2, 81),
        c2: (one(2, 128), one(2, 128)),
        out2: one(2, 5),
        warped: one(1, 6),
    }
}

struct Band {
    x: View,
    y: View,
    z: [View; 4],
    warped: View,
    flows: [View; 4],
}

fn band_for(c: &mut Carve, rows: u32, w: u32, ch: u32) -> Band {
    let mut at = |s: u32, k: u32| c.view(1, rows >> s, w >> s, k);
    Band {
        x: at(0, 17 + ch),
        y: at(0, ch),
        z: [at(1, 2 * ch), at(2, 4 * ch), at(3, 8 * ch), at(4, 16 * ch)],
        warped: at(0, 6),
        flows: [at(1, 4), at(2, 4), at(3, 4), at(4, 4)],
    }
}

fn band_rows((w, h): (u32, u32), ch: u32) -> u32 {
    let per_row = u64::from(w) * u64::from(17 + 3 * ch + 7 + 2);
    let rows = BAND / per_row;
    if rows >= u64::from(h) {
        return h;
    }
    ((rows as u32).saturating_sub(2 * HALO) / 16 * 16).max(16)
}

fn bands(h: u32, rows: u32) -> Vec<((u32, u32), (u32, u32))> {
    let mut out = Vec::new();
    let mut y0 = 0;
    while y0 < h {
        let y1 = (y0 + rows).min(h);
        let a = y0.saturating_sub(HALO);
        let b = (y1 + HALO).min(h);
        out.push(((a, b - a), (y0, y1)));
        y0 = y1;
    }
    out
}

fn motion_scale((w, h): (u32, u32)) -> f32 {
    if w >= super::UHD.0 || h >= super::UHD.1 {
        0.25
    } else if w >= 1920 || h >= 1080 {
        0.5
    } else {
        1.0
    }
}

fn centered((w, h): (u32, u32), step: u32) -> ((u32, u32), (u32, u32)) {
    let (pw, ph) = (w.next_multiple_of(step), h.next_multiple_of(step));
    ((pw, ph), ((ph - h) / 2, (pw - w) / 2))
}

struct Turn {
    img: View,
    small: Option<(View, Deep)>,
    full: Deep,
    pred: View,
}

struct Work {
    backbones: Vec<Backbone>,
    shallow: [Shallow; 2],
    temp: View,
    flow: Flow,
    band: Band,
}

pub(super) struct Pair {
    turns: Vec<Turn>,
    up: (View, View),
    scales: Vec<Scale>,
    work: Work,
    _arena: Vec<wgpu::Buffer>,
    ds: f32,
    pad: (u32, u32),
    padded: (u32, u32),
    rows: u32,
}

impl Pair {
    pub(super) fn new(gpu: &Gpu, size: (u32, u32)) -> Result<Self> {
        let ds = motion_scale(size);
        let (padded, pad) = centered(size, (32.0 / ds) as u32);
        let (pw, ph) = padded;
        let small = ((pw as f32 * ds) as u32, (ph as f32 * ds) as u32);
        let mut scales = vec![Scale::new(gpu, padded)];
        if ds < 1.0 {
            scales.push(Scale::new(gpu, small));
        }
        let ch = 64;
        let rows = band_rows(padded, ch);
        let held = (rows + 2 * HALO).min(ph);
        let (arena, work) = ops::arena(gpu, |c| {
            let mut backbones = Vec::new();
            for s in &scales {
                backbones.push(backbone_for(c, s));
                c.reset(0);
            }
            let shallow = [shallow_for(c, padded), shallow_for(c, padded)];
            let m = c.mark();
            let flow = flow_for(c, if ds < 1.0 { small } else { padded });
            c.reset(m);
            let temp = c.view(1, ph, pw, 32);
            c.reset(m);
            let band = band_for(c, held, pw, ch);
            Work {
                backbones,
                shallow,
                temp,
                flow,
                band,
            }
        })?;
        let turns = (0..2)
            .map(|_| Turn {
                img: tensor(gpu, 2, ph, pw, 3),
                small: (ds < 1.0).then(|| {
                    (
                        tensor(gpu, 2, small.1, small.0, 3),
                        deep_for(gpu, small, true),
                    )
                }),
                full: deep_for(gpu, padded, ds >= 1.0),
                pred: tensor(gpu, 1, ph, pw, 3),
            })
            .collect();
        Ok(Self {
            turns,
            up: (tensor(gpu, 1, ph, pw, 4), tensor(gpu, 1, ph, pw, 1)),
            scales,
            work,
            _arena: arena,
            ds,
            pad,
            padded,
            rows,
        })
    }

    pub(super) fn load(
        &self,
        o: &mut Ops,
        (a, b): (&wgpu::Buffer, &wgpu::Buffer),
        size: (u32, u32),
    ) {
        for (i, turn) in self.turns.iter().enumerate() {
            o.load_turned(a, size, self.pad, true, i == 1, &turn.img.frame(0));
            o.load_turned(b, size, self.pad, true, i == 1, &turn.img.frame(1));
            if let Some((img, _)) = &turn.small {
                o.scaled(&turn.img, img, img.w as f32 / turn.img.w as f32, 1.0, 0);
            }
        }
    }

    pub(super) fn backbone(&self, o: &mut Ops, net: &Ema) {
        let w = &self.work;
        for turn in &self.turns {
            net.backbone(o, &turn.img, &self.scales[0], &w.backbones[0], &turn.full);
            if let Some((img, deep)) = &turn.small {
                net.backbone(o, img, &self.scales[1], &w.backbones[1], deep);
            }
        }
    }

    pub(super) fn frame(
        &self,
        o: &mut Ops,
        net: &Ema,
        t: f32,
        size: (u32, u32),
        out: &wgpu::Buffer,
    ) {
        let w = &self.work;
        let (flow, mask) = (&self.up.0, &self.up.1);
        for turn in &self.turns {
            let (img, deep) = match &turn.small {
                Some((img, deep)) => (img, deep),
                None => (&turn.img, &turn.full),
            };
            net.flow(o, img, deep, &w.flow, t);
            if self.ds < 1.0 {
                o.scaled(&w.flow.flow, flow, 1.0 / self.ds, 1.0 / self.ds, 4);
                o.scaled(&w.flow.mask, mask, 1.0 / self.ds, 1.0, 0);
            } else {
                o.copy(&w.flow.flow, flow, 1.0, 0.0, Act::None);
                o.copy(&w.flow.mask, mask, 1.0, 0.0, Act::None);
            }
            for (k, s) in w.shallow.iter().enumerate() {
                net.shallow(o, &turn.img.frame(k as u32), s, &w.temp);
            }
            let inputs = Inputs {
                img: &turn.img,
                shallow: &w.shallow,
                deep: &turn.full,
                flow,
                mask,
            };
            for band in bands(self.padded.1, self.rows) {
                net.refine(o, &inputs, &w.band, band, &turn.pred);
            }
        }
        o.store_both(
            &self.turns[0].pred,
            Some(&self.turns[1].pred),
            self.pad,
            size,
            out,
        );
    }
}

struct Inputs<'a> {
    img: &'a View,
    shallow: &'a [Shallow; 2],
    deep: &'a Deep,
    flow: &'a View,
    mask: &'a View,
}

impl Ema {
    #[allow(clippy::too_many_arguments)]
    fn former(
        &self,
        o: &mut Ops,
        f: &Former,
        t: &Tokens,
        g: &Grid,
        x: &View,
        out: &View,
        motion: Option<&View>,
    ) {
        o.windows(g, x, &t.win);
        o.norm(
            &f.norm1.gamma,
            &f.norm1.beta,
            f.norm1.eps,
            &t.win,
            &t.normed,
        );
        o.conv(&f.q, &t.normed, &t.q, Res::None);
        o.conv(&f.kv, &t.normed, &t.kv, Res::None);
        o.cor(g, (x.h, x.w), 2, &f.cor.0, &f.cor.1, &t.ce);
        o.attend(g, f.heads, &t.q, &t.kv, &t.ce, &t.xo, &t.cr);
        if motion.is_some() {
            o.combine(&t.cr, &t.ce, &t.xa.part(0, t.ce.c), 1.0, -1.0);
            o.conv(&f.motion, &t.xa.part(0, t.ce.c), &t.mo, Res::None);
        }
        o.conv(&f.proj, &t.xo, &t.xa, Res::None);
        o.add(&t.xa, &t.normed);
        o.unwindows(g, &t.normed, &t.x1);
        if let Some(m) = motion {
            o.unwindows(g, &t.mo, m);
        }
        o.norm(&f.norm2.gamma, &f.norm2.beta, f.norm2.eps, &t.x1, &t.n2);
        o.conv(&f.fc1, &t.n2, &t.h1, Res::None);
        o.dwconv(&f.dw.0, &f.dw.1, 3, Act::Gelu, &t.h1, &t.h2);
        o.conv(&f.fc2, &t.h2, out, Res::Before(&t.x1));
    }

    #[allow(clippy::too_many_arguments)]
    fn stage(
        &self,
        o: &mut Ops,
        blocks: &[Former],
        t: &Tokens,
        grids: &[Grid; 2],
        x: &View,
        mf: Option<&View>,
        norm: &Norm,
        af: &View,
    ) {
        let cm = mf.map_or(0, |m| m.c / blocks.len() as u32);
        let mut from = x.clone();
        for (j, f) in blocks.iter().enumerate() {
            let to = if j % 2 == 0 {
                t.ping.clone()
            } else {
                x.clone()
            };
            let g = &grids[usize::from(f.shift > 0)];
            let motion = mf.map(|m| m.part(j as u32 * cm, cm));
            self.former(o, f, t, g, &from, &to, motion.as_ref());
            from = to;
        }
        o.norm(&norm.gamma, &norm.beta, norm.eps, &from, af);
    }

    fn shallow(&self, o: &mut Ops, img: &View, s: &Shallow, temp: &View) {
        let (h, w) = (img.h, img.w);
        let t0 = temp.shaped(1, h, w, 32);
        o.conv(&self.stem[0], img, &t0, Res::None);
        o.conv(&self.stem[1], &t0, &s.af[0], Res::None);
        let t1 = temp.shaped(1, h / 2, w / 2, 64);
        o.conv(&self.embed2, &s.af[0], &t1, Res::None);
        o.conv(&self.block2[0], &t1, &s.af[1], Res::None);
        o.conv(&self.block2[1], &s.af[1], &t1, Res::None);
        o.copy(&t1, &s.af[1], 1.0, 0.0, Act::None);
        let t2 = temp.shaped(1, h / 4, w / 4, 128);
        o.conv(&self.embed3, &s.af[1], &t2, Res::None);
        o.conv(&self.block3[0], &t2, &s.af[2], Res::None);
        o.conv(&self.block3[1], &s.af[2], &t2, Res::None);
        o.copy(&t2, &s.af[2], 1.0, 0.0, Act::None);
    }

    fn backbone(&self, o: &mut Ops, img: &View, s: &Scale, b: &Backbone, d: &Deep) {
        for (k, sh) in b.shallow.iter().enumerate() {
            self.shallow(o, &img.frame(k as u32), sh, &b.temp);
        }
        let levels = [2, 1, 1, 0, 0, 0, 0];
        for (k, sh) in b.shallow.iter().enumerate() {
            let cross = b.cross.frame(k as u32);
            for (j, (c, l)) in self.cross.iter().zip(levels).enumerate() {
                o.conv(c, &sh.af[l], &cross.part(32 * j as u32, 32), Res::None);
            }
        }
        o.conv(&self.cross_proj, &b.cross, &b.tmp3, Res::None);
        let n = &self.cross_norm;
        o.norm(&n.gamma, &n.beta, n.eps, &b.tmp3, &d.af3);
        let mf = d.mf.as_ref();
        self.stage(
            o,
            &self.stage4,
            &b.t4,
            &s.g4,
            &d.af3,
            mf.map(|m| &m[0]),
            &self.norm4,
            &b.tmp3,
        );
        o.copy(&b.tmp3, &d.af3, 1.0, 0.0, Act::None);
        o.conv(&self.embed5, &d.af3, &b.tmp4, Res::None);
        let n = &self.embed5_norm;
        o.norm(&n.gamma, &n.beta, n.eps, &b.tmp4, &d.af4);
        self.stage(
            o,
            &self.stage5,
            &b.t5,
            &s.g5,
            &d.af4,
            mf.map(|m| &m[1]),
            &self.norm5,
            &b.tmp4,
        );
        o.copy(&b.tmp4, &d.af4, 1.0, 0.0, Act::None);
    }

    fn flow(&self, o: &mut Ops, img: &View, d: &Deep, fl: &Flow, t: f32) {
        let (img0, img1) = (img.frame(0), img.frame(1));
        let mf = d.mf.as_ref().expect("motion features");
        let motion = |o: &mut Ops, level: usize, dst: &View| {
            let (mf, af) = match level {
                3 => (&mf[0], &d.af3),
                _ => (&mf[1], &d.af4),
            };
            let c = mf.c;
            o.copy(&mf.frame(0), &dst.part(0, c), t, 0.0, Act::None);
            o.copy(&mf.frame(1), &dst.part(c, c), 1.0 - t, 0.0, Act::None);
            o.copy(&af.frame(0), &dst.part(2 * c, af.c), 1.0, 0.0, Act::None);
            o.copy(
                &af.frame(1),
                &dst.part(2 * c + af.c, af.c),
                1.0,
                0.0,
                Act::None,
            );
        };
        motion(o, 4, &fl.m4);
        o.shuffle(&fl.m4, &fl.up4, 2);
        o.shuffle(&fl.up4, &fl.x4.part(0, 128), 2);
        o.scaled(&img0, &fl.x4.part(128, 3), 0.25, 1.0, 0);
        o.scaled(&img1, &fl.x4.part(131, 3), 0.25, 1.0, 0);
        let h = &self.heads[0];
        o.conv(&h.convs[0], &fl.x4, &fl.c4.0, Res::None);
        o.conv(&h.convs[1], &fl.c4.0, &fl.c4.1, Res::None);
        o.conv(&h.convs[2], &fl.c4.1, &fl.out4, Res::None);
        o.scaled(&fl.out4, &fl.big, 4.0, 4.0, 4);
        o.copy(&fl.big.part(0, 4), &fl.flow, 1.0, 0.0, Act::None);
        o.copy(&fl.big.part(4, 1), &fl.mask, 1.0, 0.0, Act::None);
        o.warp(&img0, &fl.flow.part(0, 2), &fl.warped.part(0, 3));
        o.warp(&img1, &fl.flow.part(2, 2), &fl.warped.part(3, 3));
        motion(o, 3, &fl.m3);
        o.shuffle(&fl.m3, &fl.up2, 2);
        o.shuffle(&fl.up2, &fl.x2.part(0, 64), 2);
        o.scaled(&img0, &fl.x2.part(64, 3), 0.5, 1.0, 0);
        o.scaled(&img1, &fl.x2.part(67, 3), 0.5, 1.0, 0);
        o.scaled(&fl.warped, &fl.x2.part(70, 6), 0.5, 1.0, 0);
        o.scaled(&fl.mask, &fl.x2.part(76, 1), 0.5, 1.0, 0);
        o.scaled(&fl.flow, &fl.x2.part(77, 4), 0.5, 0.5, 4);
        let h = &self.heads[1];
        o.conv(&h.convs[0], &fl.x2, &fl.c2.0, Res::None);
        o.conv(&h.convs[1], &fl.c2.0, &fl.c2.1, Res::None);
        o.conv(&h.convs[2], &fl.c2.1, &fl.out2, Res::None);
        o.scaled(&fl.out2, &fl.big, 2.0, 2.0, 4);
        o.add(&fl.big.part(0, 4), &fl.flow);
        o.add(&fl.big.part(4, 1), &fl.mask);
    }

    fn refine(
        &self,
        o: &mut Ops,
        i: &Inputs,
        band: &Band,
        ((a, hb), (y0, y1)): ((u32, u32), (u32, u32)),
        pred: &View,
    ) {
        let u = &self.unet;
        let w = i.img.w;
        let (img0, img1) = (i.img.frame(0), i.img.frame(1));
        let sh = i.shallow;
        let a0 = sh[0].af[0].c;
        let c = 2 * a0;
        let fl = i.flow.rows(a, hb);
        let d0 = band.x.shaped(1, hb, w, 17 + c);
        o.copy(&img0.rows(a, hb), &d0.part(0, 3), 1.0, 0.0, Act::None);
        o.copy(&img1.rows(a, hb), &d0.part(3, 3), 1.0, 0.0, Act::None);
        o.warp_at(&img0, &fl.part(0, 2), &d0.part(6, 3), a);
        o.warp_at(&img1, &fl.part(2, 2), &d0.part(9, 3), a);
        let warped = band.warped.shaped(1, hb, w, 6);
        o.warp_at(&img0, &fl.part(0, 2), &warped.part(0, 3), a);
        o.warp_at(&img1, &fl.part(2, 2), &warped.part(3, 3), a);
        o.copy(&i.mask.rows(a, hb), &d0.part(12, 1), 1.0, 0.0, Act::None);
        o.copy(&fl, &d0.part(13, 4), 1.0, 0.0, Act::None);
        o.warp_at(&sh[0].af[0], &fl.part(0, 2), &d0.part(17, a0), a);
        o.warp_at(&sh[1].af[0], &fl.part(2, 2), &d0.part(17 + a0, a0), a);
        let mut flows = Vec::new();
        let mut prev = fl;
        for (k, f) in band.flows.iter().enumerate() {
            let s = k as u32 + 1;
            let f = f.shaped(1, hb >> s, w >> s, 4);
            o.scaled(&prev, &f, 0.5, 0.5, 4);
            prev = f.clone();
            flows.push(f);
        }
        let feature = |k: usize, b: usize| match k {
            0 => sh[b].af[1].clone(),
            1 => sh[b].af[2].clone(),
            2 => i.deep.af3.frame(b as u32),
            _ => i.deep.af4.frame(b as u32),
        };
        let skip = |k: usize| {
            let s = k as u32 + 1;
            band.z[k].shaped(1, hb >> s, w >> s, u.down[k].1.cout)
        };
        let mut input = d0;
        for (k, (c1, c2)) in u.down.iter().enumerate() {
            let s = k as u32 + 1;
            let (hk, wk) = (hb >> s, w >> s);
            let mid = band.y.shaped(1, hk, wk, c1.cout);
            o.conv(c1, &input, &mid, Res::None);
            let sk = skip(k);
            o.conv(c2, &mid, &sk, Res::None);
            let (f0, f1) = (feature(k, 0), feature(k, 1));
            let dk = band.x.shaped(1, hk, wk, sk.c + 2 * f0.c);
            o.copy(&sk, &dk.part(0, sk.c), 1.0, 0.0, Act::None);
            o.warp_at(&f0, &flows[k].part(0, 2), &dk.part(sk.c, f0.c), a >> s);
            o.warp_at(
                &f1,
                &flows[k].part(2, 2),
                &dk.part(sk.c + f0.c, f0.c),
                a >> s,
            );
            input = dk;
        }
        let u0 = band.y.shaped(1, hb >> 3, w >> 3, 16 * c);
        o.deconv(&u.up[0], &input, &u0.part(0, 8 * c), 1);
        o.copy(&skip(2), &u0.part(8 * c, 8 * c), 1.0, 0.0, Act::None);
        let u1 = band.x.shaped(1, hb >> 2, w >> 2, 8 * c);
        o.deconv(&u.up[1], &u0, &u1.part(0, 4 * c), 1);
        o.copy(&skip(1), &u1.part(4 * c, 4 * c), 1.0, 0.0, Act::None);
        let u2 = band.y.shaped(1, hb >> 1, w >> 1, 4 * c);
        o.deconv(&u.up[2], &u1, &u2.part(0, 2 * c), 1);
        o.copy(&skip(0), &u2.part(2 * c, 2 * c), 1.0, 0.0, Act::None);
        let u3 = band.x.shaped(1, hb, w, c);
        o.deconv(&u.up[3], &u2, &u3, 1);
        let out = band.y.shaped(1, hb, w, 3);
        o.conv(&u.last, &u3, &out, Res::None);
        let n = y1 - y0;
        o.predict(
            &warped.part(0, 3).rows(y0 - a, n),
            &warped.part(3, 3).rows(y0 - a, n),
            &i.mask.rows(y0, n),
            &out.rows(y0 - a, n),
            &pred.rows(y0, n),
        );
    }
}
