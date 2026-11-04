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

fn first_where<'a>(
    l: Lining<'a>,
    file: &Range<i64>,
    test: impl Fn(&Clip) -> bool,
) -> Option<&'a Clip> {
    l.p.tracks
        .iter()
        .flat_map(|t| &t.clips)
        .find(|c| meets(l, c, file) && test(c))
}

fn by_marks<'a>(
    l: Lining<'a>,
    file: &Range<i64>,
    a: Option<i64>,
    b: Option<i64>,
) -> Option<&'a Clip> {
    a.and_then(|a| first_where(l, file, |c| c.start <= a && a < c.end()))
        .or_else(|| {
            b.and_then(|b| first_where(l, file, |c| a.is_none_or(|a| a < c.end()) && c.start < b))
        })
        .or_else(|| first_where(l, file, |_| true))
}

fn by_playhead<'a>(l: Lining<'a>, file: &Range<i64>, at: i64) -> Option<&'a Clip> {
    let on = |kind: TrackKind| -> Option<&'a Clip> {
        let mut clips =
            l.p.tracks
                .iter()
                .filter(|t| t.kind == kind)
                .flat_map(|t| t.clips.iter().map(move |c| (t, c)));
        let (track, c) = clips.find(|(_, c)| meets(l, c, file) && c.start <= at && at < c.end())?;
        let time = shown(l, c).map_or(0, |s| s.start) + at - c.start;
        track
            .clips
            .iter()
            .find(|x| meets(l, x, file) && shown(l, x).is_some_and(|s| s.start <= time))
    };
    on(TrackKind::Video)
        .or_else(|| on(TrackKind::Audio))
        .or_else(|| first_where(l, file, |_| true))
}

pub fn file_start_at(p: &Project, m: &Media, at: i64, group: Option<&SyncGroup>) -> Option<i64> {
    let l = Lining { p, group };
    let o = l.origin(m).unwrap_or_default();
    let whole = o..o + p.media_frames(m);
    at_playhead(l, &whole, at).map(|reference| lag(l, reference) + o)
}

fn at_playhead<'a>(l: Lining<'a>, file: &Range<i64>, at: i64) -> Option<&'a Clip> {
    let here = |kind: TrackKind| {
        l.p.tracks
            .iter()
            .filter(move |t| t.kind == kind)
            .flat_map(|t| &t.clips)
            .filter(move |c| c.start <= at && at < c.end())
    };
    let met = |kind: TrackKind| here(kind).find(|c| meets(l, c, file));
    met(TrackKind::Video)
        .or_else(|| met(TrackKind::Audio))
        .or_else(|| {
            here(TrackKind::Video)
                .chain(here(TrackKind::Audio))
                .find(|c| shown(l, c).is_some())
        })
        .or_else(|| first_where(l, file, |_| true))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLining {
    Playhead,
    Marked,
    Grid,
}

pub fn source_overwrite(
    p: &mut Project,
    src: &Source,
    lining: SourceLining,
    playhead: Option<i64>,
    by_group: bool,
) -> Result<Placed> {
    let m = p
        .media(&src.media)
        .ok_or_else(|| Error::not_found(format!("no media {}", src.media)))?;
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
        (None, None) => match lining {
            SourceLining::Grid => {
                let at = needs_playhead(playhead)?;
                first_where(l, &part, |c| c.start <= at && at < c.end())
                    .or_else(|| first_where(l, &part, |_| true))
            }
            SourceLining::Marked => match first_where(l, &part, |_| true) {
                None if playhead.is_some_and(|at| at >= p.end()) => {
                    first_where(l, &whole, |_| true)
                }
                found => found,
            },
            SourceLining::Playhead => by_playhead(l, &whole, needs_playhead(playhead)?),
        },
        (a, b) => by_marks(l, &whole, a, b),
    }
    .ok_or_else(no_common_time)?;
    let file_start = lag(l, reference) + o;
    let (a, b) = match marks {
        (None, None) => src.range,
        (a, b) => (
            a.map_or(0, |a| a - file_start),
            b.map_or(frames, |b| b - file_start),
        ),
    };
    let (a, b) = (a.max(0).max(-file_start), b.min(frames));
    if a >= b {
        return Err(no_common_time());
    }
    let kept = uncovered(a, b, &m.left_out);
    let (Some(first), Some(last)) = (kept.first(), kept.last()) else {
        return Err(Error::invalid(
            "every frame of the range is left out (media.leave_out)",
        ));
    };
    let reference = reference.id.clone();
    let (start, end) = (file_start + first.0, file_start + last.1);
    let mut clips = Vec::with_capacity(kept.len());
    for &range in &kept {
        clips.push(make_clip(
            p,
            &Source {
                range,
                ..src.clone()
            },
        )?);
    }
    let (tracks_before, timeline_end) = (p.tracks.len(), p.end());
    let ti = if clips[0].video {
        track_above(p, start, end)
    } else {
        let audio = p.tracks_of(TrackKind::Audio).len();
        tracks::nth(p, TrackKind::Audio, audio)
    };
    unlocked(p, ti)?;
    let stops_at_end = clips[0].video && p.tracks.len() > tracks_before;
    let mut ids = Vec::with_capacity(kept.len());
    for (clip, range) in clips.into_iter().zip(&kept) {
        ids.extend(place_block(p, ti, vec![clip], file_start + range.0));
    }
    fix_audio(p, ti);
    Ok(Placed {
        clip: ids.swap_remove(0),
        track: p.tracks[ti].id.clone(),
        reference,
        playhead: if stops_at_end && lining != SourceLining::Grid {
            end.min(timeline_end)
        } else {
            end
        },
    })
}

fn needs_playhead(playhead: Option<i64>) -> Result<i64> {
    playhead.ok_or_else(|| Error::usage("at is required (no live playhead)"))
}
