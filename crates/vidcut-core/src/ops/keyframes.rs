use std::collections::HashSet;

use serde_json::{Value, json};

use super::locate;
use crate::error::{Error, Result};
use crate::keyframes::{PictureValue, checked, checked_handle, ease_of, reshape};
use crate::model::{Clip, ClipAudio, Ease, Handle, Keyframe, Picture, Project};
use crate::request::{KeyParam, KeyValue};
use crate::validate::GAIN;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pick {
    pub frames: Option<Vec<i64>>,
    pub at: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyChange {
    pub value: Option<KeyValue>,
    pub by: Option<KeyValue>,
    pub ease: Option<Ease>,
    pub in_handle: Option<Option<Handle>>,
    pub out_handle: Option<Option<Handle>>,
}

fn keyed(a: &ClipAudio) -> bool {
    a.is_keyed()
}

pub fn check_keyed_gain(p: &Project, ids: &[String], at: Option<i64>) -> Result<()> {
    let clips: Vec<&Clip> = ids.iter().filter_map(|id| p.clip(id)).collect();
    let all_keyed = clips.iter().all(|c| c.audio.as_ref().is_some_and(keyed));
    if clips.is_empty() || !all_keyed {
        return Ok(());
    }
    match at {
        None => Err(Error::usage(format!(
            "{} has volume keyframes: give `at` for the keyframe",
            clips[0].id
        ))),
        Some(t) if !clips.iter().any(|c| c.sounds_at(t)) => Err(Error::invalid(format!(
            "no clip with volume keyframes has sound at frame {t}"
        ))),
        Some(_) => Ok(()),
    }
}

pub fn put_value(a: &mut ClipAudio, frame: i64, value: f64) {
    reshape(&mut a.gain_keys, |marked| {
        match marked.iter_mut().find(|(k, _)| k.frame == frame) {
            Some((k, _)) => k.value = value,
            None => marked.push((
                Keyframe {
                    frame,
                    value,
                    in_handle: None,
                    out_handle: None,
                },
                None,
            )),
        }
    });
}

fn audio_mut<'a>(p: &'a mut Project, id: &str) -> Result<(&'a mut ClipAudio, Clip)> {
    let (ti, ci) = locate(p, id)?;
    let c = &mut p.tracks[ti].clips[ci];
    let shape = Clip {
        audio: None,
        ..c.clone()
    };
    let a = c
        .audio
        .as_mut()
        .ok_or_else(|| Error::invalid(format!("{id} has no sound")))?;
    Ok((a, shape))
}

pub fn add(
    p: &mut Project,
    ids: &[String],
    param: KeyParam,
    at: i64,
    value: Option<KeyValue>,
    ease: Option<Ease>,
) -> Result<Vec<(String, i64)>> {
    if let Some(v) = picture_param(param) {
        return add_picture(p, ids, v, at, value, ease);
    }
    let value = scalar(param, value)?;
    add_volume(p, ids, at, value, ease)
}

fn add_volume(
    p: &mut Project,
    ids: &[String],
    at: i64,
    value: Option<f64>,
    ease: Option<Ease>,
) -> Result<Vec<(String, i64)>> {
    let value = value.map(checked).transpose()?;
    let targets: Vec<String> = ids
        .iter()
        .filter(|id| p.clip(id).is_some_and(|c| c.sounds_at(at)))
        .cloned()
        .collect();
    if targets.is_empty() {
        for id in ids {
            locate(p, id)?;
        }
        return Err(Error::invalid(format!(
            "no clip of {} has sound at frame {at}",
            ids.join(", ")
        )));
    }
    let mut added = Vec::new();
    for id in &targets {
        let (a, c) = audio_mut(p, id)?;
        let frame = c.source_at(at);
        let v = value.unwrap_or_else(|| a.db_at(frame as f64));
        reshape(&mut a.gain_keys, |marked| {
            let i = match marked.iter().position(|(k, _)| k.frame == frame) {
                Some(i) => {
                    marked[i].0.value = v;
                    i
                }
                None => {
                    marked.push((
                        Keyframe {
                            frame,
                            value: v,
                            in_handle: None,
                            out_handle: None,
                        },
                        None,
                    ));
                    marked.len() - 1
                }
            };
            if let Some(e) = ease {
                eased(&mut marked[i], e);
            }
        });
        added.push((id.clone(), frame));
    }
    Ok(added)
}

