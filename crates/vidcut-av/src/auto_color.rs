use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::Deserialize;
use vidcut_core::auto_color::Neural;

use crate::compute::ops::{self, Act, Conv, Kernels, Ops, Res, Shape, View};
use crate::compute::{self, Gpu};

const SIZE: usize = 128;
const OUT_CH: usize = 160;
const OUT_HW: usize = 4;
const OUT_LEN: usize = OUT_HW * OUT_HW * OUT_CH;
const STATS_LEN: usize = 42;
const FEAT_LEN: usize = OUT_CH * 3 + STATS_LEN;
const WEIGHTS: &[u8] = include_bytes!("auto_color.safetensors");
const QUANTILES: [f32; 7] = [0.01, 0.05, 0.25, 0.50, 0.75, 0.95, 0.99];
const LUMA: [f32; 3] = [0.299, 0.587, 0.114];
const EPS: f32 = 1e-3;

#[derive(Deserialize)]
struct Entry {
    dtype: String,
    shape: Vec<usize>,
    data_offsets: [usize; 2],
}

struct ConvWeights {
    cin: usize,
    cout: usize,
    stride: usize,
    weights: Vec<f32>,
    bias: Vec<f32>,
}

struct LinearWeights {
    cin: usize,
    cout: usize,
    weights: Vec<f32>,
    bias: Vec<f32>,
}

struct StageWeights {
    down: ConvWeights,
    res: ConvWeights,
}

struct Model {
    stem: ConvWeights,
    stages: [StageWeights; 4],
    head: [LinearWeights; 3],
}

struct Views {
    input: View,
    h0: View,
    h1: View,
    r1: View,
    h2: View,
    r2: View,
    h3: View,
    r3: View,
    h4: View,
    r4: View,
}

struct GpuNet {
    gpu: Gpu,
    kernels: Kernels,
    _bufs: Vec<wgpu::Buffer>,
    views: Views,
    staging: wgpu::Buffer,
    stem: Conv,
    stages: [(Conv, Conv); 4],
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exp = (bits >> 10) & 0x1f;
    let frac = u32::from(bits & 0x03ff);
    let out = if exp == 0 {
        if frac == 0 {
            sign
        } else {
            let mut m = frac;
            let mut e = 127i32 - 14;
            while m & 0x0400 == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x03ff;
            sign | ((e as u32) << 23) | (m << 13)
        }
    } else if exp == 0x1f {
        sign | 0x7f80_0000 | (frac << 13)
    } else {
        sign | ((u32::from(exp) + 112) << 23) | (frac << 13)
    };
    f32::from_bits(out)
}

fn read_tensor(
    map: &HashMap<String, Entry>,
    blob: &[u8],
    name: &str,
    expected: &[usize],
) -> Vec<f32> {
    let e = &map[name];
    assert_eq!(e.shape, expected, "unexpected shape for {name}");
    let raw = &blob[e.data_offsets[0]..e.data_offsets[1]];
    match e.dtype.as_str() {
        "F16" => raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| f16_to_f32(u16::from_le_bytes(c)))
            .collect(),
        "F32" => raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&c| f32::from_le_bytes(c))
            .collect(),
        other => panic!("unsupported dtype {other} for {name}"),
    }
}

fn conv_weights(
    map: &HashMap<String, Entry>,
    blob: &[u8],
    prefix: &str,
    cin: usize,
    cout: usize,
    stride: usize,
) -> ConvWeights {
    ConvWeights {
        cin,
        cout,
        stride,
        weights: read_tensor(map, blob, &format!("{prefix}.weight"), &[cout, cin, 3, 3]),
        bias: read_tensor(map, blob, &format!("{prefix}.bias"), &[cout]),
    }
}

fn linear_weights(
    map: &HashMap<String, Entry>,
    blob: &[u8],
    prefix: &str,
    cin: usize,
    cout: usize,
) -> LinearWeights {
    LinearWeights {
        cin,
        cout,
        weights: read_tensor(map, blob, &format!("{prefix}.weight"), &[cout, cin]),
        bias: read_tensor(map, blob, &format!("{prefix}.bias"), &[cout]),
    }
}

fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| {
        let len = u64::from_le_bytes(WEIGHTS[..8].try_into().expect("header")) as usize;
        let map: HashMap<String, Entry> =
            serde_json::from_slice(&WEIGHTS[8..8 + len]).expect("safetensors header");
        let blob = &WEIGHTS[8 + len..];
        Model {
            stem: conv_weights(&map, blob, "stem", 3, 24, 2),
            stages: [
                StageWeights {
                    down: conv_weights(&map, blob, "s1_down", 24, 48, 2),
                    res: conv_weights(&map, blob, "s1_res", 48, 48, 1),
                },
                StageWeights {
                    down: conv_weights(&map, blob, "s2_down", 48, 80, 2),
                    res: conv_weights(&map, blob, "s2_res", 80, 80, 1),
                },
                StageWeights {
                    down: conv_weights(&map, blob, "s3_down", 80, 128, 2),
                    res: conv_weights(&map, blob, "s3_res", 128, 128, 1),
                },
                StageWeights {
                    down: conv_weights(&map, blob, "s4_down", 128, OUT_CH, 2),
                    res: conv_weights(&map, blob, "s4_res", OUT_CH, OUT_CH, 1),
                },
            ],
            head: [
                linear_weights(&map, blob, "head_0", FEAT_LEN, 192),
                linear_weights(&map, blob, "head_1", 192, 64),
                linear_weights(&map, blob, "head_2", 64, 8),
            ],
        }
    })
}

fn gpu_conv(gpu: &Gpu, c: &ConvWeights) -> Conv {
    let shape = Shape::square(c.cin as u32, c.cout as u32, 3, c.stride as u32, 1);
    ops::conv(gpu, shape, &c.weights, Some(&c.bias), Act::Silu, None)
}

impl GpuNet {
    fn start(m: &Model) -> Option<Self> {
        let gpu = Gpu::start("Auto Color", ops::SHARED).ok()?;
        let (kernels, (_bufs, views), stem, stages) =
            compute::checked(&gpu.device, gpu.what, || {
                let kernels = Kernels::new(&gpu);
                let sz = SIZE as u32;
                let (bufs, views) = ops::arena(&gpu, |c| Views {
                    input: c.view(1, sz, sz, 3),
                    h0: c.view(1, 64, 64, 24),
                    h1: c.view(1, 32, 32, 48),
                    r1: c.view(1, 32, 32, 48),
                    h2: c.view(1, 16, 16, 80),
                    r2: c.view(1, 16, 16, 80),
                    h3: c.view(1, 8, 8, 128),
                    r3: c.view(1, 8, 8, 128),
                    h4: c.view(1, 4, 4, OUT_CH as u32),
                    r4: c.view(1, 4, 4, OUT_CH as u32),
                })
                .map_err(|e| crate::error::Error::Invalid(e.to_string()))?;
                let stem = gpu_conv(&gpu, &m.stem);
                let stages = std::array::from_fn(|i| {
                    (
                        gpu_conv(&gpu, &m.stages[i].down),
                        gpu_conv(&gpu, &m.stages[i].res),
                    )
                });
                Ok((kernels, (bufs, views), stem, stages))
            })
            .ok()?;
        let staging = gpu.staging((OUT_LEN * 4) as u64);
        Some(Self {
            gpu,
            kernels,
            _bufs,
            views,
            staging,
            stem,
            stages,
        })
    }

    fn forward(&self, nhwc: &[f32]) -> Option<Vec<f32>> {
        let gpu = &self.gpu;
        let out = compute::checked(&gpu.device, gpu.what, || {
            gpu.queue
                .write_buffer(&self.views.input.buf, 0, &compute::bytes(nhwc));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                let mut ops = Ops {
                    gpu,
                    k: &self.kernels,
                    pass: &mut pass,
                };
                ops.conv(&self.stem, &self.views.input, &self.views.h0, Res::None);
                let pairs = [
                    (&self.views.h0, &self.views.h1, &self.views.r1),
                    (&self.views.r1, &self.views.h2, &self.views.r2),
                    (&self.views.r2, &self.views.h3, &self.views.r3),
                    (&self.views.r3, &self.views.h4, &self.views.r4),
                ];
                for ((down, res), (inp, hid, out)) in self.stages.iter().zip(pairs) {
                    ops.conv(down, inp, hid, Res::None);
                    ops.conv(res, hid, out, Res::After(hid));
                }
            }
            enc.copy_buffer_to_buffer(
                &self.views.r4.buf,
                u64::from(self.views.r4.base) * 4,
                &self.staging,
                0,
                (OUT_LEN * 4) as u64,
            );
            gpu.flush();
            gpu.queue.submit([enc.finish()]);
            Ok(())
        });
        out.ok()?;
        let raw = gpu.fetch(&self.staging, (OUT_LEN * 4) as u64, None).ok()?;
        Some(compute::floats(&raw))
    }
}

