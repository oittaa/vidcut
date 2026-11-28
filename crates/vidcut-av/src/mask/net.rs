mod memory;

use std::path::Path;

pub(super) use memory::{Bank, Work};

use super::kernels::{self, Axis, Grid, Heads, Kernels, Rows};
use super::{Click, Config, Found};
use crate::compute::ops::{self, Act, Conv, Kernels as Basic, Ops, Res, Shape, View, tensor};
use crate::compute::{self, Gpu, bytes, words};
use crate::error::{Error, Result};
use crate::weights::Torch;

pub(super) const SIDE: u32 = 1024;
const MAP: u32 = vidcut_core::masks::SIDE as u32;
const FEAT: u32 = SIDE / 4;
const EMBED: u32 = 64;
const DIM: u32 = 256;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
const TRUNK_EPS: f32 = 1e-6;
const DECODER_EPS: f32 = 1e-5;
const MASKS: u32 = 4;
const OUTPUTS: u32 = 2 + MASKS;
const HEADS: u32 = 8;
const MLP: u32 = 2048;
const CROSS: u32 = DIM / 2;
const CUBIC: f64 = -0.75;
const HYPER: u32 = DIM / 8;
const STABLE_DELTA: f32 = 0.05;
const STABLE: f32 = 0.98;
const NO_OBJECT: f32 = -1024.0;
const MOST_CLICKS: usize = vidcut_core::masks::MOST_CLICKS;
const MOST_TOKENS: u32 = OUTPUTS + MOST_CLICKS as u32 + 1;

struct Plan {
    dim: u32,
    dout: u32,
    heads: u32,
    window: u32,
    pool: bool,
    side: u32,
    end: Option<usize>,
}

impl Plan {
    fn half(&self) -> u32 {
        if self.pool { self.side / 2 } else { self.side }
    }

    fn ws(&self) -> u32 {
        if self.window > 0 {
            self.window
        } else {
            self.side
        }
    }

    fn grid(&self) -> Grid {
        Grid {
            h: self.side,
            w: self.side,
            ws: self.ws(),
        }
    }

    fn wq(&self) -> u32 {
        if self.pool { self.ws() / 2 } else { self.ws() }
    }
}

fn plans(c: &Config) -> Vec<Plan> {
    let ends: Vec<u32> = c
        .stages
        .iter()
        .scan(0, |sum, n| {
            *sum += n;
            Some(*sum - 1)
        })
        .collect();
    let pools: Vec<u32> = ends[..3].iter().map(|e| e + 1).collect();
    let (mut embed, mut heads, mut stage, mut side) = (c.embed, c.heads, 0usize, FEAT);
    let depth = ends[3] + 1;
    (0..depth)
        .map(|i| {
            let window = if c.global.contains(&i) {
                0
            } else {
                c.windows[stage]
            };
            let mut dout = embed;
            if i > 0 && ends.contains(&(i - 1)) {
                dout = embed * 2;
                heads *= 2;
                stage += 1;
            }
            let pool = pools.contains(&i);
            let plan = Plan {
                dim: embed,
                dout,
                heads,
                window,
                pool,
                side,
                end: ends.iter().position(|e| *e == i),
            };
            embed = dout;
            if pool {
                side /= 2;
            }
            plan
        })
        .collect()
}

struct Norm {
    gamma: wgpu::Buffer,
    beta: wgpu::Buffer,
    eps: f32,
}

struct Block {
    plan: Plan,
    norm1: Norm,
    qkv: Conv,
    proj: Conv,
    norm2: Norm,
    up: Conv,
    down: Conv,
    short: Option<Conv>,
}

struct Attn {
    q: Conv,
    k: Conv,
    v: Conv,
    out: Conv,
    inner: u32,
}

struct Layer {
    own: Attn,
    norm1: Norm,
    to_image: Attn,
    norm2: Norm,
    mlp: [Conv; 2],
    norm3: Norm,
    norm4: Norm,
    to_tokens: Attn,
}

struct Prompts {
    gauss: Vec<f32>,
    points: [Vec<f32>; 2],
    none: Vec<f32>,
    outputs: Vec<f32>,
}

struct Decoder {
    layers: Vec<Layer>,
    last: Attn,
    norm: Norm,
    up1: Conv,
    norm_up: Norm,
    up2: Conv,
    hyper: Vec<[Conv; 3]>,
    iou: [Conv; 3],
    object: [Conv; 3],
    prompts: Prompts,
}

struct Space {
    x: [View; 2],
    xn: View,
    win: View,
    qkv: View,
    qp: View,
    att: View,
    aw: View,
    hid: View,
    image: View,
    fs0: View,
    fs1: View,
    l2: View,
    l3: View,
    src: View,
    dense: View,
    gemm: kernels::Gemm,
    q: View,
    q2: View,
    qpe: View,
    tq: View,
    k: View,
    k2: View,
    tk: View,
    pq: View,
    pk: View,
    pv: View,
    po: View,
    mlp: View,
    u1: View,
    u1n: View,
    up: View,
    ha: View,
    hb: View,
    hy: View,
    masks: View,
    iou: View,
    object: View,
    dynamic: Conv,
}

pub(super) struct Net {
    gpu: Gpu,
    m: Model,
}

struct Model {
    basic: Basic,
    own: Kernels,
    patch: Conv,
    pos: View,
    blocks: Vec<Block>,
    lateral: Vec<Conv>,
    s0: Conv,
    s1: Conv,
    decoder: Decoder,
    memory: memory::Memory,
    space: Space,
    staging: wgpu::Buffer,
}

struct Load<'a> {
    t: &'a Torch,
    gpu: &'a Gpu,
}

fn sizes(shape: &[u32]) -> Vec<usize> {
    shape.iter().map(|&d| d as usize).collect()
}