fn eased((k, mark): &mut (Keyframe, Option<Ease>), e: Ease) {
    *mark = Some(e);
    if e.is_linear() {
        (k.in_handle, k.out_handle) = (None, None);
    }
}

fn picked(c: &Clip, keys: &[Keyframe], pick: &Pick) -> Result<HashSet<i64>> {
    let mut out = HashSet::new();
    if pick.frames.is_none() && pick.at.is_none() {
        out.extend(keys.iter().map(|k| k.frame));
        return Ok(out);
    }
    for &f in pick.frames.iter().flatten() {
        if !keys.iter().any(|k| k.frame == f) {
            return Err(Error::not_found(format!(
                "{} has no volume keyframe on source frame {f}",
                c.id
            )));
        }
        out.insert(f);
    }
    for &t in pick.at.iter().flatten() {
        let k = keys
            .iter()
            .find(|k| c.key_time(k.frame).round() as i64 == t)
            .ok_or_else(|| {
                Error::not_found(format!("{} has no volume keyframe at frame {t}", c.id))
            })?;
        out.insert(k.frame);
    }
    Ok(out)
}

fn keyed_audio<'a>(p: &'a mut Project, id: &str) -> Result<(&'a mut ClipAudio, Clip)> {
    let (a, c) = audio_mut(p, id)?;
    if !a.is_keyed() {
        return Err(Error::invalid(format!("{id} has no volume keyframes")));
    }
    Ok((a, c))
}

pub fn set(
    p: &mut Project,
    ids: &[String],
    param: KeyParam,
    pick: &Pick,
    change: &KeyChange,
) -> Result<()> {
    if let Some(v) = picture_param(param) {
        return set_picture(p, ids, v, pick, change);
    }
    let value = change
        .value
        .map(|v| scalar(param, Some(v)))
        .transpose()?
        .flatten()
        .map(checked)
        .transpose()?;
    let by = change
        .by
        .map(|v| scalar(param, Some(v)))
        .transpose()?
        .flatten();
    if by.is_some_and(|b| !b.is_finite()) {
        return Err(Error::invalid("`by` needs a number of dB"));
    }
    if value.is_some() && by.is_some() {
        return Err(Error::usage("give `value` or `by`, not both"));
    }
    let handles = change.in_handle.is_some() || change.out_handle.is_some();
    if change.ease.is_some() && handles {
        return Err(Error::usage("give `ease` or handles, not both"));
    }
    let in_handle = change
        .in_handle
        .map(|h| h.map(|h| checked_handle(h, true)).transpose())
        .transpose()?;
    let out_handle = change
        .out_handle
        .map(|h| h.map(|h| checked_handle(h, false)).transpose())
        .transpose()?;
    for id in ids {
        let (a, c) = keyed_audio(p, id)?;
        let chosen = picked(&c, &a.gain_keys, pick)?;
        reshape(&mut a.gain_keys, |marked| {
            for entry in marked.iter_mut().filter(|(k, _)| chosen.contains(&k.frame)) {
                if let Some(v) = value {
                    entry.0.value = v;
                }
                if let Some(by) = by {
                    entry.0.value = (entry.0.value + by).clamp(*GAIN.start(), *GAIN.end());
                }
                if let Some(e) = change.ease {
                    eased(entry, e);
                }
                if let Some(h) = in_handle {
                    (entry.0.in_handle, entry.1) = (h, None);
                }
                if let Some(h) = out_handle {
                    (entry.0.out_handle, entry.1) = (h, None);
                }
            }
        });
    }
    Ok(())
}

