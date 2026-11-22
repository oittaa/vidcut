use std::ops::Range;

use crate::error::{Error, Result};
use crate::model::{Clip, Compound, Media, Project, SyncGroup, TrackKind, uncovered};

use super::titles::default_length;
use super::{Source, make_clip, put, track_above, tracks, unlocked};

pub struct Placed {
    pub clip: String,
    pub track: String,
    pub reference: String,
    pub playhead: i64,
}

fn no_common_time() -> Error {
    Error::invalid("No timecode in common with the timeline")
}

fn origin(p: &Project, m: &Media) -> f64 {
    m.timecode_at(p.fps())
}

#[derive(Clone, Copy)]
struct Lining<'a> {
    p: &'a Project,
    group: Option<&'a SyncGroup>,
}

impl Lining<'_> {
    fn origin(&self, m: &Media) -> Option<f64> {
        match self.group {
            Some(g) => g
                .at_of(&m.id)
                .map(|at| at as f64 * self.p.fps().over(self.p.settings.fps).as_f64()),
            None => Some(origin(self.p, m)),
        }
    }

    fn timed(&self, c: &Clip) -> Option<Timed<'_>> {
        match self.p.clip_compound(c) {
            Some(k) => self.group.is_none().then_some(Timed::Compound(k)),
            None => (self.p.media(&c.media))
                .filter(|m| !m.is_still())
                .map(Timed::Media),
        }
    }

    fn start(&self, c: &Clip) -> Option<f64> {
        match self.timed(c)? {
            Timed::Media(m) => self.origin(m),
            Timed::Compound(k) => Some(k.timecode_at(self.p.fps())),
        }
    }

    fn placed(&self, c: &Clip) -> Option<f64> {
        match self.timed(c)? {
            Timed::Media(m) => place(*self, m),
            Timed::Compound(k) => Some(k.timecode_at(self.p.settings.fps)),
        }
    }
}

