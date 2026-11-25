use std::path::Path;

use vidcut_core::analysis::Found;

use super::net::{self, Onnx, Runner, Seen, plane, saw, within};
use crate::compute::Gpu;
use crate::compute::ops::{self, Act, Carve, Conv, Layer, Ops, Res, Shape, View};
use crate::error::Result;

pub use vidcut_core::catalog::RETINAFACE_ONNX as FILE;

pub const SIDE: u32 = 640;
pub const LEAST: f32 = 0.3;
pub const SAME: f32 = 0.4;
const WHAT: &str = "face detection";
const RELU: Act = Act::Leaky(0.0);
const MEAN: [f32; 3] = [104.0, 117.0, 123.0];
const STRIDES: [u32; 3] = [8, 16, 32];
const ANCHORS: [[f32; 2]; 3] = [[16.0, 32.0], [64.0, 128.0], [256.0, 512.0]];
const VARIANCE: [f32; 2] = [0.1, 0.2];
const DEPTHS: [usize; 4] = [3, 4, 6, 3];
const WIDTHS: [u32; 4] = [64, 128, 256, 512];
const FEATURES: u32 = 128;
const HEAD: u32 = 12;

pub fn name() -> String {
    let hundredths = |v: f32| (v * 100.0).round();
    format!(
        "retinaface-r34-{SIDE}-{}-{}",
        hundredths(LEAST),
        hundredths(SAME)
    )
}

fn node(name: &str) -> String {
    format!("/{name}/Conv")
}

fn conv(o: &Onnx, gpu: &Gpu, name: &str, s: Shape, act: Act) -> Result<Conv> {
    o.conv(gpu, &node(name), s, act)
}

fn layer(o: &Onnx, gpu: &Gpu, name: &str, s: Shape, act: Act) -> Result<Layer> {
    o.layer(gpu, &node(name), s, act)
}

fn head(o: &Onnx, gpu: &Gpu, level: usize) -> Result<Conv> {
    let (mut w, mut b) = o.parts(
        &node(&format!("class_head/class_head.{level}")),
        &Shape::square(FEATURES, 4, 1, 1, 0),
    )?;
    let (bw, bb) = o.parts(
        &node(&format!("bbox_head/bbox_head.{level}")),
        &Shape::square(FEATURES, 8, 1, 1, 0),
    )?;
    w.extend(bw);
    b.extend(bb);
    let s = Shape::square(FEATURES, HEAD, 1, 1, 0);
    Ok(ops::conv(gpu, s, &w, Some(&b), Act::None, None))
}

fn three<T>(mut make: impl FnMut(usize) -> Result<T>) -> Result<[T; 3]> {
    Ok([make(0)?, make(1)?, make(2)?])
}

struct Block {
    a: Layer,
    b: Layer,
    down: Option<Conv>,
}

struct Context {
    three: Layer,
    five_a: Layer,
    five_b: Layer,
    seven_a: Layer,
    seven_b: Layer,
}

struct Weights {
    stem: Conv,
    layers: Vec<Vec<Block>>,
    lateral: [Conv; 3],
    merge: [Layer; 2],
    context: [Context; 3],
    heads: [Conv; 3],
}

