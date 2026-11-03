use std::collections::HashSet;

use super::{clear_range, fix_audio, locate, main_index, place, split_at, tracks};
use crate::error::Result;
use crate::model::{Clip, Project};
use crate::otio::tied;

#[derive(Debug, Default)]
pub(super) struct Parts {
    pub leads: Vec<String>,
    pub rest: Vec<(String, String)>,
}

pub(super) struct Taken {
    pub track: String,
    pub clip: Clip,
    pub lead: String,
}

pub(super) fn parts(p: &Project, ids: &[String]) -> Parts {
    let main = main_index(p);
    let on_main = |id: &&String| p.find_clip(id).is_some_and(|(ti, _)| ti == main);
    let mut seen = HashSet::new();
    let mut parts = Parts::default();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        let others = tied(p, id);
        let lead = others.iter().find(on_main).unwrap_or(id).clone();
        for x in std::iter::once(id).chain(&others) {
            seen.insert(x.clone());
            if *x != lead {
                parts.rest.push((x.clone(), lead.clone()));
            }
        }
        parts.leads.push(lead);
    }
    parts
}

pub fn leads(p: &Project, ids: &[String]) -> Vec<String> {
    parts(p, ids).leads
}

pub fn with_ties(p: &Project, ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut all = Vec::new();
    for id in ids {
        for x in std::iter::once(id.clone()).chain(tied(p, id)) {
            if seen.insert(x.clone()) {
                all.push(x);
            }
        }
    }
    all
}

pub(super) fn tied_to(p: &Project, id: &str) -> Vec<(String, String)> {
    tied(p, id)
        .into_iter()
        .map(|x| (x, id.to_string()))
        .collect()
}

pub(super) fn take(p: &mut Project, rest: &[(String, String)]) -> Result<Vec<Taken>> {
    let mut taken = Vec::new();
    for (id, lead) in rest {
        let (ti, ci) = locate(p, id)?;
        taken.push(Taken {
            track: p.tracks[ti].id.clone(),
            clip: p.tracks[ti].clips.remove(ci),
            lead: lead.clone(),
        });
    }
    Ok(taken)
}

pub(super) fn split_main(p: &mut Project, t: i64, moving: &[String]) {
    let main = main_index(p);
    let Some(c) = p.tracks[main]
        .clips
        .iter()
        .find(|c| c.start < t && t < c.end() && !moving.contains(&c.id))
    else {
        return;
    };
    let others = tied(p, &c.id);
    if others.is_empty() {
        return;
    }
    for o in others {
        if let Some((ti, _)) = p.find_clip(&o)
            && !p.tracks[ti].locked
        {
            split_at(p, ti, t);
        }
    }
    split_at(p, main, t);
}

pub(super) fn put(p: &mut Project, track: &str, c: Clip) {
    let ti = p.track_index(track).expect("tracks stay");
    clear_range(p, ti, c.start, c.end());
    place(p, ti, c);
    fix_audio(p, ti);
}

pub(super) fn put_free(p: &mut Project, track: &str, c: Clip) {
    let own = p.track_index(track).expect("tracks stay");
    let kind = p.tracks[own].kind;
    let mut n = p
        .tracks_of(kind)
        .iter()
        .position(|&ti| ti == own)
        .unwrap_or(0);
    loop {
        let ti = tracks::nth(p, kind, n);
        let t = &p.tracks[ti];
        if !t.locked
            && !t
                .clips
                .iter()
                .any(|o| o.start < c.end() && c.start < o.end())
        {
            place(p, ti, c);
            fix_audio(p, ti);
            return;
        }
        n += 1;
    }
}
