use std::ops::Range;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::model::{AngleSync, Marks, Media, Project};
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
    if sync != AngleSync::Timecode && m.marks.is_empty() {
        return Err(Error::invalid(format!(
            "{} has no marks to line up by",
            m.name
        )));
    }
    Ok(match sync {
        AngleSync::Timecode => origin(p, m) + part.start,
        AngleSync::In => 0,
        AngleSync::Out => part.start - part.end,
    })
}

pub fn layout(p: &Project, ids: &[String], sync: AngleSync, full_extents: bool) -> Result<Layout> {
    if ids.is_empty() {
        return Err(Error::usage("name the media of the angles"));
    }
    if full_extents && sync != AngleSync::Timecode {
        return Err(Error::invalid("full_extents needs sync by timecode"));
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
        start_timecode: format_timecode(start, fps, 0, drop),
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
