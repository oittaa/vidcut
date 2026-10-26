use std::path::Path;

use super::ops::{self, Act, Conv, Deconv, Grid, Ops, Res, Shape, View, tensor};
use crate::compute::{Gpu, bytes};
use crate::error::Result;
use crate::weights::Torch;

const DEPTH: usize = 4;

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
    grids: [Grid; 2],
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

fn tokens(gpu: &Gpu, h: u32, w: u32, c: u32, cm: u32) -> Tokens {
    let grids = [Grid::new(gpu, h, w, 0), Grid::new(gpu, h, w, 3)];
    let n = 2 * grids[0].tokens();
    let flat = |c: u32| tensor(gpu, 1, 1, n, c);
    Tokens {
        win: flat(c),
        normed: flat(c),
        q: flat(c),
        kv: flat(2 * c),
        ce: flat(cm),
        xo: flat(c),
        cr: flat(cm),
        mo: flat(cm),
        xa: flat(c),
        x1: tensor(gpu, 2, h, w, c),
        n2: tensor(gpu, 2, h, w, c),
        h1: tensor(gpu, 2, h, w, 4 * c),
        h2: tensor(gpu, 2, h, w, 4 * c),
        ping: tensor(gpu, 2, h, w, c),
        grids,
    }
}

pub(super) struct Features {
    pub(super) af: Vec<View>,
    pub(super) mf: [View; 2],
}

pub(super) struct Scratch {
    tmp: Vec<View>,
    cross: View,
    t4: Tokens,
    t5: Tokens,
}

pub(super) fn features_for(gpu: &Gpu, (w, h): (u32, u32)) -> Features {
    let at = |s: u32, c: u32| tensor(gpu, 2, h / s, w / s, c);
    Features {
        af: vec![at(1, 32), at(2, 64), at(4, 128), at(8, 256), at(16, 512)],
        mf: [at(8, 256), at(16, 512)],
    }
}

