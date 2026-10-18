use std::collections::HashSet;

use crate::error::{Error, Result};
use crate::model::{Cue, Place, Project, SubtitleTrack};
use crate::subtitles::{self, Read, Timed};

pub const DEFAULT_SECS: f64 = 3.0;
const HOUR: f64 = 3600.0;

fn renumber(p: &mut Project) {
    for (i, t) in p.subtitles.iter_mut().enumerate() {
        t.id = format!("S{}", i + 1);
    }
}

pub fn add_track(p: &mut Project, name: Option<String>) -> String {
    let id = format!("S{}", p.subtitles.len() + 1);
    let mut t = SubtitleTrack::new(id.clone());
    t.name = name;
    t.shown = true;
    for o in &mut p.subtitles {
        o.shown = false;
    }
    p.subtitles.push(t);
    id
}

fn index(p: &Project, id: &str) -> Result<usize> {
    p.subtitle_index(id)
        .ok_or_else(|| Error::not_found(format!("no subtitle track {id}")))
}

fn unlocked(p: &Project, ti: usize) -> Result<()> {
    let t = &p.subtitles[ti];
    if t.locked {
        return Err(Error::invalid(format!("subtitle track {} is locked", t.id)));
    }
    Ok(())
}

fn find(p: &Project, id: &str) -> Result<(usize, usize)> {
    p.find_cue(id)
        .ok_or_else(|| Error::not_found(format!("no cue {id}")))
}

fn place_cue(t: &mut SubtitleTrack, cue: Cue) {
    let key = (cue.start, cue.end());
    let at = t.cues.partition_point(|c| (c.start, c.end()) <= key);
    t.cues.insert(at, cue);
}

pub fn remove_track(p: &mut Project, id: &str, force: bool) -> Result<()> {
    let ti = index(p, id)?;
    unlocked(p, ti)?;
    let n = p.subtitles[ti].cues.len();
    if n > 0 && !force {
        return Err(Error::invalid(format!(
            "{id} has {n} cue(s); pass force to delete them too"
        )));
    }
    p.subtitles.remove(ti);
    renumber(p);
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackProps {
    pub locked: Option<bool>,
    pub name: Option<Option<String>>,
    pub shown: Option<bool>,
    pub language: Option<Option<String>>,
}

pub fn set_track(p: &mut Project, id: &str, props: &TrackProps) -> Result<()> {
    let ti = index(p, id)?;
    match props.shown {
        Some(true) => {
            for (i, t) in p.subtitles.iter_mut().enumerate() {
                t.shown = i == ti;
            }
        }
        Some(false) => p.subtitles[ti].shown = false,
        None => {}
    }
    let t = &mut p.subtitles[ti];
    if let Some(l) = props.locked {
        t.locked = l;
    }
    if let Some(n) = &props.name {
        t.name = n.clone();
    }
    if let Some(l) = &props.language {
        t.language = l.clone();
    }
    Ok(())
}

pub fn target(p: &mut Project, track: Option<&str>) -> Result<usize> {
    if let Some(id) = track {
        if let Some(i) = p.subtitle_index(id) {
            return Ok(i);
        }
        if id == format!("S{}", p.subtitles.len() + 1) {
            add_track(p, None);
            return Ok(p.subtitles.len() - 1);
        }
        return Err(Error::not_found(format!(
            "no subtitle track {id} (the next new one is S{})",
            p.subtitles.len() + 1
        )));
    }
    if let Some(i) = p.subtitles.iter().position(|t| t.shown) {
        return Ok(i);
    }
    if p.subtitles.is_empty() {
        add_track(p, None);
    }
    Ok(0)
}

pub fn shown(p: &Project) -> Result<usize> {
    p.subtitles
        .iter()
        .position(|t| t.shown)
        .or_else(|| (p.subtitles.len() == 1).then_some(0))
        .ok_or_else(|| {
            Error::usage(if p.subtitles.is_empty() {
                "the project has no subtitle track".to_string()
            } else {
                "no subtitle track is shown; name one with track".to_string()
            })
        })
}

pub fn cue_at(t: &SubtitleTrack, frame: i64) -> Option<usize> {
    t.cues
        .iter()
        .rposition(|c| c.start <= frame && frame < c.end())
}

fn default_length(p: &Project, ti: usize, start: i64) -> i64 {
    let full = (DEFAULT_SECS * p.settings.fps.as_f64()).round() as i64;
    let next = p.subtitles[ti]
        .cues
        .iter()
        .map(|c| c.start)
        .filter(|&s| s > start)
        .min();
    next.map_or(full, |n| full.min(n - start)).max(1)
}

pub fn add_cue(
    p: &mut Project,
    ti: usize,
    start: i64,
    end: Option<i64>,
    duration: Option<i64>,
    text: &str,
    place: Option<Place>,
) -> Result<String> {
    unlocked(p, ti)?;
    if start < 0 {
        return Err(Error::invalid("a cue can't start before 0"));
    }
    let duration = match (end, duration) {
        (Some(e), _) => e - start,
        (None, Some(d)) => d,
        (None, None) => default_length(p, ti, start),
    };
    if duration < 1 {
        return Err(Error::invalid("a cue lasts at least one frame"));
    }
    let id = p.alloc_cue_id();
    place_cue(
        &mut p.subtitles[ti],
        Cue {
            id: id.clone(),
            start,
            duration,
            text: subtitles::normalize(text),
            place: place.unwrap_or_default(),
        },
    );
    Ok(id)
}

pub fn set_cue(
    p: &mut Project,
    id: &str,
    text: Option<&str>,
    start: Option<i64>,
    end: Option<i64>,
    place: Option<Place>,
) -> Result<()> {
    let (ti, ci) = find(p, id)?;
    unlocked(p, ti)?;
    let c = &mut p.subtitles[ti].cues[ci];
    let (s, e) = (start.unwrap_or(c.start), end.unwrap_or(c.end()));
    if s < 0 {
        return Err(Error::invalid("a cue can't start before 0"));
    }
    if e <= s {
        return Err(Error::invalid(format!(
            "{id} would end at frame {e}, before it starts at {s}"
        )));
    }
    c.start = s;
    c.duration = e - s;
    if let Some(t) = text {
        c.text = subtitles::normalize(t);
    }
    if let Some(pl) = place {
        c.place = pl;
    }
    p.subtitles[ti].sort();
    Ok(())
}

fn take(p: &mut Project, ids: &[String]) -> Result<Vec<(usize, Cue)>> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        let (ti, _) = find(p, id)?;
        unlocked(p, ti)?;
        found.push((ti, id.clone()));
    }
    let doomed: HashSet<&str> = found.iter().map(|(_, id)| id.as_str()).collect();
    let mut taken = Vec::new();
    for (ti, t) in p.subtitles.iter_mut().enumerate() {
        let (out, keep): (Vec<Cue>, Vec<Cue>) = std::mem::take(&mut t.cues)
            .into_iter()
            .partition(|c| doomed.contains(c.id.as_str()));
        t.cues = keep;
        taken.extend(out.into_iter().map(|c| (ti, c)));
    }
    Ok(taken)
}

