use crate::model::{
    Clip, ClipAudio, Crop, Ease, Effect, Handle, Keyframe, Picture, PositionHandles, Title,
    TitleKind,
};
use crate::otio::effects::rounded;
use crate::validate::GAIN;

const FLAT: Handle = Handle {
    frames: 0.0,
    value: 0.0,
};
const PATH_STEPS: usize = 256;
const PATH_EASE: f64 = 0.125;

pub fn value_at(keys: &[Keyframe], frame: f64) -> Option<f64> {
    let (first, last) = (keys.first()?, keys.last()?);
    if frame <= first.frame as f64 {
        return Some(first.value);
    }
    if frame >= last.frame as f64 {
        return Some(last.value);
    }
    let i = keys.partition_point(|k| k.frame as f64 <= frame);
    Some(between(&keys[i - 1], &keys[i], frame))
}

#[derive(Clone, Copy)]
enum ValueOffset {
    Handle,
    Zero,
}

fn between(a: &Keyframe, b: &Keyframe, frame: f64) -> f64 {
    between_values(a, a.value, b, b.value, frame, ValueOffset::Handle)
}

fn between_values(
    a: &Keyframe,
    av: f64,
    b: &Keyframe,
    bv: f64,
    frame: f64,
    offset: ValueOffset,
) -> f64 {
    let len = (b.frame - a.frame) as f64;
    let x = frame - a.frame as f64;
    if a.out_handle.is_none() && b.in_handle.is_none() {
        return av + (bv - av) * x / len;
    }
    let (out, into) = (a.out_handle.unwrap_or(FLAT), b.in_handle.unwrap_or(FLAT));
    let xs = reach(a, b);
    let u = solve(|u| cubic(xs, u), x);
    let (ov, iv) = match offset {
        ValueOffset::Handle => (out.value, into.value),
        ValueOffset::Zero => (0.0, 0.0),
    };
    cubic([av, av + ov, bv + iv, bv], u)
}

fn reach(a: &Keyframe, b: &Keyframe) -> [f64; 4] {
    let len = (b.frame - a.frame) as f64;
    let (out, into) = (a.out_handle.unwrap_or(FLAT), b.in_handle.unwrap_or(FLAT));
    [
        0.0,
        out.frames.clamp(0.0, len),
        len + into.frames.clamp(-len, 0.0),
        len,
    ]
}

pub fn split(keys: &mut Vec<Keyframe>, frame: i64) -> usize {
    let i = keys.partition_point(|k| k.frame < frame);
    if keys.get(i).is_some_and(|k| k.frame == frame) {
        return i;
    }
    let mut key = Keyframe {
        frame,
        value: value_at(keys, frame as f64).unwrap_or(0.0),
        in_handle: None,
        out_handle: None,
    };
    if i > 0 && i < keys.len() && (keys[i - 1].out_handle.is_some() || keys[i].in_handle.is_some())
    {
        let (a, b) = (&keys[i - 1], &keys[i]);
        let xs = reach(a, b);
        let (ov, iv) = (
            a.out_handle.unwrap_or(FLAT).value,
            b.in_handle.unwrap_or(FLAT).value,
        );
        let ys = [a.value, a.value + ov, b.value + iv, b.value];
        let u = solve(|u| cubic(xs, u), (frame - a.frame) as f64);
        let lerp = |p: [f64; 2], q: [f64; 2]| [p[0] + (q[0] - p[0]) * u, p[1] + (q[1] - p[1]) * u];
        let p: [[f64; 2]; 4] = std::array::from_fn(|j| [xs[j], ys[j]]);
        let (l0, l1, l2) = (lerp(p[0], p[1]), lerp(p[1], p[2]), lerp(p[2], p[3]));
        let (m0, m1) = (lerp(l0, l1), lerp(l1, l2));
        let mid = lerp(m0, m1);
        let handle = |to: [f64; 2], from: [f64; 2]| Handle {
            frames: to[0] - from[0],
            value: to[1] - from[1],
        };
        key.value = mid[1];
        key.in_handle = Some(handle(m0, mid));
        key.out_handle = Some(handle(m1, mid));
        if keys[i - 1].out_handle.is_some() {
            keys[i - 1].out_handle = Some(handle(l0, p[0]));
        }
        if keys[i].in_handle.is_some() {
            keys[i].in_handle = Some(handle(l2, p[3]));
        }
    }
    keys.insert(i, key);
    i
}

pub fn value_mapped(keys: &[Keyframe], frame: f64, value: impl Fn(usize) -> f64) -> Option<f64> {
    mapped(keys, frame, ValueOffset::Handle, value)
}

pub fn value_paired(keys: &[Keyframe], frame: f64, value: impl Fn(usize) -> f64) -> Option<f64> {
    mapped(keys, frame, ValueOffset::Zero, value)
}

fn mapped(
    keys: &[Keyframe],
    frame: f64,
    offset: ValueOffset,
    value: impl Fn(usize) -> f64,
) -> Option<f64> {
    let (first, last) = (keys.first()?, keys.last()?);
    if frame <= first.frame as f64 {
        return Some(value(0));
    }
    if frame >= last.frame as f64 {
        return Some(value(keys.len() - 1));
    }
    let i = keys.partition_point(|k| k.frame as f64 <= frame);
    Some(between_values(
        &keys[i - 1],
        value(i - 1),
        &keys[i],
        value(i),
        frame,
        offset,
    ))
}

fn cubic([p0, p1, p2, p3]: [f64; 4], u: f64) -> f64 {
    let v = 1.0 - u;
    v * v * v * p0 + 3.0 * v * v * u * p1 + 3.0 * v * u * u * p2 + u * u * u * p3
}

