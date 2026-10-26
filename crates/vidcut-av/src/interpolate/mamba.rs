use std::path::Path;

use super::ema::{Head, Loader, Norm, Unet};
use super::ops::{Act, Conv, Deconv, Ops, Res, Scan, Shape, View, tensor};
use crate::compute::{Gpu, bytes};
use crate::error::Result;
use crate::weights::Torch;

const STATE: u32 = 16;
const DEPTH: usize = 3;

struct Vss {
    c: u32,
    rank: u32,
    ln1: Norm,
    skip: wgpu::Buffer,
    in_proj: Conv,
    dw: (wgpu::Buffer, wgpu::Buffer),
    x_proj: Conv,
    dtw: wgpu::Buffer,
    dtb: wgpu::Buffer,
    a: wgpu::Buffer,
    ds: wgpu::Buffer,
    out_norm: Norm,
    out_proj: Conv,
    skip2: wgpu::Buffer,
    ln2: Norm,
    cab1: Conv,
    cab2: Conv,
    ca1: Conv,
    ca2: Conv,
}

struct Local {
    scale: u32,
    conv0: Vec<Conv>,
    block: Vec<Conv>,
    last: Deconv,
}

pub(super) struct Mamba {
    stem: Vec<Conv>,
    embed1: Conv,
    block1: Vec<Conv>,
    embed2: Conv,
    block2: Vec<Conv>,
    norm3: Norm,
    embed3: Conv,
    stage3: Vec<Vss>,
    norm4: Norm,
    embed4: Conv,
    stage4: Vec<Vss>,
    heads: Vec<Head>,
    local: Vec<Local>,
    pub(super) unet: Unet,
}

fn vss(l: &Loader, name: &str, c: u32) -> Result<Vss> {
    let at = |s: &str| format!("{name}.{s}");
    let d = 2 * c;
    let rank = c.div_ceil(16);
    let width = rank + 2 * STATE;
    let (cu, du, ru) = (c as usize, d as usize, rank as usize);
    let xw = l.floats(
        &at("self_attention.x_proj_weight"),
        &[4, width as usize, du],
    )?;
    let x_proj = super::ops::conv(
        l.gpu,
        Shape::square(d, 4 * width, 1, 1, 0),
        &xw,
        None,
        Act::None,
        None,
    );
    let logs = l.floats(&at("self_attention.A_logs"), &[4 * du, STATE as usize])?;
    let a: Vec<f32> = logs.iter().map(|v| -v.exp()).collect();
    let small = c / 30;
    let third = c / 3;
    Ok(Vss {
        c,
        rank,
        ln1: l.norm(&at("ln_1"), c, 1e-5)?,
        skip: l.buffer(&at("skip_scale"), &[cu])?,
        in_proj: l.linear(&at("self_attention.in_proj"), c, 2 * d, false, Act::None)?,
        dw: (
            l.buffer(&at("self_attention.conv2d.weight"), &[du, 1, 3, 3])?,
            l.buffer(&at("self_attention.conv2d.bias"), &[du])?,
        ),
        x_proj,
        dtw: l.buffer(&at("self_attention.dt_projs_weight"), &[4, du, ru])?,
        dtb: l.buffer(&at("self_attention.dt_projs_bias"), &[4, du])?,
        a: l.gpu.upload(&bytes(&a)),
        ds: l.buffer(&at("self_attention.Ds"), &[4 * du])?,
        out_norm: l.norm(&at("self_attention.out_norm"), d, 1e-5)?,
        out_proj: l.linear(&at("self_attention.out_proj"), d, c, false, Act::None)?,
        skip2: l.buffer(&at("skip_scale2"), &[cu])?,
        ln2: l.norm(&at("ln_2"), c, 1e-5)?,
        cab1: l.conv(
            &at("conv_blk.cab.0"),
            Shape::square(c, third, 3, 1, 1),
            Act::Gelu,
            None,
        )?,
        cab2: l.conv(
            &at("conv_blk.cab.2"),
            Shape::square(third, c, 3, 1, 1),
            Act::None,
            None,
        )?,
        ca1: l.conv(
            &at("conv_blk.cab.3.attention.1"),
            Shape::square(c, small, 1, 1, 0),
            Act::Relu,
            None,
        )?,
        ca2: l.conv(
            &at("conv_blk.cab.3.attention.3"),
            Shape::square(small, c, 1, 1, 0),
            Act::Sigmoid,
            None,
        )?,
    })
}

