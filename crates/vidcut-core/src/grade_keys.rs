use std::borrow::Cow;

use crate::model::{
    Corrector, DynamicsProfile, Grade, GradeKey, GradeKeyKind, GradeNode, GradeTool, GradeWindow,
    KeyInput, KeyMix, KeyOutput, WindowShape,
};
use crate::window_key::{circle_scale, circle_size, linear_scale, linear_size};

pub const LAST_PROFILE: u8 = 10;

const STEPS: u32 = 1024;

const HALVINGS: u32 = 60;

type Ease = (f64, f64);

type Scale = fn(f64) -> f64;

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
            auto_color: self.auto_color.clone(),
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
            auto_color: match was.auto_color == now.auto_color {
                true => self.auto_color.clone(),
                false => now.auto_color.clone(),
            },
        }
    }
}

impl KeyOutput {
    pub fn blended(&self, to: &KeyOutput, w: f64) -> KeyOutput {
        KeyOutput {
            gain: self.gain + (to.gain - self.gain) * w,
            offset: self.offset + (to.offset - self.offset) * w,
            invert: self.invert,
        }
    }

    pub fn carried(&self, was: &KeyOutput, now: &KeyOutput) -> KeyOutput {
        KeyOutput {
            gain: if was.gain == now.gain {
                self.gain
            } else {
                now.gain
            },
            offset: if was.offset == now.offset {
                self.offset
            } else {
                now.offset
            },
            invert: if was.invert == now.invert {
                self.invert
            } else {
                now.invert
            },
        }
    }
}

impl KeyInput {
    pub fn blended(&self, to: &KeyInput, w: f64) -> KeyInput {
        let one = |a: f64, b: f64| a + (b - a) * w;
        KeyInput {
            gain: one(self.gain, to.gain),
            offset: one(self.offset, to.offset),
            blur: one(self.blur, to.blur),
            blur_ratio: one(self.blur_ratio, to.blur_ratio),
            ..*self
        }
    }

    pub fn carried(&self, was: &KeyInput, now: &KeyInput) -> KeyInput {
        KeyInput {
            invert: pick(&self.invert, &was.invert, &now.invert),
            mask: pick(&self.mask, &was.mask, &now.mask),
            gain: pick(&self.gain, &was.gain, &now.gain),
            offset: pick(&self.offset, &was.offset, &now.offset),
            blur: pick(&self.blur, &was.blur, &now.blur),
            blur_ratio: pick(&self.blur_ratio, &was.blur_ratio, &now.blur_ratio),
        }
    }
}

impl KeyMix {
    pub fn blended(&self, to: &KeyMix, w: f64) -> KeyMix {
        KeyMix {
            gain: self.gain + (to.gain - self.gain) * w,
            offset: self.offset + (to.offset - self.offset) * w,
            ..*self
        }
    }

    pub fn carried(&self, was: &KeyMix, now: &KeyMix) -> KeyMix {
        KeyMix {
            invert: pick(&self.invert, &was.invert, &now.invert),
            matte: pick(&self.matte, &was.matte, &now.matte),
            gain: pick(&self.gain, &was.gain, &now.gain),
            offset: pick(&self.offset, &was.offset, &now.offset),
        }
    }
}

fn pick<T: PartialEq + Clone>(own: &T, was: &T, now: &T) -> T {
    match was == now {
        true => own.clone(),
        false => now.clone(),
    }
}

fn each<T: PartialEq + Clone>(own: &[T], was: &[T], now: &[T]) -> Vec<T> {
    match own.len() == now.len() && was.len() == now.len() {
        true => (own.iter().zip(was).zip(now))
            .map(|((o, w), n)| pick(o, w, n))
            .collect(),
        false => now.to_vec(),
    }
}

fn mixed_list<const N: usize>(a: &[[f64; N]], b: &[[f64; N]], w: f64) -> Vec<[f64; N]> {
    match a.len() == b.len() {
        true => a.iter().zip(b).map(|(p, q)| mixed(p, q, w)).collect(),
        false => a.to_vec(),
    }
}

