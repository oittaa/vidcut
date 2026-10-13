//! Clips on other tracks follow Track 1 when it ripples (spec §6, decision
//! D21).
//!
//! A clip on any track but Track 1 is attached to the Track 1 picture under
//! its first frame: the clip there and that clip's source frame. After an
//! edit ripples Track 1, the clip moves so it starts over the same picture:
//! - **The picture moved** (later clips shifted, or its clip was moved, even
//!   to another track): the clip moves with it.
//! - **Its clip's Out was trimmed to before it:** the clip is deleted.
//! - **Its clip's head was trimmed past it:** it moves to the clip's new start.
//! - **Its clip was replaced** (Ripple Overwrite, Replace): it stays where it
//!   is while the new clip reaches it.
//! - **Its clip was deleted:** it moves to where Track 1 closed up.
//! - **Over a gap or past the end:** it keeps its distance to the next
//!   Track 1 clip, or, if that one is gone, to the end of the previous one.
//!
//! Locked tracks don't follow. A clip that lands on another clip of its track
//! moves to the first unlocked track of its kind above (audio: below) with
//! room, made if needed.

use std::collections::{HashMap, HashSet};

use crate::model::{Clip, Project, TrackKind};

use super::{MAIN, fix_audio, tracks};

/// Track 1 and the clip ids before an edit.
pub(crate) struct Before {
    main: Vec<Clip>,
    ids: HashSet<String>,
}

impl Before {
    /// Track 1 clip `id` as it was.
    pub(crate) fn main_clip(&self, id: &str) -> Option<&Clip> {
        self.main.iter().find(|c| c.id == id)
    }

    pub(crate) fn of(p: &Project) -> Self {
        Self {
            main: p.track(MAIN).map(|t| t.clips.clone()).unwrap_or_default(),
            ids: p
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter().map(|c| c.id.clone()))
                .collect(),
        }
    }
}

/// What the edit did, beyond what the before/after states show.
#[derive(Default)]
pub(crate) struct Edit<'a> {
    /// New clips the edit made (as opposed to parts of split clips).
    pub created: &'a [String],
    /// Clips the edit put in place itself: they don't follow.
    pub placed: &'a [String],
    /// Track 1 spans `[a, b)` whose clips were replaced in place: clips over
    /// them keep their position.
    pub kept: &'a [(i64, i64)],
    /// The Track 1 clip whose edge was trimmed, if any.
    pub trimmed: Option<&'a str>,
}

struct Mapper<'a> {
    before: &'a Before,
    /// Every clip now, by id.
    all: HashMap<&'a str, &'a Clip>,
    /// Track 1 clips now, by id.
    main: HashMap<&'a str, &'a Clip>,
    /// Clips that are parts of split old clips.
    parts: Vec<&'a Clip>,
    edit: &'a Edit<'a>,
}

impl Mapper<'_> {
    /// The clip now showing source frame `s` of old Track 1 clip `c`.
    fn successor(&self, c: &Clip, s: i64) -> Option<&Clip> {
        let shows = |n: &Clip| {
            n.media == c.media
                && n.generator == c.generator
                && n.source_in <= s
                && s < n.source_in + n.duration
        };
        if let Some(n) = self.all.get(c.id.as_str())
            && shows(n)
        {
            return Some(n);
        }
        self.parts.iter().copied().find(|n| {
            shows(n)
                && n.source_in >= c.source_in
                && n.source_in + n.duration <= c.source_in + c.duration
        })
    }

    /// Where Track 1 closed up over old clip `i`.
    fn closed(&self, i: usize) -> Option<i64> {
        let main = &self.before.main;
        let c = &main[i];
        if let Some(n) = main.get(i + 1)
            && let Some(now) = self.main.get(n.id.as_str())
        {
            return Some(now.start - (n.start - c.end()));
        }
        main[..i]
            .iter()
            .rev()
            .find_map(|q| self.main.get(q.id.as_str()).map(|now| (q, now)))
            .map(|(q, now)| now.end() + (c.start - q.end()))
    }

    /// The new place of a clip that started at old position `x`; `None`: it
    /// goes. `own` is the clip itself (which may have left Track 1).
    fn map(&self, x: i64, own: &str) -> Option<i64> {
        if self.edit.kept.iter().any(|&(a, b)| a <= x && x < b) {
            return Some(x);
        }
        let main = &self.before.main;
        let i = main.partition_point(|c| c.start <= x);
        if i > 0 && x < main[i - 1].end() {
            let c = &main[i - 1];
            if c.id == own {
                // Over its own old place: it stays put relative to what came
                // before it.
                return Some(
                    main[..i - 1]
                        .iter()
                        .rev()
                        .find_map(|q| self.main.get(q.id.as_str()).map(|now| (q, now)))
                        .map_or(x, |(q, now)| now.end() + (x - q.end())),
                );
            }
            let s = c.source_at(x);
            if let Some(n) = self.successor(c, s) {
                return Some(n.start + (s - n.source_in));
            }
            if let Some(n) = self.all.get(c.id.as_str()) {
                if s < n.source_in {
                    return Some(n.start);
                }
                if self.edit.trimmed == Some(c.id.as_str()) {
                    return None;
                }
            }
            return Some(self.closed(i - 1).unwrap_or(x));
        }
        // Over a gap or past the end: keep the distance to the next clip, or
        // else to the end of the previous one.
        if let Some(n) = main.get(i)
            && let Some(now) = self.main.get(n.id.as_str())
        {
            return Some(now.start - (n.start - x));
        }
        if i > 0
            && let Some(now) = self.main.get(main[i - 1].id.as_str())
        {
            return Some(now.end() + (x - main[i - 1].end()));
        }
        Some(x)
    }
}

