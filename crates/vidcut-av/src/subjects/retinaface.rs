use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tract_onnx::pb;
use tract_onnx::prelude::Framework;
use vidcut_core::analysis::Found;

use super::bad;
use crate::compute::ops::{self, Act, Carve, Conv, Kernels, Ops, Res, Shape, View};
use crate::compute::{self, Gpu};
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
const PIXELS: u64 = 2_000_000;
const MOST: u64 = 8;

pub fn name() -> String {
    let hundredths = |v: f32| (v * 100.0).round();
    format!(
        "retinaface-r34-{SIDE}-{}-{}",
        hundredths(LEAST),
        hundredths(SAME)
    )
}

struct Onnx {
    path: PathBuf,
    convs: HashMap<String, [String; 2]>,
    tensors: HashMap<String, pb::TensorProto>,
}

impl Onnx {
    fn open(path: &Path) -> Result<Self> {
        let proto = tract_onnx::onnx()
            .proto_model_for_path(path)
            .map_err(|e| bad(path, e))?;
        let graph = proto.graph.ok_or_else(|| bad(path, "it has no graph"))?;
        let convs = graph
            .node
            .iter()
            .filter(|n| n.op_type == "Conv" && n.input.len() == 3)
            .map(|n| (n.name.clone(), [n.input[1].clone(), n.input[2].clone()]))
            .collect();
        let tensors = graph
            .initializer
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();
        Ok(Self {
            path: path.to_path_buf(),
            convs,
            tensors,
        })
    }

    fn floats(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let wrong = || bad(&self.path, format!("{name} is not {shape:?} floats"));
        let t = self.tensors.get(name).ok_or_else(wrong)?;
        let dims: Vec<usize> = t.dims.iter().map(|&d| d as usize).collect();
        if dims != shape || t.data_type != pb::tensor_proto::DataType::Float as i32 {
            return Err(wrong());
        }
        let v = if t.raw_data.is_empty() {
            t.float_data.clone()
        } else {
            compute::floats(&t.raw_data)
        };
        if v.len() != shape.iter().product::<usize>() {
            return Err(wrong());
        }
        Ok(v)
    }

    fn parts(&self, node: &str, s: &Shape) -> Result<(Vec<f32>, Vec<f32>)> {
        let name = format!("/{node}/Conv");
        let [w, b] = self
            .convs
            .get(&name)
            .ok_or_else(|| bad(&self.path, format!("it has no {name}")))?;
        let (kh, kw) = s.k;
        let shape = [s.cout as usize, s.cin as usize, kh as usize, kw as usize];
        Ok((self.floats(w, &shape)?, self.floats(b, &[s.cout as usize])?))
    }

    fn conv(&self, gpu: &Gpu, node: &str, s: Shape, act: Act) -> Result<Conv> {
        let (w, b) = self.parts(node, &s)?;
        Ok(ops::conv(gpu, s, &w, Some(&b), act, None))
    }

    fn head(&self, gpu: &Gpu, level: usize) -> Result<Conv> {
        let (mut w, mut b) = self.parts(
            &format!("class_head/class_head.{level}"),
            &Shape::square(FEATURES, 4, 1, 1, 0),
        )?;
        let (bw, bb) = self.parts(
            &format!("bbox_head/bbox_head.{level}"),
            &Shape::square(FEATURES, 8, 1, 1, 0),
        )?;
        w.extend(bw);
        b.extend(bb);
        let s = Shape::square(FEATURES, HEAD, 1, 1, 0);
        Ok(ops::conv(gpu, s, &w, Some(&b), Act::None, None))
    }
}

fn three<T>(mut make: impl FnMut(usize) -> Result<T>) -> Result<[T; 3]> {
    Ok([make(0)?, make(1)?, make(2)?])
}

struct Block {
    a: Conv,
    b: Conv,
    down: Option<Conv>,
}

struct Context {
    three: Conv,
    five_a: Conv,
    five_b: Conv,
    seven_a: Conv,
    seven_b: Conv,
}

struct Net {
    stem: Conv,
    layers: Vec<Vec<Block>>,
    lateral: [Conv; 3],
    merge: [Conv; 2],
    context: [Context; 3],
    heads: [Conv; 3],
}

