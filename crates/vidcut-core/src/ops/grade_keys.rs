use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, Grade, GradeKey, GradeKeyKind, Project, Track, TrackKind};

use super::grade::{GradeOf, edited, locked, no_node, node};
use super::keyframes::Pick;
use super::locate;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KindChange {
    pub kind: Option<GradeKeyKind>,
    pub start: Option<u8>,
    pub end: Option<u8>,
}

enum Clock {
    Timeline,
    Clip(Box<Clip>),
}

impl Clock {
    fn of(p: &Project, of: &GradeOf) -> Result<Self> {
        Ok(match of {
            GradeOf::Timeline => Self::Timeline,
            GradeOf::Clip(id) => {
                let (ti, ci) = locate(p, id)?;
                Self::Clip(Box::new(p.tracks[ti].clips[ci].clone()))
            }
        })
    }

    fn frame(&self, t: i64) -> i64 {
        match self {
            Self::Timeline => t,
            Self::Clip(c) => c.content_at(t),
        }
    }

    fn time(&self, frame: i64) -> i64 {
        match self {
            Self::Timeline => frame,
            Self::Clip(c) => c.key_time(frame).round() as i64,
        }
    }

    fn moved(&self, frame: i64, by: i64) -> i64 {
        match self {
            Self::Timeline => frame + by,
            Self::Clip(c) => c.key_at(c.key_time(frame) + by as f64).round() as i64,
        }
    }
}

fn targets(g: &Grade, n: Option<u32>) -> Result<Vec<u32>> {
    match n {
        Some(n) => match g.node(n) {
            Some(d) if !d.takes_keyframes() => Err(Error::invalid(format!(
                "node {n} is a mixer, which takes no keyframes"
            ))),
            Some(d) if d.locked => Err(locked(n)),
            Some(_) => Ok(vec![n]),
            None => Err(no_node(n)),
        },
        None => {
            let all: Vec<u32> = (g.nodes.iter().zip(1..))
                .filter(|(d, _)| d.takes_keyframes() && !d.locked)
                .map(|(_, k)| k)
                .collect();
            match all.is_empty() {
                true => Err(Error::invalid(
                    "the grade has no corrector or key mixer that is not locked",
                )),
                false => Ok(all),
            }
        }
    }
}

fn picked(g: &Grade, n: Option<u32>, pick: &Pick, clock: &Clock) -> Result<Vec<(u32, i64)>> {
    let nodes = targets(g, n)?;
    let has = |k: u32, f: i64| g.node(k).is_some_and(|d| d.key_on(f).is_some());
    let mut wanted: Option<BTreeSet<i64>> = None;
    if let Some(frames) = &pick.frames {
        wanted.get_or_insert_default().extend(frames);
    }
    for &t in pick.at.iter().flatten() {
        let on = (nodes.iter().copied())
            .flat_map(|k| g.node(k).into_iter().flat_map(|d| &d.keys))
            .map(|key| key.frame)
            .find(|f| clock.time(*f) == t);
        wanted
            .get_or_insert_default()
            .insert(on.unwrap_or(clock.frame(t)));
    }
    let out: Vec<(u32, i64)> = (nodes.iter().copied())
        .flat_map(|k| {
            let keys = g.node(k).map_or(&[][..], |d| &d.keys);
            keys.iter()
                .map(move |key| (k, key.frame))
                .collect::<Vec<_>>()
        })
        .filter(|(_, f)| wanted.as_ref().is_none_or(|w| w.contains(f)))
        .collect();
    if let Some(w) = &wanted
        && let Some(f) = w.iter().find(|f| !nodes.iter().any(|k| has(*k, **f)))
    {
        return Err(Error::not_found(format!(
            "the grade has no keyframe on frame {f}"
        )));
    }
    if out.is_empty() {
        return Err(Error::not_found("the grade has no keyframes"));
    }
    Ok(out)
}

pub fn add(
    p: &mut Project,
    of: &GradeOf,
    n: Option<u32>,
    at: i64,
    kind: GradeKeyKind,
) -> Result<Value> {
    let frame = Clock::of(p, of)?.frame(at);
    let profile = p.settings.dynamics_profile;
    let mut made = Vec::new();
    edited(p, of, |g| {
        for k in targets(g, n)? {
            let d = node(g, k)?;
            match d.keys.iter_mut().find(|key| key.frame == frame) {
                Some(key) => key.kind = kind,
                None => {
                    let key = d.key_here(frame, kind, profile);
                    d.put_key(key);
                }
            }
            made.push(k);
        }
        Ok(())
    })?;
    Ok(json!({ "frame": frame, "nodes": made }))
}

