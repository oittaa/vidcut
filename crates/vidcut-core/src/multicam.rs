use std::ops::Range;

use serde::Serialize;
use serde_json::{Value, json};

use crate::editor::Env;
use crate::error::{Error, Result};
use crate::model::{AngleSync, GroupAngle, Marks, Media, Project, SyncGroup};
use crate::ops::has_bin;
use crate::time::{TimeContext, TimeSpec, format_timecode, timecode_frames};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Angle {
    pub media: String,
    pub at: i64,
    pub source: Marks,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SoundPair {
    pub a: String,
    pub b: String,
    pub shift: f64,
    pub score: f32,
    pub matched: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Layout {
    pub start_timecode: String,
    #[serde(skip)]
    pub start: i64,
    pub frames: i64,
    pub angles: Vec<Angle>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unmatched: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<SoundPair>,
}

pub const SOUND_MATCH_SCORE: f32 = 25.0;

pub const MULTI_SOURCE_ANGLES: usize = 9;

fn has_picture(m: &Media) -> Result<()> {
    match &m.info.video {
        Some(v) if v.still => Err(Error::invalid(format!("{} is a still", m.name))),
        Some(_) => Ok(()),
        None => Err(Error::invalid(format!("{} has no picture", m.name))),
    }
}

fn origin(p: &Project, m: &Media) -> i64 {
    m.info
        .timecode
        .map_or(0, |tc| tc.frames_at(p.settings.fps).floor() as i64)
}

fn used(p: &Project, m: &Media, whole: bool) -> Result<Range<i64>> {
    match (whole, m.marks.mark_in, m.marks.mark_out) {
        (true, ..) | (false, None, None) => Ok(0..p.media_limit(m)),
        (false, Some(a), Some(b)) => Ok(a..b),
        (false, ..) => Err(Error::invalid(format!(
            "{} has one mark: give it an In and an Out",
            m.name
        ))),
    }
}

fn record_start(p: &Project, m: &Media, sync: AngleSync, part: &Range<i64>) -> Result<i64> {
    let mark = match sync {
        AngleSync::Timecode | AngleSync::Sound => return Ok(origin(p, m) + part.start),
        AngleSync::In => m.marks.mark_in,
        AngleSync::Out => m.marks.mark_out,
    };
    let mark =
        mark.ok_or_else(|| Error::invalid(format!("{} has no mark to line up by", m.name)))?;
    Ok(part.start - mark)
}

fn start_timecode(p: &Project, start: i64) -> String {
    let (fps, drop) = (p.settings.fps, p.settings.counts_drop_frame());
    format_timecode(start, fps, 0, drop)
}

pub fn layout(
    p: &Project,
    env: &dyn Env,
    ids: &[String],
    sync: AngleSync,
    full_extents: bool,
) -> Result<Layout> {
    if ids.is_empty() {
        return Err(Error::usage("name the media of the angles"));
    }
    let mut placed: Vec<(&Media, i64, Range<i64>)> = Vec::with_capacity(ids.len());
    for id in ids {
        let m = p
            .media(id)
            .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
        if placed.iter().any(|(seen, ..)| seen.id == m.id) {
            return Err(Error::invalid(format!("{id} is named twice")));
        }
        has_picture(m)?;
        if sync == AngleSync::Sound && !full_extents && !m.marks.is_empty() {
            return Err(Error::invalid(format!(
                "{} has marks: sync by sound needs whole files",
                m.name
            )));
        }
        let part = used(p, m, full_extents)?;
        placed.push((m, record_start(p, m, sync, &part)?, part));
    }
    let (unmatched, pairs) = match sync {
        AngleSync::Sound => by_sound(p, env, &mut placed)?,
        _ => Default::default(),
    };
    let first = placed.iter().map(|(_, at, _)| *at).min().unwrap_or(0);
    let last = placed
        .iter()
        .map(|(_, at, part)| at + part.end - part.start)
        .max()
        .unwrap_or(0);
    let (fps, drop) = (p.settings.fps, p.settings.counts_drop_frame());
    let start = match sync {
        AngleSync::Timecode => first,
        AngleSync::In | AngleSync::Out | AngleSync::Sound => {
            timecode_frames(1, 0, 0, 0, fps, drop).unwrap_or(0)
        }
    };
    Ok(Layout {
        start_timecode: start_timecode(p, start),
        start,
        frames: last - first,
        angles: placed
            .into_iter()
            .map(|(m, at, part)| Angle {
                media: m.id.clone(),
                at: at - first,
                source: Marks {
                    mark_in: Some(part.start),
                    mark_out: Some(part.end),
                },
            })
            .collect(),
        unmatched,
        pairs,
    })
}

fn by_sound(
    p: &Project,
    env: &dyn Env,
    placed: &mut [(&Media, i64, Range<i64>)],
) -> Result<(Vec<String>, Vec<SoundPair>)> {
    let n = placed.len();
    let mut found = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            if let Some(s) = env.sound_match(placed[i].0, placed[j].0)? {
                found.push((i, j, s));
            }
        }
    }
    found.sort_by(|a, b| b.2.score.total_cmp(&a.2.score));
    let mut party: Vec<usize> = (0..n).collect();
    let mut secs = vec![0.0f64; n];
    for (i, j, s) in found.iter().filter(|x| x.2.score >= SOUND_MATCH_SCORE) {
        let (keep, join) = (party[*i], party[*j]);
        if keep == join {
            continue;
        }
        let by = secs[*i] + s.shift - secs[*j];
        for k in 0..n {
            if party[k] == join {
                secs[k] += by;
                party[k] = keep;
            }
        }
    }
    let fps = p.settings.fps.as_f64();
    let mut base = 0;
    let mut unmatched = Vec::new();
    for k in 0..n {
        if party[k] == k && party.iter().filter(|o| **o == k).count() == 1 {
            placed[k].1 = 0;
            unmatched.push(placed[k].0.id.clone());
        }
        if !party[..k].contains(&party[k]) && party.iter().filter(|o| **o == party[k]).count() > 1 {
            let who = party[k];
            let least = (0..n)
                .filter(|o| party[*o] == who)
                .map(|o| secs[o])
                .fold(f64::INFINITY, f64::min);
            let mut end = base;
            for o in (0..n).filter(|o| party[*o] == who) {
                let at = base + ((secs[o] - least) * fps + 1e-6).round() as i64;
                placed[o].1 = at;
                end = end.max(at + placed[o].2.end - placed[o].2.start);
            }
            base = end + 1;
        }
    }
    let pairs = found
        .iter()
        .map(|(i, j, s)| SoundPair {
            a: placed[*i].0.id.clone(),
            b: placed[*j].0.id.clone(),
            shift: s.shift,
            score: s.score,
            matched: s.score >= SOUND_MATCH_SCORE,
        })
        .collect();
    Ok((unmatched, pairs))
}

