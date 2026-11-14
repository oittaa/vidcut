use std::path::Path;

use super::gemm::{self, Act, Gemm, Load, Store};
use super::{DIM, FFN, LAYERS};
use crate::compute::Gpu;
use crate::compute::{bytes, halves};
use crate::error::Result;
use crate::weights::{Safetensors, Torch};

const TITLE: &str = vidcut_core::catalog::FLASHVSR.title;

pub(super) struct Wino {
    pub(super) u: Vec<wgpu::Buffer>,
    pub(super) zero: wgpu::Buffer,
}

pub(super) struct Linear {
    pub(super) w: wgpu::Buffer,
    pub(super) b: wgpu::Buffer,
    pub(super) n: u32,
    pub(super) k: u32,
    pub(super) wino: Option<Wino>,
}

pub(super) struct Block {
    pub(super) q: Linear,
    pub(super) k: Linear,
    pub(super) v: Linear,
    pub(super) o: Linear,
    pub(super) norm_q: wgpu::Buffer,
    pub(super) norm_k: wgpu::Buffer,
    pub(super) cq: Linear,
    pub(super) co: Linear,
    pub(super) cnorm_q: wgpu::Buffer,
    pub(super) ctx_k: wgpu::Buffer,
    pub(super) ctx_v: wgpu::Buffer,
    pub(super) ffn1: Linear,
    pub(super) ffn2: Linear,
    pub(super) mul_msa: wgpu::Buffer,
    pub(super) add_msa: wgpu::Buffer,
    pub(super) gate_msa: wgpu::Buffer,
    pub(super) mul_mlp: wgpu::Buffer,
    pub(super) add_mlp: wgpu::Buffer,
    pub(super) gate_mlp: wgpu::Buffer,
    pub(super) mul3: wgpu::Buffer,
    pub(super) add3: wgpu::Buffer,
}

pub(super) struct Lq {
    pub(super) conv1: Linear,
    pub(super) norm1: wgpu::Buffer,
    pub(super) conv2: Linear,
    pub(super) norm2: wgpu::Buffer,
    pub(super) linear: Linear,
}

pub(super) struct MemBlock {
    pub(super) a: Linear,
    pub(super) b: Linear,
    pub(super) c: Linear,
}

pub(super) struct Decoder {
    pub(super) first: Linear,
    pub(super) deepen1: Linear,
    pub(super) stage1: Vec<MemBlock>,
    pub(super) grow1: Linear,
    pub(super) narrow2: Linear,
    pub(super) stage2: Vec<MemBlock>,
    pub(super) grow2: Linear,
    pub(super) narrow3: Linear,
    pub(super) stage3: Vec<MemBlock>,
    pub(super) grow3: [Linear; 2],
    pub(super) last1: Linear,
    pub(super) deepen2: Linear,
    pub(super) last2: Linear,
}

pub(super) struct Model {
    pub(super) patch: Linear,
    pub(super) blocks: Vec<Block>,
    pub(super) head: Linear,
    pub(super) head_mul: wgpu::Buffer,
    pub(super) head_add: wgpu::Buffer,
    pub(super) lq: Lq,
    pub(super) dec: Decoder,
    pub(super) ones: wgpu::Buffer,
    pub(super) bytes: u64,
}

fn conv_order(w: &[f32], n: usize, cin: usize, taps: usize) -> Vec<f32> {
    let mut out = vec![0f32; w.len()];
    for o in 0..n {
        for c in 0..cin {
            for t in 0..taps {
                out[(o * taps + t) * cin + c] = w[(o * cin + c) * taps + t];
            }
        }
    }
    out
}

fn linear(gpu: &Gpu, w: &[f32], b: Option<&[f32]>, n: usize, k: usize) -> Linear {
    let bias = b.map_or_else(|| vec![0f32; n], <[f32]>::to_vec);
    Linear {
        w: gpu.upload(&halves(w)),
        b: gpu.upload(&bytes(&bias)),
        n: n as u32,
        k: k as u32,
        wino: None,
    }
}

