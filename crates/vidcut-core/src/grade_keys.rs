use std::borrow::Cow;

use crate::model::{Corrector, Grade, GradeKey, GradeKeyKind, GradeNode, GradeTool};

pub const LAST_PROFILE: u8 = 10;

const STEPS: u32 = 1024;

const HALVINGS: u32 = 60;

type Ease = (f64, f64);

fn ease(profile: u8) -> Option<Ease> {
    Some(match profile {
        2 => (3.0, 4.0),
        3 => (3.0, 6.0),
        4 => (2.0, 2.0),
        5 => (2.0, 3.0),
        6 => (2.0, 5.0),
        7 => (0.5, 5f64.powf(-0.5)),
        8 => (0.5, 3f64.powf(-0.5)),
        9 => (0.5, 2f64.powf(-0.5)),
        10 => (1.0 / 3.0, 6f64.powf(-1.0 / 3.0)),
        _ => return None,
    })
}

fn rise((p, a): Ease, u: f64) -> f64 {
    a * u.powf(p)
}

fn rise_slope((p, a): Ease, u: f64) -> f64 {
    a * p * u.powf(p - 1.0)
}

fn fall(e: Ease, u: f64) -> f64 {
    1.0 - rise(e, 1.0 - u)
}

fn fall_slope(e: Ease, u: f64) -> f64 {
    rise_slope(e, 1.0 - u)
}

fn root(f: impl Fn(f64) -> Option<f64>) -> Option<f64> {
    let mut last: Option<(f64, f64)> = None;
    for i in 1..STEPS {
        let x = f64::from(i) / f64::from(STEPS);
        let Some(y) = f(x).filter(|y| y.is_finite()) else {
            last = None;
            continue;
        };
        if y == 0.0 {
            return Some(x);
        }
        if let Some((from, was)) = last
            && (was > 0.0) != (y > 0.0)
        {
            let (mut low, mut high) = (from, x);
            for _ in 0..HALVINGS {
                let mid = (low + high) / 2.0;
                match f(mid) {
                    Some(m) if (m > 0.0) == (was > 0.0) => low = mid,
                    _ => high = mid,
                }
            }
            return Some((low + high) / 2.0);
        }
        last = Some((x, y));
    }
    None
}

fn joints(s: Ease, e: Ease) -> (f64, f64) {
    let tangent = |a: f64| {
        let m = rise_slope(s, a);
        let b = 1.0 - (m / (e.1 * e.0)).powf(1.0 / (e.0 - 1.0));
        (a <= b && b <= 1.0).then(|| (b, fall(e, b) - rise(s, a) - m * (b - a)))
    };
    if let Some(a) = root(|a| tangent(a).map(|(_, g)| g)) {
        return (a, tangent(a).map_or(a, |(b, _)| b));
    }
    let u = root(|u| Some(rise_slope(s, u) - fall_slope(e, u))).unwrap_or(0.5);
    let (low, high) = (rise(s, u), fall(e, u));
    if (low - u).abs() >= (high - u).abs() {
        let m = rise_slope(s, u);
        let b = root(|x| (x > u).then(|| low + m * (x - u) - fall(e, x)));
        (u, b.unwrap_or(u))
    } else {
        let m = fall_slope(e, u);
        let a = root(|x| (x < u).then(|| high + m * (x - u) - rise(s, x)));
        (a.unwrap_or(u), u)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dissolve {
    start: Option<Ease>,
    end: Option<Ease>,
    joints: (f64, f64),
    held: bool,
}

impl Dissolve {
    pub fn of(start: u8, end: u8) -> Self {
        let (s, e) = (ease(start), ease(end));
        let joints = match (s, e) {
            (Some(s), Some(e)) => joints(s, e),
            (Some(s), None) => {
                let a = root(|a| Some(1.0 - rise(s, a) - rise_slope(s, a) * (1.0 - a)));
                (a.unwrap_or(1.0), 1.0)
            }
            (None, Some(e)) => {
                let b = root(|b| Some(fall(e, b) - fall_slope(e, b) * b));
                (0.0, b.unwrap_or(0.0))
            }
            (None, None) => (0.0, 1.0),
        };
        Self {
            start: s,
            end: e,
            joints,
            held: start == 0 || end == 0,
        }
    }

    pub fn at(&self, u: f64) -> f64 {
        if self.held {
            return 0.0;
        }
        let u = u.clamp(0.0, 1.0);
        let (a, b) = self.joints;
        let low = |u: f64| self.start.map_or(0.0, |s| rise(s, u));
        let high = |u: f64| self.end.map_or(1.0, |e| fall(e, u));
        let w = if u < a {
            low(u)
        } else if u > b {
            high(u)
        } else if b > a {
            low(a) + (high(b) - low(a)) * (u - a) / (b - a)
        } else {
            low(a)
        };
        w.clamp(0.0, 1.0)
    }
}

fn mixed<const N: usize>(a: &[f64; N], b: &[f64; N], w: f64) -> [f64; N] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * w)
}

