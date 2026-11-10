use std::path::Path;

use tract_onnx::prelude::*;
use vidcut_core::analysis::Found;

use crate::error::{Error, Result};

pub use vidcut_core::catalog::PPOCR_DET_ONNX as FILE;

pub const SIDE: u32 = 960;
pub const LEAST: f32 = 0.5;
const INK: f32 = 0.3;
const GROW: f32 = 1.5;
const SMALLEST: f32 = 3.0;
const MOST: usize = 1000;
const LONG: f32 = 4.0;
const LEVEL: f32 = 2.0;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const SPREAD: [f32; 3] = [0.229, 0.224, 0.225];

pub(crate) struct Model {
    plan: Arc<TypedRunnableModel>,
    size: (usize, usize),
}

fn bad(path: &Path, what: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{}: {what}", path.display()))
}

fn near(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    let xs = x.saturating_sub(1)..=(x + 1).min(w - 1);
    xs.flat_map(move |nx| (y.saturating_sub(1)..=(y + 1).min(h - 1)).map(move |ny| (nx, ny)))
}

fn regions(map: &[f32], width: usize, (cw, ch): (usize, usize)) -> Vec<Vec<(usize, usize)>> {
    let mut seen = vec![false; cw * ch];
    let mut out = Vec::new();
    for start in 0..cw * ch {
        let (x, y) = (start % cw, start / cw);
        if seen[start] || map[y * width + x] <= INK {
            continue;
        }
        seen[start] = true;
        let mut region = vec![(x, y)];
        let mut next = 0;
        while let Some(&(x, y)) = region.get(next) {
            next += 1;
            for (nx, ny) in near(x, y, cw, ch) {
                if !seen[ny * cw + nx] && map[ny * width + nx] > INK {
                    seen[ny * cw + nx] = true;
                    region.push((nx, ny));
                }
            }
        }
        out.push(region);
    }
    out
}

fn lean(region: &[(usize, usize)]) -> f32 {
    let n = region.len() as f32;
    let sum = |f: &dyn Fn(f32, f32) -> f32| {
        let each = region.iter().map(|&(x, y)| f(x as f32, y as f32));
        each.sum::<f32>() / n
    };
    let (mx, my) = (sum(&|x, _| x), sum(&|_, y| y));
    let across = sum(&|x, _| (x - mx) * (x - mx));
    let down = sum(&|_, y| (y - my) * (y - my));
    let both = sum(&|x, y| (x - mx) * (y - my));
    let half = (across - down) / 2.0;
    let reach = half.hypot(both);
    let middle = (across + down) / 2.0;
    if middle + reach < LONG * (middle - reach).max(1e-6) {
        return 0.0;
    }
    let angle = (0.5 * both.atan2(half)).to_degrees();
    let angle = if angle.abs() > 45.0 {
        angle - 90.0 * angle.signum()
    } else {
        angle
    };
    if angle.abs() < LEVEL { 0.0 } else { angle }
}

fn line(map: &[f32], width: usize, region: &[(usize, usize)]) -> Option<Found> {
    let angle = lean(region);
    let (sin, cos) = angle.to_radians().sin_cos();
    let turned = |x: f32, y: f32| (x * cos + y * sin, y * cos - x * sin);
    let mut low = (f32::MAX, f32::MAX);
    let mut high = (f32::MIN, f32::MIN);
    for &(x, y) in region {
        let (u, v) = turned(x as f32 + 0.5, y as f32 + 0.5);
        low = (low.0.min(u), low.1.min(v));
        high = (high.0.max(u), high.1.max(v));
    }
    let (w, h) = (high.0 - low.0 + 1.0, high.1 - low.1 + 1.0);
    if w.min(h) < SMALLEST {
        return None;
    }
    let (u, v) = ((low.0 + high.0) / 2.0, (low.1 + high.1) / 2.0);
    let center = [u * cos - v * sin, u * sin + v * cos];
    let xs = region.iter().map(|p| p.0);
    let ys = region.iter().map(|p| p.1);
    let (x0, x1) = (xs.clone().min()?, xs.max()?);
    let (y0, y1) = (ys.clone().min()?, ys.max()?);
    let (mut total, mut count) = (0.0, 0usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (pu, pv) = turned(x as f32 + 0.5, y as f32 + 0.5);
            if (pu - u).abs() <= w / 2.0 && (pv - v).abs() <= h / 2.0 {
                total += map[y * width + x];
                count += 1;
            }
        }
    }
    let score = total / count.max(1) as f32;
    if score < LEAST {
        return None;
    }
    let more = w * h * GROW / (w + h);
    Some(Found {
        center,
        size: [w + more, h + more],
        rotation: -angle,
        score,
    })
}

