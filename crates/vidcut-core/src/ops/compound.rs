use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::follow::{self, Before, Edit};
use super::{
    Place, block_clip, clear_range, ensure_track, fix_audio, linked, locate, main_index,
    shift_from, split_at, tracks, unlocked,
};
use crate::error::{Error, Result};
use crate::model::{
    Clip, Compound, CompoundTarget, Kept, Marker, Marks, OpenCompound, PooledCompound, Project,
    Settings, Track, TrackKind,
};
use crate::otio;
use crate::time::{Rational, TimeSpec, format_timecode};

#[derive(Debug, Default)]
pub struct Nested {
    pub clips: Vec<String>,
    pub warnings: Vec<String>,
}

fn holds(t: &Track, ids: &HashSet<String>) -> bool {
    t.clips.iter().any(|c| ids.contains(&c.id))
}

fn extent(c: &Clip) -> (i64, i64) {
    c.audio_range().map_or((c.start, c.end()), |(a, b)| {
        (a.min(c.start), b.max(c.end()))
    })
}

fn span_of(p: &Project, ids: &HashSet<String>) -> (i64, i64) {
    p.tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| ids.contains(&c.id))
        .map(extent)
        .fold((i64::MAX, i64::MIN), |(lo, hi), (a, b)| {
            (lo.min(a), hi.max(b))
        })
}

fn picture_of(c: &Clip) -> (i64, i64) {
    (c.start, c.end())
}

fn free(t: &Track, (a, b): (i64, i64), range: fn(&Clip) -> (i64, i64)) -> bool {
    !t.locked
        && t.clips.iter().all(|c| {
            let (s, e) = range(c);
            e <= a || s >= b
        })
}

fn lift(p: &mut Project, ids: &HashSet<String>, kind: TrackKind, first: i64) -> Vec<Track> {
    let order = p.tracks_of(kind);
    let selected = |&i: &usize| holds(&p.tracks[i], ids);
    let (Some(lowest), Some(highest)) = (
        order.iter().position(selected),
        order.iter().rposition(selected),
    ) else {
        return Vec::new();
    };
    let mut lifted = Vec::new();
    for (n, &i) in order[lowest..=highest].iter().enumerate() {
        let track = &mut p.tracks[i];
        let id = format!("{}{}", tracks::prefix(kind), n + 1);
        let mut inner = Track::new(id, kind);
        let (moved, kept): (Vec<Clip>, Vec<Clip>) = std::mem::take(&mut track.clips)
            .into_iter()
            .partition(|c| ids.contains(&c.id));
        track.clips = kept;
        inner.clips = moved
            .into_iter()
            .map(|mut c| {
                c.start -= first;
                c
            })
            .collect();
        let within = |side: &Option<String>| side.as_ref().is_none_or(|id| ids.contains(id));
        let (inside, outside) = std::mem::take(&mut track.transitions)
            .into_iter()
            .partition(|x| within(&x.from) && within(&x.to));
        inner.transitions = inside;
        track.transitions = outside;
        lifted.push(inner);
    }
    lifted
}

fn picture_tracks(whole: &[Track]) -> Vec<Track> {
    whole
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .filter_map(|t| {
            let clips: Vec<Clip> = t
                .clips
                .iter()
                .filter(|c| c.video)
                .cloned()
                .map(|mut c| {
                    c.audio = None;
                    c
                })
                .collect();
            (!clips.is_empty()).then(|| Track {
                clips,
                transitions: t.transitions.iter().filter(|x| !x.sound).cloned().collect(),
                locked: t.locked,
                video_enabled: t.video_enabled,
                render_cache: t.render_cache,
                name: t.name.clone(),
                ..Track::new(t.id.clone(), TrackKind::Video)
            })
        })
        .collect()
}

fn sound_part(c: &Clip) -> Option<Clip> {
    let mut part = c.clone();
    let Some(a) = part.audio.as_mut() else {
        return c.is_sound_block().then_some(part);
    };
    let (head, tail) = (a.head, a.tail);
    a.head = 0;
    a.tail = 0;
    part.video = false;
    part.picture = None;
    part.effects = None;
    part.motion.clear();
    part.move_head(head);
    part.duration += tail;
    Some(part)
}

fn sound_tracks(whole: &[Track]) -> Vec<Track> {
    let mut sound = Vec::new();
    let mut under = 0;
    for t in whole {
        let clips: Vec<Clip> = t.clips.iter().filter_map(sound_part).collect();
        if clips.is_empty() {
            continue;
        }
        let n = match t.kind {
            TrackKind::Video => {
                under += 1;
                under
            }
            TrackKind::Audio => under + number(t),
        };
        sound.push(Track {
            clips,
            transitions: t.transitions.iter().filter(|x| x.sound).cloned().collect(),
            locked: t.locked,
            muted: t.muted,
            solo: t.solo,
            volume_db: t.volume_db,
            voice_isolation: t.voice_isolation,
            name: (t.kind == TrackKind::Audio)
                .then(|| t.name.clone())
                .flatten(),
            ..Track::new(format!("A{n}"), TrackKind::Audio)
        });
    }
    sound
}

fn number(t: &Track) -> usize {
    t.id[1..].parse().unwrap_or(1)
}

fn end_of(whole: &[Track]) -> i64 {
    let clips = whole
        .iter()
        .flat_map(|t| &t.clips)
        .map(|c| c.audio_range().map_or(c.end(), |(_, e)| e.max(c.end())));
    let fades = whole.iter().flat_map(|t| {
        t.transitions
            .iter()
            .filter(|x| x.to.is_none())
            .filter_map(|x| {
                let c = t.clips.iter().find(|c| Some(&c.id) == x.from.as_ref())?;
                let edge = if x.sound { c.audio_range()?.1 } else { c.end() };
                Some(edge + x.after)
            })
    });
    clips.chain(fades).max().unwrap_or(0)
}

fn stack_of(p: &mut Project, whole: &[Track], name: &str, frames: i64) -> Result<Kept> {
    stack_of_with_id(p, whole, name, frames, p.fps(), None)
}

fn stack_of_with_id(
    p: &mut Project,
    whole: &[Track],
    name: &str,
    frames: i64,
    fps: Rational,
    seq_id: Option<&str>,
) -> Result<Kept> {
    let mut inner = Project::new(Settings {
        fps,
        ..p.flat_settings()
    });
    inner.tracks = whole.to_vec();
    inner.media = std::mem::take(&mut p.media);
    let stack = otio::made_stack(&inner, name, frames, seq_id);
    p.media = inner.media;
    stack.map(Kept::from)
}

fn picture_target(p: &mut Project, lowest: usize, span: (i64, i64)) -> String {
    if free(&p.tracks[lowest], span, picture_of) {
        return p.tracks[lowest].id.clone();
    }
    tracks::insert_at(p, TrackKind::Video, lowest + 1)
}

fn sound_target(p: &mut Project, lowest: Option<String>, span: (i64, i64)) -> String {
    let order = p.tracks_of(TrackKind::Audio);
    let from = lowest
        .and_then(|id| order.iter().position(|&i| p.tracks[i].id == id))
        .unwrap_or(0);
    match order[from..]
        .iter()
        .find(|&&i| free(&p.tracks[i], span, extent))
    {
        Some(&i) => p.tracks[i].id.clone(),
        None => tracks::add_track(p, TrackKind::Audio),
    }
}

fn given_name(p: &Project, name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            let made = p
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter(|c| c.video && c.compound.is_some())
                .count();
            format!("Compound Clip {}", made.max(p.compounds.len()) + 1)
        }
    }
}

pub fn nest(
    p: &mut Project,
    ids: &[String],
    name: Option<&str>,
    bin: &str,
    start_tc: i64,
) -> Result<Nested> {
    checked_bin(p, bin)?;
    let ids: HashSet<String> = linked::with_ties(p, ids).into_iter().collect();
    if ids.is_empty() {
        return Err(Error::usage("name the clips to make a compound clip of"));
    }
    for ti in 0..p.tracks.len() {
        if holds(&p.tracks[ti], &ids) {
            unlocked(p, ti)?;
        }
    }
    let span = span_of(p, &ids);
    let nested = build(p, &ids, span, name, start_tc)?;
    keep_copy(p, &nested, bin);
    Ok(nested)
}

fn checked_bin(p: &Project, bin: &str) -> Result<()> {
    if bin.is_empty() || p.bins.iter().any(|b| b == bin) {
        return Ok(());
    }
    Err(Error::not_found(format!("no bin {bin}")))
}

fn keep_copy(p: &mut Project, nested: &Nested, bin: &str) {
    let hosts: Vec<Clip> = nested
        .clips
        .iter()
        .filter_map(|id| p.clip(id).cloned())
        .collect();
    let part = |picture: bool| {
        hosts
            .iter()
            .find(|c| c.video == picture)
            .and_then(|c| c.compound.clone())
    };
    let (picture, sound) = (part(true), part(false));
    let name = hosts
        .iter()
        .find_map(|c| c.name.clone())
        .unwrap_or_default();
    let id = p.alloc_pooled_compound_id();
    let fps = p.fps();
    p.compounds.push(PooledCompound {
        id,
        name,
        bin: bin.to_string(),
        fps,
        picture,
        sound,
    });
}

pub fn pool_range(
    p: &mut Project,
    span: (i64, i64),
    name: Option<&str>,
    bin: &str,
    start_tc: i64,
) -> Result<(String, Vec<String>)> {
    checked_bin(p, bin)?;
    let mut copy = p.clone();
    let nested = nest_range(&mut copy, span, name, bin, start_tc)?;
    let made = copy.compounds.pop().expect("a nest keeps a copy");
    let id = p.alloc_pooled_compound_id();
    p.compounds.push(PooledCompound {
        id: id.clone(),
        ..made
    });
    Ok((id, nested.warnings))
}

pub fn remove_pooled(p: &mut Project, id: &str) -> Result<()> {
    if !p.compounds.iter().any(|x| x.id == id) {
        return Err(Error::not_found(format!("no compound clip {id}")));
    }
    if p.compound_path
        .first()
        .is_some_and(|c| matches!(&c.target, CompoundTarget::Pooled { compound } if compound == id))
    {
        close_all(p)?;
    }
    p.compounds.retain(|x| x.id != id);
    Ok(())
}

struct Host<'a> {
    number: &'a str,
    name: &'a str,
    span: (i64, i64),
}

fn put_host(
    p: &mut Project,
    host: &Host,
    (kind, part): (TrackKind, Compound),
    clips: &mut Vec<String>,
) -> Result<()> {
    let ti = ensure_track(p, &format!("{}{}", tracks::prefix(kind), host.number))?;
    unlocked(p, ti)?;
    let (a, b) = host.span;
    clear_range(p, ti, a, b);
    fix_audio(p, ti);
    let mut c = block_clip(p.alloc_clip_id(), (a, b - a), None, None);
    c.video = kind == TrackKind::Video;
    c.name = Some(host.name.to_string());
    c.compound = Some(part);
    clips.push(c.id.clone());
    p.tracks[ti].clips.push(c);
    p.tracks[ti].sort();
    Ok(())
}

fn make_room(
    p: &mut Project,
    how: Place,
    at: i64,
    frames: i64,
) -> Result<(i64, Option<(i64, i64)>)> {
    let ti = main_index(p);
    unlocked(p, ti)?;
    if how == Place::Insert {
        let at = if p.settings.ripple_on {
            at.min(p.tracks[ti].end())
        } else {
            at
        };
        split_at(p, ti, at);
        shift_from(p, ti, at, frames);
        return Ok((at, None));
    }
    let t = &p.tracks[ti];
    let ci = t
        .clip_at(at)
        .ok_or_else(|| Error::invalid(format!("no clip on {} to replace", t.id)))?;
    let (a, b) = (t.clips[ci].start, t.clips[ci].end());
    let others = linked::tied_to(p, &t.clips[ci].id);
    for (id, _) in &others {
        unlocked(p, locate(p, id)?.0)?;
    }
    linked::take(p, &others)?;
    clear_range(p, ti, a, b);
    shift_from(p, ti, b, frames - (b - a));
    Ok((a, Some((a, b))))
}

