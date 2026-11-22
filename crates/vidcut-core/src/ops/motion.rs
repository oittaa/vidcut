use std::sync::Arc;

use serde_json::{Value, json};

use crate::composition::Frames;
use crate::error::{Error, Result};
use crate::model::{Clip, MotionPath, PathSample, Pattern, Project, Subject, Tracker};
use crate::motion::{self, Area};
use crate::render::FileClock;

use super::locate;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PathChange<'a> {
    pub samples: Option<&'a [PathSample]>,
    pub clear: Option<[i64; 2]>,
}

pub fn takes(c: &Clip) -> bool {
    c.video && !c.is_block()
}

fn no_picture(id: &str) -> Error {
    Error::invalid(format!(
        "{id} has no source picture: a motion path goes on footage, a still or an image sequence"
    ))
}

fn clip_of<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut Clip> {
    let (ti, ci) = locate(p, id)?;
    let c = &mut p.tracks[ti].clips[ci];
    if !takes(c) {
        return Err(no_picture(id));
    }
    Ok(c)
}

fn no_tracker(id: &str, tracker: &str) -> Error {
    Error::invalid(format!(
        "{id} has no tracker {tracker}: a motion path is on a pattern of a tracker of its \
         composition"
    ))
}

fn tracker_of<'a>(c: &'a Clip, tracker: &str) -> Result<&'a Tracker> {
    let comp = c.picture.as_ref().and_then(|x| x.composition.as_ref());
    let t = comp.and_then(|x| x.node(tracker)?.tool.tracker());
    t.ok_or_else(|| no_tracker(&c.id, tracker))
}

fn tracker_mut<'a>(c: &'a mut Clip, tracker: &str) -> Result<&'a mut Tracker> {
    let id = c.id.clone();
    let comp = c.picture.as_mut().and_then(|x| x.composition.as_mut());
    let node = comp.and_then(|x| x.nodes.iter_mut().find(|n| n.name == tracker));
    node.and_then(|n| n.tool.tracker_mut())
        .ok_or_else(|| no_tracker(&id, tracker))
}

fn index(t: &Tracker, tracker: &str, k: u32) -> Result<usize> {
    let i = (k as usize).checked_sub(1);
    i.filter(|i| *i < t.patterns.len())
        .ok_or_else(|| no_pattern(tracker, k, t.patterns.len()))
}

pub(super) fn no_pattern(tracker: &str, k: u32, count: usize) -> Error {
    let have = match count {
        0 => "it has no patterns".to_string(),
        1 => "it has pattern 1".to_string(),
        all => format!("its patterns are 1 to {all}"),
    };
    Error::invalid(format!("{tracker} has no pattern {k}: {have}"))
}

pub(super) fn pattern_of<'a>(c: &'a Clip, tracker: &str, k: u32) -> Result<&'a Pattern> {
    let t = tracker_of(c, tracker)?;
    Ok(&t.patterns[index(t, tracker, k)?])
}

fn pattern_mut<'a>(c: &'a mut Clip, tracker: &str, k: u32) -> Result<&'a mut Pattern> {
    let t = tracker_mut(c, tracker)?;
    let i = index(t, tracker, k)?;
    Ok(&mut t.patterns[i])
}

