//! Cut-page edit operations (spec §6–§8) as functions on a [`Project`].
//!
//! Every operation works on resolved frame numbers; parsing TimeSpecs and
//! ClipRefs happens in the request layer. Operations may leave the project
//! half-modified when they fail: callers apply them to a copy (the Editor does).
//!
//! **Tracks** (spec §6): Track 1 (`V1`) is magnetic while Ripple On is set.
//! Inserts, deletes, moves and trims there ripple, and the clips on every
//! other track follow the Track 1 picture under their first frame
//! ([`follow`], decision D21). Other tracks never ripple themselves, except
//! where Track 1 has nothing under them (p. 721).

mod follow;
mod tracks;

use std::collections::BTreeMap;

pub use tracks::{TrackProps, add_track, remove_track, set_track};

use crate::error::{Error, Result};
use crate::model::{Clip, ClipAudio, Marker, Project, Track, TrackClip, TrackKind};
use follow::{Before, Edit};

/// The main (magnetic) track.
pub const MAIN: &str = "V1";

/// A media range to put on the timeline, and where.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Source {
    pub media: String,
    /// Source frames `[start, end)` (project frames from the media's first frame).
    pub range: (i64, i64),
    /// Leave the audio out (the Cut page's Video Only toggle, p. 694).
    pub video_only: bool,
    /// Leave the video out (Audio Only, p. 694).
    pub audio_only: bool,
    /// The target track (spec §7 step 3). Default: Track 1 for clips with
    /// video, A1 for audio-only clips. The next new track (`V3` above V2) is
    /// made. On an audio track only the audio is used.
    pub track: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Head,
    Tail,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum MoveMode {
    /// Insert at an edit point; later clips move right.
    Insert,
    /// Write over whatever is at the target; nothing else moves.
    Overwrite,
}

// ---------------------------------------------------------------- helpers

fn main_index(p: &Project) -> usize {
    p.track_index(MAIN).expect("every project has V1")
}

fn unlocked(p: &Project, ti: usize) -> Result<()> {
    if p.tracks[ti].locked {
        return Err(Error::invalid(format!(
            "track {} is locked",
            p.tracks[ti].id
        )));
    }
    Ok(())
}

/// Locates a clip on an unlocked track: (track index, clip index).
fn locate(p: &Project, id: &str) -> Result<(usize, usize)> {
    let (ti, ci) = p
        .find_clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    unlocked(p, ti)?;
    Ok((ti, ci))
}

fn clip_index(t: &Track, id: &str) -> usize {
    t.clips
        .iter()
        .position(|c| c.id == id)
        .expect("clip present")
}

/// Whether Track 1 has nothing at or after `t`: other tracks may ripple
/// there (p. 721).
fn main_empty_from(p: &Project, t: i64) -> bool {
    p.tracks[main_index(p)].clips.iter().all(|c| c.end() <= t)
}

/// Splits the clip strictly covering `t` on track `ti`; returns the new
/// right-hand clip's id. The left half keeps the original id.
fn split_at(p: &mut Project, ti: usize, t: i64) -> Option<String> {
    let ci = p.tracks[ti]
        .clips
        .iter()
        .position(|c| c.start < t && t < c.end())?;
    // Splitting inside a J/L overlap would leave one half without any audio
    // of its own; reset that J/L cut to a straight cut first (decision D14).
    if let Some((audio_start, audio_end)) = p.tracks[ti].clips[ci].audio_range() {
        let clips = &mut p.tracks[ti].clips;
        if t <= audio_start {
            clips[ci].audio.as_mut().expect("audio").head = 0;
            if let Some(prev) = ci.checked_sub(1).and_then(|i| clips[i].audio.as_mut()) {
                prev.tail = 0;
            }
        }
        if t >= audio_end {
            clips[ci].audio.as_mut().expect("audio").tail = 0;
            if let Some(next) = clips.get_mut(ci + 1).and_then(|c| c.audio.as_mut()) {
                next.head = 0;
            }
        }
    }
    let new_id = p.alloc_clip_id();
    let track = &mut p.tracks[ti];
    let left = &mut track.clips[ci];
    let offset = t - left.start;
    let mut right = left.clone();
    right.id = new_id.clone();
    right.start = t;
    right.duration = left.duration - offset;
    right.source_in = left.source_in + offset;
    left.duration = offset;
    // The split point becomes a plain edit: audio continues seamlessly from the
    // left half into the right half.
    if let (Some(la), Some(ra)) = (left.audio.as_mut(), right.audio.as_mut()) {
        la.tail = 0;
        ra.head = 0;
    }
    track.clips.insert(ci + 1, right);
    Some(new_id)
}

/// Removes everything on track `ti` within `[a, b)`, splitting clips that
/// straddle the edges.
fn clear_range(p: &mut Project, ti: usize, a: i64, b: i64) {
    if a >= b {
        return;
    }
    split_at(p, ti, a);
    split_at(p, ti, b);
    p.tracks[ti].clips.retain(|c| c.end() <= a || c.start >= b);
}

/// Moves every clip on track `ti` that starts at or after `from` by `d` frames.
fn shift_from(p: &mut Project, ti: usize, from: i64, d: i64) {
    for c in &mut p.tracks[ti].clips {
        if c.start >= from {
            c.start += d;
        }
    }
}

fn place(p: &mut Project, ti: usize, clip: Clip) {
    let track = &mut p.tracks[ti];
    let at = track.clips.partition_point(|c| c.start < clip.start);
    track.clips.insert(at, clip);
}

/// Keeps J/L offsets consistent (decision D14): an audio offset lives at an
/// edit between two adjacent clips with audio, where the outgoing clip's
/// `tail` equals the incoming clip's `head`. Offsets that no longer sit at such
/// an edit (a neighbor changed) are reset to a straight cut.
fn fix_audio(p: &mut Project, ti: usize) {
    let clips = &mut p.tracks[ti].clips;
    // A reset can leave a clip whose audio lay wholly past its picture (in
    // its tail, under its neighbor's J/L cut) with no audio of its own:
    // then the J/L cut at its other edge goes too, and that one's
    // neighbor's offset in the next round (decision D14). Each round zeroes
    // an offset, so the rounds end (a clip of no length is left to
    // validation).
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..clips.len() {
            let adjacent_next = i + 1 < clips.len()
                && clips[i + 1].start == clips[i].end()
                && clips[i + 1].audio.is_some();
            let adjacent_prev =
                i > 0 && clips[i - 1].end() == clips[i].start && clips[i - 1].audio.is_some();
            let (prev_tail, next_head) = (
                if adjacent_prev {
                    clips[i - 1].audio.as_ref().map(|a| a.tail)
                } else {
                    None
                },
                if adjacent_next {
                    clips[i + 1].audio.as_ref().map(|a| a.head)
                } else {
                    None
                },
            );
            let duration = clips[i].duration;
            if let Some(a) = clips[i].audio.as_mut() {
                if prev_tail != Some(a.head) && a.head != 0 {
                    a.head = 0;
                    changed = true;
                }
                if next_head != Some(a.tail) && a.tail != 0 {
                    a.tail = 0;
                    changed = true;
                }
                if a.head >= duration + a.tail && (a.head, a.tail) != (0, 0) {
                    (a.head, a.tail) = (0, 0);
                    changed = true;
                }
            }
        }
    }
}