fn gpu_forward(m: &Model, nhwc: &[f32]) -> Option<Vec<f32>> {
    static RUNNER: OnceLock<Option<Mutex<GpuNet>>> = OnceLock::new();
    let slot = RUNNER.get_or_init(|| GpuNet::start(m).map(Mutex::new));
    slot.as_ref()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .forward(nhwc)
}

fn clamp01(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

fn resize_nhwc_128(width: u32, height: u32, pixels: &[[f32; 3]]) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0.0f32; SIZE * SIZE * 3];
    for y in 0..SIZE {
        let fy = (((y as f32 + 0.5) * h as f32 / SIZE as f32) - 0.5).clamp(0.0, (h - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(h - 1);
        let wy = fy - y0 as f32;
        for x in 0..SIZE {
            let fx = (((x as f32 + 0.5) * w as f32 / SIZE as f32) - 0.5).clamp(0.0, (w - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(w - 1);
            let wx = fx - x0 as f32;
            let p00 = pixels[y0 * w + x0];
            let p01 = pixels[y0 * w + x1];
            let p10 = pixels[y1 * w + x0];
            let p11 = pixels[y1 * w + x1];
            let dst = (y * SIZE + x) * 3;
            for c in 0..3 {
                let top = clamp01(p00[c]) * (1.0 - wx) + clamp01(p01[c]) * wx;
                let bot = clamp01(p10[c]) * (1.0 - wx) + clamp01(p11[c]) * wx;
                out[dst + c] = top * (1.0 - wy) + bot * wy;
            }
        }
    }
    out
}

fn color_stats(nhwc: &[f32]) -> [f32; STATS_LEN] {
    let n = SIZE * SIZE;
    let mut chans = [
        vec![0.0f32; n],
        vec![0.0f32; n],
        vec![0.0f32; n],
        vec![0.0f32; n],
    ];
    for (i, rgb) in nhwc.as_chunks::<3>().0.iter().enumerate() {
        chans[0][i] = rgb[0];
        chans[1][i] = rgb[1];
        chans[2][i] = rgb[2];
        chans[3][i] = LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2];
    }
    let mut mean = [0.0f32; 4];
    let mut std = [0.0f32; 4];
    let mut pct = [[0.0f32; 7]; 4];
    for c in 0..4 {
        let sum: f64 = chans[c].iter().map(|&v| f64::from(v)).sum();
        let m = (sum / n as f64) as f32;
        mean[c] = m;
        let var: f64 = chans[c]
            .iter()
            .map(|&v| {
                let d = f64::from(v - m);
                d * d
            })
            .sum();
        std[c] = (var / n as f64).sqrt() as f32;
        chans[c].sort_by(f32::total_cmp);
        for (qi, &q) in QUANTILES.iter().enumerate() {
            let pos = q * (n - 1) as f32;
            let lo = pos.floor() as usize;
            let hi = (lo + 1).min(n - 1);
            let frac = pos - lo as f32;
            pct[c][qi] = chans[c][lo] * (1.0 - frac) + chans[c][hi] * frac;
        }
    }
    let mut out = [0.0f32; STATS_LEN];
    out[0..4].copy_from_slice(&mean);
    out[4..8].copy_from_slice(&std);
    for c in 0..4 {
        out[8 + c * 7..8 + (c + 1) * 7].copy_from_slice(&pct[c]);
    }
    let mr = mean[0].max(EPS);
    let mg = mean[1].max(EPS);
    let mb = mean[2].max(EPS);
    let p50r = pct[0][3].max(EPS);
    let p50g = pct[1][3].max(EPS);
    let p50b = pct[2][3].max(EPS);
    let p95r = pct[0][5].max(EPS);
    let p95g = pct[1][5].max(EPS);
    let p95b = pct[2][5].max(EPS);
    out[36] = (mr / mg).ln();
    out[37] = (mb / mg).ln();
    out[38] = (p50r / p50g).ln();
    out[39] = (p50b / p50g).ln();
    out[40] = (p95r / p95g).ln();
    out[41] = (p95b / p95g).ln();
    out
}

fn silu(v: f32) -> f32 {
    v / (1.0 + (-v).exp())
}

fn conv_silu_nhwc(
    c: &ConvWeights,
    inp: &[f32],
    h: usize,
    w: usize,
    residual: bool,
) -> (Vec<f32>, usize, usize) {
    let oh = h / c.stride;
    let ow = w / c.stride;
    let mut out = vec![0.0f32; oh * ow * c.cout];
    for oy in 0..oh {
        let iy_base = oy * c.stride;
        for ox in 0..ow {
            let ix_base = ox * c.stride;
            let dst = (oy * ow + ox) * c.cout;
            for co in 0..c.cout {
                let mut acc = c.bias[co];
                let w_co = co * c.cin * 9;
                for ky in 0..3usize {
                    let iy = iy_base as isize + ky as isize - 1;
                    if iy < 0 || iy >= h as isize {
                        continue;
                    }
                    let row = iy as usize * w;
                    for kx in 0..3usize {
                        let ix = ix_base as isize + kx as isize - 1;
                        if ix < 0 || ix >= w as isize {
                            continue;
                        }
                        let src = (row + ix as usize) * c.cin;
                        let k = ky * 3 + kx;
                        for ci in 0..c.cin {
                            acc += inp[src + ci] * c.weights[w_co + ci * 9 + k];
                        }
                    }
                }
                let mut v = silu(acc);
                if residual {
                    v += inp[dst + co];
                }
                out[dst + co] = v;
            }
        }
    }
    (out, oh, ow)
}

fn cpu_forward(m: &Model, nhwc: &[f32]) -> Vec<f32> {
    let (mut cur, mut h, mut w) = conv_silu_nhwc(&m.stem, nhwc, SIZE, SIZE, false);
    for st in &m.stages {
        let (hid, nh, nw) = conv_silu_nhwc(&st.down, &cur, h, w, false);
        let (res, _, _) = conv_silu_nhwc(&st.res, &hid, nh, nw, true);
        cur = res;
        h = nh;
        w = nw;
    }
    cur
}

fn pool_head(m: &Model, nhwc_out: &[f32], stats: &[f32; STATS_LEN]) -> [f64; 8] {
    let spatial = OUT_HW * OUT_HW;
    let mut feat = vec![0.0f32; FEAT_LEN];
    for c in 0..OUT_CH {
        let mut sum = 0.0f64;
        let mut max_v = f32::NEG_INFINITY;
        for p in 0..spatial {
            let v = nhwc_out[p * OUT_CH + c];
            sum += f64::from(v);
            if v > max_v {
                max_v = v;
            }
        }
        let mean = (sum / spatial as f64) as f32;
        let mut var = 0.0f64;
        for p in 0..spatial {
            let d = f64::from(nhwc_out[p * OUT_CH + c] - mean);
            var += d * d;
        }
        feat[c] = mean;
        feat[OUT_CH + c] = max_v;
        feat[OUT_CH * 2 + c] = (var / spatial as f64).sqrt() as f32;
    }
    feat[OUT_CH * 3..FEAT_LEN].copy_from_slice(stats);
    let mut x = feat;
    for (i, l) in m.head.iter().enumerate() {
        let mut y = Vec::with_capacity(l.cout);
        for co in 0..l.cout {
            let mut acc = l.bias[co];
            let row = &l.weights[co * l.cin..(co + 1) * l.cin];
            for (&a, &w) in x.iter().zip(row) {
                acc += a * w;
            }
            y.push(if i < 2 { silu(acc) } else { acc });
        }
        x = y;
    }
    std::array::from_fn(|i| f64::from(x[i]))
}

pub fn balance(width: u32, height: u32, pixels: &[[f32; 3]], gpu: bool) -> Neural {
    if width == 0 || height == 0 || pixels.len() != width as usize * height as usize {
        return Neural {
            exposure: [1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            white: [1.0; 3],
        };
    }
    let m = model();
    let nhwc = resize_nhwc_128(width, height, pixels);
    let stats = color_stats(&nhwc);
    let feat_map = if gpu {
        gpu_forward(m, &nhwc).unwrap_or_else(|| cpu_forward(m, &nhwc))
    } else {
        cpu_forward(m, &nhwc)
    };
    let z = pool_head(m, &feat_map, &stats);
    Neural {
        exposure: [
            (0.70 * z[0].tanh()).exp(),
            0.25 * z[1].tanh(),
            (0.70 * z[2].tanh()).exp(),
            0.65 * z[3].tanh(),
            (0.90 * z[4].tanh()).exp(),
            0.45 * z[5].tanh(),
        ],
        white: [(1.40 * z[6].tanh()).exp(), 1.0, (1.40 * z[7].tanh()).exp()],
    }
}
