use std::ops::RangeInclusive;

use crate::error::{Error, Result};
use crate::model::{ClipAudio, SoundFade, USUAL_X_LEVEL, USUAL_X_POINT};
use crate::request::SoundFadeSet;

pub const X_LEVEL: RangeInclusive<f64> = -120.0..=-10.0;

pub const X_POINT: RangeInclusive<f64> = 1.0..=99.0;

pub const SHAPES: [(f64, f64); 5] = [
    (18.0, -55.0),
    (34.0, -57.0),
    (50.0, -60.0),
    (50.0, -90.0),
    (50.0, -105.0),
];

impl SoundFade {
    pub fn linear(frames: f64) -> Self {
        Self {
            frames,
            x_level: USUAL_X_LEVEL,
            x_point: USUAL_X_POINT,
        }
    }

    pub fn is_linear(&self) -> bool {
        self.x_level == USUAL_X_LEVEL && self.x_point == USUAL_X_POINT
    }

    pub fn level(&self) -> f64 {
        0.5f64.powf(-self.x_level / 60.0)
    }

    pub fn power(&self, falling: bool) -> f64 {
        let at = self.x_point / 100.0;
        let at = if falling { 1.0 - at } else { at };
        -self.x_level / 60.0 * 0.5f64.ln() / at.ln()
    }

    pub fn along(&self, u: f64, falling: bool) -> f64 {
        let u = u.clamp(0.0, 1.0);
        let t = if falling { 1.0 - u } else { u };
        t.powf(self.power(falling))
    }

    pub fn shape(&self, falling: bool) -> Option<usize> {
        SHAPES.iter().position(|&(x, y)| {
            let x = if falling { 100.0 - x } else { x };
            self.x_point == x && self.x_level == y
        })
    }
}

pub fn shaped(frames: f64, shape: usize, falling: bool) -> Option<SoundFade> {
    let &(x, y) = SHAPES.get(shape)?;
    Some(SoundFade {
        frames,
        x_level: y,
        x_point: if falling { 100.0 - x } else { x },
    })
}

pub fn problem(f: &SoundFade) -> Option<String> {
    if !(f.frames.is_finite() && f.frames > 0.0) {
        return Some(format!("a fade of {} frames is not more than 0", f.frames));
    }
    if !X_LEVEL.contains(&f.x_level) {
        return Some(format!(
            "X-Level {} is outside {}..{}",
            f.x_level,
            X_LEVEL.start(),
            X_LEVEL.end()
        ));
    }
    if !X_POINT.contains(&f.x_point) {
        return Some(format!(
            "X-Point {} is outside {}..{}",
            f.x_point,
            X_POINT.start(),
            X_POINT.end()
        ));
    }
    None
}

pub fn patched(old: Option<SoundFade>, set: &SoundFadeSet) -> Result<SoundFade> {
    let frames = set
        .frames
        .or(old.map(|f| f.frames))
        .ok_or_else(|| Error::usage("a new fade needs `frames`"))?;
    let f = SoundFade {
        frames,
        x_level: set
            .x_level
            .or(old.map(|f| f.x_level))
            .unwrap_or(USUAL_X_LEVEL),
        x_point: set
            .x_point
            .or(old.map(|f| f.x_point))
            .unwrap_or(USUAL_X_POINT),
    };
    match problem(&f) {
        Some(why) => Err(Error::invalid(why)),
        None => Ok(f),
    }
}

impl ClipAudio {
    pub fn fade_lengths(&self, duration: i64) -> (f64, f64) {
        let whole = duration.max(0) as f64;
        let into = self.fade_in.map_or(0.0, |f| f.frames.min(whole));
        let out = self.fade_out.map_or(0.0, |f| f.frames.min(whole - into));
        (into, out)
    }

    pub fn fades_fit(&self, duration: i64) -> bool {
        let into = self.fade_in.map_or(0.0, |f| f.frames);
        let out = self.fade_out.map_or(0.0, |f| f.frames);
        into + out <= duration as f64 + 1e-9
    }

    pub fn cut(&mut self, head: bool, tail: bool) {
        crate::otio::effects::cut(&mut self.effects, head, tail);
        if head {
            self.fade_in = None;
        }
        if tail {
            self.fade_out = None;
        }
    }
}

pub fn fitted(id: &str, a: &ClipAudio, duration: i64) -> Result<()> {
    if a.fades_fit(duration) {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "the fades of {id} are longer than its {duration} frames"
    )))
}