pub fn move_cues(
    p: &mut Project,
    ids: &[String],
    by: Option<i64>,
    to: Option<i64>,
    track: Option<&str>,
) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::usage("no cues to move"));
    }
    let dest = match track {
        Some(id) => {
            let i = index(p, id)?;
            unlocked(p, i)?;
            Some(i)
        }
        None => None,
    };
    let mut first = i64::MAX;
    for id in ids {
        let (ti, ci) = find(p, id)?;
        first = first.min(p.subtitles[ti].cues[ci].start);
    }
    let delta = match (by, to) {
        (Some(b), None) => b,
        (None, Some(t)) => t - first,
        (None, None) if dest.is_some() => 0,
        _ => return Err(Error::usage("give by or to")),
    };
    if first + delta < 0 {
        return Err(Error::invalid("the cues would start before 0"));
    }
    for (ti, mut c) in take(p, ids)? {
        c.start += delta;
        place_cue(&mut p.subtitles[dest.unwrap_or(ti)], c);
    }
    Ok(())
}

pub fn remove_cues(p: &mut Project, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::usage("no cues to remove"));
    }
    for id in ids {
        let (ti, _) = find(p, id)?;
        unlocked(p, ti)?;
    }
    take(p, ids).map(drop)
}

pub fn split_cue(p: &mut Project, id: &str, at: i64) -> Result<String> {
    let (ti, ci) = find(p, id)?;
    unlocked(p, ti)?;
    let c = &p.subtitles[ti].cues[ci];
    if !(c.start < at && at < c.end()) {
        return Err(Error::invalid(format!(
            "{id} runs from frame {} to {}; it can't be split at {at}",
            c.start,
            c.end()
        )));
    }
    let mut right = c.clone();
    let new_id = p.alloc_cue_id();
    right.id = new_id.clone();
    right.duration = right.end() - at;
    right.start = at;
    let t = &mut p.subtitles[ti];
    t.cues[ci].duration = at - t.cues[ci].start;
    place_cue(t, right);
    Ok(new_id)
}

pub fn join_cues(p: &mut Project, ids: &[String]) -> Result<String> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for id in ids {
        let at = find(p, id)?;
        if seen.insert(at) {
            found.push(at);
        }
    }
    if found.len() < 2 {
        return Err(Error::usage("join takes two cues or more"));
    }
    let ti = found[0].0;
    if found.iter().any(|(t, _)| *t != ti) {
        return Err(Error::invalid("the cues to join are on different tracks"));
    }
    unlocked(p, ti)?;
    let mut cues: Vec<Cue> = found
        .iter()
        .map(|(_, ci)| p.subtitles[ti].cues[*ci].clone())
        .collect();
    cues.sort_by_key(|c| (c.start, c.end()));
    let end = cues.iter().map(Cue::end).max().expect("two cues");
    let text = cues
        .iter()
        .map(|c| c.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut joined = cues[0].clone();
    joined.duration = end - joined.start;
    joined.text = subtitles::normalize(&text);
    let ids: Vec<String> = cues.iter().map(|c| c.id.clone()).collect();
    take(p, &ids)?;
    let id = joined.id.clone();
    place_cue(&mut p.subtitles[ti], joined);
    Ok(id)
}