fn local(l: &Loader, i: usize) -> Result<Local> {
    let at = |s: &str| format!("local_block.{i}.{s}");
    let prelu = |name: &str, cin: u32, cout: u32, stride: u32| {
        l.conv(
            &at(&format!("{name}.0")),
            Shape::square(cin, cout, 3, stride, 1),
            Act::None,
            Some(&at(&format!("{name}.1"))),
        )
    };
    Ok(Local {
        scale: 2 - i as u32,
        conv0: vec![prelu("conv0.0", 17, 64, 2)?, prelu("conv0.1", 64, 128, 2)?],
        block: (0..8)
            .map(|j| prelu(&format!("convblock.{j}"), 128, 128, 1))
            .collect::<Result<Vec<_>>>()?,
        last: l.deconv(&at("lastconv"), 128, 5, None)?,
    })
}

pub(super) fn load(gpu: &Gpu, path: &Path, title: &'static str) -> Result<Mamba> {
    let l = Loader {
        gpu,
        t: Torch::open(path, title)?,
        prefix: "module.",
    };
    let fb = |s: &str| format!("feature_bone.{s}");
    let down = |name: &str, cin: u32, cout: u32| {
        l.conv(
            &fb(&format!("{name}.0")),
            Shape::square(cin, cout, 3, 2, 1),
            Act::None,
            Some(&fb(&format!("{name}.1"))),
        )
    };
    let stage = |name: &str, c: u32| -> Result<Vec<Vss>> {
        (0..DEPTH)
            .map(|j| vss(&l, &fb(&format!("{name}.blocks.{j}")), c))
            .collect()
    };
    let embed = |name: &str, cin: u32, cout: u32| {
        l.conv(
            &fb(&format!("{name}.proj")),
            Shape::square(cin, cout, 3, 2, 1),
            Act::None,
            None,
        )
    };
    Ok(Mamba {
        stem: l.convs(&fb("block0.conv"), 3, 32)?,
        embed1: down("patch_embed1", 32, 64)?,
        block1: l.convs(&fb("block1.conv"), 64, 64)?,
        embed2: down("patch_embed2", 64, 128)?,
        block2: l.convs(&fb("block2.conv"), 128, 128)?,
        norm3: l.norm(&fb("patch_embed3.norm"), 128, 1e-5)?,
        embed3: embed("patch_embed3", 128, 256)?,
        stage3: stage("block3", 256)?,
        norm4: l.norm(&fb("patch_embed4.norm"), 256, 1e-5)?,
        embed4: embed("patch_embed4", 256, 512)?,
        stage4: stage("block4", 512)?,
        heads: vec![l.head(0, 71, 128)?, l.head(1, 50, 128)?],
        local: vec![local(&l, 0)?, local(&l, 1)?],
        unet: l.unet(64)?,
    })
}

struct Room {
    normed: View,
    xz: View,
    xa: View,
    proj: View,
    ys: View,
    sum: View,
    yn: View,
    zs: View,
    out: View,
    cab: View,
    pooled: View,
    squeezed: View,
    weights: View,
}