pub fn place_pooled(
    p: &mut Project,
    id: &str,
    at: Option<i64>,
    track: Option<&str>,
    how: Place,
) -> Result<Nested> {
    let mut item = p
        .compounds
        .iter()
        .find(|x| x.id == id)
        .cloned()
        .ok_or_else(|| Error::not_found(format!("no compound clip {id}")))?;
    if p.compound_path
        .iter()
        .any(|c| matches!(&c.target, CompoundTarget::Pooled { compound } if compound == &item.id))
    {
        return Err(Error::invalid("a compound clip cannot hold itself"));
    }
    let open_seq = open_sequence_ids(p);
    if !open_seq.is_empty() && item.blocks().any(|b| holds_any(b, &open_seq)) {
        return Err(Error::invalid("a compound clip cannot hold itself"));
    }
    if item.frames() < 1 {
        return Err(Error::invalid("compound clip is empty"));
    }
    if let Some(r) = super::conform::Rates::new(item.fps, p.fps()) {
        super::conform::pooled(p, &mut item, r);
    }
    if let Some(t) = track {
        ensure_track(p, t)?;
    }
    let frames = item.frames();
    let mut at = at.unwrap_or_else(|| p.end());
    let number = track.and_then(|t| t.get(1..)).unwrap_or("1");
    let ripples = number == "1" && how != Place::Overwrite;
    if ripples && how == Place::Insert {
        linked::split_main(p, at, &[]);
    }
    let before = Before::of(p);
    let mut old = None;
    if ripples {
        (at, old) = make_room(p, how, at, frames)?;
    }
    let host = Host {
        number,
        name: &item.name,
        span: (at, at + frames),
    };
    let mut clips = Vec::new();
    if let Some(k) = item.picture.clone() {
        put_host(p, &host, (TrackKind::Video, k), &mut clips)?;
    }
    if ripples {
        let kept: Vec<(i64, i64)> = old
            .iter()
            .map(|&(a, b)| (a, a + frames.min(b - a)))
            .collect();
        follow::follow(
            p,
            &before,
            &Edit {
                created: &clips,
                kept: &kept,
                replaced: old.map(|(a, b)| (a + frames, b)),
                ..Edit::default()
            },
        );
    }
    if let Some(k) = item.sound.clone() {
        put_host(p, &host, (TrackKind::Audio, k), &mut clips)?;
    }
    let mut warnings = Vec::new();
    let dropped = otio::transitions::prune(p);
    if dropped > 0 {
        warnings.push(otio::transitions::dropped(dropped));
    }
    Ok(Nested { clips, warnings })
}

fn end_sound_at(t: &mut Track, at: i64) {
    let crosses = |c: &Clip| c.audio_range().is_some_and(|(s, e)| s < at && at < e);
    if !t.clips.iter().any(crosses) {
        return;
    }
    for c in &mut t.clips {
        let (start, end) = (c.start, c.end());
        if let Some(a) = c.audio.as_mut() {
            if end == at {
                a.tail = 0;
            }
            if start == at {
                a.head = 0;
            }
        }
    }
}

pub fn nest_range(
    p: &mut Project,
    (a, b): (i64, i64),
    name: Option<&str>,
    bin: &str,
    start_tc: i64,
) -> Result<Nested> {
    checked_bin(p, bin)?;
    if a >= b {
        return Err(Error::usage(format!("empty range {a}..{b}")));
    }
    for ti in 0..p.tracks.len() {
        let reached = p.tracks[ti].clips.iter().any(|c| {
            let (s, e) = extent(c);
            s < b && e > a
        });
        if reached {
            unlocked(p, ti)?;
        }
    }
    for ti in 0..p.tracks.len() {
        for at in [a, b] {
            split_at(p, ti, at);
            end_sound_at(&mut p.tracks[ti], at);
        }
        fix_audio(p, ti);
    }
    let ids: HashSet<String> = p
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| c.start >= a && c.end() <= b)
        .map(|c| c.id.clone())
        .collect();
    if ids.is_empty() {
        return Err(Error::invalid("no clips between In and Out"));
    }
    let nested = build(p, &ids, (a, b), name, start_tc)?;
    keep_copy(p, &nested, bin);
    Ok(nested)
}

fn build(
    p: &mut Project,
    ids: &HashSet<String>,
    span: (i64, i64),
    name: Option<&str>,
    start_tc: i64,
) -> Result<Nested> {
    let frames = span.1 - span.0;
    let name = given_name(p, name);
    let lowest_picture = p
        .tracks_of(TrackKind::Video)
        .into_iter()
        .find(|&i| holds(&p.tracks[i], ids));
    let lowest_sound = p
        .tracks_of(TrackKind::Audio)
        .into_iter()
        .find(|&i| holds(&p.tracks[i], ids))
        .map(|i| p.tracks[i].id.clone());
    let mut whole = lift(p, ids, TrackKind::Video, span.0);
    whole.extend(lift(p, ids, TrackKind::Audio, span.0));
    let stack = stack_of(p, &whole, &name, frames)?;
    let (picture, sound) = (picture_tracks(&whole), sound_tracks(&whole));
    let mut clips = Vec::new();
    let mut put = |p: &mut Project, track: String, video: bool, tracks: Vec<Track>| {
        let mut host = block_clip(p.alloc_clip_id(), (span.0, frames), None, None);
        host.video = video;
        host.name = Some(name.clone());
        host.compound = Some(Compound {
            stack: stack.clone(),
            frames,
            tracks,
            made_from: whole.clone(),
            markers: Vec::new(),
            start_tc,
        });
        clips.push(host.id.clone());
        let t = p.track_mut(&track).expect("a track of the project");
        t.clips.push(host);
        t.sort();
    };
    if !picture.is_empty() {
        let lowest = lowest_picture.expect("a picture comes from a video track");
        let track = picture_target(p, lowest, span);
        put(p, track, true, picture);
    }
    if !sound.is_empty() {
        let track = sound_target(p, lowest_sound, span);
        put(p, track, false, sound);
    }
    let mut warnings = Vec::new();
    let dropped = otio::transitions::prune(p);
    if dropped > 0 {
        warnings.push(otio::transitions::dropped(dropped));
    }
    Ok(Nested { clips, warnings })
}

pub fn is_made(c: &Clip) -> bool {
    c.compound.as_ref().is_some_and(|k| !k.made_from.is_empty())
}

pub fn leads(p: &Project, ids: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for id in ids {
        let made = p.clip(id).is_some_and(is_made);
        if made && otio::tied(p, id).iter().any(|x| kept.contains(x)) {
            continue;
        }
        kept.push(id.clone());
    }
    kept
}

pub(crate) fn target(
    p: &mut Project,
    kind: TrackKind,
    base: &str,
    n: usize,
    span: (i64, i64),
) -> usize {
    let at = p
        .tracks_of(kind)
        .iter()
        .position(|&i| p.tracks[i].id == base)
        .expect("the track of the compound clip");
    let ti = tracks::nth(p, kind, at + n);
    let range = if kind == TrackKind::Video {
        picture_of
    } else {
        extent
    };
    if !free(&p.tracks[ti], span, range) {
        tracks::insert_at(p, kind, ti);
    }
    ti
}

fn put_back(p: &mut Project, ti: usize, inner: &Track, block: &Clip, made: &mut Vec<String>) {
    let mut renamed = HashMap::new();
    let cut = crate::render::stretches(block, TrackKind::Audio);
    let parts = if cut.is_empty() {
        std::slice::from_ref(block)
    } else {
        cut.as_slice()
    };
    for (x, part) in inner
        .clips
        .iter()
        .flat_map(|x| parts.iter().map(move |part| (x, part)))
    {
        let Some((mut y, _)) = crate::render::inside(x, part) else {
            continue;
        };
        if p.clip(&y.id).is_some() {
            let id = p.alloc_clip_id();
            renamed.insert(y.id.clone(), id.clone());
            y.id = id;
        }
        made.push(y.id.clone());
        p.tracks[ti].clips.push(y);
    }
    p.tracks[ti].sort();
    for x in &inner.transitions {
        let mut x = x.clone();
        for side in [&mut x.from, &mut x.to].into_iter().flatten() {
            if let Some(id) = renamed.get(side) {
                side.clone_from(id);
            }
        }
        if p.tracks
            .iter()
            .flat_map(|t| &t.transitions)
            .any(|t| t.id == x.id)
        {
            x.id = p.alloc_transition_id();
        }
        p.tracks[ti].transitions.push(x);
    }
}

fn fit_sound(p: &mut Project, ti: usize, made: &[String]) {
    let clips = &mut p.tracks[ti].clips;
    let stayed = |c: &&Clip| !made.contains(&c.id);
    for i in 0..clips.len() {
        if !made.contains(&clips[i].id) {
            continue;
        }
        let before = clips[..i]
            .iter()
            .rev()
            .filter(stayed)
            .find_map(Clip::audio_range);
        let after = clips[i + 1..]
            .iter()
            .filter(stayed)
            .find_map(Clip::audio_range);
        let (start, end) = (clips[i].start, clips[i].end());
        let Some(a) = clips[i].audio.as_mut() else {
            continue;
        };
        if let Some((_, reach)) = before {
            a.head = a.head.max(reach - start);
        }
        if let Some((reach, _)) = after {
            a.tail = a.tail.min(reach - end);
        }
        if start + a.head >= end + a.tail {
            clips[i].audio = None;
        }
    }
}

pub(crate) fn unnest(
    p: &mut Project,
    id: &str,
    warnings: &mut Vec<String>,
) -> Result<(Clip, Vec<String>)> {
    let parts: Vec<String> = std::iter::once(id.to_string())
        .chain(otio::tied(p, id))
        .collect();
    for x in &parts {
        locate(p, x)?;
    }
    let mut hosts = Vec::new();
    for x in &parts {
        let (ti, ci) = locate(p, x)?;
        hosts.push((p.tracks[ti].id.clone(), p.tracks[ti].clips.remove(ci)));
    }
    let lead = hosts.iter().find(|(_, c)| c.video).unwrap_or(&hosts[0]);
    let block = lead.1.clone();
    let k = block.compound.as_ref().expect("a compound clip");
    let source = if hosts.len() > 1 {
        &k.made_from
    } else {
        &k.tracks
    };
    let span = (block.start, block.end());
    let mut made = Vec::new();
    for inner in source.iter().filter(|t| !t.clips.is_empty()) {
        let video = inner.kind == TrackKind::Video;
        let Some((base, _)) = hosts.iter().find(|(_, c)| c.video == video) else {
            continue;
        };
        let ti = target(p, inner.kind, base, number(inner) - 1, span);
        put_back(p, ti, inner, &block, &mut made);
        fit_sound(p, ti, &made);
        fix_audio(p, ti);
    }
    let dropped = otio::transitions::prune(p);
    if dropped > 0 {
        warnings.push(otio::transitions::dropped(dropped));
    }
    Ok((block, made))
}

fn open_sequence_ids(p: &Project) -> HashSet<String> {
    p.compound_path
        .iter()
        .filter_map(|c| c.sequence_id.clone())
        .collect()
}

pub(crate) fn holds_any(k: &Compound, ids: &HashSet<String>) -> bool {
    if otio::sequence_id(k).is_some_and(|s| ids.contains(s)) || otio::stack_holds_any(&k.stack, ids)
    {
        return true;
    }
    k.tracks
        .iter()
        .chain(&k.made_from)
        .flat_map(|t| &t.clips)
        .filter_map(|c| c.compound.as_ref())
        .any(|inner| holds_any(inner, ids))
}

pub(crate) fn check_no_cycle(p: &Project, clips: &[crate::model::TrackClip]) -> Result<()> {
    let open_seq = open_sequence_ids(p);
    if open_seq.is_empty() {
        return Ok(());
    }
    if clips
        .iter()
        .filter_map(|tc| tc.clip.compound.as_ref())
        .any(|k| holds_any(k, &open_seq))
    {
        return Err(Error::invalid("a compound clip cannot hold itself"));
    }
    Ok(())
}

