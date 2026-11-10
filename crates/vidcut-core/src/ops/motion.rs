use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, EffectArea, MotionPath, PathSample, Project, Subject, TrackWay};
use crate::motion::{self, Area};

use super::locate;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PathChange<'a> {
    pub name: Option<&'a str>,
    pub samples: Option<&'a [PathSample]>,
    pub clear: Option<[i64; 2]>,
}

pub fn takes(c: &Clip) -> bool {
    c.video && !c.is_block()
}

fn clip_of<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut Clip> {
    let (ti, ci) = locate(p, id)?;
    let c = &mut p.tracks[ti].clips[ci];
    if !takes(c) {
        return Err(Error::invalid(format!(
            "{id} has no source picture: a motion path goes on footage, a still or an image \
             sequence"
        )));
    }
    Ok(c)
}

fn path_of<'a>(c: &'a mut Clip, path: &str) -> Result<&'a mut MotionPath> {
    let id = c.id.clone();
    c.motion
        .iter_mut()
        .find(|m| m.id == path)
        .ok_or_else(|| Error::not_found(format!("{id} has no motion path {path}")))
}

fn named(name: &str) -> Option<String> {
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

pub fn add(p: &mut Project, id: &str, name: Option<&str>, samples: &[PathSample]) -> Result<Value> {
    if let Some(why) = motion::samples_problem(samples) {
        return Err(Error::invalid(why));
    }
    let c = clip_of(p, id)?;
    if c.motion.len() >= motion::PATHS_KEPT {
        return Err(Error::invalid(format!(
            "{id} has {} motion paths, the most a clip takes",
            motion::PATHS_KEPT
        )));
    }
    let path = motion::next_id(&c.motion);
    c.motion.push(MotionPath {
        id: path.clone(),
        name: name.and_then(named),
        subject: None,
        samples: Arc::new(samples.to_vec()),
    });
    Ok(json!({ "clip": id, "path": path }))
}

pub fn set(p: &mut Project, id: &str, path: &str, change: PathChange) -> Result<()> {
    if change == PathChange::default() {
        return Err(Error::usage(
            "motion.set needs a name, samples or a range to clear",
        ));
    }
    if let Some([first, last]) = change.clear
        && first > last
    {
        return Err(Error::usage(format!(
            "clear is two source frames in order, not {first} then {last}"
        )));
    }
    if let Some(why) = change
        .samples
        .filter(|s| !s.is_empty())
        .and_then(motion::samples_problem)
    {
        return Err(Error::invalid(why));
    }
    let m = path_of(clip_of(p, id)?, path)?;
    let mut samples = m.samples.to_vec();
    if let Some([first, last]) = change.clear {
        motion::clear(&mut samples, (first, last));
    }
    if let Some(new) = change.samples {
        samples = motion::put(&samples, new);
    }
    if samples.is_empty() {
        return Err(Error::invalid(format!(
            "that leaves {path} of {id} no samples: motion.remove takes a path off"
        )));
    }
    if let Some(why) = motion::samples_problem(&samples) {
        return Err(Error::invalid(why));
    }
    m.samples = Arc::new(samples);
    if let Some(name) = change.name {
        m.name = named(name);
    }
    Ok(())
}

fn followers(c: &Clip) -> impl Iterator<Item = (usize, &str)> {
    let effects = c.picture.iter().flat_map(|x| x.effects.iter().enumerate());
    effects.filter_map(|(n, e)| Some((n, e.area.as_ref()?.follows.as_deref()?)))
}

pub fn remove(p: &mut Project, id: &str, paths: Option<&[String]>) -> Result<()> {
    let c = clip_of(p, id)?;
    let all: Vec<String> = c.motion.iter().map(|m| m.id.clone()).collect();
    if paths.is_none() && all.is_empty() {
        return Err(Error::not_found(format!("{id} has no motion paths")));
    }
    let paths = paths.unwrap_or(&all);
    for path in paths {
        path_of(c, path)?;
    }
    if let Some((n, path)) = followers(c).find(|(_, path)| paths.iter().any(|x| x == path)) {
        return Err(Error::invalid(format!(
            "the area of effect {n} of {id} follows motion path {path}: first give that area \
             another `follows`, or none (effects.set)"
        )));
    }
    c.motion.retain(|m| !paths.contains(&m.id));
    Ok(())
}

pub fn shift(c: &mut Clip, by: i64) {
    for m in &mut c.motion {
        for s in Arc::make_mut(&mut m.samples) {
            s.frame += by;
        }
    }
}

fn shown(c: &Clip, m: &MotionPath, at: Option<i64>, samples: bool) -> Value {
    let mut v = json!({ "id": m.id, "count": m.samples.len() });
    if let Some(name) = &m.name {
        v["name"] = json!(name);
    }
    if let Some(subject) = m.subject {
        v["subject"] = json!(subject);
    }
    if let Some((first, last)) = m.span() {
        v["first"] = json!(first);
        v["last"] = json!(last);
    }
    if let Some(at) = at {
        let frame = c.source_at(at);
        if let Some(area) = m.area_at(frame as f64) {
            v["area"] = json!({
                "frame": frame,
                "center": area.center,
                "size": area.size,
                "rotation": area.rotation,
            });
        }
    }
    if samples {
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
    for t in &p.tracks {
        for c in &t.clips {
            let wanted = match ids {
                Some(ids) => ids.contains(&c.id),
                None => !c.motion.is_empty(),
            };
            if wanted {
                let paths: Vec<Value> = c.motion.iter().map(|m| shown(c, m, at, samples)).collect();
                clips.push(json!({ "clip": c.id, "track": t.id, "paths": paths }));
            }
        }
    }
    Ok(json!({ "clips": clips }))
}

pub fn violation(c: &Clip) -> Option<String> {
    let has = |path: &str| Subject::named(path).is_some() || c.motion.iter().any(|m| m.id == path);
    if let Some((n, path)) = followers(c).find(|(_, path)| !has(path)) {
        return Some(format!(
            "{}: the area of effect {n} follows motion path {path}, which the clip has not",
            c.id
        ));
    }
    if c.motion.is_empty() {
        return None;
    }
    if !takes(c) {
        return Some(format!(
            "{}: motion paths on a clip with no source picture",
            c.id
        ));
    }
    motion::problem(&c.motion).map(|why| format!("{}: {why}", c.id))
}

fn of_subject(c: &Clip, subject: Subject) -> Vec<String> {
    let found = c.motion.iter().filter(|m| m.subject == Some(subject));
    found.map(|m| m.id.clone()).collect()
}

pub fn findable(p: &mut Project, id: &str, subject: Subject, replace: bool) -> Result<(i64, i64)> {
    let c = clip_of(p, id)?;
    let old = of_subject(c, subject);
    if !old.is_empty() && !replace {
        return Err(Error::invalid(format!(
            "{id} already has {} {} paths: pass replace (--replace) to find them again",
            old.len(),
            subject.name()
        )));
    }
    if let Some((n, path)) = followers(c).find(|(_, path)| old.iter().any(|x| x == path)) {
        return Err(Error::invalid(format!(
            "the area of effect {n} of {id} follows motion path {path}: first give that area \
             another `follows`, or none (effects.set)"
        )));
    }
    Ok(c.shown())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Start {
    pub frames: (i64, i64),
    pub frame: i64,
    pub own: Area,
    pub bounds: [f64; 4],
    pub fit: [f64; 2],
}

pub fn start(p: &Project, id: &str, effect: usize, at: i64) -> Result<Start> {
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    let media = p.media(&c.media).filter(|m| takes(c) && !m.is_still());
    let Some(media) = media else {
        return Err(Error::invalid(format!(
            "{id} has no moving source picture: a track follows a thing in footage or an image \
             sequence"
        )));
    };
    if !(c.start..c.end()).contains(&at) {
        return Err(Error::invalid(format!(
            "{id} does not show frame {at}: a track starts on a frame of the clip"
        )));
    }
    let effects = c.picture.iter().flat_map(|x| &x.effects);
    let area = effects
        .into_iter()
        .nth(effect)
        .and_then(|e| e.area.as_ref());
    let Some(area) = area else {
        return Err(Error::invalid(format!(
            "effect {effect} of {id} has no area: a track follows the thing under an area \
             (effects.set)"
        )));
    };
    let frame = c.source_at(at);
    let base = match area.follows.as_deref() {
        None => Area::WHOLE,
        Some(named) => {
            let followed = c.motion.iter().filter(|m| m.follows(named));
            let base = followed.filter_map(|m| m.area_at(frame as f64)).next();
            base.ok_or_else(|| {
                Error::invalid(format!(
                    "the area of effect {effect} of {id} follows {named}, which does not last \
                     on this frame"
                ))
            })?
        }
    };
    let shown = media.info.video.as_ref().map(|v| v.display_size());
    let (w, h) = shown.unwrap_or((p.settings.width, p.settings.height));
    let fit = [f64::from(w.max(1)), f64::from(h.max(1))];
    let own = base.within(area, fit);
    Ok(Start {
        frames: c.shown(),
        frame,
        own,
        bounds: own.bounds(area, fit),
        fit,
    })
}

const TRACK: &str = "Track";

pub fn tracked(
    p: &mut Project,
    id: &str,
    effect: usize,
    (start, way): (&Start, TrackWay),
    mut samples: Vec<PathSample>,
) -> Result<Value> {
    let c = clip_of(p, id)?;
    let effects = c.picture.iter().flat_map(|x| &x.effects);
    let area = effects.into_iter().nth(effect).and_then(|e| e.area.clone());
    let Some(area) = area else {
        return Err(Error::invalid(format!(
            "effect {effect} of {id} has no area"
        )));
    };
    let old = (area.follows.as_ref()).and_then(|f| c.motion.iter().find(|m| m.id == *f));
    if let Some(old) = old {
        let round = |v: f64| (v * 1e5).round() / 1e5;
        let stays = |s: &&PathSample| match way {
            TrackWay::Both => false,
            TrackWay::Forward => s.frame < start.frame,
            TrackWay::Backward => s.frame > start.frame,
        };
        let kept = old.samples.iter().filter(stays).map(|s| {
            let a = s.area().within(&area, start.fit);
            PathSample {
                frame: s.frame,
                center: a.center.map(round),
                size: a.size.map(round),
                rotation: (a.rotation * 100.0).round() / 100.0,
            }
        });
        samples.extend(kept);
        samples.sort_by_key(|s| s.frame);
    }
    let samples = motion::simplified(&samples);
    if let Some(why) = motion::samples_problem(&samples) {
        return Err(Error::invalid(why));
    }
    let tracks = |m: &MotionPath| {
        let name = m.name.as_deref();
        m.subject.is_none() && name.is_some_and(|n| n.starts_with(TRACK))
    };
    let old = old.filter(|m| tracks(m)).map(|m| m.id.clone());
    let shared = |old: &String| followers(c).any(|(n, path)| n != effect && path == old);
    if let Some(old) = old.filter(|old| !shared(old)) {
        c.motion.retain(|m| m.id != old);
    }
    if c.motion.len() >= motion::PATHS_KEPT {
        return Err(Error::invalid(format!(
            "{id} has {} motion paths, the most a clip takes",
            motion::PATHS_KEPT
        )));
    }
    let number = 1 + c.motion.iter().filter(|m| tracks(m)).count();
    let path = MotionPath {
        id: motion::next_id(&c.motion),
        name: Some(format!("{TRACK} {number}")),
        subject: None,
        samples: Arc::new(samples),
    };
    let shown = shown(c, &path, None, false);
    let follows = Some(path.id.clone());
    c.motion.push(path);
    let effects = c.picture.iter_mut().flat_map(|x| &mut x.effects);
    if let Some(e) = effects.into_iter().nth(effect) {
        e.area = Some(EffectArea {
            follows,
            center: None,
            size: None,
            rotation: None,
            ..area
        });
    }
    Ok(shown)
}

pub fn found(
    p: &mut Project,
    id: &str,
    subject: Subject,
    tracks: Vec<motion::FoundPath>,
) -> Result<Value> {
    let c = clip_of(p, id)?;
    let old = of_subject(c, subject);
    c.motion.retain(|m| !old.contains(&m.id));
    if c.motion.len() + tracks.len() > motion::PATHS_KEPT {
        return Err(Error::invalid(format!(
            "{id} would have {} motion paths, more than the {} a clip takes",
            c.motion.len() + tracks.len(),
            motion::PATHS_KEPT
        )));
    }
    let mut paths = Vec::new();
    for (n, track) in tracks.into_iter().enumerate() {
        let path = MotionPath {
            id: motion::next_id(&c.motion),
            name: Some(format!("{} {}", subject.title(), n + 1)),
            subject: Some(subject),
            samples: Arc::new(track.samples),
        };
        let mut v = shown(c, &path, None, false);
        v["seen"] = json!(track.seen);
        v["score"] = json!(track.score);
        v["gaps"] = json!(track.gaps);
        paths.push(v);
        c.motion.push(path);
    }
    Ok(json!({ "clip": id, "paths": paths }))
}
