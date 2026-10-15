//! Dissolves made here (decision D50): the Dissolve button and Ctrl+T put
//! a Cross Dissolve on an edit, and a Cross Fade +3 dB where the clips'
//! sounds meet there; the Cut button makes an edit a cut again. They are
//! written as the editor whose metadata OTIO files carry writes its own.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, Project, TrackKind, Transition};
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

/// Frames a dissolve of `length` takes before its edit and after it.
pub fn split(length: i64, align: Align) -> (i64, i64) {
    match align {
        Align::Center => (length - length / 2, length / 2),
        Align::Start => (0, length),
        Align::End => (length, 0),
    }
}

/// The OTIO transition a dissolve of `frames` is written as: on pictures a
/// Cross Dissolve, its curve's key frames over its length; on sounds a
/// Cross Fade +3 dB.
fn otio(sound: bool, frames: i64) -> Value {
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
    json!({
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
    })
}

/// The span of `c`'s picture (or sound).
fn span(c: &Clip, sound: bool) -> Option<(i64, i64)> {
    if sound {
        c.audio_range()
    } else {
        c.video.then(|| (c.start, c.end()))
    }
}

/// The edits of track `ti`'s pictures (or sounds): where a clip's starts
/// or ends.
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

/// The edit of track `ti`'s pictures (or sounds) nearest `at`; ties go to
/// the earlier.
fn nearest(p: &Project, ti: usize, sound: bool, at: i64) -> Option<i64> {
    edits(p, ti, sound)
        .into_iter()
        .min_by_key(|&e| ((e - at).abs(), e))
}

/// The clips of track `ti` whose pictures (or sounds) end at `e` and start
/// there.
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

/// The compound clip that plays the clips it holds on either side of edit
/// `e` of track `ti`: its edges take no dissolve, which its clips would
/// play over.
fn compound_at(p: &Project, ti: usize, sound: bool, e: i64) -> Option<String> {
    let (from, to) = sides(p, ti, sound, e);
    [from, to].into_iter().flatten().find(|id| {
        p.tracks[ti]
            .clips
            .iter()
            .any(|c| &c.id == id && crate::render::plays_inside(c))
    })
}

/// Frames a dissolve at edit `e` between clips `from` and `to` (`None`: a
/// gap) may take before the edit and after it: as far as each clip's
/// picture (or sound) or the gap goes, as far as each clip's media goes
/// past its edge, and up to the frames of the track's transitions on other
/// edits.
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
    // What else of the track is there: the nearest edge before and after.
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
    let media = |c: &Clip, tail: bool| match p.source_frames(c) {
        _ if c.is_generator() => i64::MAX / 4,
        Some(frames) => c.room(tail, frames),
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
            // On this edit: it goes.
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

/// One transition [`dissolve`] made.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Made {
    pub id: String,
    pub sound: bool,
    /// Its edit, and its frames before and after it.
    pub cut: i64,
    pub before: i64,
    pub after: i64,
}

