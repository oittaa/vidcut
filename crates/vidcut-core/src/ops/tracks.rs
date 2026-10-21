use crate::error::{Error, Result};
use crate::model::{Project, Track, TrackKind};

use super::MAIN;

fn prefix(kind: TrackKind) -> char {
    match kind {
        TrackKind::Video => 'V',
        TrackKind::Audio => 'A',
    }
}

pub fn add_track(p: &mut Project, kind: TrackKind) -> String {
    let n = p.tracks_of(kind).len() + 1;
    let id = format!("{}{n}", prefix(kind));
    let track = Track::new(id.clone(), kind);
    match kind {
        TrackKind::Video => {
            let at = p.tracks_of(TrackKind::Video).last().map_or(0, |i| i + 1);
            p.tracks.insert(at, track);
        }
        TrackKind::Audio => p.tracks.push(track),
    }
    id
}

pub(crate) fn nth(p: &mut Project, kind: TrackKind, n: usize) -> usize {
    while p.tracks_of(kind).len() <= n {
        add_track(p, kind);
    }
    p.tracks_of(kind)[n]
}

pub(crate) fn resolve(p: &mut Project, id: &str) -> Result<usize> {
    if let Some(i) = p.track_index(id) {
        return Ok(i);
    }
    let kind = match id.chars().next() {
        Some('V') => TrackKind::Video,
        Some('A') => TrackKind::Audio,
        _ => return Err(Error::not_found(format!("no track {id}"))),
    };
    let n: usize = id[1..]
        .parse()
        .map_err(|_| Error::not_found(format!("no track {id}")))?;
    if n == p.tracks_of(kind).len() + 1 {
        add_track(p, kind);
        return Ok(p.track_index(id).expect("just added"));
    }
    Err(Error::not_found(format!(
        "no track {id} (the next new {} track is {}{})",
        match kind {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        },
        prefix(kind),
        p.tracks_of(kind).len() + 1
    )))
}

fn renumber(p: &mut Project) {
    for kind in [TrackKind::Video, TrackKind::Audio] {
        for (k, i) in p.tracks_of(kind).into_iter().enumerate() {
            p.tracks[i].id = format!("{}{}", prefix(kind), k + 1);
        }
    }
}

pub fn remove_track(p: &mut Project, id: &str, force: bool) -> Result<()> {
    if id == MAIN {
        return Err(Error::invalid("Track 1 (V1) can't be removed"));
    }
    let ti = p
        .track_index(id)
        .ok_or_else(|| Error::not_found(format!("no track {id}")))?;
    let t = &p.tracks[ti];
    if t.locked {
        return Err(Error::invalid(format!("track {id} is locked")));
    }
    if !t.clips.is_empty() && !force {
        return Err(Error::invalid(format!(
            "{id} has {} clip(s); pass force to delete them too",
            t.clips.len()
        )));
    }
    p.tracks.remove(ti);
    renumber(p);
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackProps {
    pub locked: Option<bool>,
    pub muted: Option<bool>,
    pub solo: Option<bool>,
    pub volume_db: Option<f64>,
    pub voice_isolation: Option<Option<f64>>,
    pub video_enabled: Option<bool>,
    pub name: Option<Option<String>>,
}

pub fn set_track(p: &mut Project, id: &str, props: &TrackProps) -> Result<()> {
    if let Some(v) = props.volume_db
        && !(-60.0..=24.0).contains(&v)
    {
        return Err(Error::invalid(format!("volume {v} dB outside -60..24 dB")));
    }
    let t = p
        .track_mut(id)
        .ok_or_else(|| Error::not_found(format!("no track {id}")))?;
    if props.video_enabled.is_some() && t.kind != TrackKind::Video {
        return Err(Error::invalid(format!("{id} is an audio track")));
    }
    if let Some(l) = props.locked {
        t.locked = l;
    }
    if let Some(m) = props.muted {
        t.muted = m;
    }
    if let Some(s) = props.solo {
        t.solo = s;
    }
    if let Some(v) = props.volume_db {
        t.volume_db = v;
    }
    if let Some(v) = props.voice_isolation {
        t.voice_isolation = v;
    }
    if let Some(v) = props.video_enabled {
        t.video_enabled = v;
    }
    if let Some(n) = &props.name {
        t.name = n.clone();
    }
    Ok(())
}