fn named(name: &str) -> Option<String> {
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

fn trackers(c: &Clip) -> impl Iterator<Item = (&str, &Tracker)> {
    let comp = c.picture.iter().flat_map(|x| x.composition.iter());
    let nodes = comp.flat_map(|k| &k.nodes);
    nodes.filter_map(|n| Some((n.name.as_str(), n.tool.tracker()?)))
}

fn paths(c: &Clip) -> impl Iterator<Item = &MotionPath> {
    let patterns = trackers(c).flat_map(|(_, t)| &t.patterns);
    patterns.filter_map(|x| x.path.as_ref())
}

pub fn add(
    p: &mut Project,
    id: &str,
    tracker: &str,
    name: Option<&str>,
    samples: &[PathSample],
) -> Result<Value> {
    if let Some(why) = motion::samples_problem(samples) {
        return Err(Error::invalid(why));
    }
    let t = tracker_mut(clip_of(p, id)?, tracker)?;
    if t.patterns.len() >= Tracker::PATTERNS_KEPT {
        return Err(Error::invalid(format!(
            "{tracker} of {id} has {} patterns, the most a tracker takes",
            Tracker::PATTERNS_KEPT
        )));
    }
    let name = name.and_then(named).unwrap_or_else(|| t.free_name());
    t.patterns.push(Pattern {
        path: Some(MotionPath {
            subject: None,
            tracked: false,
            samples: Arc::new(samples.to_vec()),
        }),
        ..Pattern::new(name)
    });
    Ok(json!({ "clip": id, "node": tracker, "pattern": t.patterns.len() }))
}

pub fn set(p: &mut Project, id: &str, (tracker, k): (&str, u32), change: PathChange) -> Result<()> {
    if change == PathChange::default() {
        return Err(Error::usage("motion.set needs samples or a range to clear"));
    }
    if let Some([first, last]) = change.clear
        && first > last
    {
        return Err(Error::usage(format!(
            "clear is two frames of the file in order, not {first} then {last}"
        )));
    }
    if let Some(why) = change
        .samples
        .filter(|s| !s.is_empty())
        .and_then(motion::samples_problem)
    {
        return Err(Error::invalid(why));
    }
    let pattern = pattern_mut(clip_of(p, id)?, tracker, k)?;
    let mut samples = (pattern.path.as_ref()).map_or_else(Vec::new, |m| m.samples.to_vec());
    if let Some([first, last]) = change.clear {
        motion::clear(&mut samples, (first, last));
    }
    if let Some(new) = change.samples {
        samples = motion::put(&samples, new);
    }
    if samples.is_empty() {
        return Err(Error::invalid(format!(
            "that leaves pattern {k} of {tracker} of {id} no samples: motion.remove takes a path \
             off"
        )));
    }
    if let Some(why) = motion::samples_problem(&samples) {
        return Err(Error::invalid(why));
    }
    match pattern.path.as_mut() {
        Some(m) => m.samples = Arc::new(samples),
        None => {
            pattern.path = Some(MotionPath {
                subject: None,
                tracked: false,
                samples: Arc::new(samples),
            });
        }
    }
    Ok(())
}

pub fn remove(p: &mut Project, id: &str, tracker: &str, patterns: Option<&[u32]>) -> Result<()> {
    let t = tracker_mut(clip_of(p, id)?, tracker)?;
    let picked: Vec<usize> = match patterns {
        Some(ks) => (ks.iter().map(|k| index(t, tracker, *k))).collect::<Result<_>>()?,
        None => (0..t.patterns.len())
            .filter(|i| t.patterns[*i].path.is_some())
            .collect(),
    };
    if picked.is_empty() {
        return Err(Error::not_found(format!(
            "{tracker} of {id} has no motion paths"
        )));
    }
    if let Some(i) = picked.iter().find(|i| t.patterns[**i].path.is_none()) {
        return Err(Error::not_found(format!(
            "pattern {} of {tracker} of {id} has no motion path",
            i + 1
        )));
    }
    for i in picked {
        t.patterns[i].path = None;
    }
    Ok(())
}

pub fn shift(c: &mut Clip, by: i64) {
    if by == 0 {
        return;
    }
    let comp = c.picture.as_mut().and_then(|x| x.composition.as_mut());
    let nodes = comp.into_iter().flat_map(|k| k.nodes.iter_mut());
    let patterns = nodes
        .filter_map(|n| n.tool.tracker_mut())
        .flat_map(|t| t.patterns.iter_mut());
    for m in patterns.filter_map(|x| x.path.as_mut()) {
        for s in Arc::make_mut(&mut m.samples) {
            s.frame += by;
        }
    }
}

fn shown(k: usize, pattern: &Pattern, at: Option<i64>, samples: bool) -> Value {
    let mut v = json!({ "pattern": k + 1, "name": pattern.name, "count": 0 });
    if let Some(m) = &pattern.path {
        v["count"] = json!(m.samples.len());
        if let Some(subject) = m.subject {
            v["subject"] = json!(subject);
        }
        if m.tracked {
            v["tracked"] = json!(true);
        }
        if let Some((first, last)) = m.span() {
            v["first"] = json!(first);
            v["last"] = json!(last);
        }
    }
    if let Some(frame) = at
        && let Some(area) = pattern.area_at(frame as f64)
    {
        v["area"] = json!({
            "frame": frame,
            "center": area.center,
            "size": area.size,
            "rotation": area.rotation,
        });
    }
    if samples && let Some(m) = &pattern.path {
        v["samples"] = json!(m.samples);
    }
    v
}

pub fn list(p: &Project, ids: Option<&[String]>, at: Option<i64>, samples: bool) -> Result<Value> {
    for id in ids.into_iter().flatten() {
        if p.clip(id).is_none() {
            return Err(Error::not_found(format!("no clip {id}")));
        }
    }
    let mut clips = Vec::new();
    for track in &p.tracks {
        for c in &track.clips {
            let wanted = match ids {
                Some(ids) => ids.contains(&c.id),
                None => trackers(c).next().is_some(),
            };
            if !wanted {
                continue;
            }
            let clock = FileClock::of(p, c);
            let frame = at.map(|t| clock.frame(c.content_at(t)));
            let nodes: Vec<Value> = trackers(c)
                .map(|(name, t)| {
                    let patterns = t.patterns.iter().enumerate();
                    let patterns: Vec<Value> =
                        patterns.map(|(k, x)| shown(k, x, frame, samples)).collect();
                    json!({ "node": name, "patterns": patterns })
                })
                .collect();
            clips.push(json!({ "clip": c.id, "track": track.id, "trackers": nodes }));
        }
    }
    Ok(json!({ "clips": clips }))
}

pub fn violation(c: &Clip) -> Option<String> {
    if paths(c).next().is_some() && !takes(c) {
        return Some(format!(
            "{}: motion paths on a clip with no source picture",
            c.id
        ));
    }
    let why = paths(c).find_map(|m| motion::samples_problem(&m.samples));
    why.map(|why| format!("{}: {why}", c.id))
}

fn of_subject(t: &Tracker, subject: Subject) -> Vec<usize> {
    let found = |i: &usize| {
        let path = t.patterns[*i].path.as_ref();
        path.is_some_and(|m| m.subject == Some(subject))
    };
    (0..t.patterns.len()).filter(found).collect()
}

pub fn findable(
    p: &Project,
    id: &str,
    tracker: &str,
    subject: Subject,
    replace: bool,
) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    if !takes(c) {
        return Err(no_picture(id));
    }
    let old = of_subject(tracker_of(c, tracker)?, subject);
    if !old.is_empty() && !replace {
        return Err(Error::invalid(format!(
            "{tracker} of {id} already has {} {} patterns: pass replace (--replace) to find them \
             again",
            old.len(),
            subject.name()
        )));
    }
    Ok(())
}