pub fn create(
    p: &mut Project,
    env: &dyn Env,
    ids: &[String],
    sync: AngleSync,
    name: String,
    bin: &str,
) -> Result<(String, Vec<String>)> {
    if ids.len() < 2 {
        return Err(Error::invalid("a synced group needs two files or more"));
    }
    if !has_bin(p, bin) {
        return Err(Error::not_found(format!(
            "no bin {bin:?}; add it with bins.add"
        )));
    }
    if let Some((id, g)) = ids.iter().find_map(|id| Some((id, p.group_of(id)?))) {
        return Err(Error::invalid(format!(
            "{id} is in the synced group {}",
            g.name
        )));
    }
    let lined_up = layout(p, env, ids, sync, true)?;
    let id = p.new_group_id();
    p.groups.push(SyncGroup {
        id: id.clone(),
        name,
        bin: bin.to_string(),
        start: lined_up.start,
        angles: lined_up
            .angles
            .into_iter()
            .map(|a| GroupAngle {
                media: a.media,
                at: a.at,
            })
            .collect(),
        marks: Marks::default(),
    });
    Ok((id, lined_up.unmatched))
}

pub fn group_mut<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut SyncGroup> {
    p.groups
        .iter_mut()
        .find(|g| g.id == id)
        .ok_or_else(|| Error::not_found(format!("no synced group {id}")))
}

