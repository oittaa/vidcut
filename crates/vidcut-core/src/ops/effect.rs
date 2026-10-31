use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Border, Effect, EffectKind, EffectType, Project};

use super::locate;
use super::picture::{checked, effect_checked, takes};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectChange {
    pub on: Option<bool>,
    pub blend: Option<f64>,
    pub use_alpha: Option<bool>,
    pub render_cache: Option<bool>,
    pub strength: Option<f64>,
    pub split: Option<bool>,
    pub vertical_strength: Option<f64>,
    pub horizontal_channels: Option<[f64; 4]>,
    pub vertical_channels: Option<[f64; 4]>,
    pub border: Option<Border>,
}

pub fn add(p: &mut Project, ids: &[String], kind: EffectType) -> Result<Value> {
    let mut out = Vec::new();
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        takes(p, ti, &p.tracks[ti].clips[ci])?;
        let picture = p.tracks[ti].clips[ci]
            .picture
            .get_or_insert_with(Default::default);
        picture.effects.push(kind.made());
        out.push(json!({ "clip": id, "effect": picture.effects.len() - 1 }));
    }
    Ok(json!(out))
}

fn effects_of<'a>(p: &'a mut Project, id: &str, n: usize) -> Result<&'a mut Vec<Effect>> {
    let (ti, ci) = locate(p, id)?;
    takes(p, ti, &p.tracks[ti].clips[ci])?;
    let effects = p.tracks[ti].clips[ci]
        .picture
        .as_mut()
        .map(|x| &mut x.effects)
        .filter(|e| n < e.len());
    effects.ok_or_else(|| {
        Error::not_found(format!(
            "{id} has no effect {n}; a clip's effects count from 0"
        ))
    })
}

fn tidied(p: &mut Project, id: &str) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    let c = &mut p.tracks[ti].clips[ci];
    if let Some(picture) = c.picture.take() {
        c.picture = checked(&picture)?;
    }
    Ok(())
}

pub fn set(p: &mut Project, id: &str, n: usize, change: &EffectChange) -> Result<()> {
    if *change == EffectChange::default() {
        return Err(Error::usage("effects.set needs a value to change"));
    }
    let e = &mut effects_of(p, id, n)?[n];
    let mut next = e.clone();
    if let Some(v) = change.on {
        next.on = v;
    }
    if let Some(v) = change.blend {
        next.blend = Some(v);
    }
    if let Some(v) = change.use_alpha {
        next.use_alpha = v;
    }
    if let Some(v) = change.render_cache {
        next.render_cache = v;
    }
    match &mut next.kind {
        EffectKind::GaussianBlur(b) => {
            if let Some(v) = change.strength {
                b.strength = Some(v);
            }
            if let Some(v) = change.split {
                b.split = v;
            }
            if let Some(v) = change.vertical_strength {
                b.vertical_strength = Some(v);
            }
            if let Some(v) = change.horizontal_channels {
                b.horizontal_channels = Some(v);
            }
            if let Some(v) = change.vertical_channels {
                b.vertical_channels = Some(v);
            }
            if let Some(v) = change.border {
                b.border = Some(v);
            }
        }
    }
    *e = effect_checked(&next)?;
    Ok(())
}

pub fn reset(p: &mut Project, id: &str, n: usize) -> Result<()> {
    let e = &mut effects_of(p, id, n)?[n];
    *e = Effect {
        on: e.on,
        render_cache: e.render_cache,
        ..e.kind().made()
    };
    Ok(())
}

pub fn remove(p: &mut Project, id: &str, n: usize) -> Result<()> {
    effects_of(p, id, n)?.remove(n);
    tidied(p, id)
}

pub fn shift(p: &mut Project, id: &str, n: usize, to: usize) -> Result<()> {
    let effects = effects_of(p, id, n)?;
    if to >= effects.len() {
        return Err(Error::invalid(format!(
            "{id} has {} effects; `to` counts from 0",
            effects.len()
        )));
    }
    let e = effects.remove(n);
    effects.insert(to, e);
    Ok(())
}

pub fn channels(what: &str, raw: &[f64]) -> Result<[f64; 4]> {
    <[f64; 4]>::try_from(raw).map_err(|_| {
        Error::usage(format!(
            "{what} is four numbers: red, green, blue and alpha"
        ))
    })
}
