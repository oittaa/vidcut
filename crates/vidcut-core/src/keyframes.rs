use crate::model::{Clip, ClipAudio, Ease, Handle, Keyframe};
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
    let len = (b.frame - a.frame) as f64;
    let x = frame - a.frame as f64;
    if a.out_handle.is_none() && b.in_handle.is_none() {
        return a.value + (b.value - a.value) * x / len;
    }
    let (out, into) = (a.out_handle.unwrap_or(FLAT), b.in_handle.unwrap_or(FLAT));
    let x1 = out.frames.clamp(0.0, len);
    let x2 = len + into.frames.clamp(-len, 0.0);
    let u = solve(|u| cubic([0.0, x1, x2, len], u), x);
    cubic(
        [a.value, a.value + out.value, b.value + into.value, b.value],
        u,
    )
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