/// Builds a new clip (not yet placed) from a source range.
fn make_clip(p: &mut Project, src: &Source) -> Result<Clip> {
    let media = p
        .media(&src.media)
        .ok_or_else(|| Error::not_found(format!("no media {}", src.media)))?;
    if src.video_only && src.audio_only {
        return Err(Error::usage(
            "an edit can't be both Video Only and Audio Only",
        ));
    }
    let video = media.info.video.is_some() && !src.audio_only;
    let audio = !media.info.audio.is_empty() && !src.video_only;
    if !video && !audio {
        return Err(Error::invalid(format!(
            "{} has no {}",
            media.id,
            if src.audio_only { "audio" } else { "video" }
        )));
    }
    let frames = p.media_limit(media);
    let (a, b) = src.range;
    if a < 0 || b > frames || a >= b {
        return Err(Error::invalid(format!(
            "source range {a}..{b} is outside {} (0..{frames} frames)",
            media.id
        )));
    }
    let id = p.alloc_clip_id();
    Ok(Clip {
        id,
        media: src.media.clone(),
        start: 0,
        duration: b - a,
        source_in: a,
        video,
        audio: audio.then_some(ClipAudio {
            stream: 0,
            head: 0,
            tail: 0,
            gain_db: 0.0,
            muted: false,
        }),
        enabled: true,
        name: None,
    })
}

/// The track an edit puts `clip` on (spec §7 step 3, decision D3): the
/// source's track, else Track 1 for clips with video and A1 for audio-only
/// clips. On an audio track only the audio is used.
fn home(p: &mut Project, clip: &mut Clip, src: &Source) -> Result<usize> {
    let ti = match &src.track {
        Some(id) => tracks::resolve(p, id)?,
        None if clip.video => main_index(p),
        None => tracks::nth(p, TrackKind::Audio, 0),
    };
    if p.tracks[ti].kind == TrackKind::Audio && clip.video {
        if clip.audio.is_none() {
            return Err(Error::invalid(format!(
                "{} has no audio for audio track {}",
                clip.media, p.tracks[ti].id
            )));
        }
        clip.video = false;
    }
    unlocked(p, ti)?;
    Ok(ti)
}

/// Index of track `id` (`V3`, `A2`), adding tracks up to it.
pub fn ensure_track(p: &mut Project, id: &str) -> Result<usize> {
    let kind = match id.chars().next() {
        Some('V') => TrackKind::Video,
        Some('A') => TrackKind::Audio,
        _ => return Err(Error::usage(format!("{id:?} is not a track (V1, A2…)"))),
    };
    let n: usize = id[1..]
        .parse()
        .ok()
        .filter(|&n| (1..=99).contains(&n))
        .ok_or_else(|| Error::usage(format!("{id:?} is not a track (V1, A2…)")))?;
    Ok(tracks::nth(p, kind, n - 1))
}

fn is_main(p: &Project, ti: usize) -> bool {
    p.tracks[ti].id == MAIN
}

// ---------------------------------------------------------------- edit functions (spec §7)

/// Append (Shift+F12, p. 697): after the last clip of the target track.
pub fn append(p: &mut Project, src: &Source) -> Result<String> {
    let mut clip = make_clip(p, src)?;
    let ti = home(p, &mut clip, src)?;
    clip.start = p.tracks[ti].end();
    let id = clip.id.clone();
    place(p, ti, clip);
    fix_audio(p, ti);
    Ok(id)
}

/// The edit point on V1 nearest `playhead` (the Smart Indicator, p. 696);
/// ties go to the earlier edit.
pub fn nearest_edit(p: &Project, playhead: i64) -> i64 {
    let points = p
        .track(MAIN)
        .map(Track::edit_points)
        .unwrap_or_else(|| vec![0]);
    points
        .into_iter()
        .min_by_key(|&e| ((e - playhead).abs(), e))
        .unwrap_or(0)
}

/// Whether the clips at Track 1's edit `e` have media past it: not when the
/// clip ending there shows its media's last frame, or the clip starting
/// there its first. The Smart Indicator then turns red (p. 696).
pub fn edit_has_handles(p: &Project, e: i64) -> bool {
    let Some(t) = p.track(MAIN) else {
        return true;
    };
    !t.clips.iter().any(|c| {
        let at_end = c.end() == e
            && p.media(&c.media)
                .is_some_and(|m| c.source_in + c.duration >= p.media_limit(m));
        let at_start = c.start == e && c.source_in <= 0;
        at_end || at_start
    })
}

/// Smart Insert: insert at the edit nearest the playhead (p. 697).
pub fn smart_insert(p: &mut Project, src: &Source, playhead: i64) -> Result<String> {
    let at = nearest_edit(p, playhead);
    insert_at(p, src, at)
}

/// Insert at an exact frame (API-only, deviation X5). On Track 1 later clips
/// move right, splitting a clip there if needed, and other tracks follow;
/// with Ripple On a position past the end means the end. On other tracks
/// nothing is pushed (p. 697): the clip writes over what is there.
pub fn insert_at(p: &mut Project, src: &Source, at: i64) -> Result<String> {
    if at < 0 {
        return Err(Error::invalid("insert position before 0"));
    }
    let mut clip = make_clip(p, src)?;
    let ti = home(p, &mut clip, src)?;
    let id = clip.id.clone();
    if !is_main(p, ti) {
        clear_range(p, ti, at, at + clip.duration);
        clip.start = at;
        place(p, ti, clip);
        fix_audio(p, ti);
        return Ok(id);
    }
    let before = Before::of(p);
    let at = if p.settings.ripple_on {
        at.min(p.tracks[ti].end())
    } else {
        at
    };
    split_at(p, ti, at);
    shift_from(p, ti, at, clip.duration);
    clip.start = at;
    place(p, ti, clip);
    fix_audio(p, ti);
    follow::follow(
        p,
        &before,
        &Edit {
            created: std::slice::from_ref(&id),
            ..Edit::default()
        },
    );
    Ok(id)
}

/// Where an Overwrite or Place On Top goes, and the source range it takes:
/// the timeline In; else backtimed to a lone timeline Out; else `at`. With
/// both marks set it is a three-point edit: the marked range sets the length
/// (decision D7).
fn marked_start(p: &Project, src: &Source, at: i64) -> (i64, Source) {
    let mut src = src.clone();
    let len = src.range.1 - src.range.0;
    let start = match (p.marks.mark_in, p.marks.mark_out) {
        (Some(i), Some(o)) => {
            src.range.1 = src.range.0 + (o - i);
            i
        }
        (Some(i), None) => i,
        (None, Some(o)) => o - len,
        (None, None) => at,
    };
    (start, src)
}

/// Overwrite (F10, p. 701): writes over the target track without moving
/// other clips.
pub fn overwrite(p: &mut Project, src: &Source, at: i64) -> Result<String> {
    let (start, src) = marked_start(p, src, at);
    if start < 0 {
        return Err(Error::invalid("overwrite would start before 0"));
    }
    let mut clip = make_clip(p, &src)?;
    let ti = home(p, &mut clip, &src)?;
    // Decision D10: with Ripple On, Track 1 stays gapless.
    let start = if is_main(p, ti) && p.settings.ripple_on {
        start.min(p.tracks[ti].end())
    } else {
        start
    };
    clear_range(p, ti, start, start + clip.duration);
    clip.start = start;
    let id = clip.id.clone();
    place(p, ti, clip);
    fix_audio(p, ti);
    Ok(id)
}