fn weights(gpu: &Gpu, o: &Onnx) -> Result<Weights> {
    let stem = conv(o, gpu, "fx/conv1", Shape::square(3, 64, 7, 2, 3), RELU)?;
    let mut layers = Vec::new();
    let mut cin = WIDTHS[0];
    for (i, (&depth, &c)) in DEPTHS.iter().zip(&WIDTHS).enumerate() {
        let mut blocks = Vec::new();
        for j in 0..depth {
            let at = |s: &str| format!("fx/layer{n}/layer{n}.{j}/{s}", n = i + 1);
            let make = |name: &str, s: Shape| match i {
                0 => conv(o, gpu, name, s, RELU).map(Layer::Plain),
                _ => layer(o, gpu, name, s, RELU),
            };
            let (from, stride) = match (i, j) {
                (1.., 0) => (cin, 2),
                _ => (c, 1),
            };
            let down = match stride {
                2 => Some(conv(
                    o,
                    gpu,
                    &at("downsample/downsample.0"),
                    Shape::square(from, c, 1, 2, 0),
                    Act::None,
                )?),
                _ => None,
            };
            blocks.push(Block {
                a: make(&at("conv1"), Shape::square(from, c, 3, stride, 1))?,
                b: make(&at("conv2"), Shape::square(c, c, 3, 1, 1))?,
                down,
            });
        }
        cin = c;
        layers.push(blocks);
    }
    let lateral = three(|i| {
        let n = i + 1;
        let s = Shape::square(WIDTHS[n], FEATURES, 1, 1, 0);
        conv(o, gpu, &format!("fpn/output{n}/output{n}.0"), s, RELU)
    })?;
    let merge = |n: u32| {
        let s = Shape::square(FEATURES, FEATURES, 3, 1, 1);
        layer(o, gpu, &format!("fpn/merge{n}/merge{n}.0"), s, RELU)
    };
    let context = three(|i| {
        let at = |s: &str| format!("ssh{}/{s}/{s}.0", i + 1);
        let half = FEATURES / 2;
        let quarter = FEATURES / 4;
        let sq = |cin, cout| Shape::square(cin, cout, 3, 1, 1);
        Ok(Context {
            three: layer(o, gpu, &at("conv3X3"), sq(FEATURES, half), RELU)?,
            five_a: layer(o, gpu, &at("conv5X5_1"), sq(FEATURES, quarter), RELU)?,
            five_b: layer(o, gpu, &at("conv5X5_2"), sq(quarter, quarter), RELU)?,
            seven_a: layer(o, gpu, &at("conv7X7_2"), sq(quarter, quarter), RELU)?,
            seven_b: layer(o, gpu, &at("conv7x7_3"), sq(quarter, quarter), RELU)?,
        })
    })?;
    let heads = three(|i| head(o, gpu, i))?;
    Ok(Weights {
        stem,
        layers,
        lateral,
        merge: [merge(1)?, merge(2)?],
        context,
        heads,
    })
}

struct Work {
    wide: View,
    spare: [View; 3],
    input: View,
    lateral: [View; 3],
    up: View,
    merged: [View; 2],
    context: [[View; 3]; 3],
    heads: View,
    scratch: [View; 2],
}

fn plan(c: &mut Carve, (w, h): (u32, u32), n: u32) -> Work {
    let cells: u32 = STRIDES.iter().map(|s| (h / s) * (w / s)).sum();
    let tiles =
        |s: u32, c: u32| ops::WINO as u32 * n * (h / s).div_ceil(4) * (w / s).div_ceil(4) * c;
    let most = STRIDES
        .iter()
        .zip(&WIDTHS[1..])
        .map(|(&s, &c)| tiles(s, c))
        .max()
        .unwrap_or(0);
    Work {
        wide: c.view(n, h / 2, w / 2, WIDTHS[0]),
        spare: [(); 3].map(|_| c.view(n, h / 4, w / 4, WIDTHS[0])),
        input: c.view(n, h, w, 3),
        lateral: STRIDES.map(|s| c.view(n, h / s, w / s, FEATURES)),
        up: c.view(n, h / 8, w / 8, FEATURES),
        merged: [8, 16].map(|s| c.view(n, h / s, w / s, FEATURES)),
        context: STRIDES.map(|s| {
            let quarter = FEATURES / 4;
            [
                c.view(n, h / s, w / s, quarter),
                c.view(n, h / s, w / s, quarter),
                c.view(n, h / s, w / s, FEATURES),
            ]
        }),
        heads: c.view(n, 1, cells, HEAD),
        scratch: [(); 2].map(|_| c.view(1, 1, 1, most)),
    }
}

pub(crate) struct Faces {
    weights: Weights,
    work: Work,
    size: (u32, u32),
    batch: u32,
}

pub(crate) type Model = Runner<Faces>;

