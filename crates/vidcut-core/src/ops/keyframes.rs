use std::collections::HashSet;

use serde_json::{Value, json};

use super::locate;
use crate::error::{Error, Result};
use crate::keyframes::{checked, checked_handle, ease_of, reshape};
use crate::model::{Clip, ClipAudio, Ease, Handle, Keyframe, Project};
use crate::validate::GAIN;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pick {
    pub frames: Option<Vec<i64>>,
    pub at: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyChange {
    pub value: Option<f64>,
    pub by: Option<f64>,
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

pub fn set(p: &mut Project, ids: &[String], pick: &Pick, change: &KeyChange) -> Result<()> {
    let value = change.value.map(checked).transpose()?;
    if change.by.is_some_and(|b| !b.is_finite()) {
        return Err(Error::invalid("`by` needs a number of dB"));
    }
    if value.is_some() && change.by.is_some() {
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
                if let Some(by) = change.by {
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

pub fn remove(p: &mut Project, ids: &[String], pick: &Pick) -> Result<()> {
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
            let Some(a) = c.audio.as_ref().filter(|a| wanted && a.is_keyed()) else {
                continue;
            };
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
            clips
                .push(json!({ "clip": c.id, "track": t.id, "param": "volume", "keyframes": keys }));
        }
    }
    if let Some(ids) = ids {
        for id in ids {
            locate(p, id)?;
        }
    }
    Ok(json!({ "clips": clips }))
}
