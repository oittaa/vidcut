use std::collections::HashSet;

use serde_json::{Value, json};

use super::locate;
use crate::error::{Error, Result};
use crate::keyframes::{PictureValue, checked, checked_handle, ease_of, reshape};
use crate::model::{
    Clip, ClipAudio, Ease, Handle, Keyframe, LowerThirdLine, Picture, Project, Title, TitleKind,
};
use crate::request::{CopiedKey, KeyParam, KeyPicks, KeyValue};
use crate::validate::GAIN;

const ANCHOR_LINEAR: &str = "the anchor point's keyframes are linear: they take no ease or handles";
const SPEED_SET: &str =
    "a speed keyframe takes no value or handles: speed.set sets the speed of a section";
const SPEED_ADD: &str = "a new speed keyframe takes no value or ease: speed.set sets the speed \
     of a section, and keyframes.set its ease";

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
    picture_only(p, ids, param)?;
    if param == KeyParam::Speed {
        if value.is_some() || ease.is_some() {
            return Err(Error::usage(SPEED_ADD));
        }
        return super::speed::add_keyframe(p, ids, at);
    }
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
    let takes = |c: &Clip| c.audio_range().is_some();
    let targets: Vec<String> = ids
        .iter()
        .filter(|id| p.clip(id).is_some_and(takes))
        .cloned()
        .collect();
    if targets.is_empty() {
        for id in ids {
            locate(p, id)?;
        }
        return Err(Error::invalid(format!(
            "no clip of {} has sound",
            ids.join(", ")
        )));
    }
    let mut added = Vec::new();
    for id in &targets {
        let (a, c) = audio_mut(p, id)?;
        let frame = c.content_at(at);
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
    picked_as(c, keys, pick, "volume")
}

pub(super) fn picked_as(
    c: &Clip,
    keys: &[Keyframe],
    pick: &Pick,
    name: &str,
) -> Result<HashSet<i64>> {
    let mut out = HashSet::new();
    if pick.frames.is_none() && pick.at.is_none() {
        out.extend(keys.iter().map(|k| k.frame));
        return Ok(out);
    }
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
    picture_only(p, ids, param)?;
    if param == KeyParam::Speed {
        let shaped = change.value.is_some()
            || change.by.is_some()
            || change.in_handle.is_some()
            || change.out_handle.is_some();
        return match change.ease.filter(|_| !shaped) {
            Some(e) => super::speed::ease_keyframes(p, ids, pick, e),
            None => Err(Error::usage(SPEED_SET)),
        };
    }
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
    picture_only(p, ids, param)?;
    if param == KeyParam::Speed {
        return Err(Error::invalid("speed keyframes don't move"));
    }
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
        let chosen = picked(&c, &a.gain_keys, pick)?;
        if to.is_some() && chosen.len() != 1 {
            return Err(Error::usage(format!(
                "`to` moves one keyframe; {id} has {} chosen",
                chosen.len()
            )));
        }
        let moved = |f: i64| match (by, to) {
            (Some(k), _) => c.key_at(c.key_time(f) + k as f64).round() as i64,
            (_, Some(t)) => c.content_at(t),
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
    picture_only(p, ids, param)?;
    if param == KeyParam::Speed {
        return super::speed::remove_keyframes(p, ids, pick);
    }
    if let Some(v) = picture_param(param) {
        return remove_picture(p, ids, v, pick);
    }
    remove_volume(p, ids, pick)
}

fn remove_volume(p: &mut Project, ids: &[String], pick: &Pick) -> Result<()> {
    for id in ids {
        let (a, c) = keyed_audio(p, id)?;
        let chosen = picked(&c, &a.gain_keys, pick)?;
        reshape(&mut a.gain_keys, |marked| {
            marked.retain(|(k, _)| !chosen.contains(&k.frame));
        });
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
            if let Some(speed) = c.speed.as_ref().filter(|s| s.sections.len() > 1) {
                let keys: Vec<Value> = speed
                    .sections
                    .iter()
                    .skip(1)
                    .map(|s| {
                        let at = c.key_time(s.at).round() as i64;
                        let mut v = json!({
                            "frame": s.at,
                            "at": at,
                            "at_tc": p.settings.timecode(at),
                            "value": s.speed,
                        });
                        if s.ease.is_some() {
                            v["ease"] = json!(Ease::EaseInOut);
                        }
                        v
                    })
                    .collect();
                clips.push(
                    json!({ "clip": c.id, "track": t.id, "param": "speed", "keyframes": keys }),
                );
            }
            if let Some(picture) = &held_picture(c) {
                let rows = listed_values(p, c, &t.id, picture);
                let apart = c.title.as_ref().is_some_and(|t| t.upper.is_some());
                clips.extend(if apart {
                    with_line(rows, "lower")
                } else {
                    rows
                });
            }
            if let Some(title) = c.title.as_ref().filter(|t| t.upper.is_some()) {
                let picture = title.upper_view().as_picture();
                let rows = with_line(listed_values(p, c, &t.id, &picture), "upper");
                clips.extend(rows.into_iter().filter(|r| r["param"] != "opacity"));
            }
        }
    }
    for t in &p.subtitles {
        for q in &t.cues {
            if ids.is_none_or(|ids| ids.contains(&q.id))
                && let Some(picture) = &q.picture
            {
                clips.extend(listed_values(p, &q.as_clip(), &t.id, picture));
            }
        }
    }
    if let Some(ids) = ids {
        for id in ids {
            holder(p, id)?;
        }
    }
    Ok(json!({ "clips": clips }))
}