pub struct Imported {
    pub track: String,
    pub cues: usize,
    pub shift: f64,
    pub before_zero: usize,
}

fn timecode_shift(p: &Project, first: f64, forced: bool) -> f64 {
    let fps = p.settings.fps.as_f64();
    let slack = 0.5 / fps;
    let origin = subtitles::origin(&p.settings);
    if origin > 0.0 {
        return if forced || first + slack >= origin {
            origin
        } else {
            0.0
        };
    }
    let short = (p.end() as f64) / fps <= HOUR;
    if first + slack >= HOUR && (forced || short) {
        HOUR
    } else {
        0.0
    }
}

pub fn import(
    p: &mut Project,
    read: Read,
    track: Option<&str>,
    name: Option<String>,
    at: Option<i64>,
    timecode: Option<bool>,
) -> Result<Imported> {
    let fps = p.settings.fps.as_f64();
    let Some(first) = read.cues.iter().map(|c| c.start).reduce(f64::min) else {
        return Err(Error::invalid("the file has no cues"));
    };
    let shift = match (at, timecode) {
        (Some(at), _) => first - at as f64 / fps,
        (None, Some(false)) => 0.0,
        (None, forced) => timecode_shift(p, first, forced == Some(true)),
    };
    let ti = match track {
        Some(id) => target(p, Some(id))?,
        None => {
            add_track(p, name);
            p.subtitles.len() - 1
        }
    };
    unlocked(p, ti)?;
    let mut count = 0;
    let mut before_zero = 0;
    for c in read.cues {
        let s = ((c.start - shift) * fps).round() as i64;
        let e = ((c.end - shift) * fps).round() as i64;
        if e <= 0 {
            before_zero += 1;
            continue;
        }
        let s = s.max(0);
        let id = p.alloc_cue_id();
        place_cue(
            &mut p.subtitles[ti],
            Cue {
                id,
                start: s,
                duration: (e - s).max(1),
                text: subtitles::normalize(&c.text),
                place: c.place,
            },
        );
        count += 1;
    }
    Ok(Imported {
        track: p.subtitles[ti].id.clone(),
        cues: count,
        shift,
        before_zero,
    })
}

pub fn collect(p: &Project, ti: usize, range: (i64, i64), timecode: bool) -> Vec<Timed> {
    let fps = p.settings.fps.as_f64();
    let (a, b) = range;
    let (zero, base) = if timecode {
        (0, subtitles::origin(&p.settings))
    } else {
        (a, 0.0)
    };
    p.subtitles[ti]
        .cues
        .iter()
        .filter(|c| c.start < b && c.end() > a && !c.text.trim().is_empty())
        .map(|c| Timed {
            start: base + (c.start.max(a) - zero) as f64 / fps,
            end: base + (c.end().min(b) - zero) as f64 / fps,
            text: c.text.clone(),
            place: c.place,
        })
        .collect()
}

pub fn remap(p: &mut Project, map: impl Fn(i64) -> Option<i64>) {
    let mut extra: Vec<(usize, Cue)> = Vec::new();
    for (ti, t) in p.subtitles.iter_mut().enumerate() {
        if t.locked {
            continue;
        }
        let mut kept = Vec::with_capacity(t.cues.len());
        for c in std::mem::take(&mut t.cues) {
            let images: Vec<Option<i64>> = (c.start..=c.end()).map(&map).collect();
            let mut frames: Vec<i64> = images
                .windows(2)
                .filter_map(|w| match (w[0], w[1]) {
                    (Some(x), next) if next != Some(x) => Some(x),
                    _ => None,
                })
                .collect();
            frames.sort_unstable();
            frames.dedup();
            let mut runs: Vec<(i64, i64)> = Vec::new();
            for f in frames {
                match runs.last_mut() {
                    Some((_, end)) if *end == f => *end += 1,
                    _ => runs.push((f, f + 1)),
                }
            }
            for (k, (s, e)) in runs.into_iter().enumerate() {
                let mut piece = c.clone();
                piece.start = s.max(0);
                piece.duration = e - piece.start;
                if piece.duration < 1 {
                    continue;
                }
                if k == 0 {
                    kept.push(piece);
                } else {
                    extra.push((ti, piece));
                }
            }
        }
        t.cues = kept;
        t.sort();
    }
    for (ti, mut c) in extra {
        c.id = p.alloc_cue_id();
        place_cue(&mut p.subtitles[ti], c);
    }
}