fn wino(gpu: &Gpu, w: &[f32], b: Option<&[f32]>, n: usize, cin: usize) -> Linear {
    let mut u = vec![vec![0f32; n * cin]; 16];
    for co in 0..n {
        for ci in 0..cin {
            let g = |y: usize, x: usize| f64::from(w[((co * cin + ci) * 3 + y) * 3 + x]);
            let cols = [0, 1, 2].map(|x| {
                [
                    g(0, x),
                    (g(0, x) + g(1, x) + g(2, x)) / 2.0,
                    (g(0, x) - g(1, x) + g(2, x)) / 2.0,
                    g(2, x),
                ]
            });
            for r in 0..4 {
                let h = [cols[0][r], cols[1][r], cols[2][r]];
                let v = [
                    h[0],
                    (h[0] + h[1] + h[2]) / 2.0,
                    (h[0] - h[1] + h[2]) / 2.0,
                    h[2],
                ];
                for (c, v) in v.iter().enumerate() {
                    u[r * 4 + c][co * cin + ci] = *v as f32;
                }
            }
        }
    }
    let bias = b.map_or_else(|| vec![0f32; n], <[f32]>::to_vec);
    Linear {
        w: gpu.buffer(0),
        b: gpu.upload(&bytes(&bias)),
        n: n as u32,
        k: (cin * 9) as u32,
        wino: Some(Wino {
            u: u.iter().map(|x| gpu.upload(&halves(x))).collect(),
            zero: gpu.upload(&bytes(&vec![0f32; n])),
        }),
    }
}

fn silu(v: f64) -> f64 {
    v / (1.0 + (-v).exp())
}

fn gemv(w: &[f32], b: &[f32], x: &[f64]) -> Vec<f64> {
    let k = x.len();
    b.iter()
        .enumerate()
        .map(|(o, bias)| {
            let row = &w[o * k..(o + 1) * k];
            row.iter()
                .zip(x)
                .map(|(a, v)| f64::from(*a) * v)
                .sum::<f64>()
                + f64::from(*bias)
        })
        .collect()
}

fn times(st: &mut Safetensors) -> Result<(Vec<f64>, Vec<f64>)> {
    let half = 128;
    let mut emb = vec![0f64; 256];
    for i in 0..half {
        let f = 1000.0 * 10000f64.powf(-(i as f64) / half as f64);
        emb[i] = f.cos();
        emb[half + i] = f.sin();
    }
    let w0 = st.floats("time_embedding.0.weight", &[DIM, 256])?;
    let b0 = st.floats("time_embedding.0.bias", &[DIM])?;
    let w2 = st.floats("time_embedding.2.weight", &[DIM, DIM])?;
    let b2 = st.floats("time_embedding.2.bias", &[DIM])?;
    let h: Vec<f64> = gemv(&w0, &b0, &emb).into_iter().map(silu).collect();
    let t = gemv(&w2, &b2, &h);
    let wp = st.floats("time_projection.1.weight", &[6 * DIM, DIM])?;
    let bp = st.floats("time_projection.1.bias", &[6 * DIM])?;
    let s: Vec<f64> = t.iter().copied().map(silu).collect();
    let m = gemv(&wp, &bp, &s);
    Ok((t, m))
}

fn floats(v: impl Iterator<Item = f64>) -> Vec<u8> {
    bytes(&v.map(|x| x as f32).collect::<Vec<_>>())
}

struct Shift<'a> {
    base: &'a [f32],
    add: &'a [f64],
}

impl Shift<'_> {
    fn part(&self, j: usize) -> impl Iterator<Item = f64> + '_ {
        (0..DIM).map(move |c| f64::from(self.base[j * DIM + c]) + self.add[j * DIM + c])
    }
}