pub(super) fn scratch_for(gpu: &Gpu, (w, h): (u32, u32)) -> Scratch {
    let at = |s: u32, c: u32| tensor(gpu, 2, h / s, w / s, c);
    Scratch {
        tmp: vec![at(1, 32), at(2, 64), at(4, 128), at(8, 256), at(16, 512)],
        cross: at(8, 224),
        t4: tokens(gpu, h / 8, w / 8, 256, 64),
        t5: tokens(gpu, h / 16, w / 16, 512, 128),
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

pub(super) fn flow_for(gpu: &Gpu, (w, h): (u32, u32)) -> Flow {
    let one = |s: u32, c: u32| tensor(gpu, 1, h / s, w / s, c);
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

pub(super) struct Refine {
    pub(super) d0: View,
    flows: Vec<View>,
    d: Vec<View>,
    s: Vec<View>,
    mid: Vec<View>,
    u: Vec<View>,
    pub(super) out: View,
    pub(super) warped: View,
}

pub(super) fn refine_for(gpu: &Gpu, (w, h): (u32, u32), c: u32) -> Refine {
    let one = |s: u32, ch: u32| tensor(gpu, 1, h / s, w / s, ch);
    Refine {
        d0: one(1, 17 + c),
        flows: vec![one(2, 4), one(4, 4), one(8, 4), one(16, 4)],
        d: vec![
            one(2, 4 * c),
            one(4, 8 * c),
            one(8, 16 * c),
            one(16, 32 * c),
        ],
        s: vec![one(2, 2 * c), one(4, 4 * c), one(8, 8 * c), one(16, 16 * c)],
        mid: vec![one(2, 2 * c), one(4, 4 * c), one(8, 8 * c), one(16, 16 * c)],
        u: vec![one(8, 16 * c), one(4, 8 * c), one(2, 4 * c), one(1, c)],
        out: one(1, 3),
        warped: one(1, 6),
    }
}

impl Ema {
    fn former(&self, o: &mut Ops, f: &Former, t: &Tokens, x: &View, out: &View, motion: &View) {
        let g = &t.grids[usize::from(f.shift > 0)];
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
        o.combine(&t.cr, &t.ce, &t.xa.part(0, t.ce.c), 1.0, -1.0);
        o.conv(&f.motion, &t.xa.part(0, t.ce.c), &t.mo, Res::None);
        o.conv(&f.proj, &t.xo, &t.xa, Res::None);
        o.add(&t.xa, &t.normed);
        o.unwindows(g, &t.normed, &t.x1);
        o.unwindows(g, &t.mo, motion);
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
        x: &View,
        mf: &View,
        norm: &Norm,
        af: &View,
    ) {
        let cm = mf.c / blocks.len() as u32;
        let mut from = x.clone();
        for (j, f) in blocks.iter().enumerate() {
            let to = if j % 2 == 0 {
                t.ping.clone()
            } else {
                x.clone()
            };
            self.former(o, f, t, &from, &to, &mf.part(j as u32 * cm, cm));
            from = to;
        }
        o.norm(&norm.gamma, &norm.beta, norm.eps, &from, af);
    }

    pub(super) fn features(&self, o: &mut Ops, img: &View, f: &Features, x: &Scratch) {
        let (af, mf) = (&f.af, &f.mf);
        o.conv(&self.stem[0], img, &x.tmp[0], Res::None);
        o.conv(&self.stem[1], &x.tmp[0], &af[0], Res::None);
        o.conv(&self.embed2, &af[0], &x.tmp[1], Res::None);
        o.conv(&self.block2[0], &x.tmp[1], &af[1], Res::None);
        o.conv(&self.block2[1], &af[1], &x.tmp[1], Res::None);
        o.copy(&x.tmp[1], &af[1], 1.0, 0.0, Act::None);
        o.conv(&self.embed3, &af[1], &x.tmp[2], Res::None);
        o.conv(&self.block3[0], &x.tmp[2], &af[2], Res::None);
        o.conv(&self.block3[1], &af[2], &x.tmp[2], Res::None);
        o.copy(&x.tmp[2], &af[2], 1.0, 0.0, Act::None);
        let sources = [&af[2], &af[1], &af[1], &af[0], &af[0], &af[0], &af[0]];
        for (k, (c, src)) in self.cross.iter().zip(sources).enumerate() {
            o.conv(c, src, &x.cross.part(32 * k as u32, 32), Res::None);
        }
        o.conv(&self.cross_proj, &x.cross, &x.tmp[3], Res::None);
        let n = &self.cross_norm;
        o.norm(&n.gamma, &n.beta, n.eps, &x.tmp[3], &af[3]);
        self.stage(
            o,
            &self.stage4,
            &x.t4,
            &af[3],
            &mf[0],
            &self.norm4,
            &x.tmp[3],
        );
        o.copy(&x.tmp[3], &af[3], 1.0, 0.0, Act::None);
        o.conv(&self.embed5, &af[3], &x.tmp[4], Res::None);
        let n = &self.embed5_norm;
        o.norm(&n.gamma, &n.beta, n.eps, &x.tmp[4], &af[4]);
        self.stage(
            o,
            &self.stage5,
            &x.t5,
            &af[4],
            &mf[1],
            &self.norm5,
            &x.tmp[4],
        );
        o.copy(&x.tmp[4], &af[4], 1.0, 0.0, Act::None);
    }

    pub(super) fn flow(&self, o: &mut Ops, img: &View, f: &Features, fl: &Flow, t: f32) {
        let (img0, img1) = (img.frame(0), img.frame(1));
        let motion = |o: &mut Ops, level: usize, dst: &View| {
            let (mf, af) = (&f.mf[level - 3], &f.af[level]);
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

    pub(super) fn refine(
        &self,
        o: &mut Ops,
        img: &View,
        af: &[View],
        flow: &View,
        mask: &View,
        r: &Refine,
    ) {
        refine(o, &self.unet, img, af, flow, mask, r);
    }
}

pub(super) fn refine(
    o: &mut Ops,
    u: &Unet,
    img: &View,
    af: &[View],
    flow: &View,
    mask: &View,
    r: &Refine,
) {
    let (img0, img1) = (img.frame(0), img.frame(1));
    let a0 = af[0].c;
    let c = 2 * a0;
    o.copy(&img0, &r.d0.part(0, 3), 1.0, 0.0, Act::None);
    o.copy(&img1, &r.d0.part(3, 3), 1.0, 0.0, Act::None);
    o.warp(&img0, &flow.part(0, 2), &r.d0.part(6, 3));
    o.warp(&img1, &flow.part(2, 2), &r.d0.part(9, 3));
    o.warp(&img0, &flow.part(0, 2), &r.warped.part(0, 3));
    o.warp(&img1, &flow.part(2, 2), &r.warped.part(3, 3));
    o.copy(mask, &r.d0.part(12, 1), 1.0, 0.0, Act::None);
    o.copy(flow, &r.d0.part(13, 4), 1.0, 0.0, Act::None);
    o.warp(&af[0].frame(0), &flow.part(0, 2), &r.d0.part(17, a0));
    o.warp(&af[0].frame(1), &flow.part(2, 2), &r.d0.part(17 + a0, a0));
    let mut prev = flow.clone();
    for (k, f) in r.flows.iter().enumerate() {
        o.scaled(&prev, f, 0.5, 0.5, 4);
        let a = &af[k + 1];
        if k < 3 {
            let at = r.s[k].c;
            o.warp(&a.frame(0), &f.part(0, 2), &r.d[k].part(at, a.c));
            o.warp(&a.frame(1), &f.part(2, 2), &r.d[k].part(at + a.c, a.c));
        } else {
            let at = r.s[3].c;
            o.warp(&a.frame(0), &f.part(0, 2), &r.d[3].part(at, a.c));
            o.warp(&a.frame(1), &f.part(2, 2), &r.d[3].part(at + a.c, a.c));
        }
        prev = f.clone();
    }
    let mut input = r.d0.clone();
    for k in 0..4 {
        let (c1, c2) = &u.down[k];
        o.conv(c1, &input, &r.mid[k], Res::None);
        let s = &r.s[k];
        o.conv(c2, &r.mid[k], s, Res::None);
        o.copy(s, &r.d[k].part(0, s.c), 1.0, 0.0, Act::None);
        input = r.d[k].clone();
    }
    o.deconv(&u.up[0], &r.d[3], &r.u[0].part(0, 8 * c), 1);
    o.copy(&r.s[2], &r.u[0].part(8 * c, 8 * c), 1.0, 0.0, Act::None);
    o.deconv(&u.up[1], &r.u[0], &r.u[1].part(0, 4 * c), 1);
    o.copy(&r.s[1], &r.u[1].part(4 * c, 4 * c), 1.0, 0.0, Act::None);
    o.deconv(&u.up[2], &r.u[1], &r.u[2].part(0, 2 * c), 1);
    o.copy(&r.s[0], &r.u[2].part(2 * c, 2 * c), 1.0, 0.0, Act::None);
    o.deconv(&u.up[3], &r.u[2], &r.u[3], 1);
    o.conv(&u.last, &r.u[3], &r.out, Res::None);
}
