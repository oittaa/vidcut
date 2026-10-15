//! Transitions other editors put on edit points (deviation X9): kept while
//! they fit, shown and played as cuts, written back.
//!
//! - A transition sits on a cut between two clips' pictures, or their
//!   sounds, or between a clip and a gap (a fade from or to black). It
//!   names the clip on each side; a split of the clip before the cut hands
//!   it to the part that ends there.
//! - It fits while those clips still meet at the cut (or the gap is still
//!   there), the clip after the cut has `before` frames of media before its
//!   first one and the clip before the cut `after` frames after its last:
//!   during a transition each clip shows frames past its edge.
//! - An edit after which one no longer fits drops it, and says so.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{EDITOR, VIDCUT, gap, rt, schema};
use crate::model::{Clip, Media, Project, Track, Transition};

/// The name transition `t` goes by: `Cross Dissolve`, `Cross Fade +3 dB`.
pub fn name(t: &Transition) -> String {
    otio_name(&t.otio)
}

/// The name of OTIO transition `o`.
pub(super) fn otio_name(o: &Value) -> String {
    [
        &o["name"],
        &o["metadata"][EDITOR]["Effects"]["Name"],
        &o["transition_type"],
    ]
    .into_iter()
    .filter_map(Value::as_str)
    .find(|n| !n.is_empty())
    .unwrap_or("transition")
    .to_string()
}

/// Whether transition `t` plays as a dissolve (decision D50): OTIO's
/// `SMPTE_Dissolve`, but for another of Resolve's dissolves by its effect's
/// name. Any other plays as a cut.
pub fn dissolves(t: &Transition) -> bool {
    let effect = &t.otio["metadata"][EDITOR]["Effects"]["Effect Name"];
    t.otio["transition_type"] == "SMPTE_Dissolve"
        && effect.as_str().is_none_or(|n| n == "Cross Dissolve")
}

/// A transition that plays on a track ([`playing`]).
#[derive(Debug, Clone, Copy)]
pub struct Playing<'a> {
    pub transition: &'a Transition,
    /// Its frames on the timeline, `[from, to)`.
    pub frames: (i64, i64),
    /// The clip before its cut and the clip after it; none for a gap.
    pub before: Option<&'a Clip>,
    pub after: Option<&'a Clip>,
}

/// The transitions of `track` that play on its pictures (or its sounds,
/// `sound`), in order (decision D50): those that fit and that
/// `plays` takes, whose frames hold nothing of the track but the clip
/// before the cut until it and the clip after it from it, and that start
/// after the one before ends.
pub fn playing<'a>(
    p: &'a Project,
    track: &'a Track,
    sound: bool,
    plays: impl Fn(&Transition) -> bool,
) -> Vec<Playing<'a>> {
    let cuts = Cuts::of(p, track);
    let find = |id: &Option<String>| {
        id.as_ref()
            .and_then(|id| cuts.clips.get(id.as_str()).copied())
    };
    let mut found: Vec<Playing<'a>> = Vec::new();
    for t in track
        .transitions
        .iter()
        .filter(|t| t.sound == sound && plays(t))
    {
        let Some(cut) = cuts.place(p, t) else {
            continue;
        };
        let (a, b) = (cut - t.before, cut + t.after);
        let (before, after) = (find(&t.from), find(&t.to));
        let covers = |c: Option<&Clip>, x: i64, y: i64| {
            c.is_none_or(|c| span(c, sound).is_some_and(|(s, e)| s <= x && e >= y))
        };
        let others_clear = track.clips.iter().all(|c| {
            let side = |o: Option<&Clip>| o.is_some_and(|o| std::ptr::eq(o, c));
            side(before) || side(after) || span(c, sound).is_none_or(|(s, e)| e <= a || s >= b)
        });
        if b > a && covers(before, a, cut) && covers(after, cut, b) && others_clear {
            found.push(Playing {
                transition: t,
                frames: (a, b),
                before,
                after,
            });
        }
    }
    found.sort_by_key(|x| x.frames.0);
    let mut out: Vec<Playing<'a>> = Vec::new();
    for x in found {
        if out.last().is_none_or(|l| l.frames.1 <= x.frames.0) {
            out.push(x);
        }
    }
    out
}

/// The span of `c`'s picture (or sound) on the timeline.
fn span(c: &Clip, sound: bool) -> Option<(i64, i64)> {
    if sound {
        c.audio_range()
    } else {
        c.video.then(|| (c.start, c.end()))
    }
}

/// A track of `p`, found once to place its transitions: its clips by id,
/// the times where a picture or a sound starts or ends (`true`: a sound),
/// and the pool's items by id.
pub struct Cuts<'a> {
    clips: HashMap<&'a str, &'a Clip>,
    starts: HashSet<(bool, i64)>,
    ends: HashSet<(bool, i64)>,
    pool: HashMap<&'a str, &'a Media>,
}

impl<'a> Cuts<'a> {
    pub fn of(p: &'a Project, track: &'a Track) -> Self {
        let mut cuts = Self {
            clips: HashMap::new(),
            starts: HashSet::new(),
            ends: HashSet::new(),
            pool: p.pool(),
        };
        for c in &track.clips {
            cuts.clips.entry(c.id.as_str()).or_insert(c);
            for sound in [false, true] {
                if let Some((a, b)) = span(c, sound) {
                    cuts.starts.insert((sound, a));
                    cuts.ends.insert((sound, b));
                }
            }
        }
        cuts
    }

    /// The cut transition `t` on the track is on, if it still fits there.
    pub fn place(&self, p: &Project, t: &Transition) -> Option<i64> {
        place(p, self, t)
    }
}