fn moved<const N: usize>(own: &[f64; N], was: &[f64; N], now: &[f64; N]) -> [f64; N] {
    std::array::from_fn(|i| if was[i] == now[i] { own[i] } else { now[i] })
}

impl Corrector {
    pub fn blended(&self, to: &Corrector, w: f64) -> Corrector {
        let one = |a: f64, b: f64| a + (b - a) * w;
        Corrector {
            lift: mixed(&self.lift, &to.lift, w),
            gamma: mixed(&self.gamma, &to.gamma, w),
            gain: mixed(&self.gain, &to.gain, w),
            offset: mixed(&self.offset, &to.offset, w),
            contrast: one(self.contrast, to.contrast),
            pivot: one(self.pivot, to.pivot),
            saturation: one(self.saturation, to.saturation),
            hue: one(self.hue, to.hue),
            lum_mix: one(self.lum_mix, to.lum_mix),
            temp: one(self.temp, to.temp),
            tint: one(self.tint, to.tint),
            color_boost: one(self.color_boost, to.color_boost),
            shadows: one(self.shadows, to.shadows),
            highlights: one(self.highlights, to.highlights),
            midtone_detail: one(self.midtone_detail, to.midtone_detail),
            log_shadow: mixed(&self.log_shadow, &to.log_shadow, w),
            log_midtone: mixed(&self.log_midtone, &to.log_midtone, w),
            log_highlight: mixed(&self.log_highlight, &to.log_highlight, w),
            low_range: one(self.low_range, to.low_range),
            high_range: one(self.high_range, to.high_range),
        }
    }
}

impl Corrector {
    pub fn carried(&self, was: &Corrector, now: &Corrector) -> Corrector {
        let one = |own: f64, was: f64, now: f64| if was == now { own } else { now };
        Corrector {
            lift: moved(&self.lift, &was.lift, &now.lift),
            gamma: moved(&self.gamma, &was.gamma, &now.gamma),
            gain: moved(&self.gain, &was.gain, &now.gain),
            offset: moved(&self.offset, &was.offset, &now.offset),
            contrast: one(self.contrast, was.contrast, now.contrast),
            pivot: one(self.pivot, was.pivot, now.pivot),
            saturation: one(self.saturation, was.saturation, now.saturation),
            hue: one(self.hue, was.hue, now.hue),
            lum_mix: one(self.lum_mix, was.lum_mix, now.lum_mix),
            temp: one(self.temp, was.temp, now.temp),
            tint: one(self.tint, was.tint, now.tint),
            color_boost: one(self.color_boost, was.color_boost, now.color_boost),
            shadows: one(self.shadows, was.shadows, now.shadows),
            highlights: one(self.highlights, was.highlights, now.highlights),
            midtone_detail: one(self.midtone_detail, was.midtone_detail, now.midtone_detail),
            log_shadow: moved(&self.log_shadow, &was.log_shadow, &now.log_shadow),
            log_midtone: moved(&self.log_midtone, &was.log_midtone, &now.log_midtone),
            log_highlight: moved(&self.log_highlight, &was.log_highlight, &now.log_highlight),
            low_range: one(self.low_range, was.low_range, now.low_range),
            high_range: one(self.high_range, was.high_range, now.high_range),
        }
    }
}

impl GradeKey {
    pub fn is_static(&self) -> bool {
        self.kind == GradeKeyKind::Static
    }
}

impl GradeNode {
    pub fn own(&self) -> Option<&Corrector> {
        match &self.tool {
            GradeTool::Corrector(c) => Some(c),
            _ => None,
        }
    }

