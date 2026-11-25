use std::path::Path;

use tract_onnx::prelude::*;
use vidcut_core::analysis::Found;

use super::bad;
use super::net::Runner;
use crate::error::{Error, Result};

mod gpu;

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

enum Run {
    Cpu(Arc<TypedRunnableModel>),
    Gpu(Box<Runner<gpu::Things>>),
}

pub(crate) struct Model {
    run: Run,
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

fn boxes(classes: &[usize], rows: &[f32], (side, stride): (usize, usize), found: &mut Vec<Found>) {
    let cols = side / stride;
    for (i, row) in rows.as_chunks::<ROW>().0.iter().enumerate() {
        let (class, score) = named(row);
        if score < LEAST || !classes.contains(&class) {
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

impl Model {
    pub(crate) fn load(
        path: &Path,
        classes: &'static [usize],
        size: (u32, u32),
        on_gpu: bool,
    ) -> Result<Self> {
        let side = super::padded(size.0.max(size.1));
        let run = match on_gpu {
            true => Run::Gpu(Box::new(gpu::load(path, classes, side)?)),
            false => Run::Cpu(cpu(path, side as usize)?),
        };
        Ok(Self {
            run,
            classes,
            side: side as usize,
        })
    }

    pub(crate) fn size(&self) -> (usize, usize) {
        (self.side, self.side)
    }

    pub(crate) fn found(
        &self,
        pictures: &[Vec<f32>],
        content: (usize, usize),
    ) -> Result<Vec<Vec<Found>>> {
        match &self.run {
            Run::Cpu(plan) => super::each(pictures, &|p| self.find(plan, p, content)),
            Run::Gpu(run) => run.find(&self.colored(pictures, content)),
        }
    }

    pub(crate) fn probe(
        &self,
        dir: &Path,
        pictures: &[Vec<f32>],
        content: (usize, usize),
    ) -> Result<()> {
        match &self.run {
            Run::Cpu(_) => Ok(()),
            Run::Gpu(run) => run.probe(dir, &self.colored(pictures, content)),
        }
    }

    fn colored(&self, pictures: &[Vec<f32>], content: (usize, usize)) -> Vec<Vec<f32>> {
        pictures
            .iter()
            .map(|p| colors(p, content, self.side))
            .collect()
    }

    fn find(
        &self,
        plan: &Arc<TypedRunnableModel>,
        picture: &[f32],
        content: (usize, usize),
    ) -> Result<Vec<Found>> {
        let failed = |e: TractError| Error::Invalid(format!("object detection: {e}"));
        let side = self.side;
        let colors = colors(picture, content, side);
        let input = Tensor::from_shape(&[1, 3, side, side], &colors).map_err(failed)?;
        let out = plan.run(tvec!(input.into())).map_err(failed)?;
        let view = out[0].to_plain_array_view::<f32>().map_err(failed)?;
        let cells: usize = STRIDES.iter().map(|s| (side / s) * (side / s)).sum();
        let rows = view.as_slice().filter(|s| s.len() == cells * ROW);
        let rows =
            rows.ok_or_else(|| Error::Invalid("object detection: an unexpected output".into()))?;
        let mut found = Vec::new();
        let mut at = 0;
        for stride in STRIDES {
            let cells = (side / stride) * (side / stride);
            let level = &rows[at..][..cells * ROW];
            boxes(self.classes, level, (side, stride), &mut found);
            at += cells * ROW;
        }
        Ok(super::apart(found, SAME))
    }
}

fn cpu(path: &Path, side: usize) -> Result<Arc<TypedRunnableModel>> {
    let onnx = tract_onnx::onnx()
        .with_ignore_output_shapes(true)
        .with_ignore_value_info(true);
    let mut model = onnx.model_for_path(path).map_err(|e| bad(path, e))?;
    model
        .set_input_fact(0, f32::fact([1, 3, side, side]).into())
        .map_err(|e| bad(path, e))?;
    model
        .into_optimized()
        .and_then(|m| m.into_runnable())
        .map_err(|e| bad(path, e))
}