/// Ripple Overwrite (Shift+F10, p. 698): replaces the timeline In–Out range,
/// or else the whole clip under `at`, on the target track. On Track 1
/// everything after moves by the length difference (the source length always
/// wins, decision D8), and what was over the replaced clips stays. On other
/// tracks nothing moves: a longer clip writes over what follows.
pub fn ripple_overwrite(p: &mut Project, src: &Source, at: i64) -> Result<String> {
    let mut clip = make_clip(p, src)?;
    let ti = home(p, &mut clip, src)?;
    let (a, b) = match (p.marks.mark_in, p.marks.mark_out) {
        (Some(i), Some(o)) => (i, o),
        _ => {
            let t = &p.tracks[ti];
            let ci = t.clip_at(at).ok_or_else(|| {
                Error::invalid(format!("no clip on {} under the playhead to replace", t.id))
            })?;
            (t.clips[ci].start, t.clips[ci].end())
        }
    };
    let id = clip.id.clone();
    let len = clip.duration;
    if !is_main(p, ti) {
        clear_range(p, ti, a, b.max(a + len));
        clip.start = a;
        place(p, ti, clip);
        fix_audio(p, ti);
        return Ok(id);
    }
    // A marked range past the end of a magnetic V1 replaces nothing: the clip
    // goes at the end so V1 stays gapless (like decision D10).
    let end = p.tracks[ti].end();
    let (a, b) = if p.settings.ripple_on && a > end {
        (end, end)
    } else {
        (a, b)
    };
    let before = Before::of(p);
    clear_range(p, ti, a, b);
    shift_from(p, ti, b, len - (b - a));
    clip.start = a;
    place(p, ti, clip);
    fix_audio(p, ti);
    follow::follow(
        p,
        &before,
        &Edit {
            created: std::slice::from_ref(&id),
            kept: &[(a, a + len.min(b - a))],
            ..Edit::default()
        },
    );
    Ok(id)
}

/// Place On Top (F12, pp. 699–700): the clip goes on the first track above
/// every clip in its span (made if needed), ignoring the target track and
/// never rippling. It lines up with the timeline In, a lone Out
/// (backtimed), the edit nearest the playhead when within 5 frames, or the
/// playhead. Audio-only clips go on the first audio track with room.
/// Returns the clip and the track it went on.
pub fn place_on_top(p: &mut Project, src: &Source, playhead: i64) -> Result<(String, String)> {
    let marked = p.marks.mark_in.is_some() || p.marks.mark_out.is_some();
    let at = if marked {
        playhead
    } else {
        let edit = nearest_edit(p, playhead);
        if (edit - playhead).abs() <= 5 {
            edit
        } else {
            playhead
        }
    };
    let (start, src) = marked_start(p, src, at);
    if start < 0 {
        return Err(Error::invalid("the clip would start before 0"));
    }
    let mut clip = make_clip(p, &src)?;
    let end = start + clip.duration;
    let busy = |t: &Track| t.clips.iter().any(|c| c.start < end && start < c.end());
    let ti = if clip.video {
        let video = p.tracks_of(TrackKind::Video);
        let top = video.iter().rposition(|&i| busy(&p.tracks[i]));
        tracks::nth(p, TrackKind::Video, top.map_or(0, |k| k + 1))
    } else {
        let audio = p.tracks_of(TrackKind::Audio);
        let free = audio.iter().position(|&i| !busy(&p.tracks[i]));
        tracks::nth(p, TrackKind::Audio, free.unwrap_or(audio.len()))
    };
    unlocked(p, ti)?;
    // An empty Track 1 is where it lands when nothing is there; Ripple On
    // keeps it gapless (decision D10).
    let start = if is_main(p, ti) && p.settings.ripple_on {
        start.min(p.tracks[ti].end())
    } else {
        start
    };
    clear_range(p, ti, start, start + clip.duration);
    clip.start = start;
    let id = clip.id.clone();
    place(p, ti, clip);
    fix_audio(p, ti);
    Ok((id, p.tracks[ti].id.clone()))
}

// ---------------------------------------------------------------- timeline editing (spec §8)

/// Split (Ctrl+\, p. 723) at each of `at`. Decision D2: where some of
/// `selected` lie under a split point, only those are split there; otherwise
/// every clip under it on every unlocked track. Returns the new right-hand
/// ids.
pub fn split(p: &mut Project, at: &[i64], selected: &[String]) -> Result<Vec<String>> {
    for id in selected {
        locate(p, id)?;
    }
    let mut created = Vec::new();
    let mut points: Vec<i64> = at.to_vec();
    points.sort_unstable();
    points.dedup();
    let mut touched = Vec::new();
    for t in points {
        let covering: Vec<(usize, String)> = (0..p.tracks.len())
            .filter(|&ti| !p.tracks[ti].locked)
            .filter_map(|ti| {
                let track = &p.tracks[ti];
                track
                    .clips
                    .iter()
                    .find(|c| c.start < t && t < c.end())
                    .map(|c| (ti, c.id.clone()))
            })
            .collect();
        let chosen: Vec<&(usize, String)> = covering
            .iter()
            .filter(|(_, id)| selected.contains(id))
            .collect();
        let targets: Vec<usize> = if chosen.is_empty() {
            covering.iter().map(|(ti, _)| *ti).collect()
        } else {
            chosen.iter().map(|(ti, _)| *ti).collect()
        };
        for ti in targets {
            if let Some(id) = split_at(p, ti, t) {
                created.push(id);
                touched.push(ti);
            }
        }
    }
    if created.is_empty() {
        return Err(Error::invalid(
            "nothing to split: no clip strictly covers the split point",
        ));
    }
    touched.sort_unstable();
    touched.dedup();
    for ti in touched {
        fix_audio(p, ti);
    }
    Ok(created)
}

/// Delete clips (pp. 721, 724–725). On Track 1 with ripple (Ripple On unless
/// `lift`) later clips move left to close each gap and other tracks follow;
/// a lift leaves the gaps. Other tracks close their gaps only where Track 1
/// has nothing under them (p. 721).
pub fn delete(p: &mut Project, ids: &[String], lift: bool) -> Result<()> {
    let ripple = !lift && p.settings.ripple_on;
    delete_clips(p, ids, ripple)
}

fn delete_clips(p: &mut Project, ids: &[String], ripple: bool) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::usage("no clips to delete"));
    }
    let main = main_index(p);
    let mut main_spans = Vec::new();
    let mut others: BTreeMap<usize, Vec<(i64, i64, String)>> = BTreeMap::new();
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let c = &p.tracks[ti].clips[ci];
        let span = (c.start, c.end(), c.id.clone());
        if ti == main {
            main_spans.push(span);
        } else {
            others.entry(ti).or_default().push(span);
        }
    }
    // Other tracks first, so the clips deleted there don't follow Track 1.
    for (ti, mut spans) in others {
        spans.sort();
        spans.dedup();
        for (start, end, id) in spans.into_iter().rev() {
            p.tracks[ti].clips.retain(|c| c.id != id);
            if ripple && main_empty_from(p, start) {
                shift_from(p, ti, end, -(end - start));
            }
        }
        fix_audio(p, ti);
    }
    if main_spans.is_empty() {
        return Ok(());
    }
    let before = Before::of(p);
    main_spans.sort();
    main_spans.dedup();
    for (start, end, id) in main_spans.into_iter().rev() {
        p.tracks[main].clips.retain(|c| c.id != id);
        if ripple {
            shift_from(p, main, end, -(end - start));
        }
    }
    fix_audio(p, main);
    if ripple {
        follow::follow(p, &before, &Edit::default());
    }
    Ok(())
}

/// Splits clip `id` at each of `at` that falls strictly inside it (Detect
/// Scene Cuts, p. 690); returns the new clips, left to right.
pub fn split_clip(p: &mut Project, id: &str, at: &[i64]) -> Result<Vec<String>> {
    let (ti, ci) = locate(p, id)?;
    let (start, end) = {
        let c = &p.tracks[ti].clips[ci];
        (c.start, c.end())
    };
    let mut points: Vec<i64> = at
        .iter()
        .copied()
        .filter(|&t| start < t && t < end)
        .collect();
    points.sort_unstable();
    points.dedup();
    let created: Vec<String> = points
        .into_iter()
        .filter_map(|t| split_at(p, ti, t))
        .collect();
    fix_audio(p, ti);
    Ok(created)
}

