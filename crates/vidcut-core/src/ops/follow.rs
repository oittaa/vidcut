use std::collections::{HashMap, HashSet};

use crate::model::{Clip, Project, TrackKind};

use super::{MAIN, fix_audio, tracks};

pub(crate) struct Before {
    main: Vec<Clip>,
    ids: HashSet<String>,
}

impl Before {
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

#[derive(Default)]
pub(crate) struct Edit<'a> {
    pub created: &'a [String],
    pub placed: &'a [String],
    pub kept: &'a [(i64, i64)],
    pub trimmed: Option<&'a str>,
}

struct Mapper<'a> {
    before: &'a Before,
    all: HashMap<&'a str, &'a Clip>,
    main: HashMap<&'a str, &'a Clip>,
    parts: Vec<&'a Clip>,
    edit: &'a Edit<'a>,
}

impl Mapper<'_> {
    fn successor(&self, c: &Clip, u: i64) -> Option<&Clip> {
        let shows = |n: &Clip| {
            n.media == c.media
                && n.generator == c.generator
                && n.compound == c.compound
                && c.in_step(n)
                && n.content_at(n.start) <= u
                && u < n.content_at(n.end())
        };
        if let Some(n) = self.all.get(c.id.as_str())
            && shows(n)
        {
            return Some(n);
        }
        self.parts.iter().copied().find(|n| {
            shows(n)
                && n.content_at(n.start) >= c.content_at(c.start)
                && n.content_at(n.end()) <= c.content_at(c.end())
        })
    }

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

    fn map(&self, x: i64, own: &str) -> Option<i64> {
        if self.edit.kept.iter().any(|&(a, b)| a <= x && x < b) {
            return Some(x);
        }
        let main = &self.before.main;
        let i = main.partition_point(|c| c.start <= x);
        if i > 0 && x < main[i - 1].end() {
            let c = &main[i - 1];
            if c.id == own {
                return Some(
                    main[..i - 1]
                        .iter()
                        .rev()
                        .find_map(|q| self.main.get(q.id.as_str()).map(|now| (q, now)))
                        .map_or(x, |(q, now)| now.end() + (x - q.end())),
                );
            }
            let u = c.content_at(x);
            if let Some(n) = self.successor(c, u) {
                return Some(n.start + (u - n.content_at(n.start)));
            }
            if let Some(n) = self.all.get(c.id.as_str()) {
                if u < n.content_at(n.start) {
                    return Some(n.start);
                }
                if self.edit.trimmed == Some(c.id.as_str()) {
                    return None;
                }
            }
            return Some(self.closed(i - 1).unwrap_or(x));
        }
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