pub fn file_frames(p: &Project, c: &Clip) -> ((i64, i64), f64) {
    let clock = FileClock::of(p, c);
    let (lo, hi) = c.shown();
    let rate = clock.rate(p.fps().as_f64());
    ((clock.file(lo), clock.file(hi)), rate)
}

const SAME_THING: f64 = 0.5;

fn cover(a: &Area, b: &PathSample) -> f64 {
    let side = |i: usize| {
        let (a0, a1) = (a.center[i] - a.size[i] / 2.0, a.center[i] + a.size[i] / 2.0);
        let (b0, b1) = (b.center[i] - b.size[i] / 2.0, b.center[i] + b.size[i] / 2.0);
        (a1.min(b1) - a0.max(b0)).max(0.0)
    };
    let both = side(0) * side(1);
    let all = a.size[0] * a.size[1] + b.size[0] * b.size[1] - both;
    if all > 0.0 { both / all } else { 0.0 }
}

fn merged(was: &[PathSample], now: &[PathSample]) -> Vec<PathSample> {
    let (Some(first), Some(last)) = (now.first(), now.last()) else {
        return was.to_vec();
    };
    let before = was.iter().filter(|s| s.frame < first.frame);
    let after = was.iter().filter(|s| s.frame > last.frame);
    before.chain(now).chain(after).copied().collect()
}