pub(crate) fn load(path: &Path, (w, h): (u32, u32)) -> Result<Model> {
    let onnx = Onnx::open(path)?;
    let gpu = Gpu::start(WHAT, ops::SHARED).map_err(crate::error::Error::Invalid)?;
    let size = (super::padded(w), super::padded(h));
    let batch = net::batch(size);
    Runner::new(gpu, |gpu| {
        let weights = weights(gpu, &onnx)?;
        let (_, work) = ops::arena(gpu, |c| plan(c, size, batch))?;
        Ok(Faces {
            weights,
            work,
            size,
            batch,
        })
    })
}

impl Faces {
    fn levels(&self) -> [(u32, u32, u32); 3] {
        let (w, h) = self.size;
        let mut base = 0;
        STRIDES.map(|s| {
            let level = (base, h / s, w / s);
            base += (h / s) * (w / s) * HEAD * self.batch;
            level
        })
    }
}

impl net::Net for Faces {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn batch(&self) -> u32 {
        self.batch
    }

    fn wide(&self) -> &View {
        &self.work.wide
    }

    fn heads(&self) -> &View {
        &self.work.heads
    }

    fn forward(&self, o: &mut Ops, n: u32, mut seen: Option<&mut Vec<Seen>>) {
        let (net, k) = (&self.weights, &self.work);
        let (w, h) = self.size;
        let input = k.input.shaped(n, h, w, 3);
        let scratch = (&k.scratch[0], &k.scratch[1]);
        o.gpu.mark(|| "planes".into());
        for (ch, mean) in MEAN.iter().enumerate() {
            let from = plane(&k.wide, ch as u32, self.batch, n, (h, w));
            o.copy(&from, &input.part(ch as u32, 1), 1.0, -mean, Act::None);
        }
        saw(&mut seen, o.gpu, || "input".into(), &input);
        let stem = k.wide.shaped(n, h / 2, w / 2, WIDTHS[0]);
        o.gpu
            .mark(|| format!("stem {}x{}x{}", h / 2, w / 2, WIDTHS[0]));
        o.conv(&net.stem, &input, &stem, Res::None);
        saw(&mut seen, o.gpu, || "stem".into(), &stem);
        let mut dims = (h / 4, w / 4, WIDTHS[0]);
        o.gpu.mark(|| "pool".into());
        let pooled = within(&k.spare[0], 0, n, dims);
        o.pool(&stem, &pooled, 3, 2, 1);
        saw(&mut seen, o.gpu, || "pool".into(), &pooled);
        let (mut x, mut y) = (0, 1);
        let lateral = k.lateral.each_ref().map(|v| v.shaped(n, v.h, v.w, v.c));
        for (i, layer) in net.layers.iter().enumerate() {
            for (j, block) in layer.iter().enumerate() {
                let from = within(&k.spare[x], 0, n, dims);
                if block.down.is_some() {
                    dims = (dims.0 / 2, dims.1 / 2, dims.2);
                }
                dims.2 = WIDTHS[i];
                let middle = within(&k.wide, 0, n, dims);
                let out = within(&k.spare[y], 0, n, dims);
                let name = |part: &str| {
                    let (lh, lw, lc) = dims;
                    format!("layer{}.{j} {part} {lh}x{lw}x{lc}", i + 1)
                };
                o.gpu.mark(|| name("conv1"));
                o.layer(&block.a, &from, &middle, Res::None, scratch);
                saw(
                    &mut seen,
                    o.gpu,
                    || format!("layer{}.{j}.conv1", i + 1),
                    &middle,
                );
                match &block.down {
                    Some(down) => {
                        let skip = within(&k.spare[2], 0, n, dims);
                        o.gpu.mark(|| name("down"));
                        o.conv(down, &from, &skip, Res::None);
                        o.gpu.mark(|| name("conv2"));
                        o.layer(&block.b, &middle, &out, Res::Before(&skip), scratch);
                    }
                    None => {
                        o.gpu.mark(|| name("conv2"));
                        o.layer(&block.b, &middle, &out, Res::Before(&from), scratch);
                    }
                }
                saw(&mut seen, o.gpu, || format!("layer{}.{j}", i + 1), &out);
                (x, y) = (y, x);
            }
            if i > 0 {
                let out = within(&k.spare[x], 0, n, dims);
                o.gpu.mark(|| format!("lateral{i}"));
                o.conv(&net.lateral[i - 1], &out, &lateral[i - 1], Res::None);
                saw(&mut seen, o.gpu, || format!("lateral{i}"), &lateral[i - 1]);
            }
        }
        let merged = k.merged.each_ref().map(|v| v.shaped(n, v.h, v.w, v.c));
        let up = k.up.shaped(n, h / 16, w / 16, FEATURES);
        o.gpu.mark(|| "up16".into());
        o.nearest(&lateral[2], &up);
        o.add(&up, &lateral[1]);
        saw(&mut seen, o.gpu, || "add16".into(), &lateral[1]);
        o.gpu.mark(|| "merge16".into());
        o.layer(&net.merge[1], &lateral[1], &merged[1], Res::None, scratch);
        saw(&mut seen, o.gpu, || "merge16".into(), &merged[1]);
        let up = k.up.shaped(n, h / 8, w / 8, FEATURES);
        o.gpu.mark(|| "up8".into());
        o.nearest(&merged[1], &up);
        o.add(&up, &lateral[0]);
        saw(&mut seen, o.gpu, || "add8".into(), &lateral[0]);
        o.gpu.mark(|| "merge8".into());
        o.layer(&net.merge[0], &lateral[0], &merged[0], Res::None, scratch);
        saw(&mut seen, o.gpu, || "merge8".into(), &merged[0]);
        let inputs = [&merged[0], &merged[1], &lateral[2]];
        let (half, quarter) = (FEATURES / 2, FEATURES / 4);
        for (i, (base, lh, lw)) in self.levels().into_iter().enumerate() {
            let s = &net.context[i];
            let [a, b, out] = k.context[i].each_ref().map(|v| v.shaped(n, v.h, v.w, v.c));
            let stride = STRIDES[i];
            o.gpu.mark(|| format!("context{stride} three"));
            o.layer(&s.three, inputs[i], &out.part(0, half), Res::None, scratch);
            o.gpu.mark(|| format!("context{stride} five_a"));
            o.layer(&s.five_a, inputs[i], &a, Res::None, scratch);
            o.gpu.mark(|| format!("context{stride} five_b"));
            o.layer(&s.five_b, &a, &out.part(half, quarter), Res::None, scratch);
            o.gpu.mark(|| format!("context{stride} seven_a"));
            o.layer(&s.seven_a, &a, &b, Res::None, scratch);
            o.gpu.mark(|| format!("context{stride} seven_b"));
            o.layer(
                &s.seven_b,
                &b,
                &out.part(half + quarter, quarter),
                Res::None,
                scratch,
            );
            saw(&mut seen, o.gpu, || format!("context{stride}"), &out);
            let head = within(&k.heads, base, n, (lh, lw, HEAD));
            o.gpu.mark(|| format!("head{stride}"));
            o.conv(&net.heads[i], &out, &head, Res::None);
            saw(&mut seen, o.gpu, || format!("head{stride}"), &head);
        }
    }