fn mixed_curve(a: &[[[f64; 2]; 3]], b: &[[[f64; 2]; 3]], w: f64) -> Vec<[[f64; 2]; 3]> {
    match a.len() == b.len() {
        true => (a.iter().zip(b))
            .map(|(p, q)| std::array::from_fn(|i| mixed(&p[i], &q[i], w)))
            .collect(),
        false => a.to_vec(),
    }
}

fn mixed_moves(
    a: &Option<Vec<[f64; 2]>>,
    b: &Option<Vec<[f64; 2]>>,
    w: f64,
) -> Option<Vec<[f64; 2]>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(mixed_list(a, b, w)),
        _ => a.clone(),
    }
}

fn each_moves(
    own: &Option<Vec<[f64; 2]>>,
    was: &Option<Vec<[f64; 2]>>,
    now: &Option<Vec<[f64; 2]>>,
    count: usize,
) -> Option<Vec<[f64; 2]>> {
    let moves = match (own, was, now) {
        (Some(o), Some(w), Some(n)) => Some(each(o, w, n)),
        _ => pick(own, was, now),
    };
    moves.filter(|m| m.len() == count)
}

impl GradeWindow {
    pub fn blended(&self, to: &GradeWindow, w: f64) -> GradeWindow {
        let one = |a: f64, b: f64| a + (b - a) * w;
        let (scale, size): (Scale, Scale) = match self.shape {
            WindowShape::Circle { .. } => (circle_scale, circle_size),
            _ => (linear_scale, linear_size),
        };
        let shape = match (&self.shape, &to.shape) {
            (
                WindowShape::Linear { soft: a, points: p },
                WindowShape::Linear { soft: b, points: q },
            ) => WindowShape::Linear {
                soft: mixed(a, b, w),
                points: std::array::from_fn(|i| mixed(&p[i], &q[i], w)),
            },
            (WindowShape::Circle { soft: a }, WindowShape::Circle { soft: b }) => {
                WindowShape::Circle { soft: one(*a, *b) }
            }
            (
                WindowShape::Polygon {
                    points: p,
                    inside: a,
                    outside: c,
                },
                WindowShape::Polygon {
                    points: q,
                    inside: b,
                    outside: d,
                },
            ) => WindowShape::Polygon {
                points: mixed_list(p, q, w),
                inside: one(*a, *b),
                outside: one(*c, *d),
            },
            (
                WindowShape::Curve {
                    points: p,
                    soft: s,
                    inside: a,
                    outside: c,
                    inner: i,
                    outer: o,
                },
                WindowShape::Curve {
                    points: q,
                    soft: t,
                    inside: b,
                    outside: d,
                    inner: j,
                    outer: u,
                },
            ) => WindowShape::Curve {
                points: mixed_curve(p, q, w),
                soft: one(*s, *t),
                inside: one(*a, *b),
                outside: one(*c, *d),
                inner: mixed_moves(i, j, w).filter(|m| m.len() == p.len()),
                outer: mixed_moves(o, u, w).filter(|m| m.len() == p.len()),
            },
            (WindowShape::Gradient { soft: a }, WindowShape::Gradient { soft: b }) => {
                WindowShape::Gradient { soft: one(*a, *b) }
            }
            _ => self.shape.clone(),
        };
        GradeWindow {
            shape,
            on: self.on,
            name: self.name.clone(),
            invert: self.invert,
            mask: self.mask,
            size: match self.size == to.size {
                true => self.size,
                false => size(one(scale(self.size), scale(to.size))),
            },
            aspect: one(self.aspect, to.aspect),
            pan: one(self.pan, to.pan),
            tilt: one(self.tilt, to.tilt),
            rotate: one(self.rotate, to.rotate),
            opacity: one(self.opacity, to.opacity),
        }
    }

