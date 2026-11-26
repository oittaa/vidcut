use std::collections::VecDeque;

use super::super::kernels::{self, Remembered};
use super::{Attn, DIM, EMBED, Load, MAP, Net, Norm, TRUNK_EPS, flat};
use crate::compute::ops::{self, Act, Conv, Ops, Res, Shape, View, tensor};
use crate::compute::{Gpu, bytes};
use crate::error::Result;

pub(super) const MEMORIES: usize = 7;
const POINTERS: usize = 16;
const MEM: u32 = 64;
const FFN: u32 = 2048;
const FUSED: u32 = 1024;
const LEVELS: usize = 4;
const ATTENTION_EPS: f32 = 1e-5;
const MASK_SCALE: f32 = 20.0;
const MASK_BIAS: f32 = -10.0;
const THETA: f64 = 10000.0;
const TEMPERATURE: f64 = 10000.0;
const VISION_POS: f32 = 0.1;
const TOKENS: u32 = EMBED * EMBED;
const SPLIT: u32 = DIM / MEM;
const MOST_KEYS: u32 = (MEMORIES as u32 * TOKENS + POINTERS as u32 * SPLIT).next_multiple_of(64);
const QUERIES: u32 = 1024;

struct Fuse {
    dw: wgpu::Buffer,
    dwb: wgpu::Buffer,
    norm: Norm,
    up: Conv,
    down: Conv,
}

struct Level {
    own: Attn,
    norm1: Norm,
    to_memory: Attn,
    norm2: Norm,
    up: Conv,
    down: Conv,
    norm3: Norm,
}

pub(super) struct Memory {
    down: Vec<(Conv, Norm)>,
    down_out: Conv,
    pix: Conv,
    fuse: Vec<Fuse>,
    out: [Conv; 2],
    levels: Vec<Level>,
    norm: Norm,
    pub(super) pointer: [Conv; 3],
    tpos: (Vec<f32>, Vec<f32>),
    no_pointer: Vec<f32>,
    turns: wgpu::Buffer,
    vision_pos: View,
    memory_pos: Vec<View>,
    pub(super) shift: View,
    pub(super) none: View,
}

fn tiled(gpu: &Gpu, values: &[f32]) -> View {
    let c = values.len() as u32;
    let v = tensor(gpu, 1, EMBED, EMBED, c);
    let all: Vec<f32> = (0..TOKENS).flat_map(|_| values.iter().copied()).collect();
    gpu.queue.write_buffer(&v.buf, 0, &bytes(&all));
    v
}

fn sine(side: u32, feats: usize) -> Vec<f32> {
    let scale = std::f64::consts::TAU;
    let at = |i: u32| f64::from(f64::from(i + 1) as f32 / (f64::from(side) as f32 + 1e-6)) * scale;
    let turn = |v: f64, i: usize| {
        let d = TEMPERATURE.powf(2.0 * (i / 2) as f64 / feats as f64);
        match i % 2 {
            0 => (v / d).sin() as f32,
            _ => (v / d).cos() as f32,
        }
    };
    let mut out = Vec::with_capacity((side * side) as usize * 2 * feats);
    for y in 0..side {
        for x in 0..side {
            out.extend((0..feats).map(|i| turn(at(y), i)));
            out.extend((0..feats).map(|i| turn(at(x), i)));
        }
    }
    out
}

fn turns() -> Vec<f32> {
    let quarter = (DIM / 4) as usize;
    let freq = |j: usize| (1.0 / THETA.powf((4 * j) as f64 / f64::from(DIM))) as f32;
    let mut out = Vec::with_capacity(TOKENS as usize * 2 * quarter * 2);
    for r in 0..TOKENS {
        let (tx, ty) = ((r % EMBED) as f32, (r / EMBED) as f32);
        for j in 0..2 * quarter {
            let angle = match j < quarter {
                true => tx * freq(j),
                false => ty * freq(j - quarter),
            };
            out.push(f64::from(angle).cos() as f32);
            out.push(f64::from(angle).sin() as f32);
        }
    }
    out
}

fn line_sine(pos: f64) -> Vec<f32> {
    let half = (DIM / 2) as usize;
    let angle = |i: usize| pos / TEMPERATURE.powf(2.0 * (i / 2) as f64 / half as f64);
    let sin = (0..half).map(|i| angle(i).sin() as f32);
    let cos = (0..half).map(|i| angle(i).cos() as f32);
    sin.chain(cos).collect()
}

