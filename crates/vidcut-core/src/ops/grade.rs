use serde_json::Value;

use crate::error::{Error, Result};
use crate::model::{Clip, Grade, Project};
use crate::preferences::ColorPrefs;
use crate::request::{GradeChange, PrinterColor};

use super::locate;
use super::picture::takes;

impl GradeChange {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn changed(&self, old: &Grade) -> Grade {
        let mut g = old.clone();
        let put = |to: &mut f64, v: Option<f64>| {
            if let Some(v) = v {
                *to = v;
            }
        };
        if let Some(v) = self.lift {
            g.lift = v;
        }
        if let Some(v) = self.gamma {
            g.gamma = v;
        }
        if let Some(v) = self.gain {
            g.gain = v;
        }
        if let Some(v) = self.offset {
            g.offset = v;
        }
        if let Some(v) = self.log_shadow {
            g.log_shadow = v;
        }
        if let Some(v) = self.log_midtone {
            g.log_midtone = v;
        }
        if let Some(v) = self.log_highlight {
            g.log_highlight = v;
        }
        put(&mut g.contrast, self.contrast);
        put(&mut g.pivot, self.pivot);
        put(&mut g.saturation, self.saturation);
        put(&mut g.hue, self.hue);
        put(&mut g.lum_mix, self.lum_mix);
        put(&mut g.temp, self.temp);
        put(&mut g.tint, self.tint);
        put(&mut g.color_boost, self.color_boost);
        put(&mut g.shadows, self.shadows);
        put(&mut g.highlights, self.highlights);
        put(&mut g.midtone_detail, self.midtone_detail);
        put(&mut g.low_range, self.low_range);
        put(&mut g.high_range, self.high_range);
        g
    }
}

fn within(v: f64, (low, high): (f64, f64), what: &str) -> Result<()> {
    match v.is_finite() && (low..=high).contains(&v) {
        true => Ok(()),
        false => Err(Error::invalid(format!(
            "{what} must be from {low} to {high}"
        ))),
    }
}

fn finite(values: &[f64], what: &str) -> Result<()> {
    match values.iter().all(|v| v.is_finite()) {
        true => Ok(()),
        false => Err(Error::invalid(format!("{what} must be a number"))),
    }
}

pub fn checked(g: &Grade) -> Result<Option<Grade>> {
    finite(&g.lift, "lift")?;
    finite(&g.gamma, "gamma")?;
    finite(&g.gain, "gain")?;
    finite(&g.offset, "offset")?;
    finite(&g.log_shadow, "log_shadow")?;
    finite(&g.log_midtone, "log_midtone")?;
    finite(&g.log_highlight, "log_highlight")?;
    within(g.contrast, (0.0, 2.0), "contrast")?;
    within(g.pivot, (0.0, 1.0), "pivot")?;
    within(g.saturation, (0.0, 100.0), "saturation")?;
    within(g.hue, (0.0, 100.0), "hue")?;
    within(g.lum_mix, (0.0, 100.0), "lum_mix")?;
    within(g.temp, (-4000.0, 4000.0), "temp")?;
    within(g.tint, (-100.0, 100.0), "tint")?;
    within(g.color_boost, (-100.0, 100.0), "color_boost")?;
    within(g.shadows, (-100.0, 100.0), "shadows")?;
    within(g.highlights, (-100.0, 100.0), "highlights")?;
    within(g.midtone_detail, (-100.0, 100.0), "midtone_detail")?;
    if !(g.low_range > 0.0 && g.low_range < g.high_range && g.high_range < 1.0) {
        return Err(Error::invalid(
            "low_range must be above 0, below high_range, and high_range below 1",
        ));
    }
    Ok((!g.is_usual()).then(|| g.clone()))
}

const NEEDS: &str = "grade.set needs a value to change";

impl PrinterColor {
    pub fn channels(self) -> [f64; 3] {
        match self {
            Self::Red => [1.0, 0.0, 0.0],
            Self::Green => [0.0, 1.0, 0.0],
            Self::Blue => [0.0, 0.0, 1.0],
            Self::Cyan => [0.0, 1.0, 1.0],
            Self::Magenta => [1.0, 0.0, 1.0],
            Self::Yellow => [1.0, 1.0, 0.0],
            Self::Master => [1.0; 3],
        }
    }
}

pub fn printed(
    offset: [f64; 3],
    (color, points): (PrinterColor, f64),
    lights: &ColorPrefs,
) -> [f64; 3] {
    let on = color.channels();
    std::array::from_fn(|i| {
        offset[i] + points * on[i] * lights.printer_step[i] * lights.printer_density[i]
    })
}

pub fn printer(
    p: &mut Project,
    ids: &[String],
    point: (PrinterColor, f64),
    lights: &ColorPrefs,
) -> Result<()> {
    if !point.1.is_finite() {
        return Err(Error::invalid("points must be a number"));
    }
    for id in ids {
        let offset = clip_grade(p, id)?.offset;
        let change = GradeChange {
            offset: Some(printed(offset, point, lights)),
            ..GradeChange::default()
        };
        set(p, std::slice::from_ref(id), &change)?;
    }
    Ok(())
}

fn grades(p: &Project, ti: usize, c: &Clip) -> Result<()> {
    takes(p, ti, c)?;
    if c.generator.is_some() {
        return Err(Error::invalid(format!(
            "{} is a generator, which takes no grade",
            c.id
        )));
    }
    Ok(())
}

fn clip_grade(p: &Project, id: &str) -> Result<Grade> {
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    grades(p, ti, c)?;
    Ok(c.picture
        .as_ref()
        .and_then(|pic| pic.grade.clone())
        .unwrap_or_default())
}

pub fn set(p: &mut Project, ids: &[String], change: &GradeChange) -> Result<()> {
    if change.is_empty() {
        return Err(Error::usage(NEEDS));
    }
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        grades(p, ti, &p.tracks[ti].clips[ci])?;
        let c = &p.tracks[ti].clips[ci];
        let old = c
            .picture
            .as_ref()
            .and_then(|pic| pic.grade.clone())
            .unwrap_or_default();
        let grade = checked(&change.changed(&old))?;
        let mut picture = c.picture.clone().unwrap_or_default();
        picture.grade = grade;
        p.tracks[ti].clips[ci].picture = (!picture.is_usual()).then_some(picture);
    }
    Ok(())
}

pub fn clear(p: &mut Project, ids: &[String]) -> Result<()> {
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let c = &mut p.tracks[ti].clips[ci];
        if let Some(mut picture) = c.picture.take() {
            picture.grade = None;
            c.picture = (!picture.is_usual()).then_some(picture);
        }
    }
    Ok(())
}

pub fn get(p: &Project, id: &str) -> Result<Value> {
    serde_json::to_value(clip_grade(p, id)?).map_err(|e| Error::invalid(e.to_string()))
}
