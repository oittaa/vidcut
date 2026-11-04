use std::ops::Range;

use crate::error::{Error, Result};
use crate::model::{Clip, Media, Project, SyncGroup, TrackKind, uncovered};

use super::{Source, fix_audio, make_clip, place_block, track_above, tracks, unlocked};

pub struct Placed {
    pub clip: String,
    pub track: String,
    pub reference: String,
    pub playhead: i64,
}

fn no_common_time() -> Error {
    Error::invalid("No timecode in common with the timeline")
}

fn origin(p: &Project, m: &Media) -> i64 {
    m.timecode_at(p.settings.fps).round() as i64
}

fn lines_up(p: &Project, m: &Media) -> Result<()> {
    match &m.info.video {
        Some(v) if v.still => Err(Error::invalid(format!(
            "{} is a still: it has no timecode",
            m.name
        ))),
        Some(v) if v.fps == p.settings.fps => Ok(()),
        Some(_) => Err(Error::invalid(format!(
            "{} is not at the timeline's frame rate",
            m.name
        ))),
        None => Err(Error::invalid(format!(
            "{} has no picture to line up by",
            m.name
        ))),
    }
}

#[derive(Clone, Copy)]
struct Lining<'a> {
    p: &'a Project,
    group: Option<&'a SyncGroup>,
}

impl Lining<'_> {
    fn origin(&self, m: &Media) -> Option<i64> {
        match self.group {
            Some(g) => g.at_of(&m.id),
            None => Some(origin(self.p, m)),
        }
    }
}

fn shown(l: Lining<'_>, c: &Clip) -> Option<Range<i64>> {
    let m = l.p.media(&c.media).filter(|m| !m.is_still())?;
    let (a, b) = (c.source_at(c.start), c.source_at(c.end() - 1));
    let o = l.origin(m)?;
    Some(o + a.min(b)..o + a.max(b) + 1)
}

fn meets(l: Lining<'_>, c: &Clip, file: &Range<i64>) -> bool {
    shown(l, c).is_some_and(|s| s.start < file.end && file.start < s.end)
}

fn lag(l: Lining<'_>, c: &Clip) -> i64 {
    c.start - shown(l, c).map_or(0, |s| s.start)
}

fn disagree() -> Error {
    Error::invalid("A video clip and an audio clip disagree on the timecode")
}

fn first_where<'a>(
    l: Lining<'a>,
    file: &Range<i64>,
    test: impl Fn(&Clip) -> bool,
) -> Result<Option<&'a Clip>> {
    let mut found =
        l.p.tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(move |c| (t.kind, c)))
            .filter(|(_, c)| meets(l, c, file) && test(c));
    let Some((kind, c)) = found.next() else {
        return Ok(None);
    };
    match found.find(|(k, _)| *k != kind) {
        Some((_, other)) if lag(l, other) != lag(l, c) => Err(disagree()),
        _ => Ok(Some(c)),
    }
}

fn by_marks<'a>(
    l: Lining<'a>,
    file: &Range<i64>,
    a: i64,
    b: Option<i64>,
) -> Result<Option<&'a Clip>> {
    let at_in = first_where(l, file, |c| c.start <= a && a < c.end())?;
    if at_in.is_some() {
        return Ok(at_in);
    }
    let Some(b) = b else {
        return first_where(l, file, |_| true)?.map_or(Ok(None), |_| {
            Err(Error::invalid(
                "Source Overwrite needs a clip at the In mark or an Out mark",
            ))
        });
    };
    let in_range = first_where(l, file, |c| c.start < b && a < c.end())?;
    if in_range.is_some() {
        return Ok(in_range);
    }
    first_where(l, file, |_| true)
}

fn by_playhead<'a>(l: Lining<'a>, file: &Range<i64>, at: i64) -> Result<Option<&'a Clip>> {
    let on = |kind: TrackKind| -> Result<Option<&'a Clip>> {
        let mut clips =
            l.p.tracks
                .iter()
                .filter(|t| t.kind == kind)
                .flat_map(|t| t.clips.iter().map(move |c| (t, c)));
        let found = clips.find(|(_, c)| meets(l, c, file) && c.start <= at && at < c.end());
        let Some((track, c)) = found else {
            return Ok(None);
        };
        unchanged_speed(c)?;
        let time = shown(l, c).map_or(0, |s| s.start) + at - c.start;
        Ok(track
            .clips
            .iter()
            .find(|x| meets(l, x, file) && shown(l, x).is_some_and(|s| s.start <= time)))
    };
    match (on(TrackKind::Video)?, on(TrackKind::Audio)?) {
        (Some(v), Some(a)) if lag(l, v) != lag(l, a) => Err(disagree()),
        (None, None) => first_where(l, file, |_| true),
        (v, a) => Ok(v.or(a)),
    }
}