pub fn shift(
    p: &mut Project,
    ids: &[String],
    param: KeyParam,
    pick: &Pick,
    by: Option<i64>,
    to: Option<i64>,
) -> Result<()> {
    if let Some(v) = picture_param(param) {
        return shift_picture(p, ids, v, pick, by, to);
    }
    shift_volume(p, ids, pick, by, to)
}

fn shift_volume(
    p: &mut Project,
    ids: &[String],
    pick: &Pick,
    by: Option<i64>,
    to: Option<i64>,
) -> Result<()> {
    if by.is_some() == to.is_some() {
        return Err(Error::usage("give `by` or `to`"));
    }
    for id in ids {
        let (a, c) = keyed_audio(p, id)?;
        if c.speed() == 0.0 && by.is_some() {
            return Err(Error::invalid(format!(
                "{id} holds one frame: its keyframes have no time to move in"
            )));
        }
        let chosen = picked(&c, &a.gain_keys, pick)?;
        if to.is_some() && chosen.len() != 1 {
            return Err(Error::usage(format!(
                "`to` moves one keyframe; {id} has {} chosen",
                chosen.len()
            )));
        }
        let moved = |f: i64| match (by, to) {
            (Some(k), _) => c.sound_at(c.key_time(f) + k as f64).round() as i64,
            (_, Some(t)) => c.source_at(t),
            _ => f,
        };
        let mut taken: HashSet<i64> = a
            .gain_keys
            .iter()
            .map(|k| k.frame)
            .filter(|f| !chosen.contains(f))
            .collect();
        for f in &chosen {
            if !taken.insert(moved(*f)) {
                return Err(Error::invalid(format!(
                    "{id} already has a volume keyframe on source frame {}",
                    moved(*f)
                )));
            }
        }
        reshape(&mut a.gain_keys, |marked| {
            for (k, _) in marked.iter_mut().filter(|(k, _)| chosen.contains(&k.frame)) {
                k.frame = moved(k.frame);
            }
        });
    }
    Ok(())
}

pub fn remove(p: &mut Project, ids: &[String], param: KeyParam, pick: &Pick) -> Result<()> {
    if let Some(v) = picture_param(param) {
        return remove_picture(p, ids, v, pick);
    }
    remove_volume(p, ids, pick)
}

fn remove_volume(p: &mut Project, ids: &[String], pick: &Pick) -> Result<()> {
    for id in ids {
        let (a, c) = keyed_audio(p, id)?;
        let chosen = picked(&c, &a.gain_keys, pick)?;
        let first = a
            .gain_keys
            .iter()
            .find(|k| chosen.contains(&k.frame))
            .map(|k| k.value);
        reshape(&mut a.gain_keys, |marked| {
            marked.retain(|(k, _)| !chosen.contains(&k.frame));
        });
        if let (false, Some(v)) = (a.is_keyed(), first) {
            a.gain_db = v;
        }
    }
    Ok(())
}

pub fn list(p: &Project, ids: Option<&[String]>) -> Result<Value> {
    let mut clips = Vec::new();
    for t in &p.tracks {
        for c in &t.clips {
            let wanted = ids.is_none_or(|ids| ids.contains(&c.id));
            if !wanted {
                continue;
            }
            if let Some(a) = c.audio.as_ref().filter(|a| a.is_keyed()) {
                let keys: Vec<Value> = a
                    .gain_keys
                    .iter()
                    .enumerate()
                    .map(|(i, k)| {
                        let at = c.key_time(k.frame).round() as i64;
                        let mut v = json!({
                            "frame": k.frame,
                            "at": at,
                            "at_tc": p.settings.timecode(at),
                            "value": k.value,
                            "ease": ease_of(&a.gain_keys, i),
                        });
                        if let Some(h) = k.in_handle {
                            v["in"] = json!(h);
                        }
                        if let Some(h) = k.out_handle {
                            v["out"] = json!(h);
                        }
                        v
                    })
                    .collect();
                clips.push(
                    json!({ "clip": c.id, "track": t.id, "param": "volume", "keyframes": keys }),
                );
            }
            if let Some(picture) = &c.picture {
                for v in PictureValue::ALL {
                    if picture.has_keys(v) {
                        clips.push(json!({
                            "clip": c.id,
                            "track": t.id,
                            "param": v.name(),
                            "keyframes": listed(p, c, picture, v),
                        }));
                    }
                }
            }
        }
    }
    if let Some(ids) = ids {
        for id in ids {
            locate(p, id)?;
        }
    }
    Ok(json!({ "clips": clips }))
}

