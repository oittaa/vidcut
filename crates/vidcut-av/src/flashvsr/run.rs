use std::path::PathBuf;

use super::gemm::{self, Act, Gemm, Load, Store};
use super::gpu::{Gpu, floats};
use super::kernels::{self, Attend, DecInput, Grid, Norm, Resize, Unshuffle};
use super::model::{Block, Linear, MemBlock, Model};
use super::{DIM, FFN, dump, resize};
use crate::compute::{bytes, words};
use crate::error::{Error, Result};

const RING: u64 = 20;
const SLAB: u32 = 16384;
const BAND: u32 = 64;
const QUERY_GROUP: u32 = 32;
const GLOBAL_RANGE: u32 = 11;
const LOCAL_RANGE: u32 = 3;
const GLOBAL_KEEP: u32 = 3;
const LOCAL_KEEP: u32 = 1;
const TOPK_RATIO: f64 = 2.5;
const SEED: u64 = 1;

#[derive(Debug, Clone, Copy)]
pub(super) struct Geometry {
    pub(super) sw: u32,
    pub(super) sh: u32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) wm: u32,
    pub(super) hm: u32,
    pub(super) wt: u32,
    pub(super) ht: u32,
    pub(super) nw: u32,
    pub(super) nh: u32,
    pub(super) s: u32,
    pub(super) tf: u32,
    pub(super) t: u32,
    pub(super) qbs: u32,
    pub(super) w8: u32,
    pub(super) h8: u32,
    pub(super) topk: u64,
}

impl Geometry {
    pub(super) fn new((sw, sh): (u32, u32), (w, h): (u32, u32)) -> Self {
        let wm = w.next_multiple_of(128);
        let hm = h.next_multiple_of(128);
        let (wt, ht) = (wm / 16, hm / 16);
        let (nw, nh) = (wt / 8, ht / 8);
        let s = nw * nh;
        let tf = wt * ht;
        Self {
            sw,
            sh,
            w,
            h,
            wm,
            hm,
            wt,
            ht,
            nw,
            nh,
            s,
            tf,
            t: tf * 4,
            qbs: s * 2,
            w8: wm / 8,
            h8: hm / 8,
            topk: ((f64::from(s) * f64::from(s) * TOPK_RATIO) as u64).saturating_sub(1),
        }
    }

    fn p8(&self) -> u32 {
        self.w8 * self.h8
    }

    fn plane(&self) -> u32 {
        self.wm * self.hm
    }

    fn candidates(&self, range: u32, windows: u32) -> u64 {
        let half = (range / 2) as i64;
        let span = |at: i64, n: i64| ((at + half).min(n - 1) - (at - half).max(0) + 1) as u64;
        let mut per_window = 0u64;
        for r in 0..i64::from(self.nh) {
            for c in 0..i64::from(self.nw) {
                per_window += span(r, i64::from(self.nh)) * span(c, i64::from(self.nw));
            }
        }
        per_window * 2 * u64::from(windows)
    }

    fn prunes(&self, range: u32, windows: u32) -> bool {
        self.candidates(range, windows) > self.topk
    }
}

struct Cache {
    k: wgpu::Buffer,
    v: wgpu::Buffer,
    pooled: wgpu::Buffer,
    ring: Vec<u32>,
    keep: u32,
    range: u32,
}

struct Mems {
    a: Vec<wgpu::Buffer>,
    b: Vec<wgpu::Buffer>,
    c: Vec<wgpu::Buffer>,
}

struct Tables {
    hb: wgpu::Buffer,
    hc: wgpu::Buffer,
    hk: u32,
    vb: wgpu::Buffer,
    vc: wgpu::Buffer,
    vk: u32,
}

pub(super) struct Stream {
    pub(super) g: Geometry,
    frames: wgpu::Buffer,
    tables: Tables,
    caches: Vec<Cache>,
    in1: wgpu::Buffer,
    in2: wgpu::Buffer,
    map1: [u32; 6],
    map2: [u32; 4],
    mems: Mems,
    fed: u64,
    chunk: u64,
    dump: Option<PathBuf>,
}

struct Enc<'a> {
    gpu: &'a Gpu,
    enc: Option<wgpu::CommandEncoder>,
}

impl<'a> Enc<'a> {
    fn new(gpu: &'a Gpu) -> Self {
        Self { gpu, enc: None }
    }

    fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        self.enc
            .get_or_insert_with(|| self.gpu.device.create_command_encoder(&Default::default()))
    }

    fn pass<R>(&mut self, f: impl FnOnce(&mut wgpu::ComputePass) -> R) -> R {
        let mut pass = self.encoder().begin_compute_pass(&Default::default());
        f(&mut pass)
    }

    fn copy(&mut self, src: &wgpu::Buffer, from: u64, dst: &wgpu::Buffer, to: u64, size: u64) {
        self.encoder()
            .copy_buffer_to_buffer(src, from, dst, to, size);
    }

    fn submit(&mut self) {
        if let Some(enc) = self.enc.take() {
            self.gpu.queue.submit([enc.finish()]);
        }
    }
}

