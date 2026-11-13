use std::sync::Arc;

use serde_json::{Value, json};

use crate::effect_keys::Keyed;
use crate::error::{Error, Result};
use crate::model::{
    Clip, EffectValue, Follow, Mask, MotionPath, Node, PathSample, Project, Subject, Title,
    TitleKind, Tool, Track, TrackWay,
};
use crate::motion::{self, Area};
use crate::place::{MaskBase, Outline, Shaping};

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
        tracked: false,
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

fn followers(c: &Clip) -> impl Iterator<Item = (&str, &str)> {
    let nodes = c.picture.iter().flat_map(|x| x.composition.iter());
    nodes.flat_map(|k| &k.nodes).filter_map(|n| {
        let follows = n.tool.mask()?.follows.as_deref()?;
        Some((n.name.as_str(), follows))
    })
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
    if let Some((mask, path)) = followers(c).find(|(_, path)| paths.iter().any(|x| x == path)) {
        return Err(Error::invalid(format!(
            "mask {mask} of {id} follows motion path {path}: first give it another `follows`, or \
             none (comp.set)"
        )));
    }
    led(p, id, paths)?;
    clip_of(p, id)?.motion.retain(|m| !paths.contains(&m.id));
    Ok(())
}

fn led(p: &Project, id: &str, paths: &[String]) -> Result<()> {
    let titles = p.tracks.iter().flat_map(|t| &t.clips);
    let follows = |c: &Clip| {
        let follow = c.title.as_ref()?.follows.as_ref()?;
        let named = follow.clip == id && paths.contains(&follow.path);
        named.then(|| (c.id.clone(), follow.path.clone()))
    };
    match titles.into_iter().find_map(follows) {
        Some((title, path)) => Err(Error::invalid(format!(
            "the title {title} follows motion path {path} of {id}: first give that title \
             another path, or none (motion.follow)"
        ))),
        None => Ok(()),
    }
}

pub fn follow(p: &mut Project, id: &str, to: Option<(&str, &str)>) -> Result<Value> {
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    let Some(old) = c.title.clone() else {
        return Err(Error::invalid(format!("{id} is not a title")));
    };
    let follows = match to {
        None => None,
        Some(_) if old.kind == TitleKind::Scroll => {
            return Err(Error::invalid("a scroll follows no motion path"));
        }
        Some((of, path)) => {
            let (ai, aj) = locate(p, of)?;
            let a = &p.tracks[ai].clips[aj];
            if !takes(a) {
                return Err(Error::invalid(format!(
                    "{of} has no source picture: a title follows a path of footage, a still or \
                     an image sequence"
                )));
            }
            let Some(m) = a.motion.iter().find(|m| m.id == path) else {
                return Err(Error::not_found(format!("{of} has no motion path {path}")));
            };
            Some(Follow {
                clip: under(p, c, a, m).id.clone(),
                path: path.to_string(),
            })
        }
    };
    let mut next = Title {
        follows,
        ..old.clone()
    };
    if next.follows.is_some() {
        let keyed = |keys: &[crate::model::Keyframe]| !keys.is_empty();
        let upper = old.upper.as_ref().is_some_and(|u| keyed(&u.position_keys));
        if keyed(&old.position_keys) || upper {
            return Err(Error::invalid(format!(
                "the position of {id} has keyframes: first remove them (keyframes.remove)"
            )));
        }
        let [x, y] = old.place();
        let [mx, my] = crate::model::Picture::MIDDLE;
        next.moved([mx - x, my - y]);
        next.position = Some(crate::model::Picture::MIDDLE);
        if let Some(upper) = next.upper.as_mut() {
            upper.position = upper
                .position
                .map(|at| at.map(crate::otio::effects::rounded));
        }
    }
    let next = super::titles::checked(&next)?;
    let shown = json!({ "clip": id, "follows": next.follows, "position": next.place() });
    p.tracks[ti].clips[ci].title = Some(next);
    Ok(shown)
}