fn unchanged_speed(c: &Clip) -> Result<()> {
    if c.speed.is_some() {
        return Err(Error::invalid(format!(
            "clip {} has a changed speed: it can't set the timecode",
            c.id
        )));
    }
    Ok(())
}

pub fn file_start_at(p: &Project, m: &Media, at: i64, by_group: bool) -> Result<Option<i64>> {
    lines_up(p, m)?;
    let group = if by_group { p.group_of(&m.id) } else { None };
    let l = Lining { p, group };
    let o = l.origin(m).unwrap_or_default();
    let whole = o..o + p.media_frames(m);
    let Some(reference) = by_playhead(l, &whole, at)? else {
        return Ok(None);
    };
    unchanged_speed(reference)?;
    Ok(Some(lag(l, reference) + o))
}

pub fn source_overwrite(
    p: &mut Project,
    src: &Source,
    source_marked: bool,
    playhead: Option<i64>,
    by_group: bool,
) -> Result<Placed> {
    let m = p
        .media(&src.media)
        .ok_or_else(|| Error::not_found(format!("no media {}", src.media)))?;
    lines_up(p, m)?;
    if p.tracks.iter().all(|t| t.clips.is_empty()) {
        return Err(Error::invalid("The timeline is empty"));
    }
    let group = match by_group {
        true => Some(
            p.group_of(&m.id)
                .ok_or_else(|| Error::invalid(format!("{} is in no synced group", m.name)))?,
        ),
        false => None,
    };
    let l = Lining { p, group };
    let (o, frames) = (l.origin(m).unwrap_or_default(), p.media_frames(m));
    let whole = o..o + frames;
    let part = o + src.range.0..o + src.range.1;
    let marks = (p.marks.mark_in, p.marks.mark_out);
    let reference = match marks {
        (None, Some(_)) => {
            return Err(Error::invalid(
                "Source Overwrite can't use an Out mark with no In",
            ));
        }
        (Some(a), b) => by_marks(l, &whole, a, b)?,
        (None, None) if source_marked => match first_where(l, &part, |_| true)? {
            None if playhead.is_some_and(|at| at >= p.end()) => first_where(l, &whole, |_| true)?,
            found => found,
        },
        (None, None) => {
            let at = playhead.ok_or_else(|| Error::usage("at is required (no live playhead)"))?;
            by_playhead(l, &whole, at)?
        }
    }
    .ok_or_else(no_common_time)?;
    unchanged_speed(reference)?;
    let shown_by = p
        .media(&reference.media)
        .expect("a clip that matches has media");
    lines_up(p, shown_by)?;
    let file_start = lag(l, reference) + o;
    let (a, b) = match marks {
        (Some(a), b) => (a - file_start, b.map_or(frames, |b| b - file_start)),
        _ => src.range,
    };
    let (a, b) = (a.max(0).max(-file_start), b.min(frames));
    if a >= b {
        return Err(no_common_time());
    }
    if uncovered(a, b, &m.left_out) != [(a, b)] {
        return Err(Error::invalid(
            "Source Overwrite can't use a range with left-out frames",
        ));
    }
    let reference = reference.id.clone();
    let (start, end) = (file_start + a, file_start + b);
    let clip = make_clip(
        p,
        &Source {
            range: (a, b),
            ..src.clone()
        },
    )?;
    let (tracks_before, timeline_end) = (p.tracks.len(), p.end());
    let ti = if clip.video {
        track_above(p, start, end)
    } else {
        let audio = p.tracks_of(TrackKind::Audio).len();
        tracks::nth(p, TrackKind::Audio, audio)
    };
    unlocked(p, ti)?;
    let new_track = p.tracks.len() > tracks_before;
    let id = place_block(p, ti, vec![clip], start)
        .pop()
        .expect("one clip placed");
    fix_audio(p, ti);
    p.marks = Default::default();
    Ok(Placed {
        clip: id,
        track: p.tracks[ti].id.clone(),
        reference,
        playhead: if new_track {
            end.min(timeline_end)
        } else {
            end
        },
    })
}