fn picture_param(param: KeyParam) -> Option<PictureValue> {
    match param {
        KeyParam::Volume => None,
        KeyParam::Position => Some(PictureValue::Position),
        KeyParam::Zoom => Some(PictureValue::Zoom),
        KeyParam::Rotation => Some(PictureValue::Rotation),
        KeyParam::CropLeft => Some(PictureValue::CropLeft),
        KeyParam::CropRight => Some(PictureValue::CropRight),
        KeyParam::CropTop => Some(PictureValue::CropTop),
        KeyParam::CropBottom => Some(PictureValue::CropBottom),
    }
}

fn scalar(param: KeyParam, value: Option<KeyValue>) -> Result<Option<f64>> {
    match value {
        None => Ok(None),
        Some(KeyValue::Scalar(n)) => Ok(Some(n)),
        Some(KeyValue::Pair(_)) => Err(Error::usage(format!(
            "{} takes one number",
            match param {
                KeyParam::Volume => "volume",
                other => picture_param(other).map_or("value", PictureValue::name),
            }
        ))),
    }
}

fn numbers(v: PictureValue, value: KeyValue) -> Result<[f64; 2]> {
    match (v.pair(), value) {
        (true, KeyValue::Pair(n)) => Ok(n),
        (true, KeyValue::Scalar(n)) if v == PictureValue::Zoom => Ok([n, n]),
        (false, KeyValue::Scalar(n)) => Ok([n, 0.0]),
        (true, KeyValue::Scalar(_)) => Err(Error::usage(format!("{} is x and y", v.name()))),
        (false, KeyValue::Pair(_)) => Err(Error::usage(format!("{} is one number", v.name()))),
    }
}

fn finite(v: PictureValue, value: [f64; 2]) -> Result<[f64; 2]> {
    let ok = match v {
        PictureValue::Position => value.iter().all(|n| n.is_finite() && n.abs() <= 100.0),
        PictureValue::Zoom => value
            .iter()
            .all(|n| n.is_finite() && (0.01..=100.0).contains(n)),
        PictureValue::Rotation => value[0].is_finite() && value[0].abs() <= 3600.0,
        _ => value[0].is_finite() && (0.0..1.0).contains(&value[0]),
    };
    if ok {
        Ok(value)
    } else {
        Err(Error::invalid(match v {
            PictureValue::Position => {
                "a picture's position is two fractions of the frame, each from -100 to 100"
            }
            PictureValue::Zoom => "a picture's zoom is from 1/100 to 100",
            PictureValue::Rotation => "a picture's rotation is degrees, from -3600 to 3600",
            _ => "a picture's crop is a fraction of it on each side, from 0, leaving some of it",
        }))
    }
}

fn shows(c: &Clip, at: i64) -> bool {
    super::picture::has_picture(c) && c.start <= at && at < c.end()
}

fn owned_picture(p: &Project, id: &str) -> Result<(Picture, Clip)> {
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    if !super::picture::has_picture(c) {
        return Err(Error::invalid(format!("{id} has no picture")));
    }
    Ok((
        c.picture.clone().unwrap_or_default(),
        Clip {
            picture: None,
            ..c.clone()
        },
    ))
}

fn save(p: &mut Project, id: &str, picture: &Picture) -> Result<()> {
    let stored = super::picture::checked(picture)?;
    let (ti, ci) = locate(p, id)?;
    p.tracks[ti].clips[ci].picture = stored;
    Ok(())
}