impl Load<'_> {
    fn floats(&self, name: &str, shape: &[u32]) -> Result<Vec<f32>> {
        self.t.floats(&format!("model.{name}"), &sizes(shape))
    }

    fn linear(&self, name: &str, (cin, cout): (u32, u32), act: Act) -> Result<Conv> {
        let w = self.floats(&format!("{name}.weight"), &[cout, cin])?;
        let b = self.floats(&format!("{name}.bias"), &[cout])?;
        let shape = Shape::square(cin, cout, 1, 1, 0);
        Ok(ops::conv(self.gpu, shape, &w, Some(&b), act, None))
    }

    fn pointwise(&self, name: &str, (cin, cout): (u32, u32), shift: &[f32]) -> Result<Conv> {
        let w = self.floats(&format!("{name}.weight"), &[cout, cin, 1, 1])?;
        let mut b = self.floats(&format!("{name}.bias"), &[cout])?;
        for (v, s) in b.iter_mut().zip(shift) {
            *v += s;
        }
        let shape = Shape::square(cin, cout, 1, 1, 0);
        Ok(ops::conv(self.gpu, shape, &w, Some(&b), Act::None, None))
    }

    fn transposed(&self, name: &str, (cin, cout): (u32, u32), act: Act) -> Result<Conv> {
        let w = self.floats(&format!("{name}.weight"), &[cin, cout, 2, 2])?;
        let b = self.floats(&format!("{name}.bias"), &[cout])?;
        let (ci_n, co_n) = (cin as usize, cout as usize);
        let mut laid = vec![0f32; 4 * co_n * ci_n];
        let mut bias = vec![0f32; 4 * co_n];
        for co in 0..co_n {
            for k in 0..4 {
                let o = co * 4 + k;
                bias[o] = b[co];
                for ci in 0..ci_n {
                    laid[o * ci_n + ci] = w[(ci * co_n + co) * 4 + k];
                }
            }
        }
        let shape = Shape::square(cin, cout * 4, 1, 1, 0);
        Ok(ops::conv(self.gpu, shape, &laid, Some(&bias), act, None))
    }

    fn norm(&self, name: &str, c: u32, eps: f32) -> Result<Norm> {
        let gamma = self.floats(&format!("{name}.weight"), &[c])?;
        let beta = self.floats(&format!("{name}.bias"), &[c])?;
        Ok(Norm {
            gamma: self.gpu.upload(&bytes(&gamma)),
            beta: self.gpu.upload(&bytes(&beta)),
            eps,
        })
    }

    fn mlp(&self, name: &str, dims: [u32; 4], last: Act) -> Result<[Conv; 3]> {
        let relu = Act::Leaky(0.0);
        Ok([
            self.linear(&format!("{name}.layers.0"), (dims[0], dims[1]), relu)?,
            self.linear(&format!("{name}.layers.1"), (dims[1], dims[2]), relu)?,
            self.linear(&format!("{name}.layers.2"), (dims[2], dims[3]), last)?,
        ])
    }

    fn attn(&self, name: &str, inner: u32) -> Result<Attn> {
        Ok(Attn {
            q: self.linear(&format!("{name}.q_proj"), (DIM, inner), Act::None)?,
            k: self.linear(&format!("{name}.k_proj"), (DIM, inner), Act::None)?,
            v: self.linear(&format!("{name}.v_proj"), (DIM, inner), Act::None)?,
            out: self.linear(&format!("{name}.out_proj"), (inner, DIM), Act::None)?,
            inner,
        })
    }

    fn block(&self, i: usize, plan: Plan) -> Result<Block> {
        let at = format!("image_encoder.trunk.blocks.{i}");
        let (dim, dout) = (plan.dim, plan.dout);
        let short = match dim != dout {
            true => Some(self.linear(&format!("{at}.proj"), (dim, dout), Act::None)?),
            false => None,
        };
        Ok(Block {
            norm1: self.norm(&format!("{at}.norm1"), dim, TRUNK_EPS)?,
            qkv: self.linear(&format!("{at}.attn.qkv"), (dim, 3 * dout), Act::None)?,
            proj: self.linear(&format!("{at}.attn.proj"), (dout, dout), Act::None)?,
            norm2: self.norm(&format!("{at}.norm2"), dout, TRUNK_EPS)?,
            up: self.linear(&format!("{at}.mlp.layers.0"), (dout, 4 * dout), Act::Gelu)?,
            down: self.linear(&format!("{at}.mlp.layers.1"), (4 * dout, dout), Act::None)?,
            short,
            plan,
        })
    }

    fn decoder(&self) -> Result<Decoder> {
        let d = "sam_mask_decoder";
        let t = format!("{d}.transformer");
        let layers = (0..2)
            .map(|i| {
                let l = format!("{t}.layers.{i}");
                Ok(Layer {
                    own: self.attn(&format!("{l}.self_attn"), DIM)?,
                    norm1: self.norm(&format!("{l}.norm1"), DIM, DECODER_EPS)?,
                    to_image: self.attn(&format!("{l}.cross_attn_token_to_image"), CROSS)?,
                    norm2: self.norm(&format!("{l}.norm2"), DIM, DECODER_EPS)?,
                    mlp: [
                        self.linear(&format!("{l}.mlp.layers.0"), (DIM, MLP), Act::Leaky(0.0))?,
                        self.linear(&format!("{l}.mlp.layers.1"), (MLP, DIM), Act::None)?,
                    ],
                    norm3: self.norm(&format!("{l}.norm3"), DIM, DECODER_EPS)?,
                    norm4: self.norm(&format!("{l}.norm4"), DIM, DECODER_EPS)?,
                    to_tokens: self.attn(&format!("{l}.cross_attn_image_to_token"), CROSS)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let hyper = (0..MASKS)
            .map(|i| {
                let name = format!("{d}.output_hypernetworks_mlps.{i}");
                self.mlp(&name, [DIM, DIM, DIM, HYPER], Act::None)
            })
            .collect::<Result<Vec<_>>>()?;
        let p = "sam_prompt_encoder";
        let mut outputs = self.floats(&format!("{d}.obj_score_token.weight"), &[1, DIM])?;
        outputs.extend(self.floats(&format!("{d}.iou_token.weight"), &[1, DIM])?);
        outputs.extend(self.floats(&format!("{d}.mask_tokens.weight"), &[MASKS, DIM])?);
        let point = |i: u32| self.floats(&format!("{p}.point_embeddings.{i}.weight"), &[1, DIM]);
        Ok(Decoder {
            layers,
            last: self.attn(&format!("{t}.final_attn_token_to_image"), CROSS)?,
            norm: self.norm(&format!("{t}.norm_final_attn"), DIM, DECODER_EPS)?,
            up1: self.transposed(
                &format!("{d}.output_upscaling.0"),
                (DIM, DIM / 4),
                Act::None,
            )?,
            norm_up: self.norm(&format!("{d}.output_upscaling.1"), DIM / 4, TRUNK_EPS)?,
            up2: self.transposed(
                &format!("{d}.output_upscaling.3"),
                (DIM / 4, HYPER),
                Act::Gelu,
            )?,
            hyper,
            iou: self.mlp(
                &format!("{d}.iou_prediction_head"),
                [DIM, DIM, DIM, MASKS],
                Act::Sigmoid,
            )?,
            object: self.mlp(
                &format!("{d}.pred_obj_score_head"),
                [DIM, DIM, DIM, 1],
                Act::None,
            )?,
            prompts: Prompts {
                gauss: self.floats(
                    &format!("{p}.pe_layer.positional_encoding_gaussian_matrix"),
                    &[2, DIM / 2],
                )?,
                points: [point(0)?, point(1)?],
                none: self.floats(&format!("{p}.not_a_point_embed.weight"), &[1, DIM])?,
                outputs,
            },
        })
    }
}

fn cubic_taps(input: u32, output: u32) -> Vec<([usize; 4], [f64; 4])> {
    let a = CUBIC;
    let near = |x: f64| ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0;
    let far = |x: f64| ((a * x - 5.0 * a) * x + 8.0 * a) * x - 4.0 * a;
    let scale = f64::from(input) / f64::from(output);
    (0..output)
        .map(|o| {
            let real = scale * (f64::from(o) + 0.5) - 0.5;
            let i = real.floor();
            let t = real - i;
            let at = |k: i64| (i as i64 + k).clamp(0, i64::from(input) - 1) as usize;
            (
                [at(-1), at(0), at(1), at(2)],
                [far(t + 1.0), near(t), near(1.0 - t), far(2.0 - t)],
            )
        })
        .collect()
}

fn position(bkg: &[f32], b: u32, window: &[f32], ws: u32, c: u32) -> Vec<f32> {
    let taps = cubic_taps(b, FEAT);
    let (b, ws, c, side) = (b as usize, ws as usize, c as usize, FEAT as usize);
    let mut out = vec![0f32; side * side * c];
    for (y, (ys, wy)) in taps.iter().enumerate() {
        for (x, (xs, wx)) in taps.iter().enumerate() {
            for ch in 0..c {
                let plane = &bkg[ch * b * b..(ch + 1) * b * b];
                let mut v = 0.0;
                for (yy, fy) in ys.iter().zip(wy) {
                    for (xx, fx) in xs.iter().zip(wx) {
                        v += fy * fx * f64::from(plane[yy * b + xx]);
                    }
                }
                let tile = window[(ch * ws + y % ws) * ws + x % ws];
                out[(y * side + x) * c + ch] = v as f32 + tile;
            }
        }
    }
    out
}

fn encoded(gauss: &[f32], x: f64, y: f64) -> Vec<f32> {
    let half = (DIM / 2) as usize;
    let (cx, cy) = (2.0 * x - 1.0, 2.0 * y - 1.0);
    let turns: Vec<f64> = (0..half)
        .map(|j| {
            std::f64::consts::TAU * (cx * f64::from(gauss[j]) + cy * f64::from(gauss[half + j]))
        })
        .collect();
    let sin = turns.iter().map(|t| t.sin() as f32);
    let cos = turns.iter().map(|t| t.cos() as f32);
    sin.chain(cos).collect()
}

fn dense(gauss: &[f32]) -> Vec<f32> {
    let side = f64::from(EMBED);
    (0..EMBED)
        .flat_map(|y| {
            (0..EMBED).flat_map(move |x| {
                encoded(
                    gauss,
                    (f64::from(x) + 0.5) / side,
                    (f64::from(y) + 0.5) / side,
                )
            })
        })
        .collect()
}

fn tokens(p: &Prompts, clicks: &[Click]) -> Vec<f32> {
    let mut out = p.outputs.clone();
    let side = f64::from(SIDE);
    for c in clicks {
        let at = |v: f64| (v * side + 0.5) / side;
        let mut e = encoded(&p.gauss, at(c.at[0]), at(c.at[1]));
        let add = &p.points[usize::from(c.positive)];
        for (v, a) in e.iter_mut().zip(add) {
            *v += a;
        }
        out.extend(e);
    }
    out.extend(&p.none);
    out
}

fn mask(masks: &[f32], i: usize) -> impl Iterator<Item = f32> + '_ {
    (0..(MAP * MAP) as usize).map(move |px| masks[px * MASKS as usize + i])
}

fn pick(masks: &[f32], iou: &[f32], clicks: usize) -> usize {
    let best = (1..MASKS as usize).fold(1, |b, i| if iou[i] > iou[b] { i } else { b });
    match clicks {
        0 | 1 => best,
        _ => {
            let inner = mask(masks, 0).filter(|v| *v > STABLE_DELTA).count();
            let outer = mask(masks, 0).filter(|v| *v > -STABLE_DELTA).count();
            let score = match outer {
                0 => 1.0,
                n => inner as f32 / n as f32,
            };
            if score >= STABLE { 0 } else { best }
        }
    }
}

fn plane(masks: &[f32], pick: usize, object: f32) -> Vec<f32> {
    match object > 0.0 {
        true => mask(masks, pick).collect(),
        false => vec![NO_OBJECT; (MAP * MAP) as usize],
    }
}

fn pointer_token(pick: usize, clicks: usize) -> usize {
    match clicks {
        0 | 1 => pick,
        _ => 0,
    }
}

struct Decoded {
    found: Found,
    pick: usize,
    pointer: Vec<f32>,
}

fn need(plans: &[Plan]) -> [u32; 5] {
    let mut most = [0u32; 5];
    let mut grow = |i: usize, v: u32| most[i] = most[i].max(v);
    for p in plans {
        let (s, h) = (p.side, p.half());
        let g = p.grid();
        let tokens = g.count() * g.ws * g.ws;
        let queries = g.count() * p.wq() * p.wq();
        grow(0, s * s * p.dim);
        grow(0, h * h * p.dout);
        if p.window > 0 {
            grow(1, tokens * p.dim);
        }
        grow(2, tokens * 3 * p.dout);
        grow(3, queries * p.dout);
        grow(4, h * h * 4 * p.dout);
        grow(4, s * s * p.dout);
        grow(4, s * s * DIM);
    }
    most
}

fn flat(gpu: &Gpu, floats: u32) -> View {
    tensor(gpu, 1, 1, floats.max(1), 1)
}

fn space(gpu: &Gpu, plans: &[Plan]) -> Space {
    let [x, win, qkv, att, hid] = need(plans);
    let e = EMBED * EMBED;
    let hid = hid.max(e * DIM);
    let rows = |n: u32, c: u32| tensor(gpu, 1, 1, n, c);
    Space {
        x: [flat(gpu, x), flat(gpu, x)],
        xn: flat(gpu, x),
        win: flat(gpu, win),
        qkv: flat(gpu, qkv),
        qp: flat(gpu, att),
        att: flat(gpu, att),
        aw: flat(gpu, att),
        hid: flat(gpu, hid),
        image: tensor(gpu, 1, SIDE, SIDE, 3),
        fs0: tensor(gpu, 1, FEAT, FEAT, HYPER),
        fs1: tensor(gpu, 1, FEAT / 2, FEAT / 2, DIM / 4),
        l2: tensor(gpu, 1, EMBED, EMBED, DIM),
        l3: tensor(gpu, 1, EMBED / 2, EMBED / 2, DIM),
        src: tensor(gpu, 1, EMBED, EMBED, DIM),
        dense: rows(e, DIM),
        gemm: global_gemm(gpu, plans),
        q: rows(MOST_TOKENS, DIM),
        q2: rows(MOST_TOKENS, DIM),
        qpe: rows(MOST_TOKENS, DIM),
        tq: rows(MOST_TOKENS, DIM),
        k: rows(e, DIM),
        k2: rows(e, DIM),
        tk: rows(e, DIM),
        pq: rows(e, DIM),
        pk: rows(e, DIM),
        pv: rows(e, DIM),
        po: rows(e, DIM),
        mlp: rows(MOST_TOKENS, MLP),
        u1: tensor(gpu, 1, MAP / 2, MAP / 2, DIM / 4),
        u1n: tensor(gpu, 1, MAP / 2, MAP / 2, DIM / 4),
        up: tensor(gpu, 1, MAP, MAP, HYPER),
        ha: rows(1, DIM),
        hb: rows(1, DIM),
        hy: rows(MASKS, HYPER),
        masks: tensor(gpu, 1, MAP, MAP, MASKS),
        iou: rows(1, MASKS),
        object: rows(1, 1),
        dynamic: Conv {
            cin: HYPER,
            cout: MASKS,
            kh: 1,
            kw: 1,
            stride: (1, 1),
            pad: (0, 0),
            dil: (1, 1),
            cols: 64,
            weights: gpu.buffer((HYPER * 64 * 4) as usize),
            bias: gpu.upload(&bytes(&[0.0; 64])),
            alpha: gpu.upload(&bytes(&[0.0; 64])),
            act: Act::None,
        },
    }
}

fn global_gemm(gpu: &Gpu, plans: &[Plan]) -> kernels::Gemm {
    let global = plans.iter().filter(|p| p.window == 0);
    let dim = global.clone().map(|p| p.dout / p.heads).max().unwrap_or(16);
    let keys = global.map(|p| p.side * p.side).max().unwrap_or(64);
    kernels::Gemm::new(gpu, dim, keys, keys.min(GLOBAL_QUERIES))
}

const GLOBAL_QUERIES: u32 = 4096;

fn row(v: &View, i: u32, c: u32) -> View {
    View {
        base: v.base + i * v.stride,
        n: 1,
        h: 1,
        w: 1,
        c,
        ..v.clone()
    }
}

fn read_view(gpu: &Gpu, v: &View) -> Result<Vec<f32>> {
    let size = u64::from(v.n * v.h * v.w * v.c) * 4;
    let raw = gpu.read(&v.buf, u64::from(v.base) * 4, size)?;
    Ok(compute::floats(&raw))
}

impl Net {
    pub(super) fn load(path: &Path, title: &'static str, c: &Config, on_gpu: bool) -> Result<Self> {
        let gpu = match on_gpu {
            true => Gpu::start(title, ops::SHARED),
            false => Gpu::start_soft(title, ops::SHARED),
        };
        let gpu = gpu.map_err(Error::Invalid)?;
        let t = Torch::open(path, title)?;
        let device = gpu.device.clone();
        let m = compute::checked(&device, title, || {
            let load = Load { t: &t, gpu: &gpu };
            let plans = plans(c);
            let limits = gpu.device.limits();
            let cap = limits
                .max_storage_buffer_binding_size
                .min(limits.max_buffer_size)
                / 4;
            if need(&plans).iter().any(|&n| u64::from(n) > cap) {
                return Err(Error::Invalid(format!(
                    "{title}: the GPU's buffers are too small"
                )));
            }
            if let Some(p) = plans
                .iter()
                .find(|p| !kernels::attend_fits(p.dout / p.heads))
            {
                return Err(Error::Invalid(format!(
                    "{title}: heads of {} channels are not supported",
                    p.dout / p.heads
                )));
            }
            let ends: Vec<u32> = plans
                .iter()
                .filter(|p| p.end.is_some())
                .map(|p| p.dout)
                .collect();
            let bkg = load.floats("image_encoder.trunk.pos_embed", &[1, c.embed, c.bkg, c.bkg])?;
            let ws = c.windows[0];
            let window = load.floats(
                "image_encoder.trunk.pos_embed_window",
                &[1, c.embed, ws, ws],
            )?;
            let pos = tensor(&gpu, 1, FEAT, FEAT, c.embed);
            gpu.queue.write_buffer(
                &pos.buf,
                0,
                &bytes(&position(&bkg, c.bkg, &window, ws, c.embed)),
            );
            let patch_w = load.floats(
                "image_encoder.trunk.patch_embed.proj.weight",
                &[c.embed, 3, 7, 7],
            )?;
            let patch_b = load.floats("image_encoder.trunk.patch_embed.proj.bias", &[c.embed])?;
            let patch = ops::conv(
                &gpu,
                Shape::square(3, c.embed, 7, 4, 3),
                &patch_w,
                Some(&patch_b),
                Act::None,
                None,
            );
            let lateral = (0..4)
                .map(|level| {
                    let name = format!("image_encoder.neck.convs.{}.conv", 3 - level);
                    load.pointwise(&name, (ends[level], DIM), &[])
                })
                .collect::<Result<Vec<_>>>()?;
            let s0 = load.pointwise("sam_mask_decoder.conv_s0", (DIM, HYPER), &[])?;
            let s1 = load.pointwise("sam_mask_decoder.conv_s1", (DIM, DIM / 4), &[])?;
            let decoder = load.decoder()?;
            let memory = load.memory()?;
            let space = space(&gpu, &plans);
            gpu.queue
                .write_buffer(&space.dense.buf, 0, &bytes(&dense(&decoder.prompts.gauss)));
            let blocks = plans
                .into_iter()
                .enumerate()
                .map(|(i, p)| load.block(i, p))
                .collect::<Result<Vec<_>>>()?;
            let staging = gpu.staging(u64::from(MAP * MAP * MASKS + MASKS + 1 + MASKS * DIM) * 4);
            Ok(Model {
                basic: Basic::new(&gpu),
                own: Kernels::new(&gpu),
                patch,
                pos,
                blocks,
                lateral,
                s0,
                s1,
                decoder,
                memory,
                space,
                staging,
            })
        })?;
        Ok(Self { gpu, m })
    }

    fn checked<T>(&self, run: impl FnOnce() -> Result<T>) -> Result<T> {
        compute::checked(&self.gpu.device, self.gpu.what, run)
    }

    pub(super) fn encode(&self, (w, h): (u32, u32), pixels: &[[f32; 3]]) -> Result<()> {
        let gpu = &self.gpu;
        self.checked(|| {
            let flat: Vec<f32> = pixels.iter().flatten().copied().collect();
            let picture = gpu.upload(&bytes(&flat));
            let axis = |input: u32| {
                let t = crate::bicubic::taps(input, SIDE);
                let taps: Vec<f32> = t.weights.iter().map(|&v| v as f32).collect();
                Axis {
                    bounds: gpu.upload(&words(
                        &t.bounds.iter().map(|&b| b as u32).collect::<Vec<_>>(),
                    )),
                    taps: gpu.upload(&bytes(&taps)),
                    size: t.size as u32,
                }
            };
            let (across, down) = (axis(w), axis(h));
            let middle = gpu.buffer((h * SIDE * 3 * 4) as usize);
            let s = &self.m.space;
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                let mut ops = Ops {
                    gpu,
                    k: &self.m.basic,
                    pass: &mut pass,
                };
                kernels::scale_across(
                    &mut ops,
                    &self.m.own,
                    &across,
                    (w, h, SIDE),
                    &picture,
                    &middle,
                );
                kernels::scale_down(
                    &mut ops,
                    &self.m.own,
                    &down,
                    (SIDE, SIDE),
                    (MEAN, STD),
                    &middle,
                    &s.image.buf,
                );
                let embed = self.m.blocks[0].plan.dim;
                let x0 = s.x[0].shaped(1, FEAT, FEAT, embed);
                ops.conv(&self.m.patch, &s.image, &x0, Res::None);
                ops.add(&self.m.pos, &x0);
                let mut cur = 0;
                for b in &self.m.blocks {
                    cur = self.block(&mut ops, b, cur);
                    if let Some(level) = b.plan.end {
                        let side = b.plan.half();
                        let feat = s.x[cur].shaped(1, side, side, b.plan.dout);
                        self.level(&mut ops, level, &feat);
                    }
                }
            }
            gpu.flush();
            gpu.queue.submit([enc.finish()]);
            Ok(())
        })
    }

    fn block(&self, ops: &mut Ops, b: &Block, cur: usize) -> usize {
        let s = &self.m.space;
        let p = &b.plan;
        let (side, half) = (p.side, p.half());
        let x = s.x[cur].shaped(1, side, side, p.dim);
        let xn = s.xn.shaped(1, side, side, p.dim);
        ops.norm(&b.norm1.gamma, &b.norm1.beta, b.norm1.eps, &x, &xn);
        let target_at = if b.short.is_some() { 1 - cur } else { cur };
        let target = s.x[target_at].shaped(1, half, half, p.dout);
        if let Some(short) = &b.short {
            let full = s.hid.shaped(1, side, side, p.dout);
            ops.conv(short, &xn, &full, Res::None);
            match p.pool {
                true => ops.pool(&full, &target, 2, 2, 0),
                false => ops.copy(&full, &target, 1.0, 0.0, Act::None),
            }
        }
        let grid = p.grid();
        let (ws, wq, count) = (grid.ws, p.wq(), grid.count());
        let tokens = count * ws * ws;
        let rows = match p.window > 0 {
            true => {
                let win = s.win.shaped(1, 1, tokens, p.dim);
                kernels::partition(ops, &self.m.own, &grid, &xn, &win);
                win
            }
            false => xn.shaped(1, 1, tokens, p.dim),
        };
        let qkv = s.qkv.shaped(1, 1, tokens, 3 * p.dout);
        ops.conv(&b.qkv, &rows, &qkv, Res::None);
        let lq = wq * wq;
        let each = ws * ws * 3 * p.dout;
        let (q, q_each) = match p.pool {
            true => {
                let from = View {
                    n: count,
                    h: ws,
                    w: ws,
                    c: p.dout,
                    ..qkv.clone()
                };
                let pooled = s.qp.shaped(count, wq, wq, p.dout);
                ops.pool(&from, &pooled, 2, 2, 0);
                (pooled, lq * p.dout)
            }
            false => (qkv.part(0, p.dout), each),
        };
        let att = s.att.shaped(1, 1, count * lq, p.dout);
        let heads = Heads {
            batches: count,
            lq,
            lk: ws * ws,
            heads: p.heads,
            dim: p.dout / p.heads,
        };
        let (k, v) = (qkv.part(p.dout, p.dout), qkv.part(2 * p.dout, p.dout));
        if count == 1 && !p.pool {
            let d = p.dout / p.heads;
            for h in 0..p.heads {
                let head = |v: &View, at: u32| View {
                    n: 1,
                    h: 1,
                    w: lq,
                    c: d,
                    at: v.at + at,
                    ..v.clone()
                };
                let views = (&head(&q, h * d), &head(&k, h * d), &head(&v, h * d));
                let out = head(&att.shaped(1, 1, lq, p.dout), h * d);
                kernels::attend_gemm(ops, &self.m.own, &s.gemm, (views.0, views.1, views.2, &out));
            }
        } else {
            kernels::attend(
                ops,
                &heads,
                Rows {
                    view: &q,
                    each: q_each,
                },
                Rows { view: &k, each },
                Rows { view: &v, each },
                Rows {
                    view: &att,
                    each: lq * p.dout,
                },
            );
        }
        let aw = s.aw.shaped(1, 1, count * lq, p.dout);
        ops.conv(&b.proj, &att, &aw, Res::None);
        match p.window > 0 {
            true => {
                let back = Grid {
                    h: half,
                    w: half,
                    ws: wq,
                };
                kernels::unpartition_add(ops, &self.m.own, &back, &aw, &target);
            }
            false => ops.add(&aw.shaped(1, half, half, p.dout), &target),
        }
        let xn2 = s.xn.shaped(1, half, half, p.dout);
        ops.norm(&b.norm2.gamma, &b.norm2.beta, b.norm2.eps, &target, &xn2);
        let hid = s.hid.shaped(1, half, half, 4 * p.dout);
        ops.conv(&b.up, &xn2, &hid, Res::None);
        let out_at = 1 - target_at;
        let out = s.x[out_at].shaped(1, half, half, p.dout);
        ops.conv(&b.down, &hid, &out, Res::After(&target));
        out_at
    }

    fn level(&self, ops: &mut Ops, level: usize, feat: &View) {
        let s = &self.m.space;
        let (side, lateral) = (feat.h, &self.m.lateral[level]);
        match level {
            0 | 1 => {
                let t = s.hid.shaped(1, side, side, DIM);
                ops.conv(lateral, feat, &t, Res::None);
                let (conv, out) = if level == 0 {
                    (&self.m.s0, &s.fs0)
                } else {
                    (&self.m.s1, &s.fs1)
                };
                ops.conv(conv, &t, out, Res::None);
            }
            2 => ops.conv(lateral, feat, &s.l2, Res::None),
            _ => {
                ops.conv(lateral, feat, &s.l3, Res::None);
                let up = s.hid.shaped(1, EMBED, EMBED, DIM);
                ops.nearest(&s.l3, &up);
                ops.combine(&s.l2, &up, &s.src, 1.0, 1.0);
            }
        }
    }

    fn attention(&self, ops: &mut Ops, a: &Attn, (q, k, v): (&View, &View, &View)) -> View {
        let s = &self.m.space;
        let (lq, lk, inner) = (q.w, k.w, a.inner);
        let pq = s.pq.shaped(1, 1, lq, inner);
        let pk = s.pk.shaped(1, 1, lk, inner);
        let pv = s.pv.shaped(1, 1, lk, inner);
        let po = s.po.shaped(1, 1, lq, inner);
        ops.conv(&a.q, q, &pq, Res::None);
        ops.conv(&a.k, k, &pk, Res::None);
        ops.conv(&a.v, v, &pv, Res::None);
        let heads = Heads {
            batches: 1,
            lq,
            lk,
            heads: HEADS,
            dim: inner / HEADS,
        };
        let rows = |view, n: u32| Rows {
            view,
            each: n * inner,
        };
        kernels::attend(
            ops,
            &heads,
            rows(&pq, lq),
            rows(&pk, lk),
            rows(&pv, lk),
            rows(&po, lq),
        );
        po
    }

    fn norm(ops: &mut Ops, n: &Norm, src: &View, dst: &View) {
        ops.norm(&n.gamma, &n.beta, n.eps, src, dst);
    }

    fn mlp(ops: &mut Ops, layers: &[Conv; 3], input: &View, (ha, hb, out): (&View, &View, &View)) {
        ops.conv(&layers[0], input, ha, Res::None);
        ops.conv(&layers[1], ha, hb, Res::None);
        ops.conv(&layers[2], hb, out, Res::None);
    }

    fn transformer(&self, ops: &mut Ops, t: u32, (image, add): (&View, &View)) {
        let s = &self.m.space;
        let d = &self.m.decoder;
        let e = EMBED * EMBED;
        let tokens = |v: &View| v.shaped(1, 1, t, DIM);
        let (q, q2, qpe, tq) = (tokens(&s.q), tokens(&s.q2), tokens(&s.qpe), tokens(&s.tq));
        let (k, k2, tk) = (
            s.k.shaped(1, 1, e, DIM),
            s.k2.shaped(1, 1, e, DIM),
            s.tk.shaped(1, 1, e, DIM),
        );
        let kpe = s.dense.shaped(1, 1, e, DIM);
        let hid = s.mlp.shaped(1, 1, t, MLP);
        let flat = |v: &View| v.shaped(1, 1, e, DIM);
        ops.combine(&flat(image), &flat(add), &k, 1.0, 1.0);
        for (i, l) in d.layers.iter().enumerate() {
            if i == 0 {
                let po = self.attention(ops, &l.own, (&q, &q, &q));
                ops.conv(&l.own.out, &po, &q2, Res::None);
            } else {
                ops.combine(&q, &qpe, &tq, 1.0, 1.0);
                let po = self.attention(ops, &l.own, (&tq, &tq, &q));
                ops.conv(&l.own.out, &po, &q2, Res::After(&q));
            }
            Self::norm(ops, &l.norm1, &q2, &q);
            ops.combine(&q, &qpe, &tq, 1.0, 1.0);
            ops.combine(&k, &kpe, &tk, 1.0, 1.0);
            let po = self.attention(ops, &l.to_image, (&tq, &tk, &k));
            ops.conv(&l.to_image.out, &po, &q2, Res::After(&q));
            Self::norm(ops, &l.norm2, &q2, &q);
            ops.conv(&l.mlp[0], &q, &hid, Res::None);
            ops.conv(&l.mlp[1], &hid, &q2, Res::After(&q));
            Self::norm(ops, &l.norm3, &q2, &q);
            ops.combine(&q, &qpe, &tq, 1.0, 1.0);
            let po = self.attention(ops, &l.to_tokens, (&tk, &tq, &q));
            ops.conv(&l.to_tokens.out, &po, &k2, Res::After(&k));
            Self::norm(ops, &l.norm4, &k2, &k);
        }
        ops.combine(&q, &qpe, &tq, 1.0, 1.0);
        ops.combine(&k, &kpe, &tk, 1.0, 1.0);
        let po = self.attention(ops, &d.last, (&tq, &tk, &k));
        ops.conv(&d.last.out, &po, &q2, Res::After(&q));
        Self::norm(ops, &d.norm, &q2, &q);
    }

    fn heads(&self, ops: &mut Ops) {
        let s = &self.m.space;
        let d = &self.m.decoder;
        let keys = s.k.shaped(1, EMBED, EMBED, DIM);
        ops.conv_shuffled(&d.up1, &keys, &s.u1, Res::After(&s.fs1), 2);
        Self::norm(ops, &d.norm_up, &s.u1, &s.u1n);
        ops.copy(&s.u1n, &s.u1, 1.0, 0.0, Act::Gelu);
        ops.conv_shuffled(&d.up2, &s.u1, &s.up, Res::Before(&s.fs0), 2);
        for (i, layers) in d.hyper.iter().enumerate() {
            let i = i as u32;
            let token = row(&s.q, 2 + i, DIM);
            let out = row(&s.hy, i, HYPER);
            Self::mlp(ops, layers, &token, (&s.ha, &s.hb, &out));
            let from = View {
                base: s.hy.base + i * HYPER,
                n: 1,
                h: 1,
                w: HYPER,
                c: 1,
                stride: 1,
                at: 0,
                buf: s.hy.buf.clone(),
            };
            let to = View {
                buf: s.dynamic.weights.clone(),
                base: 0,
                n: 1,
                h: 1,
                w: HYPER,
                c: 1,
                stride: 64,
                at: i,
            };
            ops.copy(&from, &to, 1.0, 0.0, Act::None);
        }
        ops.conv(&s.dynamic, &s.up, &s.masks, Res::None);
        Self::mlp(ops, &d.iou, &row(&s.q, 1, DIM), (&s.ha, &s.hb, &s.iou));
        Self::mlp(
            ops,
            &d.object,
            &row(&s.q, 0, DIM),
            (&s.ha, &s.hb, &s.object),
        );
    }

    pub(super) fn decode(&self, clicks: &[Click]) -> Result<Found> {
        let s = &self.m.space;
        let image = (s.src.clone(), self.m.memory.shift.clone());
        let d = self.decoded(clicks, None, |_| image)?;
        Ok(d.found)
    }

    pub(super) fn work(&self) -> Work {
        memory::work(&self.gpu)
    }

    pub(super) fn bank(&self) -> Bank {
        Bank::new(&self.gpu)
    }

    pub(super) fn first(&self, w: &Work, bank: &mut Bank, clicks: &[Click]) -> Result<Found> {
        let s = &self.m.space;
        let image = (s.src.clone(), self.m.memory.shift.clone());
        let d = self.decoded(clicks, Some(w), |_| image)?;
        let way = match d.found.object > 0.0 {
            true => kernels::Remembered::Cut,
            false => kernels::Remembered::Empty,
        };
        self.remember(w, bank, (d.pick, way), d.pointer)?;
        Ok(d.found)
    }

    pub(super) fn next(&self, w: &Work, bank: &mut Bank, frames: usize) -> Result<Found> {
        let r = memory::Recall::of(bank, frames);
        self.recall_upload(w, &r);
        let none = self.m.memory.none.clone();
        let d = self.decoded(&[], Some(w), |ops| (self.recall(ops, w, bank, &r), none))?;
        let way = match d.found.object > 0.0 {
            true => kernels::Remembered::Soft,
            false => kernels::Remembered::Empty,
        };
        self.remember(w, bank, (d.pick, way), d.pointer)?;
        Ok(d.found)
    }

    fn decoded(
        &self,
        clicks: &[Click],
        work: Option<&Work>,
        image: impl FnOnce(&mut Ops) -> (View, View),
    ) -> Result<Decoded> {
        if clicks.len() > MOST_CLICKS {
            return Err(Error::Invalid(format!(
                "a subject takes {MOST_CLICKS} clicks at most"
            )));
        }
        let gpu = &self.gpu;
        let s = &self.m.space;
        let mut made = tokens(&self.m.decoder.prompts, clicks);
        if clicks.is_empty() {
            made.extend(&self.m.decoder.prompts.none);
        }
        let t = (made.len() / DIM as usize) as u32;
        let plane_bytes = u64::from(MAP * MAP * MASKS) * 4;
        let iou_bytes = u64::from(MASKS) * 4;
        let scores = iou_bytes + 4;
        let pointers = u64::from(MASKS * DIM) * 4;
        self.checked(|| {
            gpu.queue
                .write_buffer(&s.q.buf, u64::from(s.q.base) * 4, &bytes(&made));
            gpu.queue
                .write_buffer(&s.qpe.buf, u64::from(s.qpe.base) * 4, &bytes(&made));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                let mut ops = Ops {
                    gpu,
                    k: &self.m.basic,
                    pass: &mut pass,
                };
                let (image, add) = image(&mut ops);
                self.transformer(&mut ops, t, (&image, &add));
                self.heads(&mut ops);
                if let Some(w) = work {
                    let tokens = View {
                        base: s.q.base + 2 * DIM,
                        ..s.q.shaped(1, 1, MASKS, DIM)
                    };
                    let into = (&w.pa, &w.pb, &w.ptr);
                    Self::mlp(&mut ops, &self.m.memory.pointer, &tokens, into);
                }
            }
            let at = |v: &View| u64::from(v.base) * 4;
            let st = &self.m.staging;
            enc.copy_buffer_to_buffer(&s.masks.buf, at(&s.masks), st, 0, plane_bytes);
            enc.copy_buffer_to_buffer(&s.iou.buf, at(&s.iou), st, plane_bytes, iou_bytes);
            let after = plane_bytes + iou_bytes;
            enc.copy_buffer_to_buffer(&s.object.buf, at(&s.object), st, after, 4);
            if let Some(w) = work {
                let to = plane_bytes + scores;
                enc.copy_buffer_to_buffer(&w.ptr.buf, at(&w.ptr), st, to, pointers);
            }
            gpu.flush();
            gpu.queue.submit([enc.finish()]);
            Ok(())
        })?;
        let size = plane_bytes + scores + work.map_or(0, |_| pointers);
        let raw = gpu.fetch(&self.m.staging, size, None)?;
        let all = compute::floats(&raw);
        let n = (MAP * MAP * MASKS) as usize;
        let (masks, rest) = all.split_at(n);
        let (iou, object) = (&rest[..MASKS as usize], rest[MASKS as usize]);
        if let Some(dir) = super::dumped()? {
            let at = super::dump_index();
            crate::npy::write(
                &dir.join(format!("masks-{at}.npy")),
                &[MAP as usize, MAP as usize, MASKS as usize],
                masks,
            )?;
            crate::npy::write(
                &dir.join(format!("scores-{at}.npy")),
                &[MASKS as usize + 1],
                &rest[..=MASKS as usize],
            )?;
        }
        let pick = pick(masks, iou, clicks.len());
        let pointer = match (work, object > 0.0) {
            (None, _) => Vec::new(),
            (Some(_), false) => self.no_pointer(),
            (Some(_), true) => {
                let token = pointer_token(pick, clicks.len());
                let from = MASKS as usize + 1 + token * DIM as usize;
                rest[from..from + DIM as usize].to_vec()
            }
        };
        Ok(Decoded {
            found: Found {
                logits: plane(masks, pick, object),
                object,
            },
            pick,
            pointer,
        })
    }

    pub(super) fn dump(&self, dir: &Path, frame: Option<usize>) -> Result<()> {
        let s = &self.m.space;
        let named = [
            ("image", &s.image),
            ("feat-s0", &s.fs0),
            ("feat-s1", &s.fs1),
            ("vision", &s.src),
        ];
        for (name, v) in named {
            let data = read_view(&self.gpu, v)?;
            let shape = sizes(&[v.h, v.w, v.c]);
            let file = match frame {
                Some(n) => format!("{name}-{n}.npy"),
                None => format!("{name}.npy"),
            };
            crate::npy::write(&dir.join(file), &shape, &data)?;
        }
        Ok(())
    }
}