/// [`Project::source_frames`], its media found in `cuts`.
fn source_frames(p: &Project, cuts: &Cuts, c: &Clip) -> Option<i64> {
    match (&c.compound, &c.generator) {
        (None, None) => cuts.pool.get(c.media.as_str()).map(|m| p.media_limit(m)),
        _ => p.source_frames(c),
    }
}

fn place(p: &Project, cuts: &Cuts, t: &Transition) -> Option<i64> {
    let clip = |id: &Option<String>| -> Option<Option<&Clip>> {
        match id {
            None => Some(None),
            Some(id) => cuts.clips.get(id.as_str()).copied().map(Some),
        }
    };
    let (from, to) = (clip(&t.from)?, clip(&t.to)?);
    let cut = match (from, to) {
        (Some(f), Some(g)) => {
            let at = span(f, t.sound)?.1;
            (span(g, t.sound)?.0 == at).then_some(at)?
        }
        (Some(f), None) => span(f, t.sound)?.1,
        (None, Some(g)) => span(g, t.sound)?.0,
        (None, None) => return None,
    };
    // A gap side: nothing of the track there.
    let meets = |end: bool| {
        let at = (t.sound, cut);
        if end {
            cuts.ends.contains(&at)
        } else {
            cuts.starts.contains(&at)
        }
    };
    if (from.is_none() && meets(true)) || (to.is_none() && meets(false)) {
        return None;
    }
    // Media past the edges (a generator has any).
    if let Some(g) = to.filter(|g| !g.is_generator())
        && g.source_at(cut) < t.before
    {
        return None;
    }
    if let Some(f) = from.filter(|f| !f.is_generator())
        && f.source_at(cut) + t.after > source_frames(p, cuts, f)?
    {
        return None;
    }
    Some(cut)
}

/// Drops the transitions that no longer fit their edit points; how many.
pub fn prune(p: &mut Project) -> usize {
    let mut dropped = 0;
    for ti in 0..p.tracks.len() {
        let track = &p.tracks[ti];
        let cuts = Cuts::of(p, track);
        let fits: Vec<bool> = track
            .transitions
            .iter()
            .map(|t| cuts.place(p, t).is_some())
            .collect();
        let mut keep = fits.into_iter();
        let before = p.tracks[ti].transitions.len();
        p.tracks[ti]
            .transitions
            .retain(|_| keep.next().unwrap_or(false));
        dropped += before - p.tracks[ti].transitions.len();
    }
    dropped
}

/// What an edit that dropped `n` transitions says.
pub fn dropped(n: usize) -> String {
    format!(
        "{n} transition(s) from another editor no longer fit their edit points and were dropped"
    )
}

/// Transition `t` as written at `rate`.
fn written(t: &Transition, rate: f64) -> Value {
    let mut v = (*t.otio).clone();
    v["in_offset"] = rt(t.before as f64, rate);
    v["out_offset"] = rt(t.after as f64, rate);
    v["metadata"][VIDCUT]["transition"] = Value::String(t.id.clone());
    v
}

/// A track's transitions found once for an export: by the clip after each
/// and, for those into a gap, by the clip before (`true`: on the sound).
/// The first of each.
pub(super) struct Edits<'a> {
    cuts: Cuts<'a>,
    into: HashMap<(bool, &'a str), &'a Transition>,
    out_of: HashMap<(bool, &'a str), &'a Transition>,
}

impl<'a> Edits<'a> {
    pub(super) fn of(p: &'a Project, track: &'a Track) -> Self {
        let mut edits = Self {
            cuts: Cuts::of(p, track),
            into: HashMap::new(),
            out_of: HashMap::new(),
        };
        for t in &track.transitions {
            match (&t.from, &t.to) {
                (_, Some(to)) => {
                    edits.into.entry((t.sound, to.as_str())).or_insert(t);
                }
                (Some(from), None) => {
                    edits.out_of.entry((t.sound, from.as_str())).or_insert(t);
                }
                (None, None) => {}
            }
        }
        edits
    }
}

/// The transition to write before the item of clip `c`'s picture (or
/// sound), which starts at `s` in the export, after `items`: one at the
/// clip's first frame, from the clip or gap just written.
pub(super) fn head(
    p: &Project,
    edits: &Edits,
    c: &Clip,
    sound: bool,
    s: i64,
    items: &[Value],
    rate: f64,
) -> Option<Value> {
    let t = *edits.into.get(&(sound, c.id.as_str()))?;
    if edits.cuts.place(p, t)? != s {
        return None;
    }
    let last = items.last()?;
    let follows = match &t.from {
        Some(f) => is_clip_of(last, f),
        None => schema(last).starts_with("Gap"),
    };
    follows.then(|| written(t, rate))
}

/// The transition to write after the item of clip `c`'s picture (or
/// sound), which ends at `e` in the export: one into a gap at its end.
pub(super) fn tail(
    p: &Project,
    edits: &Edits,
    c: &Clip,
    sound: bool,
    e: i64,
    rate: f64,
) -> Option<Value> {
    let t = *edits.out_of.get(&(sound, c.id.as_str()))?;
    (edits.cuts.place(p, t)? == e).then(|| written(t, rate))
}

/// Whether OTIO item `item` is one vidcut wrote for clip `id`.
fn is_clip_of(item: &Value, id: &str) -> bool {
    item["metadata"][VIDCUT]["clip"] == id
}

/// Ends a written track: a transition into a gap needs one after it.
pub(super) fn close(items: &mut Vec<Value>, rate: f64) {
    if let Some(last) = items.last()
        && schema(last).starts_with("Transition")
    {
        let after = last["out_offset"]["value"].as_f64().unwrap_or(0.0).max(1.0);
        items.push(gap(after as i64, rate));
    }
}