impl Load<'_> {
    fn fuse(&self, at: &str) -> Result<Fuse> {
        let dw = self.floats(&format!("{at}.dwconv.weight"), &[DIM, 1, 7, 7])?;
        let dwb = self.floats(&format!("{at}.dwconv.bias"), &[DIM])?;
        let gamma = self.floats(&format!("{at}.gamma"), &[DIM])?;
        let mut w = self.floats(&format!("{at}.pwconv2.weight"), &[DIM, FUSED])?;
        let mut b = self.floats(&format!("{at}.pwconv2.bias"), &[DIM])?;
        for (o, g) in gamma.iter().enumerate() {
            b[o] *= g;
            for v in &mut w[o * FUSED as usize..(o + 1) * FUSED as usize] {
                *v *= g;
            }
        }
        let shape = Shape::square(FUSED, DIM, 1, 1, 0);
        Ok(Fuse {
            dw: self.gpu.upload(&bytes(&dw)),
            dwb: self.gpu.upload(&bytes(&dwb)),
            norm: self.norm(&format!("{at}.norm"), DIM, TRUNK_EPS)?,
            up: self.linear(&format!("{at}.pwconv1"), (DIM, FUSED), Act::Gelu)?,
            down: ops::conv(self.gpu, shape, &w, Some(&b), Act::None, None),
        })
    }

    fn level(&self, at: &str) -> Result<Level> {
        let c = format!("{at}.cross_attn_image");
        Ok(Level {
            own: self.attn(&format!("{at}.self_attn"), DIM)?,
            norm1: self.norm(&format!("{at}.norm1"), DIM, ATTENTION_EPS)?,
            to_memory: Attn {
                q: self.linear(&format!("{c}.q_proj"), (DIM, DIM), Act::None)?,
                k: self.linear(&format!("{c}.k_proj"), (MEM, DIM), Act::None)?,
                v: self.linear(&format!("{c}.v_proj"), (MEM, DIM), Act::None)?,
                out: self.linear(&format!("{c}.out_proj"), (DIM, DIM), Act::None)?,
                inner: DIM,
            },
            norm2: self.norm(&format!("{at}.norm2"), DIM, ATTENTION_EPS)?,
            up: self.linear(&format!("{at}.linear1"), (DIM, FFN), Act::Leaky(0.0))?,
            down: self.linear(&format!("{at}.linear2"), (FFN, DIM), Act::None)?,
            norm3: self.norm(&format!("{at}.norm3"), DIM, ATTENTION_EPS)?,
        })
    }

    pub(super) fn memory(&self) -> Result<Memory> {
        let e = "memory_encoder";
        let ds = format!("{e}.mask_downsampler.encoder");
        let mut down = Vec::new();
        let mut cin = 1;
        for i in 0..4 {
            let cout = cin * 4;
            let w = self.floats(&format!("{ds}.{}.weight", 3 * i), &[cout, cin, 3, 3])?;
            let b = self.floats(&format!("{ds}.{}.bias", 3 * i), &[cout])?;
            let shape = Shape::square(cin, cout, 3, 2, 1);
            let conv = ops::conv(self.gpu, shape, &w, Some(&b), Act::None, None);
            down.push((
                conv,
                self.norm(&format!("{ds}.{}", 3 * i + 1), cout, TRUNK_EPS)?,
            ));
            cin = cout;
        }
        let ow = self.floats(&format!("{e}.out_proj.weight"), &[MEM, DIM, 1, 1])?;
        let ob = self.floats(&format!("{e}.out_proj.bias"), &[MEM])?;
        let empty = self.floats("no_obj_embed_spatial", &[1, MEM])?;
        let eb: Vec<f32> = ob.iter().zip(&empty).map(|(a, b)| a + b).collect();
        let out = |b: &[f32]| {
            let shape = Shape::square(DIM, MEM, 1, 1, 0);
            ops::conv(self.gpu, shape, &ow, Some(b), Act::None, None)
        };
        let levels = (0..LEVELS)
            .map(|i| self.level(&format!("memory_attention.layers.{i}")))
            .collect::<Result<Vec<_>>>()?;
        let tpos = self.floats("maskmem_tpos_enc", &[MEMORIES as u32, 1, 1, MEM])?;
        let mem_sine = sine(EMBED, (MEM / 2) as usize);
        let memory_pos = (0..MEMORIES)
            .map(|i| {
                let add = &tpos[i * MEM as usize..(i + 1) * MEM as usize];
                let v = tensor(self.gpu, 1, EMBED, EMBED, MEM);
                let all: Vec<f32> = (mem_sine.chunks(MEM as usize))
                    .flat_map(|px| px.iter().zip(add).map(|(a, b)| a + b))
                    .collect();
                self.gpu.queue.write_buffer(&v.buf, 0, &bytes(&all));
                v
            })
            .collect();
        let vision_pos = tensor(self.gpu, 1, EMBED, EMBED, DIM);
        let vs = sine(EMBED, (DIM / 2) as usize);
        self.gpu.queue.write_buffer(&vision_pos.buf, 0, &bytes(&vs));
        let no_mem = self.floats("no_mem_embed", &[1, 1, DIM])?;
        let none = self.floats("sam_prompt_encoder.no_mask_embed.weight", &[1, DIM])?;
        let shift: Vec<f32> = no_mem.iter().zip(&none).map(|(a, b)| a + b).collect();
        Ok(Memory {
            down,
            down_out: self.pointwise(&format!("{ds}.12"), (DIM, DIM), &[])?,
            pix: self.pointwise(&format!("{e}.pix_feat_proj"), (DIM, DIM), &[])?,
            fuse: (0..2)
                .map(|i| self.fuse(&format!("{e}.fuser.layers.{i}")))
                .collect::<Result<Vec<_>>>()?,
            out: [out(&ob), out(&eb)],
            levels,
            norm: self.norm("memory_attention.norm", DIM, ATTENTION_EPS)?,
            pointer: self.mlp("obj_ptr_proj", [DIM, DIM, DIM, DIM], Act::None)?,
            tpos: (
                self.floats("obj_ptr_tpos_proj.weight", &[MEM, DIM])?,
                self.floats("obj_ptr_tpos_proj.bias", &[MEM])?,
            ),
            no_pointer: self.floats("no_obj_ptr", &[1, DIM])?,
            turns: self.gpu.upload(&bytes(&turns())),
            vision_pos,
            memory_pos,
            shift: tiled(self.gpu, &shift),
            none: tiled(self.gpu, &none),
        })
    }
}

