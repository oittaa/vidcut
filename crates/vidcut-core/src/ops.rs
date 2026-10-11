//! Cut-page edit operations (spec §7–§8) as functions on a [`Project`].
//!
//! Every operation works on resolved frame numbers; parsing TimeSpecs and
//! ClipRefs happens in the request layer. Operations may leave the project
//! half-modified when they fail: callers apply them to a copy (the Editor does).
//!
//! In M1 only track V1 is editable. It is the Cut page's Track 1: magnetic
//! while Ripple On is set (spec §6).

use crate::error::{Error, Result};
use crate::model::{Clip, ClipAudio, Marker, Project, Track};

/// The main (magnetic) track.
pub const MAIN: &str = "V1";

/// A media range to put on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub media: String,
    /// Source frames `[start, end)` (project frames from the media's first frame).
    pub range: (i64, i64),
    /// Leave the audio out (the Cut page's Video Only toggle).
    pub video_only: bool,
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

fn main_index(p: &Project) -> Result<usize> {
    let i = p
        .tracks
        .iter()
        .position(|t| t.id == MAIN)
        .ok_or_else(|| Error::internal("no V1 track"))?;
    if p.tracks[i].locked {
        return Err(Error::invalid("track V1 is locked"));
    }
    Ok(i)
}

/// Locates a clip that must be on V1 (other tracks arrive with multi-track).
fn on_main(p: &Project, id: &str) -> Result<(usize, usize)> {
    let (ti, ci) = p
        .find_clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    if p.tracks[ti].id != MAIN {
        return Err(Error::invalid(format!(
            "{id} is on {}; only V1 is editable until multi-track editing (M6)",
            p.tracks[ti].id
        )));
    }
    if p.tracks[ti].locked {
        return Err(Error::invalid(format!(
            "track {} is locked",
            p.tracks[ti].id
        )));
    }
    Ok((ti, ci))
}

fn clip_index(t: &Track, id: &str) -> usize {
    t.clips
        .iter()
        .position(|c| c.id == id)
        .expect("clip present")
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
        if let Some(a) = clips[i].audio.as_mut() {
            if prev_tail != Some(a.head) {
                a.head = 0;
            }
            if next_head != Some(a.tail) {
                a.tail = 0;
            }
        }
    }
}

/// Builds a new clip (not yet placed) from a source range.
fn make_clip(p: &mut Project, src: &Source) -> Result<Clip> {
    let media = p
        .media(&src.media)
        .ok_or_else(|| Error::not_found(format!("no media {}", src.media)))?;
    if media.info.video.is_none() {
        return Err(Error::invalid(format!(
            "{} has no video; audio-only media goes on audio tracks (multi-track, M6)",
            media.id
        )));
    }
    let frames = p.media_frames(media);
    let (a, b) = src.range;
    if a < 0 || b > frames || a >= b {
        return Err(Error::invalid(format!(
            "source range {a}..{b} is outside {} (0..{frames} frames)",
            media.id
        )));
    }
    let audio = (!src.video_only && !media.info.audio.is_empty()).then_some(ClipAudio {
        stream: 0,
        head: 0,
        tail: 0,
        gain_db: 0.0,
        muted: false,
    });
    let id = p.alloc_clip_id();
    Ok(Clip {
        id,
        media: src.media.clone(),
        start: 0,
        duration: b - a,
        source_in: a,
        video: true,
        audio,
        enabled: true,
        name: None,
    })
}

fn ripple_main(p: &Project, ripple: Option<bool>) -> bool {
    ripple.unwrap_or(p.settings.ripple_on)
}

// ---------------------------------------------------------------- edit functions (spec §7)

