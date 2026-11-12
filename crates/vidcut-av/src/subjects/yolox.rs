use std::path::Path;

use tract_onnx::prelude::*;
use vidcut_core::analysis::Found;

use super::bad;
use crate::error::{Error, Result};

pub use vidcut_core::catalog::YOLOX_ONNX as FILE;

pub const SIDE: u32 = 640;
pub const LEAST: f32 = 0.25;
pub const SAME: f32 = 0.5;
pub const PEOPLE: &[usize] = &[0];
pub const VEHICLES: &[usize] = &[1, 2, 3, 5, 6, 7];
const PAD: f32 = 114.0;
const STRIDES: [usize; 3] = [8, 16, 32];
const CLASSES: usize = 80;
const ROW: usize = 5 + CLASSES;

pub(crate) struct Model {
    plan: Arc<TypedRunnableModel>,
    classes: &'static [usize],
    side: usize,
}

pub fn name(classes: &[usize]) -> String {
    let hundredths = |v: f32| (v * 100.0).round();
    let classes: String = classes.iter().map(|c| format!("-{c}")).collect();
    format!(
        "yolox-{SIDE}-{PAD}-{}-{}-rgb{classes}",
        hundredths(LEAST),
        hundredths(SAME)
    )
}

fn colors(picture: &[f32], (cw, ch): (usize, usize), side: usize) -> Vec<f32> {
    let plane = side * side;
    let mut out = vec![PAD; 3 * plane];
    for (c, to) in out.chunks_exact_mut(plane).enumerate() {
        let from = &picture[(2 - c) * plane..(3 - c) * plane];
        for y in 0..ch.min(side) {
            let row = y * side..y * side + cw.min(side);
            to[row.clone()].copy_from_slice(&from[row]);
        }
    }
    out
}

fn named(row: &[f32]) -> (usize, f32) {
    let mut best = (0, f32::MIN);
    for (class, share) in row[5..].iter().enumerate() {
        let score = row[4] * share;
        if score > best.1 {
            best = (class, score);
        }
    }
    best
}

impl Model {
    pub(crate) fn load(path: &Path, classes: &'static [usize], size: (u32, u32)) -> Result<Self> {
        let onnx = tract_onnx::onnx()
            .with_ignore_output_shapes(true)
            .with_ignore_value_info(true);
        let mut model = onnx.model_for_path(path).map_err(|e| bad(path, e))?;
        let side = (size.0.max(size.1) as usize).div_ceil(32) * 32;
        model
            .set_input_fact(0, f32::fact([1, 3, side, side]).into())
            .map_err(|e| bad(path, e))?;
        let plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| bad(path, e))?;
        Ok(Self {
            plan,
            classes,
            side,
        })
    }

    pub(crate) fn size(&self) -> (usize, usize) {
        (self.side, self.side)
    }

    pub(crate) fn find(&self, picture: &[f32], content: (usize, usize)) -> Result<Vec<Found>> {
        let failed = |e: TractError| Error::Invalid(format!("object detection: {e}"));
        let side = self.side;
        let colors = colors(picture, content, side);
        let input = Tensor::from_shape(&[1, 3, side, side], &colors).map_err(failed)?;
        let out = self.plan.run(tvec!(input.into())).map_err(failed)?;
        let view = out[0].to_plain_array_view::<f32>().map_err(failed)?;
        let cells: usize = STRIDES.iter().map(|s| (side / s) * (side / s)).sum();
        let rows = view.as_slice().filter(|s| s.len() == cells * ROW);
        let rows =
            rows.ok_or_else(|| Error::Invalid("object detection: an unexpected output".into()))?;
        let mut rows = rows.as_chunks::<ROW>().0.iter();
        let mut found = Vec::new();
        for stride in STRIDES {
            let cols = side / stride;
            for (i, row) in rows.by_ref().take(cols * cols).enumerate() {
                let (class, score) = named(row);
                if score < LEAST || !self.classes.contains(&class) {
                    continue;
                }
                let (col, line, s) = ((i % cols) as f32, (i / cols) as f32, stride as f32);
                found.push(Found {
                    center: [(row[0] + col) * s, (row[1] + line) * s],
                    size: [row[2].exp() * s, row[3].exp() * s],
                    rotation: 0.0,
                    score,
                });
            }
        }
        Ok(super::apart(found, SAME))
    }
}
