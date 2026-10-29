use super::keyframes::{Pick, picked_as};
use super::{change_speed, retimed};
use crate::error::{Error, Result};
use crate::model::{Clip, Ease, Project, Section, Speed};

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
        "{id} is {why}: it has no speed keyframes"
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
        ease: None,
    };
    if i == 0 && u <= s.at {
        map[0] = Section {
            at: u - 1,
            from: pos(&s, u - 1),
            speed: s.speed,
            ease: None,
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

fn takes_keyframe(p: &Project, id: &str, t: i64) -> Result<Clip> {
    let c = under(p, id, t)?;
    let tc = p.settings.timecode(t);
    if t == c.start {
        return Err(Error::invalid(format!(
            "{id} starts at {tc}: a speed keyframe goes after its first frame"
        )));
    }
    let (map, skip) = sections(&c);
    let u = skip + t - c.start;
    if map.iter().skip(1).any(|s| s.at == u) {
        return Err(Error::invalid(format!(
            "{id} already has a speed keyframe at {tc}"
        )));
    }
    Ok(c)
}

pub fn add_keyframe(p: &mut Project, ids: &[String], t: i64) -> Result<Vec<(String, i64)>> {
    let mut why = None;
    let mut targets = Vec::new();
    for id in ids {
        match takes_keyframe(p, id, t) {
            Ok(c) => targets.push(c),
            Err(e) => {
                why.get_or_insert(e);
            }
        }
    }
    if targets.is_empty() {
        return Err(why.unwrap_or_else(|| Error::invalid("no clips")));
    }
    let mut out = Vec::new();
    for c in targets {
        let (mut map, skip) = sections(&c);
        let u = skip + t - c.start;
        split(&mut map, u);
        let n = with(&c, map, skip);
        retimed(p, &c, n, c.duration, false, "with this speed keyframe")?;
        out.push((c.id, u));
    }
    Ok(out)
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

pub fn remove_keyframes(p: &mut Project, ids: &[String], pick: &Pick) -> Result<()> {
    for id in ids {
        let c = clip_of(p, id)?;
        let keys = c.speed.as_ref().map(Speed::keyframes).unwrap_or_default();
        if keys.is_empty() {
            return Err(Error::invalid(format!("{id} has no speed keyframes")));
        }
        let chosen = picked_as(&c, &keys, pick, "speed")?;
        let (mut map, skip) = sections(&c);
        let end = skip + c.duration;
        for i in (1..map.len()).rev() {
            if chosen.contains(&map[i].at) {
                merge(&mut map, i, end);
            }
        }
        let n = with(&c, map, skip);
        retimed(p, &c, n, c.duration, false, "without these speed keyframes")?;
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
                 keyframes plays forward, and speed.freeze makes a freeze frame"
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
            ease: s.ease,
        };
        map.insert(
            i + 1,
            Section {
                at: u + hold,
                ease: None,
                ..s
            },
        );
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

pub fn reverse(p: &mut Project, ids: &[String], t: i64, backward: bool) -> Result<()> {
    for id in ids {
        let c = under(p, id, t)?;
        let v = c.speed_at(t);
        if v == 0.0 {
            return Err(Error::invalid(format!(
                "the section of {id} under {} is a freeze frame: it has no direction",
                p.settings.timecode(t)
            )));
        }
        if (v < 0.0) == backward {
            continue;
        }
        if c.speed.as_ref().is_none_or(|s| s.sections.len() < 2) {
            change_speed(p, std::slice::from_ref(id), -v, Some(false), Some(t))?;
            continue;
        }
        let (mut map, skip) = sections(&c);
        let end = skip + c.duration;
        let i = index(&map, skip + t - c.start);
        let s = map[i];
        let a = if i == 0 { skip } else { s.at };
        let b = map.get(i + 1).map_or(end, |n| n.at);
        map[i] = Section {
            at: a,
            from: pos(&s, a) + (b - a) as f64 * s.speed,
            speed: -s.speed,
            ease: s.ease,
        };
        let n = with(&c, map, skip);
        let how = if backward { "backward" } else { "forward" };
        retimed(p, &c, n, c.duration, false, how)?;
    }
    Ok(())
}

fn takes(map: &[Section], i: usize) -> bool {
    i > 0 && i < map.len() && map[i - 1].speed >= 0.0 && map[i].speed >= 0.0
}

fn sides(ease: Ease) -> (bool, bool) {
    match ease {
        Ease::Linear => (false, false),
        Ease::EaseIn => (true, false),
        Ease::EaseOut => (false, true),
        Ease::EaseInOut => (true, true),
    }
}

pub fn section_eases(c: &Clip, t: i64) -> Option<(Ease, Vec<Ease>)> {
    let speed = c.speed.as_ref().filter(|s| s.sections.len() > 1)?;
    let map = &speed.sections;
    let i = index(map, speed.skip + t - c.start);
    let on = |j: usize| takes(map, j) && map[j].ease.is_some();
    let now = Ease::ALL
        .into_iter()
        .find(|&e| sides(e) == (on(i), on(i + 1)))?;
    let options = Ease::ALL
        .into_iter()
        .filter(|&e| {
            let (start, end) = sides(e);
            (!start || takes(map, i)) && (!end || takes(map, i + 1))
        })
        .collect();
    Some((now, options))
}

pub fn key_ease(c: &Clip, frame: i64) -> Option<(Ease, bool)> {
    let map = &c.speed.as_ref()?.sections;
    let i = map.iter().position(|s| s.at == frame).filter(|&i| i > 0)?;
    let on = takes(map, i) && map[i].ease.is_some();
    let ease = if on { Ease::EaseInOut } else { Ease::Linear };
    Some((ease, takes(map, i)))
}

fn eased(
    p: &Project,
    c: &Clip,
    map: &mut [Section],
    i: usize,
    on: bool,
    [start, end]: [i64; 2],
) -> Result<()> {
    if !on {
        map[i].ease = None;
        return Ok(());
    }
    if !takes(map, i) {
        let at = c.key_time(map[i].at).round() as i64;
        return Err(Error::invalid(format!(
            "the speed keyframe of {} at {} is next to a section that plays backward: \
             it takes no ease",
            c.id,
            p.settings.timecode(at)
        )));
    }
    let h = p.settings.fps.as_f64() / 6.0;
    let at = map[i].at;
    let before = if at > start {
        (-h).max((start - at) as f64)
    } else {
        -h
    };
    let after = if end > at {
        h.min((end - at) as f64)
    } else {
        h
    };
    map[i].ease = map[i].ease.or(Some([before, after]));
    Ok(())
}

fn keyed(c: &Clip) -> Result<()> {
    if c.speed.as_ref().is_none_or(|s| s.sections.len() < 2) {
        return Err(Error::invalid(format!("{} has no speed keyframes", c.id)));
    }
    Ok(())
}

pub fn ease(p: &mut Project, ids: &[String], t: i64, ease: Ease) -> Result<()> {
    let (first, last) = sides(ease);
    for id in ids {
        let c = under(p, id, t)?;
        keyed(&c)?;
        let (mut map, skip) = sections(&c);
        let i = index(&map, skip + t - c.start);
        let tc = p.settings.timecode(t);
        if first && i == 0 {
            return Err(Error::invalid(format!(
                "the section of {id} under {tc} has no speed keyframe at its start"
            )));
        }
        if last && i + 1 == map.len() {
            return Err(Error::invalid(format!(
                "the section of {id} under {tc} has no speed keyframe at its end"
            )));
        }
        let edges = [skip, skip + c.duration];
        if i > 0 {
            eased(p, &c, &mut map, i, first, edges)?;
        }
        if i + 1 < map.len() {
            eased(p, &c, &mut map, i + 1, last, edges)?;
        }
        let n = with(&c, map, skip);
        retimed(p, &c, n, c.duration, false, "with this ease")?;
    }
    Ok(())
}

pub fn ease_keyframes(p: &mut Project, ids: &[String], pick: &Pick, ease: Ease) -> Result<()> {
    let on = match ease {
        Ease::Linear => false,
        Ease::EaseInOut => true,
        Ease::EaseIn | Ease::EaseOut => {
            return Err(Error::invalid(
                "a speed keyframe takes `linear` or `ease_in_out`: speed.ease eases one side \
                 of a section",
            ));
        }
    };
    for id in ids {
        let c = clip_of(p, id)?;
        keyed(&c)?;
        let keys = c.speed.as_ref().map(Speed::keyframes).unwrap_or_default();
        let chosen = picked_as(&c, &keys, pick, "speed")?;
        let (mut map, skip) = sections(&c);
        let edges = [skip, skip + c.duration];
        for i in 1..map.len() {
            if chosen.contains(&map[i].at) {
                eased(p, &c, &mut map, i, on, edges)?;
            }
        }
        let n = with(&c, map, skip);
        retimed(p, &c, n, c.duration, false, "with this ease")?;
    }
    Ok(())
}