fn solve(x: impl Fn(f64) -> f64, at: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        if x(mid) < at {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

fn along_path(path: [[f64; 2]; 4], share: f64) -> [f64; 2] {
    let point = |u: f64| [cubic(path.map(|p| p[0]), u), cubic(path.map(|p| p[1]), u)];
    let mut lengths = [0.0; PATH_STEPS + 1];
    let mut last = path[0];
    for i in 1..=PATH_STEPS {
        let at = point(i as f64 / PATH_STEPS as f64);
        lengths[i] = lengths[i - 1] + (at[0] - last[0]).hypot(at[1] - last[1]);
        last = at;
    }
    let want = share * lengths[PATH_STEPS];
    let i = lengths.partition_point(|l| *l < want).clamp(1, PATH_STEPS);
    let step = lengths[i] - lengths[i - 1];
    let part = if step > 0.0 {
        (want - lengths[i - 1]) / step
    } else {
        0.0
    };
    point((i as f64 - 1.0 + part) / PATH_STEPS as f64)
}

pub fn handle_point(a: &Keyframe, b: &Keyframe, out: bool) -> Option<(f64, f64)> {
    let len = (b.frame - a.frame) as f64;
    if out {
        let h = a.out_handle?;
        Some((a.frame as f64 + h.frames.clamp(0.0, len), a.value + h.value))
    } else {
        let h = b.in_handle?;
        Some((
            b.frame as f64 + h.frames.clamp(-len, 0.0),
            b.value + h.value,
        ))
    }
}

fn bend_share(u: f64) -> f64 {
    if u <= 1.0 / 6.0 {
        0.0
    } else if u <= 0.5 {
        ((6.0 * u - 1.0) / 2.0).powi(3) / 2.0
    } else if u <= 5.0 / 6.0 {
        (1.0 - ((6.0 * (1.0 - u) - 1.0) / 2.0).powi(3)) / 2.0 + 0.5
    } else {
        1.0
    }
}

pub fn bent(a: &Keyframe, b: &Keyframe, frame: f64, by: f64) -> (Option<Handle>, Option<Handle>) {
    let len = (b.frame - a.frame) as f64;
    let straight = a.out_handle.is_none() && b.in_handle.is_none();
    let third = |sign: f64| Handle {
        frames: sign * len / 3.0,
        value: sign * (b.value - a.value) / 3.0,
    };
    let (start, end) = if straight {
        (third(1.0), third(-1.0))
    } else {
        (FLAT, FLAT)
    };
    let (out, into) = (a.out_handle.unwrap_or(start), b.in_handle.unwrap_or(end));
    let has_out = straight || a.out_handle.is_some();
    let has_in = straight || b.in_handle.is_some();
    let xs = [
        0.0,
        out.frames.clamp(0.0, len),
        len + into.frames.clamp(-len, 0.0),
        len,
    ];
    let low = if has_out { 0.1 } else { 0.3 };
    let high = if has_in { 0.9 } else { 0.7 };
    let u = solve(|u| cubic(xs, u), frame - a.frame as f64).clamp(low, high);
    let v = 1.0 - u;
    let share = match (has_out, has_in) {
        (true, true) => bend_share(u),
        (true, false) => 0.0,
        _ => 1.0,
    };
    let raised = |h: Handle, d: f64| Handle {
        frames: h.frames,
        value: ((h.value + d) * 1e6).round() / 1e6,
    };
    (
        has_out.then(|| raised(out, (1.0 - share) * by / (3.0 * u * v * v))),
        has_in.then(|| raised(into, share * by / (3.0 * u * u * v))),
    )
}

fn sides(ease: Ease) -> (bool, bool) {
    match ease {
        Ease::Linear => (false, false),
        Ease::EaseIn => (true, false),
        Ease::EaseOut => (false, true),
        Ease::EaseInOut => (true, true),
    }
}

fn reaches(count: usize, i: usize, ease: Ease) -> (bool, bool) {
    let (wants_in, wants_out) = sides(ease);
    (wants_in && i > 0, wants_out && i + 1 < count)
}

fn handles(keys: &[Keyframe], i: usize, ease: Ease) -> (Option<Handle>, Option<Handle>) {
    let (before, after) = reaches(keys.len(), i, ease);
    let gap = |j: usize| (keys[i].frame - keys[j].frame).abs() as f64 / 3.0;
    let flat = |frames: f64| Handle { frames, value: 0.0 };
    (
        before.then(|| flat(-gap(i - 1))),
        after.then(|| flat(gap(i + 1))),
    )
}

pub fn ease(keys: &mut [Keyframe], i: usize, ease: Ease) {
    (keys[i].in_handle, keys[i].out_handle) = handles(keys, i, ease);
}

pub fn ease_of(keys: &[Keyframe], i: usize) -> Option<Ease> {
    let near = |a: Option<Handle>, b: Option<Handle>| match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            (a.frames - b.frames).abs() < 1e-6 && (a.value - b.value).abs() < 1e-9
        }
        _ => false,
    };
    Ease::ALL.into_iter().find(|&e| {
        let (a, b) = handles(keys, i, e);
        near(a, keys[i].in_handle) && near(b, keys[i].out_handle)
    })
}

pub fn eases(keys: &[Keyframe], i: usize) -> Vec<Ease> {
    Ease::ALL
        .into_iter()
        .filter(|e| reaches(keys.len(), i, *e) == sides(*e))
        .collect()
}

pub type Marked = Vec<(Keyframe, Option<Ease>)>;

pub fn reshape<R>(keys: &mut Vec<Keyframe>, change: impl FnOnce(&mut Marked) -> R) -> R {
    let mut marked: Marked = (0..keys.len())
        .map(|i| (keys[i], ease_of(keys, i)))
        .collect();
    let out = change(&mut marked);
    marked.sort_by_key(|(k, _)| k.frame);
    marked.dedup_by_key(|(k, _)| k.frame);
    *keys = marked.iter().map(|(k, _)| *k).collect();
    for (i, (_, e)) in marked.iter().enumerate() {
        if let Some(e) = e.filter(|e| !e.is_linear()) {
            ease(keys, i, e);
        }
    }
    out
}

pub fn retimed(keys: &[Keyframe], old: &Clip, new: &Clip) -> Vec<Keyframe> {
    let mut out = keys.to_vec();
    reshape(&mut out, |marked| {
        for (k, _) in marked.iter_mut() {
            k.frame = new.key_at(old.key_time(k.frame)).round() as i64;
        }
    });
    out
}