fn under<'a>(p: &'a Project, c: &Clip, x: &'a Clip, path: &MotionPath) -> &'a Clip {
    let meets = |y: &Clip| y.start < c.end() && c.start < y.end();
    if meets(x) {
        return x;
    }
    let same = |y: &&Clip| {
        let part = y.id != x.id && takes(y) && meets(y) && y.media == x.media;
        let has = |m: &MotionPath| m.id == path.id && m.samples == path.samples;
        part && y.motion.iter().any(has)
    };
    let clips = p.tracks.iter().flat_map(|t| &t.clips);
    clips.filter(same).min_by_key(|y| y.start).unwrap_or(x)
}

pub fn settle(p: &mut Project) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut changes = Vec::new();
    for (ti, t) in p.tracks.iter().enumerate() {
        for (ci, c) in t.clips.iter().enumerate() {
            let Some(f) = c.title.as_ref().and_then(|t| t.follows.as_ref()) else {
                continue;
            };
            let led = p.clip(&f.clip).filter(|x| takes(x));
            let path = led.and_then(|x| x.motion.iter().find(|m| m.id == f.path));
            match led.zip(path) {
                Some((x, path)) => {
                    let y = under(p, c, x, path);
                    if y.id != x.id {
                        changes.push((ti, ci, Some(y.id.clone())));
                    }
                }
                None => {
                    warnings.push(format!(
                        "{} follows no motion path now: {} or its path {} is gone",
                        c.id, f.clip, f.path
                    ));
                    changes.push((ti, ci, None));
                }
            }
        }
    }
    for (ti, ci, to) in changes {
        let Some(title) = p.tracks[ti].clips[ci].title.as_mut() else {
            continue;
        };
        match (to, title.follows.as_mut()) {
            (Some(id), Some(f)) => f.clip = id,
            _ => title.follows = None,
        }
    }
    for c in p.tracks.iter_mut().flat_map(|t| &mut t.clips) {
        let inside = c.compound.as_mut();
        if inside.is_some_and(|k| unled(&mut k.tracks) | unled(&mut k.made_from)) {
            warnings.push(format!(
                "{}: a title inside a compound clip follows no motion path",
                c.id
            ));
        }
    }
    warnings
}

fn unled(tracks: &mut [Track]) -> bool {
    let mut any = false;
    for c in tracks.iter_mut().flat_map(|t| &mut t.clips) {
        if let Some(title) = c.title.as_mut() {
            any |= title.follows.take().is_some();
        }
        if let Some(k) = c.compound.as_mut() {
            any |= unled(&mut k.tracks) | unled(&mut k.made_from);
        }
    }
    any
}

