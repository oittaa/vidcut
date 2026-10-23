use crate::model::{Clip, ClipAudio, Crop, Ease, Handle, Keyframe, Picture};
use crate::validate::GAIN;

const FLAT: Handle = Handle {
    frames: 0.0,
    value: 0.0,
};

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

fn between(a: &Keyframe, b: &Keyframe, frame: f64) -> f64 {
    between_values(a, a.value, b, b.value, frame)
}

fn between_values(a: &Keyframe, av: f64, b: &Keyframe, bv: f64, frame: f64) -> f64 {
    let len = (b.frame - a.frame) as f64;
    let x = frame - a.frame as f64;
    if a.out_handle.is_none() && b.in_handle.is_none() {
        return av + (bv - av) * x / len;
    }
    let (out, into) = (a.out_handle.unwrap_or(FLAT), b.in_handle.unwrap_or(FLAT));
    let x1 = out.frames.clamp(0.0, len);
    let x2 = len + into.frames.clamp(-len, 0.0);
    let u = solve(|u| cubic([0.0, x1, x2, len], u), x);
    cubic([av, av + out.value, bv + into.value, bv], u)
}

pub fn value_mapped(keys: &[Keyframe], frame: f64, value: impl Fn(usize) -> f64) -> Option<f64> {
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

fn sides(ease: Ease) -> (bool, bool) {
    match ease {
        Ease::Linear => (false, false),
        Ease::EaseIn => (true, false),
        Ease::EaseOut => (false, true),
        Ease::EaseInOut => (true, true),
    }
}

fn handles(keys: &[Keyframe], i: usize, ease: Ease) -> (Option<Handle>, Option<Handle>) {
    let (wants_in, wants_out) = sides(ease);
    let gap = |j: usize| (keys[i].frame - keys[j].frame).abs() as f64 / 3.0;
    let flat = |frames: f64| Handle { frames, value: 0.0 };
    let before = i.checked_sub(1).filter(|_| wants_in).map(|j| flat(-gap(j)));
    let after = (i + 1 < keys.len() && wants_out).then(|| flat(gap(i + 1)));
    (before, after)
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
    let last = keys.len().saturating_sub(1);
    Ease::ALL
        .into_iter()
        .filter(|e| {
            let (wants_in, wants_out) = sides(*e);
            !(wants_in && i == 0 || wants_out && i >= last)
        })
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
    let (from, to) = (old.speed(), new.speed());
    if keys.is_empty() || from == 0.0 || to == 0.0 {
        return keys.to_vec();
    }
    let ratio = to / from;
    let scaled = |h: Option<Handle>| {
        h.map(|h| Handle {
            frames: h.frames * ratio,
            value: h.value,
        })
    };
    let mut out = keys.to_vec();
    reshape(&mut out, |marked| {
        for (k, e) in marked.iter_mut() {
            k.frame = new.sound_at(old.key_time(k.frame)).round() as i64;
            let (i, o) = (scaled(k.in_handle), scaled(k.out_handle));
            (k.in_handle, k.out_handle) = if ratio < 0.0 { (o, i) } else { (i, o) };
            if ratio < 0.0 {
                *e = e.map(|e| match e {
                    Ease::EaseIn => Ease::EaseOut,
                    Ease::EaseOut => Ease::EaseIn,
                    e => e,
                });
            }
        }
    });
    out
}

pub fn checked(value: f64) -> crate::error::Result<f64> {
    if GAIN.contains(&value) {
        Ok(value)
    } else {
        Err(crate::error::Error::invalid(format!(
            "gain {value} dB outside -60..24 dB"
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
    pub fn time_of(&self, frame: f64) -> f64 {
        let pace = self.pace();
        if pace.speed == 0.0 {
            return self.start as f64;
        }
        self.start as f64 + (frame - self.source_in as f64 - pace.sound(0.0)) / pace.speed
    }

    pub fn key_time(&self, frame: i64) -> f64 {
        self.time_of(frame as f64)
    }

    pub fn gain_at(&self, t: f64) -> Option<f64> {
        self.audio.as_ref().map(|a| a.db_at(self.sound_at(t)))
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
    CropLeft,
    CropRight,
    CropTop,
    CropBottom,
}

impl PictureValue {
    pub const ALL: [PictureValue; 7] = [
        PictureValue::Position,
        PictureValue::Zoom,
        PictureValue::Rotation,
        PictureValue::CropLeft,
        PictureValue::CropRight,
        PictureValue::CropTop,
        PictureValue::CropBottom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PictureValue::Position => "position",
            PictureValue::Zoom => "zoom",
            PictureValue::Rotation => "rotation",
            PictureValue::CropLeft => "crop_left",
            PictureValue::CropRight => "crop_right",
            PictureValue::CropTop => "crop_top",
            PictureValue::CropBottom => "crop_bottom",
        }
    }

    pub fn pair(self) -> bool {
        matches!(self, PictureValue::Position | PictureValue::Zoom)
    }

    pub fn of_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == name)
    }
}

fn pair_at(keys: &[Keyframe], y: &[f64], frame: f64) -> Option<[f64; 2]> {
    if keys.is_empty() || keys.len() != y.len() {
        return None;
    }
    Some([value_at(keys, frame)?, value_mapped(keys, frame, |i| y[i])?])
}

fn one_at(keys: &[Keyframe], frame: f64) -> Option<f64> {
    (!keys.is_empty()).then(|| value_at(keys, frame)).flatten()
}

fn parts(p: &Picture) -> [(&Vec<Keyframe>, Option<&Vec<f64>>); 7] {
    [
        (&p.position_keys, Some(&p.position_y)),
        (&p.zoom_keys, Some(&p.zoom_y)),
        (&p.rotation_keys, None),
        (&p.crop_left_keys, None),
        (&p.crop_right_keys, None),
        (&p.crop_top_keys, None),
        (&p.crop_bottom_keys, None),
    ]
}

fn parts_mut(p: &mut Picture, v: PictureValue) -> (&mut Vec<Keyframe>, Option<&mut Vec<f64>>) {
    match v {
        PictureValue::Position => (&mut p.position_keys, Some(&mut p.position_y)),
        PictureValue::Zoom => (&mut p.zoom_keys, Some(&mut p.zoom_y)),
        PictureValue::Rotation => (&mut p.rotation_keys, None),
        PictureValue::CropLeft => (&mut p.crop_left_keys, None),
        PictureValue::CropRight => (&mut p.crop_right_keys, None),
        PictureValue::CropTop => (&mut p.crop_top_keys, None),
        PictureValue::CropBottom => (&mut p.crop_bottom_keys, None),
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
        PictureValue::CropLeft => [p.crop.left, 0.0],
        PictureValue::CropRight => [p.crop.right, 0.0],
        PictureValue::CropTop => [p.crop.top, 0.0],
        PictureValue::CropBottom => [p.crop.bottom, 0.0],
    }
}

fn write_plain(p: &mut Picture, v: PictureValue, value: [f64; 2]) {
    match v {
        PictureValue::Position => p.position = Some(value).filter(|at| *at != Picture::MIDDLE),
        PictureValue::Zoom => p.zoom = Some(value).filter(|z| *z != [1.0, 1.0]),
        PictureValue::Rotation => p.rotation = Some(value[0]).filter(|r| *r != 0.0),
        PictureValue::CropLeft => p.crop.left = value[0],
        PictureValue::CropRight => p.crop.right = value[0],
        PictureValue::CropTop => p.crop.top = value[0],
        PictureValue::CropBottom => p.crop.bottom = value[0],
    }
}

impl Picture {
    pub fn is_keyed(&self) -> bool {
        PictureValue::ALL.into_iter().any(|v| self.has_keys(v))
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
        p
    }

    pub fn scale_times(&mut self, conv: &impl Fn(i64) -> i64, ratio: f64) {
        for v in PictureValue::ALL {
            scale_channel(self, v, &conv, ratio);
        }
    }

    pub fn retimed_to(&mut self, old: &Clip, new: &Clip) {
        for v in PictureValue::ALL {
            retime_channel(self, v, old, new);
        }
    }

    pub fn retarget(&mut self, map: impl Fn(i64) -> i64) {
        for v in PictureValue::ALL {
            retarget_channel(self, v, &map);
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
        let (keys, _) = parts_mut(self, v);
        reshape(keys, |marked| {
            if let Some((k, mark)) = marked.iter_mut().find(|(k, _)| k.frame == frame) {
                *mark = Some(ease);
                if ease.is_linear() {
                    (k.in_handle, k.out_handle) = (None, None);
                }
            }
        });
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

    pub fn drop_chosen(
        &mut self,
        v: PictureValue,
        chosen: &std::collections::HashSet<i64>,
    ) -> Option<[f64; 2]> {
        let first = self
            .keys_of(v)
            .iter()
            .find(|k| chosen.contains(&k.frame))
            .and_then(|k| self.key_value(v, k.frame));
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
        if let (false, Some(value)) = (self.has_keys(v), first) {
            write_plain(self, v, value);
        }
        first
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
            for h in [&mut k.in_handle, &mut k.out_handle].into_iter().flatten() {
                h.frames *= ratio;
            }
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
                new.sound_at(old.key_time(k.frame)).round() as i64,
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
    for v in PictureValue::ALL {
        let (keys, y) = parts(p)[v as usize];
        if let Some(why) = channel_problem(v, keys, y, p) {
            return Some(why);
        }
    }
    None
}

fn channel_problem(
    v: PictureValue,
    keys: &[Keyframe],
    y: Option<&Vec<f64>>,
    p: &Picture,
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
        value_problem(v, value, p, k.frame).or(handle).map(|e| {
            format!(
                "{} keyframe at source frame {}: {}",
                v.name().replace('_', " "),
                k.frame,
                e
            )
        })
    })
}

fn value_problem(v: PictureValue, value: [f64; 2], p: &Picture, frame: i64) -> Option<String> {
    let bad = match v {
        PictureValue::Position => !value.iter().all(|n| n.is_finite() && n.abs() <= 100.0),
        PictureValue::Zoom => !value
            .iter()
            .all(|n| n.is_finite() && (0.01..=100.0).contains(n)),
        PictureValue::Rotation => !(value[0].is_finite() && value[0].abs() <= 3600.0),
        PictureValue::CropLeft
        | PictureValue::CropRight
        | PictureValue::CropTop
        | PictureValue::CropBottom => {
            let at = p.crop_at(frame as f64);
            !(value[0].is_finite() && (0.0..1.0).contains(&value[0]))
                || at.left + at.right >= 1.0
                || at.top + at.bottom >= 1.0
        }
    };
    bad.then(|| match v {
        PictureValue::Position => {
            "a picture's position is two fractions of the frame, each from -100 to 100".into()
        }
        PictureValue::Zoom => "a picture's zoom is from 1/100 to 100".into(),
        PictureValue::Rotation => "a picture's rotation is degrees, from -3600 to 3600".into(),
        _ => "a picture's crop is a fraction of it on each side, from 0, leaving some of it".into(),
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
    fn linear_zoom_is_a_quarter_at_the_middle() {
        let p = Picture {
            zoom_keys: vec![key(0, 0.1), key(30, 0.4)],
            zoom_y: vec![0.1, 0.4],
            ..Picture::default()
        };
        assert_eq!(p.scale_at(15.0), [0.25, 0.25]);
    }
}