fn scale_handles(k: &mut Keyframe, ratio: f64) {
    for h in [&mut k.in_handle, &mut k.out_handle].into_iter().flatten() {
        h.frames *= ratio;
    }
    if ratio < 0.0 {
        std::mem::swap(&mut k.in_handle, &mut k.out_handle);
    }
}

pub fn scaled(keys: &[Keyframe], frame: impl Fn(i64) -> i64, ratio: f64) -> Vec<Keyframe> {
    let mut out: Vec<Keyframe> = keys
        .iter()
        .map(|k| {
            let mut k = Keyframe {
                frame: frame(k.frame),
                ..*k
            };
            scale_handles(&mut k, ratio);
            k
        })
        .collect();
    out.sort_by_key(|k| k.frame);
    out.dedup_by_key(|k| k.frame);
    out
}

pub fn checked(value: f64) -> crate::error::Result<f64> {
    if GAIN.contains(&value) {
        Ok(value)
    } else {
        Err(crate::error::Error::invalid(format!(
            "gain {value} dB outside -100..30 dB"
        )))
    }
}

pub fn checked_handle(h: Handle, before: bool) -> crate::error::Result<Handle> {
    let side = if before {
        h.frames <= 0.0
    } else {
        h.frames >= 0.0
    };
    if !h.frames.is_finite() || !h.value.is_finite() || !side {
        return Err(crate::error::Error::invalid(format!(
            "an {} handle needs finite numbers and frames {} 0",
            if before { "In" } else { "Out" },
            if before { "at most" } else { "at least" }
        )));
    }
    Ok(h)
}

pub fn problem(keys: &[Keyframe]) -> Option<String> {
    if keys.windows(2).any(|w| w[0].frame >= w[1].frame) {
        return Some("volume keyframes out of order or on one frame".into());
    }
    keys.iter().find_map(|k| {
        let handles = [(k.in_handle, true), (k.out_handle, false)];
        checked(k.value)
            .err()
            .or_else(|| {
                handles
                    .into_iter()
                    .find_map(|(h, before)| checked_handle(h?, before).err())
            })
            .map(|e| format!("volume keyframe at source frame {}: {}", k.frame, e.message))
    })
}

impl ClipAudio {
    pub fn is_keyed(&self) -> bool {
        !self.gain_keys.is_empty()
    }

    pub fn db_at(&self, frame: f64) -> f64 {
        value_at(&self.gain_keys, frame)
            .map_or(self.gain_db, |v| v.clamp(*GAIN.start(), *GAIN.end()))
    }

    pub fn top_db(&self) -> f64 {
        self.gain_keys
            .iter()
            .map(|k| k.value)
            .reduce(f64::max)
            .unwrap_or(self.gain_db)
    }

    pub fn raise(&mut self, by: f64) {
        let moved = |v: f64| (v + by).clamp(*GAIN.start(), *GAIN.end());
        if self.is_keyed() {
            for k in &mut self.gain_keys {
                k.value = moved(k.value);
            }
        } else {
            self.gain_db = moved(self.gain_db);
        }
    }
}

impl Clip {
    pub fn key_at(&self, t: f64) -> f64 {
        self.content_at(self.start) as f64 + t - self.start as f64
    }

    pub fn time_of(&self, frame: f64) -> f64 {
        self.start as f64 + frame - self.content_at(self.start) as f64
    }

    pub fn key_time(&self, frame: i64) -> f64 {
        self.time_of(frame as f64)
    }

    pub fn gain_at(&self, t: f64) -> Option<f64> {
        self.audio.as_ref().map(|a| a.db_at(self.key_at(t)))
    }

    pub fn sounds_at(&self, t: i64) -> bool {
        self.audio_range().is_some_and(|(a, b)| a <= t && t < b)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Gain<'a> {
    pub db: f64,
    pub keys: &'a [Keyframe],
    pub origin: f64,
    pub fps: f64,
}

impl<'a> Gain<'a> {
    pub fn flat(db: f64) -> Self {
        Self {
            db,
            keys: &[],
            origin: 0.0,
            fps: 1.0,
        }
    }

    pub fn of(audio: &'a ClipAudio, extra_db: f64, origin: f64, fps: f64) -> Self {
        let db = if audio.is_keyed() {
            extra_db
        } else {
            audio.gain_db + extra_db
        };
        Self {
            db,
            keys: &audio.gain_keys,
            origin,
            fps,
        }
    }

    pub fn retimed(self, pace: &crate::render::Pace, source_in: i64, key: i64) -> Self {
        let v = pace.speed();
        if v == 0.0 {
            return self;
        }
        let at = source_in as f64 + pace.sound(0.0);
        Self {
            origin: self.origin + (at - key as f64 * v) / self.fps,
            fps: self.fps / v,
            ..self
        }
    }