/// Append: after the last clip, ignoring the playhead (p. 697).
pub fn append(p: &mut Project, src: &Source) -> Result<String> {
    let ti = main_index(p)?;
    let mut clip = make_clip(p, src)?;
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

/// Smart Insert: insert at the edit nearest the playhead; later clips move
/// right (p. 697).
pub fn smart_insert(p: &mut Project, src: &Source, playhead: i64) -> Result<String> {
    let at = nearest_edit(p, playhead);
    insert_at(p, src, at)
}

/// Insert at an exact frame, splitting a clip there if needed (API-only,
/// deviation X5). With Ripple On, a position past the end means the end.
pub fn insert_at(p: &mut Project, src: &Source, at: i64) -> Result<String> {
    let ti = main_index(p)?;
    if at < 0 {
        return Err(Error::invalid("insert position before 0"));
    }
    let at = if p.settings.ripple_on {
        at.min(p.tracks[ti].end())
    } else {
        at
    };
    let mut clip = make_clip(p, src)?;
    split_at(p, ti, at);
    shift_from(p, ti, at, clip.duration);
    clip.start = at;
    let id = clip.id.clone();
    place(p, ti, clip);
    fix_audio(p, ti);
    Ok(id)
}

/// Overwrite (F10, p. 701): writes over V1 without moving other clips.
/// Placement: the timeline In; else backtimed to a lone timeline Out; else
/// `at`. With both marks set it is a three-point edit and the marked range sets
/// the length (decision D7).
pub fn overwrite(p: &mut Project, src: &Source, at: i64) -> Result<String> {
    let ti = main_index(p)?;
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
    if start < 0 {
        return Err(Error::invalid("overwrite would start before 0"));
    }
    // Decision D10: with Ripple On, Track 1 stays gapless.
    let start = if p.settings.ripple_on {
        start.min(p.tracks[ti].end())
    } else {
        start
    };
    let mut clip = make_clip(p, &src)?;
    clear_range(p, ti, start, start + clip.duration);
    clip.start = start;
    let id = clip.id.clone();
    place(p, ti, clip);
    fix_audio(p, ti);
    Ok(id)
}

/// Ripple Overwrite (Shift+F10, p. 698): replaces the timeline In–Out range,
/// or else the whole clip under `at`; everything after moves by the length
/// difference. The source length always wins (decision D8).
pub fn ripple_overwrite(p: &mut Project, src: &Source, at: i64) -> Result<String> {
    let ti = main_index(p)?;
    let (a, b) = match (p.marks.mark_in, p.marks.mark_out) {
        (Some(i), Some(o)) => (i, o),
        _ => {
            let t = &p.tracks[ti];
            let ci = t
                .clip_at(at)
                .ok_or_else(|| Error::invalid("no clip under the playhead to replace"))?;
            (t.clips[ci].start, t.clips[ci].end())
        }
    };
    // A marked range past the end of a magnetic V1 replaces nothing: the clip
    // goes at the end so V1 stays gapless (like decision D10).
    let end = p.tracks[ti].end();
    let (a, b) = if p.settings.ripple_on && a > end {
        (end, end)
    } else {
        (a, b)
    };
    let mut clip = make_clip(p, src)?;
    clear_range(p, ti, a, b);
    shift_from(p, ti, b, clip.duration - (b - a));
    clip.start = a;
    let id = clip.id.clone();
    place(p, ti, clip);
    fix_audio(p, ti);
    Ok(id)
}

// ---------------------------------------------------------------- timeline editing (spec §8)

/// Split (Ctrl+\, p. 723) at each of `at`. Decision D2: if any of `selected`
/// lies under a split point, only selected clips are split there; otherwise
/// every clip under it on the editable track. Returns the new right-hand ids.
pub fn split(p: &mut Project, at: &[i64], selected: &[String]) -> Result<Vec<String>> {
    let ti = main_index(p)?;
    for id in selected {
        on_main(p, id)?;
    }
    let mut created = Vec::new();
    let mut points: Vec<i64> = at.to_vec();
    points.sort_unstable();
    points.dedup();
    for t in points {
        let covering = p.tracks[ti]
            .clip_at(t)
            .map(|ci| p.tracks[ti].clips[ci].id.clone());
        let allowed =
            selected.is_empty() || covering.as_ref().is_some_and(|id| selected.contains(id));
        if allowed && let Some(id) = split_at(p, ti, t) {
            created.push(id);
        }
    }
    if created.is_empty() {
        return Err(Error::invalid(
            "nothing to split: no clip strictly covers the split point",
        ));
    }
    fix_audio(p, ti);
    Ok(created)
}

/// Delete clips (p. 724–725). On V1 with ripple (Ripple On unless `lift`),
/// later clips move left to close each gap; a lift leaves the gaps.
pub fn delete(p: &mut Project, ids: &[String], lift: bool) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::usage("no clips to delete"));
    }
    let ti = main_index(p)?;
    let mut spans = Vec::new();
    for id in ids {
        let (_, ci) = on_main(p, id)?;
        let c = &p.tracks[ti].clips[ci];
        spans.push((c.start, c.end(), c.id.clone()));
    }
    let ripple = !lift && p.settings.ripple_on;
    spans.sort();
    spans.dedup();
    for (start, end, id) in spans.into_iter().rev() {
        let track = &mut p.tracks[ti];
        track.clips.retain(|c| c.id != id);
        if ripple {
            shift_from(p, ti, end, -(end - start));
        }
    }
    fix_audio(p, ti);
    Ok(())
}