fn edges(f: &Found) -> [f32; 4] {
    let ([x, y], [w, h]) = (f.center, f.size);
    [x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0]
}

fn meets(a: &Found, b: &Found) -> bool {
    let (a, b) = (edges(a), edges(b));
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

fn both(a: &Found, b: &Found) -> Found {
    let (ea, eb) = (edges(a), edges(b));
    let low = [ea[0].min(eb[0]), ea[1].min(eb[1])];
    let high = [ea[2].max(eb[2]), ea[3].max(eb[3])];
    let (sa, sb) = (a.size[0] * a.size[1], b.size[0] * b.size[1]);
    Found {
        center: [(low[0] + high[0]) / 2.0, (low[1] + high[1]) / 2.0],
        size: [high[0] - low[0], high[1] - low[1]],
        rotation: 0.0,
        score: (a.score * sa + b.score * sb) / (sa + sb).max(1e-6),
    }
}

fn blocks(lines: Vec<Found>) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for mut line in lines {
        while line.rotation == 0.0
            && let Some(i) = out
                .iter()
                .position(|o| o.rotation == 0.0 && meets(o, &line))
        {
            line = both(&out.swap_remove(i), &line);
        }
        out.push(line);
    }
    out
}

fn lines(map: &[f32], width: usize, content: (usize, usize)) -> Vec<Found> {
    let regions = regions(map, width, content);
    let found: Vec<Found> = regions.iter().filter_map(|r| line(map, width, r)).collect();
    let mut found = blocks(found);
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    found.truncate(MOST);
    found
}

pub fn name() -> String {
    let hundredths = |v: f32| (v * 100.0).round();
    let colors = MEAN.iter().chain(&SPREAD);
    let colors: String = colors
        .map(|v| format!("-{}", (v * 1000.0).round()))
        .collect();
    format!(
        "ppocr-{SIDE}-{}-{}-{}-{}-{MOST}-{}-{}{colors}-blocks",
        hundredths(LEAST),
        hundredths(INK),
        hundredths(GROW),
        hundredths(SMALLEST),
        hundredths(LONG),
        hundredths(LEVEL)
    )
}

impl Model {
    pub(crate) fn load(path: &Path, (w, h): (u32, u32)) -> Result<Self> {
        let onnx = tract_onnx::onnx()
            .with_ignore_output_shapes(true)
            .with_ignore_value_info(true);
        let mut model = onnx.model_for_path(path).map_err(|e| bad(path, e))?;
        let (w, h) = (super::padded(w) as usize, super::padded(h) as usize);
        model
            .set_input_fact(0, f32::fact([1, 3, h, w]).into())
            .map_err(|e| bad(path, e))?;
        let plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| bad(path, e))?;
        Ok(Self { plan, size: (w, h) })
    }

    pub(crate) fn size(&self) -> (usize, usize) {
        self.size
    }

    pub(crate) fn find(&self, picture: &[f32], content: (usize, usize)) -> Result<Vec<Found>> {
        let failed = |e: TractError| Error::Invalid(format!("text detection: {e}"));
        let (w, h) = self.size;
        let plane = w * h;
        let scaled: Vec<f32> = picture
            .iter()
            .enumerate()
            .map(|(i, v)| (v / 255.0 - MEAN[i / plane]) / SPREAD[i / plane])
            .collect();
        let input = Tensor::from_shape(&[1, 3, h, w], &scaled).map_err(failed)?;
        let out = self.plan.run(tvec!(input.into())).map_err(failed)?;
        let view = out[0].to_plain_array_view::<f32>().map_err(failed)?;
        let map = view.as_slice().filter(|s| s.len() == plane);
        let map =
            map.ok_or_else(|| Error::Invalid("text detection: an unexpected output".into()))?;
        Ok(lines(map, w, content))
    }
}