    pub fn is_flat(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn at(&self, time: f64) -> f64 {
        match value_at(self.keys, (time - self.origin) * self.fps) {
            Some(v) => self.db + v.clamp(*GAIN.start(), *GAIN.end()),
            None => self.db,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PictureValue {
    Position,
    Zoom,
    Rotation,
    Anchor,
    Pitch,
    Yaw,
    CropLeft,
    CropRight,
    CropTop,
    CropBottom,
    CropSoftness,
    Opacity,
}

impl PictureValue {
    pub const ALL: [PictureValue; 12] = [
        PictureValue::Position,
        PictureValue::Zoom,
        PictureValue::Rotation,
        PictureValue::Anchor,
        PictureValue::Pitch,
        PictureValue::Yaw,
        PictureValue::CropLeft,
        PictureValue::CropRight,
        PictureValue::CropTop,
        PictureValue::CropBottom,
        PictureValue::CropSoftness,
        PictureValue::Opacity,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PictureValue::Position => "position",
            PictureValue::Zoom => "zoom",
            PictureValue::Rotation => "rotation",
            PictureValue::Anchor => "anchor",
            PictureValue::Pitch => "pitch",
            PictureValue::Yaw => "yaw",
            PictureValue::CropLeft => "crop_left",
            PictureValue::CropRight => "crop_right",
            PictureValue::CropTop => "crop_top",
            PictureValue::CropBottom => "crop_bottom",
            PictureValue::CropSoftness => "crop_softness",
            PictureValue::Opacity => "opacity",
        }
    }

    pub fn pair(self) -> bool {
        matches!(
            self,
            PictureValue::Position | PictureValue::Zoom | PictureValue::Anchor
        )
    }

    pub fn of_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == name)
    }
}

fn pair_at(keys: &[Keyframe], y: &[f64], frame: f64) -> Option<[f64; 2]> {
    if keys.is_empty() || keys.len() != y.len() {
        return None;
    }
    Some([value_at(keys, frame)?, value_paired(keys, frame, |i| y[i])?])
}

fn one_at(keys: &[Keyframe], frame: f64) -> Option<f64> {
    (!keys.is_empty()).then(|| value_at(keys, frame)).flatten()
}

fn parts(p: &Picture) -> [(&Vec<Keyframe>, Option<&Vec<f64>>); 12] {
    [
        (&p.position_keys, Some(&p.position_y)),
        (&p.zoom_keys, Some(&p.zoom_y)),
        (&p.rotation_keys, None),
        (&p.anchor_keys, Some(&p.anchor_y)),
        (&p.pitch_keys, None),
        (&p.yaw_keys, None),
        (&p.crop_left_keys, None),
        (&p.crop_right_keys, None),
        (&p.crop_top_keys, None),
        (&p.crop_bottom_keys, None),
        (&p.crop_softness_keys, None),
        (&p.opacity_keys, None),
    ]
}

fn parts_mut(p: &mut Picture, v: PictureValue) -> (&mut Vec<Keyframe>, Option<&mut Vec<f64>>) {
    match v {
        PictureValue::Position => (&mut p.position_keys, Some(&mut p.position_y)),
        PictureValue::Zoom => (&mut p.zoom_keys, Some(&mut p.zoom_y)),
        PictureValue::Rotation => (&mut p.rotation_keys, None),
        PictureValue::Anchor => (&mut p.anchor_keys, Some(&mut p.anchor_y)),
        PictureValue::Pitch => (&mut p.pitch_keys, None),
        PictureValue::Yaw => (&mut p.yaw_keys, None),
        PictureValue::CropLeft => (&mut p.crop_left_keys, None),
        PictureValue::CropRight => (&mut p.crop_right_keys, None),
        PictureValue::CropTop => (&mut p.crop_top_keys, None),
        PictureValue::CropBottom => (&mut p.crop_bottom_keys, None),
        PictureValue::CropSoftness => (&mut p.crop_softness_keys, None),
        PictureValue::Opacity => (&mut p.opacity_keys, None),
    }
}

pub fn plain_channel(p: &Picture, v: PictureValue) -> [f64; 2] {
    plain_of(p, v)
}

pub fn write_channel(p: &mut Picture, v: PictureValue, value: [f64; 2]) {
    write_plain(p, v, value);
}

fn plain_of(p: &Picture, v: PictureValue) -> [f64; 2] {
    match v {
        PictureValue::Position => p.place(),
        PictureValue::Zoom => p.scale(),
        PictureValue::Rotation => [p.turn(), 0.0],
        PictureValue::Anchor => p.pivot(),
        PictureValue::Pitch => [p.pitch(), 0.0],
        PictureValue::Yaw => [p.yaw(), 0.0],
        PictureValue::CropLeft => [p.crop.left, 0.0],
        PictureValue::CropRight => [p.crop.right, 0.0],
        PictureValue::CropTop => [p.crop.top, 0.0],
        PictureValue::CropBottom => [p.crop.bottom, 0.0],
        PictureValue::CropSoftness => [p.softness(), 0.0],
        PictureValue::Opacity => [p.opaque(), 0.0],
    }
}

fn write_plain(p: &mut Picture, v: PictureValue, value: [f64; 2]) {
    match v {
        PictureValue::Position => p.position = Some(value).filter(|at| *at != Picture::MIDDLE),
        PictureValue::Zoom => p.zoom = Some(value).filter(|z| *z != [1.0, 1.0]),
        PictureValue::Rotation => p.rotation = Some(value[0]).filter(|r| *r != 0.0),
        PictureValue::Anchor => p.anchor = Some(value).filter(|at| *at != Picture::MIDDLE),
        PictureValue::Pitch => p.pitch = Some(value[0]).filter(|v| *v != 0.0),
        PictureValue::Yaw => p.yaw = Some(value[0]).filter(|v| *v != 0.0),
        PictureValue::CropLeft => p.crop.left = value[0],
        PictureValue::CropRight => p.crop.right = value[0],
        PictureValue::CropTop => p.crop.top = value[0],
        PictureValue::CropBottom => p.crop.bottom = value[0],
        PictureValue::CropSoftness => p.crop_softness = Some(value[0]).filter(|s| *s != 0.0),
        PictureValue::Opacity => p.opacity = Some(value[0]).filter(|o| *o != 1.0),
    }
}

impl Picture {
    pub fn is_keyed(&self) -> bool {
        PictureValue::ALL.into_iter().any(|v| self.has_keys(v))
            || self.effects.iter().any(Effect::has_keys)
    }

    pub fn has_keys(&self, v: PictureValue) -> bool {
        !parts(self)[v as usize].0.is_empty()
    }

    pub fn keyed_names(&self) -> Vec<&'static str> {
        PictureValue::ALL
            .into_iter()
            .filter(|v| self.has_keys(*v))
            .map(PictureValue::name)
            .collect()
    }

    pub fn keys_of(&self, v: PictureValue) -> &[Keyframe] {
        parts(self)[v as usize].0
    }

    pub fn channel_at(&self, v: PictureValue, frame: f64) -> [f64; 2] {
        let (keys, y) = parts(self)[v as usize];
        if let Some(y) = y {
            pair_at(keys, y, frame).unwrap_or_else(|| plain_of(self, v))
        } else {
            one_at(keys, frame).map_or_else(|| plain_of(self, v), |n| [n, 0.0])
        }
    }

    pub fn place_at(&self, frame: f64) -> [f64; 2] {
        self.channel_at(PictureValue::Position, frame)
    }

    pub fn scale_at(&self, frame: f64) -> [f64; 2] {
        self.channel_at(PictureValue::Zoom, frame)
    }