    pub fn changed_key(&self, frame: i64) -> Option<usize> {
        self.keys
            .partition_point(|k| k.frame <= frame)
            .checked_sub(1)
    }

    fn paired_key(&self, key: Option<usize>) -> Option<Option<usize>> {
        match key {
            Some(i) if !self.keys[i].is_static() => match i.checked_sub(1) {
                None => Some(None),
                Some(j) => self.keys[j].is_static().then_some(Some(j)),
            },
            _ => {
                let next = key.map_or(0, |i| i + 1);
                let dynamic = self.keys.get(next).is_some_and(|k| !k.is_static());
                dynamic.then_some(Some(next))
            }
        }
    }

    fn values_mut(&mut self, key: Option<usize>) -> Option<&mut Corrector> {
        match (key, &mut self.tool) {
            (Some(i), GradeTool::Corrector(_)) => Some(&mut self.keys[i].values),
            (None, GradeTool::Corrector(c)) => Some(&mut **c),
            _ => None,
        }
    }

    pub fn changed_at(&self, frame: i64) -> Option<&Corrector> {
        let own = self.own()?;
        Some(
            self.changed_key(frame)
                .map_or(own, |i| &self.keys[i].values),
        )
    }

    pub fn change_at(
        &mut self,
        frame: i64,
        change: impl FnOnce(&Corrector) -> Corrector,
    ) -> Option<()> {
        let key = self.changed_key(frame);
        let was = self.changed_at(frame)?.clone();
        let now = change(&was);
        if let Some(other) = self.paired_key(key).and_then(|k| self.values_mut(k)) {
            *other = other.carried(&was, &now);
        }
        *self.values_mut(key)? = now;
        Some(())
    }

    pub fn corrector_at(&self, frame: f64) -> Option<Cow<'_, Corrector>> {
        let own = self.own()?;
        let i = self.keys.partition_point(|k| k.frame as f64 <= frame);
        let Some(k) = i.checked_sub(1).map(|i| &self.keys[i]) else {
            return Some(Cow::Borrowed(own));
        };
        let next = self.keys.get(i).filter(|_| !k.is_static());
        Some(match next {
            Some(n) => {
                let u = (frame - k.frame as f64) / (n.frame - k.frame) as f64;
                match Dissolve::of(k.start, k.end).at(u) {
                    w if w <= 0.0 => Cow::Borrowed(&k.values),
                    w => Cow::Owned(k.values.blended(&n.values, w)),
                }
            }
            None => Cow::Borrowed(&k.values),
        })
    }

    pub fn key_on(&self, frame: i64) -> Option<&GradeKey> {
        self.keys.iter().find(|k| k.frame == frame)
    }

    pub fn put_key(&mut self, key: GradeKey) {
        match self.keys.binary_search_by_key(&key.frame, |k| k.frame) {
            Ok(i) => self.keys[i] = key,
            Err(i) => self.keys.insert(i, key),
        }
    }
}

impl Grade {
    pub fn is_keyed(&self) -> bool {
        self.nodes.iter().any(|d| !d.keys.is_empty())
    }

    pub fn at(&self, frame: f64) -> Cow<'_, Grade> {
        if !self.is_keyed() {
            return Cow::Borrowed(self);
        }
        let mut g = self.clone();
        for (d, from) in g.nodes.iter_mut().zip(&self.nodes) {
            if let (GradeTool::Corrector(c), Some(now)) = (&mut d.tool, from.corrector_at(frame)) {
                **c = now.into_owned();
            }
            d.keys.clear();
            d.animated = false;
        }
        Cow::Owned(g)
    }

    pub fn clear_keys(&mut self) {
        for d in &mut self.nodes {
            d.keys.clear();
            d.animated = false;
        }
    }

    pub fn key_frames(&self) -> Vec<i64> {
        let mut frames: Vec<i64> = self
            .nodes
            .iter()
            .flat_map(|d| d.keys.iter().map(|k| k.frame))
            .collect();
        frames.sort_unstable();
        frames.dedup();
        frames
    }

    pub fn map_key_frames(&mut self, map: impl Fn(i64) -> i64) {
        for d in &mut self.nodes {
            for k in &mut d.keys {
                k.frame = map(k.frame);
            }
            d.keys.sort_by_key(|k| k.frame);
            d.keys.dedup_by_key(|k| k.frame);
        }
    }
}
