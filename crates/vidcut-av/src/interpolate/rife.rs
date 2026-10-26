use std::path::Path;

use super::ops::{self, Act, Conv, Deconv, Ops, Res, Shape, View, tensor};
use crate::compute::Gpu;
use crate::error::Result;
use crate::weights::Torch;

const LEAK: Act = Act::Leaky(0.2);
const WIDTHS: [u32; 5] = [192, 128, 96, 64, 32];
const SCALES: [f32; 5] = [16.0, 8.0, 4.0, 2.0, 1.0];

struct Block {
    c: u32,
    conv0a: Conv,
    conv0b: Conv,
    res: Vec<Conv>,
    last: Deconv,
}

pub(super) struct Rife {
    head: [Conv; 3],
    head_up: Deconv,
    blocks: Vec<Block>,
}

fn weights(t: &Torch, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
    t.floats(&format!("module.{name}"), shape)
}

fn conv(
    gpu: &Gpu,
    t: &Torch,
    name: &str,
    s: Shape,
    act: Act,
    beta: Option<&[f32]>,
) -> Result<Conv> {
    let (kh, kw) = s.k;
    let shape = [s.cout as usize, s.cin as usize, kh as usize, kw as usize];
    let mut w = weights(t, &format!("{name}.weight"), &shape)?;
    let mut b = weights(t, &format!("{name}.bias"), &[s.cout as usize])?;
    if let Some(beta) = beta {
        let each = w.len() / s.cout as usize;
        for (co, k) in beta.iter().enumerate() {
            w[co * each..(co + 1) * each]
                .iter_mut()
                .for_each(|v| *v *= k);
            b[co] *= k;
        }
    }
    Ok(ops::conv(gpu, s, &w, Some(&b), act, None))
}

fn deconv(gpu: &Gpu, t: &Torch, name: &str, cin: u32, cout: u32) -> Result<Deconv> {
    let w = weights(
        t,
        &format!("{name}.weight"),
        &[cin as usize, cout as usize, 4, 4],
    )?;
    let b = weights(t, &format!("{name}.bias"), &[cout as usize])?;
    Ok(ops::deconv(gpu, cin, cout, &w, Some(&b), Act::None, None))
}

pub(super) fn load(gpu: &Gpu, path: &Path, title: &'static str) -> Result<Rife> {
    let t = Torch::open(path, title)?;
    let head = [
        conv(
            gpu,
            &t,
            "encode.cnn0",
            Shape::square(3, 16, 3, 2, 1),
            LEAK,
            None,
        )?,
        conv(
            gpu,
            &t,
            "encode.cnn1",
            Shape::square(16, 16, 3, 1, 1),
            LEAK,
            None,
        )?,
        conv(
            gpu,
            &t,
            "encode.cnn2",
            Shape::square(16, 16, 3, 1, 1),
            LEAK,
            None,
        )?,
    ];
    let head_up = deconv(gpu, &t, "encode.cnn3", 16, 4)?;
    let mut blocks = Vec::new();
    for (i, &c) in WIDTHS.iter().enumerate() {
        let cin = if i == 0 { 15 } else { 28 };
        let at = |s: &str| format!("block{i}.{s}");
        let mut res = Vec::new();
        for j in 0..8 {
            let beta = weights(
                &t,
                &at(&format!("convblock.{j}.beta")),
                &[1, c as usize, 1, 1],
            )?;
            res.push(conv(
                gpu,
                &t,
                &at(&format!("convblock.{j}.conv")),
                Shape::square(c, c, 3, 1, 1),
                LEAK,
                Some(&beta),
            )?);
        }
        blocks.push(Block {
            c,
            conv0a: conv(
                gpu,
                &t,
                &at("conv0.0.0"),
                Shape::square(cin, c / 2, 3, 2, 1),
                LEAK,
                None,
            )?,
            conv0b: conv(
                gpu,
                &t,
                &at("conv0.1.0"),
                Shape::square(c / 2, c, 3, 2, 1),
                LEAK,
                None,
            )?,
            res,
            last: deconv(gpu, &t, &at("lastconv.0"), c, 52)?,
        });
    }
    Ok(Rife {
        head,
        head_up,
        blocks,
    })
}

struct Level {
    x: View,
    a: View,
    b: View,
    c: View,
    out: View,
}

pub(super) struct Pair {
    pub(super) img: View,
    pub(super) feat: View,
    head: (View, View),
    levels: Vec<Level>,
    pub(super) warped: View,
    pub(super) up: View,
    pub(super) flow: View,
    scale: f32,
}

