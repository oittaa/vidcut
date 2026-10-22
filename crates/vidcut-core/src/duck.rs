use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use crate::analysis::{self, AudioLevels, Span, VoiceActivity};
use crate::editor::Env;
use crate::error::{Error, Result};
use crate::model::{Duck, DuckDetect, Media, Project};
use crate::render::{Dip, DuckSource, Ducking, LaneTrack, RenderPlan};
use crate::request::DuckSet;

pub const AMOUNT_DB: f64 = 2.7;
pub const LOOKAHEAD_MS: f64 = 15.0;
pub const RISE_MS: f64 = 10.0;
pub const HOLD_MS: f64 = 150.0;
pub const RECOVERY_MS: f64 = 750.0;
pub const THRESHOLD_DB: f64 = -42.0;

pub const AMOUNT: RangeInclusive<f64> = 0.0..=60.0;
pub const LOOKAHEAD: RangeInclusive<f64> = 0.0..=1000.0;
pub const RISE: RangeInclusive<f64> = 1.0..=1000.0;
pub const HOLD: RangeInclusive<f64> = 0.0..=5000.0;
pub const RECOVERY: RangeInclusive<f64> = 1.0..=1000.0;
pub const THRESHOLD: RangeInclusive<f64> = -96.0..=0.0;

impl Duck {
    pub fn new(sources: Vec<String>) -> Self {
        Self {
            on: true,
            sources,
            amount_db: None,
            lookahead_ms: None,
            rise_ms: None,
            hold_ms: None,
            recovery_ms: None,
            detect: None,
            threshold_db: None,
        }
    }

    pub fn amount(&self) -> f64 {
        self.amount_db.unwrap_or(AMOUNT_DB)
    }

    pub fn lookahead(&self) -> f64 {
        self.lookahead_ms.unwrap_or(LOOKAHEAD_MS)
    }

    pub fn rise(&self) -> f64 {
        self.rise_ms.unwrap_or(RISE_MS)
    }

    pub fn hold(&self) -> f64 {
        self.hold_ms.unwrap_or(HOLD_MS)
    }

    pub fn recovery(&self) -> f64 {
        self.recovery_ms.unwrap_or(RECOVERY_MS)
    }

    pub fn detect(&self) -> DuckDetect {
        self.detect.unwrap_or_default()
    }

    pub fn threshold(&self) -> f64 {
        self.threshold_db.unwrap_or(THRESHOLD_DB)
    }

    pub fn shown(&self) -> serde_json::Value {
        serde_json::json!({
            "on": self.on,
            "sources": self.sources,
            "amount_db": self.amount(),
            "lookahead_ms": self.lookahead(),
            "rise_ms": self.rise(),
            "hold_ms": self.hold(),
            "recovery_ms": self.recovery(),
            "detect": self.detect(),
            "threshold_db": self.threshold(),
        })
    }
}

pub fn default_sources(p: &Project, target: &str) -> Vec<String> {
    p.tracks
        .iter()
        .filter(|t| t.id != target && t.has_sound())
        .map(|t| t.id.clone())
        .collect()
}

fn unless_usual(v: f64, usual: f64) -> Option<f64> {
    (v != usual).then_some(v)
}

pub fn patched(p: &Project, target: &str, set: &DuckSet) -> Result<Duck> {
    let t = p
        .track(target)
        .ok_or_else(|| Error::not_found(format!("no track {target}")))?;
    let mut d = match &t.duck {
        Some(d) => d.clone(),
        None if set.sources.is_some() => Duck::new(Vec::new()),
        None => Duck::new(default_sources(p, target)),
    };
    if let Some(on) = set.on {
        d.on = on;
    }
    if let Some(sources) = &set.sources {
        d.sources = Vec::new();
        for s in sources {
            if !d.sources.contains(s) {
                d.sources.push(s.clone());
            }
        }
    }
    if let Some(v) = set.amount_db {
        d.amount_db = unless_usual(v, AMOUNT_DB);
    }
    if let Some(v) = set.lookahead_ms {
        d.lookahead_ms = unless_usual(v, LOOKAHEAD_MS);
    }
    if let Some(v) = set.rise_ms {
        d.rise_ms = unless_usual(v, RISE_MS);
    }
    if let Some(v) = set.hold_ms {
        d.hold_ms = unless_usual(v, HOLD_MS);
    }
    if let Some(v) = set.recovery_ms {
        d.recovery_ms = unless_usual(v, RECOVERY_MS);
    }
    if let Some(v) = set.detect {
        d.detect = (v != DuckDetect::default()).then_some(v);
    }
    if let Some(v) = set.threshold_db {
        d.threshold_db = unless_usual(v, THRESHOLD_DB);
    }
    checked(p, target, &d)?;
    Ok(d)
}

