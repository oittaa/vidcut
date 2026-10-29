use super::{change_speed, retimed};
use crate::error::{Error, Result};
use crate::model::{Clip, Project, Section, Speed};

fn clip_of(p: &Project, id: &str) -> Result<Clip> {
    let c = p
        .clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    let why = if c.title.is_some() {
        "a title"
    } else if c.compound.is_some() {
        "a compound clip"
    } else if c.is_generator() {
        "a generator from another editor"
    } else {
        return Ok(c.clone());
    };
    Err(Error::invalid(format!(
        "{id} is {why}: it has no speed points"
    )))
}

fn under(p: &Project, id: &str, t: i64) -> Result<Clip> {
    let c = clip_of(p, id)?;
    if !(c.start..c.end()).contains(&t) {
        return Err(Error::invalid(format!(
            "{id} isn't under {}",
            p.settings.timecode(t)
        )));
    }
    Ok(c)
}

fn latest_first(p: &Project, ids: &[String]) -> Vec<String> {
    let mut list: Vec<(i64, String)> = ids
        .iter()
        .map(|id| (p.clip(id).map_or(0, |c| c.start), id.clone()))
        .collect();
    list.sort();
    list.into_iter().rev().map(|(_, id)| id).collect()
}

fn sections(c: &Clip) -> (Vec<Section>, i64) {
    match &c.speed {
        Some(s) => (s.sections.to_vec(), s.skip),
        None => (Speed::constant(1.0).sections.to_vec(), c.source_in),
    }
}

fn offset(c: &Clip) -> i64 {
    match &c.speed {
        Some(_) => c.source_in - c.pace().shown(),
        None => 0,
    }
}

fn index(map: &[Section], u: i64) -> usize {
    map.partition_point(|s| s.at <= u).saturating_sub(1)
}

fn pos(s: &Section, u: i64) -> f64 {
    s.from + (u - s.at) as f64 * s.speed
}

fn split(map: &mut Vec<Section>, u: i64) -> usize {
    let i = index(map, u);
    let s = map[i];
    let cut = Section {
        at: u,
        from: pos(&s, u),
        speed: s.speed,
    };
    if i == 0 && u <= s.at {
        map[0] = Section {
            at: u - 1,
            from: pos(&s, u - 1),
            speed: s.speed,
        };
        map.insert(1, cut);
        return 1;
    }
    if s.at == u {
        return i;
    }
    map.insert(i + 1, cut);
    i + 1
}

fn with(c: &Clip, map: Vec<Section>, skip: i64) -> Clip {
    let mut n = c.clone();
    let speed = Speed {
        sections: map.into(),
        skip,
        effect: None,
    };
    n.source_in = offset(c) + crate::render::Pace::mapped(speed.sections.clone(), skip).shown();
    n.speed = (speed.scalar() != Some(1.0)).then_some(speed);
    n
}

pub fn add_point(p: &mut Project, ids: &[String], t: i64) -> Result<()> {
    for id in ids {
        let c = under(p, id, t)?;
        let tc = p.settings.timecode(t);
        if t == c.start {
            return Err(Error::invalid(format!(
                "{id} starts at {tc}: a speed point goes after its first frame"
            )));
        }
        let (mut map, skip) = sections(&c);
        let u = skip + t - c.start;
        if map.iter().skip(1).any(|s| s.at == u) {
            return Err(Error::invalid(format!(
                "{id} already has a speed point at {tc}"
            )));
        }
        split(&mut map, u);
        let n = with(&c, map, skip);
        retimed(p, &c, n, c.duration, false, "with this speed point")?;
    }
    Ok(())
}

fn merge(map: &mut Vec<Section>, i: usize, end: i64) {
    let (a, b) = (map[i - 1], map[i]);
    let (to, at) = match map.get(i + 1) {
        Some(n) => (n.at, n.from),
        None => (end, pos(&b, end)),
    };
    if to > a.at {
        map[i - 1].speed = (at - a.from) / (to - a.at) as f64;
    }
    map.remove(i);
}