    pub fn carried(&self, was: &GradeWindow, now: &GradeWindow) -> GradeWindow {
        let one = |own: f64, was: f64, now: f64| if was == now { own } else { now };
        let shape = match (&self.shape, &was.shape, &now.shape) {
            (
                WindowShape::Linear { soft, points },
                WindowShape::Linear {
                    soft: ws,
                    points: wp,
                },
                WindowShape::Linear {
                    soft: ns,
                    points: np,
                },
            ) => WindowShape::Linear {
                soft: moved(soft, ws, ns),
                points: std::array::from_fn(|i| moved(&points[i], &wp[i], &np[i])),
            },
            (
                WindowShape::Circle { soft },
                WindowShape::Circle { soft: ws },
                WindowShape::Circle { soft: ns },
            ) => WindowShape::Circle {
                soft: one(*soft, *ws, *ns),
            },
            (
                WindowShape::Polygon {
                    points,
                    inside,
                    outside,
                },
                WindowShape::Polygon {
                    points: wp,
                    inside: wi,
                    outside: wo,
                },
                WindowShape::Polygon {
                    points: np,
                    inside: ni,
                    outside: no,
                },
            ) => WindowShape::Polygon {
                points: each(points, wp, np),
                inside: one(*inside, *wi, *ni),
                outside: one(*outside, *wo, *no),
            },
            (
                WindowShape::Curve {
                    points,
                    soft,
                    inside,
                    outside,
                    inner,
                    outer,
                },
                WindowShape::Curve {
                    points: wp,
                    soft: ws,
                    inside: wi,
                    outside: wo,
                    inner: wn,
                    outer: wu,
                },
                WindowShape::Curve {
                    points: np,
                    soft: ns,
                    inside: ni,
                    outside: no,
                    inner: nn,
                    outer: nu,
                },
            ) => {
                let points = each(points, wp, np);
                let count = points.len();
                WindowShape::Curve {
                    points,
                    soft: one(*soft, *ws, *ns),
                    inside: one(*inside, *wi, *ni),
                    outside: one(*outside, *wo, *no),
                    inner: each_moves(inner, wn, nn, count),
                    outer: each_moves(outer, wu, nu, count),
                }
            }
            (
                WindowShape::Gradient { soft },
                WindowShape::Gradient { soft: ws },
                WindowShape::Gradient { soft: ns },
            ) => WindowShape::Gradient {
                soft: one(*soft, *ws, *ns),
            },
            _ => pick(&self.shape, &was.shape, &now.shape),
        };
        GradeWindow {
            shape,
            on: pick(&self.on, &was.on, &now.on),
            name: pick(&self.name, &was.name, &now.name),
            invert: pick(&self.invert, &was.invert, &now.invert),
            mask: pick(&self.mask, &was.mask, &now.mask),
            size: one(self.size, was.size, now.size),
            aspect: one(self.aspect, was.aspect, now.aspect),
            pan: one(self.pan, was.pan, now.pan),
            tilt: one(self.tilt, was.tilt, now.tilt),
            rotate: one(self.rotate, was.rotate, now.rotate),
            opacity: one(self.opacity, was.opacity, now.opacity),
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

    fn keyed_change<T: Clone>(
        &mut self,
        frame: i64,
        part: impl Fn(&mut Self, Option<usize>) -> Option<&mut T>,
        change: impl FnOnce(&T) -> T,
        carried: fn(&T, &T, &T) -> T,
    ) -> Option<()> {
        let key = self.changed_key(frame);
        let was = part(self, key)?.clone();
        let now = change(&was);
        if let Some(other) = self.paired_key(key).and_then(|k| part(self, k)) {
            *other = carried(other, &was, &now);
        }
        *part(self, key)? = now;
        Some(())
    }

    fn values_mut(&mut self, key: Option<usize>) -> Option<&mut Corrector> {
        match (key, &mut self.tool) {
            (Some(i), GradeTool::Corrector(_)) => Some(&mut self.keys[i].values),
            (None, GradeTool::Corrector(c)) => Some(&mut **c),
            _ => None,
        }
    }

    fn windows_mut(&mut self, key: Option<usize>) -> Option<&mut Vec<GradeWindow>> {
        match key {
            _ if self.tool.is_mixer() => None,
            Some(i) => Some(&mut self.keys[i].windows),
            None => Some(&mut self.windows),
        }
    }

    pub fn window_lists_mut(&mut self) -> impl Iterator<Item = &mut Vec<GradeWindow>> {
        std::iter::once(&mut self.windows).chain(self.keys.iter_mut().map(|k| &mut k.windows))
    }

    pub fn windows_changed_at(&self, frame: i64) -> &[GradeWindow] {
        self.changed_key(frame)
            .map_or(&self.windows, |i| &self.keys[i].windows)
    }

    pub fn change_window_at(
        &mut self,
        frame: i64,
        i: usize,
        change: impl FnOnce(&GradeWindow) -> GradeWindow,
    ) -> Option<()> {
        self.keyed_change(
            frame,
            |d, k| d.windows_mut(k).and_then(|w| w.get_mut(i)),
            change,
            GradeWindow::carried,
        )
    }

    pub fn windows_at(&self, frame: f64) -> Cow<'_, [GradeWindow]> {
        match self.span(frame) {
            None => Cow::Borrowed(&self.windows),
            Some((k, Some((n, w)))) => Cow::Owned(
                (k.windows.iter().zip(&n.windows))
                    .map(|(a, b)| a.blended(b, w))
                    .collect(),
            ),
            Some((k, None)) => Cow::Borrowed(&k.windows),
        }
    }

    fn key_output_mut(&mut self, key: Option<usize>) -> Option<&mut KeyOutput> {
        match key {
            _ if !self.takes_keyframes() => None,
            Some(i) => Some(&mut self.keys[i].key),
            None => Some(&mut self.key),
        }
    }

    pub fn key_output_changed_at(&self, frame: i64) -> Option<KeyOutput> {
        let key = self
            .changed_key(frame)
            .map_or(self.key, |i| self.keys[i].key);
        self.takes_keyframes().then_some(key)
    }

    pub fn change_key_output_at(
        &mut self,
        frame: i64,
        change: impl FnOnce(KeyOutput) -> KeyOutput,
    ) -> Option<()> {
        let change = |k: &KeyOutput| change(*k);
        self.keyed_change(frame, Self::key_output_mut, change, KeyOutput::carried)
    }

    fn key_input_mut(&mut self, key: Option<usize>) -> Option<&mut KeyInput> {
        match key {
            _ if self.tool.is_mixer() => None,
            Some(i) => Some(&mut self.keys[i].key_input),
            None => Some(&mut self.key_input),
        }
    }

    pub fn key_input_changed_at(&self, frame: i64) -> Option<KeyInput> {
        let input = (self.changed_key(frame)).map_or(self.key_input, |i| self.keys[i].key_input);
        self.own().map(|_| input)
    }

    pub fn change_key_input_at(
        &mut self,
        frame: i64,
        change: impl FnOnce(KeyInput) -> KeyInput,
    ) -> Option<()> {
        let change = |k: &KeyInput| change(*k);
        self.keyed_change(frame, Self::key_input_mut, change, KeyInput::carried)
    }

    pub fn key_input_at(&self, frame: f64) -> KeyInput {
        match self.span(frame) {
            None => self.key_input,
            Some((k, Some((n, w)))) => k.key_input.blended(&n.key_input, w),
            Some((k, None)) => k.key_input,
        }
    }

    fn mix_mut(&mut self, key: Option<usize>) -> Option<&mut Vec<KeyMix>> {
        match (key, &mut self.tool) {
            (Some(i), GradeTool::KeyMixer { .. }) => Some(&mut self.keys[i].mix),
            (None, GradeTool::KeyMixer { mix }) => Some(mix),
            _ => None,
        }
    }

    pub fn mix_lists_mut(&mut self) -> impl Iterator<Item = &mut Vec<KeyMix>> {
        let own = match &mut self.tool {
            GradeTool::KeyMixer { mix } => Some(mix),
            _ => None,
        };
        own.into_iter()
            .chain(self.keys.iter_mut().map(|k| &mut k.mix))
    }

    pub fn mix_changed_at(&self, frame: i64) -> &[KeyMix] {
        match (self.changed_key(frame), &self.tool) {
            (Some(i), GradeTool::KeyMixer { .. }) => &self.keys[i].mix,
            (None, GradeTool::KeyMixer { mix }) => mix,
            _ => &[],
        }
    }

    pub fn change_mix_at(
        &mut self,
        frame: i64,
        i: usize,
        change: impl FnOnce(KeyMix) -> KeyMix,
    ) -> Option<()> {
        let change = |m: &KeyMix| change(*m);
        self.keyed_change(
            frame,
            |d, k| d.mix_mut(k).and_then(|m| m.get_mut(i)),
            change,
            KeyMix::carried,
        )
    }

    pub fn mix_at(&self, frame: f64) -> Option<Cow<'_, [KeyMix]>> {
        let GradeTool::KeyMixer { mix } = &self.tool else {
            return None;
        };
        Some(match self.span(frame) {
            None => Cow::Borrowed(mix),
            Some((k, Some((n, w)))) => Cow::Owned(
                (k.mix.iter().zip(&n.mix))
                    .map(|(a, b)| a.blended(b, w))
                    .collect(),
            ),
            Some((k, None)) => Cow::Borrowed(&k.mix),
        })
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
        self.keyed_change(frame, Self::values_mut, change, Corrector::carried)
    }