/// Cuts parts out of clips and closes the gaps (Ripple Delete Silence,
/// p. 773): each clip is split at its parts' edges, and the pieces inside
/// go as Delete with ripple removes clips, whatever Ripple On says (Track 1
/// closes up and other tracks follow; elsewhere p. 721 applies). Parts are
/// frame ranges inside their clip. Returns the pieces removed.
pub fn remove_parts(p: &mut Project, parts: &[(String, Vec<(i64, i64)>)]) -> Result<Vec<String>> {
    let mut doomed = Vec::new();
    for (id, spans) in parts {
        let edges: Vec<i64> = spans.iter().flat_map(|&(a, b)| [a, b]).collect();
        let mut pieces = vec![id.clone()];
        pieces.extend(split_clip(p, id, &edges)?);
        for piece in pieces {
            let c = p.clip(&piece).expect("just split");
            if spans.iter().any(|&(a, b)| a <= c.start && c.end() <= b) {
                doomed.push(piece);
            }
        }
    }
    if !doomed.is_empty() {
        delete_clips(p, &doomed, true)?;
    }
    Ok(doomed)
}

/// Slide (Dynamic Trim's Slide mode, p. 731): clip `id` keeps its media and
/// moves `by` frames; the clip before it gets longer and the one after it
/// shorter, or the reverse. Both must touch it, so the program length stays.
pub fn slide(p: &mut Project, id: &str, by: i64) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    if by == 0 {
        return Ok(());
    }
    let t = &p.tracks[ti];
    let c = t.clips[ci].clone();
    let left = ci.checked_sub(1).filter(|&i| t.clips[i].end() == c.start);
    let right = Some(ci + 1).filter(|&i| i < t.clips.len() && t.clips[i].start == c.end());
    let (Some(l), Some(r)) = (left, right) else {
        return Err(Error::invalid(format!(
            "{id} slides only between two clips that touch it"
        )));
    };
    let (mut before_c, mut moved, mut after_c) = (t.clips[l].clone(), c, t.clips[r].clone());
    before_c.duration += by;
    moved.start += by;
    after_c.start += by;
    after_c.source_in += by;
    after_c.duration -= by;
    check_source(p, &before_c)?;
    check_source(p, &after_c)?;
    let before = (ti == main_index(p)).then(|| Before::of(p));
    let track = &mut p.tracks[ti];
    track.clips[l] = before_c;
    track.clips[ci] = moved;
    track.clips[r] = after_c;
    fix_audio(p, ti);
    if let Some(before) = before {
        follow::follow(p, &before, &Edit::default());
    }
    Ok(())
}

/// Delete the range `[a, b)` on every unlocked track (Delete with timeline
/// In/Out set, p. 725); with ripple (unless `lift`) everything after moves
/// left on those tracks.
pub fn delete_range(p: &mut Project, a: i64, b: i64, lift: bool) -> Result<()> {
    if a >= b {
        return Err(Error::usage(format!("empty range {a}..{b}")));
    }
    unlocked(p, main_index(p))?;
    let ripple = !lift && p.settings.ripple_on;
    for ti in 0..p.tracks.len() {
        if p.tracks[ti].locked {
            continue;
        }
        clear_range(p, ti, a, b);
        if ripple {
            shift_from(p, ti, b, -(b - a));
        }
        fix_audio(p, ti);
    }
    Ok(())
}

/// Closes the gap on V1 containing frame `at`; other tracks follow.
pub fn close_gap(p: &mut Project, at: i64) -> Result<()> {
    let ti = main_index(p);
    unlocked(p, ti)?;
    let t = &p.tracks[ti];
    let mut gap_start = 0;
    let mut gap = None;
    for c in &t.clips {
        if gap_start <= at && at < c.start {
            gap = Some((gap_start, c.start));
            break;
        }
        gap_start = c.end();
    }
    let (a, b) = gap.ok_or_else(|| Error::invalid(format!("no gap at frame {at}")))?;
    let before = Before::of(p);
    shift_from(p, ti, b, -(b - a));
    fix_audio(p, ti);
    follow::follow(p, &before, &Edit::default());
    Ok(())
}

/// Takes clips out of V1, closing the gaps; returns them in timeline order and
/// maps `to` into the resulting timeline.
fn extract(p: &mut Project, ti: usize, clips: &[Clip], to: i64) -> Result<i64> {
    if let Some(c) = clips.iter().find(|c| c.start < to && to < c.end()) {
        return Err(Error::invalid(format!("cannot move {} into itself", c.id)));
    }
    let mut mapped = to;
    for c in clips.iter().rev() {
        p.tracks[ti].clips.retain(|x| x.id != c.id);
        shift_from(p, ti, c.end(), -c.duration);
        if c.end() <= to {
            mapped -= c.duration;
        }
    }
    Ok(mapped)
}

/// Move clips (pp. 720–723). `to` is where the first of them starts, in the
/// timeline as it is before the move; `track` is the track it goes to (the
/// others keep theirs: with `track`, the clips must all be on one track).
///
/// On Track 1:
/// - `Insert` (dropping on an edit point): the clips are taken out, the gaps
///   they leave close, and they go in at `to`, pushing later clips right.
/// - `Overwrite` (dropping after hovering, or over the edge of a neighbor):
///   the clips are lifted, written over whatever is at `to`, and then, with
///   Ripple On, what remains of the spans they left closes (p. 722: moving a
///   clip a few frames onto its neighbor shortens the neighbor).
///
/// Other tracks don't ripple: clips write over what is at their new place.
/// Clips moved off Track 1 leave a gap that closes with Ripple On, and a
/// Track 1 clip dropped on another track can only overwrite (p. 721). Clips
/// on other tracks follow Track 1 (decision D21).
pub fn move_clips(
    p: &mut Project,
    ids: &[String],
    to: i64,
    mode: MoveMode,
    track: Option<&str>,
) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::usage("no clips to move"));
    }
    if to < 0 {
        return Err(Error::invalid("move target before 0"));
    }
    // A new destination track comes first: making it shifts track indexes.
    let dest = track.map(|id| tracks::resolve(p, id)).transpose()?;
    let main = main_index(p);
    let mut clips: Vec<(usize, Clip)> = Vec::new();
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        clips.push((ti, p.tracks[ti].clips[ci].clone()));
    }
    clips.sort_by_key(|(_, c)| c.start);
    clips.dedup_by(|a, b| a.1.id == b.1.id);
    let from = clips[0].0;
    let dest = dest.unwrap_or(from);
    unlocked(p, dest)?;
    if dest != from && clips.iter().any(|(ti, _)| *ti != from) {
        return Err(Error::usage(
            "clips from several tracks keep their tracks; move them to another track one track at a time",
        ));
    }
    if p.tracks[dest].kind == TrackKind::Audio && clips.iter().any(|(_, c)| c.video) {
        return Err(Error::invalid(format!(
            "{} is an audio track: only audio clips go there",
            p.tracks[dest].id
        )));
    }
    let on_main: Vec<Clip> = clips
        .iter()
        .filter(|(ti, _)| *ti == main)
        .map(|(_, c)| c.clone())
        .collect();
    if dest == main && from == main {
        // Clips on other tracks that were picked too follow Track 1.
        let before = Before::of(p);
        match mode {
            MoveMode::Insert => move_insert(p, main, &on_main, to)?,
            MoveMode::Overwrite => move_overwrite(p, main, &on_main, to),
        }
        follow::follow(p, &before, &Edit::default());
        return Ok(());
    }
    if !on_main.is_empty() && dest == from {
        // A mix of tracks moved sideways: Track 1's clips move by Track 1's
        // rules and the others follow them.
        let before = Before::of(p);
        let to = to + (on_main[0].start - clips[0].1.start);
        match mode {
            MoveMode::Insert => move_insert(p, main, &on_main, to)?,
            MoveMode::Overwrite => move_overwrite(p, main, &on_main, to),
        }
        follow::follow(p, &before, &Edit::default());
        return Ok(());
    }
    let before = Before::of(p);
    let delta = to - clips[0].1.start;
    for (ti, c) in &clips {
        p.tracks[*ti].clips.retain(|x| x.id != c.id);
    }
    let moved: Vec<Clip> = clips
        .iter()
        .map(|(_, c)| Clip {
            start: c.start + delta,
            ..c.clone()
        })
        .collect();
    if dest == main {
        // Onto Track 1 from another track.
        match mode {
            MoveMode::Insert => {
                let total: i64 = moved.iter().map(|c| c.duration).sum();
                let at = if p.settings.ripple_on {
                    to.min(p.tracks[main].end())
                } else {
                    to
                };
                split_at(p, main, at);
                shift_from(p, main, at, total);
                let mut t = at;
                for mut c in moved {
                    c.start = t;
                    t += c.duration;
                    place(p, main, c);
                }
            }
            MoveMode::Overwrite => {
                let first = if p.settings.ripple_on {
                    to.min(p.tracks[main].end())
                } else {
                    to
                };
                for mut c in moved {
                    c.start += first - to;
                    clear_range(p, main, c.start, c.end());
                    place(p, main, c);
                }
            }
        }
        fix_all(p);
        follow::follow(p, &before, &Edit::default());
        return Ok(());
    }
    // Onto a track other than Track 1 (or along one): overwrite there.
    let placed: Vec<String> = moved.iter().map(|c| c.id.clone()).collect();
    for ((own, _), c) in clips.iter().zip(moved) {
        let ti = if dest != from { dest } else { *own };
        clear_range(p, ti, c.start, c.end());
        place(p, ti, c);
    }
    if from == main && p.settings.ripple_on {
        // The spans left on Track 1 close (p. 649), and what was over them
        // follows; the moved clips themselves sit where they were dropped.
        let mut spans: Vec<(i64, i64)> = clips.iter().map(|(_, c)| (c.start, c.end())).collect();
        spans.sort_unstable();
        for (a, b) in spans.into_iter().rev() {
            shift_from(p, main, b, -(b - a));
        }
        fix_all(p);
        follow::follow(
            p,
            &before,
            &Edit {
                placed: &placed,
                ..Edit::default()
            },
        );
        return Ok(());
    }
    fix_all(p);
    Ok(())
}

