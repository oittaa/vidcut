use std::ops::Range;

use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{AngleSync, GroupAngle, Marks, Media, Project, SyncGroup};
use crate::ops::has_bin;
use crate::time::{format_timecode, timecode_frames};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Angle {
    pub media: String,
    pub at: i64,
    pub source: Marks,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Layout {
    pub start_timecode: String,
    #[serde(skip)]
    pub start: i64,
    pub frames: i64,
    pub angles: Vec<Angle>,
}

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
        AngleSync::Timecode => return Ok(origin(p, m) + part.start),
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

pub fn layout(p: &Project, ids: &[String], sync: AngleSync, full_extents: bool) -> Result<Layout> {
    if ids.is_empty() {
        return Err(Error::usage("name the media of the angles"));
    }
    let mut placed: Vec<(&str, i64, Range<i64>)> = Vec::with_capacity(ids.len());
    for id in ids {
        let m = p
            .media(id)
            .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
        if placed.iter().any(|(seen, ..)| *seen == m.id) {
            return Err(Error::invalid(format!("{id} is named twice")));
        }
        has_picture(m)?;
        let part = used(p, m, full_extents)?;
        placed.push((m.id.as_str(), record_start(p, m, sync, &part)?, part));
    }
    let first = placed.iter().map(|(_, at, _)| *at).min().unwrap_or(0);
    let last = placed
        .iter()
        .map(|(_, at, part)| at + part.end - part.start)
        .max()
        .unwrap_or(0);
    let (fps, drop) = (p.settings.fps, p.settings.counts_drop_frame());
    let start = match sync {
        AngleSync::Timecode => first,
        AngleSync::In | AngleSync::Out => timecode_frames(1, 0, 0, 0, fps, drop).unwrap_or(0),
    };
    Ok(Layout {
        start_timecode: start_timecode(p, start),
        start,
        frames: last - first,
        angles: placed
            .into_iter()
            .map(|(id, at, part)| Angle {
                media: id.to_string(),
                at: at - first,
                source: Marks {
                    mark_in: Some(part.start),
                    mark_out: Some(part.end),
                },
            })
            .collect(),
    })
}

pub fn create(
    p: &mut Project,
    ids: &[String],
    sync: AngleSync,
    name: String,
    bin: &str,
) -> Result<String> {
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
    let lined_up = layout(p, ids, sync, true)?;
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
    });
    Ok(id)
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
    }
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
    v
}