pub fn remove(p: &mut Project, id: &str) -> Result<()> {
    group_mut(p, id)?;
    p.groups.retain(|g| g.id != id);
    Ok(())
}

pub fn move_to_bin(p: &mut Project, id: &str, bin: &str) -> Result<()> {
    if !has_bin(p, bin) {
        return Err(Error::not_found(format!(
            "no bin {bin:?}; add it with bins.add"
        )));
    }
    group_mut(p, id)?.bin = bin.to_string();
    Ok(())
}

fn layout_of(p: &Project, g: &SyncGroup) -> Layout {
    let angles: Vec<Angle> = g
        .angles
        .iter()
        .filter_map(|a| {
            let m = p.media(&a.media)?;
            Some(Angle {
                media: a.media.clone(),
                at: a.at,
                source: Marks {
                    mark_in: Some(0),
                    mark_out: Some(p.media_limit(m)),
                },
            })
        })
        .collect();
    Layout {
        start_timecode: start_timecode(p, g.start),
        start: g.start,
        frames: angles
            .iter()
            .map(|a| a.at + a.source.mark_out.unwrap_or(0))
            .max()
            .unwrap_or(0),
        angles,
        unmatched: Vec::new(),
        pairs: Vec::new(),
    }
}

pub fn frames(p: &Project, g: &SyncGroup) -> i64 {
    layout_of(p, g).frames
}

pub fn set_marks(
    p: &mut Project,
    id: &str,
    mark_in: Option<&TimeSpec>,
    out: Option<&TimeSpec>,
) -> Result<Marks> {
    if mark_in.is_none() && out.is_none() {
        return Err(Error::usage("multicam.marks.set needs `in` and/or `out`"));
    }
    let g = group_mut(p, id)?.clone();
    let end = frames(p, &g);
    let ctx = TimeContext {
        fps: p.settings.fps,
        start_tc: g.start,
        drop_frame: p.settings.counts_drop_frame(),
        end,
        mark_in: g.marks.mark_in,
        mark_out: g.marks.mark_out,
        playhead: None,
    };
    let mut marks = g.marks;
    if let Some(t) = mark_in {
        marks.mark_in = Some(ctx.resolve(t)?);
    }
    if let Some(t) = out {
        marks.mark_out = Some(ctx.resolve(t)?);
    }
    if let (Some(i), Some(o)) = (marks.mark_in, marks.mark_out)
        && i >= o
    {
        return Err(Error::invalid(format!("In ({i}) must be before Out ({o})")));
    }
    if marks.mark_in.is_some_and(|i| i < 0 || i >= end)
        || marks.mark_out.is_some_and(|o| o < 1 || o > end)
    {
        return Err(Error::invalid(format!(
            "marks must be inside {id} (0..{end} frames)"
        )));
    }
    group_mut(p, id)?.marks = marks.clone();
    Ok(marks)
}

pub fn marked_range(p: &Project, g: &SyncGroup, media: &str) -> Result<(i64, i64)> {
    let m = p
        .media(media)
        .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
    let at = g
        .at_of(media)
        .ok_or_else(|| Error::invalid(format!("{} is not in {}", m.name, g.name)))?;
    let frames = p.media_frames(m);
    let a = g.marks.mark_in.map_or(0, |i| i - at);
    let b = g.marks.mark_out.map_or(frames, |o| o - at);
    if a < 0 || b > frames || a >= b {
        return Err(Error::invalid(format!(
            "The marks of {} are outside {}",
            g.name, m.name
        )));
    }
    Ok((a, b))
}

pub fn group_json(p: &Project, g: &SyncGroup) -> Value {
    let lined_up = layout_of(p, g);
    let mut v = json!({
        "id": g.id,
        "name": g.name,
        "start_timecode": lined_up.start_timecode,
        "frames": lined_up.frames,
        "angles": lined_up.angles,
    });
    if !g.bin.is_empty() {
        v["bin"] = json!(g.bin);
    }
    if !g.marks.is_empty() {
        v["marks"] = json!(g.marks);
    }
    v
}
