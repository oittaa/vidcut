use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, MotionPath, PathSample, Project};
use crate::motion;

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
    let has = |path: &str| c.motion.iter().any(|m| m.id == path);
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
