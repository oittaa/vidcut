use std::collections::{HashMap, HashSet};

use super::{block_clip, ensure_track, fix_audio, linked, locate, split_at, tracks, unlocked};
use crate::error::{Error, Result};
use crate::model::{Clip, Compound, Kept, Project, Track, TrackKind};
use crate::otio;

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

fn lift(
    p: &mut Project,
    ids: &HashSet<String>,
    kind: TrackKind,
    first: i64,
    every_track: bool,
) -> Vec<Track> {
    let order = p.tracks_of(kind);
    let selected = |&i: &usize| holds(&p.tracks[i], ids);
    let (Some(lowest), Some(highest)) = (
        order.iter().position(selected),
        order.iter().rposition(selected),
    ) else {
        return Vec::new();
    };
    let lowest = if every_track { 0 } else { lowest };
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
            ..Track::new(format!("A{n}"), TrackKind::Audio)
        });
    }
    sound
}

fn number(t: &Track) -> usize {
    t.id[1..].parse().unwrap_or(1)
}

fn stack_of(p: &mut Project, whole: &[Track], name: &str, frames: i64) -> Result<Kept> {
    let mut inner = Project::new(p.settings.clone());
    for t in whole {
        let ti = ensure_track(&mut inner, &t.id)?;
        inner.tracks[ti].clips.clone_from(&t.clips);
        inner.tracks[ti].transitions.clone_from(&t.transitions);
    }
    inner.media = std::mem::take(&mut p.media);
    let stack = otio::made_stack(&inner, name, frames);
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
            format!("Compound Clip {}", made + 1)
        }
    }
}

pub fn nest(p: &mut Project, ids: &[String], name: Option<&str>) -> Result<Nested> {
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
    let lowest_picture = p
        .tracks_of(TrackKind::Video)
        .into_iter()
        .find(|&i| holds(&p.tracks[i], &ids));
    let lowest_sound = p
        .tracks_of(TrackKind::Audio)
        .into_iter()
        .find(|&i| holds(&p.tracks[i], &ids))
        .map(|i| p.tracks[i].id.clone());
    build(p, &ids, span, name, (lowest_picture, lowest_sound), false)
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

pub fn nest_range(p: &mut Project, (a, b): (i64, i64), name: Option<&str>) -> Result<Nested> {
    if a >= b {
        return Err(Error::usage(format!("empty range {a}..{b}")));
    }
    let first_picture = p.tracks_of(TrackKind::Video).first().copied();
    for ti in 0..p.tracks.len() {
        let reached = p.tracks[ti].clips.iter().any(|c| {
            let (s, e) = extent(c);
            s < b && e > a
        });
        if reached || Some(ti) == first_picture {
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
    build(p, &ids, (a, b), name, (first_picture, None), true)
}

fn build(
    p: &mut Project,
    ids: &HashSet<String>,
    span: (i64, i64),
    name: Option<&str>,
    (lowest_picture, lowest_sound): (Option<usize>, Option<String>),
    every_track: bool,
) -> Result<Nested> {
    let frames = span.1 - span.0;
    let name = given_name(p, name);
    let mut whole = lift(p, ids, TrackKind::Video, span.0, every_track);
    whole.extend(lift(p, ids, TrackKind::Audio, span.0, every_track));
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
    for x in &inner.clips {
        let Some((mut y, _)) = crate::render::inside(x, block) else {
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
    }
    let dropped = otio::transitions::prune(p);
    if dropped > 0 {
        warnings.push(otio::transitions::dropped(dropped));
    }
    Ok((block, made))
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

    #[test]
    fn a_dissolve_plays_the_same_inside_a_compound_clip() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 10, 50);
        let b = f.append("m1", 50, 90);
        f.ok(req("edit.dissolve", json!({"at": 40})));
        let plan = |f: &Fixture| {
            crate::render::flatten(f.editor.project(), 0, 80)
                .unwrap()
                .video
        };
        let before = plan(&f);
        nest(&mut f, &[&a, &b]);
        assert_eq!(plan(&f), before);
    }

    #[test]
    fn a_compound_clip_cut_through_a_dissolve_shows_that_part_of_it() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 10, 50);
        let b = f.append("m1", 50, 90);
        f.ok(req("edit.dissolve", json!({"at": 40})));
        let plan = |f: &Fixture| {
            crate::render::flatten(f.editor.project(), 0, 35)
                .unwrap()
                .video
        };
        let before = plan(&f);
        let host = nest(&mut f, &[&a, &b]).remove(0);
        let trim = json!({"clip": host, "edge": "tail", "to": "35f", "ripple": false});
        f.ok(req("edit.trim", trim));
        assert_eq!(plan(&f), before);
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
}