pub fn edl_frames(p: &Project) -> Vec<i64> {
    let end = p.end();
    let frames: BTreeSet<i64> = (p.tracks.iter())
        .filter(|t| t.kind == TrackKind::Video)
        .flat_map(Track::edit_points)
        .filter(|f| *f > 0 && *f < end)
        .collect();
    frames.into_iter().collect()
}

pub fn add_marks(p: &mut Project, n: Option<u32>) -> Result<Value> {
    let frames = edl_frames(p);
    if frames.is_empty() {
        return Err(Error::invalid("the timeline has no edits"));
    }
    let profile = p.settings.dynamics_profile;
    let mut made = Vec::new();
    edited(p, &GradeOf::Timeline, |g| {
        for k in targets(g, n)? {
            let d = node(g, k)?;
            let marks: Vec<GradeKey> = (frames.iter())
                .filter(|f| d.key_on(**f).is_none())
                .map(|f| GradeKey {
                    edl: true,
                    ..d.key_here(*f, GradeKeyKind::Static, profile)
                })
                .collect();
            for mark in marks {
                d.put_key(mark);
            }
            made.push(k);
        }
        Ok(())
    })?;
    Ok(json!({ "frames": frames, "nodes": made }))
}

pub fn remove_marks(p: &mut Project, of: &GradeOf, n: Option<u32>) -> Result<()> {
    edited(p, of, |g| {
        let mut gone = false;
        for k in targets(g, n)? {
            let d = node(g, k)?;
            gone |= d.keys.iter().any(|key| key.edl);
            d.keys.retain(|key| !key.edl);
        }
        match gone {
            true => Ok(()),
            false => Err(Error::not_found("the grade has no EDL marks")),
        }
    })
}

pub fn set(
    p: &mut Project,
    of: &GradeOf,
    n: Option<u32>,
    pick: &Pick,
    change: &KindChange,
) -> Result<()> {
    if *change == KindChange::default() {
        return Err(Error::usage("give `kind`, `start` or `end`"));
    }
    let clock = Clock::of(p, of)?;
    edited(p, of, |g| {
        for (k, f) in picked(g, n, pick, &clock)? {
            let Some(key) = node(g, k)?.keys.iter_mut().find(|key| key.frame == f) else {
                continue;
            };
            key.kind = change.kind.unwrap_or(key.kind);
            key.start = change.start.unwrap_or(key.start);
            key.end = change.end.unwrap_or(key.end);
        }
        Ok(())
    })
}

pub fn shift(
    p: &mut Project,
    of: &GradeOf,
    n: Option<u32>,
    pick: &Pick,
    (by, to): (Option<i64>, Option<i64>),
) -> Result<()> {
    if by.is_some() == to.is_some() {
        return Err(Error::usage("give `by` or `to`"));
    }
    let clock = Clock::of(p, of)?;
    edited(p, of, |g| {
        let chosen = picked(g, n, pick, &clock)?;
        let frames: BTreeSet<i64> = chosen.iter().map(|(_, f)| *f).collect();
        if to.is_some() && frames.len() != 1 {
            return Err(Error::usage(format!(
                "`to` moves the keyframes of one frame; {} frames are chosen",
                frames.len()
            )));
        }
        let moved = |f: i64| match (by, to) {
            (Some(b), _) => clock.moved(f, b),
            (_, Some(t)) => clock.frame(t),
            _ => f,
        };
        for k in chosen.iter().map(|(k, _)| *k).collect::<BTreeSet<u32>>() {
            let d = node(g, k)?;
            let mine = |f: i64| chosen.contains(&(k, f));
            let stays: BTreeSet<i64> = (d.keys.iter())
                .map(|key| key.frame)
                .filter(|f| !mine(*f))
                .collect();
            let mut taken = stays;
            for key in d.keys.iter().filter(|key| mine(key.frame)) {
                if !taken.insert(moved(key.frame)) {
                    return Err(Error::invalid(format!(
                        "node {k} already has a keyframe on frame {}",
                        moved(key.frame)
                    )));
                }
            }
            for key in d.keys.iter_mut().filter(|key| mine(key.frame)) {
                key.frame = moved(key.frame);
            }
            d.keys.sort_by_key(|key| key.frame);
        }
        Ok(())
    })
}

pub fn remove(p: &mut Project, of: &GradeOf, n: Option<u32>, pick: &Pick) -> Result<()> {
    let clock = Clock::of(p, of)?;
    edited(p, of, |g| {
        let chosen = picked(g, n, pick, &clock)?;
        for (k, f) in chosen {
            node(g, k)?.keys.retain(|key| key.frame != f);
        }
        Ok(())
    })
}