/// Keeps J/L offsets consistent on every track (decision D14).
fn fix_all(p: &mut Project) {
    for ti in 0..p.tracks.len() {
        fix_audio(p, ti);
    }
}

/// Track 1 insert-move: take the clips out (closing their gaps) and insert
/// them at `to`.
fn move_insert(p: &mut Project, ti: usize, clips: &[Clip], to: i64) -> Result<()> {
    let to = extract(p, ti, clips, to)?;
    let total: i64 = clips.iter().map(|c| c.duration).sum();
    let to = to.min(p.tracks[ti].end());
    split_at(p, ti, to);
    shift_from(p, ti, to, total);
    let mut at = to;
    for c in clips {
        let mut c = c.clone();
        c.start = at;
        at += c.duration;
        place(p, ti, c);
    }
    fix_audio(p, ti);
    Ok(())
}

/// Track 1 overwrite-move.
fn move_overwrite(p: &mut Project, ti: usize, clips: &[Clip], to: i64) {
    // Lift: the clips' spans become gaps for now.
    p.tracks[ti]
        .clips
        .retain(|c| !clips.iter().any(|m| m.id == c.id));
    let total: i64 = clips.iter().map(|c| c.duration).sum();
    let magnetic = p.settings.ripple_on;
    // Decision D10: with Ripple On nothing starts after the end.
    let to = if magnetic {
        to.min(p.tracks[ti].end())
    } else {
        to
    };
    clear_range(p, ti, to, to + total);
    let mut at = to;
    for c in clips {
        let mut c = c.clone();
        c.start = at;
        at += c.duration;
        place(p, ti, c);
    }
    if magnetic {
        // Close the vacated spans not covered by the clips' new place, right
        // to left so earlier spans keep their positions.
        let mut gaps: Vec<(i64, i64)> = Vec::new();
        for c in clips {
            for (a, b) in [
                (c.start, c.end().min(to)),
                (c.start.max(to + total), c.end()),
            ] {
                if a < b {
                    gaps.push((a, b));
                }
            }
        }
        gaps.sort_unstable();
        gaps.dedup();
        for (a, b) in gaps.into_iter().rev() {
            // Only where nothing else sits (a span may straddle `to`).
            let occupied = p.tracks[ti]
                .clips
                .iter()
                .any(|c| c.start < b && a < c.end());
            if !occupied {
                shift_from(p, ti, b, -(b - a));
            }
        }
    }
    fix_audio(p, ti);
}

/// Move a clip onto another on Track 1 and replace it, rippling the
/// difference (dropping on a clip right away, p. 721). What was over the
/// replaced clip stays.
pub fn move_replace(p: &mut Project, id: &str, onto: &str) -> Result<()> {
    let main = main_index(p);
    if id == onto {
        return Err(Error::invalid("a clip cannot replace itself"));
    }
    let (ti, ci) = locate(p, id)?;
    let (ui, _) = locate(p, onto)?;
    if ti != main || ui != main {
        return Err(Error::invalid(
            "Replace works on Track 1; on other tracks a dropped clip overwrites (p. 721)",
        ));
    }
    let clip = p.tracks[main].clips[ci].clone();
    let before = Before::of(p);
    extract(p, main, std::slice::from_ref(&clip), 0)?;
    let target = &p.tracks[main].clips[clip_index(&p.tracks[main], onto)];
    let (a, b) = (target.start, target.end());
    // The replaced span in the timeline as it was.
    let (ka, kb) = {
        let t = before.main_clip(onto).expect("on Track 1");
        (t.start, t.start + clip.duration.min(t.duration))
    };
    clear_range(p, main, a, b);
    shift_from(p, main, b, clip.duration - (b - a));
    place(p, main, Clip { start: a, ..clip });
    fix_audio(p, main);
    follow::follow(
        p,
        &before,
        &Edit {
            kept: &[(ka, kb)],
            ..Edit::default()
        },
    );
    Ok(())
}

/// Checks a clip's video and audio stay within its media.
fn check_source(p: &Project, c: &Clip) -> Result<()> {
    let m = p
        .media(&c.media)
        .ok_or_else(|| Error::not_found(format!("no media {}", c.media)))?;
    let frames = p.media_limit(m);
    if c.duration < 1 {
        return Err(Error::invalid(format!(
            "{} would be shorter than one frame",
            c.id
        )));
    }
    if c.source_in < 0 || c.source_in + c.duration > frames {
        return Err(Error::invalid(format!(
            "{} would need source frames {}..{}, but {} has 0..{frames}",
            c.id,
            c.source_in,
            c.source_in + c.duration,
            m.id
        )));
    }
    if let Some(a) = &c.audio {
        let (s, e) = (c.source_in + a.head, c.source_in + c.duration + a.tail);
        if s < 0 || e > frames || e - s < 1 {
            return Err(Error::invalid(format!(
                "{}'s audio would need source frames {s}..{e}, but {} has 0..{frames}",
                c.id, m.id
            )));
        }
    }
    Ok(())
}