/// Delete the range `[a, b)` (Delete with timeline In/Out set, p. 725);
/// ripples unless `lift`.
pub fn delete_range(p: &mut Project, a: i64, b: i64, lift: bool) -> Result<()> {
    if a >= b {
        return Err(Error::usage(format!("empty range {a}..{b}")));
    }
    let ti = main_index(p)?;
    clear_range(p, ti, a, b);
    if !lift && p.settings.ripple_on {
        shift_from(p, ti, b, -(b - a));
    }
    fix_audio(p, ti);
    Ok(())
}

/// Closes the gap on V1 containing frame `at`.
pub fn close_gap(p: &mut Project, at: i64) -> Result<()> {
    let ti = main_index(p)?;
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
    shift_from(p, ti, b, -(b - a));
    fix_audio(p, ti);
    Ok(())
}

/// Takes clips out of V1, closing the gaps; returns them in timeline order and
/// maps `to` into the resulting timeline.
fn extract(p: &mut Project, ti: usize, ids: &[String], to: i64) -> Result<(Vec<Clip>, i64)> {
    let mut taken: Vec<Clip> = Vec::new();
    for id in ids {
        let (_, ci) = on_main(p, id)?;
        taken.push(p.tracks[ti].clips[ci].clone());
    }
    taken.sort_by_key(|c| c.start);
    taken.dedup_by(|a, b| a.id == b.id);
    if let Some(c) = taken.iter().find(|c| c.start < to && to < c.end()) {
        return Err(Error::invalid(format!("cannot move {} into itself", c.id)));
    }
    let mut mapped = to;
    for c in taken.iter().rev() {
        p.tracks[ti].clips.retain(|x| x.id != c.id);
        shift_from(p, ti, c.end(), -c.duration);
        if c.end() <= to {
            mapped -= c.duration;
        }
    }
    Ok((taken, mapped))
}

/// Move clips on V1 (p. 720–723).
///
/// `to` is a position in the timeline as it is before the move.
/// - `Insert` (dropping on an edit point): the clips are taken out, the gaps
///   they leave close, and they go in at `to`, pushing later clips right.
/// - `Overwrite` (dropping after hovering, or over the edge of a neighbor):
///   the clips are lifted, written over whatever is at `to`, and then, with
///   Ripple On, what remains of the spans they left closes (p. 722: moving a
///   clip a few frames onto its neighbor shortens the neighbor).
pub fn move_clips(p: &mut Project, ids: &[String], to: i64, mode: MoveMode) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::usage("no clips to move"));
    }
    let ti = main_index(p)?;
    if to < 0 {
        return Err(Error::invalid("move target before 0"));
    }
    if mode == MoveMode::Overwrite {
        return move_overwrite(p, ti, ids, to);
    }
    let (clips, to) = extract(p, ti, ids, to)?;
    let total: i64 = clips.iter().map(|c| c.duration).sum();
    let to = to.min(p.tracks[ti].end());
    split_at(p, ti, to);
    shift_from(p, ti, to, total);
    let mut at = to;
    for mut c in clips {
        c.start = at;
        at += c.duration;
        place(p, ti, c);
    }
    fix_audio(p, ti);
    Ok(())
}