    fn found(&self, values: &[f32], b: u32) -> Vec<Found> {
        let mut found = Vec::new();
        for (i, (base, lh, lw)) in self.levels().into_iter().enumerate() {
            let s = STRIDES[i] as f32;
            let first = (base + b * lh * lw * HEAD) as usize;
            for cell in 0..(lh * lw) as usize {
                let v = &values[first + cell * HEAD as usize..][..HEAD as usize];
                let (x, y) = ((cell % lw as usize) as f32, (cell / lw as usize) as f32);
                for (a, side) in ANCHORS[i].iter().enumerate() {
                    let score = 1.0 / (1.0 + (v[2 * a] - v[2 * a + 1]).exp());
                    if score < LEAST {
                        continue;
                    }
                    let b = &v[4 + 4 * a..8 + 4 * a];
                    found.push(Found {
                        center: [
                            (x + 0.5) * s + b[0] * VARIANCE[0] * side,
                            (y + 0.5) * s + b[1] * VARIANCE[0] * side,
                        ],
                        size: [
                            side * (b[2] * VARIANCE[1]).exp(),
                            side * (b[3] * VARIANCE[1]).exp(),
                        ],
                        rotation: 0.0,
                        score,
                    });
                }
            }
        }
        super::apart(found, SAME)
    }
}