pub(crate) fn check_clips_no_cycle(p: &Project, clip_ids: &[String]) -> Result<()> {
    let open_seq = open_sequence_ids(p);
    if open_seq.is_empty() {
        return Ok(());
    }
    let hit = clip_ids
        .iter()
        .filter_map(|id| p.clip(id).and_then(|c| c.compound.as_ref()))
        .any(|k| holds_any(k, &open_seq));
    if hit {
        return Err(Error::invalid("a compound clip cannot hold itself"));
    }
    Ok(())
}

pub fn breadcrumbs(p: &Project) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(t) = p.timeline() {
        out.push(json!({ "kind": "timeline", "id": t.id, "name": t.name }));
    }
    for (i, c) in p.compound_path.iter().enumerate() {
        let mut entry = json!({ "kind": "compound", "depth": i, "name": c.name });
        match &c.target {
            CompoundTarget::Clip { clip } => entry["clip"] = json!(clip),
            CompoundTarget::Pooled { compound } => entry["compound"] = json!(compound),
        }
        out.push(entry);
    }
    out
}

pub fn path_json(p: &Project) -> Value {
    json!({
        "name": p.open_compound().map_or("", |c| c.name.as_str()),
        "fps": p.fps(),
        "start_tc": p.timecode(0),
        "path": breadcrumbs(p),
    })
}

fn otio_has_items(v: &Value) -> bool {
    let s = v["OTIO_SCHEMA"].as_str().unwrap_or("");
    if s.starts_with("Clip.") || otio::is_compound_stack(v) {
        return true;
    }
    v["children"]
        .as_array()
        .into_iter()
        .flatten()
        .any(otio_has_items)
}

fn extract_pooled_tracks(pool: &PooledCompound) -> (Vec<Track>, Vec<Marker>) {
    let primary = pool.picture.as_ref().or(pool.sound.as_ref());
    if let Some(k) = primary
        && !k.made_from.is_empty()
    {
        return (k.made_from.clone(), k.markers.clone());
    }
    let mut out: Vec<Track> = pool
        .picture
        .iter()
        .flat_map(|k| k.tracks.iter().cloned())
        .collect();
    out.extend(pool.sound.iter().flat_map(|k| k.tracks.iter().cloned()));
    let markers = primary.map_or_else(Vec::new, |k| k.markers.clone());
    (out, markers)
}

fn combined_tracks(lead: &Compound, partner: Option<&Compound>) -> Vec<Track> {
    let all = lead
        .tracks
        .iter()
        .chain(partner.into_iter().flat_map(|k| &k.tracks));
    let mut out: Vec<Track> = all
        .clone()
        .filter(|t| t.kind == TrackKind::Video)
        .cloned()
        .collect();
    out.extend(all.filter(|t| t.kind == TrackKind::Audio).cloned());
    out
}

fn collect_ids(
    p: &Project,
    clips: &mut HashSet<String>,
    transitions: &mut HashSet<String>,
    markers: &mut HashSet<String>,
) {
    let mut scan = |ts: &[Track]| {
        for t in ts {
            clips.extend(t.clips.iter().map(|c| c.id.clone()));
            transitions.extend(t.transitions.iter().map(|x| x.id.clone()));
        }
    };
    scan(&p.tracks);
    markers.extend(p.markers.iter().map(|k| k.id.clone()));
    markers.extend(
        p.media
            .iter()
            .flat_map(|m| m.markers.iter().map(|k| k.id.clone())),
    );
    for t in &p.timelines {
        scan(&t.tracks);
        markers.extend(t.markers.iter().map(|k| k.id.clone()));
    }
    for oc in &p.compound_path {
        scan(&oc.tracks);
        markers.extend(oc.markers.iter().map(|k| k.id.clone()));
    }
}

fn renumber_list(video: &mut [Track], audio: &mut [Track]) {
    for (n, t) in video.iter_mut().enumerate() {
        t.id = format!("V{}", n + 1);
    }
    for (n, t) in audio.iter_mut().enumerate() {
        t.id = format!("A{}", n + 1);
    }
}

fn prepare_open_tracks(
    p: &mut Project,
    raw_tracks: Vec<Track>,
    raw_markers: Vec<Marker>,
) -> (Vec<Track>, Vec<Marker>) {
    let (mut used_clips, mut used_transitions, mut used_markers) =
        (HashSet::new(), HashSet::new(), HashSet::new());
    collect_ids(p, &mut used_clips, &mut used_transitions, &mut used_markers);

    let (mut video, mut audio): (Vec<Track>, Vec<Track>) = raw_tracks
        .into_iter()
        .partition(|t| t.kind == TrackKind::Video);
    if video.is_empty() {
        video.push(Track::new("V1", TrackKind::Video));
    }
    renumber_list(&mut video, &mut audio);
    let mut out: Vec<Track> = video.into_iter().chain(audio).collect();

    let mut renamed_clips = HashMap::new();
    for t in &mut out {
        for c in &mut t.clips {
            if !used_clips.insert(c.id.clone()) {
                let id = p.alloc_clip_id();
                used_clips.insert(id.clone());
                renamed_clips.insert(std::mem::replace(&mut c.id, id.clone()), id);
            }
        }
        t.sort();
    }
    for t in &mut out {
        for c in &mut t.clips {
            if let Some(f) = c.title.as_mut().and_then(|x| x.follows.as_mut())
                && let Some(new) = renamed_clips.get(&f.clip)
            {
                f.clip.clone_from(new);
            }
        }
        for x in &mut t.transitions {
            for side in [&mut x.from, &mut x.to].into_iter().flatten() {
                if let Some(new) = renamed_clips.get(side) {
                    side.clone_from(new);
                }
            }
            if !used_transitions.insert(x.id.clone()) {
                x.id = p.alloc_transition_id();
                used_transitions.insert(x.id.clone());
            }
        }
    }
    let mut markers = raw_markers;
    for k in &mut markers {
        if !used_markers.insert(k.id.clone()) {
            k.id = p.alloc_marker_id();
            used_markers.insert(k.id.clone());
        }
    }
    (out, markers)
}

struct OpenData {
    name: String,
    target: CompoundTarget,
    fps: Rational,
    sequence_id: Option<String>,
    tracks: Vec<Track>,
    markers: Vec<Marker>,
    start_tc: i64,
}

fn push_open(p: &mut Project, data: OpenData) {
    let (tracks, markers) = prepare_open_tracks(p, data.tracks, data.markers);
    p.check_in();
    p.compound_path.push(OpenCompound {
        id: data.target.key(),
        name: data.name,
        target: data.target,
        fps: data.fps,
        sequence_id: data.sequence_id,
        tracks,
        marks: Marks::default(),
        markers,
        start_tc: data.start_tc,
    });
    p.check_out();
}

pub fn open_clip(
    p: &mut Project,
    id: &str,
    imported: Option<(Vec<Track>, Vec<Marker>)>,
) -> Result<Value> {
    let (ti, ci) = locate(p, id)?;
    if p.tracks[ti].clips[ci].compound.is_none() {
        return Err(Error::invalid(format!("{id} isn't a compound clip")));
    }
    let mut all_ids = vec![id.to_string()];
    all_ids.extend(otio::tied(p, id));
    let lead_id = all_ids
        .iter()
        .find(|cid| p.clip(cid).is_some_and(|c| c.video))
        .cloned()
        .unwrap_or_else(|| id.to_string());
    let lead = p.clip(&lead_id).expect("located").clone();
    let partner = all_ids
        .iter()
        .find(|cid| *cid != &lead_id)
        .and_then(|cid| p.clip(cid).and_then(|c| c.compound.clone()));
    let k = lead.compound.expect("checked");
    let seq_id = otio::sequence_id(&k).map(str::to_string);
    if seq_id
        .as_deref()
        .is_some_and(|sid| open_sequence_ids(p).contains(sid))
    {
        return Err(Error::invalid("that compound clip is already open"));
    }
    let name = lead
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            k.stack["name"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Compound Clip".into());
    let pooled_other_fps = seq_id.as_deref().and_then(|sid| {
        p.compounds
            .iter()
            .find(|x| x.sequence_id() == Some(sid) && x.fps != p.fps())
            .cloned()
    });
    let (open_fps, start_tc, raw_tracks, raw_markers) = if let Some(pool) = pooled_other_fps {
        let tc = pool.start_tc();
        let (ts, ms) = extract_pooled_tracks(&pool);
        (pool.fps, tc, ts, ms)
    } else if !k.made_from.is_empty() {
        (p.fps(), k.start_tc, k.made_from.clone(), k.markers.clone())
    } else if let Some((ts, ms)) = imported {
        (p.fps(), k.start_tc, ts, ms)
    } else {
        let fallback = combined_tracks(&k, partner.as_ref());
        if fallback.is_empty() && otio_has_items(&k.stack) {
            return Err(Error::invalid(format!("{lead_id}'s clips weren't found")));
        }
        (p.fps(), k.start_tc, fallback, k.markers.clone())
    };
    push_open(
        p,
        OpenData {
            name,
            target: CompoundTarget::Clip { clip: lead_id },
            fps: open_fps,
            sequence_id: seq_id,
            tracks: raw_tracks,
            markers: raw_markers,
            start_tc,
        },
    );
    Ok(path_json(p))
}

pub fn open_pooled(p: &mut Project, query: &str) -> Result<Value> {
    let found = p
        .compounds
        .iter()
        .find(|x| x.id == query)
        .or_else(|| p.compounds.iter().find(|x| x.name == query))
        .cloned()
        .ok_or_else(|| Error::not_found(format!("no compound clip {query:?}")))?;
    if p.compound_path.len() == 1
        && matches!(&p.compound_path[0].target, CompoundTarget::Pooled { compound } if compound == &found.id)
    {
        return Ok(path_json(p));
    }
    if !p.compound_path.is_empty() {
        close_all(p)?;
    }
    let start_tc = found.start_tc();
    let seq_id = found.sequence_id().map(str::to_string);
    let (raw_tracks, raw_markers) = extract_pooled_tracks(&found);
    push_open(
        p,
        OpenData {
            name: found.name,
            target: CompoundTarget::Pooled { compound: found.id },
            fps: found.fps,
            sequence_id: seq_id,
            tracks: raw_tracks,
            markers: raw_markers,
            start_tc,
        },
    );
    Ok(path_json(p))
}

pub fn close(p: &mut Project, to: Option<usize>) -> Result<Value> {
    if p.compound_path.is_empty() {
        return Err(Error::invalid("no compound clip is open"));
    }
    let target = to.unwrap_or(p.compound_path.len() - 1);
    if target >= p.compound_path.len() {
        return Err(Error::usage(format!("no ancestor {target} to close to")));
    }
    while p.compound_path.len() > target {
        close_one(p)?;
    }
    let name = p
        .open_compound()
        .map(|c| c.name.as_str())
        .or_else(|| p.timeline().map(|t| t.name.as_str()));
    Ok(json!({
        "open": p.open_id(),
        "name": name,
        "path": breadcrumbs(p),
    }))
}

pub fn close_all(p: &mut Project) -> Result<()> {
    while !p.compound_path.is_empty() {
        close_one(p)?;
    }
    Ok(())
}

fn normalize_tracks(tracks: Vec<Track>) -> Vec<Track> {
    let (mut video, mut audio): (Vec<Track>, Vec<Track>) =
        tracks.into_iter().partition(|t| t.kind == TrackKind::Video);
    if video.len() == 1 && video[0].clips.is_empty() && video[0].transitions.is_empty() {
        video.clear();
    }
    if audio.len() == 1 && audio[0].clips.is_empty() && audio[0].transitions.is_empty() {
        audio.clear();
    }
    renumber_list(&mut video, &mut audio);
    video.into_iter().chain(audio).collect()
}

fn strip_follows(tracks: &mut [Track]) {
    for c in tracks.iter_mut().flat_map(|t| &mut t.clips) {
        if let Some(title) = c.title.as_mut() {
            title.follows = None;
        }
    }
}

fn conform_compound(
    p: &mut Project,
    k: &mut Compound,
    from: Rational,
    to: Rational,
    name: &str,
    seq_id: Option<&str>,
) -> Result<()> {
    let Some(r) = super::conform::Rates::new(from, to) else {
        return Ok(());
    };
    super::conform::compound(p, k, r);
    let source = if k.made_from.is_empty() {
        &k.tracks
    } else {
        &k.made_from
    };
    let frames = end_of(source);
    k.frames = k.frames.max(frames);
    k.stack = stack_of_with_id(p, source, name, frames, to, seq_id)?;
    Ok(())
}

struct Replacement<'a> {
    sid: &'a str,
    name: &'a str,
    from: Rational,
    pic: &'a Compound,
    snd: &'a Compound,
}