fn move_overwrite(p: &mut Project, ti: usize, ids: &[String], to: i64) -> Result<()> {
    let mut clips = Vec::new();
    for id in ids {
        let (_, ci) = on_main(p, id)?;
        clips.push(p.tracks[ti].clips[ci].clone());
    }
    clips.sort_by_key(|c| c.start);
    clips.dedup_by(|a, b| a.id == b.id);
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
    for mut c in clips.iter().cloned() {
        c.start = at;
        at += c.duration;
        place(p, ti, c);
    }
    if magnetic {
        // Close the vacated spans not covered by the clips' new place, right
        // to left so earlier spans keep their positions.
        let mut gaps: Vec<(i64, i64)> = Vec::new();
        for c in &clips {
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
    Ok(())
}

/// Move a clip onto another and replace it, rippling the difference (dropping
/// on a clip right away, p. 721).
pub fn move_replace(p: &mut Project, id: &str, onto: &str) -> Result<()> {
    let ti = main_index(p)?;
    if id == onto {
        return Err(Error::invalid("a clip cannot replace itself"));
    }
    on_main(p, onto)?;
    let (mut clips, _) = extract(p, ti, &[id.to_string()], 0)?;
    let clip = clips.pop().expect("one clip");
    let target = &p.tracks[ti].clips[clip_index(&p.tracks[ti], onto)];
    let (a, b) = (target.start, target.end());
    clear_range(p, ti, a, b);
    shift_from(p, ti, b, clip.duration - (b - a));
    place(p, ti, Clip { start: a, ..clip });
    fix_audio(p, ti);
    Ok(())
}

/// Checks a clip's video and audio stay within its media.
fn check_source(p: &Project, c: &Clip) -> Result<()> {
    let m = p
        .media(&c.media)
        .ok_or_else(|| Error::not_found(format!("no media {}", c.media)))?;
    let frames = p.media_frames(m);
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
/// On V1 with ripple (Ripple On unless overridden), the clip keeps its start
/// and later clips move; without ripple the edge moves in place, writing over
/// a neighbor when extending into it.
pub fn trim(p: &mut Project, id: &str, edge: Edge, delta: i64, ripple: Option<bool>) -> Result<()> {
    let (ti, ci) = on_main(p, id)?;
    let ripple = ripple_main(p, ripple);
    let c = p.tracks[ti].clips[ci].clone();
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
    let (ti, _) = on_main(p, id)?;
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
    let (ti, ci) = on_main(p, id)?;
    let mut c = p.tracks[ti].clips[ci].clone();
    c.source_in += delta;
    check_source(p, &c)?;
    p.tracks[ti].clips[ci] = c;
    Ok(())
}

/// Audio-only roll at a clip's edge: a J/L cut (p. 728). Only the audio edit
/// moves; positive `delta` lets the outgoing audio run longer.
pub fn audio_roll(p: &mut Project, id: &str, edge: Edge, delta: i64) -> Result<()> {
    let (ti, _) = on_main(p, id)?;
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
        let ti = main_index(p)?;
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
        let (ti, ci) = p
            .find_clip(id)
            .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
        if p.tracks[ti].locked {
            return Err(Error::invalid(format!(
                "track {} is locked",
                p.tracks[ti].id
            )));
        }
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
        let room = p.media_frames(media) - c.source_in - c.duration;
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

/// Track header toggles (pp. 647, 717).
pub fn set_track(
    p: &mut Project,
    id: &str,
    locked: Option<bool>,
    muted: Option<bool>,
    video_enabled: Option<bool>,
) -> Result<()> {
    let t = p
        .track_mut(id)
        .ok_or_else(|| Error::not_found(format!("no track {id}")))?;
    if video_enabled.is_some() && t.kind != crate::model::TrackKind::Video {
        return Err(Error::invalid(format!("{id} is an audio track")));
    }
    if let Some(l) = locked {
        t.locked = l;
    }
    if let Some(m) = muted {
        t.muted = m;
    }
    if let Some(v) = video_enabled {
        t.video_enabled = v;
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
    let frames = p.media_frames(m);
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

/// The clips `ids` in timeline order, as the clipboard holds them (Ctrl+C,
/// p. 723).
pub fn copy_clips(p: &Project, ids: &[String]) -> Result<Vec<Clip>> {
    if ids.is_empty() {
        return Err(Error::usage("no clips to copy"));
    }
    let mut clips = ids
        .iter()
        .map(|id| {
            p.clip(id)
                .cloned()
                .ok_or_else(|| Error::not_found(format!("no clip {id}")))
        })
        .collect::<Result<Vec<_>>>()?;
    clips.sort_by_key(|c| c.start);
    clips.dedup_by(|a, b| a.id == b.id);
    Ok(clips)
}

/// Paste (Ctrl+V, p. 723): copies of `clips` overwrite Track 1 from `at`,
/// keeping their spacing and properties. J/L cuts reset to straight cuts
/// (decision D14); with Ripple On a paste past the end starts at the end
/// (like decision D10). Returns the new clip ids.
pub fn paste(p: &mut Project, clips: &[Clip], at: i64) -> Result<Vec<String>> {
    let ti = main_index(p)?;
    if clips.is_empty() {
        return Err(Error::usage("nothing to paste"));
    }
    if at < 0 {
        return Err(Error::invalid("paste position before 0"));
    }
    let mut clips = clips.to_vec();
    clips.sort_by_key(|c| c.start);
    let first = clips[0].start;
    let mut ids = Vec::new();
    for c in clips {
        let media = p
            .media(&c.media)
            .ok_or_else(|| Error::not_found(format!("no media {} in this project", c.media)))?;
        let frames = p.media_frames(media);
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
        let mut start = at + (c.start - first);
        if p.settings.ripple_on {
            start = start.min(p.tracks[ti].end());
        }
        let mut clip = c;
        clip.id = p.alloc_clip_id();
        clip.start = start;
        clip.video = true;
        if let Some(a) = clip.audio.as_mut() {
            a.head = 0;
            a.tail = 0;
        }
        clear_range(p, ti, start, start + clip.duration);
        ids.push(clip.id.clone());
        place(p, ti, clip);
    }
    fix_audio(p, ti);
    Ok(ids)
}