fn block(gpu: &Gpu, st: &mut Safetensors, i: usize, t_mod: &[f64]) -> Result<Block> {
    let name = |s: &str| format!("blocks.{i}.{s}");
    let lin = |st: &mut Safetensors, s: &str, n: usize, k: usize| -> Result<Linear> {
        let w = st.floats(&name(&format!("{s}.weight")), &[n, k])?;
        let b = st.floats(&name(&format!("{s}.bias")), &[n])?;
        Ok(linear(gpu, &w, Some(&b), n, k))
    };
    let q = lin(st, "self_attn.q", DIM, DIM)?;
    let k = lin(st, "self_attn.k", DIM, DIM)?;
    let v = lin(st, "self_attn.v", DIM, DIM)?;
    let o = lin(st, "self_attn.o", DIM, DIM)?;
    let cq = lin(st, "cross_attn.q", DIM, DIM)?;
    let co = lin(st, "cross_attn.o", DIM, DIM)?;
    let ffn1 = lin(st, "ffn.0", FFN, DIM)?;
    let ffn2 = lin(st, "ffn.2", DIM, FFN)?;
    let vec = |st: &mut Safetensors, s: &str| -> Result<wgpu::Buffer> {
        Ok(gpu.upload(&bytes(&st.floats(&name(s), &[DIM])?)))
    };
    let norm_q = vec(st, "self_attn.norm_q.weight")?;
    let norm_k = vec(st, "self_attn.norm_k.weight")?;
    let cnorm_q = vec(st, "cross_attn.norm_q.weight")?;
    let mul3 = vec(st, "norm3.weight")?;
    let add3 = vec(st, "norm3.bias")?;
    let modulation = st.floats(&name("modulation"), &[1, 6, DIM])?;
    let m = Shift {
        base: &modulation,
        add: t_mod,
    };
    let up = |v: Vec<u8>| gpu.upload(&v);
    Ok(Block {
        q,
        k,
        v,
        o,
        norm_q,
        norm_k,
        cq,
        co,
        cnorm_q,
        ctx_k: gpu.buffer(0),
        ctx_v: gpu.buffer(0),
        ffn1,
        ffn2,
        add_msa: up(floats(m.part(0))),
        mul_msa: up(floats(m.part(1).map(|s| 1.0 + s))),
        gate_msa: up(floats(m.part(2))),
        add_mlp: up(floats(m.part(3))),
        mul_mlp: up(floats(m.part(4).map(|s| 1.0 + s))),
        gate_mlp: up(floats(m.part(5))),
        mul3,
        add3,
    })
}

fn conv(
    t: &Torch,
    gpu: &Gpu,
    i: &str,
    n: usize,
    cin: usize,
    size: usize,
    bias: bool,
) -> Result<Linear> {
    let w = t.floats(&format!("decoder.{i}.weight"), &[n, cin, size, size])?;
    let b = bias
        .then(|| t.floats(&format!("decoder.{i}.bias"), &[n]))
        .transpose()?;
    if size == 3 && n > 3 {
        return Ok(wino(gpu, &w, b.as_deref(), n, cin));
    }
    let w = conv_order(&w, n, cin, size * size);
    Ok(linear(gpu, &w, b.as_deref(), n, cin * size * size))
}

fn halves_of(t: &Torch, gpu: &Gpu, i: &str, cin: usize) -> Result<[Linear; 2]> {
    let w = t.floats(&format!("decoder.{i}.weight"), &[2 * cin, cin, 1, 1])?;
    let (first, second) = w.split_at(cin * cin);
    Ok([
        linear(gpu, first, None, cin, cin),
        linear(gpu, second, None, cin, cin),
    ])
}

fn mem_block(t: &Torch, gpu: &Gpu, i: usize, n: usize) -> Result<MemBlock> {
    Ok(MemBlock {
        a: conv(t, gpu, &format!("{i}.conv.0"), n, 2 * n, 3, true)?,
        b: conv(t, gpu, &format!("{i}.conv.2"), n, n, 3, true)?,
        c: conv(t, gpu, &format!("{i}.conv.4"), n, n, 3, true)?,
    })
}