/// Resize a clip edge by `delta` frames (positive = later), p. 726.
///
/// On Track 1 with ripple (Ripple On unless overridden), the clip keeps its
/// start, later clips move, and other tracks follow (a clip whose first
/// frame was over the trimmed-off tail goes, p. 727). Other tracks ripple
/// only where Track 1 has nothing under them (p. 721). Without ripple the
/// edge moves in place, writing over a neighbor when extending into it.
pub fn trim(p: &mut Project, id: &str, edge: Edge, delta: i64, ripple: Option<bool>) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    let main = ti == main_index(p);
    let c = p.tracks[ti].clips[ci].clone();
    let ripple = ripple.unwrap_or(p.settings.ripple_on) && (main || main_empty_from(p, c.start));
    let old_end = c.end();
    let mut new = c.clone();
    match edge {
        Edge::Tail => new.duration += delta,
        Edge::Head => {
            new.source_in += delta;
            new.duration -= delta;
            if !ripple {
                new.start += delta;
            }
        }
    }
    check_source(p, &new)?;
    if new.start < 0 {
        return Err(Error::invalid(format!("{id} would start before 0")));
    }
    let before = (main && ripple).then(|| Before::of(p));
    if ripple {
        // Later clips follow the clip's new end.
        shift_from(p, ti, old_end, new.end() - old_end);
    } else {
        // Extending writes over the neighbor.
        match edge {
            Edge::Tail if delta > 0 => clear_range(p, ti, old_end, new.end()),
            Edge::Head if delta < 0 => clear_range(p, ti, new.start, c.start),
            _ => {}
        }
    }
    let ci = clip_index(&p.tracks[ti], id);
    p.tracks[ti].clips[ci] = new;
    p.tracks[ti].sort();
    fix_audio(p, ti);
    if let Some(before) = before {
        follow::follow(
            p,
            &before,
            &Edit {
                trimmed: Some(id),
                ..Edit::default()
            },
        );
    }
    Ok(())
}

/// The two clips meeting at a clip's `edge` (outgoing, incoming), which must be
/// adjacent.
fn edit_pair(p: &Project, ti: usize, id: &str, edge: Edge) -> Result<(usize, usize)> {
    let t = &p.tracks[ti];
    let ci = clip_index(t, id);
    let (a, b) = match edge {
        Edge::Tail => (ci, ci + 1),
        Edge::Head => (ci.wrapping_sub(1), ci),
    };
    if a >= t.clips.len() || b >= t.clips.len() || t.clips[a].end() != t.clips[b].start {
        return Err(Error::invalid(format!(
            "{id} has no adjacent clip at its {edge:?} edge"
        )));
    }
    Ok((a, b))
}

/// Roll the edit at a clip's edge by `delta` (p. 727): the outgoing clip's end
/// and the incoming clip's start move together; the program length stays.
pub fn roll(p: &mut Project, id: &str, edge: Edge, delta: i64) -> Result<()> {
    let (ti, _) = locate(p, id)?;
    let (a, b) = edit_pair(p, ti, id, edge)?;
    let mut out = p.tracks[ti].clips[a].clone();
    let mut inc = p.tracks[ti].clips[b].clone();
    out.duration += delta;
    inc.start += delta;
    inc.source_in += delta;
    inc.duration -= delta;
    check_source(p, &out)?;
    check_source(p, &inc)?;
    p.tracks[ti].clips[a] = out;
    p.tracks[ti].clips[b] = inc;
    Ok(())
}

/// Slip a clip's content by `delta` source frames (p. 727); position and
/// length stay.
pub fn slip(p: &mut Project, id: &str, delta: i64) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    let mut c = p.tracks[ti].clips[ci].clone();
    c.source_in += delta;
    check_source(p, &c)?;
    p.tracks[ti].clips[ci] = c;
    Ok(())
}

/// Audio-only roll at a clip's edge: a J/L cut (p. 728). Only the audio edit
/// moves; positive `delta` lets the outgoing audio run longer.
pub fn audio_roll(p: &mut Project, id: &str, edge: Edge, delta: i64) -> Result<()> {
    let (ti, _) = locate(p, id)?;
    let (a, b) = edit_pair(p, ti, id, edge)?;
    let mut out = p.tracks[ti].clips[a].clone();
    let mut inc = p.tracks[ti].clips[b].clone();
    let (Some(oa), Some(ia)) = (out.audio.as_mut(), inc.audio.as_mut()) else {
        return Err(Error::invalid(
            "both clips at the edit need audio for a J/L cut",
        ));
    };
    oa.tail += delta;
    ia.head += delta;
    check_source(p, &out)?;
    check_source(p, &inc)?;
    p.tracks[ti].clips[a] = out;
    p.tracks[ti].clips[b] = inc;
    Ok(())
}

/// Trim Start/End to Playhead (p. 729): moves the clips' head or tail to
/// `at`. Decision D4: the given clips, or the V1 clip under `at`.
pub fn trim_to(
    p: &mut Project,
    edge: Edge,
    at: i64,
    ids: &[String],
    ripple: Option<bool>,
) -> Result<()> {
    let targets: Vec<String> = if ids.is_empty() {
        let ti = main_index(p);
        let t = &p.tracks[ti];
        // At a clip's end the playhead sits on the next clip; for the tail use
        // the clip ending here.
        let ci = t
            .clip_at(at)
            .or_else(|| {
                (edge == Edge::Tail)
                    .then(|| t.clips.iter().position(|c| c.end() == at))
                    .flatten()
            })
            .ok_or_else(|| Error::invalid("no clip under the playhead"))?;
        vec![t.clips[ci].id.clone()]
    } else {
        ids.to_vec()
    };
    for id in targets {
        let c = p
            .clip(&id)
            .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
        let delta = match edge {
            Edge::Head => at - c.start,
            Edge::Tail => at - c.end(),
        };
        if delta != 0 {
            trim(p, &id, edge, delta, ripple)?;
        }
    }
    Ok(())
}

/// Change Clip Duration (Ctrl+D, p. 693) for each clip.
pub fn change_duration(
    p: &mut Project,
    ids: &[String],
    frames: i64,
    ripple: Option<bool>,
) -> Result<()> {
    if frames < 1 {
        return Err(Error::invalid("duration must be at least one frame"));
    }
    // Later clips first so rippling doesn't move the ones still to change.
    let mut targets = Vec::new();
    for id in ids {
        let c = p
            .clip(id)
            .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
        targets.push((c.start, id.clone(), c.duration));
    }
    targets.sort();
    for (_, id, duration) in targets.into_iter().rev() {
        if frames != duration {
            trim(p, &id, Edge::Tail, frames - duration, ripple)?;
        }
    }
    Ok(())
}

/// Clip properties settable through `clip.set`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipProps {
    pub enabled: Option<bool>,
    pub muted: Option<bool>,
    pub gain_db: Option<f64>,
    /// `Some(None)` clears the name.
    pub name: Option<Option<String>>,
}

/// Sets properties on clips (D = enable, Mute, clip volume, name).
pub fn set_clips(p: &mut Project, ids: &[String], props: &ClipProps) -> Result<()> {
    if let Some(g) = props.gain_db
        && !(-60.0..=24.0).contains(&g)
    {
        return Err(Error::invalid(format!(
            "gain {g} dB outside -60..24 dB (decision D12)"
        )));
    }
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let c = &mut p.tracks[ti].clips[ci];
        if let Some(e) = props.enabled {
            c.enabled = e;
        }
        if let Some(n) = &props.name {
            c.name = n.clone();
        }
        if props.muted.is_some() || props.gain_db.is_some() {
            let a = c
                .audio
                .as_mut()
                .ok_or_else(|| Error::invalid(format!("{id} has no audio")))?;
            if let Some(m) = props.muted {
                a.muted = m;
            }
            if let Some(g) = props.gain_db {
                a.gain_db = g;
            }
        }
    }
    Ok(())
}

/// Change Clip Duration > End: each clip runs to the end of its media.
pub fn extend_to_media_end(p: &mut Project, ids: &[String], ripple: Option<bool>) -> Result<()> {
    let mut targets = Vec::new();
    for id in ids {
        let c = p
            .clip(id)
            .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
        let media = p
            .media(&c.media)
            .ok_or_else(|| Error::not_found(format!("no media {}", c.media)))?;
        let room = p.media_limit(media) - c.source_in - c.duration;
        targets.push((c.start, id.clone(), room));
    }
    targets.sort();
    for (_, id, room) in targets.into_iter().rev() {
        if room != 0 {
            trim(p, &id, Edge::Tail, room, ripple)?;
        }
    }
    Ok(())
}