pub fn checked(p: &Project, target: &str, d: &Duck) -> Result<()> {
    for (i, s) in d.sources.iter().enumerate() {
        if s == target {
            return Err(Error::invalid(format!("{target} can't duck under itself")));
        }
        if p.track(s).is_none() {
            return Err(Error::not_found(format!(
                "no track {s} to duck {target} under"
            )));
        }
        if d.sources[..i].contains(s) {
            return Err(Error::invalid(format!("{s} is a source of {target} twice")));
        }
    }
    let values = [
        ("amount_db", d.amount_db, AMOUNT, "dB"),
        ("lookahead_ms", d.lookahead_ms, LOOKAHEAD, "ms"),
        ("rise_ms", d.rise_ms, RISE, "ms"),
        ("hold_ms", d.hold_ms, HOLD, "ms"),
        ("recovery_ms", d.recovery_ms, RECOVERY, "ms"),
        ("threshold_db", d.threshold_db, THRESHOLD, "dB"),
    ];
    for (name, v, range, unit) in values {
        if let Some(v) = v
            && !range.contains(&v)
        {
            return Err(Error::invalid(format!(
                "duck {name} {v} is outside {} to {} {unit}",
                range.start(),
                range.end()
            )));
        }
    }
    Ok(())
}

pub(crate) fn renamed(p: &mut Project, names: &HashMap<String, String>) {
    for t in &mut p.tracks {
        if let Some(d) = t.duck.as_mut() {
            d.sources = d
                .sources
                .iter()
                .filter_map(|s| names.get(s).cloned())
                .collect();
        }
    }
}

pub(crate) fn tidy(p: &mut Project) {
    let ids: Vec<String> = p.tracks.iter().map(|t| t.id.clone()).collect();
    for t in &mut p.tracks {
        let id = t.id.clone();
        if let Some(d) = t.duck.as_mut() {
            let mut kept: Vec<String> = Vec::new();
            for s in &d.sources {
                if *s != id && ids.contains(s) && !kept.contains(s) {
                    kept.push(s.clone());
                }
            }
            d.sources = kept;
        }
    }
    for i in 0..p.tracks.len() {
        let bad = p.tracks[i]
            .duck
            .as_ref()
            .is_some_and(|d| checked(p, &p.tracks[i].id, d).is_err());
        if bad {
            p.tracks[i].duck = None;
        }
    }
}

pub fn sounds<'a>(p: &'a Project, target: &str, d: &Duck) -> Vec<(&'a Media, u32)> {
    let mut out: Vec<(&Media, u32)> = Vec::new();
    for t in p
        .tracks
        .iter()
        .filter(|t| t.id != target && d.sources.contains(&t.id))
    {
        for c in t.clips.iter().filter(|c| !c.is_block()) {
            let Some(a) = c.audio.as_ref().filter(|a| !a.muted) else {
                continue;
            };
            let Some(m) = p
                .media(&c.media)
                .filter(|m| m.info.audio.iter().any(|x| x.index == a.stream))
            else {
                continue;
            };
            if !out.iter().any(|(x, s)| x.id == m.id && *s == a.stream) {
                out.push((m, a.stream));
            }
        }
    }
    out
}

pub fn measure(p: &Project, target: &str, d: &Duck, env: &dyn Env, download: bool) -> Result<()> {
    for (m, stream) in sounds(p, target, d)
        .into_iter()
        .filter(|(m, _)| env.present(m))
    {
        match d.detect() {
            DuckDetect::Speech => env.voice_activity(m, stream, download).map(drop)?,
            DuckDetect::Level => env.audio_levels(m, stream).map(drop)?,
        }
    }
    Ok(())
}