fn add_picture(
    p: &mut Project,
    ids: &[String],
    v: PictureValue,
    at: i64,
    value: Option<KeyValue>,
    ease: Option<Ease>,
) -> Result<Vec<(String, i64)>> {
    let value = value
        .map(|n| numbers(v, n).and_then(|n| finite(v, n)))
        .transpose()?;
    let targets: Vec<String> = ids
        .iter()
        .filter(|id| p.clip(id).is_some_and(|c| shows(c, at)))
        .cloned()
        .collect();
    if targets.is_empty() {
        for id in ids {
            locate(p, id)?;
        }
        return Err(Error::invalid(format!(
            "no clip of {} shows a picture at frame {at}",
            ids.join(", ")
        )));
    }
    let mut added = Vec::new();
    for id in &targets {
        let (mut picture, c) = owned_picture(p, id)?;
        let frame = c.source_at(at);
        let n = value.unwrap_or_else(|| picture.channel_at(v, frame as f64));
        picture.put_channel(v, frame, n);
        if let Some(e) = ease {
            picture.mark_ease(v, frame, e);
        }
        save(p, id, &picture)?;
        added.push((id.clone(), frame));
    }
    Ok(added)
}

fn set_picture(
    p: &mut Project,
    ids: &[String],
    v: PictureValue,
    pick: &Pick,
    change: &KeyChange,
) -> Result<()> {
    let value = change
        .value
        .map(|n| numbers(v, n).and_then(|n| finite(v, n)))
        .transpose()?;
    let by = change.by.map(|n| numbers(v, n)).transpose()?;
    if by
        .as_ref()
        .is_some_and(|b| b.iter().any(|n| !n.is_finite()))
    {
        return Err(Error::invalid("`by` needs finite numbers"));
    }
    if value.is_some() && by.is_some() {
        return Err(Error::usage("give `value` or `by`, not both"));
    }
    let handles = change.in_handle.is_some() || change.out_handle.is_some();
    if change.ease.is_some() && handles {
        return Err(Error::usage("give `ease` or handles, not both"));
    }
    let in_handle = change
        .in_handle
        .map(|h| h.map(|h| checked_handle(h, true)).transpose())
        .transpose()?;
    let out_handle = change
        .out_handle
        .map(|h| h.map(|h| checked_handle(h, false)).transpose())
        .transpose()?;
    for id in ids {
        let (mut picture, c) = owned_picture(p, id)?;
        if !picture.has_keys(v) {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                v.name().replace('_', " ")
            )));
        }
        let chosen = picked_name(&c, picture.keys_of(v), pick, v)?;
        for frame in &chosen {
            let mut next =
                value.unwrap_or_else(|| picture.key_value(v, *frame).unwrap_or([0.0, 0.0]));
            if let Some(by) = by {
                next = [next[0] + by[0], next[1] + by[1]];
                next = finite(v, next)?;
            }
            if value.is_some() || by.is_some() {
                picture.put_channel(v, *frame, next);
            }
            if let Some(e) = change.ease {
                picture.mark_ease(v, *frame, e);
            }
            if let Some(h) = in_handle {
                picture.set_handle(v, *frame, true, h);
            }
            if let Some(h) = out_handle {
                picture.set_handle(v, *frame, false, h);
            }
        }
        save(p, id, &picture)?;
    }
    Ok(())
}