#[derive(Clone, Copy)]
enum Timed<'a> {
    Media(&'a Media),
    Compound(&'a Compound),
}

fn shown(l: Lining<'_>, c: &Clip) -> Option<Range<i64>> {
    let o = l.start(c)?.floor() as i64;
    let (a, b) = (c.source_at(c.start), c.source_at(c.end() - 1));
    Some(o + a.min(b)..o + a.max(b) + 1)
}

fn exact_start(l: Lining<'_>, c: &Clip) -> Option<f64> {
    let o = l.start(c)?;
    let a = c.source_at(c.start).min(c.source_at(c.end() - 1));
    Some(o + a as f64 + c.phase)
}

fn meets(l: Lining<'_>, c: &Clip, file: &Range<i64>) -> bool {
    shown(l, c).is_some_and(|s| s.start < file.end && file.start < s.end)
}

fn lag(l: Lining<'_>, c: &Clip) -> f64 {
    c.start as f64 - exact_start(l, c).unwrap_or(0.0)
}

const SLACK: f64 = 1e-6;

fn place(l: Lining<'_>, m: &Media) -> Option<f64> {
    match l.group {
        Some(g) => g.at_of(&m.id).map(|at| at as f64),
        None => Some(m.timecode_at(l.p.settings.fps)),
    }
}

#[derive(Clone, Copy)]
struct Grid {
    zero: f64,
    step: f64,
}

impl Grid {
    fn of(l: Lining<'_>, reference: &Clip) -> Self {
        let step = l.p.settings.fps.over(l.p.fps()).as_f64();
        let first = (reference.source_at(reference.start))
            .min(reference.source_at(reference.end() - 1)) as f64
            + reference.phase;
        let start = l.placed(reference).map_or(0.0, |x| x + first * step);
        Self {
            zero: start - reference.start as f64 * step,
            step,
        }
    }

    fn place(self, at: i64) -> i64 {
        (self.zero + at as f64 * self.step - SLACK).ceil() as i64
    }

    fn first(self, place: i64) -> i64 {
        (((place - 1) as f64 - self.zero) / self.step + SLACK).floor() as i64 + 1
    }
}

#[derive(Clone, Copy)]
struct Spot {
    place: i64,
    rate: f64,
}

impl Spot {
    fn of(l: Lining<'_>, m: &Media) -> Option<Self> {
        let project = l.p.settings.fps;
        Some(Self {
            place: (place(l, m)? + SLACK).floor() as i64,
            rate: m.rate(project).over(project).as_f64(),
        })
    }

    fn frame(self, place: i64) -> i64 {
        ((place - self.place) as f64 * self.rate + SLACK).floor() as i64
    }

    fn shows(self, frame: i64) -> i64 {
        self.place + (frame as f64 / self.rate - SLACK).ceil() as i64
    }
}

pub struct Synced {
    pub frame: i64,
    pub offset: i64,
}

pub fn synced_at(p: &Project, m: &Media, at: i64, group: Option<&SyncGroup>) -> Option<Synced> {
    let l = Lining { p, group };
    let base = l.origin(m).unwrap_or_default().floor() as i64;
    let whole = base..base + p.media_frames(m);
    let grid = Grid::of(l, at_playhead(l, &whole, at)?);
    let spot = Spot::of(l, m)?;
    let place = grid.place(at);
    Some(Synced {
        frame: spot.frame(place),
        offset: place - spot.place,
    })
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

pub fn file_start_at(p: &Project, m: &Media, at: i64, group: Option<&SyncGroup>) -> Option<f64> {
    let l = Lining { p, group };
    let o = l.origin(m).unwrap_or_default();
    let base = o.floor() as i64;
    let whole = base..base + p.media_frames(m);
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
    own: Option<(i64, i64)>,
    lining: SourceLining,
    playhead: Option<i64>,
    by_group: bool,
) -> Result<Placed> {
    let k = match &src.compound {
        Some(id) => Some(
            p.pooled_compound(id)
                .ok_or_else(|| Error::not_found(format!("no compound clip {id}")))?,
        ),
        None => None,
    };
    let m = match (&src.generator, k) {
        (None, None) => Some(
            p.media(&src.media)
                .ok_or_else(|| Error::not_found(format!("no media {}", src.media)))?,
        ),
        _ => None,
    };
    if p.tracks.iter().all(|t| t.clips.is_empty()) {
        return Err(Error::invalid("The timeline is empty"));
    }
    let group = match (by_group, m, k) {
        (false, _, _) => None,
        (true, Some(m), _) => Some(
            p.group_of(&m.id)
                .ok_or_else(|| Error::invalid(format!("{} is in no synced group", m.name)))?,
        ),
        (true, None, Some(_)) => {
            return Err(Error::invalid("A compound clip is in no synced group"));
        }
        (true, None, None) => return Err(Error::invalid("A generator is in no synced group")),
    };
    let l = Lining { p, group };
    let (o, frames) = match (m, k) {
        (Some(m), _) => (l.origin(m).unwrap_or_default(), p.media_frames(m)),
        (None, Some(k)) => (k.timecode_at(p.fps()), p.compound_frames(k)),
        (None, None) => (0.0, default_length(p).max(src.range.1)),
    };
    let base = o.floor() as i64;
    let whole = base..base + frames;
    let part = base + src.range.0..base + src.range.1;
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
    let grid = Grid::of(l, reference);
    let spot = m.and_then(|m| Spot::of(l, m));
    let plain = (lag(l, reference) + o).round() as i64;
    let first = |frame: i64| match spot {
        Some(s) => grid.first(s.shows(frame)),
        None => plain + frame,
    };
    let shown = |at: i64| match spot {
        Some(s) => s.frame(grid.place(at)),
        None => at - plain,
    };
    let (own, count) = match m {
        Some(m) => (
            own.unwrap_or_else(|| p.clip_span(m, src.range)),
            p.clip_frames(m),
        ),
        None => (src.range, frames),
    };
    let file = (first(0), first(count));
    let (from, to) = match marks {
        (None, None) => {
            let start = match playhead {
                Some(at) if shown(at) == own.0 => at,
                _ => first(own.0),
            };
            (start, start + src.range.1 - src.range.0)
        }
        (a, b) => (a.unwrap_or(file.0), b.unwrap_or(file.1)),
    };
    let a = from.max(0).max(file.0);
    let b = if src.generator.is_none() {
        to.min(file.1)
    } else {
        to
    };
    if a >= b {
        return Err(no_common_time());
    }
    let marked = marks == (None, None) && a == from;
    let (first_source, phase) = match (marked, m) {
        (true, _) => (src.range.0, src.phase),
        (false, Some(m)) => {
            let ((start, _), phase) = p.on_timeline(m, (shown(a).max(0), count));
            (start, phase)
        }
        (false, None) => (shown(a).max(0), 0.0),
    };
    let source = (first_source, first_source + b - a);
    let kept = match m {
        Some(m) => uncovered(source.0, source.1, &p.left_out_frames(m, phase)),
        None => vec![source],
    };
    let (Some(first), Some(last)) = (kept.first(), kept.last()) else {
        return Err(Error::invalid(
            "every frame of the range is left out (media.leave_out)",
        ));
    };
    let reference = reference.id.clone();
    let place = |range: (i64, i64)| a + range.0 - source.0;
    let (start, end) = (place(*first), place(*last) + last.1 - last.0);
    let mut clips = Vec::with_capacity(kept.len());
    for &range in &kept {
        clips.push(make_clip(
            p,
            &Source {
                range,
                phase,
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
        ids.extend(put(p, ti, vec![clip], place(*range), src)?);
    }
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