fn with_line(mut rows: Vec<Value>, line: &str) -> Vec<Value> {
    for row in rows.iter_mut().filter(|r| r["param"] != "opacity") {
        row["param"] = json!(format!("{line}_{}", row["param"].as_str().unwrap_or("")));
    }
    rows
}

pub fn on_line<T>(
    p: &mut Project,
    ids: &[String],
    param: KeyParam,
    work: impl FnOnce(&mut Project, KeyParam) -> Result<T>,
) -> Result<T> {
    let (value, line) = param.on_line();
    super::titles::on_line(p, ids, line, |p| work(p, value))
}

fn of_line(c: &Clip, line: Option<LowerThirdLine>) -> Clip {
    let mut c = c.clone();
    if line == Some(LowerThirdLine::Upper)
        && let Some(t) = c.title.as_mut()
    {
        t.swap_lines();
    }
    c
}

fn listed_values(p: &Project, c: &Clip, track: &str, picture: &Picture) -> Vec<Value> {
    PictureValue::ALL
        .into_iter()
        .filter(|v| picture.has_keys(*v))
        .map(|v| {
            json!({
                "clip": c.id,
                "track": track,
                "param": v.name(),
                "keyframes": listed(p, c, picture, v),
            })
        })
        .collect()
}

fn holder(p: &Project, id: &str) -> Result<Clip> {
    p.key_holder(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))
}

fn picture_only(p: &Project, ids: &[String], param: KeyParam) -> Result<()> {
    match (
        picture_param(param),
        ids.iter().find(|id| p.cue(id).is_some()),
    ) {
        (None, Some(id)) => Err(Error::invalid(format!(
            "{id} is a subtitle, which has no {} keyframes",
            param_name(param)
        ))),
        _ => Ok(()),
    }
}

fn keys_of(c: &Clip, param: KeyParam) -> &[Keyframe] {
    match (param, picture_param(param), &c.title) {
        (KeyParam::Speed, _, _) => &[],
        (_, Some(v), Some(t)) => t.keys_of(v),
        (_, Some(v), None) => c.picture.as_ref().map_or(&[], |p| p.keys_of(v)),
        (_, None, _) => c.audio.as_ref().map_or(&[], |a| a.gain_keys.as_slice()),
    }
}

fn param_name(param: KeyParam) -> &'static str {
    match param {
        KeyParam::Speed => "speed",
        _ => picture_param(param).map_or("volume", PictureValue::name),
    }
}

pub type ChosenKeys = (KeyParam, Vec<i64>);

fn listed_params(c: &Clip) -> impl Iterator<Item = KeyParam> + use<> {
    let apart = c.title.as_ref().is_some_and(|t| t.upper.is_some());
    KeyParam::ALL.into_iter().filter(move |&param| {
        if param.on_line().1.is_some() {
            return apart;
        }
        !apart || param.for_line(LowerThirdLine::Lower) == param
    })
}