    fn span(&self, frame: f64) -> Option<(&GradeKey, Option<(&GradeKey, f64)>)> {
        let i = self.keys.partition_point(|k| k.frame as f64 <= frame);
        let k = &self.keys[i.checked_sub(1)?];
        let next = self.keys.get(i).filter(|_| !k.is_static()).map(|n| {
            let u = (frame - k.frame as f64) / (n.frame - k.frame) as f64;
            (n, Dissolve::of(k.start, k.end).at(u))
        });
        Some((k, next.filter(|(_, w)| *w > 0.0)))
    }

    pub fn corrector_at(&self, frame: f64) -> Option<Cow<'_, Corrector>> {
        let own = self.own()?;
        Some(match self.span(frame) {
            None => Cow::Borrowed(own),
            Some((k, Some((n, w)))) => Cow::Owned(k.values.blended(&n.values, w)),
            Some((k, None)) => Cow::Borrowed(&k.values),
        })
    }

    pub fn key_output_at(&self, frame: f64) -> KeyOutput {
        match self.span(frame) {
            None => self.key,
            Some((k, Some((n, w)))) => k.key.blended(&n.key, w),
            Some((k, None)) => k.key,
        }
    }

    pub fn key_here(&self, frame: i64, kind: GradeKeyKind, profile: DynamicsProfile) -> GradeKey {
        let at = frame as f64;
        GradeKey {
            frame,
            kind,
            start: profile.start,
            end: profile.end,
            values: self
                .corrector_at(at)
                .map(Cow::into_owned)
                .unwrap_or_default(),
            key: self.key_output_at(at),
            key_input: self.key_input_at(at),
            mix: self.mix_at(at).map_or_else(Vec::new, Cow::into_owned),
            windows: self.windows_at(at).into_owned(),
            edl: false,
        }
    }

    pub fn auto_keyed(&mut self, frame: i64, profile: DynamicsProfile) {
        if self.auto_key && self.takes_keyframes() && self.key_on(frame).is_none() {
            let key = self.key_here(frame, GradeKeyKind::Dynamic, profile);
            self.put_key(key);
        }
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
            if let (GradeTool::KeyMixer { mix }, Some(now)) = (&mut d.tool, from.mix_at(frame)) {
                *mix = now.into_owned();
            }
            d.key = from.key_output_at(frame);
            d.key_input = from.key_input_at(frame);
            d.windows = from.windows_at(frame).into_owned();
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