pub(crate) fn planned(p: &Project, lanes: &[LaneTrack], a: i64, b: i64) -> Vec<Ducking> {
    let s = &p.settings;
    let fps = s.fps.as_f64();
    let rate = f64::from(s.sample_rate);
    let samples = |ms: f64| ms * rate / 1000.0;
    let frames = |ms: f64| (ms * fps / 1000.0).ceil() as i64 + 1;
    let any_solo = p.any_solo();
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for t in &p.tracks {
        let Some(d) = t.duck.as_ref().filter(|d| d.on && d.amount() > 0.0) else {
            continue;
        };
        if seen.contains(&t.id.as_str()) {
            continue;
        }
        seen.push(&t.id);
        let ducked: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.track == t.id)
            .map(|(i, _)| i)
            .collect();
        if ducked.is_empty() {
            continue;
        }
        let from = a - frames(d.hold() + d.recovery());
        let to = b + frames(d.lookahead() + d.rise());
        let mut sources = Vec::new();
        for x in p
            .tracks
            .iter()
            .filter(|x| x.id != t.id && d.sources.contains(&x.id) && x.audible(any_solo))
        {
            for c in &x.clips {
                let Some(au) = c.audio.as_ref().filter(|au| !au.muted) else {
                    continue;
                };
                if c.is_block() || c.speed() == 0.0 {
                    continue;
                }
                let Some((ra, rb)) = c.audio_range() else {
                    continue;
                };
                let (x0, x1) = (ra.max(from), rb.min(to));
                if x1 <= x0 {
                    continue;
                }
                let Ok((_, origin)) = p.media_origin(&c.media) else {
                    continue;
                };
                sources.push(DuckSource {
                    media: c.media.clone(),
                    stream: au.stream,
                    origin,
                    frames: (x0, x1),
                    start: c.start,
                    source_in: c.source_in,
                    pace: c.pace(),
                    gain_db: au.gain_db + x.volume_db,
                });
            }
        }
        out.push(Ducking {
            lanes: ducked,
            dip: Dip {
                amount_db: d.amount(),
                lookahead: samples(d.lookahead()),
                rise: samples(d.rise()),
                hold: samples(d.hold()),
                recovery: samples(d.recovery()),
                active: Vec::new(),
            },
            detect: d.detect(),
            threshold_db: d.threshold(),
            sources,
        });
    }
    out
}

#[derive(Debug, Clone)]
pub enum Measured {
    Speech(Arc<VoiceActivity>),
    Levels(Arc<AudioLevels>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Unmeasured {
    pub media: String,
    pub stream: u32,
    pub detect: DuckDetect,
}

pub type Measure<'a> = dyn FnMut(&Media, u32, DuckDetect) -> Result<Option<Measured>> + 'a;

pub fn hear(plan: &mut RenderPlan, p: &Project, measured: &mut Measure) -> Result<Vec<Unmeasured>> {
    let fps = plan.fps;
    let (rate, f) = (f64::from(plan.sample_rate), fps.as_f64());
    let first = plan.range.0 as f64;
    let sample = |frame: f64| ((frame - first) * rate / f).round() as i64;
    let mut missing: Vec<Unmeasured> = Vec::new();
    for d in &mut plan.ducking {
        let mut active = Vec::new();
        for s in &d.sources {
            let Some(m) = p
                .media(&s.media)
                .filter(|m| m.info.audio.iter().any(|x| x.index == s.stream))
            else {
                continue;
            };
            let span = Span {
                start: s.start,
                end: s.frames.1,
                source_in: s.source_in,
                origin: s.origin,
                fps,
                pace: s.pace,
            };
            let found = match measured(m, s.stream, d.detect)? {
                Some(Measured::Speech(va)) => analysis::speech_frames(&span, s.frames, &va),
                Some(Measured::Levels(l)) => {
                    analysis::loud_frames(&span, s.frames, &l, d.threshold_db, s.gain_db)
                }
                None => {
                    let u = Unmeasured {
                        media: s.media.clone(),
                        stream: s.stream,
                        detect: d.detect,
                    };
                    if !missing.contains(&u) {
                        missing.push(u);
                    }
                    continue;
                }
            };
            active.extend(found.into_iter().map(|(x, y)| (sample(x), sample(y))));
        }
        active.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (x, y) in active {
            match merged.last_mut() {
                Some(last) if x <= last.1 => last.1 = last.1.max(y),
                _ => merged.push((x, y)),
            }
        }
        d.dip.active = merged;
    }
    Ok(missing)
}

pub fn hear_with(plan: &mut RenderPlan, p: &Project, env: &dyn Env, download: bool) -> Result<()> {
    if plan.ducking.is_empty() {
        return Ok(());
    }
    let mut measured = |m: &Media, stream: u32, detect: DuckDetect| -> Result<Option<Measured>> {
        Ok(Some(match detect {
            DuckDetect::Speech => Measured::Speech(env.voice_activity(m, stream, download)?),
            DuckDetect::Level => Measured::Levels(env.audio_levels(m, stream)?),
        }))
    };
    hear(plan, p, &mut measured).map(drop)
}