fn decoder(gpu: &Gpu, path: &Path) -> Result<Decoder> {
    let t = Torch::open(path, TITLE)?;
    let stage = |first: usize, n: usize| -> Result<Vec<MemBlock>> {
        (first..first + 3)
            .map(|i| mem_block(&t, gpu, i, n))
            .collect()
    };
    Ok(Decoder {
        first: conv(&t, gpu, "1", 512, 784, 3, true)?,
        deepen1: conv(&t, gpu, "3", 512, 512, 3, false)?,
        stage1: stage(5, 512)?,
        grow1: conv(&t, gpu, "9.conv", 512, 512, 1, false)?,
        narrow2: conv(&t, gpu, "10", 256, 512, 3, false)?,
        stage2: stage(11, 256)?,
        grow2: conv(&t, gpu, "15.conv", 512, 256, 1, false)?,
        narrow3: conv(&t, gpu, "16", 128, 256, 3, false)?,
        stage3: stage(17, 128)?,
        grow3: halves_of(&t, gpu, "21.conv", 128)?,
        last1: conv(&t, gpu, "22", 128, 128, 3, false)?,
        deepen2: conv(&t, gpu, "24", 128, 128, 3, false)?,
        last2: conv(&t, gpu, "26", 3, 128, 3, true)?,
    })
}

fn lq(gpu: &Gpu, path: &Path) -> Result<Lq> {
    let mut st = Safetensors::open(path, TITLE)?;
    let w1 = st.floats("conv1.weight", &[2048, 768, 4, 3, 3])?;
    let w1 = conv_order(&w1, 2048, 768, 36);
    let b1 = st.floats("conv1.bias", &[2048])?;
    let conv1 = linear(gpu, &w1, Some(&b1), 2048, 768 * 36);
    drop(w1);
    let norm1 = gpu.upload(&bytes(&st.floats("norm1.weight", &[2048, 1, 1, 1])?));
    let w2 = st.floats("conv2.weight", &[3072, 2048, 4, 3, 3])?;
    let w2 = conv_order(&w2, 3072, 2048, 36);
    let b2 = st.floats("conv2.bias", &[3072])?;
    let conv2 = linear(gpu, &w2, Some(&b2), 3072, 2048 * 36);
    drop(w2);
    let norm2 = gpu.upload(&bytes(&st.floats("norm2.weight", &[3072, 1, 1, 1])?));
    let wl = st.floats("linear_layers.0.weight", &[DIM, 3072])?;
    let bl = st.floats("linear_layers.0.bias", &[DIM])?;
    Ok(Lq {
        conv1,
        norm1,
        conv2,
        norm2,
        linear: linear(gpu, &wl, Some(&bl), DIM, 3072),
    })
}

pub(super) struct Paths<'a> {
    pub(super) dit: &'a Path,
    pub(super) lq: &'a Path,
    pub(super) decoder: &'a Path,
    pub(super) prompt: &'a Path,
}

pub(super) fn load(gpu: &Gpu, paths: &Paths) -> Result<Model> {
    let before = gpu.uploaded();
    let mut st = Safetensors::open(paths.dit, TITLE)?;
    let (t_emb, t_mod) = times(&mut st)?;
    let pw = st.floats("patch_embedding.weight", &[DIM, 16, 1, 2, 2])?;
    let pw = conv_order(&pw, DIM, 16, 4);
    let pb = st.floats("patch_embedding.bias", &[DIM])?;
    let patch = linear(gpu, &pw, Some(&pb), DIM, 64);
    let mut blocks = (0..LAYERS)
        .map(|i| block(gpu, &mut st, i, &t_mod))
        .collect::<Result<Vec<_>>>()?;
    let hw = st.floats("head.proj.weight", &[64, DIM])?;
    let hb = st.floats("head.proj.bias", &[64])?;
    let head = linear(gpu, &hw, Some(&hb), 64, DIM);
    let hm = st.floats("head.modulation", &[1, 2, DIM])?;
    let head_add = gpu.upload(&floats((0..DIM).map(|c| f64::from(hm[c]) + t_emb[c])));
    let head_mul = gpu.upload(&floats(
        (0..DIM).map(|c| 1.0 + f64::from(hm[DIM + c]) + t_emb[c]),
    ));
    let ones = gpu.upload(&bytes(&vec![1f32; FFN]));
    let kept = gpu.uploaded() - before;
    let zeros = gpu.upload(&bytes(&vec![0f32; FFN]));
    context(gpu, &mut st, paths.prompt, &mut blocks, &zeros)?;
    let after = gpu.uploaded();
    drop(st);
    let lq = lq(gpu, paths.lq)?;
    let dec = decoder(gpu, paths.decoder)?;
    Ok(Model {
        patch,
        blocks,
        head,
        head_mul,
        head_add,
        lq,
        dec,
        ones,
        bytes: kept + (gpu.uploaded() - after) + 2 * (LAYERS * PROMPT * DIM * 2) as u64,
    })
}