pub fn follow_violation(p: &Project, c: &Clip) -> Option<String> {
    let title = c.title.as_ref()?;
    let f = title.follows.as_ref()?;
    if title.kind == TitleKind::Scroll {
        return Some(format!("{}: a scroll that follows a motion path", c.id));
    }
    let has = |x: &Clip| takes(x) && x.motion.iter().any(|m| m.id == f.path);
    (!p.clip(&f.clip).is_some_and(has)).then(|| {
        format!(
            "{}: the title follows motion path {} of {}, which is not there",
            c.id, f.path, f.clip
        )
    })
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
    if m.tracked {
        v["tracked"] = json!(true);
    }
    if let Some((first, last)) = m.span() {
        v["first"] = json!(first);
        v["last"] = json!(last);
    }
    if let Some(at) = at {
        let frame = c.source_at(at);
        if let Some(area) = m.base_at(frame as f64) {
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
    if let Some((mask, path)) = followers(c).find(|(_, path)| !has(path)) {
        return Some(format!(
            "{}: mask {mask} follows motion path {path}, which the clip has not",
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
    Ok(c.shown())
}

fn followed(p: &Project, id: &str) -> Vec<String> {
    let titles = p.tracks.iter().flat_map(|t| &t.clips);
    let led = titles.filter_map(|c| {
        let follow = c.title.as_ref()?.follows.as_ref()?;
        (follow.clip == id).then(|| follow.path.clone())
    });
    let mut out: Vec<String> = led.collect();
    if let Some(c) = p.clip(id) {
        out.extend(followers(c).map(|(_, path)| path.to_string()));
    }
    out
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
    pub frame: i64,
    pub own: Area,
    pub bounds: [f64; 4],
    pub points: Vec<[f64; 2]>,
}

pub fn start(p: &Project, id: &str, mask: &str, at: i64) -> Result<Start> {
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
    let frame = c.source_at(at);
    let shown = media.info.video.as_ref().map(|v| v.display_size());
    let (w, h) = shown.unwrap_or((p.settings.width, p.settings.height));
    let fit = [f64::from(w.max(1)), f64::from(h.max(1))];
    let (own, bounds, points) = mask_start(c, id, mask, frame, fit)?;
    Ok(Start {
        frames: c.shown(),
        frame,
        own,
        bounds,
        points,
    })
}

fn mask_of<'a>(c: &'a Clip, id: &str, name: &str) -> Result<(&'a Node, &'a Mask)> {
    let comp = c.picture.as_ref().and_then(|x| x.composition.as_ref());
    let node = comp.and_then(|k| k.node(name));
    node.and_then(|n| Some((n, n.tool.mask()?))).ok_or_else(|| {
        Error::invalid(format!(
            "{id} has no mask {name}: a track follows the thing under a mask of its composition"
        ))
    })
}

const PLACE_VALUES: [EffectValue; 8] = [
    EffectValue::Center,
    EffectValue::Width,
    EffectValue::Height,
    EffectValue::Angle,
    EffectValue::Size,
    EffectValue::ZRotation,
    EffectValue::XRotation,
    EffectValue::YRotation,
];

fn mask_unkeyed(n: &Node, id: &str) -> Result<()> {
    match PLACE_VALUES.into_iter().find(|v| !n.keys_of(*v).is_empty()) {
        Some(value) => Err(Error::invalid(format!(
            "mask {} of {id} has {} keyframes, and a track sets its place: first remove those \
             keyframes (comp.key)",
            n.name,
            value.name().replace('_', " ")
        ))),
        None => Ok(()),
    }
}

type MaskStart = (Area, [f64; 4], Vec<[f64; 2]>);

fn mask_start(c: &Clip, id: &str, name: &str, frame: i64, fit: [f64; 2]) -> Result<MaskStart> {
    let (n, m) = mask_of(c, id, name)?;
    mask_unkeyed(n, id)?;
    let [fw, fh] = fit;
    let base = match m.follows.as_deref() {
        None => MaskBase::whole((fw, fh)),
        Some(named) => {
            let found = motion::areas_of(&c.motion, named, frame as f64);
            let first = found.first().ok_or_else(|| {
                Error::invalid(format!(
                    "mask {name} of {id} follows {named}, which does not last on this frame"
                ))
            })?;
            MaskBase::of(first, (fw, fh))
        }
    };
    let figure = Shaping::figure(m, &n.tool, &base);
    let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for spot in figure.line() {
        for i in 0..2 {
            (low[i], high[i]) = (low[i].min(spot[i]), high[i].max(spot[i]));
        }
    }
    if !(low[0] < high[0] && low[1] < high[1]) {
        return Err(Error::invalid(format!(
            "mask {name} of {id} covers nothing: a track follows the thing under a mask"
        )));
    }
    let middle = [(low[0] + high[0]) / 2.0, (low[1] + high[1]) / 2.0];
    let size = [high[0] - low[0], high[1] - low[1]];
    let bounds = [middle[0] / fw, middle[1] / fh, size[0] / fw, size[1] / fh];
    Ok(match &figure.outline {
        Outline::Ellipse { half, turn } | Outline::Rectangle { half, turn, .. } => {
            let own = Area {
                center: [figure.middle[0] / fw, figure.middle[1] / fh],
                size: [2.0 * half[0] / fw, 2.0 * half[1] / fh],
                rotation: turn.to_degrees(),
            };
            (own, bounds, Vec::new())
        }
        Outline::Polygon { points, .. } => {
            let own = Area {
                center: [bounds[0], bounds[1]],
                size: [bounds[2], bounds[3]],
                rotation: 0.0,
            };
            let boxed = |[x, y]: &[f64; 2]| [(x - middle[0]) / size[0], (middle[1] - y) / size[1]];
            (own, bounds, points.iter().map(boxed).collect())
        }
    })
}

const TRACK: &str = "Track";

pub fn tracked(
    p: &mut Project,
    id: &str,
    mask: &str,
    (start, way): (&Start, TrackWay),
    mut samples: Vec<PathSample>,
) -> Result<Value> {
    let c = clip_of(p, id)?;
    let (n, m) = mask_of(c, id, mask)?;
    mask_unkeyed(n, id)?;
    let followed = m.follows.clone();
    let old = (followed.as_ref()).and_then(|f| c.motion.iter().find(|m| m.id == *f));
    if let Some(old) = old {
        let stays = |s: &&PathSample| match way {
            TrackWay::Both => false,
            TrackWay::Forward => s.frame < start.frame,
            TrackWay::Backward => s.frame > start.frame,
        };
        samples.extend(old.samples.iter().filter(stays).cloned());
        samples.sort_by_key(|s| s.frame);
    }
    let samples = motion::simplified(&samples);
    if let Some(why) = motion::samples_problem(&samples) {
        return Err(Error::invalid(why));
    }
    let shared = |old: &String| followers(c).any(|(w, path)| w != mask && path == old);
    let own = old.filter(|m| m.tracked && !shared(&m.id));
    let path = match own.map(|m| m.id.clone()) {
        Some(own) => {
            path_of(c, &own)?.samples = Arc::new(samples);
            own
        }
        None => {
            if c.motion.len() >= motion::PATHS_KEPT {
                return Err(Error::invalid(format!(
                    "{id} has {} motion paths, the most a clip takes",
                    motion::PATHS_KEPT
                )));
            }
            let named = |n: &usize| {
                c.motion
                    .iter()
                    .any(|m| m.name == Some(format!("{TRACK} {n}")))
            };
            let number = (1..).find(|n| !named(n)).unwrap_or(1);
            let path = motion::next_id(&c.motion);
            c.motion.push(MotionPath {
                id: path.clone(),
                name: Some(format!("{TRACK} {number}")),
                subject: None,
                tracked: true,
                samples: Arc::new(samples),
            });
            path
        }
    };
    let stored = c.motion.iter().find(|m| m.id == path);
    let shown = stored.map_or(Value::Null, |m| shown(c, m, None, false));
    let comp = c.picture.as_mut().and_then(|x| x.composition.as_mut());
    let node = comp.and_then(|k| k.nodes.iter_mut().find(|n| n.name == mask));
    match node.map(|n| &mut n.tool) {
        Some(Tool::Polygon(m)) => {
            m.follows = Some(path);
            m.center = None;
            m.points = start.points.clone();
            (m.size, m.z_rotation, m.x_rotation, m.y_rotation) = (None, None, None, None);
        }
        Some(t) => {
            if let Some(m) = t.mask_mut() {
                m.follows = Some(path);
                (m.center, m.angle) = (None, None);
                (m.width, m.height) = (Some(1.0), Some(1.0));
            }
        }
        None => {}
    }
    Ok(shown)
}

pub fn found(
    p: &mut Project,
    id: &str,
    subject: Subject,
    tracks: Vec<motion::FoundPath>,
) -> Result<Value> {
    let kept = followed(p, id);
    let c = clip_of(p, id)?;
    let old = of_subject(c, subject);
    c.motion
        .retain(|m| !old.contains(&m.id) || kept.contains(&m.id));
    let mut fresh = Vec::new();
    for track in tracks {
        let stays = c
            .motion
            .iter_mut()
            .find(|m| old.contains(&m.id) && same_thing(m, &track.samples));
        match stays {
            Some(m) => m.samples = Arc::new(merged(&m.samples, &track.samples)),
            None => fresh.push(track),
        }
    }
    let tracks: Vec<motion::FoundPath> = fresh;
    if c.motion.len() + tracks.len() > motion::PATHS_KEPT {
        return Err(Error::invalid(format!(
            "{id} would have {} motion paths, more than the {} a clip takes",
            c.motion.len() + tracks.len(),
            motion::PATHS_KEPT
        )));
    }
    let mut paths = Vec::new();
    let mut n = 0;
    for track in tracks {
        let name = loop {
            n += 1;
            let name = format!("{} {n}", subject.title());
            if !c.motion.iter().any(|m| m.name.as_deref() == Some(&name)) {
                break name;
            }
        };
        let path = MotionPath {
            id: motion::next_id(&c.motion),
            name: Some(name),
            subject: Some(subject),
            tracked: false,
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