fn same_thing(kept: &MotionPath, found: &[PathSample]) -> bool {
    let shared: Vec<f64> = found
        .iter()
        .filter_map(|s| Some(cover(&kept.base_at(s.frame as f64)?, s)))
        .collect();
    let same = shared.iter().filter(|v| **v >= SAME_THING).count();
    same * 2 > shared.len()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Start {
    pub frames: (i64, i64),
    pub reach: Option<(i64, i64)>,
    pub base: i64,
    pub frame: i64,
    pub own: Area,
    pub bounds: [f64; 4],
}

pub fn start(
    p: &Project,
    id: &str,
    (tracker, k): (&str, u32),
    (frame, length): (i64, Option<u32>),
) -> Result<Start> {
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    let media = p.media(&c.media).filter(|m| takes(c) && !m.is_still());
    if media.is_none() {
        return Err(Error::invalid(format!(
            "{id} has no moving source picture: a track follows a thing in footage or an image \
             sequence"
        )));
    }
    let f = Frames::of(p, c);
    if !f.inside(frame) {
        return Err(Error::invalid(format!(
            "frame {frame} is not between the In ({}) and the Out ({}) of {id}: a track starts \
             there",
            f.mark_in, f.mark_out
        )));
    }
    if length == Some(0) {
        return Err(Error::usage("a track's length is from 1"));
    }
    let frames = (f.file(f.mark_in), f.file(f.mark_out) + 1);
    let reach = length.map(i64::from).map(|n| {
        let lo = f.file((frame - n).max(f.mark_in));
        (lo, f.file((frame + n).min(f.mark_out)) + 1)
    });
    let at = f.file(frame);
    let pattern = pattern_of(c, tracker, k)?;
    let own = pattern.area_at(at as f64);
    let own = own.unwrap_or_else(|| pattern.own());
    Ok(Start {
        frames,
        reach,
        base: f.file(f.first),
        frame: at,
        own,
        bounds: [own.center[0], own.center[1], own.size[0], own.size[1]],
    })
}

pub fn tracked(
    p: &mut Project,
    id: &str,
    (tracker, k): (&str, u32),
    mut samples: Vec<PathSample>,
) -> Result<Value> {
    let pattern = pattern_mut(clip_of(p, id)?, tracker, k)?;
    if let Some(old) = &pattern.path {
        let frames = samples.iter().map(|s| s.frame);
        let covered = frames.clone().min().zip(frames.max());
        let stays = |s: &&PathSample| covered.is_none_or(|(a, b)| s.frame < a || s.frame > b);
        let edges = covered.into_iter().flat_map(|(a, b)| [a - 1, b + 1]);
        let pinned: Vec<PathSample> = edges
            .filter(|f| !old.samples.iter().any(|s| s.frame == *f))
            .filter_map(|f| Some(super::comp::sample_of(f, &old.area_at(f as f64)?)))
            .collect();
        samples.extend(old.samples.iter().filter(stays).cloned());
        samples.extend(pinned);
        samples.sort_by_key(|s| s.frame);
    }
    let samples = motion::simplified(&samples);
    if let Some(why) = motion::samples_problem(&samples) {
        return Err(Error::invalid(why));
    }
    pattern.path = Some(MotionPath {
        subject: None,
        tracked: true,
        samples: Arc::new(samples),
    });
    let mut v = shown(k as usize - 1, pattern, None, false);
    v["node"] = json!(tracker);
    Ok(v)
}

pub fn found(
    p: &mut Project,
    id: &str,
    (subject, tracker): (Subject, &str),
    tracks: Vec<motion::FoundPath>,
) -> Result<Value> {
    let t = tracker_mut(clip_of(p, id)?, tracker)?;
    let old = of_subject(t, subject);
    let mut fresh = Vec::new();
    for track in tracks {
        let same = |i: &&usize| {
            let path = t.patterns[**i].path.as_ref();
            path.is_some_and(|m| same_thing(m, &track.samples))
        };
        let stays = old.iter().find(same).copied();
        match stays.and_then(|i| t.patterns[i].path.as_mut()) {
            Some(m) => m.samples = Arc::new(merged(&m.samples, &track.samples)),
            None => fresh.push(track),
        }
    }
    if t.patterns.len() + fresh.len() > Tracker::PATTERNS_KEPT {
        return Err(Error::invalid(format!(
            "{tracker} of {id} would have {} patterns, more than the {} a tracker takes",
            t.patterns.len() + fresh.len(),
            Tracker::PATTERNS_KEPT
        )));
    }
    let mut patterns = Vec::new();
    let mut n = 0;
    for track in fresh {
        let name = loop {
            n += 1;
            let name = format!("{} {n}", subject.title());
            if !t.patterns.iter().any(|x| x.name == name) {
                break name;
            }
        };
        t.patterns.push(Pattern {
            path: Some(MotionPath {
                subject: Some(subject),
                tracked: false,
                samples: Arc::new(track.samples),
            }),
            ..Pattern::new(name)
        });
        let k = t.patterns.len() - 1;
        let mut v = shown(k, &t.patterns[k], None, false);
        v["seen"] = json!(track.seen);
        v["score"] = json!(track.score);
        v["gaps"] = json!(track.gaps);
        patterns.push(v);
    }
    Ok(json!({ "clip": id, "node": tracker, "patterns": patterns }))
}
