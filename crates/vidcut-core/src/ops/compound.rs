use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::{
    Source, block_clip, clear_range, ensure_track, fix_audio, linked, locate, split_at, tracks,
    unlocked,
};
use crate::error::{Error, Result};
use crate::model::{
    Clip, Compound, CompoundTarget, Marker, Marks, OpenCompound, Project, Settings, Track,
    TrackClip, TrackKind,
};
use crate::otio;
use crate::time::{Rational, TimeSpec, format_timecode, rescale};

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

pub(crate) fn picture_tracks(whole: &[Track]) -> Vec<Track> {
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

pub(crate) fn sound_tracks(whole: &[Track]) -> Vec<Track> {
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

pub(crate) fn end_of(whole: &[Track]) -> i64 {
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
    build(p, &ids, span, name, bin, start_tc)
}

fn checked_bin(p: &Project, bin: &str) -> Result<()> {
    if bin.is_empty() || p.bins.iter().any(|b| b == bin) {
        return Ok(());
    }
    Err(Error::not_found(format!("no bin {bin}")))
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
    let mut made = copy.compounds.pop().expect("a nest keeps a copy");
    let id = p.alloc_compound_id();
    made.id = id.clone();
    p.compounds.push(made);
    Ok((id, nested.warnings))
}

pub fn remove_pooled(p: &mut Project, id: &str) -> Result<()> {
    if p.pooled_compound(id).is_none() {
        return Err(Error::not_found(format!("no compound clip {id}")));
    }
    if p.compound_path
        .first()
        .is_some_and(|c| matches!(&c.target, CompoundTarget::Pooled { compound } if compound == id))
    {
        close_all(p)?;
    }
    if let Some(k) = p.compound_mut(id) {
        k.removed = true;
        k.bin.clear();
    }
    Ok(())
}

pub(crate) fn with_sound(p: &mut Project, ids: Vec<String>, src: &Source) -> Result<Vec<String>> {
    if src.compound.is_none() || src.video_only {
        return Ok(ids);
    }
    let mut out = ids.clone();
    for id in &ids {
        let (ti, ci) = locate(p, id)?;
        let c = &p.tracks[ti].clips[ci];
        if !c.video || !p.clip_compound(c).is_some_and(Compound::has_sound) {
            continue;
        }
        let mut sound = c.clone();
        let audio = format!("A{}", &p.tracks[ti].id[1..]);
        let ai = ensure_track(p, &audio)?;
        unlocked(p, ai)?;
        clear_range(p, ai, sound.start, sound.end());
        fix_audio(p, ai);
        sound.id = p.alloc_clip_id();
        sound.video = false;
        out.push(sound.id.clone());
        p.tracks[ai].clips.push(sound);
        p.tracks[ai].sort();
    }
    Ok(out)
}

pub(crate) fn landed(p: &mut Project, ids: Vec<String>, src: &Source) -> Result<Vec<String>> {
    let audio = p.tracks_of(TrackKind::Audio).len();
    let ids = with_sound(p, ids, src)?;
    if !src.decompose {
        return Ok(ids);
    }
    let mut made = Vec::new();
    for id in &ids {
        if p.clip(id).is_some_and(|c| c.compound.is_some()) {
            made.extend(unnest(p, id, Lay::OnEdit, &mut Vec::new())?.1);
        }
    }
    while p.tracks_of(TrackKind::Audio).len() > audio
        && (p.tracks.last()).is_some_and(|t| t.kind == TrackKind::Audio && t.clips.is_empty())
    {
        p.tracks.pop();
    }
    if made.is_empty() {
        return Err(Error::invalid(
            "the marked part of the compound clip holds no clips",
        ));
    }
    Ok(made)
}

pub fn pooled_id(p: &Project, query: &str) -> Result<String> {
    p.pool_compounds()
        .find(|x| x.id == query)
        .or_else(|| p.pool_compounds().find(|x| x.name == query))
        .map(|x| x.id.clone())
        .ok_or_else(|| Error::not_found(format!("no compound clip {query:?}")))
}

pub fn set_marks(
    p: &mut Project,
    id: &str,
    mark_in: Option<Option<i64>>,
    mark_out: Option<Option<i64>>,
) -> Result<Marks> {
    let k = p
        .pooled_compound(id)
        .ok_or_else(|| Error::not_found(format!("no compound clip {id}")))?;
    let open = p.checked_out_compound() == Some(id);
    let frames = if open {
        k.frames.max(p.end())
    } else {
        k.frames
    };
    let held = p.compound_sheet(k).marks;
    let marks = super::merged_marks(held, mark_in, mark_out, frames, &k.name)?;
    match open {
        true => p.marks = marks.clone(),
        false => p.compound_mut(id).expect("found above").marks = marks.clone(),
    }
    Ok(marks)
}

pub fn marked(p: &Project, k: &Compound) -> (i64, i64) {
    let marks = p.compound_sheet(k).marks;
    (
        marks.mark_in.unwrap_or(0),
        marks.mark_out.unwrap_or(k.frames),
    )
}

pub fn host_span(p: &Project, k: &Compound, (a, b): (i64, i64)) -> (i64, i64) {
    let to = p.fps();
    (
        rescale(a, k.fps, to),
        rescale(b, k.fps, to).min(p.compound_frames(k)),
    )
}

pub fn placeable(p: &Project, id: &str) -> Result<i64> {
    let item = p
        .pooled_compound(id)
        .ok_or_else(|| Error::not_found(format!("no compound clip {id}")))?;
    let open_ids: HashSet<String> = p.compound_path.iter().map(|c| c.compound.clone()).collect();
    let open_seq = open_sequence_ids(p);
    if (!open_ids.is_empty() || !open_seq.is_empty())
        && holds_compound(p, &item.id, &open_ids, &open_seq)
    {
        return Err(Error::invalid("a compound clip cannot hold itself"));
    }
    if item.frames < 1 {
        return Err(Error::invalid("compound clip is empty"));
    }
    let frames = p.compound_frames(item);
    if frames < 1 {
        return Err(Error::invalid(
            "compound clip is shorter than one timeline frame",
        ));
    }
    Ok(frames)
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

fn cut_every_track(p: &mut Project, points: [i64; 2]) {
    for ti in 0..p.tracks.len() {
        for at in points {
            split_at(p, ti, at);
            end_sound_at(&mut p.tracks[ti], at);
        }
        fix_audio(p, ti);
    }
    for at in points {
        super::subtitles::cut_all(p, at);
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
    cut_every_track(p, [a, b]);
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
    build(p, &ids, (a, b), name, bin, start_tc)
}

fn build(
    p: &mut Project,
    ids: &HashSet<String>,
    span: (i64, i64),
    name: Option<&str>,
    bin: &str,
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
    let (picture, sound) = (picture_tracks(&whole), sound_tracks(&whole));
    let cid = p.alloc_compound_id();
    let seq_id = otio::stack_id(&json!({
        "id": cid,
        "name": name,
        "frames": frames,
        "tracks": whole,
    }));
    let mut clips = Vec::new();
    let mut put = |p: &mut Project, track: String, video: bool| {
        let mut host = block_clip(p.alloc_clip_id(), (span.0, frames), None, None);
        host.video = video;
        host.compound = Some(cid.clone());
        clips.push(host.id.clone());
        let t = p.track_mut(&track).expect("a track of the project");
        t.clips.push(host);
        t.sort();
    };
    if !picture.is_empty() {
        let lowest = lowest_picture.expect("a picture comes from a video track");
        let track = picture_target(p, lowest, span);
        put(p, track, true);
    }
    if !sound.is_empty() {
        let track = sound_target(p, lowest_sound, span);
        put(p, track, false);
    }
    let fps = p.fps();
    let drop_frame = p.counts_drop_frame();
    p.compounds.push(Compound {
        id: cid,
        name,
        bin: bin.to_string(),
        removed: false,
        sequence_id: seq_id,
        fps,
        drop_frame,
        start_tc,
        frames,
        tracks: whole,
        marks: Marks::default(),
        markers: Vec::new(),
        kept_otio: None,
    });
    let mut warnings = Vec::new();
    let dropped = otio::transitions::prune(p);
    if dropped > 0 {
        warnings.push(otio::transitions::dropped(dropped));
    }
    Ok(Nested { clips, warnings })
}

pub fn leads(p: &Project, ids: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for id in ids {
        let compound = p.clip(id).is_some_and(|c| c.compound.is_some());
        if compound && otio::tied(p, id).iter().any(|x| kept.contains(x)) {
            continue;
        }
        kept.push(id.clone());
    }
    kept
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lay {
    InPlace,
    OnEdit,
}

pub(crate) fn target(
    p: &mut Project,
    kind: TrackKind,
    base: &str,
    n: usize,
    span: (i64, i64),
    lay: Lay,
) -> Result<usize> {
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
    match lay {
        Lay::OnEdit => unlocked(p, ti)?,
        Lay::InPlace if !free(&p.tracks[ti], span, range) => {
            tracks::insert_at(p, kind, ti);
        }
        Lay::InPlace => {}
    }
    Ok(ti)
}

fn put_back(
    p: &mut Project,
    (ti, lay): (usize, Lay),
    inner: &Track,
    block: &Clip,
    made: &mut Vec<String>,
) {
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
        if lay == Lay::OnEdit {
            clear_range(p, ti, y.start, y.end());
        }
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
    lay: Lay,
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
    let cid = block.compound.as_deref().expect("a compound clip");
    let mut k = p.compound(cid).expect("compound exists").clone();
    if let Some(r) = super::conform::Rates::new(k.fps, p.fps()) {
        for t in &mut k.tracks {
            super::conform::track(p, t, r);
        }
    }
    let source = if hosts.len() > 1 {
        k.tracks
    } else if block.video {
        picture_tracks(&k.tracks)
    } else {
        sound_tracks(&k.tracks)
    };
    let span = (block.start, block.end());
    let mut made = Vec::new();
    for inner in source.iter().filter(|t| !t.clips.is_empty()) {
        let video = inner.kind == TrackKind::Video;
        let Some((base, _)) = hosts.iter().find(|(_, c)| c.video == video) else {
            continue;
        };
        let ti = target(p, inner.kind, base, number(inner) - 1, span, lay)?;
        put_back(p, (ti, lay), inner, &block, &mut made);
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
        .filter_map(|oc| p.compound(&oc.compound).map(|k| k.sequence_id.clone()))
        .collect()
}

fn holds_compound_inner(
    p: &Project,
    cid: &str,
    open_ids: &HashSet<String>,
    open_seq: &HashSet<String>,
    seen: &mut HashSet<String>,
) -> bool {
    if open_ids.contains(cid) || !seen.insert(cid.to_string()) {
        return open_ids.contains(cid);
    }
    let Some(k) = p.compound(cid) else {
        return false;
    };
    if open_seq.contains(&k.sequence_id)
        || k.kept_otio
            .as_ref()
            .is_some_and(|s| otio::stack_holds_any(s, open_seq))
    {
        return true;
    }
    p.compound_tracks(k)
        .iter()
        .flat_map(|t| &t.clips)
        .filter_map(|c| c.compound.as_deref())
        .any(|inner| holds_compound_inner(p, inner, open_ids, open_seq, seen))
}

pub(crate) fn holds_compound(
    p: &Project,
    cid: &str,
    open_ids: &HashSet<String>,
    open_seq: &HashSet<String>,
) -> bool {
    holds_compound_inner(p, cid, open_ids, open_seq, &mut HashSet::new())
}

pub(crate) fn check_no_cycle(p: &Project, clips: &[crate::model::TrackClip]) -> Result<()> {
    let open_ids: HashSet<String> = p.compound_path.iter().map(|c| c.compound.clone()).collect();
    let open_seq = open_sequence_ids(p);
    if open_ids.is_empty() && open_seq.is_empty() {
        return Ok(());
    }
    if clips
        .iter()
        .filter_map(|tc| tc.clip.compound.as_deref())
        .any(|cid| holds_compound(p, cid, &open_ids, &open_seq))
    {
        return Err(Error::invalid("a compound clip cannot hold itself"));
    }
    Ok(())
}

pub(crate) fn check_clips_no_cycle(p: &Project, clip_ids: &[String]) -> Result<()> {
    let open_ids: HashSet<String> = p.compound_path.iter().map(|c| c.compound.clone()).collect();
    let open_seq = open_sequence_ids(p);
    if open_ids.is_empty() && open_seq.is_empty() {
        return Ok(());
    }
    let hit = clip_ids
        .iter()
        .filter_map(|id| p.clip(id).and_then(|c| c.compound.as_deref()))
        .any(|cid| holds_compound(p, cid, &open_ids, &open_seq));
    if hit {
        return Err(Error::invalid("a compound clip cannot hold itself"));
    }
    Ok(())
}

pub(crate) fn check_imported_no_cycle(p: &Project, clips: &[otio::ImportedClip]) -> Result<()> {
    let open_ids: HashSet<String> = p.compound_path.iter().map(|c| c.compound.clone()).collect();
    let open_seq = open_sequence_ids(p);
    if open_ids.is_empty() && open_seq.is_empty() {
        return Ok(());
    }
    let hit = clips.iter().filter_map(|c| c.compound.as_ref()).any(|k| {
        k.compound_id
            .as_deref()
            .is_some_and(|id| open_ids.contains(id))
            || open_seq.contains(&k.sequence_id)
            || otio::stack_holds_any(&k.stack, &open_seq)
    });
    if hit {
        return Err(Error::invalid("a compound clip cannot hold itself"));
    }
    Ok(())
}

pub(crate) fn copied_compounds(p: &Project, clips: &[TrackClip]) -> Vec<Compound> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut pending: Vec<String> = clips
        .iter()
        .filter_map(|tc| tc.clip.compound.clone())
        .collect();
    while let Some(cid) = pending.pop() {
        if !seen.insert(cid.clone()) {
            continue;
        }
        let Some(k) = p.compound(&cid) else {
            continue;
        };
        let sheet = p.compound_sheet(k);
        for c in sheet.tracks.iter().flat_map(|t| &t.clips) {
            if let Some(child) = &c.compound {
                pending.push(child.clone());
            }
        }
        let mut copy = k.clone();
        copy.tracks = sheet.tracks.to_vec();
        copy.marks = sheet.marks.clone();
        out.push(copy);
    }
    out
}

fn check_cycle_from<'a>(
    p: &'a Project,
    cid: &'a str,
    state: &mut HashMap<&'a str, u8>,
) -> Result<()> {
    match state.get(cid).copied() {
        Some(1) => return Err(Error::invalid("a compound clip cannot hold itself")),
        Some(_) => return Ok(()),
        None => {}
    }
    state.insert(cid, 1);
    if let Some(k) = p.compound(cid) {
        for c in p.compound_tracks(k).iter().flat_map(|t| &t.clips) {
            if let Some(inner) = &c.compound {
                check_cycle_from(p, inner, state)?;
            }
        }
    }
    state.insert(cid, 2);
    Ok(())
}

pub(crate) fn restore_copied(
    p: &mut Project,
    clips: &mut [TrackClip],
    compounds: &[Compound],
) -> Result<()> {
    if compounds.is_empty() {
        return Ok(());
    }
    let mut remap: HashMap<String, String> = HashMap::new();
    let mut added = Vec::new();
    for k in compounds {
        if remap.contains_key(&k.id) {
            continue;
        }
        if k.sequence_id.trim().is_empty() {
            return Err(Error::invalid("compound clip sequence_id is empty"));
        }
        let existing = p
            .compound(&k.id)
            .filter(|e| e.sequence_id == k.sequence_id)
            .or_else(|| p.compounds.iter().find(|e| e.sequence_id == k.sequence_id));
        if let Some(e) = existing {
            remap.insert(k.id.clone(), e.id.clone());
            continue;
        }
        if k.id.is_empty() {
            return Err(Error::invalid("compound clip id is empty"));
        }
        if k.drop_frame && !k.fps.has_drop_frame() {
            return Err(Error::invalid(format!(
                "compound {} counts drop-frame at {}",
                k.id, k.fps
            )));
        }
        if k.start_tc < 0 {
            return Err(Error::invalid("compound clip starts before 00:00:00:00"));
        }
        if k.frames < 0 {
            return Err(Error::invalid(format!(
                "compound {} has negative length {}",
                k.id, k.frames
            )));
        }
        let new_id = p.alloc_compound_id();
        let mut copy = k.clone();
        copy.id = new_id.clone();
        copy.removed = true;
        copy.bin.clear();
        p.compounds.push(copy);
        remap.insert(k.id.clone(), new_id.clone());
        added.push(new_id);
    }
    for id in &added {
        let k = p.compound_mut(id).expect("added above");
        for c in k.tracks.iter_mut().flat_map(|t| &mut t.clips) {
            if let Some(cid) = c.compound.as_mut()
                && let Some(new_id) = remap.get(cid.as_str())
            {
                cid.clone_from(new_id);
            }
        }
    }
    for tc in clips.iter_mut() {
        if let Some(cid) = tc.clip.compound.as_mut()
            && let Some(new_id) = remap.get(cid.as_str())
        {
            cid.clone_from(new_id);
        }
    }
    if added.is_empty() {
        return Ok(());
    }
    for id in &added {
        let k = p.compound(id).expect("added above");
        for c in k.tracks.iter().flat_map(|t| &t.clips) {
            if let Some(cid) = &c.compound {
                let child = p
                    .compound(cid)
                    .ok_or_else(|| Error::invalid(format!("{}: unknown compound {cid}", c.id)))?;
                let (lo, hi) = c.shown();
                let frames = crate::time::rescale(child.frames, child.fps, k.fps);
                if lo < 0 || hi > frames {
                    return Err(Error::invalid(format!(
                        "{}: source {lo}..{hi} outside its compound (0..{frames})",
                        c.id
                    )));
                }
            } else if !c.is_block() && p.media(&c.media).is_none() {
                return Err(Error::not_found(format!(
                    "no media {} in this project",
                    c.media
                )));
            }
        }
    }
    let mut state = HashMap::new();
    for id in &added {
        check_cycle_from(p, id, &mut state)?;
    }
    Ok(())
}

pub fn breadcrumbs(p: &Project) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(t) = p.timeline() {
        out.push(json!({ "kind": "timeline", "id": t.id, "name": t.name }));
    }
    for (i, oc) in p.compound_path.iter().enumerate() {
        let name = p.compound(&oc.compound).map_or("", |k| k.name.as_str());
        let mut entry = json!({ "kind": "compound", "depth": i, "name": name });
        match &oc.target {
            CompoundTarget::Clip { clip } => entry["clip"] = json!(clip),
            CompoundTarget::Pooled { compound } => entry["compound"] = json!(compound),
        }
        out.push(entry);
    }
    out
}

pub fn path_json(p: &Project) -> Value {
    json!({
        "name": p.open_compound().map_or("", |k| k.name.as_str()),
        "fps": p.fps(),
        "start_tc": p.timecode(0),
        "path": breadcrumbs(p),
    })
}

pub(crate) fn otio_has_items(v: &Value) -> bool {
    v["children"]
        .as_array()
        .into_iter()
        .flatten()
        .any(otio_node_has_items)
}

fn otio_node_has_items(v: &Value) -> bool {
    let s = v["OTIO_SCHEMA"].as_str().unwrap_or("");
    if s.starts_with("Clip.") || s.starts_with("Stack.") {
        return true;
    }
    otio_has_items(v)
}

fn collect_ids(
    p: &Project,
    skip_compound: Option<&str>,
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
    let open_tl = p.checked_out_timeline();
    for t in &p.timelines {
        if Some(t.id.as_str()) == open_tl {
            continue;
        }
        scan(&t.tracks);
        markers.extend(t.markers.iter().map(|k| k.id.clone()));
    }
    let open_k = p.checked_out_compound();
    for k in &p.compounds {
        if Some(k.id.as_str()) == open_k || Some(k.id.as_str()) == skip_compound {
            continue;
        }
        scan(&k.tracks);
        markers.extend(k.markers.iter().map(|m| m.id.clone()));
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
    skip_compound: &str,
    raw_tracks: Vec<Track>,
    raw_markers: Vec<Marker>,
) -> (Vec<Track>, Vec<Marker>) {
    let (mut used_clips, mut used_transitions, mut used_markers) =
        (HashSet::new(), HashSet::new(), HashSet::new());
    collect_ids(
        p,
        Some(skip_compound),
        &mut used_clips,
        &mut used_transitions,
        &mut used_markers,
    );

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

fn push_open(p: &mut Project, cid: String, target: CompoundTarget, missing: &str) -> Result<Value> {
    if p.compound_path.iter().any(|oc| oc.compound == cid) {
        return Err(Error::invalid("that compound clip is already open"));
    }
    let k = p
        .compound(&cid)
        .cloned()
        .ok_or_else(|| Error::not_found(format!("no compound clip {cid}")))?;
    if open_sequence_ids(p).contains(&k.sequence_id) {
        return Err(Error::invalid("that compound clip is already open"));
    }
    if k.tracks.is_empty() && k.kept_otio.as_ref().is_some_and(|s| otio_has_items(s)) {
        return Err(Error::invalid(format!("{missing}'s clips weren't found")));
    }
    let (tracks, markers) = prepare_open_tracks(p, &cid, k.tracks, k.markers);
    if let Some(k) = p.compound_mut(&cid) {
        k.tracks = tracks;
        k.markers = markers;
    }
    p.check_in();
    p.compound_path.push(OpenCompound {
        compound: cid,
        target,
    });
    p.check_out();
    Ok(path_json(p))
}

pub fn open_clip(p: &mut Project, id: &str) -> Result<Value> {
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
    let lead = p.clip(&lead_id).expect("located");
    let cid = lead.compound.clone().expect("checked");
    push_open(
        p,
        cid,
        CompoundTarget::Clip {
            clip: lead_id.clone(),
        },
        &lead_id,
    )
}

pub fn open_pooled(p: &mut Project, query: &str) -> Result<Value> {
    let id = pooled_id(p, query)?;
    let found = (
        id.clone(),
        p.compound(&id).map(|k| k.name.clone()).unwrap_or(id),
    );
    if p.compound_path.len() == 1
        && matches!(&p.compound_path[0].target, CompoundTarget::Pooled { compound } if compound == &found.0)
    {
        return Ok(path_json(p));
    }
    if !p.compound_path.is_empty() {
        close_all(p)?;
    }
    let cid = found.0.clone();
    push_open(
        p,
        cid,
        CompoundTarget::Pooled { compound: found.0 },
        &found.1,
    )
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
        .map(|k| k.name.as_str())
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

fn close_one(p: &mut Project) -> Result<()> {
    let cid = p
        .checked_out_compound()
        .map(str::to_string)
        .ok_or_else(|| Error::invalid("no compound clip is open"))?;
    let whole = normalize_tracks(std::mem::take(&mut p.tracks));
    let inner_frames = end_of(&whole);
    let empty = whole.is_empty();
    p.tracks = whole;
    p.marks = Marks::default();
    p.subtitles.clear();
    if let Some(k) = p.compound_mut(&cid) {
        k.frames = k.frames.max(inner_frames);
        if empty {
            k.kept_otio = None;
        }
    }
    p.check_in();
    p.compound_path.pop();
    p.check_out();
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

pub fn set(
    p: &mut Project,
    target: SetTarget<'_>,
    name: Option<&str>,
    tc: Option<&TimeSpec>,
) -> Result<Value> {
    if name.is_none() && tc.is_none() {
        return Err(Error::usage("give `name` or `start_tc`"));
    }
    if name.is_some_and(|n| n.trim().is_empty()) {
        return Err(Error::usage("a compound clip needs a name"));
    }
    let cid = match target {
        SetTarget::Open => p
            .checked_out_compound()
            .ok_or_else(|| Error::invalid("no compound clip is open"))?
            .to_string(),
        SetTarget::Pooled(query) => pooled_id(p, query)?,
        SetTarget::Clip(query_id) => {
            let (ti, ci) = locate(p, query_id)?;
            p.tracks[ti].clips[ci]
                .compound
                .clone()
                .ok_or_else(|| Error::invalid(format!("{query_id} isn't a compound clip")))?
        }
    };
    let k = p
        .compound(&cid)
        .ok_or_else(|| Error::not_found(format!("no compound clip {cid}")))?;
    let fmt = settings_with(&p.settings, k.fps, k.counts_drop_frame());
    let frames = match tc {
        Some(tc) => fmt.start_frames(tc)?,
        None => k.start_tc,
    };
    let k = p.compound_mut(&cid).expect("found above");
    k.start_tc = frames;
    if let Some(n) = name {
        k.name = n.to_string();
    }
    Ok(json!({
        "name": k.name,
        "start_tc": format_timecode(0, fmt.fps, frames, fmt.counts_drop_frame()),
        "frames": frames,
    }))
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
        assert_eq!(f.editor.project().compounds[0].frames, 30);
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
        let place = json!({"compound": id, "at": "45f", "use_marks": false});
        f.ok(req("edit.ripple_overwrite", place));
        assert_eq!(f.editor.project().clip(&above).unwrap().start, 50);
    }

    #[test]
    fn a_pooled_compound_clip_inserts_at_an_edit_and_moves_the_clips_after_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let _ = f.append("m1", 0, 30);
        let after = f.append("m1", 30, 60);
        f.ok(req("media.compound", json!({"range": "0f..12f"})));
        let id = f.editor.project().compounds[0].id.clone();
        let place = json!({"compound": id, "at": "30f"});
        f.ok(req("edit.insert", place));
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
        let r = f.ok(req("edit.append", json!({"compound": id})));
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
        let p = f.editor.project();
        let v2 = p.track("V2").unwrap();
        assert_eq!(p.clip_compound(&v2.clips[1]).unwrap().tracks[0].id, "V1");
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
        let p = f.editor.project();
        let host = p.clip(&hosts[0]).unwrap();
        let tracks = &p.clip_compound(host).unwrap().tracks;
        let shape: Vec<(&str, usize)> = tracks.iter().map(|t| (&*t.id, t.clips.len())).collect();
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
        let p = f.editor.project();
        let host = p.clip(&hosts[1]).unwrap();
        let sound_tracks = super::sound_tracks(&p.clip_compound(host).unwrap().tracks);
        let sound: &Clip = &sound_tracks[0].clips[0];
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
        let k = p.clip_compound(p.clip(&hosts[0]).unwrap()).unwrap();
        assert_eq!(k.tracks[0].clips[0].source_in, 5);
        assert_eq!(p.compounds[0].tracks[0].clips[0].source_in, 5);
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
        let k = p.clip_compound(outer).unwrap();
        assert_eq!(
            (outer.duration, k.frames, k.tracks[0].clips.len()),
            (50, 80, 2)
        );
        assert_eq!(
            k.tracks[0].clips[0].picture.as_ref().unwrap().zoom,
            Some([1.5, 1.5])
        );
        assert_eq!(k.tracks[0].clips[1].id, b);
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
        let k = p.clip_compound(outer).unwrap();
        assert_eq!(
            (outer.duration, k.frames, k.tracks[0].clips[0].duration),
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
            f.ok(req("edit.append", json!({"compound": pool}))).result["clips"].clone(),
        )
        .unwrap();
        let r = f.ok(req(
            "timeline.new",
            json!({"name": "T2", "own": {"fps": "50"}}),
        ));
        let _t2 = r.result["id"].as_str().unwrap().to_string();
        let on_t2: Vec<String> = serde_json::from_value(
            f.ok(req("edit.append", json!({"compound": pool}))).result["clips"].clone(),
        )
        .unwrap();
        f.ok(req("compound.open", json!({"clip": on_t2[0]})));
        f.append("m1", 0, 20);
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert_eq!(
            p.compound_frames(p.clip_compound(p.clip(&on_t2[0]).unwrap()).unwrap()),
            120
        );
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
        f.ok(req("timeline.open", json!({"timeline": "t1"})));
        let p = f.editor.project();
        assert_eq!(
            p.compound_frames(p.clip_compound(p.clip(&hosts[0]).unwrap()).unwrap()),
            60
        );
        assert_eq!(
            p.compound_frames(p.clip_compound(p.clip(&placed[0]).unwrap()).unwrap()),
            60
        );
        assert_eq!(p.compounds[0].frames, 60);
        assert_eq!(p.compounds[0].tracks[0].clips.len(), 2);
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
            p.clip_compound(p.clip(&hosts[0]).unwrap()).unwrap().frames,
            60
        );
        assert_eq!(
            p.clip_compound(p.clip(&hosts[1]).unwrap()).unwrap().frames,
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
        assert!(!p.compounds[0].has_picture());
        assert!(p.compounds[0].has_sound());
        assert_eq!(p.compounds[0].tracks[0].clips.len(), 2);
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
        let p = f.editor.project();
        let outer = p.clip(&hosts[0]).unwrap();
        assert_eq!(
            p.clip_compound(outer).unwrap().tracks[0].clips[0]
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
            p.compounds[0].tracks[0].clips[0]
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
        let k = p.clip_compound(p.clip(&nested_id).unwrap()).unwrap();
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
        let outer_k = p.clip_compound(p.clip(&outer[0]).unwrap()).unwrap();
        let ok_zoom = p.clip_compound(&outer_k.tracks[0].clips[0]).unwrap().tracks[0].clips[0]
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
        let e1 = f.err(req("edit.append", json!({"compound": pool})));
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
            p.clip_compound(p.clip(&hosts[0]).unwrap()).unwrap().tracks[0].clips[0]
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
            p.compounds[0].tracks[0].clips[0]
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
        assert_eq!(p.pool_compounds().count(), 0);
        assert!(p.compounds[0].removed);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn nested_copy_updates_even_when_clip_id_matches_top_level_clip() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let x_hosts = nest(&mut f, &[&a]);
        let pool_x = f.editor.project().compounds[0].id.clone();
        let x_placed: Vec<String> = serde_json::from_value(
            f.ok(req("edit.append", json!({"compound": pool_x}))).result["clips"].clone(),
        )
        .unwrap();
        let y_hosts = nest(&mut f, &[&x_placed[0]]);
        f.ok(req("edit.delete", json!({"clips": x_hosts})));
        f.ok(req("edit.decompose", json!({"clips": [y_hosts[0]]})));
        f.ok(req("compound.open", json!({"clip": x_placed[0]})));
        f.append("m1", 30, 50);
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        let nested_x = p.clip_compound(&p.compounds[1].tracks[0].clips[0]).unwrap();
        assert_eq!(nested_x.frames, 50);
        assert_eq!(nested_x.tracks[0].clips.len(), 2);
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
            p.clip_compound(p.clip(&hosts[0]).unwrap()).unwrap().frames,
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
        assert_eq!(f.editor.project().compounds[0].frames, 50);
        f.ok(req("compound.open", json!({"compound": pool})));
        f.ok(req("compound.close", json!({})));
        let p = f.editor.project();
        assert_eq!(p.compounds[0].frames, 50);
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
        let e = super::open_clip(&mut proj, "c99").unwrap_err();
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
        assert_eq!(f.editor.project().compounds[0].start_tc, 93079);
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

    fn ntsc_fixture(drop_frame: bool) -> Fixture {
        let mut f = Fixture::new(&[]);
        let ntsc = crate::time::Rational::new(30000, 1001).unwrap();
        let info = crate::test_support::media_info(4.0, ntsc, true);
        f.env = crate::test_support::FakeEnv::default().with("/media/ntsc.mp4", info);
        f.ok(req(
            "timeline.set",
            json!({"own": {"fps": "30000/1001", "drop_frame": drop_frame}}),
        ));
        f.ok(req("media.add", json!({"paths": ["/media/ntsc.mp4"]})));
        f
    }

    #[test]
    fn compound_set_from_media_pool_updates_placed_copies() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        let placed = f.ok(req("edit.overwrite", json!({"compound": kid, "at": 60})));
        let placed_id = placed.result["clip"].as_str().unwrap().to_string();
        let r = f.ok(req(
            "compound.set",
            json!({"compound": kid, "start_tc": "05:00:00:00"}),
        ));
        assert_eq!(r.result["start_tc"], json!("05:00:00:00"));
        assert_eq!(f.editor.project().compounds[0].start_tc, 450_000);
        f.ok(req("compound.open", json!({"clip": placed_id})));
        assert_eq!(f.editor.project().timecode(0), "05:00:00:00");
    }

    #[test]
    fn compound_set_keeps_placed_position_and_trim() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        let placed = f.ok(req("edit.overwrite", json!({"compound": kid, "at": 50})));
        let placed_id = placed.result["clip"].as_str().unwrap().to_string();
        f.ok(req(
            "edit.trim",
            json!({"clip": &placed_id, "edge": "head", "by": 10, "ripple": false}),
        ));
        f.ok(req(
            "compound.set",
            json!({"compound": kid, "start_tc": "05:00:00:00"}),
        ));
        let clip = f.editor.project().clip(&placed_id).unwrap();
        assert_eq!((clip.start, clip.source_in), (60, 10));
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
        let p = f.editor.project();
        assert_eq!(
            p.clip_compound(p.clip(&hosts[0]).unwrap())
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
        assert_eq!(f.editor.project().compounds[0].start_tc, 90_000);
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
        let mut f = ntsc_fixture(true);
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
    fn compound_start_tc_preserves_canonical_rate_when_placed_on_different_fps_timeline() {
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
        let r = f.ok(req("edit.append", json!({"compound": kid})));
        let clip_id = r.result["clip"].as_str().unwrap().to_string();
        let p = f.editor.project();
        let k = p.clip_compound(p.clip(&clip_id).unwrap()).unwrap();
        assert_eq!(k.start_tc, 90_000);
        assert_eq!(k.fps.to_string(), "25");
        f.ok(req("compound.open", json!({"clip": clip_id})));
        assert_eq!(f.editor.project().timecode(0), "01:00:00:00");
    }

    #[test]
    fn compound_set_updates_nested_compound_start_tc_in_parent_otio_stack() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let inner_hosts = nest(&mut f, &[&a]);
        let inner_kid = f.editor.project().compounds[0].id.clone();
        nest(&mut f, &[&inner_hosts[0]]);
        f.ok(req(
            "compound.set",
            json!({"compound": inner_kid, "start_tc": "02:00:00:00"}),
        ));
        let doc = crate::otio::export(
            f.editor.project(),
            "T1",
            None,
            crate::otio::Locations::Absolute,
        )
        .unwrap();
        let mut f2 = Fixture::new(&[(4.0, true)]);
        f2.ok(req("timeline.import", json!({"otio": doc})));
        let outer_id = f2.editor.project().tracks[0].clips[0].id.clone();
        f2.ok(req("compound.open", json!({"clip": outer_id})));
        let imported_inner = f2.editor.project().tracks[0].clips[0].id.clone();
        f2.ok(req("compound.open", json!({"clip": imported_inner})));
        assert_eq!(f2.editor.project().timecode(0), "02:00:00:00");
    }

    #[test]
    fn compound_set_updates_nested_start_tc_inside_imported_otio_compound() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let inner_hosts = nest(&mut f, &[&a]);
        let inner_kid = f.editor.project().compounds[0].id.clone();
        nest(&mut f, &[&inner_hosts[0]]);
        f.ok(req("edit.append", json!({"compound": inner_kid})));
        let doc = crate::otio::export(
            f.editor.project(),
            "T1",
            None,
            crate::otio::Locations::Absolute,
        )
        .unwrap();
        let mut f2 = Fixture::new(&[(4.0, true)]);
        f2.ok(req("timeline.import", json!({"otio": doc})));
        let outer_id = f2.editor.project().tracks[0].clips[0].id.clone();
        let copy_id = f2.editor.project().tracks[0].clips[1].id.clone();
        let p2 = f2.editor.project();
        let outer_k = p2.clip_compound(p2.clip(&outer_id).unwrap()).unwrap();
        assert!(outer_k.removed && outer_k.kept_otio.is_some());
        f2.ok(req(
            "compound.set",
            json!({"clip": copy_id, "start_tc": "02:30:00:00"}),
        ));
        f2.ok(req("compound.open", json!({"clip": outer_id})));
        let inner_id = f2.editor.project().tracks[0].clips[0].id.clone();
        f2.ok(req("compound.open", json!({"clip": inner_id})));
        assert_eq!(f2.editor.project().timecode(0), "02:30:00:00");
    }

    #[test]
    fn pooled_drop_frame_compound_opens_as_drop_frame_from_non_drop_timeline() {
        let mut f = ntsc_fixture(true);
        let a = f.append("m1", 0, 30);
        f.ok(req(
            "edit.compound",
            json!({"clips": [a], "start_tc": "01:00:00;00"}),
        ));
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req(
            "timeline.new",
            json!({"name": "NDF", "own": {"fps": "30000/1001", "drop_frame": false}}),
        ));
        f.ok(req("compound.open", json!({"compound": kid})));
        assert_eq!(f.editor.project().timecode(0), "01:00:00;00");
    }

    #[test]
    fn placing_drop_frame_compound_on_non_drop_timeline_preserves_drop_frame() {
        let mut f = ntsc_fixture(true);
        let a = f.append("m1", 0, 30);
        f.ok(req(
            "edit.compound",
            json!({"clips": [a], "start_tc": "01:00:00;00"}),
        ));
        let kid = f.editor.project().compounds[0].id.clone();
        assert_eq!(f.editor.project().compounds[0].start_tc, 107_892);
        f.ok(req(
            "timeline.new",
            json!({"name": "NDF", "own": {"fps": "30000/1001", "drop_frame": false}}),
        ));
        let r = f.ok(req("edit.append", json!({"compound": kid})));
        let cid = r.result["clip"].as_str().unwrap().to_string();
        let p = f.editor.project();
        let k = p.clip_compound(p.clip(&cid).unwrap()).unwrap();
        assert_eq!((k.start_tc, k.drop_frame), (107_892, true));
        f.ok(req("compound.open", json!({"clip": cid})));
        assert_eq!(f.editor.project().timecode(0), "01:00:00;00");
    }

    #[test]
    fn placing_drop_frame_compound_on_different_fps_preserves_drop_frame() {
        let mut f = ntsc_fixture(true);
        let a = f.append("m1", 0, 30);
        f.ok(req(
            "edit.compound",
            json!({"clips": [a], "start_tc": "01:00:00;00"}),
        ));
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req(
            "timeline.new",
            json!({"name": "PAL", "own": {"fps": "25"}}),
        ));
        let r = f.ok(req("edit.append", json!({"compound": kid})));
        let cid = r.result["clip"].as_str().unwrap().to_string();
        let p = f.editor.project();
        let k = p.clip_compound(p.clip(&cid).unwrap()).unwrap();
        assert_eq!((k.start_tc, k.drop_frame), (107_892, true));
        f.ok(req("compound.open", json!({"clip": cid})));
        assert_eq!(f.editor.project().timecode(0), "01:00:00;00");
    }

    #[test]
    fn closing_compound_from_drop_frame_timeline_preserves_non_drop_start_tc() {
        let mut f = ntsc_fixture(false);
        let a = f.append("m1", 0, 30);
        let t1_hosts = f
            .ok(req(
                "edit.compound",
                json!({"clips": [a], "start_tc": "01:00:00:00"}),
            ))
            .result["clips"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        let kid = f.editor.project().compounds[0].id.clone();
        let t1_id = f.editor.project().open_id().unwrap().to_string();
        f.ok(req(
            "timeline.new",
            json!({"name": "DF", "own": {"fps": "30000/1001", "drop_frame": true}}),
        ));
        let placed = f.ok(req("edit.append", json!({"compound": kid})));
        let t2_clip = placed.result["clip"].as_str().unwrap().to_string();
        f.ok(req("compound.open", json!({"clip": t2_clip})));
        f.ok(req("compound.close", json!({})));
        let k = &f.editor.project().compounds[0];
        assert_eq!((k.start_tc, k.drop_frame), (108_000, false));
        f.ok(req("timeline.open", json!({"timeline": t1_id})));
        f.ok(req("compound.open", json!({"clip": t1_hosts[0]})));
        assert_eq!(f.editor.project().timecode(0), "01:00:00:00");
    }

    #[test]
    fn compound_set_from_pool_uses_compounds_own_drop_frame() {
        let mut f = ntsc_fixture(true);
        let a = f.append("m1", 0, 30);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req(
            "timeline.new",
            json!({"name": "NDF", "own": {"fps": "30000/1001", "drop_frame": false}}),
        ));
        let placed = f.ok(req("edit.append", json!({"compound": kid})));
        let ndf_clip = placed.result["clip"].as_str().unwrap().to_string();
        f.ok(req(
            "compound.set",
            json!({"compound": kid, "start_tc": "01:00:00;00"}),
        ));
        let p = f.editor.project();
        assert_eq!(p.compounds[0].start_tc, 107_892);
        assert_eq!(
            p.clip_compound(p.clip(&ndf_clip).unwrap())
                .unwrap()
                .start_tc,
            107_892
        );
        f.ok(req("compound.open", json!({"clip": ndf_clip})));
        assert_eq!(f.editor.project().timecode(0), "01:00:00;00");
    }

    #[test]
    fn removing_placed_compound_from_pool_keeps_playback_and_drops_when_last_copy_goes() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req("media.compound_remove", json!({"compound": kid})));
        let p = f.editor.project();
        assert_eq!(p.pool_compounds().count(), 0);
        assert!(p.compounds[0].removed);
        let plan = crate::render::flatten(p, 0, 40).unwrap();
        assert!(!matches!(
            plan.video.as_slice(),
            [crate::render::VideoPiece::Black { .. }]
        ));
        f.ok(req("edit.decompose", json!({"clips": [hosts[0]]})));
        let p = f.editor.project();
        assert!(p.compounds.is_empty());
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn cross_rate_compound_conforms_on_playback_and_decompose() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 25);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req(
            "timeline.new",
            json!({"name": "50fps", "own": {"fps": "50"}}),
        ));
        let placed = f.ok(req("edit.append", json!({"compound": kid})));
        let host = placed.result["clip"].as_str().unwrap().to_string();
        let p = f.editor.project();
        assert_eq!(p.clip(&host).unwrap().duration, 50);
        let plan = crate::render::flatten(p, 0, 50).unwrap();
        assert_eq!(plan.frames, 50);
        assert!(!matches!(
            plan.video.as_slice(),
            [crate::render::VideoPiece::Black { .. }]
        ));
        f.ok(req("edit.decompose", json!({"clips": [host]})));
        let p = f.editor.project();
        assert_eq!(p.tracks[0].clips.len(), 1);
        assert_eq!(p.tracks[0].clips[0].media, "m1");
        assert_eq!(p.tracks[0].clips[0].duration, 50);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn cut_and_paste_of_removed_compound_restores_definition() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let hosts = nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req("media.compound_remove", json!({"compound": kid})));
        let cut = f.ok(req("edit.cut", json!({"clips": [hosts[0]]})));
        assert!(f.editor.project().compounds.is_empty());
        let pasted = f.ok(req(
            "edit.paste",
            json!({
                "clips": cut.result["clips"],
                "compounds": cut.result["compounds"],
                "fps": cut.result["fps"],
                "at": 0,
            }),
        ));
        let p = f.editor.project();
        let pasted_id = pasted.result["clips"][0].as_str().unwrap().to_string();
        assert_eq!(p.compounds.len(), 1);
        assert!(p.compounds[0].removed && p.compounds[0].bin.is_empty());
        assert_ne!(p.compounds[0].id, kid);
        assert_eq!(
            p.clip(&pasted_id).unwrap().compound.as_deref(),
            Some(p.compounds[0].id.as_str())
        );
        assert_eq!(p.pool_compounds().count(), 0);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
        f.ok(req("history.undo", json!({})));
        f.ok(req("history.undo", json!({})));
        let p = f.editor.project();
        assert_eq!(p.compounds.len(), 1);
        assert_eq!(p.compounds[0].id, kid);
        assert!(p.clip(&hosts[0]).is_some());
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn cut_and_paste_of_nested_removed_compounds_restores_hierarchy() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let inner_hosts = nest(&mut f, &[&a]);
        let outer_hosts = nest(&mut f, &[&inner_hosts[0]]);
        let ids: Vec<String> = f
            .editor
            .project()
            .compounds
            .iter()
            .map(|k| k.id.clone())
            .collect();
        for kid in &ids {
            f.ok(req("media.compound_remove", json!({"compound": kid})));
        }
        let cut = f.ok(req("edit.cut", json!({"clips": [outer_hosts[0]]})));
        assert!(f.editor.project().compounds.is_empty());
        f.ok(req(
            "edit.paste",
            json!({
                "clips": cut.result["clips"],
                "compounds": cut.result["compounds"],
                "fps": cut.result["fps"],
                "at": 0,
            }),
        ));
        let p = f.editor.project();
        assert_eq!(p.compounds.len(), 2);
        assert!(p.compounds.iter().all(|k| k.removed && k.bin.is_empty()));
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn cut_and_paste_of_otio_imported_compound_restores_definition() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        nest(&mut f, &[&a]);
        let doc = crate::otio::export(
            f.editor.project(),
            "T1",
            None,
            crate::otio::Locations::Absolute,
        )
        .unwrap();
        let mut f2 = Fixture::new(&[(4.0, true)]);
        f2.ok(req("timeline.import", json!({"otio": doc})));
        let imported = f2.editor.project().tracks[0].clips[0].id.clone();
        let cut = f2.ok(req("edit.cut", json!({"clips": [imported]})));
        assert!(f2.editor.project().compounds.is_empty());
        f2.ok(req(
            "edit.paste",
            json!({
                "clips": cut.result["clips"],
                "compounds": cut.result["compounds"],
                "fps": cut.result["fps"],
                "at": 0,
            }),
        ));
        let p2 = f2.editor.project();
        assert_eq!(p2.compounds.len(), 1);
        assert!(p2.compounds[0].removed && p2.compounds[0].bin.is_empty());
        assert_eq!(crate::validate::everywhere(p2), Vec::<String>::new());
    }

    #[test]
    fn cross_rate_paste_clamps_full_compound_rounding_overshoot() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 12);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req(
            "timeline.new",
            json!({"name": "24fps", "own": {"fps": "24"}}),
        ));
        let r = f.ok(req("edit.append", json!({"compound": kid})));
        let on_24 = r.result["clip"].as_str().unwrap().to_string();
        assert_eq!(f.editor.project().clip(&on_24).unwrap().duration, 12);
        let copied = f.ok(req("edit.copy", json!({"clips": [on_24]})));
        let t1 = f.editor.project().timelines[0].id.clone();
        f.ok(req("timeline.open", json!({"timeline": t1})));
        let pasted = f.ok(req(
            "edit.paste",
            json!({
                "clips": copied.result["clips"],
                "compounds": copied.result["compounds"],
                "fps": copied.result["fps"],
                "at": 20,
            }),
        ));
        let pasted_id = pasted.result["clips"][0].as_str().unwrap().to_string();
        let p = f.editor.project();
        let c = p.clip(&pasted_id).unwrap();
        assert_eq!((c.duration, c.shown()), (12, (0, 12)));
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn cross_rate_paste_clamps_trimmed_compound_rounding_overshoot() {
        let mut f = Fixture::new(&[]);
        let r30 = crate::time::Rational::new(30, 1).unwrap();
        let info = crate::test_support::media_info(5.0, r30, true);
        f.env = crate::test_support::FakeEnv::default().with("/media/r30.mp4", info);
        f.ok(req("timeline.set", json!({"own": {"fps": "30"}})));
        f.ok(req("media.add", json!({"paths": ["/media/r30.mp4"]})));
        let a = f.append("m1", 0, 100);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        let t1 = f.editor.project().timelines[0].id.clone();
        f.ok(req(
            "timeline.new",
            json!({"name": "24fps", "own": {"fps": "24"}}),
        ));
        let r = f.ok(req("edit.append", json!({"compound": kid})));
        let on_24 = r.result["clip"].as_str().unwrap().to_string();
        f.ok(req(
            "edit.trim",
            json!({"clip": on_24, "edge": "head", "by": 79, "ripple": true}),
        ));
        f.ok(req(
            "edit.move",
            json!({"clips": [on_24], "to": "1f", "track": "V1"}),
        ));
        let copied = f.ok(req("edit.copy", json!({"clips": [on_24]})));
        f.ok(req("timeline.open", json!({"timeline": t1})));
        let pasted = f.ok(req(
            "edit.paste",
            json!({
                "clips": copied.result["clips"],
                "compounds": copied.result["compounds"],
                "fps": copied.result["fps"],
                "at": 110,
            }),
        ));
        let pasted_id = pasted.result["clips"][0].as_str().unwrap().to_string();
        let p = f.editor.project();
        let (lo, hi) = p.clip(&pasted_id).unwrap().shown();
        assert!(lo >= 0 && hi <= 100);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn nested_cross_rate_decompose_clamps_inner_compound_rounding_overshoot() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 12);
        nest(&mut f, &[&a]);
        let kid_b = f.editor.project().compounds[0].id.clone();
        let t1 = f.editor.project().timelines[0].id.clone();
        f.ok(req(
            "timeline.new",
            json!({"name": "24fps", "own": {"fps": "24"}}),
        ));
        let r_b = f.ok(req("edit.append", json!({"compound": kid_b})));
        let c_b = r_b.result["clip"].as_str().unwrap().to_string();
        nest(&mut f, &[&c_b]);
        let kid_a = f
            .editor
            .project()
            .compounds
            .iter()
            .find(|k| k.id != kid_b)
            .unwrap()
            .id
            .clone();
        f.ok(req("timeline.open", json!({"timeline": t1})));
        let r_a = f.ok(req("edit.overwrite", json!({"compound": kid_a, "at": 20})));
        let c_a = r_a.result["clip"].as_str().unwrap().to_string();
        let dec = f.ok(req("edit.decompose", json!({"clips": [c_a]})));
        let out_b = dec.result["clips"][0].as_str().unwrap().to_string();
        let p = f.editor.project();
        let c = p.clip(&out_b).unwrap();
        assert_eq!(c.compound.as_deref(), Some(kid_b.as_str()));
        assert_eq!((c.duration, c.shown()), (12, (0, 12)));
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn placing_one_frame_high_fps_compound_on_lower_fps_refuses_zero_duration() {
        let mut f = Fixture::new(&[]);
        let r60 = crate::time::Rational::new(60, 1).unwrap();
        let info = crate::test_support::media_info(1.0, r60, true);
        f.env = crate::test_support::FakeEnv::default().with("/media/r60.mp4", info);
        f.ok(req("timeline.set", json!({"own": {"fps": "60"}})));
        f.ok(req("media.add", json!({"paths": ["/media/r60.mp4"]})));
        let a = f.append("m1", 0, 1);
        nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req(
            "timeline.new",
            json!({"name": "24fps", "own": {"fps": "24"}}),
        ));
        let err = f.err(req("edit.append", json!({"compound": kid})));
        assert!(err.message.contains("shorter than one timeline frame"));
        assert_eq!(
            crate::validate::everywhere(f.editor.project()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn cross_project_paste_allocates_fresh_compound_id_without_future_collision() {
        let mut a = Fixture::new(&[(4.0, true)]);
        let mut last = String::new();
        for _ in 0..11 {
            last = a.append("m1", 0, 10);
        }
        let hosts = nest(&mut a, &[&last]);
        let src_kid = a.editor.project().compounds[0].id.clone();
        assert_eq!(src_kid, "k12");
        let copied = a.ok(req("edit.copy", json!({"clips": [hosts[0]]})));

        let mut b = Fixture::new(&[(4.0, true)]);
        b.ok(req(
            "edit.paste",
            json!({
                "clips": copied.result["clips"],
                "compounds": copied.result["compounds"],
                "fps": copied.result["fps"],
                "at": 0,
            }),
        ));
        for _ in 0..4 {
            let clip_id = b.append("m1", 0, 10);
            nest(&mut b, &[&clip_id]);
        }
        let p = b.editor.project();
        assert_eq!(p.compounds.len(), 5);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn cross_project_paste_with_same_compound_id_keeps_both_definitions() {
        let mut a = Fixture::new(&[(4.0, true)]);
        let ca = a.append("m1", 0, 20);
        let hosts_a = serde_json::from_value::<Vec<String>>(
            a.ok(req(
                "edit.compound",
                json!({"clips": [ca], "name": "From A"}),
            ))
            .result["clips"]
                .clone(),
        )
        .unwrap();
        let kid_a = a.editor.project().compounds[0].id.clone();
        let copied = a.ok(req("edit.copy", json!({"clips": [hosts_a[0]]})));

        let mut b = Fixture::new(&[(4.0, true)]);
        let cb = b.append("m1", 0, 50);
        b.ok(req(
            "edit.compound",
            json!({"clips": [cb], "name": "From B"}),
        ));
        let kid_b = b.editor.project().compounds[0].id.clone();
        assert_eq!(kid_b, kid_a);

        let pasted = b.ok(req(
            "edit.paste",
            json!({
                "clips": copied.result["clips"],
                "compounds": copied.result["compounds"],
                "fps": copied.result["fps"],
                "at": 60,
            }),
        ));
        let pasted_id = pasted.result["clips"][0].as_str().unwrap().to_string();
        let p = b.editor.project();
        assert_eq!(p.compounds.len(), 2);
        let b_def = p.compound(&kid_b).unwrap();
        assert_eq!(
            (b_def.name.as_str(), b_def.frames, b_def.removed),
            ("From B", 50, false)
        );
        let pasted_clip = p.clip(&pasted_id).unwrap();
        let pasted_kid = pasted_clip.compound.as_deref().unwrap();
        assert_ne!(pasted_kid, kid_b);
        let a_def = p.compound(pasted_kid).unwrap();
        assert_eq!(
            (a_def.name.as_str(), a_def.frames, a_def.removed),
            ("From A", 20, true)
        );
        assert_eq!(pasted_clip.duration, 20);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    #[test]
    fn repeated_paste_of_removed_compound_shares_restored_definition() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 20);
        let hosts = nest(&mut f, &[&a]);
        let kid = f.editor.project().compounds[0].id.clone();
        f.ok(req("media.compound_remove", json!({"compound": kid})));
        let cut = f.ok(req("edit.cut", json!({"clips": [hosts[0]]})));
        let p1 = f.ok(req(
            "edit.paste",
            json!({
                "clips": cut.result["clips"],
                "compounds": cut.result["compounds"],
                "fps": cut.result["fps"],
                "at": 0,
            }),
        ));
        let p2 = f.ok(req(
            "edit.paste",
            json!({
                "clips": cut.result["clips"],
                "compounds": cut.result["compounds"],
                "fps": cut.result["fps"],
                "at": 50,
            }),
        ));
        let c1 = p1.result["clips"][0].as_str().unwrap().to_string();
        let c2 = p2.result["clips"][0].as_str().unwrap().to_string();
        let p = f.editor.project();
        assert_eq!(p.compounds.len(), 1);
        assert_eq!(p.clip(&c1).unwrap().compound, p.clip(&c2).unwrap().compound);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }

    fn strip_otio_ids(doc: &mut Value) {
        doc.as_object_mut().unwrap().remove("metadata");
        for track in doc["tracks"]["children"].as_array_mut().unwrap() {
            for item in track["children"].as_array_mut().unwrap() {
                if let Some(meta) = item["metadata"].as_object_mut() {
                    meta.remove("vidcut");
                    if let Some(ed) = meta.get_mut("Resolve_OTIO").and_then(Value::as_object_mut) {
                        ed.remove("Sequence ID");
                    }
                }
            }
        }
    }

    fn audio_only_fixture() -> Fixture {
        let mut f = Fixture::new(&[]);
        let mut info =
            crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true);
        info.video = None;
        f.env = crate::test_support::FakeEnv::default().with("/media/a.wav", info);
        f.ok(req("media.add", json!({"paths": ["/media/a.wav"]})));
        f
    }

    #[test]
    fn pasting_copied_foreign_compound_into_itself_while_open_is_refused() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        nest(&mut f, &[&a]);
        let mut doc = crate::otio::export(
            f.editor.project(),
            "T1",
            None,
            crate::otio::Locations::Absolute,
        )
        .unwrap();
        strip_otio_ids(&mut doc);
        let mut f2 = Fixture::new(&[(4.0, true)]);
        f2.ok(req("timeline.import", json!({"otio": doc})));
        let imported = f2.editor.project().tracks[0].clips[0].id.clone();
        assert!(!f2.editor.project().compounds[0].sequence_id.is_empty());
        let copied = f2.ok(req("edit.copy", json!({"clips": [imported]})));
        f2.ok(req("compound.open", json!({"clip": imported})));
        let inner = f2.editor.project().tracks[0].clips[0].id.clone();
        f2.ok(req("picture.set", json!({"clips": [inner], "zoom": 1.5})));
        let err = f2.err(req(
            "edit.paste",
            json!({
                "clips": copied.result["clips"],
                "compounds": copied.result["compounds"],
                "fps": copied.result["fps"],
                "at": 0,
            }),
        ));
        assert!(err.message.contains("cannot hold itself"));
        let p2 = f2.editor.project();
        assert_eq!(p2.compounds.len(), 1);
        assert_eq!(crate::validate::everywhere(p2), Vec::<String>::new());
    }

    #[test]
    fn pasting_copied_audio_only_foreign_compound_into_itself_while_open_is_refused() {
        let mut f = audio_only_fixture();
        let ac = f.append("m1", 0, 30);
        nest(&mut f, &[&ac]);
        let mut doc = crate::otio::export(
            f.editor.project(),
            "T1",
            None,
            crate::otio::Locations::Absolute,
        )
        .unwrap();
        strip_otio_ids(&mut doc);
        let mut f2 = audio_only_fixture();
        f2.ok(req("timeline.import", json!({"otio": doc})));
        let imported = f2
            .editor
            .project()
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .find(|c| c.compound.is_some())
            .unwrap()
            .id
            .clone();
        let copied = f2.ok(req("edit.copy", json!({"clips": [imported]})));
        f2.ok(req("compound.open", json!({"clip": imported})));
        let err = f2.err(req(
            "edit.paste",
            json!({
                "clips": copied.result["clips"],
                "compounds": copied.result["compounds"],
                "fps": copied.result["fps"],
                "at": 0,
            }),
        ));
        assert!(err.message.contains("cannot hold itself"));
        let p2 = f2.editor.project();
        assert_eq!(p2.compounds.len(), 1);
        assert_eq!(crate::validate::everywhere(p2), Vec::<String>::new());
    }

    #[test]
    fn timeline_import_append_refuses_foreign_compound_without_sequence_id_while_open() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        nest(&mut f, &[&a]);
        let mut doc = crate::otio::export(
            f.editor.project(),
            "T1",
            None,
            crate::otio::Locations::Absolute,
        )
        .unwrap();
        strip_otio_ids(&mut doc);
        let mut f2 = Fixture::new(&[(4.0, true)]);
        f2.ok(req("timeline.import", json!({"otio": doc})));
        let imported = f2.editor.project().tracks[0].clips[0].id.clone();
        f2.ok(req("compound.open", json!({"clip": imported})));
        let err = f2.err(req(
            "timeline.import",
            json!({"otio": doc, "mode": "append"}),
        ));
        assert!(err.message.contains("hold itself"));
    }

    #[test]
    fn edit_paste_refuses_compound_with_empty_sequence_id() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 20);
        let hosts = nest(&mut f, &[&a]);
        let copied = f.ok(req("edit.copy", json!({"clips": [hosts[0]]})));
        let mut compounds = copied.result["compounds"].as_array().unwrap().clone();
        compounds[0]["sequence_id"] = json!("");
        let err = f.err(req(
            "edit.paste",
            json!({
                "clips": copied.result["clips"],
                "compounds": compounds,
                "fps": copied.result["fps"],
                "at": 30,
            }),
        ));
        assert!(err.message.contains("sequence_id is empty"));
    }

    fn strip_all_stack_metadata(v: &mut Value) {
        if let Some(o) = v.as_object_mut() {
            if o.get("OTIO_SCHEMA")
                .and_then(Value::as_str)
                .is_some_and(|s| s.starts_with("Stack"))
            {
                o.remove("metadata");
            }
            if let Some(ch) = o.get_mut("children").and_then(Value::as_array_mut) {
                for child in ch {
                    strip_all_stack_metadata(child);
                }
            }
        }
    }

    #[test]
    fn timeline_import_append_refuses_nested_generic_stack_without_sequence_type_while_open() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 40);
        let inner = nest(&mut f, &[&a]);
        nest(&mut f, &[&inner[0]]);
        let mut doc = crate::otio::export(
            f.editor.project(),
            "T1",
            None,
            crate::otio::Locations::Absolute,
        )
        .unwrap();
        doc.as_object_mut().unwrap().remove("metadata");
        strip_all_stack_metadata(&mut doc["tracks"]);
        let mut f2 = Fixture::new(&[(4.0, true)]);
        f2.ok(req("timeline.import", json!({"otio": doc})));
        let outer = f2.editor.project().tracks[0].clips[0].id.clone();
        f2.ok(req("compound.open", json!({"clip": outer})));
        let inner_clip = f2.editor.project().tracks[0].clips[0].id.clone();
        f2.ok(req("compound.open", json!({"clip": inner_clip})));
        let err = f2.err(req(
            "timeline.import",
            json!({"otio": doc, "mode": "append"}),
        ));
        assert!(err.message.contains("hold itself"));
    }

    #[test]
    fn empty_imported_compound_stack_opens_without_missing_clips_error() {
        let doc = json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "T1",
            "global_start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": 90000.0},
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "name": "Video 1",
                    "kind": "Video",
                    "children": [{
                        "OTIO_SCHEMA": "Stack.1",
                        "name": "Empty CC",
                        "metadata": {"Resolve_OTIO": {"Sequence Type": "Compound Clip", "Sequence Fps": 25.0}},
                        "source_range": {
                            "OTIO_SCHEMA": "TimeRange.1",
                            "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": 0.0},
                            "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": 40.0}
                        },
                        "children": [{
                            "OTIO_SCHEMA": "Track.1",
                            "name": "Video 1",
                            "kind": "Video",
                            "children": [{
                                "OTIO_SCHEMA": "Gap.1",
                                "source_range": {
                                    "OTIO_SCHEMA": "TimeRange.1",
                                    "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": 0.0},
                                    "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": 40.0}
                                }
                            }]
                        }]
                    }]
                }]
            }
        });
        let mut f = Fixture::new(&[]);
        f.ok(req("timeline.import", json!({"otio": doc})));
        let clip = f.editor.project().tracks[0].clips[0].id.clone();
        f.ok(req("compound.open", json!({"clip": clip})));
        let p = f.editor.project();
        assert_eq!(p.compound_path.len(), 1);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
        f.ok(req("history.undo", json!({})));
        let p = f.editor.project();
        assert!(p.compound_path.is_empty());
        assert!(p.compounds[0].tracks.is_empty());
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
        f.ok(req("history.redo", json!({})));
        let p = f.editor.project();
        assert_eq!(p.compound_path.len(), 1);
        assert_eq!(crate::validate::everywhere(p), Vec::<String>::new());
    }
}