    pub fn turn_at(&self, frame: f64) -> f64 {
        self.channel_at(PictureValue::Rotation, frame)[0]
    }

    pub fn pivot_at(&self, frame: f64) -> [f64; 2] {
        self.channel_at(PictureValue::Anchor, frame)
    }

    pub fn crop_at(&self, frame: f64) -> Crop {
        Crop {
            left: self.channel_at(PictureValue::CropLeft, frame)[0],
            right: self.channel_at(PictureValue::CropRight, frame)[0],
            top: self.channel_at(PictureValue::CropTop, frame)[0],
            bottom: self.channel_at(PictureValue::CropBottom, frame)[0],
        }
    }

    pub fn sampled(&self, frame: f64) -> Picture {
        let mut p = self.clone();
        for v in PictureValue::ALL {
            if self.has_keys(v) {
                write_plain(&mut p, v, self.channel_at(v, frame));
            }
        }
        p.effects = self.effects.iter().map(|e| e.sampled(frame)).collect();
        p.clear_keys();
        p
    }

    pub fn largest_zoom(&self) -> f64 {
        if self.zoom_keys.is_empty() {
            let [x, y] = self.scale();
            return x.max(y);
        }
        self.zoom_keys
            .iter()
            .zip(&self.zoom_y)
            .fold(0.0, |m, (k, y)| m.max(k.value).max(*y))
    }

    pub fn clear_keys(&mut self) {
        for v in PictureValue::ALL {
            self.clear_channel(v);
        }
        for e in &mut self.effects {
            e.keys.clear();
        }
    }

    pub fn clear_channel(&mut self, v: PictureValue) {
        let (keys, y) = parts_mut(self, v);
        keys.clear();
        if let Some(y) = y {
            y.clear();
        }
    }

    pub fn clear_transform_keys(&mut self) {
        for v in [
            PictureValue::Position,
            PictureValue::Zoom,
            PictureValue::Rotation,
            PictureValue::Anchor,
            PictureValue::Pitch,
            PictureValue::Yaw,
        ] {
            self.clear_channel(v);
        }
    }

    pub fn clear_crop_keys(&mut self) {
        for v in [
            PictureValue::CropLeft,
            PictureValue::CropRight,
            PictureValue::CropTop,
            PictureValue::CropBottom,
            PictureValue::CropSoftness,
        ] {
            self.clear_channel(v);
        }
    }

    pub fn put_channel(&mut self, v: PictureValue, frame: i64, value: [f64; 2]) {
        let (keys, y) = parts_mut(self, v);
        if let Some(y) = y {
            put_pair(keys, y, frame, value);
        } else {
            reshape(keys, |marked| {
                match marked.iter_mut().find(|(k, _)| k.frame == frame) {
                    Some((k, _)) => k.value = value[0],
                    None => marked.push((key_at(frame, value[0]), None)),
                }
            });
        }
    }

    pub fn adopt(&self, frame: f64) -> Picture {
        let mut p = self.clone();
        for v in PictureValue::ALL {
            if self.has_keys(v) {
                write_plain(&mut p, v, self.channel_at(v, frame));
            }
        }
        p.effects = self.effects.iter().map(|e| e.adopted(frame)).collect();
        p
    }

    pub fn preview(&self, baseline: &Picture, edited: &Picture, frame: i64) -> Picture {
        let mut p = self.clone();
        for v in PictureValue::ALL {
            let next = plain_of(edited, v);
            if next != plain_of(baseline, v) {
                if p.has_keys(v) {
                    p.put_channel(v, frame, next);
                } else {
                    write_plain(&mut p, v, next);
                }
            }
        }
        if edited.effects != baseline.effects {
            p.effects = previewed_effects(&self.effects, &baseline.effects, &edited.effects, frame);
        }
        p
    }

    pub fn scale_times(&mut self, conv: &impl Fn(i64) -> i64, ratio: f64) {
        for v in PictureValue::ALL {
            scale_channel(self, v, &conv, ratio);
        }
        for e in &mut self.effects {
            e.scale_key_times(conv, ratio);
        }
    }

    pub fn retimed_to(&mut self, old: &Clip, new: &Clip) {
        for v in PictureValue::ALL {
            retime_channel(self, v, old, new);
        }
        for e in &mut self.effects {
            e.retime_keys(old, new);
        }
    }

    pub fn retarget(&mut self, map: impl Fn(i64) -> i64) {
        for v in PictureValue::ALL {
            retarget_channel(self, v, &map);
        }
        for e in &mut self.effects {
            e.retarget_keys(&map);
        }
    }

    pub fn key_value(&self, v: PictureValue, frame: i64) -> Option<[f64; 2]> {
        let (keys, y) = parts(self)[v as usize];
        let i = keys.iter().position(|k| k.frame == frame)?;
        Some([
            keys[i].value,
            y.and_then(|y| y.get(i).copied()).unwrap_or(0.0),
        ])
    }

    pub fn set_handle(&mut self, v: PictureValue, frame: i64, into: bool, handle: Option<Handle>) {
        let (keys, _) = parts_mut(self, v);
        reshape(keys, |marked| {
            if let Some(entry) = marked.iter_mut().find(|(k, _)| k.frame == frame) {
                if into {
                    (entry.0.in_handle, entry.1) = (handle, None);
                } else {
                    (entry.0.out_handle, entry.1) = (handle, None);
                }
            }
        });
    }

    pub fn mark_ease(&mut self, v: PictureValue, frame: i64, ease: Ease) {
        ease_mark(parts_mut(self, v).0, frame, ease);
    }