impl Memory {
    fn pointer_pos(&self, pos: f64) -> Vec<f32> {
        let sine = line_sine(pos);
        let (w, b) = &self.tpos;
        (0..MEM as usize)
            .map(|o| {
                let row = &w[o * DIM as usize..(o + 1) * DIM as usize];
                let dot: f64 = row.iter().zip(&sine).map(|(a, x)| f64::from(a * x)).sum();
                (dot + f64::from(b[o])) as f32
            })
            .collect()
    }
}

pub(in crate::mask) struct Work {
    big: View,
    cut: View,
    down: Vec<(View, View)>,
    masks: View,
    x: [View; 2],
    t: View,
    tn: View,
    hid: View,
    mem: View,
    mpos: View,
    mk: View,
    a: [View; 2],
    n: View,
    q: View,
    k: View,
    v: View,
    o: View,
    out: View,
    gemm: kernels::Gemm,
    pub(super) pa: View,
    pub(super) pb: View,
    pub(super) ptr: View,
}

pub(super) fn work(gpu: &Gpu) -> Work {
    let image = |side: u32, c: u32| tensor(gpu, 1, side, side, c);
    let rows = |n: u32, c: u32| tensor(gpu, 1, 1, n, c);
    let mut down = Vec::new();
    let (mut side, mut c) = (SIDE_IN / 2, 4);
    for _ in 0..4 {
        down.push((image(side, c), image(side, c)));
        side /= 2;
        c *= 4;
    }
    Work {
        big: image(SIDE_IN, 1),
        cut: image(SIDE_IN, 1),
        down,
        masks: image(EMBED, DIM),
        x: [image(EMBED, DIM), image(EMBED, DIM)],
        t: image(EMBED, DIM),
        tn: image(EMBED, DIM),
        hid: flat(gpu, TOKENS * FFN.max(FUSED)),
        mem: rows(MOST_KEYS, MEM),
        mpos: rows(MOST_KEYS, MEM),
        mk: rows(MOST_KEYS, MEM),
        a: [rows(TOKENS, DIM), rows(TOKENS, DIM)],
        n: rows(TOKENS, DIM),
        q: rows(TOKENS, DIM),
        k: rows(MOST_KEYS, DIM),
        v: rows(MOST_KEYS, DIM),
        o: rows(TOKENS, DIM),
        out: rows(TOKENS, DIM),
        gemm: kernels::Gemm::new(gpu, DIM, MOST_KEYS, QUERIES),
        pa: rows(4, DIM),
        pb: rows(4, DIM),
        ptr: rows(4, DIM),
    }
}

const SIDE_IN: u32 = super::SIDE;

pub(in crate::mask) struct Bank {
    slots: View,
    pointers: VecDeque<(usize, Vec<f32>)>,
    steps: usize,
}

