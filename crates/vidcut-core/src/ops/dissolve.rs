use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{
    Clip, MAX_BORDER, MotionEase, PictureTransition, Project, TrackKind, Transition,
};
use crate::otio::transitions::{self, Cuts};

/// Where a dissolve goes about its edit (decision D50).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    /// Centered on it, an odd length's middle frame before it.
    #[default]
    Center,
    /// Starting on it.
    Start,
    /// Ending on it.
    End,
}

pub fn split(length: i64, align: Align) -> (i64, i64) {
    match align {
        Align::Center => (length - length / 2, length / 2),
        Align::Start => (0, length),
        Align::End => (length, 0),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Length(i64, Align),
    Sides(i64, i64),
}

fn wanted(shape: Shape, outgoing: bool, incoming: bool) -> Option<(i64, i64)> {
    let (length, align) = match shape {
        Shape::Sides(before, after) => {
            let sides = (
                if outgoing { before } else { 0 },
                if incoming { after } else { 0 },
            );
            return (sides.0 + sides.1 > 0).then_some(sides);
        }
        Shape::Length(length, align) => (length, align),
    };
    match (outgoing, incoming, align) {
        (true, false, Align::Start) | (false, true, Align::End) => None,
        (true, false, _) => Some((length, 0)),
        (false, true, _) => Some((0, length)),
        _ => Some(split(length, align)),
    }
}

fn otio(sound: bool, frames: i64, kind: &PictureTransition) -> Value {
    if sound {
        return json!({
            "OTIO_SCHEMA": "Transition.1",
            "name": "Cross Fade +3 dB",
            "transition_type": "Custom_Transition",
            "metadata": {"Resolve_OTIO": {
                "Effects": {
                    "Display Type": 1,
                    "Effect Name": "Cross Fade +3 dB",
                    "Enabled": true,
                    "Name": "Cross Fade",
                    "Parameters": [],
                    "Type": 41
                },
                "Transition Type": "Cross Fade +3DB"
            }}
        });
    }
    let mut keys = serde_json::Map::new();
    keys.insert("0".into(), json!({"Value": 0.0, "Variant Type": "Double"}));
    keys.insert(
        frames.to_string(),
        json!({"Value": 1.0, "Variant Type": "Double"}),
    );
    let dissolve = json!({
        "OTIO_SCHEMA": "Transition.1",
        "name": "Cross Dissolve",
        "transition_type": "SMPTE_Dissolve",
        "metadata": {"Resolve_OTIO": {
            "Effects": {
                "Display Type": 1,
                "Effect Name": "Cross Dissolve",
                "Enabled": true,
                "Name": "Cross Dissolve",
                "Parameters": [{
                    "Default Parameter Value": 0.0,
                    "Key Frames": keys,
                    "Parameter ID": "transitionCustomCurvesKeyframes",
                    "Parameter Value": 0.0,
                    "Variant Type": "Double",
                    "maxValue": 1.0,
                    "minValue": 0.0
                }],
                "Type": 9
            },
            "Transition Type": "Cross Dissolve"
        }}
    });
    match kind {
        PictureTransition::Dissolve => dissolve,
        PictureTransition::DipToColor { color } => transitions::dip_otio(*color),
        PictureTransition::Wipe { direction } => transitions::wipe_otio(*direction),
        PictureTransition::Push { preset, motion } => {
            transitions::push_otio(*preset, motion, frames)
        }
        PictureTransition::Slide {
            preset,
            out,
            motion,
        } => transitions::slide_otio(*preset, *out, motion, frames),
    }
}

fn checked(kind: &PictureTransition) -> Result<()> {
    let Some(m) = kind.motion() else {
        return Ok(());
    };
    if !(0.0..=MAX_BORDER).contains(&m.border) {
        return Err(Error::invalid(format!(
            "a border is from 0 to {MAX_BORDER}"
        )));
    }
    if !(0.0..=1.0).contains(&m.motion_blur) {
        return Err(Error::invalid("a motion blur is from 0 to 1"));
    }
    if !m.curve.is_empty() && m.ease != MotionEase::Custom {
        return Err(Error::invalid("a curve goes with the custom ease only"));
    }
    let finite = |h: &crate::model::Handle| h.frames.is_finite() && h.value.is_finite();
    let fits = |k: &crate::model::Keyframe| {
        (0.0..=1.0).contains(&k.value) && k.in_handle.iter().chain(&k.out_handle).all(finite)
    };
    if !m.curve.iter().all(fits) {
        return Err(Error::invalid(
            "a key of a curve is from 0 to 1, with finite handles",
        ));
    }
    Ok(())
}

fn span(c: &Clip, sound: bool) -> Option<(i64, i64)> {
    if sound {
        c.audio_range()
    } else {
        c.video.then(|| (c.start, c.end()))
    }
}

fn edits(p: &Project, ti: usize, sound: bool) -> Vec<i64> {
    let mut points: Vec<i64> = p.tracks[ti]
        .clips
        .iter()
        .filter_map(|c| span(c, sound))
        .flat_map(|(a, b)| [a, b])
        .collect();
    points.sort_unstable();
    points.dedup();
    points
}

fn nearest(p: &Project, ti: usize, sound: bool, at: i64) -> Option<i64> {
    edits(p, ti, sound)
        .into_iter()
        .min_by_key(|&e| ((e - at).abs(), e))
}

fn sides(p: &Project, ti: usize, sound: bool, e: i64) -> (Option<String>, Option<String>) {
    let clips = &p.tracks[ti].clips;
    let find = |end: bool| {
        clips
            .iter()
            .find(|c| span(c, sound).is_some_and(|(a, b)| if end { b == e } else { a == e }))
            .map(|c| c.id.clone())
    };
    (find(true), find(false))
}

fn compound_at(p: &Project, ti: usize, sound: bool, e: i64) -> Option<String> {
    let (from, to) = sides(p, ti, sound, e);
    [from, to].into_iter().flatten().find(|id| {
        p.tracks[ti]
            .clips
            .iter()
            .any(|c| &c.id == id && crate::render::plays_inside(c))
    })
}

fn room(
    p: &Project,
    ti: usize,
    sound: bool,
    e: i64,
    from: Option<&str>,
    to: Option<&str>,
) -> (i64, i64) {
    let clips = &p.tracks[ti].clips;
    let clip = |id: Option<&str>| id.and_then(|id| clips.iter().find(|c| c.id == id));
    let (f, g) = (clip(from), clip(to));
    let spans: Vec<(i64, i64)> = clips.iter().filter_map(|c| span(c, sound)).collect();
    let free_before = e - spans
        .iter()
        .map(|s| s.1)
        .filter(|&b| b <= e)
        .max()
        .unwrap_or(0);
    let free_after = spans
        .iter()
        .map(|s| s.0)
        .filter(|&a| a >= e)
        .min()
        .map_or(i64::MAX / 4, |a| a - e);
    let media = |c: &Clip, tail: bool| match p.source_range(c) {
        _ if c.is_generator() => i64::MAX / 4,
        Some(have) => c.room(tail, have),
        None => 0,
    };
    let before = match f {
        Some(f) => span(f, sound).map_or(0, |(a, _)| e - a),
        None => free_before,
    }
    .min(g.map_or(i64::MAX / 4, |g| media(g, false)));
    let after = match g {
        Some(g) => span(g, sound).map_or(0, |(_, b)| b - e),
        None => free_after,
    }
    .min(f.map_or(i64::MAX / 4, |f| media(f, true)));
    let (mut before, mut after) = (before, after);
    for t in transitions::playing(p, &p.tracks[ti], sound, |_| true) {
        let (w0, w1) = t.frames;
        if w0 + t.transition.before == e {
        } else if w1 <= e {
            before = before.min(e - w1);
        } else if w0 >= e {
            after = after.min(w0 - e);
        } else {
            (before, after) = (0, 0);
        }
    }
    (before.max(0), after.max(0))
}

pub fn room_at(p: &Project, track: &str, at: i64, sound: bool) -> Option<(i64, i64)> {
    let ti = p.track_index(track)?;
    let (from, to) = sides(p, ti, sound, at);
    Some(room(p, ti, sound, at, from.as_deref(), to.as_deref()))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Made {
    pub id: String,
    pub sound: bool,
    pub cut: i64,
    pub before: i64,
    pub after: i64,
}

type Plan = (Option<String>, Option<String>, i64, i64);

fn plan(p: &Project, ti: usize, sound: bool, e: i64, shape: Shape) -> Option<Plan> {
    let (from, to) = sides(p, ti, sound, e);
    if from.is_none() && to.is_none() || compound_at(p, ti, sound, e).is_some() {
        return None;
    }
    let (want_before, want_after) = wanted(shape, from.is_some(), to.is_some())?;
    let (room_before, room_after) = room(p, ti, sound, e, from.as_deref(), to.as_deref());
    let (before, after) = (want_before.min(room_before), want_after.min(room_after));
    (before + after > 0).then_some((from, to, before, after))
}

pub fn planned(p: &Project, track: &str, e: i64, shape: Shape) -> Option<(i64, i64)> {
    let ti = p.track_index(track)?;
    plan(p, ti, false, e, shape).map(|(_, _, before, after)| (before, after))
}

fn put(
    p: &mut Project,
    ti: usize,
    sound: bool,
    e: i64,
    shape: Shape,
    kind: &PictureTransition,
) -> Option<Made> {
    let (from, to, before, after) = plan(p, ti, sound, e, shape)?;
    let mut t = Transition {
        id: String::new(),
        sound,
        from,
        to,
        before,
        after,
        otio: otio(sound, before + after, kind).into(),
    };
    let cuts = Cuts::of(p, &p.tracks[ti]);
    if cuts.place(p, &t) != Some(e) {
        return None;
    }
    let gone: Vec<bool> = p.tracks[ti]
        .transitions
        .iter()
        .map(|x| x.sound == sound && cuts.place(p, x) == Some(e))
        .collect();
    let mut gone = gone.into_iter();
    p.tracks[ti]
        .transitions
        .retain(|_| !gone.next().unwrap_or(false));
    let id = p.alloc_transition_id();
    t.id = id.clone();
    p.tracks[ti].transitions.push(t);
    Some(Made {
        id,
        sound,
        cut: e,
        before,
        after,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn dissolve(
    p: &mut Project,
    track: &str,
    at: i64,
    shape: Shape,
    picture: bool,
    sound: bool,
    all: bool,
    kind: &PictureTransition,
) -> Result<Vec<Made>> {
    checked(kind)?;
    let ti = p
        .track_index(track)
        .ok_or_else(|| Error::not_found(format!("no track {track}")))?;
    let pictures = picture && p.tracks[ti].kind == TrackKind::Video;
    let mut made = Vec::new();
    if all {
        for band in [false, true] {
            if (band && !sound) || (!band && !pictures) {
                continue;
            }
            for e in edits(p, ti, band) {
                if let (Some(_), Some(_)) = sides(p, ti, band, e)
                    && let Some(m) = put(p, ti, band, e, shape, kind)
                {
                    made.push(m);
                }
            }
        }
        return Ok(made);
    }
    let e = nearest(p, ti, !pictures, at)
        .ok_or_else(|| Error::invalid(format!("{track} has no edit to dissolve")))?;
    if pictures && let Some(c) = compound_at(p, ti, false, e) {
        return Err(compound_error(&c));
    }
    let made = at_edit(p, ti, pictures, sound, e, shape, kind);
    if made.is_empty() {
        let (from, to) = sides(p, ti, !pictures, e);
        return Err(Error::invalid(
            if wanted(shape, from.is_some(), to.is_some()).is_none() {
                "the edit has no clip on that side to dissolve"
            } else {
                "the clips at the edit have no frames past it to dissolve with"
            },
        ));
    }
    Ok(made)
}

pub fn cross_fade_joins(p: &mut Project, joins: &[(usize, i64)]) -> Vec<Made> {
    let mut made = Vec::new();
    for &(ti, e) in joins {
        let shape = Shape::Length(2, Align::Center);
        let Some(m) = put(p, ti, true, e, shape, &PictureTransition::Dissolve) else {
            continue;
        };
        if let Some(t) = p.tracks[ti].transitions.iter_mut().find(|t| t.id == m.id) {
            t.otio = zero_db().into();
        }
        made.push(m);
    }
    made
}

fn zero_db() -> Value {
    json!({
        "OTIO_SCHEMA": "Transition.1",
        "name": "Cross Fade 0 dB",
        "transition_type": "SMPTE_Dissolve",
        "metadata": {"Resolve_OTIO": {
            "Effects": {
                "Display Type": 1,
                "Effect Name": "Cross Fade 0 dB",
                "Enabled": true,
                "Name": "Cross Fade",
                "Parameters": [],
                "Type": 40
            },
            "Transition Type": "Cross Fade 0DB"
        }}
    })
}

pub fn dissolve_clips(
    p: &mut Project,
    clips: &[String],
    shape: Shape,
    picture: bool,
    sound: bool,
    kind: &PictureTransition,
) -> Result<Vec<Made>> {
    checked(kind)?;
    let mut points: Vec<(usize, bool, i64)> = p
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(ti, t)| {
            let pictures = picture && t.kind == TrackKind::Video;
            t.clips
                .iter()
                .filter(|c| clips.contains(&c.id))
                .filter_map(move |c| span(c, !pictures))
                .flat_map(move |(a, b)| [(ti, pictures, a), (ti, pictures, b)])
        })
        .collect();
    points.sort_unstable();
    points.dedup();
    let mut made = Vec::new();
    for &(ti, pictures, e) in &points {
        made.extend(at_edit(p, ti, pictures, sound, e, shape, kind));
    }
    if made.is_empty() {
        return Err(points
            .iter()
            .find_map(|&(ti, pictures, e)| compound_at(p, ti, !pictures, e))
            .map_or_else(
                || Error::invalid("the clips have no frames past their edges to dissolve with"),
                |c| compound_error(&c),
            ));
    }
    Ok(made)
}

fn compound_error(c: &str) -> Error {
    Error::invalid(format!(
        "{c} is a compound clip, and its edges take no dissolve: decompose it first"
    ))
}

fn at_edit(
    p: &mut Project,
    ti: usize,
    pictures: bool,
    sound: bool,
    e: i64,
    shape: Shape,
    kind: &PictureTransition,
) -> Vec<Made> {
    let mut made = Vec::new();
    let sound_e = if pictures {
        let (from, to) = sides(p, ti, false, e);
        made.extend(put(p, ti, false, e, shape, kind));
        sound_edit(p, ti, from.as_deref(), to.as_deref())
    } else {
        Some(e)
    };
    if sound && let Some(e) = sound_e {
        made.extend(put(p, ti, true, e, shape, kind));
    }
    made
}

fn sound_edit(p: &Project, ti: usize, from: Option<&str>, to: Option<&str>) -> Option<i64> {
    let clips = &p.tracks[ti].clips;
    let range = |id: Option<&str>| {
        id.and_then(|id| clips.iter().find(|c| c.id == id))
            .and_then(Clip::audio_range)
    };
    match (range(from), range(to), from, to) {
        (Some((_, b)), Some((a, _)), ..) => (a == b).then_some(a),
        (Some((_, b)), None, _, None) => Some(b),
        (None, Some((a, _)), None, _) => Some(a),
        _ => None,
    }
}

pub fn set_transition(
    p: &mut Project,
    track: &str,
    at: i64,
    kind: &PictureTransition,
) -> Result<Made> {
    checked(kind)?;
    let ti = p
        .track_index(track)
        .ok_or_else(|| Error::not_found(format!("no track {track}")))?;
    if p.tracks[ti].kind != TrackKind::Video {
        return Err(Error::invalid(format!("{track} has no picture transition")));
    }
    let e =
        nearest(p, ti, false, at).ok_or_else(|| Error::invalid(format!("{track} has no edit")))?;
    let cuts = Cuts::of(p, &p.tracks[ti]);
    let slot = p.tracks[ti]
        .transitions
        .iter()
        .position(|t| !t.sound && cuts.place(p, t) == Some(e));
    let Some(slot) = slot else {
        return Err(Error::invalid("the edit has no picture transition"));
    };
    let t = &mut p.tracks[ti].transitions[slot];
    let frames = t.before + t.after;
    *t.otio.make_mut() = otio(false, frames, kind);
    Ok(Made {
        id: t.id.clone(),
        sound: false,
        cut: e,
        before: t.before,
        after: t.after,
    })
}

pub fn to_cut(
    p: &mut Project,
    track: &str,
    at: i64,
    picture: bool,
    sound: bool,
    all: bool,
) -> Result<Vec<String>> {
    let ti = p
        .track_index(track)
        .ok_or_else(|| Error::not_found(format!("no track {track}")))?;
    let keeps = |t: &Transition| if t.sound { !sound } else { !picture };
    let gone: Vec<bool> = if all {
        p.tracks[ti].transitions.iter().map(|t| !keeps(t)).collect()
    } else {
        let video = p.tracks[ti].kind == TrackKind::Video && picture;
        let e = nearest(p, ti, !video, at)
            .ok_or_else(|| Error::invalid(format!("{track} has no edit")))?;
        let (from, to) = sides(p, ti, !video, e);
        let cuts = Cuts::of(p, &p.tracks[ti]);
        let sound_e = if video {
            sound_edit(p, ti, from.as_deref(), to.as_deref())
        } else {
            Some(e)
        };
        p.tracks[ti]
            .transitions
            .iter()
            .map(|t| {
                let at = cuts.place(p, t);
                let here = if t.sound {
                    at.is_some() && at == sound_e
                } else {
                    at == Some(e)
                };
                here && !keeps(t)
            })
            .collect()
    };
    let ids: Vec<String> = p.tracks[ti]
        .transitions
        .iter()
        .zip(&gone)
        .filter(|(_, g)| **g)
        .map(|(t, _)| t.id.clone())
        .collect();
    if ids.is_empty() {
        return Err(Error::invalid("no transition there to take away"));
    }
    let mut gone = gone.into_iter();
    p.tracks[ti]
        .transitions
        .retain(|_| !gone.next().unwrap_or(false));
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::test_support::Fixture;

    fn fixture() -> Fixture {
        Fixture::new(&[(10.0, true), (4.0, true)])
    }

    fn req(f: &mut Fixture, method: &str, params: Value) -> Value {
        f.ok(json!({"method": method, "params": params})).result
    }

    fn made(r: &Value) -> Vec<(bool, i64, i64, i64)> {
        r["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                let n = |k: &str| t[k].as_i64().unwrap();
                (
                    t["sound"].as_bool().unwrap(),
                    n("cut"),
                    n("before"),
                    n("after"),
                )
            })
            .collect()
    }

    #[test]
    fn a_dissolve_goes_on_the_nearest_edit_as_long_as_the_media_allow() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let r = req(&mut f, "edit.dissolve", json!({"at": 48}));
        assert_eq!(made(&r), [(false, 50, 10, 12), (true, 50, 10, 12)]);
    }

    #[test]
    fn a_wipe_from_the_left_reads_back_as_that_wipe() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let wipe = json!({"type": "wipe", "direction": "left"});
        req(
            &mut f,
            "edit.dissolve",
            json!({"at": 50, "transition": wipe}),
        );
        let p = f.editor.project();
        let t = p.track("V1").unwrap().transitions.iter().find(|t| !t.sound);
        assert_eq!(
            t.and_then(crate::otio::transitions::kind_of),
            Some(crate::model::PictureTransition::Wipe {
                direction: crate::model::WipeDirection::Left
            })
        );
    }

    #[test]
    fn a_red_dip_reads_back_as_that_dip() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let dip = json!({"type": "dip_to_color", "color": "#ff0000"});
        req(
            &mut f,
            "edit.dissolve",
            json!({"at": 50, "transition": dip}),
        );
        let p = f.editor.project();
        let t = p.track("V1").unwrap().transitions.iter().find(|t| !t.sound);
        assert_eq!(
            t.and_then(crate::otio::transitions::kind_of),
            Some(crate::model::PictureTransition::DipToColor { color: [255, 0, 0] })
        );
    }

    #[test]
    fn a_dip_without_a_color_is_black() {
        let dip: crate::model::PictureTransition =
            serde_json::from_value(json!({"type": "dip_to_color"})).unwrap();
        assert_eq!(
            dip,
            crate::model::PictureTransition::DipToColor { color: [0, 0, 0] }
        );
    }

    fn kinds_after_a_dissolve() -> Vec<Option<crate::model::PictureTransition>> {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        req(&mut f, "edit.dissolve", json!({"at": 50}));
        let p = f.editor.project();
        let mut all = Vec::new();
        for t in &p.tracks {
            all.extend(t.transitions.iter().map(crate::otio::transitions::kind_of));
        }
        all
    }

    #[test]
    fn a_cross_dissolve_reads_back_as_a_dissolve() {
        assert!(
            kinds_after_a_dissolve().contains(&Some(crate::model::PictureTransition::Dissolve))
        );
    }

    #[test]
    fn a_cross_fade_has_no_picture_type() {
        assert!(kinds_after_a_dissolve().contains(&None));
    }

    #[test]
    fn a_centered_picture_transition_at_the_timeline_start_goes_after_it() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        let shape = super::Shape::Length(20, super::Align::Center);
        assert_eq!(
            super::planned(f.editor.project(), "V1", 0, shape),
            Some((0, 20))
        );
    }

    fn one_clip_on_v2(f: &mut Fixture) {
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        f.append("m1", 0, 50);
        f.ok(json!({"method": "edit.move", "params": {"clips": "c3", "to": "20f", "track": "V2"}}));
    }

    fn transitions(f: &Fixture, track: &str) -> usize {
        f.editor.project().track(track).unwrap().transitions.len()
    }

    fn cuts(r: &Value) -> Vec<(bool, i64)> {
        made(r).iter().map(|m| (m.0, m.1)).collect()
    }

    #[test]
    fn the_dissolve_button_takes_track_1s_edit_though_another_tracks_is_nearer() {
        let mut f = fixture();
        one_clip_on_v2(&mut f);
        let r = req(&mut f, "edit.dissolve", json!({"at": 68}));
        assert_eq!((made(&r)[0].1, transitions(&f, "V2")), (50, 0));
    }

    #[test]
    fn the_cut_button_takes_track_1s_edit_though_another_tracks_is_nearer() {
        let mut f = fixture();
        one_clip_on_v2(&mut f);
        req(&mut f, "edit.dissolve", json!({"at": 48}));
        req(&mut f, "edit.dissolve", json!({"at": 70, "track": "V2"}));
        req(&mut f, "edit.remove_transition", json!({"at": 68}));
        assert_eq!(
            (transitions(&f, "V1"), transitions(&f, "V2") > 0),
            (0, true)
        );
    }

    #[test]
    fn ctrl_t_with_a_clip_selected_dissolves_each_of_its_edges() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        f.append("m1", 100, 150);
        let r = req(&mut f, "edit.dissolve", json!({"at": 0, "clips": ["c2"]}));
        assert_eq!(
            cuts(&r),
            [(false, 50), (true, 50), (false, 100), (true, 100)]
        );
    }

    #[test]
    fn ctrl_t_with_two_clips_selected_dissolves_the_edit_between_them_once() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let r = req(&mut f, "edit.dissolve", json!({"clips": ["c1", "c2"]}));
        assert_eq!(
            cuts(&r),
            [
                (false, 0),
                (true, 0),
                (false, 50),
                (true, 50),
                (false, 100),
                (true, 100)
            ]
        );
    }

    #[test]
    fn ctrl_t_with_a_clip_selected_on_v2_dissolves_it_there() {
        let mut f = fixture();
        one_clip_on_v2(&mut f);
        let r = req(&mut f, "edit.dissolve", json!({"at": 55, "clips": ["c3"]}));
        assert_eq!(
            (cuts(&r), transitions(&f, "V1")),
            (vec![(false, 20), (true, 20), (false, 70), (true, 70)], 0)
        );
    }

    #[test]
    fn dissolves_on_clips_take_no_track() {
        let mut f = fixture();
        one_clip_on_v2(&mut f);
        f.err(json!({"method": "edit.dissolve", "params": {"clips": ["c3"], "track": "V1"}}));
    }

    #[test]
    fn a_dissolve_starting_on_a_clip_after_a_gap_fades_it_from_black() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m1", 50, 100);
        f.ok(json!({"method": "edit.delete", "params": {"clips": "c1", "lift": true}}));
        let r = req(&mut f, "edit.dissolve", json!({"at": 50, "align": "start"}));
        assert_eq!(made(&r), [(false, 50, 0, 25), (true, 50, 0, 25)]);
    }

    #[test]
    fn an_edit_without_media_past_it_takes_no_dissolve() {
        let mut f = fixture();
        f.append("m1", 0, 250);
        f.append("m2", 0, 100);
        f.err(json!({"method": "edit.dissolve", "params": {"at": 250}}));
    }

    #[test]
    fn the_cut_button_takes_the_edits_transitions_away() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        req(&mut f, "edit.dissolve", json!({"at": 50}));
        let r = req(&mut f, "edit.remove_transition", json!({"at": 52}));
        assert_eq!(r["removed"], json!(["t1", "t2"]));
    }

    #[test]
    fn dissolves_go_on_all_edits_between_clips() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        f.append("m1", 100, 150);
        let r = req(
            &mut f,
            "edit.dissolve",
            json!({"all": true, "length": "4f", "sound": false}),
        );
        assert_eq!(made(&r), [(false, 50, 2, 2), (false, 100, 2, 2)]);
    }

    #[test]
    fn a_dissolve_on_the_last_clips_end_fades_it_out_inside_it() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        let r = req(&mut f, "edit.dissolve", json!({"at": 50, "length": "10f"}));
        assert_eq!(made(&r), [(false, 50, 10, 0), (true, 50, 10, 0)]);
    }

    #[test]
    fn a_dissolve_at_the_timeline_start_goes_inside_the_first_clip_though_it_has_media_before() {
        let mut f = fixture();
        f.append("m1", 20, 70);
        let r = req(&mut f, "edit.dissolve", json!({"at": 10}));
        assert_eq!(made(&r), [(false, 0, 0, 25), (true, 0, 0, 25)]);
    }

    #[test]
    fn ctrl_t_on_a_clip_with_empty_track_around_it_dissolves_inside_its_edges() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m1", 100, 150);
        f.ok(json!({"method": "edit.move", "params": {"clips": "c2", "to": "20f", "track": "V2"}}));
        let r = req(
            &mut f,
            "edit.dissolve",
            json!({"clips": ["c2"], "sound": false}),
        );
        assert_eq!(made(&r), [(false, 20, 0, 25), (false, 70, 25, 0)]);
    }

    #[test]
    fn a_dissolve_toward_the_empty_side_of_a_clips_end_is_refused() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        let e = f.err(json!({"method": "edit.dissolve", "params": {"at": 50, "align": "start"}}));
        assert_eq!(
            e.to_string(),
            "the edit has no clip on that side to dissolve"
        );
    }

    #[test]
    fn dissolves_on_all_edits_stop_where_the_one_before_ends() {
        let mut f = fixture();
        f.append("m1", 0, 20);
        f.append("m1", 100, 120);
        f.append("m1", 200, 220);
        let r = req(
            &mut f,
            "edit.dissolve",
            json!({"all": true, "sound": false}),
        );
        assert_eq!(made(&r), [(false, 20, 13, 12), (false, 40, 8, 12)]);
    }

    #[test]
    fn the_cut_button_takes_a_sound_fade_with_the_picture_left_alone() {
        let mut f = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
        f.append("m1", 0, 50);
        f.append("m3", 0, 50);
        req(&mut f, "edit.dissolve", json!({"at": 50, "picture": false}));
        let r = req(
            &mut f,
            "edit.remove_transition",
            json!({"at": 50, "picture": false}),
        );
        assert_eq!(r["removed"], json!(["t1"]));
    }

    fn c2_compound() -> crate::model::Project {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let mut p = f.editor.project().clone();
        let inner = p.tracks[0].clone();
        p.tracks[0].clips[1].compound = Some(crate::model::Compound {
            stack: Default::default(),
            frames: 100,
            tracks: vec![inner],
        });
        p
    }

    #[test]
    fn a_compound_clips_edge_takes_no_dissolve() {
        let mut p = c2_compound();
        let made = super::dissolve(
            &mut p,
            "V1",
            50,
            super::Shape::Length(25, super::Align::Center),
            true,
            false,
            false,
            &crate::model::PictureTransition::Dissolve,
        );
        assert!(made.is_err());
    }

    #[test]
    fn ctrl_t_on_a_selected_compound_clip_asks_to_decompose_it() {
        let mut p = c2_compound();
        let e = super::dissolve_clips(
            &mut p,
            &["c2".to_string()],
            super::Shape::Length(25, super::Align::Center),
            true,
            false,
            &crate::model::PictureTransition::Dissolve,
        )
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "c2 is a compound clip, and its edges take no dissolve: decompose it first"
        );
    }

    #[test]
    fn a_dissolve_longer_than_both_clips_fills_both() {
        let mut f = Fixture::new(&[(60.0, true)]);
        f.append("m1", 100, 400);
        f.append("m1", 700, 1000);
        let r = req(
            &mut f,
            "edit.dissolve",
            json!({"at": 300, "length": "999f"}),
        );
        assert_eq!(made(&r), [(false, 300, 300, 300), (true, 300, 300, 300)]);
    }

    #[test]
    fn a_dissolves_sides_are_each_shortened_to_their_own_media() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let r = req(
            &mut f,
            "edit.dissolve",
            json!({"at": 50, "before": "12f", "after": "17f", "sound": false}),
        );
        assert_eq!(made(&r), [(false, 50, 10, 17)]);
    }

    #[test]
    fn a_dissolve_takes_its_sides_or_a_length_not_both() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        f.err(json!({"method": "edit.dissolve", "params": {"at": 50, "before": "5f", "after": "5f", "length": "10f"}}));
    }

    #[test]
    fn a_dissolve_of_odd_length_has_its_middle_frame_before_the_edit() {
        assert_eq!(super::split(7, super::Align::Center), (4, 3));
    }

    #[test]
    fn a_dissolve_request_can_make_a_left_wipe() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        req(
            &mut f,
            "edit.dissolve",
            json!({"at": 50, "transition": {"type": "wipe", "direction": "left"}}),
        );
        let p = f.editor.project();
        let t = p
            .track("V1")
            .unwrap()
            .transitions
            .iter()
            .find(|t| !t.sound)
            .unwrap();
        assert_eq!(
            crate::otio::transitions::wipe_of(t),
            Some(crate::model::WipeDirection::Left)
        );
    }

    #[test]
    fn a_transition_request_turns_a_dissolve_into_a_left_wipe() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        req(&mut f, "edit.dissolve", json!({"at": 50, "sound": false}));
        let (before, after) = {
            let p = f.editor.project();
            let t = &p.track("V1").unwrap().transitions[0];
            (t.before, t.after)
        };
        req(
            &mut f,
            "edit.transition",
            json!({"at": 50, "transition": {"type": "wipe", "direction": "left"}}),
        );
        let p = f.editor.project();
        let t = &p.track("V1").unwrap().transitions[0];
        assert_eq!(
            (t.before, t.after, crate::otio::transitions::wipe_of(t)),
            (before, after, Some(crate::model::WipeDirection::Left))
        );
    }

    #[test]
    fn a_transition_request_turns_a_wipe_into_a_dissolve() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        req(
            &mut f,
            "edit.dissolve",
            json!({"at": 50, "sound": false, "transition": {"type": "wipe", "direction": "right"}}),
        );
        let (before, after) = {
            let p = f.editor.project();
            let t = &p.track("V1").unwrap().transitions[0];
            (t.before, t.after)
        };
        req(
            &mut f,
            "edit.transition",
            json!({"at": 50, "transition": {"type": "dissolve"}}),
        );
        let p = f.editor.project();
        let t = &p.track("V1").unwrap().transitions[0];
        assert_eq!(
            (
                t.before,
                t.after,
                crate::otio::transitions::dissolves(t),
                crate::otio::transitions::wipe_of(t)
            ),
            (before, after, true, None)
        );
    }
}