pub fn chosen(p: &Project, id: &str, picks: Option<&[KeyPicks]>) -> Result<Vec<ChosenKeys>> {
    let whole = &holder(p, id)?;
    let wanted: Vec<(KeyParam, Option<Vec<i64>>)> = match picks {
        Some(picks) => picks
            .iter()
            .map(|k| (k.param, k.frames.as_ref().map(|f| f.to_vec())))
            .collect(),
        None => listed_params(whole)
            .filter(|&param| {
                let (value, line) = param.on_line();
                !keys_of(&of_line(whole, line), value).is_empty()
            })
            .map(|param| (param, None))
            .collect(),
    };
    let mut out: Vec<ChosenKeys> = Vec::new();
    for (param, frames) in wanted {
        if param == KeyParam::Speed {
            return Err(Error::invalid("speed keyframes don't copy"));
        }
        let (value, line) = param.on_line();
        let c = &of_line(whole, line);
        let keys = keys_of(c, value);
        if keys.is_empty() {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                param_name(value).replace('_', " ")
            )));
        }
        let pick = Pick { frames, at: None };
        let mut taken: Vec<i64> = match picture_param(value) {
            Some(v) => picked_name(c, keys, &pick, v)?,
            None => picked(c, keys, &pick)?,
        }
        .into_iter()
        .collect();
        taken.sort_unstable();
        match out.iter_mut().find(|(q, _)| *q == param) {
            Some((_, have)) => {
                have.extend(taken);
                have.sort_unstable();
                have.dedup();
            }
            None => out.push((param, taken)),
        }
    }
    if out.is_empty() {
        return Err(Error::invalid(format!("{id} has no keyframes")));
    }
    Ok(out)
}

pub fn copy(p: &Project, id: &str, picks: &[ChosenKeys]) -> Result<Vec<CopiedKey>> {
    let whole = &holder(p, id)?;
    let mut rows = Vec::new();
    for (param, frames) in picks {
        let (plain, line) = param.on_line();
        let c = &of_line(whole, line);
        let held = held_picture(c);
        let keys = keys_of(c, plain);
        for (i, k) in keys
            .iter()
            .enumerate()
            .filter(|(_, k)| frames.contains(&k.frame))
        {
            let value = match (picture_param(plain), &held) {
                (Some(v), Some(picture)) if v.pair() => {
                    KeyValue::Pair(picture.key_value(v, k.frame).unwrap_or([k.value, 0.0]))
                }
                _ => KeyValue::Scalar(k.value),
            };
            let at = c.key_time(k.frame).round() as i64;
            let bent = c
                .title
                .as_ref()
                .filter(|_| plain == KeyParam::Position)
                .and_then(|t| t.handles_at(k.frame));
            rows.push((
                at,
                CopiedKey {
                    param: *param,
                    offset: 0,
                    value,
                    ease: ease_of(keys, i).filter(|_| bent.is_none()),
                    in_handle: k.in_handle,
                    out_handle: k.out_handle,
                    in_point: bent.and_then(|h| h.in_handle),
                    out_point: bent.and_then(|h| h.out_handle),
                },
            ));
        }
    }
    let first = rows.iter().map(|(at, _)| *at).min().unwrap_or(0);
    rows.sort_by_key(|(at, k)| (*at, k.param as u8));
    Ok(rows
        .into_iter()
        .map(|(at, k)| CopiedKey {
            offset: at - first,
            ..k
        })
        .collect())
}