impl Bank {
    pub(super) fn new(gpu: &Gpu) -> Self {
        Self {
            slots: tensor(gpu, MEMORIES as u32, EMBED, EMBED, MEM),
            pointers: VecDeque::new(),
            steps: 0,
        }
    }

    fn slot(&self, step: usize) -> View {
        let i = match step {
            0 => 0,
            s => 1 + (s - 1) % (MEMORIES - 1),
        };
        self.slots.frame(i as u32)
    }

    fn kept(&mut self, pointer: Vec<f32>) {
        self.pointers.push_back((self.steps, pointer));
        self.steps += 1;
        let next = self.steps;
        self.pointers
            .retain(|(s, _)| *s == 0 || s + POINTERS > next);
    }
}

fn rows_of(v: &View, from: u32, count: u32) -> View {
    View {
        base: v.base + from * v.stride,
        w: count,
        ..v.clone()
    }
}

pub(super) struct Recall {
    memories: Vec<(usize, usize)>,
    pointers: Vec<(f64, Vec<f32>)>,
}

impl Recall {
    pub(super) fn of(bank: &Bank, frames: usize) -> Self {
        let k = bank.steps;
        let mut memories = vec![(0, MEMORIES - 1)];
        for t_pos in 1..MEMORIES {
            let back = MEMORIES - t_pos;
            if k > back {
                memories.push((k - back, MEMORIES - 1 - t_pos));
            }
        }
        let most = frames.clamp(2, POINTERS);
        let span = (most - 1) as f64;
        let mut pointers = Vec::new();
        for (s, p) in bank.pointers.iter().filter(|(s, _)| *s == 0) {
            pointers.push(((k - s) as f64 / span, p.clone()));
        }
        for t_diff in 1..most {
            if k <= t_diff {
                break;
            }
            let s = k - t_diff;
            if let Some((_, p)) = bank.pointers.iter().find(|(v, _)| *v == s) {
                pointers.push((t_diff as f64 / span, p.clone()));
            }
        }
        Self { memories, pointers }
    }

    fn keys(&self) -> (u32, u32) {
        let spatial = self.memories.len() as u32 * TOKENS;
        (spatial, spatial + self.pointers.len() as u32 * SPLIT)
    }
}

impl Net {
    pub(super) fn recall_upload(&self, w: &Work, r: &Recall) {
        let mem = &self.m.memory;
        let (spatial, _) = r.keys();
        let mut tokens = Vec::new();
        let mut pos = Vec::new();
        for (at, p) in &r.pointers {
            let tp = mem.pointer_pos(*at);
            tokens.extend(p.iter().copied());
            for _ in 0..SPLIT {
                pos.extend(tp.iter().copied());
            }
        }
        let at = |v: &View| u64::from(v.base + spatial * v.stride) * 4;
        self.gpu
            .queue
            .write_buffer(&w.mem.buf, at(&w.mem), &bytes(&tokens));
        self.gpu
            .queue
            .write_buffer(&w.mpos.buf, at(&w.mpos), &bytes(&pos));
    }

