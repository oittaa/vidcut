use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::gemm::{self, Act, Gemm, Load, Store};
use super::kernels::{
    self, Attend, DecInput, Grid, Last, Norm, Resize, Unshuffle, WinoIn, WinoOut,
};
use super::model::{Block, Linear, MemBlock, Model, Wino};
use super::{DIM, FFN, resize};
use crate::compute::{Gpu, floats};
use crate::compute::{bytes, words};
use crate::error::{Error, Result};
use crate::screen::held::Held;

const RING: u64 = 20;
const SLAB: u32 = 16384;
const BAND: u32 = 64;
const WINO_ROWS: u32 = 16;
const ATTEND: u32 = 19200;
const CROSS: u32 = 128;
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

enum Keys {
    Gpu { k: wgpu::Buffer, v: wgpu::Buffer },
    Host { k: Vec<Vec<u8>>, v: Vec<Vec<u8>> },
}

struct Cache {
    keys: Keys,
    pooled: wgpu::Buffer,
    ring: Vec<u32>,
    keep: u32,
    range: u32,
}

struct Staged {
    k: wgpu::Buffer,
    v: wgpu::Buffer,
    fresh: Vec<u64>,
    done: Option<wgpu::SubmissionIndex>,
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
    loaner: Option<(wgpu::Buffer, wgpu::Buffer)>,
    upload: Option<wgpu::SubmissionIndex>,
    lent: Option<crate::player::LentMemory>,
    fed: u64,
    chunk: u64,
    dump: Option<PathBuf>,
    host_wait: Duration,
    host_bytes: u64,
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

    fn clear(&mut self, buffer: &wgpu::Buffer) {
        self.encoder().clear_buffer(buffer, 0, None);
    }

    fn copy(&mut self, src: &wgpu::Buffer, from: u64, dst: &wgpu::Buffer, to: u64, size: u64) {
        self.encoder()
            .copy_buffer_to_buffer(src, from, dst, to, size);
    }