    pub fn move_chosen(
        &mut self,
        v: PictureValue,
        chosen: &std::collections::HashSet<i64>,
        moved: impl Fn(i64) -> i64,
    ) {
        let y_was = parts(self)[v as usize].1.cloned().unwrap_or_default();
        let (keys, y) = parts_mut(self, v);
        let mut rows: Vec<(Keyframe, f64)> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let frame = if chosen.contains(&k.frame) {
                    moved(k.frame)
                } else {
                    k.frame
                };
                (
                    Keyframe { frame, ..*k },
                    y_was.get(i).copied().unwrap_or(0.0),
                )
            })
            .collect();
        rows.sort_by_key(|(k, _)| k.frame);
        rows.dedup_by_key(|(k, _)| k.frame);
        *keys = rows.iter().map(|(k, _)| *k).collect();
        if let Some(y) = y {
            *y = rows.into_iter().map(|(_, n)| n).collect();
        }
    }

    pub fn drop_chosen(&mut self, v: PictureValue, chosen: &std::collections::HashSet<i64>) {
        let y_was = parts(self)[v as usize].1.cloned().unwrap_or_default();
        let (keys, y) = parts_mut(self, v);
        let rows: Vec<(Keyframe, f64)> = keys
            .iter()
            .enumerate()
            .filter(|(_, k)| !chosen.contains(&k.frame))
            .map(|(i, k)| (*k, y_was.get(i).copied().unwrap_or(0.0)))
            .collect();
        *keys = rows.iter().map(|(k, _)| *k).collect();
        if let Some(y) = y {
            *y = rows.into_iter().map(|(_, n)| n).collect();
        }
    }
}

impl Title {
    pub fn takes_keys(&self, v: PictureValue) -> bool {
        match v {
            PictureValue::Position | PictureValue::Opacity => true,
            PictureValue::Zoom | PictureValue::Rotation => self.kind != TitleKind::Scroll,
            _ => false,
        }
    }

    pub fn keys_of(&self, v: PictureValue) -> &[Keyframe] {
        match v {
            PictureValue::Position => &self.position_keys,
            PictureValue::Zoom => &self.zoom_keys,
            PictureValue::Rotation => &self.rotation_keys,
            PictureValue::Opacity => &self.opacity_keys,
            _ => &[],
        }
    }

    pub fn is_keyed(&self) -> bool {
        let upper = self.upper.as_ref().is_some_and(|u| {
            !u.position_keys.is_empty() || !u.zoom_keys.is_empty() || !u.rotation_keys.is_empty()
        });
        upper
            || PictureValue::ALL
                .into_iter()
                .any(|v| !self.keys_of(v).is_empty())
    }

    pub fn handles_at(&self, frame: i64) -> Option<&PositionHandles> {
        self.position_handles.iter().find(|h| h.frame == frame)
    }

    pub fn place_at(&self, frame: f64) -> [f64; 2] {
        self.on_path(frame)
            .unwrap_or_else(|| self.as_picture().place_at(frame))
    }

    fn on_path(&self, frame: f64) -> Option<[f64; 2]> {
        let (keys, y) = (&self.position_keys, &self.position_y);
        if keys.len() != y.len() {
            return None;
        }
        let i = keys.partition_point(|k| k.frame as f64 <= frame);
        let (a, b) = (keys.get(i.checked_sub(1)?)?, keys.get(i)?);
        let out = self.handles_at(a.frame).and_then(|h| h.out_handle);
        let into = self.handles_at(b.frame).and_then(|h| h.in_handle);
        out.or(into)?;
        let (first, last) = ([a.value, y[i - 1]], [b.value, y[i]]);
        let from = |at: [f64; 2], h: Option<[f64; 2]>| {
            let [x, y] = h.unwrap_or([0.0, 0.0]);
            [at[0] + x, at[1] + y]
        };
        let path = [first, from(first, out), from(last, into), last];
        let len = (b.frame - a.frame) as f64;
        Some(along_path(path, (frame - a.frame as f64) / len))
    }

    pub fn move_handles(&mut self, frame: impl Fn(i64) -> i64) {
        for h in &mut self.position_handles {
            h.frame = frame(h.frame);
        }
        self.position_handles.sort_by_key(|h| h.frame);
        self.position_handles.dedup_by_key(|h| h.frame);
    }

    pub fn put_handles(
        &mut self,
        frame: i64,
        in_handle: Option<[f64; 2]>,
        out_handle: Option<[f64; 2]>,
    ) {
        self.position_handles.retain(|h| h.frame != frame);
        if in_handle.is_some() || out_handle.is_some() {
            self.position_handles.push(PositionHandles {
                frame,
                in_handle,
                out_handle,
            });
            self.position_handles.sort_by_key(|h| h.frame);
        }
    }

    pub fn in_file(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        [x, 1.0 - y]
    }

    pub fn ease_position(&mut self, frame: i64, ease: Ease) {
        let keys = &self.position_keys;
        let Some(i) = keys.iter().position(|k| k.frame == frame) else {
            return;
        };
        let (wants_in, wants_out) = reaches(keys.len(), i, ease);
        let beside = |j: Option<usize>| {
            j.and_then(|j| Some(self.in_file([keys.get(j)?.value, *self.position_y.get(j)?])))
                .unwrap_or([0.0, 0.0])
        };
        let (before, after) = (beside(i.checked_sub(1)), beside(Some(i + 1)));
        let x = rounded((after[0] - before[0]) * PATH_EASE);
        let y = rounded((after[1] - before[1]) * PATH_EASE);
        self.put_handles(
            frame,
            wants_in.then_some([0.0 - x, y]),
            wants_out.then_some([x, 0.0 - y]),
        );
    }

    pub fn position_ease(&self, frame: i64) -> Ease {
        let h = self.handles_at(frame);
        match (
            h.is_some_and(|h| h.in_handle.is_some()),
            h.is_some_and(|h| h.out_handle.is_some()),
        ) {
            (false, false) => Ease::Linear,
            (true, false) => Ease::EaseIn,
            (false, true) => Ease::EaseOut,
            (true, true) => Ease::EaseInOut,
        }
    }

    pub fn as_picture(&self) -> Picture {
        let zoom = self.scale();
        Picture {
            position: Some(self.place()),
            position_keys: self.position_keys.clone(),
            position_y: self.position_y.clone(),
            zoom: Some([zoom, zoom]),
            zoom_keys: self.zoom_keys.clone(),
            zoom_y: self.zoom_keys.iter().map(|k| k.value).collect(),
            rotation: self.rotation,
            rotation_keys: self.rotation_keys.clone(),
            opacity: self.opacity,
            opacity_keys: self.opacity_keys.clone(),
            ..Picture::default()
        }
    }