pub fn paste(p: &mut Project, ids: &[String], at: i64, keys: &[CopiedKey]) -> Result<Vec<Value>> {
    let Some(first) = keys.iter().map(|k| k.offset).min() else {
        return Err(Error::usage("keyframes.paste needs keyframes"));
    };
    let mut starts = Vec::new();
    for id in ids {
        starts.push(holder(p, id)?.start);
    }
    let Some(earliest) = starts.iter().copied().min() else {
        return Err(Error::usage("keyframes.paste needs clips"));
    };
    let mut out = Vec::new();
    for (id, start) in ids.iter().zip(starts) {
        let one = std::slice::from_ref(id);
        let mut placed = Vec::new();
        let thirds = title_mut(p, id).is_some_and(|t| t.kind == TitleKind::LowerThird);
        for k in keys {
            let t = start + at - earliest + k.offset - first;
            let param = if thirds { k.param } else { k.param.on_line().0 };
            let added = on_line(p, one, param, |p, param| {
                add(p, one, param, t, Some(k.value), None)
            })?;
            for (_, frame) in added {
                placed.push((*k, frame));
            }
        }
        for (k, frame) in placed {
            let pick = Pick {
                frames: Some(vec![frame]),
                at: None,
            };
            let change = match k.ease {
                Some(e) => KeyChange {
                    ease: Some(e),
                    ..KeyChange::default()
                },
                None => KeyChange {
                    in_handle: Some(k.in_handle),
                    out_handle: Some(k.out_handle),
                    ..KeyChange::default()
                },
            };
            let param = if thirds { k.param } else { k.param.on_line().0 };
            on_line(p, one, param, |p, param| {
                set(p, one, param, &pick, &change)?;
                if param == KeyParam::Position
                    && let Some(t) = title_mut(p, id).filter(|t| !t.moves())
                {
                    t.put_handles(frame, k.in_point, k.out_point);
                }
                Ok(())
            })?;
            out.push(json!({ "clip": id, "param": k.param, "value": k.value, "frame": frame }));
        }
    }
    Ok(out)
}