/// Puts a dissolve of `frames` aligned by `align` on edit `e` of track
/// `ti`'s pictures (or sounds), between the clips that meet there or a
/// clip and a gap, shortened to the frames there are; none where no clip
/// starts or ends there, or no frame is free. Transitions already on that
/// edit go.
fn put(p: &mut Project, ti: usize, sound: bool, e: i64, frames: i64, align: Align) -> Option<Made> {
    let (from, to) = sides(p, ti, sound, e);
    if from.is_none() && to.is_none() || compound_at(p, ti, sound, e).is_some() {
        return None;
    }
    let (want_before, want_after) = split(frames, align);
    let (room_before, room_after) = room(p, ti, sound, e, from.as_deref(), to.as_deref());
    let (before, after) = (want_before.min(room_before), want_after.min(room_after));
    if before + after == 0 {
        return None;
    }
    let mut t = Transition {
        id: String::new(),
        sound,
        from,
        to,
        before,
        after,
        otio: otio(sound, before + after).into(),
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

/// The Dissolve button and Ctrl+T: a dissolve of `frames` aligned by
/// `align` on the edit of track `track` nearest `at`, on its pictures and
/// where its clips' sounds meet there (decision D50); `all`: on every edit
/// between two clips. `picture` or `sound` false leaves that one out. What
/// it made, or why there was nothing to make.
#[allow(clippy::too_many_arguments)]
pub fn dissolve(
    p: &mut Project,
    track: &str,
    at: i64,
    frames: i64,
    align: Align,
    picture: bool,
    sound: bool,
    all: bool,
) -> Result<Vec<Made>> {
    if frames < 1 {
        return Err(Error::invalid("a dissolve lasts a frame at least"));
    }
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
                    && let Some(m) = put(p, ti, band, e, frames, align)
                {
                    made.push(m);
                }
            }
        }
        return Ok(made);
    }
    // The picture's edit nearest `at`; the sounds of the same clips where
    // they meet, or the nearest sound edit on a track without pictures.
    let mut clips = None;
    if pictures {
        let e = nearest(p, ti, false, at)
            .ok_or_else(|| Error::invalid(format!("{track} has no edit to dissolve")))?;
        if let Some(c) = compound_at(p, ti, false, e) {
            return Err(Error::invalid(format!(
                "{c} is a compound clip, and its edges take no dissolve: decompose it first"
            )));
        }
        clips = Some(sides(p, ti, false, e));
        made.extend(put(p, ti, false, e, frames, align));
    }
    if sound {
        let e = match &clips {
            Some((from, to)) => sound_edit(p, ti, from.as_deref(), to.as_deref()),
            None => nearest(p, ti, true, at),
        };
        if let Some(e) = e {
            made.extend(put(p, ti, true, e, frames, align));
        }
    }
    if made.is_empty() {
        return Err(Error::invalid(
            "the clips at the edit have no frames past it to dissolve with",
        ));
    }
    Ok(made)
}

/// Where the sounds of clips `from` and `to` (`None`: a gap) meet on track
/// `ti`: the first's end and the second's start, when both have sound and
/// those are one frame; a lone clip's sound's edge otherwise.
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

/// The Cut button: the edit of track `track` nearest `at` becomes a cut,
/// its picture's transitions going and those of its clips' sounds (decision
/// D50); `all`: every transition of the track. `picture` or `sound` false
/// keeps those. The ids of those it took away.
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
        // As the Dissolve button finds them: the picture's edit nearest
        // `at`, and the sounds of its clips where they meet; or the nearest
        // sound edit when the picture is left alone.
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
        // 25 fps; m1 10 s and m2 4 s with sound.
        Fixture::new(&[(10.0, true), (4.0, true)])
    }

    fn req(f: &mut Fixture, method: &str, params: Value) -> Value {
        f.ok(json!({"method": method, "params": params})).result
    }

    /// `(sound, cut, before, after)` of each transition made.
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
        // Ten frames of m2 before its first.
        f.append("m2", 10, 60);
        let r = req(&mut f, "edit.dissolve", json!({"at": 48}));
        // A second centered on the edit is 13 + 12 frames: 10 before it.
        assert_eq!(made(&r), [(false, 50, 10, 12), (true, 50, 10, 12)]);
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
        // The project stays as it was.
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
    fn a_fade_to_black_past_the_last_clip_ends_the_program() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        req(&mut f, "edit.dissolve", json!({"at": 50, "length": "10f"}));
        // Centered: 5 frames after the clip's end.
        assert_eq!(f.editor.project().end(), 55);
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
        // The first ends at 32: 8 frames before the second's edit at 40.
        assert_eq!(made(&r), [(false, 20, 13, 12), (false, 40, 8, 12)]);
    }

    #[test]
    fn the_cut_button_takes_a_sound_fade_with_the_picture_left_alone() {
        let mut f = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
        f.append("m1", 0, 50);
        // Silent: c1's sound fades out into silence at 50.
        f.append("m3", 0, 50);
        req(&mut f, "edit.dissolve", json!({"at": 50, "picture": false}));
        let r = req(
            &mut f,
            "edit.remove_transition",
            json!({"at": 50, "picture": false}),
        );
        assert_eq!(r["removed"], json!(["t1"]));
    }

    #[test]
    fn a_compound_clips_edge_takes_no_dissolve() {
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
        let made = super::dissolve(
            &mut p,
            "V1",
            50,
            25,
            super::Align::Center,
            true,
            false,
            false,
        );
        assert!(made.is_err());
    }

    #[test]
    fn a_dissolve_of_odd_length_has_its_middle_frame_before_the_edit() {
        assert_eq!(super::split(7, super::Align::Center), (4, 3));
    }
}