fn updated_clip_compound(
    p: &mut Project,
    c: &Clip,
    rep: &Replacement,
    host_fps: Rational,
) -> Result<Compound> {
    let mut k = if c.video {
        rep.pic.clone()
    } else {
        rep.snd.clone()
    };
    conform_compound(p, &mut k, rep.from, host_fps, rep.name, Some(rep.sid))?;
    let old_frames = c.compound.as_ref().map_or(0, |old| old.frames);
    k.frames = k.frames.max(c.shown().1).max(old_frames);
    Ok(k)
}

fn pooled_blocks(
    p: &mut Project,
    rep: &Replacement,
    pool_fps: Rational,
    old_frames: i64,
) -> Result<(Option<Compound>, Option<Compound>)> {
    let mut pic = (!rep.pic.tracks.is_empty()).then(|| rep.pic.clone());
    let mut snd = (!rep.snd.tracks.is_empty()).then(|| rep.snd.clone());
    if pic.is_none() && snd.is_none() {
        pic = Some(rep.pic.clone());
    }
    for k in pic.iter_mut().chain(&mut snd) {
        strip_follows(&mut k.tracks);
        strip_follows(&mut k.made_from);
        conform_compound(p, k, rep.from, pool_fps, rep.name, Some(rep.sid))?;
        k.frames = k.frames.max(old_frames);
    }
    Ok((pic, snd))
}

fn propagate_in_compound(
    p: &mut Project,
    k: &mut Compound,
    video: bool,
    shown_end: i64,
    host_fps: Rational,
    rep: &Replacement,
    visited: &mut HashSet<String>,
) -> Result<bool> {
    let outer_sid = otio::sequence_id(k).map(str::to_string);
    if let Some(sid) = outer_sid.as_ref()
        && !visited.insert(sid.clone())
    {
        return Ok(false);
    }
    let outer_name = k.stack["name"]
        .as_str()
        .unwrap_or("Compound Clip")
        .to_string();
    let old_frames = k.frames;
    let res = if !k.made_from.is_empty() {
        let mut whole = std::mem::take(&mut k.made_from);
        let hit = propagate_in_tracks(p, &mut whole, host_fps, rep, None, visited);
        let out = match hit {
            Ok(true) => {
                let frames = end_of(&whole);
                k.tracks = if video {
                    picture_tracks(&whole)
                } else {
                    sound_tracks(&whole)
                };
                k.frames = old_frames.max(frames).max(shown_end);
                stack_of_with_id(
                    p,
                    &whole,
                    &outer_name,
                    frames,
                    host_fps,
                    outer_sid.as_deref(),
                )
                .map(|stack| {
                    k.stack = stack;
                    true
                })
            }
            other => other,
        };
        k.made_from = whole;
        out
    } else {
        let mut inner = std::mem::take(&mut k.tracks);
        let hit = propagate_in_tracks(p, &mut inner, host_fps, rep, None, visited);
        let out = match hit {
            Ok(true) => {
                let frames = end_of(&inner);
                k.frames = old_frames.max(frames).max(shown_end);
                stack_of_with_id(
                    p,
                    &inner,
                    &outer_name,
                    frames,
                    host_fps,
                    outer_sid.as_deref(),
                )
                .map(|stack| {
                    k.stack = stack;
                    true
                })
            }
            other => other,
        };
        k.tracks = inner;
        out
    };
    if let Some(sid) = outer_sid.as_deref() {
        visited.remove(sid);
    }
    res
}

fn propagate_in_tracks(
    p: &mut Project,
    tracks: &mut [Track],
    host_fps: Rational,
    rep: &Replacement,
    skip_ids: Option<&HashSet<String>>,
    visited: &mut HashSet<String>,
) -> Result<bool> {
    let mut changed = false;
    for t in tracks {
        for c in &mut t.clips {
            if skip_ids.is_none_or(|s| !s.contains(&c.id))
                && c.compound
                    .as_ref()
                    .and_then(otio::sequence_id)
                    .is_some_and(|s| s == rep.sid)
            {
                let mut updated = updated_clip_compound(p, c, rep, host_fps)?;
                strip_follows(&mut updated.tracks);
                strip_follows(&mut updated.made_from);
                c.compound = Some(updated);
                changed = true;
                continue;
            }
            let (video, shown_end) = (c.video, c.shown().1);
            if let Some(k) = c.compound.as_mut()
                && propagate_in_compound(p, k, video, shown_end, host_fps, rep, visited)?
            {
                changed = true;
            }
        }
    }
    Ok(changed)
}

fn close_one(p: &mut Project) -> Result<()> {
    let whole = normalize_tracks(std::mem::take(&mut p.tracks));
    let markers = std::mem::take(&mut p.markers);
    p.marks = Marks::default();
    p.subtitles.clear();
    let open = p
        .compound_path
        .pop()
        .ok_or_else(|| Error::invalid("no compound clip is open"))?;
    p.check_out();

    let inner_frames = end_of(&whole);
    let pic_tracks = picture_tracks(&whole);
    let snd_tracks = sound_tracks(&whole);
    let stack = stack_of_with_id(
        p,
        &whole,
        &open.name,
        inner_frames,
        open.fps,
        open.sequence_id.as_deref(),
    )?;
    let pic_base = Compound {
        stack: stack.clone(),
        frames: inner_frames,
        tracks: pic_tracks,
        made_from: whole.clone(),
        markers: markers.clone(),
        start_tc: open.start_tc,
    };
    let snd_base = Compound {
        stack,
        frames: inner_frames,
        tracks: snd_tracks,
        made_from: whole,
        markers,
        start_tc: open.start_tc,
    };
    let sid = otio::sequence_id(&pic_base)
        .map(str::to_string)
        .unwrap_or_default();

    let rep = Replacement {
        sid: open.sequence_id.as_deref().unwrap_or(&sid),
        name: &open.name,
        from: open.fps,
        pic: &pic_base,
        snd: &snd_base,
    };
    let mut updated_targets = HashSet::new();
    let mut updated_pooled: Option<String> = None;

    match &open.target {
        CompoundTarget::Clip { clip } => {
            let mut ids = vec![clip.clone()];
            ids.extend(otio::tied(p, clip));
            let host_fps = p.fps();
            for tid in ids {
                if let Some((ti, ci)) = p.find_clip(&tid) {
                    let c = p.tracks[ti].clips[ci].clone();
                    let k = updated_clip_compound(p, &c, &rep, host_fps)?;
                    p.tracks[ti].clips[ci].compound = Some(k);
                    updated_targets.insert(tid);
                }
            }
        }
        CompoundTarget::Pooled { compound } => {
            if let Some(i) = p.compounds.iter().position(|x| &x.id == compound) {
                let pool_fps = p.compounds[i].fps;
                let old_frames = p.compounds[i].frames();
                let (pic, snd) = pooled_blocks(p, &rep, pool_fps, old_frames)?;
                p.compounds[i].picture = pic;
                p.compounds[i].sound = snd;
                updated_pooled = Some(compound.clone());
            }
        }
    }

    if let Some(old_sid) = open.sequence_id.as_deref() {
        let rep = Replacement {
            sid: old_sid,
            ..rep
        };
        let mut compounds = std::mem::take(&mut p.compounds);
        for item in &mut compounds {
            if updated_pooled.as_deref() == Some(item.id.as_str()) {
                continue;
            }
            if item.sequence_id() == Some(old_sid) {
                let old_frames = item.frames();
                let (pic, snd) = pooled_blocks(p, &rep, item.fps, old_frames)?;
                item.picture = pic;
                item.sound = snd;
                continue;
            }
            let fps = item.fps;
            for (video, slot) in [(true, &mut item.picture), (false, &mut item.sound)] {
                if let Some(k) = slot.as_mut() {
                    let mut visited = HashSet::new();
                    propagate_in_compound(p, k, video, 0, fps, &rep, &mut visited)?;
                }
            }
        }
        p.compounds = compounds;

        let host_fps = p.fps();
        let mut tracks = std::mem::take(&mut p.tracks);
        let mut visited = HashSet::new();
        propagate_in_tracks(
            p,
            &mut tracks,
            host_fps,
            &rep,
            Some(&updated_targets),
            &mut visited,
        )?;
        p.tracks = tracks;

        let checked_out_tl = p.checked_out_timeline().map(str::to_string);
        let project_fps = p.settings.fps;
        let mut timelines = std::mem::take(&mut p.timelines);
        for t in &mut timelines {
            if checked_out_tl.as_deref() == Some(t.id.as_str()) {
                continue;
            }
            let fps = t.own.map_or(project_fps, |o| o.fps);
            let mut visited = HashSet::new();
            propagate_in_tracks(p, &mut t.tracks, fps, &rep, None, &mut visited)?;
        }
        p.timelines = timelines;

        let mut path = std::mem::take(&mut p.compound_path);
        for oc in &mut path {
            let mut visited = HashSet::new();
            propagate_in_tracks(p, &mut oc.tracks, oc.fps, &rep, None, &mut visited)?;
        }
        p.compound_path = path;
    }

    Ok(())
}

