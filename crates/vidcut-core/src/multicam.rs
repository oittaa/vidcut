use std::cmp::Ordering;
use std::ops::Range;

use serde::Serialize;
use serde_json::{Value, json};

use crate::editor::Env;
use crate::error::{Error, Result};
use crate::model::{AngleSync, GroupAngle, Marks, Media, Project, SyncGroup};
use crate::ops::{self, has_bin};
use crate::time::{Rational, TimeContext, TimeSpec, format_timecode, timecode_frames};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Angle {
    pub media: String,
    pub at: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub hidden: i64,
    pub source: Marks,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

fn is_zero(frames: &i64) -> bool {
    *frames == 0
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

pub const SYNC_CLIPS_CAMERAS: usize = 12;

const PICK_SECONDS: f64 = 5.0;

const LONGEST_GAP_SECONDS: f64 = 600.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncRow {
    pub number: usize,
    pub media: String,
    pub camera: String,
    pub frames: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<i64>,
    pub active: bool,
}

fn has_picture(m: &Media) -> Result<()> {
    match &m.info.video {
        Some(v) if v.still => Err(Error::invalid(format!("{} is a still", m.name))),
        Some(_) => Ok(()),
        None => Err(Error::invalid(format!("{} has no picture", m.name))),
    }
}

fn origin(p: &Project, m: &Media) -> i64 {
    m.timecode_at(p.settings.fps).floor() as i64
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
    let start = group_start(p, sync, first);
    Ok(Layout {
        start_timecode: start_timecode(p, start),
        start,
        frames: last - first,
        angles: placed
            .into_iter()
            .map(|(m, at, part)| Angle {
                media: m.id.clone(),
                at: at - first,
                hidden: 0,
                source: Marks {
                    mark_in: Some(part.start),
                    mark_out: Some(part.end),
                },
                locked: false,
            })
            .collect(),
        unmatched,
        pairs,
    })
}

fn one_hour(p: &Project) -> i64 {
    let (fps, drop) = (p.settings.fps, p.settings.counts_drop_frame());
    timecode_frames(1, 0, 0, 0, fps, drop).unwrap_or(0)
}

fn group_start(p: &Project, sync: AngleSync, first: i64) -> i64 {
    match sync {
        AngleSync::Timecode => first,
        AngleSync::In | AngleSync::Out | AngleSync::Sound => one_hour(p),
    }
}

pub fn ruler_start(p: &Project, ids: &[String]) -> i64 {
    let starts = || {
        ids.iter()
            .filter_map(move |id| p.media(id))
            .map(move |m| origin(p, m))
    };
    if starts().all(|start| start == 0) {
        return one_hour(p);
    }
    starts().min().unwrap_or_default()
}

pub fn close_far_gaps(fps: Rational, spans: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let longest = (LONGEST_GAP_SECONDS * fps.as_f64()).round() as i64;
    let short = fps.as_f64().round() as i64;
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|i| spans[*i].0);
    let first = order.first().map_or(0, |i| spans[*i].0);
    let (mut hidden, mut reach) = (0, first);
    let mut shown = vec![(0, 0); spans.len()];
    for i in order {
        let (real, frames) = spans[i];
        if real - reach > longest {
            hidden += real - reach - short;
        }
        shown[i] = (real - first - hidden, hidden);
        reach = reach.max(real + frames);
    }
    shown
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
        let shift = s.shift + placed[*j].0.skipped() - placed[*i].0.skipped();
        let by = secs[*i] + shift - secs[*j];
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

fn by_hand(p: &Project, ids: &[String], angles: &[GroupAngle]) -> Result<Vec<GroupAngle>> {
    if let Some(a) = angles.iter().find(|a| !ids.contains(&a.media)) {
        return Err(Error::invalid(format!(
            "{} is not one of the files",
            a.media
        )));
    }
    let wanted = ids
        .iter()
        .map(|id| {
            let a = angles
                .iter()
                .find(|a| a.media == *id)
                .ok_or_else(|| Error::invalid(format!("`angles` leaves out {id}")))?;
            if a.at < 0 || a.hidden < 0 {
                return Err(Error::invalid(format!("{id} can't start before the group")));
            }
            let frames = p.media(id).map_or(0, |m| p.media_frames(m));
            Ok((a, (a.at + a.hidden, frames)))
        })
        .collect::<Result<Vec<_>>>()?;
    let spans: Vec<(i64, i64)> = wanted.iter().map(|(_, span)| *span).collect();
    Ok(wanted
        .iter()
        .zip(close_far_gaps(p.settings.fps, &spans))
        .map(|((a, _), (at, hidden))| GroupAngle {
            media: a.media.clone(),
            at,
            hidden,
            locked: a.locked,
        })
        .collect())
}

pub struct NewGroup<'a> {
    pub sync: AngleSync,
    pub angles: Option<&'a [GroupAngle]>,
    pub name: String,
    pub bin: &'a str,
}

pub fn create(
    p: &mut Project,
    env: &dyn Env,
    ids: &[String],
    new: NewGroup<'_>,
) -> Result<(String, Vec<String>)> {
    let NewGroup {
        sync,
        angles,
        name,
        bin,
    } = new;
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
    let (start, angles, unmatched) = match angles {
        Some(angles) => {
            layout(p, env, ids, AngleSync::Timecode, true)?;
            (one_hour(p), by_hand(p, ids, angles)?, Vec::new())
        }
        None => {
            let lined_up = layout(p, env, ids, sync, true)?;
            let unmatched = lined_up.unmatched;
            let angles = lined_up
                .angles
                .into_iter()
                .map(|a| GroupAngle {
                    locked: !unmatched.contains(&a.media),
                    media: a.media,
                    at: a.at,
                    hidden: 0,
                })
                .collect();
            (lined_up.start, angles, unmatched)
        }
    };
    let id = p.new_group_id();
    p.groups.push(SyncGroup {
        id: id.clone(),
        name,
        bin: bin.to_string(),
        start,
        angles,
        marks: Marks::default(),
    });
    Ok((id, unmatched))
}

pub fn set_angles(p: &mut Project, id: &str, angles: &[GroupAngle]) -> Result<()> {
    let g = group_mut(p, id)?;
    let ids: Vec<String> = g.angles.iter().map(|a| a.media.clone()).collect();
    let placed = by_hand(p, &ids, angles)?;
    group_mut(p, id)?.angles = placed;
    Ok(())
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
                hidden: a.hidden,
                source: Marks {
                    mark_in: Some(0),
                    mark_out: Some(p.media_limit(m)),
                },
                locked: a.locked,
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

fn sync_start(p: &Project, m: &Media, at: i64) -> Result<Option<i64>> {
    let by_group = p.group_of(&m.id).is_some();
    if !by_group && m.info.timecode.is_none() {
        return Err(Error::invalid(format!(
            "{} has no timecode and no synced group",
            m.name
        )));
    }
    ops::file_start_at(p, m, at, by_group)
}

pub fn camera_order(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

pub fn cameras<'a>(p: &'a Project, bin: &str) -> Result<Vec<(&'a Media, &'a str)>> {
    let mut files: Vec<(&Media, &str)> = Vec::new();
    for m in p
        .media
        .iter()
        .filter(|m| bin.is_empty() || ops::within(&m.bin, bin))
    {
        has_picture(m)?;
        let camera = m
            .camera
            .as_deref()
            .ok_or_else(|| Error::invalid(format!("{} has no camera name", m.name)))?;
        if let Some((other, _)) = files.iter().find(|(_, c)| *c == camera) {
            return Err(Error::invalid(format!(
                "{} and {} have one camera name",
                other.name, m.name
            )));
        }
        files.push((m, camera));
    }
    if files.len() > MULTI_SOURCE_ANGLES {
        return Err(Error::invalid(format!(
            "A bin to sync holds {MULTI_SOURCE_ANGLES} cameras at most"
        )));
    }
    let group_of = |m: &Media| p.group_of(&m.id).map(|g| g.id.as_str());
    if let Some(((first, _), (other, _))) = files.first().zip(
        files
            .iter()
            .find(|(m, _)| group_of(m) != files.first().and_then(|(f, _)| group_of(f))),
    ) {
        return Err(Error::invalid(format!(
            "{} and {} are not in one synced group",
            first.name, other.name
        )));
    }
    files.sort_by(|a, b| camera_order(a.1, b.1));
    Ok(files)
}

fn window_order(mut files: Vec<&Media>) -> Vec<&Media> {
    files.sort_by(|a, b| match (&a.camera, &b.camera) {
        (Some(x), Some(y)) => camera_order(x, y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    files
}

pub fn group_files<'a>(p: &'a Project, g: &SyncGroup) -> Vec<&'a Media> {
    window_order(
        p.media
            .iter()
            .filter(|m| g.at_of(&m.id).is_some())
            .collect(),
    )
}

pub fn sync_clips_files<'a>(
    p: &'a Project,
    bin: &str,
    selected: &[String],
) -> Result<Vec<&'a Media>> {
    let shown = || {
        p.media
            .iter()
            .filter(move |m| bin.is_empty() || ops::within(&m.bin, bin))
    };
    let picked: Vec<&Media> = shown().filter(|m| selected.contains(&m.id)).collect();
    let files: Vec<&Media> = match picked.len() {
        0 => shown().collect(),
        1 => return Err(Error::invalid("Select two files or more, or none")),
        _ => picked,
    };
    for (i, m) in files.iter().enumerate() {
        has_picture(m)?;
        if let Some(other) = files[..i]
            .iter()
            .find(|o| m.camera.is_some() && o.camera == m.camera)
        {
            return Err(Error::invalid(format!(
                "{} and {} have one camera name",
                other.name, m.name
            )));
        }
    }
    if files.len() < 2 {
        return Err(Error::invalid("A sync needs two files or more"));
    }
    if files.len() > SYNC_CLIPS_CAMERAS {
        return Err(Error::invalid(format!(
            "The window holds {SYNC_CLIPS_CAMERAS} cameras at most"
        )));
    }
    let group_of = |m: &Media| p.group_of(&m.id).map(|g| g.id.as_str());
    if let Some(other) = files.iter().find(|m| group_of(m) != group_of(files[0])) {
        return Err(Error::invalid(format!(
            "{} and {} are not in one synced group",
            files[0].name, other.name
        )));
    }
    Ok(window_order(files))
}

pub fn sync_rows(p: &Project, bin: &str, at: i64) -> Result<Vec<SyncRow>> {
    if !bin.is_empty() && !has_bin(p, bin) {
        return Err(Error::not_found(format!("no bin {bin:?}")));
    }
    let shown = p.shown_clip(at).map(|c| c.media.as_str());
    cameras(p, bin)?
        .into_iter()
        .enumerate()
        .map(|(i, (m, camera))| {
            let start = sync_start(p, m, at)?;
            let frames = p.media_frames(m);
            Ok(SyncRow {
                number: i + 1,
                media: m.id.clone(),
                camera: camera.to_string(),
                frames,
                start,
                frame: start.map(|s| at - s).filter(|f| (0..frames).contains(f)),
                active: shown == Some(m.id.as_str()),
            })
        })
        .collect()
}

pub fn pick(p: &mut Project, media: &str, at: i64) -> Result<Marks> {
    let m = p
        .media(media)
        .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
    has_picture(m)?;
    let frames = p.media_frames(m);
    let start = sync_start(p, m, at)?
        .filter(|s| (0..frames).contains(&(at - s)))
        .ok_or_else(|| Error::invalid(format!("{} has no frame at the playhead", m.name)))?;
    let end = p.end();
    let (a, b) = if at > end {
        (end - start, at - start)
    } else {
        let length = (PICK_SECONDS * p.settings.fps.as_f64()).round() as i64;
        (at - start, at - start + length)
    };
    let (a, b) = (a.max(0), b.min(frames));
    if a >= b {
        return Err(Error::invalid(format!(
            "{} has no frames before the playhead",
            m.name
        )));
    }
    ops::set_media_marks(p, media, Some(Some(a)), Some(Some(b)))?;
    Ok(Marks {
        mark_in: Some(a),
        mark_out: Some(b),
    })
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