fn params(values: &[u32]) -> [u32; 16] {
    let mut p = [0u32; 16];
    p[..values.len()].copy_from_slice(values);
    p
}

fn rows(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    l: &Linear,
    m: u32,
    (a, out): (&wgpu::Buffer, &wgpu::Buffer),
    (store, aux): (Store, Option<&wgpu::Buffer>),
    extra: &[(usize, u32)],
) {
    let mut p = params(&[l.k / 2]);
    for (i, v) in extra {
        p[*i] = *v;
    }
    gemm::run(
        gpu,
        pass,
        &Gemm {
            m,
            n: l.n,
            k: l.k,
            load: Load::Rows,
            store,
            params: p,
            weights: &l.w,
            bias: &l.b,
            a,
            a2: None,
            out,
            aux,
        },
    );
}

struct Plane<'a> {
    w: u32,
    h: u32,
    y0: u32,
    rows: u32,
    src: &'a wgpu::Buffer,
    cin: u32,
    stride: u32,
    offset: u32,
    two: Option<&'a wgpu::Buffer>,
    up: bool,
    sy0: u32,
}

fn plane(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    l: &Linear,
    p: &Plane,
    out: &wgpu::Buffer,
    store: Store,
    aux: Option<&wgpu::Buffer>,
) {
    let ctot = l.k / 9;
    gemm::run(
        gpu,
        pass,
        &Gemm {
            m: p.rows * p.w,
            n: l.n,
            k: l.k,
            load: Load::Plane {
                two: p.two.is_some(),
            },
            store,
            params: params(&[
                p.w,
                p.h,
                p.y0,
                p.cin,
                p.stride,
                p.offset,
                p.cin / 2,
                u32::from(p.up),
                p.sy0,
                ctot,
            ]),
            weights: &l.w,
            bias: &l.b,
            a: p.src,
            a2: p.two,
            out,
            aux,
        },
    );
}