    fn submit(&mut self) -> Option<wgpu::SubmissionIndex> {
        let enc = self.enc.take()?;
        self.gpu.flush();
        Some(self.gpu.queue.submit([enc.finish()]))
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
    if let Some(u) = &l.wino {
        winograd(gpu, pass, (l, u), p, out, (store, aux));
        return;
    }
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

fn winograd(
    gpu: &Gpu,
    pass: &mut wgpu::ComputePass,
    (l, u): (&Linear, &Wino),
    p: &Plane,
    out: &wgpu::Buffer,
    (store, aux): (Store, Option<&wgpu::Buffer>),
) {
    let (ctot, cout) = (l.k / 9, l.n);
    let (act, skip) = match store {
        Store::Half(Act::None) => (0, 0),
        Store::Half(Act::Relu) => (1, 0),
        Store::Skip => (0, 1),
        _ => panic!("Winograd takes a half store with ReLU or none, or a skip"),
    };
    let tw = p.w.div_ceil(2);
    let th = p.rows.div_ceil(2);
    for tr0 in (0..th).step_by(WINO_ROWS as usize) {
        let tiles = WINO_ROWS.min(th - tr0) * tw;
        let (v, m) = gpu.scratch(
            16 * u64::from(tiles) * u64::from(ctot) * 2,
            16 * u64::from(tiles) * u64::from(cout) * 4,
        );
        kernels::wino_in(
            gpu,
            pass,
            &WinoIn {
                src: p.src,
                src2: p.two.unwrap_or(p.src),
                out: &v,
                values: [
                    p.w,
                    p.h,
                    p.y0,
                    p.rows,
                    p.cin,
                    p.stride,
                    p.offset,
                    p.cin / 2,
                    u32::from(p.up),
                    p.sy0,
                    ctot,
                    tw,
                    tr0,
                    tiles,
                    0,
                    0,
                ],
            },
            tiles * (ctot / 2),
        );
        for (xi, w) in u.u.iter().enumerate() {
            let at = xi as u32 * tiles;
            let mut values = params(&[ctot / 2, at]);
            values[11] = at;
            gemm::run(
                gpu,
                pass,
                &Gemm {
                    m: tiles,
                    n: cout,
                    k: ctot,
                    load: Load::Rows,
                    store: Store::Full,
                    params: values,
                    weights: w,
                    bias: &u.zero,
                    a: &v,
                    a2: None,
                    out: &m,
                    aux: None,
                },
            );
        }
        kernels::wino_out(
            gpu,
            pass,
            &WinoOut {
                m: &m,
                bias: &l.b,
                aux: aux.unwrap_or(&l.b),
                out,
                values: [p.w, p.rows, cout, tw, tr0, tiles, act, skip],
            },
            tiles * (cout / 4),
        );
    }
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

fn keep_of(layer: usize) -> u32 {
    if super::GLOBAL.contains(&layer) {
        GLOBAL_KEEP
    } else {
        LOCAL_KEEP
    }
}

fn kept(cache: &Cache) -> Vec<i64> {
    let mut order: Vec<i64> = cache.ring.iter().map(|s| i64::from(*s)).collect();
    order.push(-1);
    order.push(-2);
    order[order.len().saturating_sub(cache.keep as usize)..].to_vec()
}

fn window(g: &Geometry) -> u64 {
    u64::from(g.s) * 128 * DIM as u64 * 2
}

fn stride() -> u64 {
    u64::from(GLOBAL_RANGE * GLOBAL_RANGE * (GLOBAL_KEEP + 2))
}

fn persistent(g: &Geometry) -> u64 {
    let (tf, p8) = (u64::from(g.tf), u64::from(g.p8()));
    let pooled: u64 = (0..super::LAYERS)
        .map(|i| u64::from(keep_of(i) * g.s) * 12 * 128 * 4)
        .sum();
    RING * u64::from(g.plane()) * 4
        + 6 * tf * 768 * 2
        + 4 * tf * 2048 * 2
        + 3 * (p8 * 512 + 4 * p8 * 256 + 16 * p8 * 128) * 2
        + pooled
}

fn chunk_work(g: &Geometry) -> u64 {
    let (t, tf, p8) = (u64::from(g.t), u64::from(g.tf), u64::from(g.p8()));
    let slab = u64::from(SLAB.min(g.t));
    let rows = 12 * u64::from(g.qbs);
    let x = t * DIM as u64 * 4;
    let lq = x + 2 * tf * 2048 * 4 + tf * 3072 * (4 + 2);
    let blocks = x
        + t * DIM as u64 * 4 * 2
        + slab * (DIM + FFN) as u64 * 2
        + rows * 128 * 4 * 2
        + rows * stride() * 4 * 2;
    let wm = u64::from(g.wm);
    let bands = u64::from(BAND + 4) * wm * 128 * 2
        + u64::from(BAND + 2) * wm * 128 * 2
        + u64::from(g.w * g.h) * 3;
    let first = p8 * (784 + 3 * 512 + 4 * 256) * 2;
    let second = p8 * 4 * (2 * 256 + 512) * 2;
    let third = p8 * 4 * 512 * 2 + p8 * 16 * 2 * 128 * 2;
    let decode = bands + first.max(second).max(third) + wino_scratch(g);
    lq.max(blocks).max(decode) + 2 * 16 * 4 * p8 * 4
}

fn wino_scratch(g: &Geometry) -> u64 {
    let w = u64::from(g.wm);
    [
        (w / 8, 1024, 512),
        (w / 8, 784, 512),
        (w / 4, 512, 256),
        (w / 2, 256, 128),
        (w, 128, 128),
    ]
    .iter()
    .map(|(width, cin, cout)| 16 * u64::from(WINO_ROWS) * width.div_ceil(2) * (cin * 2 + cout * 4))
    .max()
    .unwrap_or(0)
}

fn gigabytes(b: u64) -> String {
    format!("{:.1}", b as f64 / 1e9)
}

pub(super) struct Plan {
    pub(super) resident: Vec<bool>,
    pub(super) need: u64,
    pub(super) room: Option<u64>,
}

fn in_ram(host: u64, g: &Geometry) -> Result<()> {
    let ram = crate::player::system_memory().map_or(0, |b| b as u64);
    if host > ram / 2 {
        return Err(Error::Invalid(format!(
            "the AI Best upscaler needs about {} GB of main memory at {}x{} for what does not fit on the GPU, and this computer has {} GB",
            gigabytes(host),
            g.wm,
            g.hm,
            gigabytes(ram)
        )));
    }
    Ok(())
}

pub(super) fn plan(gpu: &Gpu, g: &Geometry) -> Result<Plan> {
    let kv: Vec<u64> = (0..super::LAYERS)
        .map(|i| 2 * u64::from(keep_of(i)) * window(g))
        .collect();
    let fixed = persistent(g) + chunk_work(g);
    let all = Plan {
        resident: vec![true; kv.len()],
        need: fixed + kv.iter().sum::<u64>(),
        room: None,
    };
    let Some(room) = gpu.listed.as_ref().and_then(crate::player::room_on) else {
        return Ok(all);
    };
    if all.need <= room {
        return Ok(Plan {
            room: Some(room),
            ..all
        });
    }
    let host_path = 2 * kv.iter().copied().max().unwrap_or(0);
    if fixed + host_path > room {
        return Err(Error::Invalid(format!(
            "the AI Best upscaler needs about {} GB of GPU memory at {}x{}, and {} GB of this GPU is free",
            gigabytes(fixed + host_path),
            g.wm,
            g.hm,
            gigabytes(room)
        )));
    }
    let mut used = fixed + host_path;
    let resident: Vec<bool> = kv
        .iter()
        .map(|b| {
            let fits = used + b <= room;
            if fits {
                used += b;
            }
            fits
        })
        .collect();
    let host: u64 = kv
        .iter()
        .zip(&resident)
        .filter(|(_, r)| !**r)
        .map(|(b, _)| b)
        .sum();
    in_ram(host, g)?;
    Ok(Plan {
        resident,
        need: used,
        room: Some(room),
    })
}

impl Stream {
    pub(super) fn new(
        gpu: &Gpu,
        g: Geometry,
        resident: &[bool],
        dump: Option<PathBuf>,
    ) -> Result<Self> {
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
        let size = window(&g) as usize;
        let caches = (0..super::LAYERS)
            .map(|i| {
                let keep = keep_of(i);
                let keys = if resident[i] {
                    Keys::Gpu {
                        k: gpu.buffer(keep as usize * size),
                        v: gpu.buffer(keep as usize * size),
                    }
                } else {
                    Keys::Host {
                        k: vec![Vec::new(); keep as usize],
                        v: vec![Vec::new(); keep as usize],
                    }
                };
                Cache {
                    keys,
                    pooled: gpu.buffer((keep * g.s * 12 * 128 * 4) as usize),
                    ring: Vec::new(),
                    keep,
                    range: if super::GLOBAL.contains(&i) {
                        GLOBAL_RANGE
                    } else {
                        LOCAL_RANGE
                    },
                }
            })
            .collect::<Vec<_>>();
        let loaner = caches
            .iter()
            .filter(|c| matches!(c.keys, Keys::Host { .. }))
            .map(|c| c.keep as usize * size)
            .max()
            .map(|b| (gpu.buffer(b), gpu.buffer(b)));
        let p8 = g.p8() as usize;
        let mem = |p: usize, n: usize| (0..3).map(|_| gpu.buffer(p * n * 2)).collect();
        let mut stream = Self {
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
            loaner,
            upload: None,
            lent: None,
            fed: 0,
            chunk: 0,
            host_wait: Duration::ZERO,
            host_bytes: 0,
            dump,
        };
        stream.lend_memory();
        Ok(stream)
    }

    fn lend_memory(&mut self) {
        let size = window(&self.g);
        let host: u64 = self
            .caches
            .iter()
            .filter(|c| matches!(c.keys, Keys::Host { .. }))
            .map(|c| 2 * u64::from(c.keep) * size)
            .sum();
        self.lent = None;
        self.lent = (host > 0).then(|| crate::player::lend_memory(host));
    }

    fn fit(&mut self, gpu: &Gpu) -> Result<()> {
        let g = self.g;
        gpu.wait()?;
        while let Some(room) = gpu.listed.as_ref().and_then(crate::player::room_on) {
            let upload = self.loaner.as_ref().map_or(0, |(k, v)| k.size() + v.size());
            let need = chunk_work(&g) + upload;
            if need <= room {
                break;
            }
            let Some(i) = self
                .caches
                .iter()
                .rposition(|c| matches!(c.keys, Keys::Gpu { .. }))
            else {
                break;
            };
            let host: u64 = (0..self.caches.len())
                .filter(|j| *j == i || matches!(self.caches[*j].keys, Keys::Host { .. }))
                .map(|j| 2 * u64::from(self.caches[j].keep) * window(&g))
                .sum();
            in_ram(host, &g)?;
            crate::debug!(
                "flashvsr",
                "chunk {}: {} MB of the GPU free and {} MB needed; cache {i} goes to main memory",
                self.chunk,
                room >> 20,
                need >> 20
            );
            self.move_to_host(gpu, i)?;
        }
        Ok(())
    }

    fn move_to_host(&mut self, gpu: &Gpu, i: usize) -> Result<()> {
        let size = window(&self.g);
        let cache = &mut self.caches[i];
        let Keys::Gpu { k, v } = &cache.keys else {
            return Ok(());
        };
        let mut hk = vec![Vec::new(); cache.keep as usize];
        let mut hv = vec![Vec::new(); cache.keep as usize];
        for slot in &cache.ring {
            let at = u64::from(*slot) * size;
            hk[*slot as usize] = gpu.read(k, at, size)?;
            hv[*slot as usize] = gpu.read(v, at, size)?;
        }
        cache.keys = Keys::Host { k: hk, v: hv };
        let bytes = cache.keep as u64 * size;
        self.lend_memory();
        if self.loaner.as_ref().is_none_or(|(k, _)| k.size() < bytes) {
            self.loaner = None;
            gpu.wait()?;
            self.loaner = Some((gpu.buffer(bytes as usize), gpu.buffer(bytes as usize)));
        }
        gpu.wait()
    }

    fn slot(&self, frame: u64) -> u32 {
        (frame % RING) as u32
    }

    fn need(&self) -> u64 {
        self.chunk * 16 + 17
    }

    pub(super) fn push(
        &mut self,
        gpu: &Gpu,
        model: &Model,
        src: &wgpu::Buffer,
    ) -> Result<Vec<Held>> {
        let g = self.g;
        if src.size() < u64::from(g.sw * g.sh * 3) {
            return Err(Error::Invalid(format!(
                "the AI Best upscaler got a picture of {} bytes for {}x{}",
                src.size(),
                g.sw,
                g.sh
            )));
        }
        let temp = gpu.buffer((g.sh * g.w * 4) as usize);
        let mut e = Enc::new(gpu);
        let t = &self.tables;
        let base = self.slot(self.fed) * g.plane();
        e.pass(|pass| {
            kernels::resize(
                gpu,
                pass,
                &Resize {
                    src,
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
            crate::npy::write_bytes(
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

    pub(super) fn finish(&mut self, gpu: &Gpu, model: &Model) -> Result<Vec<Held>> {
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

    fn run_chunk(&mut self, gpu: &Gpu, model: &Model) -> Result<Vec<Held>> {
        self.fit(gpu)?;
        let g = self.g;
        let k = self.chunk;
        let started = Instant::now();
        self.host_wait = Duration::ZERO;
        self.host_bytes = 0;
        let projected: Instant;
        let p8 = g.p8();
        let noise_v = noise(k, (16 * 4 * p8) as usize);
        let noise = gpu.upload(&bytes(&noise_v));
        let latents = gpu.buffer((16 * 4 * p8 * 4) as usize);
        {
            let x = gpu.buffer(g.t as usize * DIM * 4);
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
                        out: &x,
                        aux: None,
                    },
                );
            });
            {
                let lq = LqWork::new(gpu, &g);
                self.project(gpu, model, (&x, &lq), &mut e);
                e.submit();
                gpu.wait()?;
                projected = Instant::now();
            }
            let work = Work::new(gpu, &g, x);
            self.dump_tensor(gpu, "x_in", &work.x, g.t, true)?;
            for (i, block) in model.blocks.iter().enumerate() {
                crate::cancel::stop()?;
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
            gpu.wait()?;
        }
        if let Some(dir) = self.dump.as_ref() {
            let shape = [16, 4, g.h8 as usize, g.w8 as usize];
            crate::npy::write(&dir.join(format!("chunk{k}_noise.npy")), &shape, &noise_v)?;
            let raw = gpu.read(&latents, 0, u64::from(16 * 4 * p8 * 4))?;
            crate::npy::write(
                &dir.join(format!("chunk{k}_latents.npy")),
                &shape,
                &floats(&raw),
            )?;
        }
        let blocked = Instant::now();
        let frames = self.decode(gpu, model, &latents)?;
        let host = self
            .caches
            .iter()
            .filter(|c| matches!(c.keys, Keys::Host { .. }))
            .count();
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        crate::debug!(
            "flashvsr",
            "chunk {k} at {}x{}: input projection {:.0} ms, {} blocks {:.0} ms, decoder {:.0} ms; {host} of {} caches in main memory: {:.2} GB copied there and back, {:.0} ms waiting for the copies back",
            g.w,
            g.h,
            ms(projected - started),
            super::LAYERS,
            ms(blocked - projected),
            ms(blocked.elapsed()),
            self.caches.len(),
            self.host_bytes as f64 / 1e9,
            ms(self.host_wait)
        );
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
        crate::npy::write(
            &dir.join(format!("chunk{}_{name}.npy", self.chunk)),
            &[tokens as usize, DIM],
            &raster,
        )
    }

    fn project(
        &mut self,
        gpu: &Gpu,
        model: &Model,
        (x, work): (&wgpu::Buffer, &LqWork),
        e: &mut Enc,
    ) {
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
                    (&work.c2h, x),
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
        let mut staged = self.stage(gpu, i, work, &mut e);
        let done = e.submit();
        if let Some(s) = staged.as_mut() {
            s.done = done;
        }
        let (kc, vc) = match &cache.keys {
            Keys::Gpu { k, v } => (k, v),
            Keys::Host { k, v } => {
                if let Some(done) = self.upload.take() {
                    gpu.wait_for(done)?;
                }
                let (lk, lv) = self.loaner.as_ref().expect("a loaner for the host caches");
                let size = window(&g);
                for slot in &cache.ring {
                    let at = u64::from(*slot) * size;
                    gpu.queue.write_buffer(lk, at, &k[*slot as usize]);
                    gpu.queue.write_buffer(lv, at, &v[*slot as usize]);
                    self.host_bytes += 2 * size;
                }
                (lk, lv)
            }
        };
        let attend = Attend {
            q: &work.q,
            kc,
            vc,
            kn: &work.k,
            vn: &work.v,
            list: &work.list,
            counts: &work.counts,
            out: &work.h,
            qbs: g.qbs,
            s: g.s,
            stride,
            dense: 0,
            src,
        };
        let host = matches!(cache.keys, Keys::Host { .. });
        let group = (ATTEND / stride).clamp(1, g.qbs);
        for first in (0..g.qbs).step_by(group as usize) {
            let count = group.min(g.qbs - first);
            e.pass(|pass| kernels::attend(gpu, pass, &attend, first, count));
            let done = e.submit();
            if host && self.upload.is_none() {
                self.upload = done;
            }
        }
        e.pass(|pass| {
            rows(
                gpu,
                pass,
                &b.o,
                t,
                (&work.h, &work.x),
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
            out: &work.h,
            qbs: g.qbs,
            s: g.s,
            stride,
            dense: 4,
            src,
        };
        for first in (0..g.qbs).step_by(CROSS as usize) {
            let count = CROSS.min(g.qbs - first);
            e.pass(|pass| kernels::attend(gpu, pass, &cross, first, count));
        }
        e.pass(|pass| {
            rows(
                gpu,
                pass,
                &b.co,
                t,
                (&work.h, &work.x),
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
        }
        e.submit();
        self.keep(gpu, i, work, staged, &mut e)?;
        e.submit();
        Ok(())
    }

    fn stage(&self, gpu: &Gpu, i: usize, work: &Work, e: &mut Enc) -> Option<Staged> {
        let cache = &self.caches[i];
        if !matches!(cache.keys, Keys::Host { .. }) {
            return None;
        }
        let window = window(&self.g);
        let fresh: Vec<u64> = kept(cache)
            .into_iter()
            .filter(|w| *w < 0)
            .map(|w| (-w - 1) as u64)
            .collect();
        let size = window * fresh.len() as u64;
        let (k, v) = (gpu.staging(size), gpu.staging(size));
        for (n, j) in fresh.iter().enumerate() {
            e.copy(&work.k, j * window, &k, n as u64 * window, window);
            e.copy(&work.v, j * window, &v, n as u64 * window, window);
        }
        Some(Staged {
            k,
            v,
            fresh,
            done: None,
        })
    }

    fn keep(
        &mut self,
        gpu: &Gpu,
        i: usize,
        work: &Work,
        staged: Option<Staged>,
        e: &mut Enc,
    ) -> Result<()> {
        let g = self.g;
        let window = window(&g);
        let fetched = match staged {
            Some(s) => {
                let size = window * s.fresh.len() as u64;
                let waited = Instant::now();
                let k = gpu.fetch(&s.k, size, s.done.clone())?;
                let v = gpu.fetch(&s.v, size, s.done)?;
                self.host_wait += waited.elapsed();
                self.host_bytes += 2 * size;
                Some((s.fresh, k, v))
            }
            None => None,
        };
        let cache = &mut self.caches[i];
        let kept = kept(cache);
        let mut free: Vec<u32> = (0..cache.keep)
            .filter(|s| !kept.contains(&i64::from(*s)))
            .collect();
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
            match &mut cache.keys {
                Keys::Gpu { k, v } => {
                    e.copy(&work.k, j * window, k, to * window, window);
                    e.copy(&work.v, j * window, v, to * window, window);
                }
                Keys::Host { k, v } => {
                    let (fresh, kb, vb) = fetched.as_ref().expect("the staged windows");
                    let n = fresh.iter().position(|f| *f == j).expect("a staged window");
                    let part = n * window as usize..(n + 1) * window as usize;
                    k[slot as usize] = kb[part.clone()].to_vec();
                    v[slot as usize] = vb[part].to_vec();
                }
            }
            e.copy(&work.kp, j * pooled, &cache.pooled, to * pooled, pooled);
            ring.push(slot);
        }
        cache.ring = ring;
        Ok(())
    }

    fn decode(&mut self, gpu: &Gpu, model: &Model, latents: &wgpu::Buffer) -> Result<Vec<Held>> {
        let g = self.g;
        let d = &model.dec;
        let (w8, h8) = (g.w8, g.h8);
        let p8 = g.p8();
        let half = |count: u32, n: u32| gpu.buffer((count * n * 2) as usize);
        let t1 = gpu.buffer(((BAND + 4) * g.wm * 128 * 2) as usize);
        let t2 = gpu.buffer(((BAND + 2) * g.wm * 128 * 2) as usize);
        let shown = (g.w * g.h * 3) as usize;
        let picture = gpu.buffer(shown);
        let mut frames = Vec::new();
        for j in 0..4u32 {
            crate::cancel::stop()?;
            let latent = self.chunk * 4 + u64::from(j);
            let slots = [0, 1, 2, 3].map(|i| self.slot((latent * 4 + i).saturating_sub(3)));
            let mut e = Enc::new(gpu);
            let mut bx = half(p8 * 4, 256);
            {
                let din = half(p8, 784);
                let (mut ax, mut at1, g1) = (half(p8, 512), half(p8, 512), half(p8, 512));
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
                            (&mut ax, &mut self.mems.a[i], &mut at1),
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
                e.submit();
            }
            gpu.wait()?;
            let g2 = half(p8 * 4, 512);
            {
                let mut bt1 = half(p8 * 4, 256);
                for (i, mb) in d.stage2.iter().enumerate() {
                    e.pass(|pass| {
                        mem_block(
                            gpu,
                            pass,
                            mb,
                            (w8 * 2, h8 * 2, 256),
                            (&mut bx, &mut self.mems.b[i], &mut bt1),
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
            }
            drop(bx);
            gpu.wait()?;
            let (mut cx, mut ct1) = (half(p8 * 16, 128), half(p8 * 16, 128));
            for part in 0..2u32 {
                crate::cancel::stop()?;
                e.pass(|pass| {
                    let up = Plane {
                        w: w8 * 4,
                        h: h8 * 4,
                        y0: 0,
                        rows: h8 * 4,
                        src: &g2,
                        cin: 256,
                        stride: 256,
                        offset: part * 128,
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
                            (&mut cx, &mut self.mems.c[i], &mut ct1),
                        )
                    });
                }
                e.submit();
                for sub in 0..2u32 {
                    let index = latent as i64 * 4 - 3 + i64::from(part * 2 + sub);
                    if index < 0 {
                        continue;
                    }
                    e.pass(|pass| {
                        rows(
                            gpu,
                            pass,
                            &d.grow3[sub as usize],
                            p8 * 16,
                            (&cx, &ct1),
                            (Store::Half(Act::None), None),
                            &[],
                        )
                    });
                    self.last(gpu, model, &ct1, (&t1, &t2, &picture), &mut e);
                    let whole = shown.next_multiple_of(4) as u64;
                    let out = gpu.buffer(whole as usize);
                    e.copy(&picture, 0, &out, 0, whole);
                    e.submit();
                    if let Some(dir) = self.dump.as_ref() {
                        let mut rgb = gpu.read(&out, 0, whole)?;
                        rgb.truncate(shown);
                        crate::npy::write_bytes(
                            &dir.join(format!("frame{index:03}.npy")),
                            &[g.h as usize, g.w as usize, 3],
                            &rgb,
                        )?;
                    }
                    frames.push(Held::new(out, (&gpu.device, &gpu.queue), gpu.id));
                }
                gpu.wait()?;
            }
        }
        gpu.free_scratch();
        Ok(frames)
    }

    fn last(
        &self,
        gpu: &Gpu,
        model: &Model,
        g3: &wgpu::Buffer,
        (t1, t2, picture): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer),
        e: &mut Enc,
    ) {
        let g = self.g;
        let d = &model.dec;
        let (w, h) = (g.wm, g.hm);
        e.clear(picture);
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
                    stride: 64,
                    offset: 0,
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
                kernels::last(
                    gpu,
                    pass,
                    &Last {
                        src: t2,
                        weights: &d.last2.w,
                        bias: &d.last2.b,
                        out: picture,
                        wm: w,
                        hm: h,
                        r0,
                        rows: r1.min(g.h) - r0,
                        sy0: b0,
                        w: g.w,
                    },
                );
            });
        }
        e.submit();
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
    (x, past, t1): (&mut wgpu::Buffer, &mut wgpu::Buffer, &mut wgpu::Buffer),
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
        past,
        Store::Half(Act::Relu),
        None,
    );
    plane(
        gpu,
        pass,
        &mb.c,
        &whole((w, h, n), past, None),
        t1,
        Store::Skip,
        Some(x),
    );
    std::mem::swap(x, past);
    std::mem::swap(x, t1);
}

struct Work {
    x: wgpu::Buffer,
    h: wgpu::Buffer,
    q: wgpu::Buffer,
    k: wgpu::Buffer,
    v: wgpu::Buffer,
    h2: wgpu::Buffer,
    u: wgpu::Buffer,
    qp: wgpu::Buffer,
    kp: wgpu::Buffer,
    probs: wgpu::Buffer,
    list: wgpu::Buffer,
    counts: wgpu::Buffer,
    hist: wgpu::Buffer,
}

struct LqWork {
    c1: wgpu::Buffer,
    c2: wgpu::Buffer,
    c2h: wgpu::Buffer,
}

impl LqWork {
    fn new(gpu: &Gpu, g: &Geometry) -> Self {
        let p = g.tf as usize;
        Self {
            c1: gpu.buffer(2 * p * 2048 * 4),
            c2: gpu.buffer(p * 3072 * 4),
            c2h: gpu.buffer(p * 3072 * 2),
        }
    }
}

impl Work {
    fn new(gpu: &Gpu, g: &Geometry, x: wgpu::Buffer) -> Self {
        let t = g.t as usize;
        let half = |n: usize| gpu.buffer(n * 2);
        let slab = SLAB.min(g.t) as usize;
        let stride = (GLOBAL_RANGE * GLOBAL_RANGE * (GLOBAL_KEEP + 2)) as usize;
        let rows = 12 * g.qbs as usize;
        Self {
            x,
            h: half(t * DIM),
            q: half(t * DIM),
            k: half(t * DIM),
            v: half(t * DIM),
            h2: half(slab * DIM),
            u: half(slab * FFN),
            qp: gpu.buffer(rows * 128 * 4),
            kp: gpu.buffer(rows * 128 * 4),
            probs: gpu.buffer(rows * stride * 4),
            list: gpu.buffer(rows * stride * 4),
            counts: gpu.buffer(rows * 4),
            hist: gpu.buffer(12 * 256 * 4),
        }
    }
}

fn ints(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