/// Sets or clears timeline marks.
pub fn set_marks(
    p: &mut Project,
    mark_in: Option<Option<i64>>,
    mark_out: Option<Option<i64>>,
) -> Result<()> {
    let mut marks = p.marks.clone();
    if let Some(i) = mark_in {
        marks.mark_in = i;
    }
    if let Some(o) = mark_out {
        marks.mark_out = o;
    }
    if let (Some(i), Some(o)) = (marks.mark_in, marks.mark_out)
        && i >= o
    {
        return Err(Error::invalid(format!("In ({i}) must be before Out ({o})")));
    }
    if marks.mark_in.is_some_and(|i| i < 0) || marks.mark_out.is_some_and(|o| o < 1) {
        return Err(Error::invalid("marks must be on the timeline"));
    }
    p.marks = marks;
    Ok(())
}

/// Marker colors, as in Resolve (p. 718).
pub const MARKER_COLORS: &[&str] = &[
    "blue", "cyan", "green", "yellow", "red", "pink", "purple", "fuchsia", "rose", "lavender",
    "sky", "mint", "lemon", "sand", "cocoa", "cream",
];

fn check_color(color: &str) -> Result<()> {
    if MARKER_COLORS.contains(&color) {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "unknown marker color {color:?}; use one of {}",
            MARKER_COLORS.join(", ")
        )))
    }
}

/// Adds a marker (M, p. 718); returns its id.
pub fn add_marker(
    p: &mut Project,
    at: i64,
    name: String,
    color: Option<String>,
    note: String,
) -> Result<String> {
    if at < 0 {
        return Err(Error::invalid("marker before 0"));
    }
    if let Some(c) = &color {
        check_color(c)?;
    }
    let id = p.alloc_marker_id();
    p.markers.push(Marker {
        id: id.clone(),
        at,
        name,
        color: color.unwrap_or_else(|| "blue".into()),
        note,
    });
    p.markers.sort_by_key(|k| k.at);
    Ok(id)
}

pub fn move_marker(p: &mut Project, id: &str, to: i64) -> Result<()> {
    if to < 0 {
        return Err(Error::invalid("marker before 0"));
    }
    let k = p
        .markers
        .iter_mut()
        .find(|k| k.id == id)
        .ok_or_else(|| Error::not_found(format!("no marker {id}")))?;
    k.at = to;
    p.markers.sort_by_key(|k| k.at);
    Ok(())
}

/// Changes a marker's name, color or note.
pub fn set_marker(
    p: &mut Project,
    id: &str,
    name: Option<String>,
    color: Option<String>,
    note: Option<String>,
) -> Result<()> {
    if let Some(c) = &color {
        check_color(c)?;
    }
    let k = p
        .markers
        .iter_mut()
        .find(|k| k.id == id)
        .ok_or_else(|| Error::not_found(format!("no marker {id}")))?;
    if let Some(n) = name {
        k.name = n;
    }
    if let Some(c) = color {
        k.color = c;
    }
    if let Some(n) = note {
        k.note = n;
    }
    Ok(())
}

pub fn remove_marker(p: &mut Project, id: &str) -> Result<()> {
    let before = p.markers.len();
    p.markers.retain(|k| k.id != id);
    if p.markers.len() == before {
        return Err(Error::not_found(format!("no marker {id}")));
    }
    Ok(())
}

// ---------------------------------------------------------------- source marks, clipboard

/// Sets or clears a media item's source marks (spec §4.4, decision D6).
pub fn set_media_marks(
    p: &mut Project,
    media: &str,
    mark_in: Option<Option<i64>>,
    mark_out: Option<Option<i64>>,
) -> Result<()> {
    let m = p
        .media(media)
        .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
    let frames = p.media_limit(m);
    let mut marks = m.marks.clone();
    if let Some(i) = mark_in {
        marks.mark_in = i;
    }
    if let Some(o) = mark_out {
        marks.mark_out = o;
    }
    if let (Some(i), Some(o)) = (marks.mark_in, marks.mark_out)
        && i >= o
    {
        return Err(Error::invalid(format!("In ({i}) must be before Out ({o})")));
    }
    if marks.mark_in.is_some_and(|i| i < 0 || i >= frames)
        || marks.mark_out.is_some_and(|o| o < 1 || o > frames)
    {
        return Err(Error::invalid(format!(
            "source marks must be inside {media} (0..{frames} frames)"
        )));
    }
    let m = p.media.iter_mut().find(|m| m.id == media).expect("found");
    m.marks = marks;
    Ok(())
}

/// The one clip a tape span mirrors onto, when it does not cross clips.
fn mirrored_piece(
    entries: &[crate::render::TapeEntry],
    marks: &crate::model::TapeMarks,
) -> Option<crate::render::SpanPiece> {
    let (a, b) = crate::render::active_span(entries, marks.mark_in, marks.mark_out)?;
    let mut pieces = crate::render::span_pieces(entries, a, b);
    (pieces.len() == 1).then(|| pieces.remove(0))
}

/// Clears source marks that this tape span wrote onto its one clip.
fn clear_owned_mirror(
    p: &mut Project,
    marks: &crate::model::TapeMarks,
    entries: &[crate::render::TapeEntry],
) {
    let Some(piece) = mirrored_piece(entries, marks) else {
        return;
    };
    let Some(m) = p.media.iter_mut().find(|m| m.id == piece.media) else {
        return;
    };
    if marks.mark_in.is_some() && m.marks.mark_in == Some(piece.range.0) {
        m.marks.mark_in = None;
    }
    if marks.mark_out.is_some() && m.marks.mark_out == Some(piece.range.1) {
        m.marks.mark_out = None;
    }
}

/// Writes a one-clip tape span onto that clip's source marks (D5, D6).
fn write_mirror(
    p: &mut Project,
    marks: &crate::model::TapeMarks,
    entries: &[crate::render::TapeEntry],
) -> Result<()> {
    let Some(piece) = mirrored_piece(entries, marks) else {
        return Ok(());
    };
    set_media_marks(
        p,
        &piece.media,
        marks.mark_in.map(|_| Some(piece.range.0)),
        marks.mark_out.map(|_| Some(piece.range.1)),
    )
}

/// Sets Source Tape In/Out (decision D17). `entries` are `order` laid end
/// to end. A span that stays on one clip is also that clip's source marks;
/// a span across clips is only the tape marks.
pub fn set_tape_marks(
    p: &mut Project,
    new_in: Option<i64>,
    new_out: Option<i64>,
    order: &[String],
    entries: &[crate::render::TapeEntry],
) -> Result<()> {
    let tape_end = entries.last().map(|e| e.start + e.frames).unwrap_or(0);
    if tape_end <= 0 {
        return Err(Error::invalid("no media to mark"));
    }
    let same_order = p.tape_marks.order == order;
    let mut marks = if same_order {
        p.tape_marks.clone()
    } else {
        crate::model::TapeMarks::default()
    };
    let previous = marks.clone();
    if let Some(i) = new_in {
        marks.mark_in = Some(i);
    }
    if let Some(o) = new_out {
        marks.mark_out = Some(o);
    }
    if marks.mark_in.is_some_and(|i| !(0..tape_end).contains(&i))
        || marks.mark_out.is_some_and(|o| o < 1 || o > tape_end)
    {
        return Err(Error::invalid(format!(
            "tape marks must lie on the tape (0..{tape_end} frames)"
        )));
    }
    if let (Some(i), Some(o)) = (marks.mark_in, marks.mark_out)
        && i >= o
    {
        return Err(Error::invalid(format!("In ({i}) must be before Out ({o})")));
    }
    marks.order = order.to_vec();
    if same_order {
        clear_owned_mirror(p, &previous, entries);
    } else if !p.tape_marks.order.is_empty() {
        let old = p.tape_marks.clone();
        if let Ok((_, old_entries)) = crate::render::tape(p, &old.order) {
            clear_owned_mirror(p, &old, &old_entries);
        }
    }
    write_mirror(p, &marks, entries)?;
    p.tape_marks = marks;
    Ok(())
}