    pub fn take_picture(&mut self, p: &Picture) {
        let at = p.place();
        self.position = (at != Self::usual_position(self.kind, self.align())).then_some(at);
        self.position_keys.clone_from(&p.position_keys);
        self.position_y.clone_from(&p.position_y);
        let held = |h: &PositionHandles| p.position_keys.iter().any(|k| k.frame == h.frame);
        self.position_handles.retain(held);
        self.zoom = Some(p.scale()[0]).filter(|z| *z != 1.0);
        self.zoom_keys.clone_from(&p.zoom_keys);
        self.rotation = p.rotation;
        self.rotation_keys.clone_from(&p.rotation_keys);
        self.opacity = p.opacity;
        self.opacity_keys.clone_from(&p.opacity_keys);
    }

    pub fn sampled(&self, frame: f64) -> Title {
        if !self.is_keyed() {
            return self.clone();
        }
        let mut t = self.clone();
        t.each_line(|line| {
            let mut picture = line.as_picture().sampled(frame);
            if let Some(at) = line.on_path(frame) {
                picture.position = Some(at);
            }
            line.take_picture(&picture);
        });
        t
    }
}

fn scale_channel(p: &mut Picture, v: PictureValue, conv: &impl Fn(i64) -> i64, ratio: f64) {
    let y_was = parts(p)[v as usize].1.cloned().unwrap_or_default();
    let (keys, y) = parts_mut(p, v);
    let mut rows: Vec<(Keyframe, f64)> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let mut k = *k;
            k.frame = conv(k.frame);
            scale_handles(&mut k, ratio);
            (k, y_was.get(i).copied().unwrap_or(0.0))
        })
        .collect();
    rows.sort_by_key(|(k, _)| k.frame);
    rows.dedup_by_key(|(k, _)| k.frame);
    *keys = rows.iter().map(|(k, _)| *k).collect();
    if let Some(y) = y {
        *y = rows.into_iter().map(|(_, v)| v).collect();
    }
}

fn retime_channel(p: &mut Picture, v: PictureValue, old: &Clip, new: &Clip) {
    let y_was = parts(p)[v as usize].1.cloned().unwrap_or_default();
    let (keys, y) = parts_mut(p, v);
    if keys.is_empty() {
        return;
    }
    let mapped: Vec<(i64, f64)> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            (
                new.key_at(old.key_time(k.frame)).round() as i64,
                y_was.get(i).copied().unwrap_or(0.0),
            )
        })
        .collect();
    let next = retimed(keys, old, new);
    if let Some(y) = y {
        *y = next
            .iter()
            .map(|k| {
                mapped
                    .iter()
                    .find(|(f, _)| *f == k.frame)
                    .map(|(_, v)| *v)
                    .unwrap_or(0.0)
            })
            .collect();
    }
    *keys = next;
}

fn retarget_channel(p: &mut Picture, v: PictureValue, map: &impl Fn(i64) -> i64) {
    let y_was = parts(p)[v as usize].1.cloned().unwrap_or_default();
    let (keys, y) = parts_mut(p, v);
    let mut rows: Vec<(Keyframe, f64)> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            (
                Keyframe {
                    frame: map(k.frame),
                    ..*k
                },
                y_was.get(i).copied().unwrap_or(0.0),
            )
        })
        .collect();
    rows.sort_by_key(|(k, _)| k.frame);
    rows.dedup_by_key(|(k, _)| k.frame);
    *keys = rows.iter().map(|(k, _)| *k).collect();
    if let Some(y) = y {
        *y = rows.into_iter().map(|(_, v)| v).collect();
    }
}

fn previewed_effects(
    stored: &[Effect],
    baseline: &[Effect],
    edited: &[Effect],
    frame: i64,
) -> Vec<Effect> {
    if stored.len() != edited.len() || baseline.len() != edited.len() {
        return edited.to_vec();
    }
    stored
        .iter()
        .zip(baseline)
        .zip(edited)
        .map(|((s, b), e)| s.previewed(b, e, frame))
        .collect()
}

pub fn ease_mark(keys: &mut Vec<Keyframe>, frame: i64, ease: Ease) {
    reshape(keys, |marked| {
        if let Some((k, mark)) = marked.iter_mut().find(|(k, _)| k.frame == frame) {
            *mark = Some(ease);
            if ease.is_linear() {
                (k.in_handle, k.out_handle) = (None, None);
            }
        }
    });
}

fn key_at(frame: i64, value: f64) -> Keyframe {
    Keyframe {
        frame,
        value,
        in_handle: None,
        out_handle: None,
    }
}

fn put_pair(keys: &mut Vec<Keyframe>, y: &mut Vec<f64>, frame: i64, value: [f64; 2]) {
    let mut rows: Vec<(Keyframe, f64, Option<Ease>)> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, y.get(i).copied().unwrap_or(value[1]), ease_of(keys, i)))
        .collect();
    match rows.iter_mut().find(|(k, _, _)| k.frame == frame) {
        Some((k, yy, _)) => {
            k.value = value[0];
            *yy = value[1];
        }
        None => rows.push((key_at(frame, value[0]), value[1], None)),
    }
    rows.sort_by_key(|(k, _, _)| k.frame);
    rows.dedup_by_key(|(k, _, _)| k.frame);
    *keys = rows.iter().map(|(k, _, _)| *k).collect();
    *y = rows.iter().map(|(_, v, _)| *v).collect();
    for (i, (_, _, e)) in rows.iter().enumerate() {
        if let Some(e) = e.filter(|e| !e.is_linear()) {
            ease(keys, i, e);
        }
    }
}

pub fn picture_problem(p: &Picture) -> Option<String> {
    PictureValue::ALL.into_iter().find_map(|v| {
        let (keys, y) = parts(p)[v as usize];
        channel_problem(v, keys, y, true)
    })
}

pub fn title_keys_problem(p: &Picture) -> Option<String> {
    PictureValue::ALL.into_iter().find_map(|v| {
        let (keys, y) = parts(p)[v as usize];
        channel_problem(v, keys, y, v != PictureValue::Position)
    })
}