fn room(gpu: &Gpu, h: u32, w: u32, c: u32) -> Room {
    let d = 2 * c;
    let width = c.div_ceil(16) + 2 * STATE;
    let at = |ch: u32| tensor(gpu, 2, h, w, ch);
    let ys = tensor(gpu, 8, h, w, d);
    Room {
        normed: at(c),
        xz: at(2 * d),
        xa: at(d),
        proj: at(4 * width),
        ys: View { n: 2, ..ys },
        sum: at(d),
        yn: at(d),
        zs: at(d),
        out: at(c),
        cab: at(c / 3),
        pooled: tensor(gpu, 2, 1, 1, c),
        squeezed: tensor(gpu, 2, 1, 1, c / 30),
        weights: tensor(gpu, 2, 1, 1, c),
    }
}

pub(super) struct Features {
    pub(super) af: Vec<View>,
}

pub(super) struct Scratch {
    tmp: Vec<View>,
    pre: [View; 2],
    rooms: [Room; 2],
}

pub(super) fn features_for(gpu: &Gpu, (w, h): (u32, u32)) -> Features {
    let at = |s: u32, c: u32| tensor(gpu, 2, h / s, w / s, c);
    Features {
        af: vec![at(1, 32), at(2, 64), at(4, 128), at(8, 256), at(16, 512)],
    }
}

pub(super) fn scratch_for(gpu: &Gpu, (w, h): (u32, u32)) -> Scratch {
    let at = |s: u32, c: u32| tensor(gpu, 2, h / s, w / s, c);
    Scratch {
        tmp: vec![at(1, 32), at(2, 64), at(4, 128)],
        pre: [at(4, 128), at(8, 256)],
        rooms: [room(gpu, h / 8, w / 8, 256), room(gpu, h / 16, w / 16, 512)],
    }
}

pub(super) struct Flow {
    pub(super) flow: View,
    pub(super) mask: View,
    m4: View,
    up4: View,
    x4: View,
    c: (View, View),
    out4: View,
    big: View,
    m3: View,
    up3: View,
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
        m4: one(16, 1024),
        up4: one(8, 256),
        x4: one(4, 71),
        c: (one(4, 128), one(4, 128)),
        out4: one(4, 5),
        big: one(1, 5),
        m3: one(8, 512),
        up3: one(4, 128),
        x2: one(2, 50),
        c2: (one(2, 128), one(2, 128)),
        out2: one(2, 5),
        warped: one(1, 6),
    }
}

pub(super) struct Locals {
    x: [View; 2],
    a: [View; 2],
    b: [View; 2],
    c: [View; 2],
    d: [View; 2],
    out: [View; 2],
    big: View,
    warped: View,
}

pub(super) fn locals_for(gpu: &Gpu, (w, h): (u32, u32)) -> Locals {
    let one = |s: u32, c: u32| tensor(gpu, 1, h / s, w / s, c);
    Locals {
        x: [one(2, 17), one(1, 17)],
        a: [one(4, 64), one(2, 64)],
        b: [one(8, 128), one(4, 128)],
        c: [one(8, 128), one(4, 128)],
        d: [one(8, 128), one(4, 128)],
        out: [one(4, 5), one(2, 5)],
        big: one(1, 5),
        warped: one(1, 6),
    }
}