fn noise(chunk: u64, count: usize) -> Vec<f32> {
    let mut s = SEED ^ chunk.wrapping_mul(0xD1B5_4A32_D192_ED03);
    let mut next = || {
        s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut unit = || ((next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
    let mut out = Vec::with_capacity(count + 1);
    while out.len() < count {
        let r = (-2.0 * unit().ln()).sqrt();
        let a = std::f64::consts::TAU * unit();
        out.push((r * a.cos()) as f32);
        out.push((r * a.sin()) as f32);
    }
    out.truncate(count);
    out
}

fn rope(g: &Geometry, chunk: u64) -> Vec<f32> {
    let axis = |pairs: u32, positions: std::ops::Range<u64>, out: &mut Vec<f32>| {
        for pos in positions {
            for i in 0..pairs {
                let inv = 10000f32.powf(-(i as f32) / pairs as f32);
                let angle = f64::from(pos as f32 * inv);
                out.push(angle.cos() as f32);
                out.push(angle.sin() as f32);
            }
        }
    };
    let mut t = Vec::new();
    axis(22, chunk * 4..chunk * 4 + 4, &mut t);
    axis(21, 0..u64::from(g.ht), &mut t);
    axis(21, 0..u64::from(g.wt), &mut t);
    t
}

impl Stream {
    pub(super) fn new(gpu: &Gpu, g: Geometry, dump: Option<PathBuf>) -> Result<Self> {
        let (hb, hc, hk) = resize::coefficients(g.sw, g.w);
        let (vb, vc, vk) = resize::coefficients(g.sh, g.h);
        let tables = Tables {
            hb: gpu.upload(&ints(&hb)),
            hc: gpu.upload(&ints(&hc)),
            hk,
            vb: gpu.upload(&ints(&vb)),
            vc: gpu.upload(&ints(&vc)),
            vk,
        };
        let caches = (0..super::LAYERS)
            .map(|i| {
                let global = super::GLOBAL.contains(&i);
                let keep = if global { GLOBAL_KEEP } else { LOCAL_KEEP };
                let tokens = u64::from(keep * g.s * 128);
                Cache {
                    k: gpu.buffer((tokens * DIM as u64 * 2) as usize),
                    v: gpu.buffer((tokens * DIM as u64 * 2) as usize),
                    pooled: gpu.buffer((keep * g.s * 12 * 128 * 4) as usize),
                    ring: Vec::new(),
                    keep,
                    range: if global { GLOBAL_RANGE } else { LOCAL_RANGE },
                }
            })
            .collect();
        let p8 = g.p8() as usize;
        let mem = |p: usize, n: usize| (0..3).map(|_| gpu.buffer(p * n * 2)).collect();
        Ok(Self {
            g,
            frames: gpu.buffer((RING * u64::from(g.plane()) * 4) as usize),
            tables,
            caches,
            in1: gpu.buffer((6 * g.tf * 768 * 2) as usize),
            in2: gpu.buffer((4 * g.tf * 2048 * 2) as usize),
            map1: [0, 1, 2, 3, 4, 5],
            map2: [0, 1, 2, 3],
            mems: Mems {
                a: mem(p8, 512),
                b: mem(p8 * 4, 256),
                c: mem(p8 * 16, 128),
            },
            fed: 0,
            chunk: 0,
            dump,
        })
    }

    fn slot(&self, frame: u64) -> u32 {
        (frame % RING) as u32
    }

    fn need(&self) -> u64 {
        self.chunk * 16 + 17
    }

    pub(super) fn push(&mut self, gpu: &Gpu, model: &Model, rgb: &[u8]) -> Result<Vec<Vec<u8>>> {
        let g = self.g;
        if rgb.len() != (g.sw * g.sh * 3) as usize {
            return Err(Error::Invalid(format!(
                "the AI Best upscaler got a picture of {} bytes for {}x{}",
                rgb.len(),
                g.sw,
                g.sh
            )));
        }
        let rgba: Vec<u32> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| u32::from_le_bytes([p[0], p[1], p[2], 255]))
            .collect();
        let src = gpu.upload(&words(&rgba));
        let temp = gpu.buffer((g.sh * g.w * 4) as usize);
        let mut e = Enc::new(gpu);
        let t = &self.tables;
        let base = self.slot(self.fed) * g.plane();
        e.pass(|pass| {
            kernels::resize(
                gpu,
                pass,
                &Resize {
                    src: &src,
                    bounds: &t.hb,
                    coef: &t.hc,
                    out: &temp,
                    values: [g.sw, g.sh, g.w, g.sh, t.hk, 0, g.wm, g.hm, 0, 0, 0, 0],
                },
                (g.w, g.sh),
            );
            kernels::resize(
                gpu,
                pass,
                &Resize {
                    src: &temp,
                    bounds: &t.vb,
                    coef: &t.vc,
                    out: &self.frames,
                    values: [g.w, g.sh, g.w, g.h, t.vk, 1, g.wm, g.hm, base, 0, 0, 0],
                },
                (g.wm, g.hm),
            );
        });
        e.submit();
        if let Some(dir) = self.dump.as_ref() {
            let raw = gpu.read(&self.frames, u64::from(base) * 4, u64::from(g.plane()) * 4)?;
            let rgb: Vec<u8> = raw
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            dump::write_bytes(
                &dir.join(format!("input{:03}.npy", self.fed)),
                &[g.hm as usize, g.wm as usize, 3],
                &rgb,
            )?;
        }
        self.fed += 1;
        let mut out = Vec::new();
        while self.fed >= self.need() {
            out.extend(self.run_chunk(gpu, model)?);
        }
        Ok(out)
    }

    pub(super) fn finish(&mut self, gpu: &Gpu, model: &Model) -> Result<Vec<Vec<u8>>> {
        let real = self.fed;
        let mut out = Vec::new();
        if real == 0 {
            return Ok(out);
        }
        let plane = u64::from(self.g.plane()) * 4;
        while self.emitted() < real {
            let mut e = Enc::new(gpu);
            while self.fed < self.need() {
                let last = u64::from(self.slot(self.fed - 1)) * plane;
                let next = u64::from(self.slot(self.fed)) * plane;
                let scratch = gpu.buffer(plane as usize);
                e.copy(&self.frames, last, &scratch, 0, plane);
                e.copy(&scratch, 0, &self.frames, next, plane);
                e.submit();
                self.fed += 1;
            }
            out.extend(self.run_chunk(gpu, model)?);
        }
        let first = self.emitted() - out.len() as u64;
        out.truncate(real.saturating_sub(first) as usize);
        Ok(out)
    }

    fn emitted(&self) -> u64 {
        if self.chunk == 0 {
            0
        } else {
            self.chunk * 16 - 3
        }
    }

    fn run_chunk(&mut self, gpu: &Gpu, model: &Model) -> Result<Vec<Vec<u8>>> {
        let g = self.g;
        let k = self.chunk;
        let p8 = g.p8();
        let noise_v = noise(k, (16 * 4 * p8) as usize);
        let noise = gpu.upload(&bytes(&noise_v));
        let latents = gpu.buffer((16 * 4 * p8 * 4) as usize);
        {
            let work = Work::new(gpu, &g);
            let table = gpu.upload(&bytes(&rope(&g, k)));
            let mut e = Enc::new(gpu);
            e.pass(|pass| {
                gemm::run(
                    gpu,
                    pass,
                    &Gemm {
                        m: g.t,
                        n: DIM as u32,
                        k: 64,
                        load: Load::Patch,
                        store: Store::Full,
                        params: params(&[g.w8, g.h8, g.nh, g.nw]),
                        weights: &model.patch.w,
                        bias: &model.patch.b,
                        a: &noise,
                        a2: None,
                        out: &work.x,
                        aux: None,
                    },
                );
            });
            self.project(gpu, model, &work, &mut e);
            e.submit();
            self.dump_tensor(gpu, "x_in", &work.x, g.t, true)?;
            for (i, block) in model.blocks.iter().enumerate() {
                self.block(gpu, model, &work, &table, i, block)?;
                if matches!(i, 0 | 6 | 29) {
                    self.dump_tensor(gpu, &format!("x_block{i}"), &work.x, g.t, true)?;
                }
            }
            let mut e = Enc::new(gpu);
            e.pass(|pass| {
                kernels::layer_norm(
                    gpu,
                    pass,
                    &Norm {
                        x: &work.x,
                        mul: &model.head_mul,
                        add: &model.head_add,
                        out: &work.h,
                        rows: g.t,
                        first: 0,
                    },
                );
                rows(
                    gpu,
                    pass,
                    &model.head,
                    g.t,
                    (&work.h, &latents),
                    (Store::Head, Some(&noise)),
                    &[(12, g.w8), (13, g.h8), (14, g.nh), (15, g.nw)],
                );
            });
            e.submit();
        }
        if let Some(dir) = self.dump.as_ref() {
            let shape = [16, 4, g.h8 as usize, g.w8 as usize];
            dump::write(&dir.join(format!("chunk{k}_noise.npy")), &shape, &noise_v)?;
            let raw = gpu.read(&latents, 0, u64::from(16 * 4 * p8 * 4))?;
            dump::write(
                &dir.join(format!("chunk{k}_latents.npy")),
                &shape,
                &floats(&raw),
            )?;
        }
        let frames = self.decode(gpu, model, &latents)?;
        self.chunk += 1;
        Ok(frames)
    }

    fn dump_tensor(
        &self,
        gpu: &Gpu,
        name: &str,
        x: &wgpu::Buffer,
        tokens: u32,
        full: bool,
    ) -> Result<()> {
        let Some(dir) = self.dump.as_ref().filter(|_| self.chunk < 2) else {
            return Ok(());
        };
        let g = self.g;
        let size = u64::from(tokens) * DIM as u64 * if full { 4 } else { 2 };
        let raw = gpu.read(x, 0, size)?;
        let v = floats(&raw);
        let mut raster = vec![0f32; v.len()];
        for token in 0..tokens as usize {
            let b = token / 128;
            let l = token % 128;
            let s = g.s as usize;
            let f = (b / s) * 2 + l / 64;
            let sb = b % s;
            let y = (sb / g.nw as usize) * 8 + (l / 8) % 8;
            let x = (sb % g.nw as usize) * 8 + l % 8;
            let at = (f * g.ht as usize + y) * g.wt as usize + x;
            raster[at * DIM..(at + 1) * DIM].copy_from_slice(&v[token * DIM..(token + 1) * DIM]);
        }
        dump::write(
            &dir.join(format!("chunk{}_{name}.npy", self.chunk)),
            &[tokens as usize, DIM],
            &raster,
        )
    }

    fn project(&mut self, gpu: &Gpu, model: &Model, work: &Work, e: &mut Enc) {
        let g = self.g;
        let lq = &model.lq;
        let k = self.chunk;
        let p = g.tf;
        let conv1 = |s: &Self, pass: &mut wgpu::ComputePass| {
            let m = s.map1;
            gemm::run(
                gpu,
                pass,
                &Gemm {
                    m: 2 * p,
                    n: 2048,
                    k: 768 * 36,
                    load: Load::Causal,
                    store: Store::Full,
                    params: params(&[p, g.wt, g.ht, 768, m[0], m[1], m[2], m[3], m[4], m[5]]),
                    weights: &lq.conv1.w,
                    bias: &lq.conv1.b,
                    a: &s.in1,
                    a2: None,
                    out: &work.c1,
                    aux: None,
                },
            );
            kernels::chan_norm(
                gpu,
                pass,
                (&work.c1, &lq.norm1, &s.in2),
                2 * p,
                2048,
                s.map2[2] * p * 1024,
            );
        };
        let unshuffle = |s: &Self, pass: &mut wgpu::ComputePass, frame: u64, logical: usize| {
            kernels::unshuffle(
                gpu,
                pass,
                &Unshuffle {
                    frames: &s.frames,
                    out: &s.in1,
                    wt: g.wt,
                    count: p,
                    wm: g.wm,
                    plane: g.plane(),
                    slot: s.slot(frame),
                    frame: s.map1[logical],
                },
            );
        };
        if k == 0 {
            e.pass(|pass| {
                for logical in 0..6 {
                    unshuffle(self, pass, 0, logical);
                }
                conv1(self, pass);
            });
            self.rotate();
        }
        for j in 0..4u32 {
            let latent = k * 4 + u64::from(j);
            e.pass(|pass| {
                for c in 0..4 {
                    unshuffle(self, pass, latent * 4 + 1 + c as u64, 2 + c);
                }
                conv1(self, pass);
                let m = self.map2;
                gemm::run(
                    gpu,
                    pass,
                    &Gemm {
                        m: p,
                        n: 3072,
                        k: 2048 * 36,
                        load: Load::Causal,
                        store: Store::Full,
                        params: params(&[p, g.wt, g.ht, 2048, m[0], m[1], m[2], m[3]]),
                        weights: &lq.conv2.w,
                        bias: &lq.conv2.b,
                        a: &self.in2,
                        a2: None,
                        out: &work.c2,
                        aux: None,
                    },
                );
                kernels::chan_norm(gpu, pass, (&work.c2, &lq.norm2, &work.c2h), p, 3072, 0);
                rows(
                    gpu,
                    pass,
                    &lq.linear,
                    p,
                    (&work.c2h, &work.x),
                    (Store::Cond, None),
                    &[(12, g.wt), (13, g.nh), (14, g.nw), (15, j)],
                );
            });
            e.submit();
            self.rotate();
        }
    }

    fn rotate(&mut self) {
        let m = self.map1;
        self.map1 = [m[4], m[5], m[0], m[1], m[2], m[3]];
        let m = self.map2;
        self.map2 = [m[2], m[3], m[0], m[1]];
    }

    fn block(
        &mut self,
        gpu: &Gpu,
        model: &Model,
        work: &Work,
        table: &wgpu::Buffer,
        i: usize,
        b: &Block,
    ) -> Result<()> {
        let g = self.g;
        let t = g.t;
        let cache = &self.caches[i];
        let windows = cache.ring.len() as u32 + 2;
        let prune = g.prunes(cache.range, windows);
        let stride = cache.range * cache.range * windows;
        let mut src = [0u32; 8];
        for (w, slot) in cache.ring.iter().enumerate() {
            src[w] = *slot;
        }
        src[cache.ring.len()] = 64;
        src[cache.ring.len() + 1] = 65;
        let grid = Grid {
            qbs: g.qbs,
            s: g.s,
            nw: g.nw,
            nh: g.nh,
            range: cache.range,
            stride,
            src,
        };
        let mut state = Vec::new();
        for _ in 0..12 {
            state.extend_from_slice(&[0u32, (g.topk + 1).min(u64::from(u32::MAX)) as u32, 0, 0]);
        }
        let state = gpu.upload(&words(&state));
        let mut e = Enc::new(gpu);
        e.pass(|pass| {
            kernels::layer_norm(
                gpu,
                pass,
                &Norm {
                    x: &work.x,
                    mul: &b.mul_msa,
                    add: &b.add_msa,
                    out: &work.h,
                    rows: t,
                    first: 0,
                },
            );
            for (l, out) in [(&b.q, &work.q), (&b.k, &work.k), (&b.v, &work.v)] {
                rows(
                    gpu,
                    pass,
                    l,
                    t,
                    (&work.h, out),
                    (Store::Half(Act::None), None),
                    &[],
                );
            }
            kernels::rms(gpu, pass, &work.q, &b.norm_q, table, t, Some((g.nh, g.nw)));
            kernels::rms(gpu, pass, &work.k, &b.norm_k, table, t, Some((g.nh, g.nw)));
            kernels::pool(gpu, pass, &work.k, &work.kp, g.qbs);
            if prune {
                kernels::pool(gpu, pass, &work.q, &work.qp, g.qbs);
                kernels::scores(
                    gpu,
                    pass,
                    &grid,
                    (&work.qp, &cache.pooled, &work.kp, &work.probs),
                );
                for shift in [24, 16, 8, 0] {
                    kernels::hist(
                        gpu,
                        pass,
                        (&work.probs, &state, &work.hist),
                        g.qbs * stride,
                        shift,
                    );
                    kernels::pick(gpu, pass, (&state, &work.hist), shift);
                }
            }
            kernels::select(
                gpu,
                pass,
                &grid,
                !prune,
                (&work.probs, &state, &work.list, &work.counts),
            );
        });
        e.submit();
        let attend = Attend {
            q: &work.q,
            kc: &cache.k,
            vc: &cache.v,
            kn: &work.k,
            vn: &work.v,
            list: &work.list,
            counts: &work.counts,
            out: &work.a,
            qbs: g.qbs,
            s: g.s,
            stride,
            dense: 0,
            src,
        };
        for first in (0..g.qbs).step_by(QUERY_GROUP as usize) {
            let count = QUERY_GROUP.min(g.qbs - first);
            e.pass(|pass| kernels::attend(gpu, pass, &attend, first, count));
            e.submit();
        }
        e.pass(|pass| {
            rows(
                gpu,
                pass,
                &b.o,
                t,
                (&work.a, &work.x),
                (Store::Gated, Some(&b.gate_msa)),
                &[],
            );
            kernels::layer_norm(
                gpu,
                pass,
                &Norm {
                    x: &work.x,
                    mul: &b.mul3,
                    add: &b.add3,
                    out: &work.h,
                    rows: t,
                    first: 0,
                },
            );
            rows(
                gpu,
                pass,
                &b.cq,
                t,
                (&work.h, &work.q),
                (Store::Half(Act::None), None),
                &[],
            );
            kernels::rms(gpu, pass, &work.q, &b.cnorm_q, table, t, None);
        });
        let cross = Attend {
            q: &work.q,
            kc: &b.ctx_k,
            vc: &b.ctx_v,
            kn: &b.ctx_k,
            vn: &b.ctx_v,
            list: &work.list,
            counts: &work.counts,
            out: &work.a,
            qbs: g.qbs,
            s: g.s,
            stride,
            dense: 4,
            src,
        };
        for first in (0..g.qbs).step_by(QUERY_GROUP as usize * 4) {
            let count = (QUERY_GROUP * 4).min(g.qbs - first);
            e.pass(|pass| kernels::attend(gpu, pass, &cross, first, count));
        }
        e.pass(|pass| {
            rows(
                gpu,
                pass,
                &b.co,
                t,
                (&work.a, &work.x),
                (Store::Gated, Some(&model.ones)),
                &[],
            );
        });
        e.submit();
        for first in (0..t).step_by(SLAB as usize) {
            let n = SLAB.min(t - first);
            e.pass(|pass| {
                kernels::layer_norm(
                    gpu,
                    pass,
                    &Norm {
                        x: &work.x,
                        mul: &b.mul_mlp,
                        add: &b.add_mlp,
                        out: &work.h2,
                        rows: n,
                        first,
                    },
                );
                rows(
                    gpu,
                    pass,
                    &b.ffn1,
                    n,
                    (&work.h2, &work.u),
                    (Store::Half(Act::Gelu), None),
                    &[],
                );
                rows(
                    gpu,
                    pass,
                    &b.ffn2,
                    n,
                    (&work.u, &work.x),
                    (Store::Gated, Some(&b.gate_mlp)),
                    &[(11, first)],
                );
            });
            e.submit();
        }
        self.keep(gpu, i, work, &mut e);
        e.submit();
        Ok(())
    }

    fn keep(&mut self, _gpu: &Gpu, i: usize, work: &Work, e: &mut Enc) {
        let g = self.g;
        let cache = &mut self.caches[i];
        let mut order: Vec<i64> = cache.ring.iter().map(|s| i64::from(*s)).collect();
        order.push(-1);
        order.push(-2);
        let kept: Vec<i64> = order[order.len().saturating_sub(cache.keep as usize)..].to_vec();
        let mut free: Vec<u32> = (0..cache.keep)
            .filter(|s| !kept.contains(&i64::from(*s)))
            .collect();
        let window = u64::from(g.s) * 128 * DIM as u64 * 2;
        let pooled = u64::from(g.s) * 12 * 128 * 4;
        let mut ring = Vec::new();
        for w in kept {
            if w >= 0 {
                ring.push(w as u32);
                continue;
            }
            let j = (-w - 1) as u64;
            let slot = free.remove(0);
            let to = u64::from(slot);
            e.copy(&work.k, j * window, &cache.k, to * window, window);
            e.copy(&work.v, j * window, &cache.v, to * window, window);
            e.copy(&work.kp, j * pooled, &cache.pooled, to * pooled, pooled);
            ring.push(slot);
        }
        cache.ring = ring;
    }

    fn decode(&mut self, gpu: &Gpu, model: &Model, latents: &wgpu::Buffer) -> Result<Vec<Vec<u8>>> {
        let g = self.g;
        let d = &model.dec;
        let (w8, h8) = (g.w8, g.h8);
        let p8 = g.p8();
        let din = gpu.buffer((p8 * 784 * 2) as usize);
        let a = [0, 1, 2].map(|_| gpu.buffer((p8 * 512 * 2) as usize));
        let g1 = gpu.buffer((p8 * 512 * 2) as usize);
        let b = [0, 1, 2].map(|_| gpu.buffer((p8 * 4 * 256 * 2) as usize));
        let g2 = gpu.buffer((p8 * 4 * 512 * 2) as usize);
        let c = [0, 1, 2].map(|_| gpu.buffer((p8 * 16 * 128 * 2) as usize));
        let g3 = gpu.buffer((p8 * 16 * 256 * 2) as usize);
        let t1 = gpu.buffer(((BAND + 4) * g.wm * 128 * 2) as usize);
        let t2 = gpu.buffer(((BAND + 2) * g.wm * 128 * 2) as usize);
        let o = gpu.buffer((BAND * g.wm * 3 * 4) as usize);
        let picture = gpu.buffer((g.w * g.h * 4) as usize);
        let mut frames = Vec::new();
        let [mut ax, at1, at2] = a;
        let [mut bx, bt1, bt2] = b;
        let [mut cx, ct1, ct2] = c;
        for j in 0..4u32 {
            let latent = self.chunk * 4 + u64::from(j);
            let slots = [0, 1, 2, 3].map(|i| self.slot((latent * 4 + i).saturating_sub(3)));
            let mut e = Enc::new(gpu);
            e.pass(|pass| {
                kernels::dec_input(
                    gpu,
                    pass,
                    &DecInput {
                        frames: &self.frames,
                        latents,
                        out: &din,
                        w8,
                        h8,
                        wm: g.wm,
                        plane: g.plane(),
                        f: j,
                        slots,
                    },
                );
                let flat = |src, cin| Plane {
                    w: w8,
                    h: h8,
                    y0: 0,
                    rows: h8,
                    src,
                    cin,
                    stride: cin / 2,
                    offset: 0,
                    two: None,
                    up: false,
                    sy0: 0,
                };
                plane(
                    gpu,
                    pass,
                    &d.first,
                    &flat(&din, 784),
                    &at1,
                    Store::Half(Act::Relu),
                    None,
                );
                plane(
                    gpu,
                    pass,
                    &d.deepen1,
                    &flat(&at1, 512),
                    &ax,
                    Store::Half(Act::Relu),
                    None,
                );
            });
            for (i, mb) in d.stage1.iter().enumerate() {
                e.pass(|pass| {
                    mem_block(
                        gpu,
                        pass,
                        mb,
                        (w8, h8, 512),
                        (&mut ax, &mut self.mems.a[i], &at1, &at2),
                    )
                });
            }
            e.pass(|pass| {
                rows(
                    gpu,
                    pass,
                    &d.grow1,
                    p8,
                    (&ax, &g1),
                    (Store::Half(Act::None), None),
                    &[],
                );
                let up = Plane {
                    w: w8 * 2,
                    h: h8 * 2,
                    y0: 0,
                    rows: h8 * 2,
                    src: &g1,
                    cin: 512,
                    stride: 256,
                    offset: 0,
                    two: None,
                    up: true,
                    sy0: 0,
                };
                plane(
                    gpu,
                    pass,
                    &d.narrow2,
                    &up,
                    &bx,
                    Store::Half(Act::None),
                    None,
                );
            });
            for (i, mb) in d.stage2.iter().enumerate() {
                e.pass(|pass| {
                    mem_block(
                        gpu,
                        pass,
                        mb,
                        (w8 * 2, h8 * 2, 256),
                        (&mut bx, &mut self.mems.b[i], &bt1, &bt2),
                    )
                });
            }
            e.pass(|pass| {
                rows(
                    gpu,
                    pass,
                    &d.grow2,
                    p8 * 4,
                    (&bx, &g2),
                    (Store::Half(Act::None), None),
                    &[],
                )
            });
            e.submit();
            for half in 0..2u32 {
                e.pass(|pass| {
                    let up = Plane {
                        w: w8 * 4,
                        h: h8 * 4,
                        y0: 0,
                        rows: h8 * 4,
                        src: &g2,
                        cin: 256,
                        stride: 256,
                        offset: half * 128,
                        two: None,
                        up: true,
                        sy0: 0,
                    };
                    plane(
                        gpu,
                        pass,
                        &d.narrow3,
                        &up,
                        &cx,
                        Store::Half(Act::None),
                        None,
                    );
                });
                for (i, mb) in d.stage3.iter().enumerate() {
                    e.pass(|pass| {
                        mem_block(
                            gpu,
                            pass,
                            mb,
                            (w8 * 4, h8 * 4, 128),
                            (&mut cx, &mut self.mems.c[i], &ct1, &ct2),
                        )
                    });
                }
                e.pass(|pass| {
                    rows(
                        gpu,
                        pass,
                        &d.grow3,
                        p8 * 16,
                        (&cx, &g3),
                        (Store::Half(Act::None), None),
                        &[],
                    )
                });
                e.submit();
                for sub in 0..2u32 {
                    let index = latent as i64 * 4 - 3 + i64::from(half * 2 + sub);
                    if index < 0 {
                        continue;
                    }
                    self.last(gpu, model, &g3, sub, (&t1, &t2, &o, &picture), &mut e);
                    let raw = gpu.read(&picture, 0, u64::from(g.w * g.h * 4))?;
                    let rgb: Vec<u8> = raw
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|p| [p[0], p[1], p[2]])
                        .collect();
                    if let Some(dir) = self.dump.as_ref() {
                        dump::write_bytes(
                            &dir.join(format!("frame{index:03}.npy")),
                            &[g.h as usize, g.w as usize, 3],
                            &rgb,
                        )?;
                    }
                    frames.push(rgb);
                }
            }
        }
        Ok(frames)
    }

    fn last(
        &self,
        gpu: &Gpu,
        model: &Model,
        g3: &wgpu::Buffer,
        sub: u32,
        (t1, t2, o, picture): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer),
        e: &mut Enc,
    ) {
        let g = self.g;
        let d = &model.dec;
        let (w, h) = (g.wm, g.hm);
        for r0 in (0..g.h).step_by(BAND as usize) {
            let r1 = (r0 + BAND).min(h);
            let (a0, a1) = (r0.saturating_sub(2), (r1 + 2).min(h));
            let (b0, b1) = (r0.saturating_sub(1), (r1 + 1).min(h));
            e.pass(|pass| {
                let first = Plane {
                    w,
                    h,
                    y0: a0,
                    rows: a1 - a0,
                    src: g3,
                    cin: 128,
                    stride: 128,
                    offset: sub * 64,
                    two: None,
                    up: true,
                    sy0: 0,
                };
                plane(
                    gpu,
                    pass,
                    &d.last1,
                    &first,
                    t1,
                    Store::Half(Act::Relu),
                    None,
                );
                let second = Plane {
                    w,
                    h,
                    y0: b0,
                    rows: b1 - b0,
                    src: t1,
                    cin: 128,
                    stride: 64,
                    offset: 0,
                    two: None,
                    up: false,
                    sy0: a0,
                };
                plane(
                    gpu,
                    pass,
                    &d.deepen2,
                    &second,
                    t2,
                    Store::Half(Act::Relu),
                    None,
                );
                let third = Plane {
                    w,
                    h,
                    y0: r0,
                    rows: r1 - r0,
                    src: t2,
                    cin: 128,
                    stride: 64,
                    offset: 0,
                    two: None,
                    up: false,
                    sy0: b0,
                };
                plane(gpu, pass, &d.last2, &third, o, Store::Full, None);
                let shown = r1.min(g.h) - r0;
                kernels::output(gpu, pass, (o, picture), (g.w, shown, r0, w));
            });
            e.submit();
        }
    }
}