/// Moves the clips on other tracks after an edit rippled Track 1.
pub(crate) fn follow(p: &mut Project, before: &Before, edit: &Edit<'_>) {
    let Some(main_index) = p.track_index(MAIN) else {
        return;
    };
    let snapshot = p.tracks.clone();
    let all: HashMap<&str, &Clip> = snapshot
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| (c.id.as_str(), c)))
        .collect();
    let mapper = Mapper {
        before,
        main: snapshot[main_index]
            .clips
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect(),
        parts: all
            .values()
            .copied()
            .filter(|c| !before.ids.contains(&c.id) && !edit.created.contains(&c.id))
            .collect(),
        all,
        edit,
    };
    let mut moved = HashSet::new();
    for (ti, track) in p.tracks.iter_mut().enumerate() {
        if ti == main_index || track.locked {
            continue;
        }
        let mut gone = HashSet::new();
        for c in &mut track.clips {
            if edit.placed.contains(&c.id) {
                continue;
            }
            match mapper.map(c.start, &c.id) {
                Some(x) if x != c.start => {
                    c.start = x.max(0);
                    moved.insert(c.id.clone());
                }
                Some(_) => {}
                None => {
                    gone.insert(c.id.clone());
                }
            }
        }
        track.clips.retain(|c| !gone.contains(&c.id));
    }
    settle(p, &moved);
    for ti in 0..p.tracks.len() {
        if ti != main_index {
            fix_audio(p, ti);
        }
    }
}

/// Moves clips of `moved` that landed on another clip of their track to the
/// first unlocked track of their kind above (audio: below) with room.
pub(crate) fn settle(p: &mut Project, moved: &HashSet<String>) {
    for kind in [TrackKind::Video, TrackKind::Audio] {
        let mut k = 0;
        while k < p.tracks_of(kind).len() {
            let ti = p.tracks_of(kind)[k];
            if p.tracks[ti].id == MAIN {
                k += 1;
                continue;
            }
            let mut clips = std::mem::take(&mut p.tracks[ti].clips);
            clips.sort_by_key(|c| c.start);
            // Clips that didn't move keep their place; moved ones fit in
            // around them or go up.
            let (mut keep, mut rest): (Vec<Clip>, Vec<Clip>) =
                clips.into_iter().partition(|c| !moved.contains(&c.id));
            let mut bumped = Vec::new();
            rest.sort_by_key(|c| c.start);
            for c in rest {
                if keep.iter().any(|o| o.start < c.end() && c.start < o.end()) {
                    bumped.push(c);
                } else {
                    let at = keep.partition_point(|o| o.start < c.start);
                    keep.insert(at, c);
                }
            }
            p.tracks[ti].clips = keep;
            if !bumped.is_empty() {
                // The next unlocked track of this kind (made if there is none).
                let mut n = k + 1;
                loop {
                    let to = tracks::nth(p, kind, n);
                    if !p.tracks[to].locked {
                        p.tracks[to].clips.extend(bumped);
                        p.tracks[to].sort();
                        break;
                    }
                    n += 1;
                }
            }
            k += 1;
        }
    }
}