    pub(super) fn recall(&self, ops: &mut Ops, w: &Work, bank: &Bank, r: &Recall) -> View {
        let mem = &self.m.memory;
        let s = &self.m.space;
        let (spatial, all) = r.keys();
        for (i, (step, idx)) in r.memories.iter().enumerate() {
            let at = i as u32 * TOKENS;
            let to = |v: &View| rows_of(v, at, TOKENS);
            let slot = bank.slot(*step).shaped(1, 1, TOKENS, MEM);
            ops.copy(&slot, &to(&w.mem), 1.0, 0.0, Act::None);
            let pos = mem.memory_pos[*idx].shaped(1, 1, TOKENS, MEM);
            ops.copy(&pos, &to(&w.mpos), 1.0, 0.0, Act::None);
        }
        let keys = |v: &View| rows_of(v, 0, all);
        ops.combine(&keys(&w.mem), &keys(&w.mpos), &keys(&w.mk), 1.0, 1.0);
        let vision = s.src.shaped(1, 1, TOKENS, DIM);
        let vpos = mem.vision_pos.shaped(1, 1, TOKENS, DIM);
        ops.combine(&vision, &vpos, &w.a[0], 1.0, VISION_POS);
        let mut cur = 0;
        for l in &mem.levels {
            Self::norm(ops, &l.norm1, &w.a[cur], &w.n);
            let own_k = rows_of(&w.k, 0, TOKENS);
            let own_v = rows_of(&w.v, 0, TOKENS);
            ops.conv(&l.own.q, &w.n, &w.q, Res::None);
            ops.conv(&l.own.k, &w.n, &own_k, Res::None);
            ops.conv(&l.own.v, &w.n, &own_v, Res::None);
            kernels::rope(ops, &self.m.own, &mem.turns, &w.q, TOKENS);
            kernels::rope(ops, &self.m.own, &mem.turns, &own_k, TOKENS);
            self.attend(ops, w, (&own_k, &own_v));
            ops.conv(&l.own.out, &w.o, &w.a[1 - cur], Res::After(&w.a[cur]));
            cur = 1 - cur;
            Self::norm(ops, &l.norm2, &w.a[cur], &w.n);
            let ks = rows_of(&w.k, 0, all);
            let vs = rows_of(&w.v, 0, all);
            ops.conv(&l.to_memory.q, &w.n, &w.q, Res::None);
            ops.conv(&l.to_memory.k, &keys(&w.mk), &ks, Res::None);
            ops.conv(&l.to_memory.v, &keys(&w.mem), &vs, Res::None);
            kernels::rope(ops, &self.m.own, &mem.turns, &w.q, TOKENS);
            kernels::rope(ops, &self.m.own, &mem.turns, &ks, spatial);
            self.attend(ops, w, (&ks, &vs));
            ops.conv(&l.to_memory.out, &w.o, &w.a[1 - cur], Res::After(&w.a[cur]));
            cur = 1 - cur;
            Self::norm(ops, &l.norm3, &w.a[cur], &w.n);
            let hid = w.hid.shaped(1, 1, TOKENS, FFN);
            ops.conv(&l.up, &w.n, &hid, Res::None);
            ops.conv(&l.down, &hid, &w.a[1 - cur], Res::After(&w.a[cur]));
            cur = 1 - cur;
        }
        Self::norm(ops, &mem.norm, &w.a[cur], &w.out);
        w.out.shaped(1, EMBED, EMBED, DIM)
    }

    fn attend(&self, ops: &mut Ops, w: &Work, (k, v): (&View, &View)) {
        let qkvo = (&w.q, k, v, &w.o);
        kernels::attend_gemm(ops, &self.m.own, &w.gemm, qkvo);
    }

    pub(super) fn remember(
        &self,
        w: &Work,
        bank: &mut Bank,
        (pick, way): (usize, Remembered),
        pointer: Vec<f32>,
    ) -> Result<()> {
        let gpu = &self.gpu;
        let mem = &self.m.memory;
        let s = &self.m.space;
        let slot = bank.slot(bank.steps);
        self.checked(|| {
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                let mut ops = Ops {
                    gpu,
                    k: &self.m.basic,
                    pass: &mut pass,
                };
                let ops = &mut ops;
                let low = s.masks.part(pick as u32, 1);
                let scale = MAP as f32 / SIDE_IN as f32;
                ops.resize(&low, &w.big, (scale, scale), 1.0, 0);
                let mm = (MASK_SCALE, MASK_BIAS);
                kernels::memory_mask(ops, &self.m.own, mm, way, (&w.big, &w.cut));
                let mut input = w.cut.clone();
                for ((conv, norm), (raw, normed)) in mem.down.iter().zip(&w.down) {
                    ops.conv(conv, &input, raw, Res::None);
                    Self::norm(ops, norm, raw, normed);
                    ops.copy(normed, raw, 1.0, 0.0, Act::Gelu);
                    input = raw.clone();
                }
                ops.conv(&mem.down_out, &input, &w.masks, Res::None);
                ops.conv(&mem.pix, &s.src, &w.x[0], Res::After(&w.masks));
                for (i, f) in mem.fuse.iter().enumerate() {
                    let (x, y) = (&w.x[i % 2], &w.x[1 - i % 2]);
                    ops.dwconv(&f.dw, &f.dwb, 7, Act::None, x, &w.t);
                    Self::norm(ops, &f.norm, &w.t, &w.tn);
                    let hid = w.hid.shaped(1, EMBED, EMBED, FUSED);
                    ops.conv(&f.up, &w.tn, &hid, Res::None);
                    ops.conv(&f.down, &hid, y, Res::After(x));
                }
                let x = &w.x[mem.fuse.len() % 2];
                let out = &mem.out[usize::from(way == Remembered::Empty)];
                ops.conv(out, x, &slot, Res::None);
            }
            gpu.flush();
            gpu.queue.submit([enc.finish()]);
            Ok(())
        })?;
        bank.kept(pointer);
        Ok(())
    }

    pub(super) fn no_pointer(&self) -> Vec<f32> {
        self.m.memory.no_pointer.clone()
    }
}