const PROMPT: usize = 512;
const TEXT: usize = 4096;

fn context(
    gpu: &Gpu,
    st: &mut Safetensors,
    prompt: &Path,
    blocks: &mut [Block],
    zeros: &wgpu::Buffer,
) -> Result<()> {
    let p = Torch::open(prompt, TITLE)?.floats("", &[1, PROMPT, TEXT])?;
    let text = gpu.upload(&halves(&p));
    let lin = |st: &mut Safetensors, s: &str, n: usize, k: usize| -> Result<Linear> {
        let w = st.floats(&format!("{s}.weight"), &[n, k])?;
        let b = st.floats(&format!("{s}.bias"), &[n])?;
        Ok(linear(gpu, &w, Some(&b), n, k))
    };
    let t0 = lin(st, "text_embedding.0", DIM, TEXT)?;
    let t2 = lin(st, "text_embedding.2", DIM, DIM)?;
    let hidden = gpu.buffer(PROMPT * DIM * 2);
    let ctx = gpu.buffer(PROMPT * DIM * 2);
    let mut keys = Vec::new();
    for (i, _) in blocks.iter().enumerate() {
        keys.push((
            lin(st, &format!("blocks.{i}.cross_attn.k"), DIM, DIM)?,
            lin(st, &format!("blocks.{i}.cross_attn.v"), DIM, DIM)?,
            gpu.upload(&bytes(
                &st.floats(&format!("blocks.{i}.cross_attn.norm_k.weight"), &[DIM])?,
            )),
        ));
    }
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        let dense = |pass: &mut wgpu::ComputePass,
                     l: &Linear,
                     a: &wgpu::Buffer,
                     out: &wgpu::Buffer,
                     act: Act| {
            let mut params = [0u32; 16];
            params[0] = l.k / 2;
            gemm::run(
                gpu,
                pass,
                &Gemm {
                    m: PROMPT as u32,
                    n: l.n,
                    k: l.k,
                    load: Load::Rows,
                    store: Store::Half(act),
                    params,
                    weights: &l.w,
                    bias: &l.b,
                    a,
                    a2: None,
                    out,
                    aux: None,
                },
            );
        };
        dense(&mut pass, &t0, &text, &hidden, Act::Gelu);
        dense(&mut pass, &t2, &hidden, &ctx, Act::None);
        for (b, (k, v, norm)) in blocks.iter_mut().zip(&keys) {
            b.ctx_k = gpu.buffer(PROMPT * DIM * 2);
            b.ctx_v = gpu.buffer(PROMPT * DIM * 2);
            dense(&mut pass, k, &ctx, &b.ctx_k, Act::None);
            dense(&mut pass, v, &ctx, &b.ctx_v, Act::None);
            super::kernels::rms(gpu, &mut pass, &b.ctx_k, norm, zeros, PROMPT as u32, None);
        }
    }
    gpu.flush();
    gpu.queue.submit([enc.finish()]);
    gpu.wait()?;
    Ok(())
}