fn whole<'a>(
    (w, h, n): (u32, u32, u32),
    src: &'a wgpu::Buffer,
    two: Option<&'a wgpu::Buffer>,
) -> Plane<'a> {
    Plane {
        w,
        h,
        y0: 0,
        rows: h,
        src,
        cin: n,
        stride: n / 2,
        offset: 0,
        two,
        up: false,
        sy0: 0,
    }
}

fn mem_block(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    mb: &MemBlock,
    (w, h, n): (u32, u32, u32),
    (x, past, t1, t2): (
        &mut wgpu::Buffer,
        &mut wgpu::Buffer,
        &wgpu::Buffer,
        &wgpu::Buffer,
    ),
) {
    plane(
        gpu,
        pass,
        &mb.a,
        &whole((w, h, n), x, Some(past)),
        t1,
        Store::Half(Act::Relu),
        None,
    );
    plane(
        gpu,
        pass,
        &mb.b,
        &whole((w, h, n), t1, None),
        t2,
        Store::Half(Act::Relu),
        None,
    );
    plane(
        gpu,
        pass,
        &mb.c,
        &whole((w, h, n), t2, None),
        past,
        Store::Skip,
        Some(x),
    );
    std::mem::swap(x, past);
}

struct Work {
    x: wgpu::Buffer,
    h: wgpu::Buffer,
    q: wgpu::Buffer,
    k: wgpu::Buffer,
    v: wgpu::Buffer,
    a: wgpu::Buffer,
    h2: wgpu::Buffer,
    u: wgpu::Buffer,
    qp: wgpu::Buffer,
    kp: wgpu::Buffer,
    probs: wgpu::Buffer,
    list: wgpu::Buffer,
    counts: wgpu::Buffer,
    hist: wgpu::Buffer,
    c1: wgpu::Buffer,
    c2: wgpu::Buffer,
    c2h: wgpu::Buffer,
}

impl Work {
    fn new(gpu: &Gpu, g: &Geometry) -> Self {
        let t = g.t as usize;
        let half = |n: usize| gpu.buffer(n * 2);
        let slab = SLAB.min(g.t) as usize;
        let stride = (GLOBAL_RANGE * GLOBAL_RANGE * (GLOBAL_KEEP + 2)) as usize;
        let rows = 12 * g.qbs as usize;
        let p = g.tf as usize;
        Self {
            x: gpu.buffer(t * DIM * 4),
            h: half(t * DIM),
            q: half(t * DIM),
            k: half(t * DIM),
            v: half(t * DIM),
            a: half(t * DIM),
            h2: half(slab * DIM),
            u: half(slab * FFN),
            qp: gpu.buffer(rows * 128 * 4),
            kp: gpu.buffer(rows * 128 * 4),
            probs: gpu.buffer(rows * stride * 4),
            list: gpu.buffer(rows * stride * 4),
            counts: gpu.buffer(rows * 4),
            hist: gpu.buffer(12 * 256 * 4),
            c1: gpu.buffer(2 * p * 2048 * 4),
            c2: gpu.buffer(p * 3072 * 4),
            c2h: half(p * 3072),
        }
    }
}

fn ints(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
