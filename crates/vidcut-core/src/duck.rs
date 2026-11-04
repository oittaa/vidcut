use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use crate::analysis::{self, AudioLevels, Heard, Span, VoiceActivity};
use crate::editor::Env;
use crate::error::{Error, Result};
use crate::keyframes::Gain;
use crate::model::{Duck, DuckDetect, Media, Project};
use crate::render::{Dip, DuckSource, Ducking, LaneTrack, RenderPlan};
use crate::request::DuckSet;
use crate::time::Rational;

pub const AMOUNT_DB: f64 = 2.7;
pub const LOOKAHEAD_MS: f64 = 15.0;
pub const RISE_MS: f64 = 10.0;
pub const HOLD_MS: f64 = 150.0;
pub const RECOVERY_MS: f64 = 750.0;
pub const THRESHOLD_DB: f64 = -42.0;

pub const AMOUNT: RangeInclusive<f64> = 0.0..=18.0;
pub const LOOKAHEAD: RangeInclusive<f64> = 0.0..=500.0;
pub const RISE: RangeInclusive<f64> = 0.0..=1000.0;
pub const HOLD: RangeInclusive<f64> = 0.0..=1000.0;
pub const RECOVERY: RangeInclusive<f64> = 0.0..=1000.0;
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
        out.push(ducking_of(p, &t.id, d, ducked, (a, b)));
    }
    out
}

fn ducking_of(
    p: &Project,
    target: &str,
    d: &Duck,
    lanes: Vec<usize>,
    (a, b): (i64, i64),
) -> Ducking {
    let s = &p.settings;
    let fps = s.fps.as_f64();
    let rate = f64::from(s.sample_rate);
    let samples = |ms: f64| ms * rate / 1000.0;
    let frames = |ms: f64| (ms * fps / 1000.0).ceil() as i64 + 1;
    let any_solo = p.any_solo();
    let from = a - frames(d.hold() + d.recovery());
    let to = b + frames(d.lookahead() + d.rise());
    let mut sources = Vec::new();
    for x in p
        .tracks
        .iter()
        .filter(|x| x.id != target && d.sources.contains(&x.id) && x.audible(any_solo))
    {
        for c in &x.clips {
            let Some(au) = c.audio.as_ref().filter(|au| !au.muted) else {
                continue;
            };
            if c.is_block() || c.is_frozen() {
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
            for (a, b) in c.runs(x0, x1) {
                let pace = c.pace().after(a - c.start);
                if pace.speed() == 0.0 {
                    continue;
                }
                sources.push(DuckSource {
                    media: c.media.clone(),
                    stream: au.stream,
                    origin,
                    frames: (a, b),
                    start: a,
                    source_in: c.source_at(a),
                    pace,
                    gain_db: if au.is_keyed() {
                        x.volume_db
                    } else {
                        au.gain_db + x.volume_db
                    },
                    keys: au.gain_keys.clone(),
                    key: c.content_at(a),
                });
            }
        }
    }
    Ducking {
        lanes,
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
    }
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
    let clock = Clock {
        fps: plan.fps,
        rate: f64::from(plan.sample_rate),
        first: plan.range.0,
    };
    let mut missing: Vec<Unmeasured> = Vec::new();
    for d in &mut plan.ducking {
        hear_one(d, p, &clock, measured, &mut missing)?;
    }
    Ok(missing)
}

struct Clock {
    fps: Rational,
    rate: f64,
    first: i64,
}

fn span_of(s: &DuckSource, fps: Rational) -> Span {
    Span {
        start: s.start,
        end: s.frames.1,
        source_in: s.source_in,
        origin: s.origin,
        fps,
        pace: s.pace.clone(),
    }
}

fn gain_of(s: &DuckSource, fps: f64) -> Gain<'_> {
    Gain {
        db: s.gain_db,
        keys: &s.keys,
        origin: s.origin,
        fps,
    }
    .retimed(&s.pace, s.source_in, s.key)
}

