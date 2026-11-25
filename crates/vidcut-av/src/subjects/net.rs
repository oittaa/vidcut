use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tract_onnx::pb;
use tract_onnx::prelude::Framework;
use vidcut_core::analysis::Found;

use super::bad;
use crate::compute::ops::{self, Act, Conv, Kernels, Layer, Ops, Shape, View};
use crate::compute::{self, Gpu};
use crate::error::Result;

const STEPS: [&str; 5] = ["upload", "send", "clock", "wait", "boxes"];
const PIXELS: u64 = 2_000_000;
const MOST: u64 = 8;
const CLOCK: u32 = 256;

pub(super) struct Onnx {
    path: PathBuf,
    order: Vec<String>,
    convs: HashMap<String, [String; 2]>,
    tensors: HashMap<String, pb::TensorProto>,
}

impl Onnx {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let proto = tract_onnx::onnx()
            .proto_model_for_path(path)
            .map_err(|e| bad(path, e))?;
        let graph = proto.graph.ok_or_else(|| bad(path, "it has no graph"))?;
        let convs: Vec<(String, [String; 2])> = graph
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
            order: convs.iter().map(|(n, _)| n.clone()).collect(),
            convs: convs.into_iter().collect(),
            tensors,
        })
    }

    pub(super) fn order(&self) -> &[String] {
        &self.order
    }

    pub(super) fn wrong(&self, what: impl std::fmt::Display) -> crate::error::Error {
        bad(&self.path, what)
    }

    fn floats(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let wrong = || self.wrong(format!("{name} is not {shape:?} floats"));
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

    pub(super) fn parts(&self, node: &str, s: &Shape) -> Result<(Vec<f32>, Vec<f32>)> {
        let [w, b] = self
            .convs
            .get(node)
            .ok_or_else(|| self.wrong(format!("it has no {node}")))?;
        let (kh, kw) = s.k;
        let shape = [s.cout as usize, s.cin as usize, kh as usize, kw as usize];
        Ok((self.floats(w, &shape)?, self.floats(b, &[s.cout as usize])?))
    }

    pub(super) fn conv(&self, gpu: &Gpu, node: &str, s: Shape, act: Act) -> Result<Conv> {
        let (w, b) = self.parts(node, &s)?;
        Ok(ops::conv(gpu, s, &w, Some(&b), act, None))
    }

    pub(super) fn layer(&self, gpu: &Gpu, node: &str, s: Shape, act: Act) -> Result<Layer> {
        let (w, b) = self.parts(node, &s)?;
        Ok(ops::layer(gpu, s, &w, &b, act))
    }
}

pub(super) fn batch((w, h): (u32, u32)) -> u32 {
    (PIXELS / (u64::from(w) * u64::from(h))).clamp(1, MOST) as u32
}

pub(super) fn within(v: &View, base: u32, n: u32, (h, w, c): (u32, u32, u32)) -> View {
    View {
        base: v.base + base,
        ..v.shaped(n, h, w, c)
    }
}

pub(super) fn plane(wide: &View, ch: u32, batch: u32, n: u32, (h, w): (u32, u32)) -> View {
    within(wide, ch * batch * h * w, n, (h, w, 1))
}

pub(crate) struct Seen {
    name: String,
    view: View,
    after: u32,
}

pub(super) fn saw(
    seen: &mut Option<&mut Vec<Seen>>,
    gpu: &Gpu,
    name: impl FnOnce() -> String,
    view: &View,
) {
    if let Some(list) = seen {
        list.push(Seen {
            name: name(),
            view: view.clone(),
            after: gpu.dispatches(),
        });
    }
}

pub(crate) trait Net {
    fn size(&self) -> (u32, u32);
    fn batch(&self) -> u32;
    fn wide(&self) -> &View;
    fn heads(&self) -> &View;
    fn forward(&self, o: &mut Ops, n: u32, seen: Option<&mut Vec<Seen>>);
    fn found(&self, values: &[f32], b: u32) -> Vec<Found>;
}

struct Laps {
    on: bool,
    at: Instant,
    took: [f64; STEPS.len()],
}

impl Laps {
    fn new() -> Self {
        Self {
            on: crate::debug(),
            at: Instant::now(),
            took: [0.0; STEPS.len()],
        }
    }

    fn lap(&mut self, step: usize) {
        if self.on {
            let now = Instant::now();
            self.took[step] += (now - self.at).as_secs_f64() * 1000.0;
            self.at = now;
        }
    }

    fn report(&self, found: &[Vec<Found>], passes: usize) {
        if self.on {
            let pictures = found.len();
            let boxes: usize = found.iter().map(Vec::len).sum();
            let steps: Vec<String> = STEPS
                .iter()
                .zip(self.took)
                .map(|(name, ms)| format!("{name} {ms:.2} ms"))
                .collect();
            crate::debug!(
                "subjects",
                "model call of {pictures} pictures in {passes} passes, {boxes} boxes: {}",
                steps.join(", ")
            );
        }
    }
}

pub(crate) struct Runner<N> {
    gpu: Gpu,
    kernels: Kernels,
    net: N,
    timed: AtomicBool,
}