/// Clears Source Tape marks. When the span does not cross clips, also clears
/// `media` (the clip under the playhead, Alt+X) and the one-clip mirror.
pub fn clear_tape_marks(p: &mut Project, media: Option<&str>) -> Result<()> {
    let previous = std::mem::take(&mut p.tape_marks);
    let crossed = if previous.order.is_empty() {
        false
    } else if let Ok((_, entries)) = crate::render::tape(p, &previous.order) {
        let crossed =
            crate::render::crossing_span(&entries, previous.mark_in, previous.mark_out).is_some();
        if !crossed {
            clear_owned_mirror(p, &previous, &entries);
        }
        crossed
    } else {
        false
    };
    if !crossed && let Some(media) = media {
        set_media_marks(p, media, Some(None), Some(None))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- clipboard

/// The clips `ids` in timeline order with their tracks, as the clipboard
/// holds them (Ctrl+C, p. 723).
pub fn copy_clips(p: &Project, ids: &[String]) -> Result<Vec<TrackClip>> {
    if ids.is_empty() {
        return Err(Error::usage("no clips to copy"));
    }
    let mut clips = ids
        .iter()
        .map(|id| {
            p.find_clip(id)
                .map(|(ti, ci)| TrackClip {
                    track: p.tracks[ti].id.clone(),
                    clip: p.tracks[ti].clips[ci].clone(),
                })
                .ok_or_else(|| Error::not_found(format!("no clip {id}")))
        })
        .collect::<Result<Vec<_>>>()?;
    clips.sort_by_key(|c| c.clip.start);
    clips.dedup_by(|a, b| a.clip.id == b.clip.id);
    Ok(clips)
}

/// Paste (Ctrl+V, p. 723): copies of `clips` overwrite their own tracks
/// (made if missing) from `at`, keeping their spacing and properties. J/L
/// cuts reset to straight cuts (decision D14); with Ripple On a paste past
/// the end of Track 1 starts there (like decision D10). Returns the new ids.
pub fn paste(p: &mut Project, clips: &[TrackClip], at: i64) -> Result<Vec<String>> {
    if clips.is_empty() {
        return Err(Error::usage("nothing to paste"));
    }
    if at < 0 {
        return Err(Error::invalid("paste position before 0"));
    }
    let mut clips = clips.to_vec();
    clips.sort_by_key(|c| c.clip.start);
    let first = clips[0].clip.start;
    let mut ids = Vec::new();
    for TrackClip { track, clip: c } in clips {
        let media = p
            .media(&c.media)
            .ok_or_else(|| Error::not_found(format!("no media {} in this project", c.media)))?;
        let frames = p.media_limit(media);
        if c.duration <= 0 || c.source_in < 0 || c.source_in + c.duration > frames {
            return Err(Error::invalid(format!(
                "pasted clip {} is outside {} (0..{frames} frames)",
                c.id, c.media
            )));
        }
        if c.audio.as_ref().is_some_and(|a| {
            a.stream as usize >= media.info.audio.len() || !(-60.0..=24.0).contains(&a.gain_db)
        }) {
            return Err(Error::invalid(format!(
                "pasted clip {} has invalid audio settings",
                c.id
            )));
        }
        let has_video = media.info.video.is_some();
        // A track that's gone: the paste makes tracks up to it.
        let ti = ensure_track(p, &track)?;
        unlocked(p, ti)?;
        let mut start = at + (c.start - first);
        if is_main(p, ti) && p.settings.ripple_on {
            start = start.min(p.tracks[ti].end());
        }
        let mut clip = c;
        clip.id = p.alloc_clip_id();
        clip.start = start;
        clip.video = clip.video && has_video && p.tracks[ti].kind == TrackKind::Video;
        if !clip.video && clip.audio.is_none() {
            return Err(Error::invalid(format!(
                "pasted clip has nothing for track {}",
                p.tracks[ti].id
            )));
        }
        if let Some(a) = clip.audio.as_mut() {
            a.head = 0;
            a.tail = 0;
        }
        clear_range(p, ti, start, start + clip.duration);
        ids.push(clip.id.clone());
        place(p, ti, clip);
    }
    fix_all(p);
    Ok(ids)
}

// ---------------------------------------------------------------- bins

/// Checks a bin path: `/`-separated names, none empty.
fn check_bin(path: &str) -> Result<()> {
    if path.is_empty()
        || path
            .split('/')
            .any(|n| n.trim().is_empty() || n != n.trim())
    {
        return Err(Error::usage(format!(
            "bin {path:?}: use names separated by '/', like \"Interviews/Day 1\""
        )));
    }
    Ok(())
}

fn has_bin(p: &Project, path: &str) -> bool {
    path.is_empty() || p.bins.iter().any(|b| b == path)
}

/// Whether `path` is `bin` or inside it.
fn within(path: &str, bin: &str) -> bool {
    path == bin || path.starts_with(&format!("{bin}/"))
}

/// Adds a bin, and any missing parents (p. 655).
pub fn add_bin(p: &mut Project, path: &str) -> Result<()> {
    check_bin(path)?;
    let mut prefix = String::new();
    for name in path.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(name);
        if !has_bin(p, &prefix) {
            p.bins.push(prefix.clone());
        }
    }
    p.bins.sort();
    Ok(())
}

/// Removes a bin with the bins inside it. Their media go to the bin's
/// parent: removing a bin never removes media.
pub fn remove_bin(p: &mut Project, path: &str) -> Result<()> {
    if !has_bin(p, path) || path.is_empty() {
        return Err(Error::not_found(format!("no bin {path:?}")));
    }
    let parent = path.rsplit_once('/').map_or("", |(a, _)| a).to_string();
    p.bins.retain(|b| !within(b, path));
    for m in &mut p.media {
        if within(&m.bin, path) {
            m.bin = parent.clone();
        }
    }
    Ok(())
}

/// Renames or moves a bin, with what is inside it.
pub fn rename_bin(p: &mut Project, path: &str, to: &str) -> Result<()> {
    if !has_bin(p, path) || path.is_empty() {
        return Err(Error::not_found(format!("no bin {path:?}")));
    }
    check_bin(to)?;
    if within(to, path) {
        return Err(Error::invalid(format!(
            "can't move bin {path:?} into itself"
        )));
    }
    if has_bin(p, to) {
        return Err(Error::invalid(format!("bin {to:?} exists")));
    }
    let rebase = |b: &str| format!("{to}{}", &b[path.len()..]);
    for b in &mut p.bins {
        if within(b, path) {
            *b = rebase(b);
        }
    }
    for m in &mut p.media {
        if within(&m.bin, path) {
            m.bin = rebase(&m.bin);
        }
    }
    add_bin(p, to)?;
    p.bins.sort();
    p.bins.dedup();
    Ok(())
}

/// Puts media into a bin (`""`: the top level).
pub fn move_media(p: &mut Project, ids: &[String], bin: &str) -> Result<()> {
    if !has_bin(p, bin) {
        return Err(Error::not_found(format!(
            "no bin {bin:?}; add it with bins.add"
        )));
    }
    for id in ids {
        let m = p
            .media
            .iter_mut()
            .find(|m| &m.id == id)
            .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
        m.bin = bin.to_string();
    }
    Ok(())
}