fn channel_problem(
    v: PictureValue,
    keys: &[Keyframe],
    y: Option<&Vec<f64>>,
    ranged: bool,
) -> Option<String> {
    if keys.is_empty() {
        return y.filter(|y| !y.is_empty()).map(|_| {
            format!(
                "{} keyframes need one Y for each keyframe",
                v.name().replace('_', " ")
            )
        });
    }
    if keys.windows(2).any(|w| w[0].frame >= w[1].frame) {
        return Some(format!(
            "{} keyframes out of order or on one frame",
            v.name().replace('_', " ")
        ));
    }
    if let Some(y) = y
        && y.len() != keys.len()
    {
        return Some(format!(
            "{} keyframes need one Y for each keyframe",
            v.name().replace('_', " ")
        ));
    }
    keys.iter().enumerate().find_map(|(i, k)| {
        let value = match y {
            Some(y) => [k.value, y[i]],
            None => [k.value, 0.0],
        };
        let handle = [k.in_handle, k.out_handle]
            .into_iter()
            .zip([true, false])
            .find_map(|(h, before)| {
                h.and_then(|h| checked_handle(h, before).err().map(|e| e.message))
            });
        let range = if ranged {
            value_problem(v, value).map(String::from)
        } else {
            None
        };
        range.or(handle).map(|e| {
            format!(
                "{} keyframe at source frame {}: {}",
                v.name().replace('_', " "),
                k.frame,
                e
            )
        })
    })
}

pub fn value_problem(v: PictureValue, value: [f64; 2]) -> Option<&'static str> {
    let reach = |n: &f64| (n - 0.5).abs() <= Picture::REACH;
    let ok = match v {
        PictureValue::Position | PictureValue::Anchor => value.iter().all(reach),
        PictureValue::Zoom => value
            .iter()
            .all(|n| (Picture::ZOOM.0..=Picture::ZOOM.1).contains(n)),
        PictureValue::Rotation => value[0].abs() <= Picture::TURNS,
        PictureValue::Pitch | PictureValue::Yaw => value[0].abs() <= Picture::LEAN,
        PictureValue::CropSoftness => value[0].abs() <= Picture::SOFT,
        PictureValue::CropLeft
        | PictureValue::CropRight
        | PictureValue::CropTop
        | PictureValue::CropBottom
        | PictureValue::Opacity => (0.0..=1.0).contains(&value[0]),
    };
    (!ok).then_some(match v {
        PictureValue::Position => {
            "a picture's position is two fractions of the frame, each from -3.5 to 4.5"
        }
        PictureValue::Zoom => "a picture's zoom is from 0 to 100",
        PictureValue::Rotation => "a picture's rotation is degrees, from -100000 to 100000",
        PictureValue::Anchor => {
            "a picture's anchor is two fractions of the picture, each from -3.5 to 4.5"
        }
        PictureValue::Pitch => "a picture's pitch is from -1.5 to 1.5",
        PictureValue::Yaw => "a picture's yaw is from -1.5 to 1.5",
        PictureValue::CropSoftness => "a picture's crop softness is from -100 to 100",
        PictureValue::Opacity => "a picture's opacity is from 0 to 1",
        PictureValue::CropLeft
        | PictureValue::CropRight
        | PictureValue::CropTop
        | PictureValue::CropBottom => {
            "a picture's crop is a fraction of it on each side, from 0 to 1"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Picture;

    fn key(frame: i64, value: f64) -> Keyframe {
        Keyframe {
            frame,
            value,
            in_handle: None,
            out_handle: None,
        }
    }

    #[test]
    fn a_bend_of_a_straight_part_moves_its_middle_by_the_drag() {
        let (a, mut b) = (key(0, 0.0), key(30, 30.0));
        let (out, into) = bent(&a, &b, 15.0, 6.0);
        let (out, into) = (out.unwrap(), into.unwrap());
        assert_eq!((out.frames, out.value), (10.0, 18.0));
        assert_eq!((into.frames, into.value), (-10.0, -2.0));
        let mut a = a;
        a.out_handle = Some(out);
        b.in_handle = Some(into);
        let middle = value_at(&[a, b], 15.0).unwrap();
        assert!((middle - 21.0).abs() < 1e-6);
    }

    #[test]
    fn a_bend_near_the_first_key_moves_only_its_handle() {
        let (a, b) = (key(0, 0.0), key(30, 0.0));
        let (out, into) = bent(&a, &b, 3.0, 1.0);
        assert!(out.unwrap().value > 0.0);
        assert_eq!(into.unwrap().value, 0.0);
    }

    #[test]
    fn a_bend_leaves_a_side_with_no_handle_without_one() {
        let (mut a, b) = (key(0, 0.0), key(30, 30.0));
        a.out_handle = Some(Handle {
            frames: 10.0,
            value: 0.0,
        });
        let (out, into) = bent(&a, &b, 15.0, 3.0);
        assert!(out.is_some());
        assert!(into.is_none());
    }

    #[test]
    fn a_retarget_moves_an_effect_key() {
        let mut e = crate::model::EffectType::GaussianBlur.made();
        e.keys = vec![crate::model::EffectKeys {
            value: crate::model::EffectValue::Strength,
            keys: vec![key(10, 0.5)],
        }];
        let mut p = Picture {
            effects: vec![e],
            ..Picture::default()
        };
        p.retarget(|f| f + 5);
        assert_eq!(p.effects[0].keys[0].keys[0].frame, 15);
    }

    #[test]
    fn linear_zoom_is_a_quarter_at_the_middle() {
        let p = Picture {
            zoom_keys: vec![key(0, 0.1), key(30, 0.4)],
            zoom_y: vec![0.1, 0.4],
            ..Picture::default()
        };
        assert_eq!(p.scale_at(15.0), [0.25, 0.25]);
    }

    #[test]
    fn a_sideways_position_handle_keeps_y_at_zero() {
        let mut first = key(0, 0.0);
        first.out_handle = Some(Handle {
            frames: 10.0,
            value: 4.0,
        });
        let p = Picture {
            position_keys: vec![first, key(30, 10.0)],
            position_y: vec![0.0, 0.0],
            ..Picture::default()
        };
        assert_eq!(p.place_at(15.0)[1], 0.0);
    }
}