impl<N: Net> Runner<N> {
    pub(super) fn new(gpu: Gpu, make: impl FnOnce(&Gpu) -> Result<N>) -> Result<Self> {
        let (kernels, net) = compute::checked(&gpu.device, gpu.what, || {
            Ok((Kernels::new(&gpu), make(&gpu)?))
        })?;
        Ok(Self {
            gpu,
            kernels,
            net,
            timed: AtomicBool::new(false),
        })
    }

    pub(crate) fn size(&self) -> (usize, usize) {
        let (w, h) = self.net.size();
        (w as usize, h as usize)
    }

    pub(crate) fn find(&self, pictures: &[Vec<f32>]) -> Result<Vec<Vec<Found>>> {
        let gpu = &self.gpu;
        let heads = self.net.heads();
        let each = u64::from(heads.n * heads.h * heads.w * heads.c) * 4;
        let runs = pictures.chunks(self.net.batch() as usize);
        let all = each * runs.len() as u64;
        let staging = gpu.staging(all);
        let clocked = crate::debug() && !self.timed.swap(true, Ordering::Relaxed);
        let timed = clocked.then(|| runs.len().min(2) - 1);
        let mut laps = Laps::new();
        compute::checked(&gpu.device, gpu.what, || {
            for (i, run) in runs.clone().enumerate() {
                if timed == Some(i) {
                    gpu.time_from(CLOCK);
                }
                self.upload(run);
                laps.lap(0);
                let mut enc = self.encode(run.len() as u32, None);
                enc.copy_buffer_to_buffer(&heads.buf, 0, &staging, i as u64 * each, each);
                gpu.flush();
                gpu.queue.submit([enc.finish()]);
                laps.lap(1);
                if timed == Some(i) {
                    self.report(run.len())?;
                }
                laps.lap(2);
            }
            Ok(())
        })?;
        let raw = gpu.fetch(&staging, all, None)?;
        laps.lap(3);
        let values = compute::floats(&raw);
        let mut out = Vec::new();
        let passes = runs.len();
        for (run, v) in runs.zip(values.chunks((each / 4) as usize)) {
            out.extend((0..run.len()).map(|b| self.net.found(v, b as u32)));
        }
        laps.lap(4);
        laps.report(&out, passes);
        Ok(out)
    }

    fn upload(&self, run: &[Vec<f32>]) {
        let (w, h) = self.size();
        let plane = w * h;
        let batch = self.net.batch() as usize;
        let wide = &self.net.wide().buf;
        for (b, picture) in run.iter().enumerate() {
            for (ch, values) in picture.chunks(plane).take(3).enumerate() {
                let at = (ch * batch + b) * plane * 4;
                self.gpu
                    .queue
                    .write_buffer(wide, at as u64, &compute::bytes(values));
            }
        }
    }

    fn encode(&self, n: u32, seen: Option<&mut Vec<Seen>>) -> wgpu::CommandEncoder {
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            let mut o = Ops {
                gpu: &self.gpu,
                k: &self.kernels,
                pass: &mut pass,
            };
            o.gpu.mark(|| "start".into());
            o.gpu.begin(o.pass);
            self.net.forward(&mut o, n, seen);
        }
        enc
    }

    pub(crate) fn probe(&self, dir: &Path, pictures: &[Vec<f32>]) -> Result<()> {
        let gpu = &self.gpu;
        let run = &pictures[..pictures.len().min(2).min(self.net.batch() as usize)];
        let n = run.len() as u32;
        let mut seen = Vec::new();
        let done = compute::checked(&gpu.device, gpu.what, || {
            gpu.stop_after(Some(u32::MAX));
            drop(self.encode(n, Some(&mut seen)));
            for (i, s) in seen.iter().enumerate() {
                gpu.stop_after(Some(s.after));
                self.upload(run);
                let mut enc = self.encode(n, None);
                let v = &s.view;
                let size = u64::from((v.n * v.h * v.w - 1) * v.stride + v.at + v.c) * 4;
                let staging = gpu.staging(size);
                enc.copy_buffer_to_buffer(&v.buf, u64::from(v.base) * 4, &staging, 0, size);
                gpu.flush();
                gpu.queue.submit([enc.finish()]);
                let values = compute::floats(&gpu.fetch(&staging, size, None)?);
                let mut out = Vec::new();
                for p in 0..(v.n * v.h * v.w) as usize {
                    let at = p * v.stride as usize + v.at as usize;
                    out.extend_from_slice(&values[at..at + v.c as usize]);
                }
                let shape = [v.n, v.h, v.w, v.c].map(|x| x as usize);
                let file = dir.join(format!("stage-{i:02}-{}.npy", s.name));
                crate::npy::write(&file, &shape, &out)?;
            }
            Ok(())
        });
        gpu.stop_after(None);
        done
    }

    fn report(&self, n: usize) -> Result<()> {
        let how = if self.gpu.stamps() {
            "timestamps"
        } else {
            "a wait for each dispatch"
        };
        let times = self.gpu.times()?;
        let all: f64 = times.iter().map(|(_, ms)| ms).sum();
        let (w, h) = self.size();
        let name = &self.gpu.name;
        crate::debug!(
            "subjects",
            "gpu pass of {n} pictures of {w}x{h} on {name}: {all:.3} ms ({how})"
        );
        for (name, ms) in &times {
            crate::debug!("subjects", "gpu {name}: {ms:.3} ms");
        }
        Ok(())
    }
}