impl Mamba {
    fn vss(&self, o: &mut Ops, v: &Vss, x: &View, r: &Room) {
        let d = 2 * v.c;
        o.norm(&v.ln1.gamma, &v.ln1.beta, v.ln1.eps, x, &r.normed);
        o.conv(&v.in_proj, &r.normed, &r.xz, Res::None);
        o.dwconv(&v.dw.0, &v.dw.1, 3, Act::Silu, &r.xz.part(0, d), &r.xa);
        o.conv(&v.x_proj, &r.xa, &r.proj, Res::None);
        let s = Scan {
            dtw: &v.dtw,
            dtb: &v.dtb,
            a: &v.a,
            d: &v.ds,
            rank: v.rank,
        };
        o.scan(&s, &r.xa, &r.proj, &r.ys);
        let plane = 2 * r.ys.h * r.ys.w * d;
        o.copy(&r.ys, &r.sum, 1.0, 0.0, Act::None);
        for k in 1..4 {
            let part = View {
                base: k * plane,
                ..r.ys.clone()
            };
            o.add(&part, &r.sum);
        }
        o.norm(
            &v.out_norm.gamma,
            &v.out_norm.beta,
            v.out_norm.eps,
            &r.sum,
            &r.yn,
        );
        o.copy(&r.xz.part(d, d), &r.zs, 1.0, 0.0, Act::Silu);
        o.mul(&r.zs, &r.yn);
        o.conv(&v.out_proj, &r.yn, &r.out, Res::None);
        o.scale_add(&r.out, x, &v.skip);
        o.norm(&v.ln2.gamma, &v.ln2.beta, v.ln2.eps, x, &r.normed);
        o.conv(&v.cab1, &r.normed, &r.cab, Res::None);
        o.conv(&v.cab2, &r.cab, &r.out, Res::None);
        o.pool(&r.out, &r.pooled);
        o.conv(&v.ca1, &r.pooled, &r.squeezed, Res::None);
        o.conv(&v.ca2, &r.squeezed, &r.weights, Res::None);
        o.mul_channels(&r.out, &r.weights.buf);
        o.scale_add(&r.out, x, &v.skip2);
    }

    pub(super) fn features(&self, o: &mut Ops, img: &View, f: &Features, x: &Scratch) {
        let af = &f.af;
        o.conv(&self.stem[0], img, &x.tmp[0], Res::None);
        o.conv(&self.stem[1], &x.tmp[0], &af[0], Res::None);
        o.conv(&self.embed1, &af[0], &x.tmp[1], Res::None);
        o.conv(&self.block1[0], &x.tmp[1], &af[1], Res::None);
        o.conv(&self.block1[1], &af[1], &x.tmp[1], Res::None);
        o.copy(&x.tmp[1], &af[1], 1.0, 0.0, Act::None);
        o.conv(&self.embed2, &af[1], &x.tmp[2], Res::None);
        o.conv(&self.block2[0], &x.tmp[2], &af[2], Res::None);
        o.conv(&self.block2[1], &af[2], &x.tmp[2], Res::None);
        o.copy(&x.tmp[2], &af[2], 1.0, 0.0, Act::None);
        let n = &self.norm3;
        o.norm(&n.gamma, &n.beta, n.eps, &af[2], &x.pre[0]);
        o.conv(&self.embed3, &x.pre[0], &af[3], Res::None);
        for v in &self.stage3 {
            self.vss(o, v, &af[3], &x.rooms[0]);
        }
        let n = &self.norm4;
        o.norm(&n.gamma, &n.beta, n.eps, &af[3], &x.pre[1]);
        o.conv(&self.embed4, &x.pre[1], &af[4], Res::None);
        for v in &self.stage4 {
            self.vss(o, v, &af[4], &x.rooms[1]);
        }
    }