fn net(gpu: &Gpu, o: &Onnx) -> Result<Net> {
    let stem = o.conv(gpu, "fx/conv1", Shape::square(3, 64, 7, 2, 3), RELU)?;
    let mut layers = Vec::new();
    let mut cin = WIDTHS[0];
    for (i, (&depth, &c)) in DEPTHS.iter().zip(&WIDTHS).enumerate() {
        let mut blocks = Vec::new();
        for j in 0..depth {
            let at = |s: &str| format!("fx/layer{n}/layer{n}.{j}/{s}", n = i + 1);
            let (from, stride) = match (i, j) {
                (1.., 0) => (cin, 2),
                _ => (c, 1),
            };
            let down = match stride {
                2 => Some(o.conv(
                    gpu,
                    &at("downsample/downsample.0"),
                    Shape::square(from, c, 1, 2, 0),
                    Act::None,
                )?),
                _ => None,
            };
            blocks.push(Block {
                a: o.conv(
                    gpu,
                    &at("conv1"),
                    Shape::square(from, c, 3, stride, 1),
                    RELU,
                )?,
                b: o.conv(gpu, &at("conv2"), Shape::square(c, c, 3, 1, 1), RELU)?,
                down,
            });
        }
        cin = c;
        layers.push(blocks);
    }
    let lateral = three(|i| {
        let n = i + 1;
        let s = Shape::square(WIDTHS[n], FEATURES, 1, 1, 0);
        o.conv(gpu, &format!("fpn/output{n}/output{n}.0"), s, RELU)
    })?;
    let merge = |n: u32| {
        let s = Shape::square(FEATURES, FEATURES, 3, 1, 1);
        o.conv(gpu, &format!("fpn/merge{n}/merge{n}.0"), s, RELU)
    };
    let context = three(|i| {
        let at = |s: &str| format!("ssh{}/{s}/{s}.0", i + 1);
        let half = FEATURES / 2;
        let quarter = FEATURES / 4;
        let sq = |cin, cout| Shape::square(cin, cout, 3, 1, 1);
        Ok(Context {
            three: o.conv(gpu, &at("conv3X3"), sq(FEATURES, half), RELU)?,
            five_a: o.conv(gpu, &at("conv5X5_1"), sq(FEATURES, quarter), RELU)?,
            five_b: o.conv(gpu, &at("conv5X5_2"), sq(quarter, quarter), RELU)?,
            seven_a: o.conv(gpu, &at("conv7X7_2"), sq(quarter, quarter), RELU)?,
            seven_b: o.conv(gpu, &at("conv7x7_3"), sq(quarter, quarter), RELU)?,
        })
    })?;
    let heads = three(|i| o.head(gpu, i))?;
    Ok(Net {
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
}

fn within(v: &View, base: u32, n: u32, (h, w, c): (u32, u32, u32)) -> View {
    View {
        base: v.base + base,
        ..v.shaped(n, h, w, c)
    }
}

fn plan(c: &mut Carve, (w, h): (u32, u32), n: u32) -> Work {
    let cells: u32 = STRIDES.iter().map(|s| (h / s) * (w / s)).sum();
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
    }
}

pub(crate) struct Model {
    gpu: Gpu,
    kernels: Kernels,
    net: Net,
    work: Work,
    size: (usize, usize),
    batch: u32,
}

impl Model {
    pub(crate) fn load(path: &Path, (w, h): (u32, u32)) -> Result<Self> {
        let onnx = Onnx::open(path)?;
        let gpu = Gpu::start(WHAT, ops::SHARED).map_err(crate::error::Error::Invalid)?;
        let (w, h) = (super::padded(w), super::padded(h));
        let batch = (PIXELS / (u64::from(w) * u64::from(h))).clamp(1, MOST) as u32;
        let (kernels, net, (_, work)) = compute::checked(&gpu.device, WHAT, || {
            Ok((
                Kernels::new(&gpu),
                net(&gpu, &onnx)?,
                ops::arena(&gpu, |c| plan(c, (w, h), batch))?,
            ))
        })?;
        Ok(Self {
            gpu,
            kernels,
            net,
            work,
            size: (w as usize, h as usize),
            batch,
        })
    }

    pub(crate) fn size(&self) -> (usize, usize) {
        self.size
    }

    fn levels(&self) -> [(u32, u32, u32); 3] {
        let (w, h) = (self.size.0 as u32, self.size.1 as u32);
        let mut base = 0;
        STRIDES.map(|s| {
            let level = (base, h / s, w / s);
            base += (h / s) * (w / s) * HEAD * self.batch;
            level
        })
    }

    fn forward(&self, o: &mut Ops, n: u32) {
        let (net, k) = (&self.net, &self.work);
        let (w, h) = (self.size.0 as u32, self.size.1 as u32);
        let input = k.input.shaped(n, h, w, 3);
        for (ch, mean) in MEAN.iter().enumerate() {
            let plane = within(&k.wide, ch as u32 * self.batch * h * w, n, (h, w, 1));
            o.copy(&plane, &input.part(ch as u32, 1), 1.0, -mean, Act::None);
        }
        let stem = k.wide.shaped(n, h / 2, w / 2, WIDTHS[0]);
        o.conv(&net.stem, &input, &stem, Res::None);
        let mut dims = (h / 4, w / 4, WIDTHS[0]);
        o.pool(&stem, &within(&k.spare[0], 0, n, dims), 3, 2, 1);
        let (mut x, mut y) = (0, 1);
        let lateral = k.lateral.each_ref().map(|v| v.shaped(n, v.h, v.w, v.c));
        for (i, layer) in net.layers.iter().enumerate() {
            for block in layer {
                let from = within(&k.spare[x], 0, n, dims);
                if block.down.is_some() {
                    dims = (dims.0 / 2, dims.1 / 2, dims.2);
                }
                dims.2 = WIDTHS[i];
                let middle = within(&k.wide, 0, n, dims);
                let out = within(&k.spare[y], 0, n, dims);
                o.conv(&block.a, &from, &middle, Res::None);
                match &block.down {
                    Some(down) => {
                        let skip = within(&k.spare[2], 0, n, dims);
                        o.conv(down, &from, &skip, Res::None);
                        o.conv(&block.b, &middle, &out, Res::Before(&skip));
                    }
                    None => o.conv(&block.b, &middle, &out, Res::Before(&from)),
                }
                (x, y) = (y, x);
            }
            if i > 0 {
                let out = within(&k.spare[x], 0, n, dims);
                o.conv(&net.lateral[i - 1], &out, &lateral[i - 1], Res::None);
            }
        }
        let merged = k.merged.each_ref().map(|v| v.shaped(n, v.h, v.w, v.c));
        let up = k.up.shaped(n, h / 16, w / 16, FEATURES);
        o.nearest(&lateral[2], &up);
        o.add(&up, &lateral[1]);
        o.conv(&net.merge[1], &lateral[1], &merged[1], Res::None);
        let up = k.up.shaped(n, h / 8, w / 8, FEATURES);
        o.nearest(&merged[1], &up);
        o.add(&up, &lateral[0]);
        o.conv(&net.merge[0], &lateral[0], &merged[0], Res::None);
        let inputs = [&merged[0], &merged[1], &lateral[2]];
        let (half, quarter) = (FEATURES / 2, FEATURES / 4);
        for (i, (base, lh, lw)) in self.levels().into_iter().enumerate() {
            let s = &net.context[i];
            let [a, b, out] = k.context[i].each_ref().map(|v| v.shaped(n, v.h, v.w, v.c));
            o.conv(&s.three, inputs[i], &out.part(0, half), Res::None);
            o.conv(&s.five_a, inputs[i], &a, Res::None);
            o.conv(&s.five_b, &a, &out.part(half, quarter), Res::None);
            o.conv(&s.seven_a, &a, &b, Res::None);
            o.conv(
                &s.seven_b,
                &b,
                &out.part(half + quarter, quarter),
                Res::None,
            );
            let head = within(&k.heads, base, n, (lh, lw, HEAD));
            o.conv(&net.heads[i], &out, &head, Res::None);
        }
    }

    pub(crate) fn find(&self, pictures: &[Vec<f32>]) -> Result<Vec<Vec<Found>>> {
        let gpu = &self.gpu;
        let heads = &self.work.heads;
        let each = u64::from(heads.n * heads.w * heads.c) * 4;
        let runs = pictures.chunks(self.batch as usize);
        let all = each * runs.len() as u64;
        let staging = gpu.staging(all);
        let plane = self.size.0 * self.size.1;
        compute::checked(&gpu.device, WHAT, || {
            for (i, run) in runs.clone().enumerate() {
                for (b, picture) in run.iter().enumerate() {
                    for (ch, values) in picture.chunks(plane).take(3).enumerate() {
                        let at = (ch * self.batch as usize + b) * plane * 4;
                        let wide = &self.work.wide.buf;
                        gpu.queue
                            .write_buffer(wide, at as u64, &compute::bytes(values));
                    }
                }
                let mut enc = gpu.device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_compute_pass(&Default::default());
                    let mut o = Ops {
                        gpu,
                        k: &self.kernels,
                        pass: &mut pass,
                    };
                    self.forward(&mut o, run.len() as u32);
                }
                enc.copy_buffer_to_buffer(&heads.buf, 0, &staging, i as u64 * each, each);
                gpu.flush();
                gpu.queue.submit([enc.finish()]);
            }
            Ok(())
        })?;
        let raw = gpu.fetch(&staging, all, None)?;
        let values = compute::floats(&raw);
        let mut out = Vec::new();
        for (run, v) in runs.zip(values.chunks((each / 4) as usize)) {
            out.extend((0..run.len()).map(|b| self.found(v, b as u32)));
        }
        Ok(out)
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