fn shift_picture(
    p: &mut Project,
    ids: &[String],
    v: PictureValue,
    pick: &Pick,
    by: Option<i64>,
    to: Option<i64>,
) -> Result<()> {
    if by.is_some() == to.is_some() {
        return Err(Error::usage("give `by` or `to`"));
    }
    for id in ids {
        let (mut picture, c) = owned_picture(p, id)?;
        if !picture.has_keys(v) {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                v.name().replace('_', " ")
            )));
        }
        if c.speed() == 0.0 && by.is_some() {
            return Err(Error::invalid(format!(
                "{id} holds one frame: its keyframes have no time to move in"
            )));
        }
        let chosen = picked_name(&c, picture.keys_of(v), pick, v)?;
        if to.is_some() && chosen.len() != 1 {
            return Err(Error::usage(format!(
                "`to` moves one keyframe; {id} has {} chosen",
                chosen.len()
            )));
        }
        let moved = |f: i64| match (by, to) {
            (Some(k), _) => c.sound_at(c.key_time(f) + k as f64).round() as i64,
            (_, Some(t)) => c.source_at(t),
            _ => f,
        };
        let mut taken: HashSet<i64> = picture
            .keys_of(v)
            .iter()
            .map(|k| k.frame)
            .filter(|f| !chosen.contains(f))
            .collect();
        for f in &chosen {
            if !taken.insert(moved(*f)) {
                return Err(Error::invalid(format!(
                    "{id} already has a {} keyframe on source frame {}",
                    v.name().replace('_', " "),
                    moved(*f)
                )));
            }
        }
        picture.move_chosen(v, &chosen, moved);
        save(p, id, &picture)?;
    }
    Ok(())
}

fn remove_picture(p: &mut Project, ids: &[String], v: PictureValue, pick: &Pick) -> Result<()> {
    for id in ids {
        let (mut picture, c) = owned_picture(p, id)?;
        if !picture.has_keys(v) {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                v.name().replace('_', " ")
            )));
        }
        let chosen = picked_name(&c, picture.keys_of(v), pick, v)?;
        picture.drop_chosen(v, &chosen);
        save(p, id, &picture)?;
    }
    Ok(())
}

fn picked_name(c: &Clip, keys: &[Keyframe], pick: &Pick, v: PictureValue) -> Result<HashSet<i64>> {
    let mut out = HashSet::new();
    if pick.frames.is_none() && pick.at.is_none() {
        out.extend(keys.iter().map(|k| k.frame));
        return Ok(out);
    }
    let name = v.name().replace('_', " ");
    for &f in pick.frames.iter().flatten() {
        if !keys.iter().any(|k| k.frame == f) {
            return Err(Error::not_found(format!(
                "{} has no {name} keyframe on source frame {f}",
                c.id
            )));
        }
        out.insert(f);
    }
    for &t in pick.at.iter().flatten() {
        let k = keys
            .iter()
            .find(|k| c.key_time(k.frame).round() as i64 == t)
            .ok_or_else(|| {
                Error::not_found(format!("{} has no {name} keyframe at frame {t}", c.id))
            })?;
        out.insert(k.frame);
    }
    Ok(out)
}

fn listed(p: &Project, c: &Clip, picture: &Picture, v: PictureValue) -> Vec<Value> {
    picture
        .keys_of(v)
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let at = c.key_time(k.frame).round() as i64;
            let value = picture.key_value(v, k.frame).unwrap_or([k.value, 0.0]);
            let mut row = json!({
                "frame": k.frame,
                "at": at,
                "at_tc": p.settings.timecode(at),
                "value": if v.pair() { json!(value) } else { json!(value[0]) },
                "ease": ease_of(picture.keys_of(v), i),
            });
            if let Some(h) = k.in_handle {
                row["in"] = json!(h);
            }
            if let Some(h) = k.out_handle {
                row["out"] = json!(h);
            }
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::keyframes::PictureValue;
    use crate::test_support::Fixture;
    use serde_json::json;

    fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
        json!({ "method": method, "params": params })
    }

    #[test]
    fn linear_zoom_through_keyframes_add_is_a_quarter_at_frame_15() {
        let mut f = Fixture::new(&[(10.0, true)]);
        f.append("m1", 0, 100);
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": "zoom", "at": 0, "value": 0.1}),
        ));
        f.ok(req(
            "keyframes.add",
            json!({"clips": "c1", "param": "zoom", "at": 30, "value": 0.4}),
        ));
        let picture = f
            .editor
            .project()
            .clip("c1")
            .and_then(|c| c.picture.clone())
            .unwrap_or_default();
        assert_eq!(picture.scale_at(15.0), [0.25, 0.25]);
        assert!(picture.has_keys(PictureValue::Zoom));
    }
}