fn picture_param(param: KeyParam) -> Option<PictureValue> {
    match param {
        KeyParam::Volume => None,
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition => {
            Some(PictureValue::Position)
        }
        KeyParam::Zoom | KeyParam::UpperZoom | KeyParam::LowerZoom => Some(PictureValue::Zoom),
        KeyParam::Rotation | KeyParam::UpperRotation | KeyParam::LowerRotation => {
            Some(PictureValue::Rotation)
        }
        KeyParam::Anchor => Some(PictureValue::Anchor),
        KeyParam::Pitch => Some(PictureValue::Pitch),
        KeyParam::Yaw => Some(PictureValue::Yaw),
        KeyParam::CropLeft => Some(PictureValue::CropLeft),
        KeyParam::CropRight => Some(PictureValue::CropRight),
        KeyParam::CropTop => Some(PictureValue::CropTop),
        KeyParam::CropBottom => Some(PictureValue::CropBottom),
        KeyParam::CropSoftness => Some(PictureValue::CropSoftness),
        KeyParam::Opacity => Some(PictureValue::Opacity),
        KeyParam::Speed => None,
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

fn holds(c: &Clip) -> bool {
    super::picture::has_picture(c) || c.title.is_some()
}

fn held_picture(c: &Clip) -> Option<Picture> {
    c.title
        .as_ref()
        .map(crate::model::Title::as_picture)
        .or_else(|| c.picture.clone())
}

fn owned_picture(p: &Project, id: &str, v: PictureValue) -> Result<(Picture, Clip)> {
    if let Some(q) = p.cue(id) {
        if v == PictureValue::Anchor {
            return Err(Error::invalid(format!(
                "{id} is a subtitle, which has no anchor keyframes"
            )));
        }
        return Ok((q.picture.clone().unwrap_or_default(), q.as_clip()));
    }
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    if let Some(t) = &c.title {
        if !t.takes_keys(v) {
            return Err(Error::invalid(format!(
                "{id} is a {}, which has no {} keyframes",
                if t.moves() { "scroll" } else { "title" },
                v.name().replace('_', " ")
            )));
        }
        return Ok((t.as_picture(), c.clone()));
    }
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

fn title_mut<'a>(p: &'a mut Project, id: &str) -> Option<&'a mut Title> {
    let (ti, ci) = p.find_clip(id)?;
    p.tracks[ti].clips[ci].title.as_mut()
}

fn bends(c: &Clip, v: PictureValue) -> bool {
    v == PictureValue::Position && c.title.as_ref().is_some_and(|t| !t.moves())
}

fn ease_path(p: &mut Project, id: &str, frames: impl IntoIterator<Item = i64>, ease: Ease) {
    if let Some(t) = title_mut(p, id) {
        for frame in frames {
            t.ease_position(frame, ease);
        }
    }
}

fn save(p: &mut Project, id: &str, picture: &Picture) -> Result<()> {
    if let Some((ti, ci)) = p.find_cue(id) {
        super::subtitles::unlocked(p, ti)?;
        p.subtitles[ti].cues[ci].picture = super::picture::checked(picture)?;
        return Ok(());
    }
    let (ti, ci) = locate(p, id)?;
    let c = &mut p.tracks[ti].clips[ci];
    let Some(mut t) = c.title.clone() else {
        c.picture = super::picture::checked(picture)?;
        return Ok(());
    };
    if picture
        .zoom_keys
        .iter()
        .zip(&picture.zoom_y)
        .any(|(k, y)| k.value != *y)
    {
        return Err(Error::invalid("a title's zoom is one number"));
    }
    t.take_picture(picture);
    c.title = Some(super::titles::checked(&t)?);
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
    if v == PictureValue::Anchor && ease.is_some_and(|e| !e.is_linear()) {
        return Err(Error::invalid(ANCHOR_LINEAR));
    }
    let value = value.map(|n| numbers(v, n)).transpose()?;
    let targets: Vec<String> = ids
        .iter()
        .filter(|id| p.cue(id).is_some() || p.clip(id).is_some_and(holds))
        .cloned()
        .collect();
    if targets.is_empty() {
        for id in ids {
            locate(p, id)?;
        }
        return Err(Error::invalid(format!(
            "no clip of {} has a picture",
            ids.join(", ")
        )));
    }
    let mut added = Vec::new();
    for id in &targets {
        let (mut picture, c) = owned_picture(p, id, v)?;
        let frame = c.content_at(at);
        let n = value.unwrap_or_else(|| match (v, &c.title) {
            (PictureValue::Position, Some(t)) => t.place_at(frame as f64),
            _ => picture.channel_at(v, frame as f64),
        });
        picture.put_channel(v, frame, n);
        let path = bends(&c, v);
        if let Some(e) = ease.filter(|_| !path) {
            picture.mark_ease(v, frame, e);
        }
        save(p, id, &picture)?;
        if let Some(e) = ease.filter(|_| path) {
            ease_path(p, id, [frame], e);
        }
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
    let value = change.value.map(|n| numbers(v, n)).transpose()?;
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
    let curved = change.ease.is_some_and(|e| !e.is_linear())
        || change.in_handle.flatten().is_some()
        || change.out_handle.flatten().is_some();
    if v == PictureValue::Anchor && curved {
        return Err(Error::invalid(ANCHOR_LINEAR));
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
        let (mut picture, c) = owned_picture(p, id, v)?;
        if !picture.has_keys(v) {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                v.name().replace('_', " ")
            )));
        }
        let chosen = picked_name(&c, picture.keys_of(v), pick, v)?;
        let path = bends(&c, v);
        for frame in &chosen {
            let mut next =
                value.unwrap_or_else(|| picture.key_value(v, *frame).unwrap_or([0.0, 0.0]));
            if let Some(by) = by {
                next = [next[0] + by[0], next[1] + by[1]];
            }
            if value.is_some() || by.is_some() {
                picture.put_channel(v, *frame, next);
            }
            if let Some(e) = change.ease.filter(|_| !path) {
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
        if let Some(e) = change.ease.filter(|_| path) {
            ease_path(p, id, chosen, e);
        }
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
        let (mut picture, c) = owned_picture(p, id, v)?;
        if !picture.has_keys(v) {
            return Err(Error::invalid(format!(
                "{id} has no {} keyframes",
                v.name().replace('_', " ")
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
            (Some(k), _) => c.key_at(c.key_time(f) + k as f64).round() as i64,
            (_, Some(t)) => c.content_at(t),
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
        if v == PictureValue::Position
            && let Some(t) = title_mut(p, id)
        {
            t.move_handles(|f| if chosen.contains(&f) { moved(f) } else { f });
        }
        save(p, id, &picture)?;
    }
    Ok(())
}

fn remove_picture(p: &mut Project, ids: &[String], v: PictureValue, pick: &Pick) -> Result<()> {
    for id in ids {
        let (mut picture, c) = owned_picture(p, id, v)?;
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
    picked_as(c, keys, pick, &v.name().replace('_', " "))
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
            let path = c.title.as_ref().filter(|_| v == PictureValue::Position);
            if let Some(t) = path
                && let Some(h) = t.handles_at(k.frame)
            {
                row["ease"] = json!(t.position_ease(k.frame));
                row["in"] = json!(h.in_handle);
                row["out"] = json!(h.out_handle);
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
