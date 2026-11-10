use std::path::Path;

use tract_onnx::prelude::*;
use vidcut_core::analysis::Found;

use crate::error::{Error, Result};

pub use vidcut_core::catalog::YUNET_ONNX as FILE;

pub const SIDE: u32 = 640;
pub const LEAST: f32 = 0.3;
pub const SAME: f32 = 0.3;
const STRIDES: [usize; 3] = [8, 16, 32];

pub(crate) struct Model {
    plan: Arc<TypedRunnableModel>,
    size: (usize, usize),
    outputs: [[usize; 3]; 3],
}

pub fn name() -> String {
    format!("yunet-{SIDE}-{}", (LEAST * 100.0).round())
}

fn bad(path: &Path, what: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{}: {what}", path.display()))
}

fn overlap(a: &Found, b: &Found) -> f32 {
    let side = |i: usize| {
        let (a0, a1) = (a.center[i] - a.size[i] / 2.0, a.center[i] + a.size[i] / 2.0);
        let (b0, b1) = (b.center[i] - b.size[i] / 2.0, b.center[i] + b.size[i] / 2.0);
        (a1.min(b1) - a0.max(b0)).max(0.0)
    };
    let both = side(0) * side(1);
    let all = a.size[0] * a.size[1] + b.size[0] * b.size[1] - both;
    if all > 0.0 { both / all } else { 0.0 }
}

impl Model {
    pub(crate) fn load(path: &Path, (w, h): (u32, u32)) -> Result<Self> {
        let onnx = tract_onnx::onnx()
            .with_ignore_output_shapes(true)
            .with_ignore_value_info(true);
        let proto = onnx.proto_model_for_path(path).map_err(|e| bad(path, e))?;
        let names: Vec<&str> = proto
            .graph
            .iter()
            .flat_map(|g| g.output.iter().map(|o| o.name.as_str()))
            .collect();
        let at = |kind: &str, stride: usize| {
            let name = format!("{kind}_{stride}");
            let found = names.iter().position(|n| **n == name);
            found.ok_or_else(|| bad(path, format!("it has no output {name}")))
        };
        let mut outputs = [[0; 3]; 3];
        for (slots, stride) in outputs.iter_mut().zip(STRIDES) {
            *slots = [at("cls", stride)?, at("obj", stride)?, at("bbox", stride)?];
        }
        let (w, h) = (super::padded(w) as usize, super::padded(h) as usize);
        let mut model = onnx
            .model_for_proto_model(&proto)
            .map_err(|e| bad(path, e))?;
        model
            .set_input_fact(0, f32::fact([1, 3, h, w]).into())
            .map_err(|e| bad(path, e))?;
        let plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| bad(path, e))?;
        Ok(Self {
            plan,
            size: (w, h),
            outputs,
        })
    }

    pub(crate) fn size(&self) -> (usize, usize) {
        self.size
    }

    pub(crate) fn find(&self, picture: &[f32]) -> Result<Vec<Found>> {
        let failed = |e: TractError| Error::Invalid(format!("face detection: {e}"));
        let (w, h) = self.size;
        let input = Tensor::from_shape(&[1, 3, h, w], picture).map_err(failed)?;
        let out = self.plan.run(tvec!(input.into())).map_err(failed)?;
        let mut found = Vec::new();
        for (slots, stride) in self.outputs.iter().zip(STRIDES) {
            let (cols, rows) = (w / stride, h / stride);
            let views = [
                out[slots[0]].to_plain_array_view::<f32>().map_err(failed)?,
                out[slots[1]].to_plain_array_view::<f32>().map_err(failed)?,
                out[slots[2]].to_plain_array_view::<f32>().map_err(failed)?,
            ];
            let flat = |i: usize, len: usize| {
                let s = views[i].as_slice().filter(|s| s.len() == len);
                s.ok_or_else(|| Error::Invalid("face detection: an unexpected output".into()))
            };
            let cells = cols * rows;
            let (cls, obj, boxes) = (flat(0, cells)?, flat(1, cells)?, flat(2, cells * 4)?);
            for i in 0..cells {
                let score = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
                if score < LEAST {
                    continue;
                }
                let (col, row, s) = ((i % cols) as f32, (i / cols) as f32, stride as f32);
                let b = &boxes[i * 4..i * 4 + 4];
                found.push(Found {
                    center: [(col + b[0]) * s, (row + b[1]) * s],
                    size: [b[2].exp() * s, b[3].exp() * s],
                    rotation: 0.0,
                    score,
                });
            }
        }
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut kept: Vec<Found> = Vec::new();
        for f in found {
            if kept.iter().all(|k| overlap(k, &f) <= SAME) {
                kept.push(f);
            }
        }
        Ok(kept)
    }
}
