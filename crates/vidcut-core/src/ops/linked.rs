use std::collections::{HashMap, HashSet};

use super::{clear_range, fix_audio, locate, main_index, place, split_at, tracks};
use crate::error::Result;
use crate::model::{Clip, Project};

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

pub(crate) struct Relink {
    next: u32,
    given: HashMap<i64, u32>,
}

impl Relink {
    pub fn new(p: &Project) -> Self {
        Self {
            next: p.free_link(),
            given: HashMap::new(),
        }
    }

    pub fn of(&mut self, old: i64) -> u32 {
        let next = &mut self.next;
        *self.given.entry(old).or_insert_with(|| {
            *next += 1;
            *next - 1
        })
    }

    pub fn clip(&mut self, c: &mut Clip) {
        c.link = c.link.map(|l| self.of(i64::from(l)));
    }
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
        let others = p.tied(id);
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

pub(crate) fn link_blocks(p: &mut Project, ids: &[String]) {
    let loose = |p: &Project, picture: bool| -> Vec<(usize, usize)> {
        ids.iter()
            .filter_map(|id| p.find_clip(id))
            .filter(|&(ti, ci)| {
                let c = &p.tracks[ti].clips[ci];
                c.link.is_none() && c.compound.is_some() && c.video == picture
            })
            .collect()
    };
    let sounds = loose(p, false);
    let mut taken = HashSet::new();
    for (ti, ci) in loose(p, true) {
        let c = &p.tracks[ti].clips[ci];
        let Some(&(si, sj)) = sounds.iter().find(|&&(si, sj)| {
            let s = &p.tracks[si].clips[sj];
            !taken.contains(&(si, sj))
                && (s.start, s.duration, s.source_in) == (c.start, c.duration, c.source_in)
                && s.pace() == c.pace()
                && s.compound == c.compound
        }) else {
            continue;
        };
        taken.insert((si, sj));
        let link = p.free_link();
        p.tracks[ti].clips[ci].link = Some(link);
        p.tracks[si].clips[sj].link = Some(link);
    }
}

pub fn leads(p: &Project, ids: &[String]) -> Vec<String> {
    parts(p, ids).leads
}

pub fn with_ties(p: &Project, ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut all = Vec::new();
    for id in ids {
        for x in std::iter::once(id.clone()).chain(p.tied(id)) {
            if seen.insert(x.clone()) {
                all.push(x);
            }
        }
    }
    all
}

pub(super) fn tied_to(p: &Project, id: &str) -> Vec<(String, String)> {
    p.tied(id)
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
    let others = p.tied(&c.id);
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