pub(super) fn padded(size: (u32, u32), scale: f32) -> (u32, u32) {
    let step = 128u32.max((128.0 / scale) as u32);
    (size.0.next_multiple_of(step), size.1.next_multiple_of(step))
}

impl Rife {
    pub(super) fn pair(&self, gpu: &Gpu, (w, h): (u32, u32), scale: f32) -> Pair {
        let mut levels = Vec::new();
        for (i, b) in self.blocks.iter().enumerate() {
            let s = SCALES[i] / scale;
            let (lw, lh) = ((w as f32 / s) as u32, (h as f32 / s) as u32);
            let cin = if i == 0 { 15 } else { 28 };
            levels.push(Level {
                x: tensor(gpu, 1, lh, lw, cin),
                a: tensor(gpu, 1, lh / 2, lw / 2, b.c / 2),
                b: tensor(gpu, 1, lh / 4, lw / 4, b.c),
                c: tensor(gpu, 1, lh / 4, lw / 4, b.c),
                out: tensor(gpu, 1, lh, lw, 13),
            });
        }
        Pair {
            img: tensor(gpu, 2, h, w, 3),
            feat: tensor(gpu, 2, h, w, 4),
            head: (
                tensor(gpu, 2, h / 2, w / 2, 16),
                tensor(gpu, 2, h / 2, w / 2, 16),
            ),
            levels,
            warped: tensor(gpu, 1, h, w, 14),
            up: tensor(gpu, 1, h, w, 13),
            flow: tensor(gpu, 1, h, w, 4),
            scale,
        }
    }

    pub(super) fn features(&self, o: &mut Ops, p: &Pair) {
        o.conv(&self.head[0], &p.img, &p.head.0, Res::None);
        o.conv(&self.head[1], &p.head.0, &p.head.1, Res::None);
        o.conv(&self.head[2], &p.head.1, &p.head.0, Res::None);
        o.deconv(&self.head_up, &p.head.0, &p.feat, 1);
    }

    pub(super) fn frame(&self, o: &mut Ops, p: &Pair, t: f32) {
        let (img0, img1) = (p.img.frame(0), p.img.frame(1));
        let (f0, f1) = (p.feat.frame(0), p.feat.frame(1));
        for (i, b) in self.blocks.iter().enumerate() {
            let s = SCALES[i] / p.scale;
            let l = &p.levels[i];
            if i == 0 {
                o.scaled(&img0, &l.x.part(0, 3), 1.0 / s, 1.0, 0);
                o.scaled(&img1, &l.x.part(3, 3), 1.0 / s, 1.0, 0);
                o.scaled(&f0, &l.x.part(6, 4), 1.0 / s, 1.0, 0);
                o.scaled(&f1, &l.x.part(10, 4), 1.0 / s, 1.0, 0);
                o.fill(&l.x.part(14, 1), t);
            } else {
                o.warp(&f0, &p.flow.part(0, 2), &p.warped.part(6, 4));
                o.warp(&f1, &p.flow.part(2, 2), &p.warped.part(10, 4));
                o.scaled(&p.warped, &l.x.part(0, 14), 1.0 / s, 1.0, 0);
                o.fill(&l.x.part(14, 1), t);
                o.scaled(&p.up.part(4, 9), &l.x.part(15, 9), 1.0 / s, 1.0, 0);
                o.scaled(&p.flow, &l.x.part(24, 4), 1.0 / s, 1.0 / s, 4);
            }
            o.conv(&b.conv0a, &l.x, &l.a, Res::None);
            o.conv(&b.conv0b, &l.a, &l.b, Res::None);
            for (j, r) in b.res.iter().enumerate() {
                let (from, to) = if j % 2 == 0 {
                    (&l.b, &l.c)
                } else {
                    (&l.c, &l.b)
                };
                o.conv(r, from, to, Res::Before(from));
            }
            o.deconv(&b.last, &l.b, &l.out, 2);
            o.scaled(&l.out, &p.up, s, s, 4);
            if i == 0 {
                o.copy(&p.up.part(0, 4), &p.flow, 1.0, 0.0, Act::None);
            } else {
                o.add(&p.up.part(0, 4), &p.flow);
            }
            o.warp(&img0, &p.flow.part(0, 2), &p.warped.part(0, 3));
            o.warp(&img1, &p.flow.part(2, 2), &p.warped.part(3, 3));
        }
    }

    pub(super) fn finish(&self, o: &mut Ops, p: &Pair, size: (u32, u32), out: &wgpu::Buffer) {
        o.finish(
            &p.warped.part(0, 3),
            &p.warped.part(3, 3),
            &p.up.part(4, 1),
            None,
            (0, 0),
            size,
            out,
        );
    }
}