fn sounding<'a>(p: &'a Project, s: &DuckSource) -> Option<&'a Media> {
    p.media(&s.media)
        .filter(|m| m.info.audio.iter().any(|x| x.index == s.stream))
}

fn hear_one(
    d: &mut Ducking,
    p: &Project,
    clock: &Clock,
    measured: &mut Measure,
    missing: &mut Vec<Unmeasured>,
) -> Result<()> {
    let f = clock.fps.as_f64();
    let sample = |frame: f64| ((frame - clock.first as f64) * clock.rate / f).round() as i64;
    let mut active = Vec::new();
    for s in &d.sources {
        let Some(m) = sounding(p, s) else {
            continue;
        };
        let span = span_of(s, clock.fps);
        let found = match measured(m, s.stream, d.detect)? {
            Some(Measured::Speech(va)) => analysis::speech_frames(&span, s.frames, &va),
            Some(Measured::Levels(l)) => {
                analysis::loud_frames(&span, s.frames, &l, d.threshold_db, &gain_of(s, f))
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
    Ok(())
}

fn measuring(
    env: &dyn Env,
    download: bool,
) -> impl FnMut(&Media, u32, DuckDetect) -> Result<Option<Measured>> + '_ {
    move |m, stream, detect| {
        Ok(Some(match detect {
            DuckDetect::Speech => Measured::Speech(env.voice_activity(m, stream, download)?),
            DuckDetect::Level => Measured::Levels(env.audio_levels(m, stream)?),
        }))
    }
}

pub fn hear_with(plan: &mut RenderPlan, p: &Project, env: &dyn Env, download: bool) -> Result<()> {
    if plan.ducking.is_empty() {
        return Ok(());
    }
    hear(plan, p, &mut measuring(env, download)).map(drop)
}

pub const GRAPH_POINTS_PER_SECOND: u32 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    pub source_db: Vec<Option<f64>>,
    pub duck_db: Vec<f64>,
}

fn hundredths(v: f64) -> f64 {
    (v * 100.0).round() / 100.0 + 0.0
}

pub fn graph(
    p: &Project,
    target: &str,
    d: &Duck,
    (a, b): (i64, i64),
    env: &dyn Env,
) -> Result<Graph> {
    let s = &p.settings;
    let (fps, f, rate) = (s.fps, s.fps.as_f64(), f64::from(s.sample_rate));
    let mut ducking = ducking_of(p, target, d, Vec::new(), (a, b));
    ducking
        .sources
        .retain(|x| sounding(p, x).is_some_and(|m| env.present(m)));
    let clock = Clock {
        fps,
        rate,
        first: a,
    };
    hear_one(
        &mut ducking,
        p,
        &clock,
        &mut measuring(env, false),
        &mut Vec::new(),
    )?;
    let mut sounds = Vec::new();
    for x in &ducking.sources {
        if let Some(m) = sounding(p, x) {
            sounds.push((span_of(x, fps), env.audio_levels(m, x.stream)?, x));
        }
    }
    let heard: Vec<Heard> = sounds
        .iter()
        .map(|(span, levels, x)| Heard {
            span,
            levels,
            gain: gain_of(x, f),
        })
        .collect();
    let every = f64::from(GRAPH_POINTS_PER_SECOND);
    let points = ((b - a) as f64 / f * every).ceil() as usize;
    let mut out = Graph {
        source_db: Vec::with_capacity(points),
        duck_db: Vec::with_capacity(points),
    };
    for i in 0..points {
        let secs = i as f64 / every;
        let peak = heard
            .iter()
            .filter_map(|h| h.peak(a as f64 + secs * f))
            .fold(0.0_f64, f64::max);
        out.source_db
            .push((peak > 0.0).then(|| hundredths(20.0 * peak.log10())));
        out.duck_db.push(hundredths(
            ducking.dip.gain_db((secs * rate).round() as i64),
        ));
    }
    Ok(out)
}