    pub(super) fn flow(&self, o: &mut Ops, img: &View, af: &[View], fl: &Flow, t: f32) {
        let (img0, img1) = (img.frame(0), img.frame(1));
        let both = |o: &mut Ops, a: &View, dst: &View| {
            o.copy(&a.frame(0), &dst.part(0, a.c), 1.0, 0.0, Act::None);
            o.copy(&a.frame(1), &dst.part(a.c, a.c), 1.0, 0.0, Act::None);
        };
        both(o, &af[4], &fl.m4);
        o.shuffle(&fl.m4, &fl.up4, 2);
        o.shuffle(&fl.up4, &fl.x4.part(0, 64), 2);
        o.scaled(&img0, &fl.x4.part(64, 3), 0.25, 1.0, 0);
        o.scaled(&img1, &fl.x4.part(67, 3), 0.25, 1.0, 0);
        o.fill(&fl.x4.part(70, 1), t);
        let h = &self.heads[0];
        o.conv(&h.convs[0], &fl.x4, &fl.c.0, Res::None);
        o.conv(&h.convs[1], &fl.c.0, &fl.c.1, Res::None);
        o.conv(&h.convs[2], &fl.c.1, &fl.out4, Res::None);
        o.scaled(&fl.out4, &fl.big, 4.0, 4.0, 4);
        o.copy(&fl.big.part(0, 4), &fl.flow, 1.0, 0.0, Act::None);
        o.copy(&fl.big.part(4, 1), &fl.mask, 1.0, 0.0, Act::None);
        o.warp(&img0, &fl.flow.part(0, 2), &fl.warped.part(0, 3));
        o.warp(&img1, &fl.flow.part(2, 2), &fl.warped.part(3, 3));
        both(o, &af[3], &fl.m3);
        o.shuffle(&fl.m3, &fl.up3, 2);
        o.shuffle(&fl.up3, &fl.x2.part(0, 32), 2);
        o.scaled(&img0, &fl.x2.part(32, 3), 0.5, 1.0, 0);
        o.scaled(&img1, &fl.x2.part(35, 3), 0.5, 1.0, 0);
        o.scaled(&fl.warped, &fl.x2.part(38, 6), 0.5, 1.0, 0);
        o.scaled(&fl.mask, &fl.x2.part(44, 1), 0.5, 1.0, 0);
        o.fill(&fl.x2.part(45, 1), t);
        o.scaled(&fl.flow, &fl.x2.part(46, 4), 0.5, 0.5, 4);
        let h = &self.heads[1];
        o.conv(&h.convs[0], &fl.x2, &fl.c2.0, Res::None);
        o.conv(&h.convs[1], &fl.c2.0, &fl.c2.1, Res::None);
        o.conv(&h.convs[2], &fl.c2.1, &fl.out2, Res::None);
        o.scaled(&fl.out2, &fl.big, 2.0, 2.0, 4);
        o.add(&fl.big.part(0, 4), &fl.flow);
        o.add(&fl.big.part(4, 1), &fl.mask);
    }

    pub(super) fn locals(&self, o: &mut Ops, img: &View, flow: &View, mask: &View, l: &Locals) {
        let (img0, img1) = (img.frame(0), img.frame(1));
        for (i, b) in self.local.iter().enumerate() {
            let s = b.scale as f32;
            o.warp(&img0, &flow.part(0, 2), &l.warped.part(0, 3));
            o.warp(&img1, &flow.part(2, 2), &l.warped.part(3, 3));
            let x = &l.x[i];
            o.scaled(&img0, &x.part(0, 3), 1.0 / s, 1.0, 0);
            o.scaled(&img1, &x.part(3, 3), 1.0 / s, 1.0, 0);
            o.scaled(&l.warped, &x.part(6, 6), 1.0 / s, 1.0, 0);
            o.scaled(mask, &x.part(12, 1), 1.0 / s, 1.0, 0);
            o.scaled(flow, &x.part(13, 4), 1.0 / s, 1.0 / s, 4);
            o.conv(&b.conv0[0], x, &l.a[i], Res::None);
            o.conv(&b.conv0[1], &l.a[i], &l.b[i], Res::None);
            let mut from = l.b[i].clone();
            for (j, c) in b.block.iter().enumerate() {
                let to = if j % 2 == 0 {
                    l.c[i].clone()
                } else {
                    l.d[i].clone()
                };
                if j + 1 == b.block.len() {
                    o.conv(c, &from, &to, Res::After(&l.b[i]));
                } else {
                    o.conv(c, &from, &to, Res::None);
                }
                from = to;
            }
            o.deconv(&b.last, &from, &l.out[i], 1);
            o.scaled(&l.out[i], &l.big, 2.0 * s, 2.0 * s, 4);
            o.add(&l.big.part(0, 4), flow);
            o.add(&l.big.part(4, 1), mask);
        }
    }
}