pub enum SetTarget<'a> {
    Open,
    Pooled(&'a str),
    Clip(&'a str),
}

fn settings_with(base: &Settings, fps: Rational, drop_frame: bool) -> Settings {
    Settings {
        fps,
        drop_frame: drop_frame && fps.has_drop_frame(),
        ..base.clone()
    }
}

fn settings_for_rate(p: &Project, fps: Rational) -> Settings {
    settings_with(&p.flat_settings(), fps, p.format().drop_frame)
}

fn propagate_tracks_start_tc(
    tracks: &mut [Track],
    sid: &str,
    from: &Settings,
    to: &Settings,
    frames: i64,
) {
    let target = super::conform::relabel_start(from, to, frames);
    for c in tracks.iter_mut().flat_map(|t| &mut t.clips) {
        if let Some(k) = c.compound.as_mut() {
            if otio::sequence_id(k) == Some(sid) {
                k.start_tc = target;
            }
            propagate_tracks_start_tc(&mut k.tracks, sid, from, to, frames);
            propagate_tracks_start_tc(&mut k.made_from, sid, from, to, frames);
        }
    }
}

fn propagate_start_tc(p: &mut Project, sid: &str, from: &Settings, frames: i64) {
    let base = p.flat_settings();
    let project_fps = p.settings.fps;
    let project_df = p.settings.drop_frame;
    for item in &mut p.compounds {
        let item_fps = item.fps;
        let item_to = settings_with(&base, item_fps, base.drop_frame);
        let target = super::conform::relabel_start(from, &item_to, frames);
        let item_match = item.sequence_id() == Some(sid);
        for b in item.blocks_mut() {
            if item_match {
                b.start_tc = target;
            }
            propagate_tracks_start_tc(&mut b.tracks, sid, from, &item_to, frames);
            propagate_tracks_start_tc(&mut b.made_from, sid, from, &item_to, frames);
        }
    }
    let active_to = settings_with(&base, p.fps(), base.drop_frame);
    let mut tracks = std::mem::take(&mut p.tracks);
    propagate_tracks_start_tc(&mut tracks, sid, from, &active_to, frames);
    p.tracks = tracks;

    let checked_out = p.checked_out_timeline().map(str::to_string);
    let mut timelines = std::mem::take(&mut p.timelines);
    for t in &mut timelines {
        if checked_out.as_deref() == Some(t.id.as_str()) {
            continue;
        }
        let tl_fps = t.own.map_or(project_fps, |o| o.fps);
        let tl_df = t.own.map_or(project_df, |o| o.drop_frame);
        let tl_to = settings_with(&base, tl_fps, tl_df);
        propagate_tracks_start_tc(&mut t.tracks, sid, from, &tl_to, frames);
    }
    p.timelines = timelines;

    let mut path = std::mem::take(&mut p.compound_path);
    for oc in &mut path {
        let oc_to = settings_with(&base, oc.fps, base.drop_frame);
        let target = super::conform::relabel_start(from, &oc_to, frames);
        if oc.sequence_id.as_deref() == Some(sid) {
            oc.start_tc = target;
        }
        propagate_tracks_start_tc(&mut oc.tracks, sid, from, &oc_to, frames);
    }
    p.compound_path = path;
}

pub fn set_start_tc(p: &mut Project, target: SetTarget<'_>, tc: &TimeSpec) -> Result<Value> {
    let (fmt, frames, sid) = match target {
        SetTarget::Open => {
            if p.compound_path.is_empty() {
                return Err(Error::invalid("no compound clip is open"));
            }
            let fmt = p.flat_settings();
            let frames = fmt.start_frames(tc)?;
            let open = p.compound_path.last_mut().expect("non-empty");
            open.start_tc = frames;
            let sid = open.sequence_id.clone();
            (fmt, frames, sid)
        }
        SetTarget::Pooled(query) => {
            let idx = p
                .compounds
                .iter()
                .position(|x| x.id == query)
                .or_else(|| p.compounds.iter().position(|x| x.name == query))
                .ok_or_else(|| Error::not_found(format!("no compound clip {query:?}")))?;
            let fmt = settings_for_rate(p, p.compounds[idx].fps);
            let frames = fmt.start_frames(tc)?;
            let id = p.compounds[idx].id.clone();
            let sid = p.compounds[idx].sequence_id().map(str::to_string);
            for b in p.compounds[idx].blocks_mut() {
                b.start_tc = frames;
            }
            if sid.is_none()
                && let Some(oc) = p.compound_path.first_mut()
                && matches!(&oc.target, CompoundTarget::Pooled { compound } if compound == &id)
            {
                oc.start_tc = frames;
            }
            (fmt, frames, sid)
        }
        SetTarget::Clip(query_id) => {
            let (ti, ci) = locate(p, query_id)?;
            if p.tracks[ti].clips[ci].compound.is_none() {
                return Err(Error::invalid(format!("{query_id} isn't a compound clip")));
            }
            let fmt = p.flat_settings();
            let frames = fmt.start_frames(tc)?;
            let sid = p.tracks[ti].clips[ci]
                .compound
                .as_ref()
                .and_then(otio::sequence_id)
                .map(str::to_string);
            let mut ids = vec![query_id.to_string()];
            ids.extend(otio::tied(p, query_id));
            for id in ids {
                if let Some((tti, cci)) = p.find_clip(&id)
                    && let Some(k) = p.tracks[tti].clips[cci].compound.as_mut()
                {
                    k.start_tc = frames;
                }
            }
            (fmt, frames, sid)
        }
    };
    if let Some(sid) = sid {
        propagate_start_tc(p, &sid, &fmt, frames);
    }
    Ok(json!({
        "start_tc": format_timecode(0, fmt.fps, frames, fmt.counts_drop_frame()),
        "frames": frames,
    }))
}

fn relabel_nested_compounds(tracks: &mut [Track], new: &Settings, old: &Settings) {
    for c in tracks.iter_mut().flat_map(|t| &mut t.clips) {
        if let Some(k) = c.compound.as_mut() {
            if k.start_tc != 0 {
                k.start_tc = new.relabel(k.start_tc, old);
            }
            relabel_nested_compounds(&mut k.tracks, new, old);
            relabel_nested_compounds(&mut k.made_from, new, old);
        }
    }
}

pub fn relabel_start_tc(p: &mut Project, was: &Settings) {
    let now = p.settings.clone();
    for item in &mut p.compounds {
        if item.fps != was.fps {
            continue;
        }
        for b in item.blocks_mut() {
            if b.start_tc != 0 {
                b.start_tc = now.relabel(b.start_tc, was);
            }
            relabel_nested_compounds(&mut b.tracks, &now, was);
            relabel_nested_compounds(&mut b.made_from, &now, was);
        }
    }
    let active_inherits = p.timeline().is_none_or(|t| t.own.is_none());
    if active_inherits && p.compound_path.last().is_none_or(|oc| oc.fps == was.fps) {
        relabel_nested_compounds(&mut p.tracks, &now, was);
    }
    let checked_out = p.checked_out_timeline().map(str::to_string);
    for t in &mut p.timelines {
        if checked_out.as_deref() == Some(t.id.as_str()) || t.own.is_some() {
            continue;
        }
        relabel_nested_compounds(&mut t.tracks, &now, was);
    }
    if active_inherits {
        for oc in &mut p.compound_path {
            if oc.fps == was.fps {
                if oc.start_tc != 0 {
                    oc.start_tc = now.relabel(oc.start_tc, was);
                }
                relabel_nested_compounds(&mut oc.tracks, &now, was);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::model::Clip;
    use crate::test_support::Fixture;

    fn req(method: &str, params: Value) -> Value {
        json!({"method": method, "params": params})
    }

    fn nest(f: &mut Fixture, ids: &[&String]) -> Vec<String> {
        let r = f.ok(req("edit.compound", json!({"clips": ids})));
        serde_json::from_value(r.result["clips"].clone()).unwrap()
    }

    fn l_cut(f: &mut Fixture, id: &str, by: i64) {
        f.ok(req(
            "edit.audio_roll",
            json!({"clip": id, "edge": "tail", "by": by}),
        ));
    }

    #[test]
    fn a_decompose_ends_an_l_cut_at_the_trimmed_end_of_the_compound_clip() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        l_cut(&mut f, &a, 10);
        let hosts = nest(&mut f, &[&a]);
        let trim = json!({"clip": hosts[0], "edge": "tail", "to": "55f", "ripple": false});
        f.ok(req("edit.trim", trim));
        f.append("m1", 0, 15);
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        let range = f.editor.project().clip(&a).unwrap().audio_range();
        assert_eq!(range, Some((0, 55)));
    }

    #[test]
    fn a_decompose_keeps_an_l_cut_longer_than_the_first_clip_back() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let short = f.append("m1", 50, 55);
        let long = f.append("m1", 55, 100);
        let hosts = nest(&mut f, &[&short, &long]);
        l_cut(&mut f, &a, 10);
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        let range = f.editor.project().clip(&a).unwrap().audio_range();
        assert_eq!(range, Some((0, 60)));
    }

    #[test]
    fn a_decompose_between_an_l_cut_and_a_j_cut_that_meet_keeps_both() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let short = f.append("m1", 50, 58);
        let c = f.append("m1", 58, 98);
        let hosts = nest(&mut f, &[&short]);
        l_cut(&mut f, &a, 5);
        let j_cut = json!({"clip": c, "edge": "head", "by": -3});
        f.ok(req("edit.audio_roll", j_cut));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        let range = |id: &str| f.editor.project().clip(id).unwrap().audio_range();
        assert_eq!(
            [range(&a), range(&short), range(&c)],
            [Some((0, 55)), None, Some((55, 98))]
        );
    }

    #[test]
    fn a_media_replace_takes_the_sound_of_the_compound_clip() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        nest(&mut f, &[&a]);
        let replace = json!({"media": "m1", "range": "0f..10f", "at": 5});
        f.ok(req("edit.ripple_overwrite", replace));
        assert_eq!(f.track("A1"), vec![]);
    }

    #[test]
    fn a_decompose_keeps_the_l_cut_of_the_clip_before_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let b = f.append("m1", 50, 100);
        let hosts = nest(&mut f, &[&b]);
        l_cut(&mut f, &a, 10);
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        let p = f.editor.project();
        let ranges = (
            p.clip(&a).unwrap().audio_range(),
            p.clip(&b).unwrap().audio_range(),
        );
        assert_eq!(ranges, (Some((0, 60)), Some((60, 100))));
    }

    #[test]
    fn a_range_cuts_each_track_at_the_marks_and_nests_what_is_between() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 60);
        let upper = f.append("m1", 0, 60);
        let moved = json!({"clips": [upper], "to": 0, "mode": "overwrite", "track": "V2"});
        f.ok(req("edit.move", moved));
        f.ok(req("edit.compound_range", json!({"range": "10f..40f"})));
        let host = f.editor.project().track("V1").unwrap().clips[1].clone();
        assert_eq!((host.start, host.duration), (10, 30));
        assert_eq!(f.track("V2").len(), 2);
    }

    #[test]
    fn create_compound_clip_leaves_the_timeline_and_fills_the_pool() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 60);
        let before = f.editor.project().tracks.clone();
        f.ok(req("media.compound", json!({"range": "10f..40f"})));
        assert_eq!(f.editor.project().tracks, before);
        assert_eq!(f.editor.project().compounds[0].frames(), 30);
    }

    #[test]
    fn a_replace_by_a_pooled_compound_clip_leaves_the_clip_above_where_it_is() {
        let mut f = Fixture::new(&[(8.0, true)]);
        let _ = f.append("m1", 0, 40);
        let _ = f.append("m1", 0, 40);
        let _ = f.append("m1", 0, 30);
        let above = f.append("m1", 0, 20);
        let moved = json!({"clips": [above], "to": 50, "mode": "overwrite", "track": "V2"});
        f.ok(req("edit.move", moved));
        f.ok(req("media.compound", json!({"range": "0f..12f"})));
        let id = f.editor.project().compounds[0].id.clone();
        let place = json!({"compound": id, "at": "45f", "how": "replace"});
        f.ok(req("edit.place_compound", place));
        assert_eq!(f.editor.project().clip(&above).unwrap().start, 50);
    }

    #[test]
    fn a_pooled_compound_clip_inserts_at_an_edit_and_moves_the_clips_after_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let _ = f.append("m1", 0, 30);
        let after = f.append("m1", 30, 60);
        f.ok(req("media.compound", json!({"range": "0f..12f"})));
        let id = f.editor.project().compounds[0].id.clone();
        let place = json!({"compound": id, "at": "30f", "how": "insert"});
        f.ok(req("edit.place_compound", place));
        assert_eq!(f.editor.project().clip(&after).unwrap().start, 42);
    }

    #[test]
    fn a_longer_speed_edit_moves_the_sound_of_the_next_compound_clip() {
        let mut f = Fixture::new(&[(8.0, true)]);
        let a = f.append("m1", 0, 60);
        let b = f.append("m1", 60, 100);
        let first = nest(&mut f, &[&a]);
        let second = nest(&mut f, &[&b]);
        let slower = json!({"clips": [first[0]], "speed": 0.5});
        f.ok(req("edit.change_speed", slower));
        let start = |id: &str| f.editor.project().clip(id).map(|c| c.start);
        assert_eq!(
            (start(&second[0]), start(&second[1])),
            (Some(120), Some(120))
        );
    }

    #[test]
    fn a_speed_keyframe_goes_on_both_blocks_of_a_compound_clip() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 60);
        let parts = nest(&mut f, &[&a]);
        let key = json!({"clips": [parts[0]], "param": "speed", "at": "30f"});
        f.ok(req("keyframes.add", key));
        let sections = |id: &str| {
            let c = f.editor.project().clip(id).unwrap().clone();
            c.speed.map(|s| s.sections.len())
        };
        assert_eq!(
            (sections(&parts[0]), sections(&parts[1])),
            (Some(2), Some(2))
        );
    }

    #[test]
    fn a_pooled_compound_clip_lands_on_v1_and_a1_linked() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 60);
        f.ok(req("media.compound", json!({"range": "10f..40f"})));
        let id = f.editor.project().compounds[0].id.clone();
        let r = f.ok(req("edit.place_compound", json!({"compound": id})));
        let hosts = r.result["clips"].as_array().unwrap().len();
        assert_eq!((hosts, f.track("V1")[1].1), (2, 60));
    }

    #[test]
    fn a_range_goes_on_the_lowest_track_with_clips_in_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 10);
        let upper = f.append("m1", 0, 60);
        let moved = json!({"clips": [upper], "to": 20, "mode": "overwrite", "track": "V2"});
        f.ok(req("edit.move", moved));
        f.ok(req("edit.compound_range", json!({"range": "30f..40f"})));
        let v2 = f.editor.project().track("V2").unwrap();
        assert_eq!(v2.clips[1].compound.as_ref().unwrap().tracks[0].id, "V1");
        assert_eq!(f.track("V1").len(), 1);
    }

    #[test]
    fn a_nest_and_a_decompose_restore_the_clips_and_their_edits() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let b = f.append("m1", 50, 100);
        f.ok(req("clip.set", json!({"clips": a, "pan": 40.0})));
        f.ok(req("picture.set", json!({"clips": b, "zoom": 1.5})));
        l_cut(&mut f, &a, -5);
        let before = f.editor.project().tracks[0].clips.clone();
        let hosts = nest(&mut f, &[&a, &b]);
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        assert_eq!(f.editor.project().tracks[0].clips, before);
    }

    #[test]
    fn a_track_between_selected_tracks_stays_inside_as_an_empty_track() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let b = f.append("m1", 50, 100);
        for track in ["V2", "V3", "V4", "V3"] {
            let moved = json!({"clips": [b], "to": 0, "mode": "overwrite", "track": track});
            f.ok(req("edit.move", moved));
        }
        let hosts = nest(&mut f, &[&a, &b]);
        let host = f.editor.project().clip(&hosts[0]).unwrap();
        let made = &host.compound.as_ref().unwrap().made_from;
        let shape: Vec<(&str, usize)> = made.iter().map(|t| (&*t.id, t.clips.len())).collect();
        assert_eq!(shape, [("V1", 1), ("V2", 0), ("V3", 1)]);
    }

    #[test]
    fn a_clip_in_the_way_of_a_decompose_moves_up_on_a_new_track() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let b = f.append("m1", 50, 100);
        let c = f.append("m1", 0, 50);
        let moved = json!({"clips": [b], "to": 0, "mode": "overwrite", "track": "V2"});
        f.ok(req("edit.move", moved));
        let hosts = nest(&mut f, &[&a, &b]);
        let moved = json!({"clips": [c], "to": 0, "mode": "overwrite", "track": "V2"});
        f.ok(req("edit.move", moved));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        assert_eq!(f.track("V2")[0].0, b);
        assert_eq!(f.track("V3")[0].0, c);
    }

    #[test]
    fn a_neighbours_l_cut_stays_on_its_track_after_a_decompose() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let b = f.append("m1", 50, 100);
        l_cut(&mut f, &a, 10);
        let hosts = nest(&mut f, &[&b]);
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        assert_eq!(f.track("V1")[0].0, a);
    }

    #[test]
    fn a_compound_clip_runs_to_the_end_of_an_l_cut() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        l_cut(&mut f, &a, 10);
        let hosts = nest(&mut f, &[&a]);
        let host = f.editor.project().clip(&hosts[0]).unwrap();
        assert_eq!((host.start, host.duration), (0, 60));
    }

    #[test]
    fn a_skipped_clip_puts_the_compound_clip_on_a_new_track_above_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 25);
        let b = f.append("m1", 25, 50);
        let c = f.append("m1", 50, 75);
        let hosts = nest(&mut f, &[&a, &c]);
        assert_eq!(f.track("V1").len(), 1);
        assert_eq!(f.track("V1")[0].0, b);
        assert_eq!(f.track("V2")[0].0, hosts[0]);
    }

    #[test]
    fn a_sound_block_holds_an_l_cuts_sound_at_its_own_length() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 10, 60);
        l_cut(&mut f, &a, 5);
        let hosts = nest(&mut f, &[&a]);
        let host = f.editor.project().clip(&hosts[1]).unwrap();
        let sound: &Clip = &host.compound.as_ref().unwrap().tracks[0].clips[0];
        assert_eq!((sound.start, sound.duration, sound.source_in), (0, 55, 10));
    }

    fn keyed_clip(f: &mut Fixture) -> String {
        let a = f.append("m1", 20, 70);
        for (at, value) in [("10f", 0.0), ("30f", 1.0)] {
            let key = json!({"clips": a, "param": "opacity", "at": at, "value": value});
            f.ok(req("keyframes.add", key));
        }
        a
    }

    fn opacity_keys(f: &Fixture, id: &str) -> Vec<(i64, f64)> {
        let c = f.editor.project().clip(id).unwrap();
        let keys = &c.picture.as_ref().unwrap().opacity_keys;
        keys.iter().map(|k| (k.frame, k.value)).collect()
    }

    fn opacity_key_frames(f: &Fixture, id: &str) -> Vec<i64> {
        let c = f.editor.project().clip(id).unwrap();
        let keys = &c.picture.as_ref().unwrap().opacity_keys;
        keys.iter().map(|k| k.frame).collect()
    }

    #[test]
    fn a_faster_compound_clip_plays_the_keyframes_in_it_faster() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = keyed_clip(&mut f);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req(
            "edit.change_speed",
            json!({"clips": [hosts[0]], "speed": 2.0}),
        ));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        assert_eq!(opacity_key_frames(&f, &a), vec![5, 15]);
    }

    #[test]
    fn a_reversed_compound_clip_plays_the_keyframes_in_it_backward() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = keyed_clip(&mut f);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req(
            "edit.change_speed",
            json!({"clips": [hosts[0]], "speed": -1.0}),
        ));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        assert_eq!(opacity_key_frames(&f, &a), vec![19, 39]);
    }

    #[test]
    fn a_ramped_compound_clip_moves_each_keyframe_by_the_speed_of_its_section() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = keyed_clip(&mut f);
        let hosts = nest(&mut f, &[&a]);
        let key = json!({"clips": [hosts[0]], "param": "speed", "at": "20f"});
        f.ok(req("keyframes.add", key));
        let fast = json!({"clips": [hosts[0]], "at": "0f", "speed": 2.0});
        f.ok(req("speed.set", fast));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        assert_eq!(opacity_keys(&f, &a), vec![(5, 0.0), (10, 0.5), (20, 1.0)]);
    }

    #[test]
    fn a_freeze_inside_a_compound_clip_holds_the_value_of_its_frame() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = keyed_clip(&mut f);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req(
            "speed.freeze",
            json!({"clips": [hosts[0]], "at": "20f"}),
        ));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        let held = vec![(10, 0.0), (20, 0.5), (70, 0.5), (80, 1.0)];
        assert_eq!(opacity_keys(&f, &a), held);
    }

    #[test]
    fn a_freeze_at_the_head_of_a_compound_clip_keeps_the_keyframes_in_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = keyed_clip(&mut f);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req(
            "speed.freeze",
            json!({"clips": [hosts[0]], "at": "0f"}),
        ));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        assert_eq!(opacity_key_frames(&f, &a), vec![60, 80]);
    }

    #[test]
    fn a_frozen_compound_clip_holds_the_value_of_the_keyframes_in_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = keyed_clip(&mut f);
        let hosts = nest(&mut f, &[&a]);
        let trim = json!({"clip": hosts[0], "edge": "head", "to": "20f", "ripple": false});
        f.ok(req("edit.trim", trim));
        f.ok(req(
            "edit.change_speed",
            json!({"clips": [hosts[0]], "speed": 0.0}),
        ));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        let c = f.editor.project().clip(&a).unwrap();
        let picture = c.picture.as_ref().unwrap();
        assert_eq!(
            (picture.opacity, picture.opacity_keys.len()),
            (Some(0.5), 0)
        );
    }

    #[test]
    fn a_frozen_compound_clip_holds_the_level_of_its_frame_in_a_dissolve() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 10, 50);
        let b = f.append("m1", 50, 90);
        f.ok(req("edit.dissolve", json!({"at": 40})));
        let host = nest(&mut f, &[&a, &b]).remove(0);
        let trim = json!({"clip": host, "edge": "head", "to": "35f", "ripple": false});
        f.ok(req("edit.trim", trim));
        f.ok(req(
            "edit.change_speed",
            json!({"clips": [host], "speed": 0.0}),
        ));
        let plan = crate::render::flatten(f.editor.project(), 35, 80).unwrap();
        let crate::render::VideoPiece::Mix { over, .. } = &plan.video[0] else {
            panic!("the frozen frame is a mix");
        };
        let ramp = over[0].ramp.unwrap();
        assert_eq!((ramp.at, ramp.of, ramp.held), (8, 25, true));
    }

    #[test]
    fn a_dissolve_between_nested_clips_returns_with_a_decompose() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let b = f.append("m1", 50, 100);
        f.ok(req("edit.dissolve", json!({"at": 50})));
        let before = f.editor.project().tracks[0].transitions.clone();
        let hosts = nest(&mut f, &[&a, &b]);
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        assert_eq!(f.editor.project().tracks[0].transitions, before);
    }

    #[test]
    fn media_used_by_lists_the_compound_clip_that_holds_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let hosts = nest(&mut f, &[&a]);
        let r = f.ok(req("media.get", json!({"media": "m1"})));
        let pooled = f.editor.project().compounds[0].id.clone();
        assert_eq!(r.result["used_by"], json!([hosts[0], hosts[1], pooled]));
    }

    #[test]
    fn a_new_subclip_start_moves_the_clips_of_compound_clips() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let sub = json!({"media": "m1", "range": "10f..60f", "name": "s"});
        f.ok(req("media.subclip", sub));
        let sub = f.editor.project().media.last().unwrap().id.clone();
        let a = f.append(&sub, 0, 20);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req(
            "media.subclip_set",
            json!({"media": sub, "range": "5f..60f"}),
        ));
        let p = f.editor.project();
        let k = p.clip(&hosts[0]).unwrap().compound.as_ref().unwrap();
        let tracks = k.tracks[0].clips[0].source_in;
        let made = k.made_from[0].clips[0].source_in;
        let pooled = p.compounds[0].picture.as_ref().unwrap();
        let in_pool = pooled.made_from[0].clips[0].source_in;
        assert_eq!((tracks, made, in_pool), (5, 5, 5));
    }

    #[test]
    fn a_subclip_cannot_lose_frames_only_a_pooled_compound_clip_uses() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let sub = json!({"media": "m1", "range": "10f..60f", "name": "s"});
        f.ok(req("media.subclip", sub));
        let sub = f.editor.project().media.last().unwrap().id.clone();
        let a = f.append(&sub, 0, 40);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("edit.delete", json!({"clips": hosts})));
        f.err(req(
            "media.subclip_set",
            json!({"media": sub, "range": "10f..30f"}),
        ));
    }

    #[test]
    fn a_renamed_bin_keeps_its_compound_clip() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 60);
        f.ok(req("bins.add", json!({"path": "X"})));
        let made = json!({"range": "10f..40f", "bin": "X"});
        f.ok(req("media.compound", made));
        f.ok(req("bins.rename", json!({"path": "X", "to": "Y"})));
        assert_eq!(f.editor.project().compounds[0].bin, "Y");
    }

    #[test]
    fn a_removed_bin_deletes_its_compound_clip() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 60);
        f.ok(req("bins.add", json!({"path": "X"})));
        let made = json!({"range": "10f..40f", "bin": "X"});
        f.ok(req("media.compound", made));
        f.ok(req("bins.remove", json!({"path": "X"})));
        assert!(f.editor.project().compounds.is_empty());
    }

    #[test]
    fn a_pooled_compound_clip_keeps_the_frame_rate() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("edit.delete", json!({"clips": hosts})));
        f.err(req("project.set", json!({"fps": "30"})));
    }

    #[test]
    fn opening_a_compound_clip_edits_inside_and_unlocks_outer_tail_trim() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        assert_eq!(f.editor.project().compound_path.len(), 1);
        f.ok(req("picture.set", json!({"clips": [a], "zoom": 1.5})));
        let b = f.append("m1", 50, 80);
        assert_eq!(f.v1().len(), 2);
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert!(p.compound_path.is_empty());
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
        let outer = p.clip(&hosts[0]).unwrap();
        let k = outer.compound.as_ref().unwrap();
        assert_eq!(
            (outer.duration, k.frames, k.made_from[0].clips.len()),
            (50, 80, 2)
        );
        assert_eq!(
            k.tracks[0].clips[0].picture.as_ref().unwrap().zoom,
            Some([1.5, 1.5])
        );
        assert_eq!(k.made_from[0].clips[1].id, b);
        let trim = json!({"clip": hosts[0], "edge": "tail", "to": "75f", "ripple": false});
        f.ok(req("edit.trim", trim));
        let p = f.editor.project();
        assert_eq!(p.clip(&hosts[0]).unwrap().duration, 75);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn shortening_inner_clips_preserves_outer_duration_and_plays_black_past_end() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 60);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req(
            "edit.trim",
            json!({"clip": a, "edge": "tail", "to": "20f", "ripple": false}),
        ));
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
        let outer = p.clip(&hosts[0]).unwrap();
        let k = outer.compound.as_ref().unwrap();
        assert_eq!(
            (outer.duration, k.frames, k.made_from[0].clips[0].duration),
            (60, 60, 20)
        );
        let plan = crate::render::flatten(p, 25, 30).unwrap();
        assert!(matches!(
            plan.video.as_slice(),
            [crate::render::VideoPiece::Black { frames: 5 }]
        ));
        assert!(
            plan.audio
                .iter()
                .flatten()
                .all(|p| matches!(p, crate::render::AudioPiece::Silence { .. }))
        );
    }

    #[test]
    fn editing_one_compound_updates_siblings_other_timelines_and_pool() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        let pool = f.editor.project().compounds[0].id.clone();
        let placed: Vec<String> = serde_json::from_value(
            f.ok(req("edit.place_compound", json!({"compound": pool})))
                .result["clips"]
                .clone(),
        )
        .unwrap();
        let r = f.ok(req(
            "timeline.new",
            json!({"name": "T2", "own": {"fps": "50"}}),
        ));
        let _t2 = r.result["id"].as_str().unwrap().to_string();
        let on_t2: Vec<String> = serde_json::from_value(
            f.ok(req("edit.place_compound", json!({"compound": pool})))
                .result["clips"]
                .clone(),
        )
        .unwrap();
        f.ok(req("compound.open", json!({"clip": on_t2[0]})));
        f.append("m1", 0, 20);
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert_eq!(
            p.clip(&on_t2[0]).unwrap().compound.as_ref().unwrap().frames,
            120
        );
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
        f.ok(req("timeline.open", json!({"timeline": "t1"})));
        let p = f.editor.project();
        assert_eq!(
            p.clip(&hosts[0]).unwrap().compound.as_ref().unwrap().frames,
            60
        );
        assert_eq!(
            p.clip(&placed[0])
                .unwrap()
                .compound
                .as_ref()
                .unwrap()
                .frames,
            60
        );
        assert_eq!(p.compounds[0].frames(), 60);
        assert_eq!(
            p.compounds[0].picture.as_ref().unwrap().made_from[0]
                .clips
                .len(),
            2
        );
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn opening_audio_block_opens_whole_compound_and_updates_both_blocks() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[1]})));
        assert_eq!(f.v1().len(), 1);
        f.append("m1", 40, 60);
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert_eq!(
            p.clip(&hosts[0]).unwrap().compound.as_ref().unwrap().frames,
            60
        );
        assert_eq!(
            p.clip(&hosts[1]).unwrap().compound.as_ref().unwrap().frames,
            60
        );
    }

    #[test]
    fn audio_only_compound_gets_temporary_v1_and_drops_empty_v1_on_close() {
        let mut f = Fixture::new(&[]);
        let mut info =
            crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true);
        info.video = None;
        f.env = crate::test_support::FakeEnv::default().with("/media/a.wav", info);
        f.ok(req("media.add", json!({"paths": ["/media/a.wav"]})));
        let ac = f.append("m1", 0, 30);
        let ah = nest(&mut f, &[&ac]);
        f.ok(req("compound.open", json!({"clip": ah[0]})));
        assert!(f.editor.project().track("V1").is_some());
        f.append("m1", 30, 50);
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert!(p.compounds[0].picture.is_none());
        assert_eq!(
            p.compounds[0].sound.as_ref().unwrap().tracks[0].clips.len(),
            2
        );
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn opening_pooled_compound_with_timeline_updates_placement() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        let pool = f.editor.project().compounds[0].id.clone();
        f.ok(req("compound.open", json!({"compound": pool})));
        f.ok(req("picture.set", json!({"clips": [a], "zoom": 2.0})));
        f.ok(req("compound.close", json!({})));
        let outer = f.editor.project().clip(&hosts[0]).unwrap();
        assert_eq!(
            outer.compound.as_ref().unwrap().tracks[0].clips[0]
                .picture
                .as_ref()
                .unwrap()
                .zoom,
            Some([2.0, 2.0])
        );
    }

    #[test]
    fn opening_pooled_compound_without_timelines_saves_to_pool() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        let pool = f.editor.project().compounds[0].id.clone();
        f.ok(req("edit.delete", json!({"clips": hosts})));
        f.ok(req("timeline.delete", json!({"timeline": "t1"})));
        assert!(f.editor.project().timelines.is_empty());
        f.ok(req("compound.open", json!({"compound": pool})));
        assert_eq!(
            crate::validate::everywhere(f.editor.project()),
            Vec::<String>::new()
        );
        let cid = f.editor.project().tracks[0].clips[0].id.clone();
        f.ok(req("picture.set", json!({"clips": [cid], "zoom": 1.25})));
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert!(p.timelines.is_empty());
        assert!(p.is_fresh_sheet());
        assert_eq!(
            p.compounds[0].picture.as_ref().unwrap().made_from[0].clips[0]
                .picture
                .as_ref()
                .unwrap()
                .zoom,
            Some([1.25, 1.25])
        );
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn nested_two_level_compounds_step_up_and_close_to_root() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let inner = nest(&mut f, &[&a]);
        let b = f.append("m1", 30, 60);
        let outer = nest(&mut f, &[&inner[0], &b]);
        f.ok(req("compound.open", json!({"clip": outer[0]})));
        assert_eq!(f.editor.project().compound_path.len(), 1);
        let nested_id = f.editor.project().tracks[0].clips[0].id.clone();
        f.ok(req("compound.open", json!({"clip": nested_id})));
        assert_eq!(f.editor.project().compound_path.len(), 2);
        let leaf = f.editor.project().tracks[0].clips[0].id.clone();
        f.ok(req("picture.set", json!({"clips": [leaf], "zoom": 1.4})));
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert_eq!(p.compound_path.len(), 1);
        let k = p.clip(&nested_id).unwrap().compound.as_ref().unwrap();
        assert_eq!(
            k.tracks[0].clips[0].picture.as_ref().unwrap().zoom,
            Some([1.4, 1.4])
        );
        f.ok(req("compound.open", json!({"clip": nested_id})));
        let leaf2 = f.editor.project().tracks[0].clips[0].id.clone();
        f.ok(req("picture.set", json!({"clips": [leaf2], "zoom": 1.8})));
        f.ok(req("compound.close", json!({"to": 0})));
        let p = f.editor.project();
        assert!(p.compound_path.is_empty());
        let ok_zoom = p
            .clip(&outer[0])
            .unwrap()
            .compound
            .as_ref()
            .unwrap()
            .made_from[0]
            .clips[0]
            .compound
            .as_ref()
            .unwrap()
            .tracks[0]
            .clips[0]
            .picture
            .as_ref()
            .unwrap()
            .zoom;
        assert_eq!(ok_zoom, Some([1.8, 1.8]));
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn placing_or_pasting_a_compound_inside_itself_is_refused() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        let pool = f.editor.project().compounds[0].id.clone();
        let copied = f.ok(req("edit.copy", json!({"clips": hosts}))).result["clips"].clone();
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        let e1 = f.err(req("edit.place_compound", json!({"compound": pool})));
        let e2 = f.err(req("edit.paste", json!({"clips": copied, "at": "0f"})));
        let e3 = f.err(req(
            "edit.paste",
            json!({"clips": copied, "at": "0f", "insert": true}),
        ));
        assert!(e1.message.contains("hold itself"));
        assert!(e2.message.contains("hold itself"));
        assert!(e3.message.contains("hold itself"));
    }

    #[test]
    fn project_file_roundtrip_preserves_open_compound() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req("picture.set", json!({"clips": [a], "zoom": 1.5})));
        let saved = serde_json::to_string(f.editor.project()).unwrap();
        let pf: crate::model::ProjectFile = serde_json::from_str(&saved).unwrap();
        let loaded = crate::model::Project::from(pf);
        assert_eq!(loaded.compound_path.len(), 1);
        assert_eq!(
            loaded.tracks[0].clips[0].picture.as_ref().unwrap().zoom,
            Some([1.5, 1.5])
        );
        assert_eq!(crate::validate::everywhere(&loaded), Vec::<String>::new());
    }

    #[test]
    fn undo_and_redo_work_across_open_edit_and_close() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req("picture.set", json!({"clips": [a], "zoom": 1.5})));
        f.ok(req("compound.close", json!({})));
        f.ok(req("history.undo", json!({"steps": 1})));
        assert_eq!(f.editor.project().compound_path.len(), 1);
        assert_eq!(
            f.editor.project().tracks[0].clips[0]
                .picture
                .as_ref()
                .unwrap()
                .zoom,
            Some([1.5, 1.5])
        );
        f.ok(req("history.undo", json!({"steps": 1})));
        assert_eq!(f.editor.project().compound_path.len(), 1);
        assert!(f.editor.project().tracks[0].clips[0].picture.is_none());
        f.ok(req("history.undo", json!({"steps": 1})));
        assert!(f.editor.project().compound_path.is_empty());
        f.ok(req("history.redo", json!({"steps": 3})));
        let p = f.editor.project();
        assert!(p.compound_path.is_empty());
        assert_eq!(
            p.clip(&hosts[0]).unwrap().compound.as_ref().unwrap().tracks[0].clips[0]
                .picture
                .as_ref()
                .unwrap()
                .zoom,
            Some([1.5, 1.5])
        );
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn subtitles_and_timeline_grade_are_refused_inside_compound() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.err(req("subtitles.add", json!({"text": "hi"})));
        f.err(req(
            "grade.set",
            json!({"timeline": true, "gain": [1.1, 1.1, 1.1, 1.1]}),
        ));
    }

    #[test]
    fn timeline_new_auto_closes_and_saves_open_compound() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req("picture.set", json!({"clips": [a], "zoom": 1.6})));
        f.ok(req("timeline.new", json!({})));
        let p = f.editor.project();
        assert!(p.compound_path.is_empty());
        assert_eq!(
            p.compounds[0].picture.as_ref().unwrap().made_from[0].clips[0]
                .picture
                .as_ref()
                .unwrap()
                .zoom,
            Some([1.6, 1.6])
        );
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn removing_open_pooled_compound_auto_closes_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let _hosts = nest(&mut f, &[&a]);
        let pool = f.editor.project().compounds[0].id.clone();
        f.ok(req("compound.open", json!({"compound": pool})));
        f.ok(req("media.compound_remove", json!({"compound": pool})));
        let p = f.editor.project();
        assert!(p.compound_path.is_empty());
        assert!(p.compounds.is_empty());
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn nested_copy_updates_even_when_clip_id_matches_top_level_clip() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let x_hosts = nest(&mut f, &[&a]);
        let pool_x = f.editor.project().compounds[0].id.clone();
        let x_placed: Vec<String> = serde_json::from_value(
            f.ok(req("edit.place_compound", json!({"compound": pool_x})))
                .result["clips"]
                .clone(),
        )
        .unwrap();
        let y_hosts = nest(&mut f, &[&x_placed[0]]);
        f.ok(req("edit.delete", json!({"clips": x_hosts})));
        f.ok(req("edit.decompose", json!({"clips": [y_hosts[0]]})));
        f.ok(req("compound.open", json!({"clip": x_placed[0]})));
        f.append("m1", 30, 50);
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        let nested_x = p.compounds[1].picture.as_ref().unwrap().made_from[0].clips[0]
            .compound
            .as_ref()
            .unwrap();
        assert_eq!(nested_x.frames, 50);
        assert_eq!(nested_x.made_from[0].clips.len(), 2);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn shortening_inner_clips_preserves_edge_dissolve_on_trimmed_placement() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        let hosts = nest(&mut f, &[&a]);
        let _b = f.append("m1", 50, 90);
        f.ok(req(
            "edit.trim",
            json!({"clip": hosts[0], "edge": "tail", "to": "30f", "ripple": true}),
        ));
        f.ok(req(
            "edit.dissolve",
            json!({"track": "V1", "at": "30f", "before": "5f", "after": "5f"}),
        ));
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req(
            "edit.trim",
            json!({"clip": a, "edge": "tail", "to": "25f", "ripple": false}),
        ));
        let closed = f.ok(req("compound.close", json!({})));
        assert!(closed.warnings.is_empty());
        let p = f.editor.project();
        assert_eq!(
            p.tracks[0].transitions.iter().filter(|x| !x.sound).count(),
            1
        );
        assert_eq!(
            p.clip(&hosts[0]).unwrap().compound.as_ref().unwrap().frames,
            50
        );
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn opening_and_closing_wider_range_pooled_compound_keeps_original_frames() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 20);
        let made = f.ok(req("media.compound", json!({"range": "0f..50f"})));
        let pool = made.result["compound"].as_str().unwrap().to_string();
        assert_eq!(f.editor.project().compounds[0].frames(), 50);
        f.ok(req("compound.open", json!({"compound": pool})));
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert_eq!(p.compounds[0].frames(), 50);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn timeline_import_append_refuses_compound_holding_open_compound() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        let doc = crate::otio::export(
            f.editor.project(),
            "T1",
            None,
            crate::otio::Locations::Absolute,
        )
        .unwrap();
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        let e = f.err(req(
            "timeline.import",
            json!({"otio": doc, "mode": "append"}),
        ));
        assert!(e.message.contains("hold itself"));
    }

    #[test]
    fn open_clip_refuses_already_open_sequence_id() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        let mut inner = f.editor.project().clip(&hosts[0]).unwrap().clone();
        inner.id = "c99".into();
        inner.start = 40;
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        let mut proj = f.editor.project().clone();
        proj.tracks[0].clips.push(inner);
        let e = super::open_clip(&mut proj, "c99", None).unwrap_err();
        assert!(e.message.contains("already open"));
    }

    #[test]
    fn expanded_compound_adds_inner_and_outer_track_volume() {
        let mut f = Fixture::new(&[]);
        let mut info =
            crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true);
        info.video = None;
        f.env = crate::test_support::FakeEnv::default().with("/media/a.wav", info);
        f.ok(req("media.add", json!({"paths": ["/media/a.wav"]})));
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req("track.set", json!({"track": "A1", "volume_db": -6.0})));
        f.ok(req("compound.close", json!({})));
        f.ok(req("track.set", json!({"track": "A1", "volume_db": -3.0})));
        let plan = crate::render::flatten(f.editor.project(), 0, 10).unwrap();
        let crate::render::AudioPiece::Source {
            gain_db, silent, ..
        } = &plan.audio[0][0]
        else {
            panic!("expected audio source");
        };
        assert!(!silent);
        assert!((gain_db + 9.0).abs() < 1e-9);
    }

    #[test]
    fn expanded_compound_keeps_silent_when_inner_fader_is_at_bottom() {
        let mut f = Fixture::new(&[]);
        let mut info =
            crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true);
        info.video = None;
        f.env = crate::test_support::FakeEnv::default().with("/media/a.wav", info);
        f.ok(req("media.add", json!({"paths": ["/media/a.wav"]})));
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req(
            "track.set",
            json!({"track": "A1", "volume_db": -100.0}),
        ));
        f.ok(req("compound.close", json!({})));
        f.ok(req("track.set", json!({"track": "A1", "volume_db": 6.0})));
        let plan = crate::render::flatten(f.editor.project(), 0, 10).unwrap();
        let crate::render::AudioPiece::Source {
            gain_db, silent, ..
        } = &plan.audio[0][0]
        else {
            panic!("expected audio source");
        };
        assert!(*silent);
        assert!((gain_db + 100.0).abs() < 1e-9);
    }

    #[test]
    fn expanded_compound_honors_inner_track_solo() {
        let mut f = Fixture::new(&[]);
        let mut info =
            crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true);
        info.video = None;
        f.env = crate::test_support::FakeEnv::default()
            .with("/media/a1.wav", info.clone())
            .with("/media/a2.wav", info);
        f.ok(req(
            "media.add",
            json!({"paths": ["/media/a1.wav", "/media/a2.wav"]}),
        ));
        f.ok(req("track.add", json!({"kind": "audio"})));
        let a = f.append("m1", 0, 30);
        let b = f
            .ok(req(
                "edit.append",
                json!({"media": "m2", "track": "A2", "range": "0f..30f"}),
            ))
            .result["clip"]
            .as_str()
            .unwrap()
            .to_string();
        let hosts = nest(&mut f, &[&a, &b]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req("track.set", json!({"track": "A1", "solo": true})));
        f.ok(req("compound.close", json!({})));
        let plan = crate::render::flatten(f.editor.project(), 0, 10).unwrap();
        assert_eq!(plan.audio.len(), 1);
        assert!(matches!(
            &plan.audio[0][0],
            crate::render::AudioPiece::Source { media, silent: false, .. } if media == "m1"
        ));
    }

    #[test]
    fn steps_since_sees_edits_inside_open_compound_before_close() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        let before = f.editor.project().clone();
        f.ok(req("picture.set", json!({"clips": [a], "zoom": 1.5})));
        let s = crate::steps::since(&before, f.editor.project());
        assert_eq!(s.clips.changed, 1);
    }

    #[test]
    fn compound_open_and_project_set_give_specific_error_messages() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        let e1 = f.err(req("compound.open", json!({})));
        assert_eq!(e1.message, "give `clip` or `compound`");
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        let e2 = f.err(req("project.set", json!({"fps": "30"})));
        assert_eq!(
            e2.message,
            "close the open compound clip before changing the project frame rate"
        );
    }

    #[test]
    fn new_compound_defaults_to_zero_start_tc() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.ok(req("timeline.set", json!({"start_tc": "01:00:00:00"})));
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        assert_eq!(f.editor.project().start_tc(), 0);
        assert_eq!(f.editor.project().timecode(0), "00:00:00:00");
    }

    #[test]
    fn new_compound_stores_custom_start_tc_and_opens_at_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let r = f.ok(req(
            "edit.compound",
            json!({"clips": [a], "start_tc": "01:02:03:04"}),
        ));
        let hosts: Vec<String> = serde_json::from_value(r.result["clips"].clone()).unwrap();
        let opened = f.ok(req("compound.open", json!({"clip": hosts[0]})));
        assert_eq!(opened.result["start_tc"], json!("01:02:03:04"));
        assert_eq!(f.editor.project().timecode(0), "01:02:03:04");
        assert_eq!(f.editor.project().compounds[0].start_tc(), 93079);
    }

    #[test]
    fn edit_compound_range_stores_custom_start_tc() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 50);
        let r = f.ok(req(
            "edit.compound_range",
            json!({"range": "10f..40f", "start_tc": "02:00:00:00"}),
        ));
        let hosts: Vec<String> = serde_json::from_value(r.result["clips"].clone()).unwrap();
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        assert_eq!(f.editor.project().timecode(0), "02:00:00:00");
    }

    #[test]
    fn media_compound_stores_custom_start_tc() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 50);
        let r = f.ok(req(
            "media.compound",
            json!({"range": "10f..40f", "start_tc": "03:00:00:00"}),
        ));
        let kid = r.result["compound"].as_str().unwrap().to_string();
        f.ok(req("compound.open", json!({"compound": kid})));
        assert_eq!(f.editor.project().timecode(0), "03:00:00:00");
    }

    #[test]
    fn compound_set_from_media_pool_updates_placed_copies_and_keeps_trims() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req(
            "edit.place_compound",
            json!({"compound": kid, "at": 60}),
        ));
        let placed_id = f.editor.project().tracks[0].clips[1].id.clone();
        f.ok(req(
            "edit.trim",
            json!({"clip": &placed_id, "edge": "head", "by": 10, "ripple": false}),
        ));
        let r = f.ok(req(
            "compound.set",
            json!({"compound": kid, "start_tc": "05:00:00:00"}),
        ));
        assert_eq!(r.result["start_tc"], json!("05:00:00:00"));
        assert_eq!(f.editor.project().compounds[0].start_tc(), 450_000);
        assert_eq!(f.editor.project().tracks[0].clips[1].start, 70);
        assert_eq!(f.editor.project().tracks[0].clips[1].source_in, 10);
        f.ok(req("compound.open", json!({"clip": &placed_id})));
        assert_eq!(f.editor.project().timecode(0), "05:00:00:00");
    }

    #[test]
    fn compound_set_while_open_updates_ruler_and_undoes_cleanly() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let hosts = nest(&mut f, &[&a]);
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        f.ok(req("compound.set", json!({"start_tc": "04:00:00:00"})));
        assert_eq!(f.editor.project().timecode(0), "04:00:00:00");
        f.ok(req("history.undo", json!({})));
        assert_eq!(f.editor.project().timecode(0), "00:00:00:00");
        f.ok(req("history.redo", json!({})));
        assert_eq!(f.editor.project().timecode(0), "04:00:00:00");
        f.ok(req("compound.close", json!({})));
        assert_eq!(
            f.editor
                .project()
                .clip(&hosts[0])
                .unwrap()
                .compound
                .as_ref()
                .unwrap()
                .start_tc,
            360_000
        );
    }

    #[test]
    fn compound_set_updates_already_open_compound_when_targeted_by_pool_id() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req("compound.open", json!({"compound": kid})));
        f.ok(req(
            "compound.set",
            json!({"compound": kid, "start_tc": "01:00:00:00"}),
        ));
        assert_eq!(f.editor.project().timecode(0), "01:00:00:00");
        f.ok(req("compound.close", json!({})));
        assert_eq!(f.editor.project().compounds[0].start_tc(), 90_000);
    }

    #[test]
    fn compound_start_tc_rejects_negative_values() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let err = f.err(req("edit.compound", json!({"clips": [a], "start_tc": -1})));
        assert_eq!(err.message, "a start timecode is 00:00:00:00 or later");
    }

    #[test]
    fn compound_start_tc_accepts_drop_frame_labels_at_ntsc() {
        let mut f = Fixture::new(&[]);
        let ntsc = crate::time::Rational::new(30000, 1001).unwrap();
        let info = crate::test_support::media_info(4.0, ntsc, true);
        f.env = crate::test_support::FakeEnv::default().with("/media/ntsc.mp4", info);
        f.ok(req(
            "timeline.set",
            json!({"own": {"fps": "30000/1001", "drop_frame": true}}),
        ));
        f.ok(req("media.add", json!({"paths": ["/media/ntsc.mp4"]})));
        let a = f.append("m1", 0, 30);
        let hosts = f
            .ok(req(
                "edit.compound",
                json!({"clips": [a], "start_tc": "01:02:03;04"}),
            ))
            .result["clips"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        f.ok(req("compound.open", json!({"clip": hosts[0]})));
        assert_eq!(f.editor.project().timecode(0), "01:02:03;04");
        let bad = f.err(req("compound.set", json!({"start_tc": "01:01:00;00"})));
        assert!(bad.message.contains("drop-frame"));
    }

    #[test]
    fn compound_start_tc_relabels_when_placed_on_different_fps_timeline() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 25);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req(
            "compound.set",
            json!({"compound": kid, "start_tc": "01:00:00:00"}),
        ));
        f.ok(req(
            "timeline.new",
            json!({"name": "30fps", "own": {"fps": "30"}}),
        ));
        let r = f.ok(req("edit.place_compound", json!({"compound": kid})));
        let clip_id = r.result["clips"][0].as_str().unwrap().to_string();
        assert_eq!(
            f.editor
                .project()
                .clip(&clip_id)
                .unwrap()
                .compound
                .as_ref()
                .unwrap()
                .start_tc,
            108_000
        );
    }
}
