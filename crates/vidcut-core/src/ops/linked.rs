use std::collections::{HashMap, HashSet};

use super::{
    Source, clear_range, ensure_track, locate, main_index, place, split_at, tracks, unlocked,
};
use crate::error::Result;
use crate::model::{Clip, ClipAudio, ClipCache, ClipRetime, Compound, Project, TrackKind};

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
    used: HashSet<u32>,
}

impl Relink {
    pub fn new(p: &Project) -> Self {
        Self {
            next: 1,
            given: HashMap::new(),
            used: p
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter_map(|c| c.link)
                .collect(),
        }
    }

    pub fn of(&mut self, old: i64) -> u32 {
        if let Some(&l) = self.given.get(&old) {
            return l;
        }
        let l = match u32::try_from(old) {
            Ok(o) if o > 0 && !self.used.contains(&o) => o,
            _ => {
                while self.used.contains(&self.next) {
                    self.next += 1;
                }
                self.next
            }
        };
        self.used.insert(l);
        self.given.insert(old, l);
        l
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

pub fn sounds(p: &Project, ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        let heard: Vec<String> = match p.clip(id) {
            Some(c) if c.audio.is_none() => p
                .tied(id)
                .into_iter()
                .filter(|x| p.clip(x).is_some_and(|c| c.audio.is_some()))
                .collect(),
            _ => Vec::new(),
        };
        let list = if heard.is_empty() {
            vec![id.clone()]
        } else {
            heard
        };
        for x in list {
            if !out.contains(&x) {
                out.push(x);
            }
        }
    }
    out
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
            return;
        }
        n += 1;
    }
}

fn sound_of(c: &Clip, stream: Option<u32>) -> Clip {
    Clip {
        video: false,
        audio: stream.map(ClipAudio::of),
        picture: None,
        effects: None,
        retime: ClipRetime::Project,
        render_cache: ClipCache::Auto,
        ..c.clone()
    }
}

fn sound_parts(p: &Project, c: &Clip, src: &Source) -> Vec<Clip> {
    if src.video_only {
        return Vec::new();
    }
    if c.compound.is_some() {
        let heard = c.video && p.clip_compound(c).is_some_and(Compound::has_sound);
        return heard.then(|| sound_of(c, None)).into_iter().collect();
    }
    let Some(m) = p.media(&c.media) else {
        return Vec::new();
    };
    let own = c.audio.as_ref().map(|a| a.stream);
    (0..m.info.audio.len() as u32)
        .filter(|&k| Some(k) != own)
        .map(|k| sound_of(c, Some(k)))
        .collect()
}

pub(crate) fn with_sound(p: &mut Project, ids: Vec<String>, src: &Source) -> Result<Vec<String>> {
    let mut out = ids.clone();
    for id in &ids {
        let (mut ti, ci) = locate(p, id)?;
        let n = p.tracks[ti].number();
        if p.tracks[ti].kind == TrackKind::Video && !p.tracks[ti].clips[ci].video {
            let c = p.tracks[ti].clips.remove(ci);
            ti = ensure_track(p, &format!("A{n}"))?;
            unlocked(p, ti)?;
            clear_range(p, ti, c.start, c.end());
            place(p, ti, c);
        }
        let (ti, ci) = locate(p, id)?;
        let c = &p.tracks[ti].clips[ci];
        let parts = sound_parts(p, c, src);
        if parts.is_empty() {
            continue;
        }
        let first = match c.video {
            true => n,
            false => p.tracks[ti].number() + 1,
        };
        let link = c.link.unwrap_or_else(|| p.free_link());
        p.tracks[ti].clips[ci].link = Some(link);
        for (k, mut s) in parts.into_iter().enumerate() {
            let ai = ensure_track(p, &format!("A{}", first + k))?;
            unlocked(p, ai)?;
            s.id = p.alloc_clip_id();
            s.link = Some(link);
            clear_range(p, ai, s.start, s.end());
            out.push(s.id.clone());
            place(p, ai, s);
        }
    }
    Ok(out)
}
