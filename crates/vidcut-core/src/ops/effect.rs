use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{
    Effect, EffectKind, EffectType, FilmGrain, GaussianBlur, Glow, MosaicBlur, Project, Vignette,
};
use crate::request::EffectChange;

use super::locate;
use super::picture::{checked, effect_checked, takes};

pub fn add(p: &mut Project, ids: &[String], kind: EffectType) -> Result<Value> {
    let mut out = Vec::new();
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        takes(p, ti, &p.tracks[ti].clips[ci])?;
        if p.tracks[ti].clips[ci].generator.is_some() {
            return Err(Error::invalid(format!(
                "{id} is a generator, which takes no effects"
            )));
        }
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

fn taken<T>(from: &mut Option<T>, to: &mut T) {
    if let Some(v) = from.take() {
        *to = v;
    }
}

fn given<T>(from: &mut Option<T>, to: &mut Option<T>) {
    if from.is_some() {
        *to = from.take();
    }
}

fn gaussian_values(c: &mut EffectChange, b: &mut GaussianBlur) -> Result<()> {
    given(&mut c.strength, &mut b.strength);
    taken(&mut c.split, &mut b.split);
    given(&mut c.vertical_strength, &mut b.vertical_strength);
    if let Some(k) = c.horizontal_channels.take() {
        b.horizontal_channels = Some(channels("horizontal_channels", &k)?);
    }
    if let Some(k) = c.vertical_channels.take() {
        b.vertical_channels = Some(channels("vertical_channels", &k)?);
    }
    given(&mut c.border, &mut b.border);
    Ok(())
}

fn mosaic_values(c: &mut EffectChange, m: &mut MosaicBlur) -> Result<()> {
    given(&mut c.pixel_frequency, &mut m.pixel_frequency);
    given(&mut c.smooth_strength, &mut m.smooth_strength);
    given(&mut c.cell_shape, &mut m.cell_shape);
    given(&mut c.aliasing, &mut m.aliasing);
    given(&mut c.x_offset, &mut m.x_offset);
    given(&mut c.y_offset, &mut m.y_offset);
    given(&mut c.aspect, &mut m.aspect);
    if let Some(at) = c.position.take() {
        let at = <[f64; 2]>::try_from(at)
            .map_err(|_| Error::usage("position is two numbers: x and y"))?;
        m.position = Some(at);
    }
    given(&mut c.border, &mut m.border);
    taken(&mut c.antialias, &mut m.antialias);
    given(&mut c.blur_alpha, &mut m.blur_alpha);
    taken(&mut c.whole_cells, &mut m.whole_cells);
    Ok(())
}

fn color(what: &str, raw: Option<Vec<f64>>, to: &mut Option<[f64; 3]>) -> Result<()> {
    if let Some(k) = raw {
        let k = <[f64; 3]>::try_from(k)
            .map_err(|_| Error::usage(format!("{what} is three numbers: red, green and blue")))?;
        *to = Some(k);
    }
    Ok(())
}

fn glow_values(c: &mut EffectChange, g: &mut Glow) -> Result<()> {
    given(&mut c.output, &mut g.output);
    given(&mut c.input_alpha, &mut g.input_alpha);
    given(&mut c.threshold, &mut g.threshold);
    given(&mut c.alpha_threshold, &mut g.alpha_threshold);
    given(&mut c.spread, &mut g.spread);
    given(&mut c.hv_ratio, &mut g.hv_ratio);
    color(
        "relative_spread",
        c.relative_spread.take(),
        &mut g.relative_spread,
    )?;
    given(&mut c.gain, &mut g.gain);
    given(&mut c.gamma, &mut g.gamma);
    given(&mut c.saturation, &mut g.saturation);
    color("color_filter", c.color_filter.take(), &mut g.color_filter)?;
    taken(&mut c.secondary, &mut g.secondary);
    given(&mut c.secondary_gain, &mut g.secondary_gain);
    given(&mut c.secondary_spread, &mut g.secondary_spread);
    color(
        "secondary_color",
        c.secondary_color.take(),
        &mut g.secondary_color,
    )?;
    taken(&mut c.atmosphere, &mut g.atmosphere);
    given(&mut c.atmosphere_strength, &mut g.atmosphere_strength);
    given(&mut c.atmosphere_detail, &mut g.atmosphere_detail);
    given(&mut c.atmosphere_scale, &mut g.atmosphere_scale);
    given(&mut c.atmosphere_ratio, &mut g.atmosphere_ratio);
    given(&mut c.atmosphere_evolution, &mut g.atmosphere_evolution);
    given(&mut c.atmosphere_seethe, &mut g.atmosphere_seethe);
    taken(&mut c.atmosphere_tracked, &mut g.atmosphere_tracked);
    given(&mut c.framing, &mut g.framing);
    given(&mut c.composite_type, &mut g.composite_type);
    given(&mut c.opacity, &mut g.opacity);
    taken(&mut c.invert, &mut g.invert);
    given(&mut c.recovery, &mut g.recovery);
    Ok(())
}

fn grain_values(c: &mut EffectChange, g: &mut FilmGrain) -> Result<()> {
    if let Some(preset) = c.preset.take() {
        g.take_preset(preset);
    }
    given(&mut c.grain_composite, &mut g.grain_composite);
    given(&mut c.opacity, &mut g.opacity);
    taken(&mut c.grain_only, &mut g.grain_only);
    taken(&mut c.freeze, &mut g.freeze);
    given(&mut c.texture, &mut g.texture);
    given(&mut c.grain_size, &mut g.grain_size);
    given(&mut c.grain_aspect, &mut g.grain_aspect);
    given(&mut c.grain_strength, &mut g.grain_strength);
    given(&mut c.offset, &mut g.offset);
    given(&mut c.symmetry, &mut g.symmetry);
    given(&mut c.softness, &mut g.softness);
    given(&mut c.saturation, &mut g.saturation);
    given(&mut c.detail_loss, &mut g.detail_loss);
    color(
        "channel_gains",
        c.channel_gains.take(),
        &mut g.channel_gains,
    )?;
    given(&mut c.shadows, &mut g.shadows);
    given(&mut c.midtones, &mut g.midtones);
    given(&mut c.highlights, &mut g.highlights);
    given(&mut c.optimized, &mut g.optimized);
    taken(&mut c.animate, &mut g.animate);
    Ok(())
}

fn vignette_values(c: &mut EffectChange, v: &mut Vignette) -> Result<()> {
    given(&mut c.operating_mode, &mut v.operating_mode);
    given(&mut c.border_shape, &mut v.border_shape);
    given(&mut c.size, &mut v.size);
    given(&mut c.rotation, &mut v.rotation);
    given(&mut c.anamorphism, &mut v.anamorphism);
    if let Some(at) = c.center.take() {
        let at =
            <[f64; 2]>::try_from(at).map_err(|_| Error::usage("center is two numbers: x and y"))?;
        v.center = Some(at);
    }
    given(&mut c.softness, &mut v.softness);
    given(&mut c.transparency, &mut v.transparency);
    color("color", c.color.take(), &mut v.color)?;
    given(&mut c.composite_type, &mut v.composite_type);
    Ok(())
}

pub fn set(p: &mut Project, id: &str, n: usize, change: &EffectChange) -> Result<()> {
    if *change == EffectChange::default() {
        return Err(Error::usage("effects.set needs a value to change"));
    }
    let e = &mut effects_of(p, id, n)?[n];
    let mut next = e.clone();
    let mut left = change.clone();
    taken(&mut left.on, &mut next.on);
    given(&mut left.blend, &mut next.blend);
    taken(&mut left.use_alpha, &mut next.use_alpha);
    taken(&mut left.render_cache, &mut next.render_cache);
    match &mut next.kind {
        EffectKind::GaussianBlur(b) => gaussian_values(&mut left, b)?,
        EffectKind::MosaicBlur(m) => mosaic_values(&mut left, m)?,
        EffectKind::Glow(g) => glow_values(&mut left, g)?,
        EffectKind::FilmGrain(g) => grain_values(&mut left, g)?,
        EffectKind::Vignette(v) => vignette_values(&mut left, v)?,
    }
    if left != EffectChange::default() {
        return Err(Error::invalid(format!(
            "effects.set has a value that a {} doesn't have",
            e.kind().name()
        )));
    }
    *e = effect_checked(&next)?;
    Ok(())
}

pub fn reset(p: &mut Project, id: &str, n: usize) -> Result<()> {
    let e = &mut effects_of(p, id, n)?[n];
    if e.on {
        *e = Effect {
            render_cache: e.render_cache,
            ..e.kind().made()
        };
    }
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

fn channels(what: &str, raw: &[f64]) -> Result<[f64; 4]> {
    <[f64; 4]>::try_from(raw).map_err(|_| {
        Error::usage(format!(
            "{what} is four numbers: red, green, blue and alpha"
        ))
    })
}