pub fn remove_point(p: &mut Project, ids: &[String], t: Option<i64>, all: bool) -> Result<()> {
    for id in ids {
        let c = match t.filter(|_| !all) {
            Some(t) => under(p, id, t)?,
            None => clip_of(p, id)?,
        };
        let (mut map, skip) = sections(&c);
        if map.len() < 2 {
            return Err(Error::invalid(format!("{id} has no speed points")));
        }
        let end = skip + c.duration;
        if all {
            while map.len() > 1 {
                let i = map.len() - 1;
                merge(&mut map, i, end);
            }
        } else {
            let t = t.ok_or_else(|| Error::usage("speed.remove_point needs `at` (no playhead)"))?;
            let u = skip + t - c.start;
            match map.iter().rposition(|s| s.at <= u) {
                Some(i) if i >= 1 => merge(&mut map, i, end),
                _ => {
                    return Err(Error::invalid(format!(
                        "{id} has no speed point at or before {}",
                        p.settings.timecode(t)
                    )));
                }
            }
        }
        let n = with(&c, map, skip);
        retimed(p, &c, n, c.duration, false, "without this speed point")?;
    }
    Ok(())
}

pub fn set(
    p: &mut Project,
    ids: &[String],
    t: i64,
    speed: f64,
    ripple: Option<bool>,
) -> Result<()> {
    let ripple_on = ripple.unwrap_or(p.settings.ripple_on);
    for id in latest_first(p, ids) {
        let c = under(p, &id, t)?;
        if c.speed.as_ref().is_none_or(|s| s.sections.len() < 2) {
            change_speed(p, std::slice::from_ref(&id), speed, ripple, Some(t))?;
            continue;
        }
        if !speed.is_finite() || !(0.01..=100.0).contains(&speed) {
            return Err(Error::invalid(format!(
                "speed {speed} isn't 1% to 10000% forward: a section between speed \
                 points plays forward, and speed.freeze makes a freeze frame"
            )));
        }
        let (mut map, skip) = sections(&c);
        let end = skip + c.duration;
        let u = skip + t - c.start;
        let i = index(&map, u);
        let s = map[i];
        if s.speed <= 0.0 {
            let what = if s.speed == 0.0 {
                "a freeze frame"
            } else {
                "backward"
            };
            return Err(Error::invalid(format!(
                "the section of {id} under {} is {what}: only a forward section takes a speed",
                p.settings.timecode(t)
            )));
        }
        let last = &map[index(&map, end - 1)];
        let shown_end = (pos(last, end - 1) + 1e-9).floor() + 1.0;
        let inside = map.get(i + 1).is_none_or(|n| n.at >= end);
        let (span, old) = match map.get(i + 1) {
            Some(n) => (n.from - s.from, n.at - s.at),
            None => (shown_end - s.from, end - s.at),
        };
        let frames = ((span / speed) - 1e-9).ceil().max(1.0) as i64;
        let d = frames - old;
        map[i].speed = speed;
        for x in &mut map[i + 1..] {
            x.at += d;
        }
        let new_end = if inside {
            s.at + (((shown_end - s.from) / speed) - 1e-9).ceil() as i64
        } else {
            end + d
        };
        let want = if ripple_on {
            new_end - skip
        } else {
            c.duration
        };
        let n = with(&c, map, skip);
        let how = format!("at {}%", (speed * 100.0).round());
        retimed(p, &c, n, want, ripple_on, &how)?;
    }
    Ok(())
}

pub fn freeze(p: &mut Project, ids: &[String], t: i64, ripple: Option<bool>) -> Result<()> {
    let ripple_on = ripple.unwrap_or(p.settings.ripple_on);
    let hold = (2.0 * p.settings.fps.as_f64()).round() as i64;
    for id in latest_first(p, ids) {
        let c = under(p, &id, t)?;
        let (mut map, skip) = sections(&c);
        let u = skip + t - c.start;
        let held = (c.source_at(t) - offset(&c)) as f64;
        let i = split(&mut map, u);
        let s = map[i];
        map[i] = Section {
            at: u,
            from: if s.speed < 0.0 { held } else { s.from },
            speed: 0.0,
        };
        map.insert(i + 1, Section { at: u + hold, ..s });
        for x in &mut map[i + 2..] {
            x.at += hold;
        }
        let want = if ripple_on {
            c.duration + hold
        } else {
            c.duration
        };
        let n = with(&c, map, skip);
        retimed(p, &c, n, want, ripple_on, "with a freeze frame")?;
    }
    Ok(())
}
