use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::keyframes::PictureValue;
use crate::model::{
    AreaShape, Border, CellShape, Clip, CompositeMode, Crop, DynamicZoom, Effect, EffectArea,
    EffectKeys, EffectKind, FilmGrain, GaussianBlur, Glow, GlowAlpha, GlowFraming, GlowOutput,
    GrainComposite, MosaicBlur, Picture, PictureTool, Project, Sharpen, TrackKind, Vignette,
    VignetteMode, VignetteShape, ZoomBox,
};
use crate::request::DynamicZoomSet;

use super::locate;

pub fn checked(p: &Picture) -> Result<Option<Picture>> {
    for v in PictureValue::ALL {
        if let Some(why) = crate::keyframes::value_problem(v, crate::keyframes::plain_channel(p, v))
        {
            return Err(Error::invalid(why));
        }
    }
    if let Some(z) = &p.dynamic_zoom {
        zoom_checked(z)?;
    }
    if let Some(why) = crate::keyframes::picture_problem(p) {
        return Err(Error::invalid(why));
    }
    let effects = p
        .effects
        .iter()
        .map(effect_checked)
        .collect::<Result<Vec<_>>>()?;
    let out = Picture {
        position: p.position.filter(|at| *at != Picture::MIDDLE),
        zoom: p.zoom.filter(|z| *z != [1.0, 1.0]),
        rotation: p.rotation.filter(|r| *r != 0.0),
        anchor: p.anchor.filter(|at| *at != Picture::MIDDLE),
        pitch: p.pitch.filter(|v| *v != 0.0),
        yaw: p.yaw.filter(|v| *v != 0.0),
        crop_softness: p.crop_softness.filter(|s| *s != 0.0),
        composite_mode: p.composite_mode.filter(|m| *m != CompositeMode::Normal),
        opacity: p.opacity.filter(|o| *o != 1.0),
        effects,
        ..p.clone()
    };
    Ok((!out.is_usual()).then_some(out))
}

fn within(v: Option<f64>, (low, high): (f64, f64), why: &str) -> Result<()> {
    match v.is_none_or(|v| v.is_finite() && (low..=high).contains(&v)) {
        true => Ok(()),
        false => Err(Error::invalid(why)),
    }
}

fn gaussian_checked(b: &GaussianBlur) -> Result<GaussianBlur> {
    for v in [b.strength, b.vertical_strength] {
        within(
            v,
            GaussianBlur::STRENGTHS,
            "a Gaussian Blur's strength is from 0 to 1",
        )?;
    }
    for k in [b.horizontal_channels, b.vertical_channels]
        .iter()
        .flatten()
        .flatten()
    {
        within(
            Some(*k),
            GaussianBlur::CHANNELS,
            "a Gaussian Blur's channel values are from 0 to 2",
        )?;
    }
    let usual = |v: f64| v != GaussianBlur::STRENGTH;
    let same = |k: &[f64; 4]| *k != GaussianBlur::SAME;
    Ok(GaussianBlur {
        strength: b.strength.filter(|v| usual(*v)),
        vertical_strength: b.vertical_strength.filter(|v| usual(*v)),
        horizontal_channels: b.horizontal_channels.filter(same),
        vertical_channels: b.vertical_channels.filter(same),
        border: b.border.filter(|v| *v != Border::default()),
        ..b.clone()
    })
}

fn mosaic_checked(m: &MosaicBlur) -> Result<MosaicBlur> {
    within(
        m.pixel_frequency,
        MosaicBlur::FREQUENCIES,
        "a Mosaic Blur's pixel_frequency is from 0.1 to 4000",
    )?;
    within(
        m.smooth_strength,
        MosaicBlur::SMOOTHS,
        "a Mosaic Blur's smooth_strength is from 0 to 1",
    )?;
    within(
        m.aliasing,
        MosaicBlur::ALIASINGS,
        "a Mosaic Blur's aliasing is from 0 to 2",
    )?;
    for v in [m.x_offset, m.y_offset] {
        within(
            v,
            MosaicBlur::OFFSETS,
            "a Mosaic Blur's offsets are from -1 to 1",
        )?;
    }
    within(
        m.aspect,
        MosaicBlur::STRETCHES,
        "a Mosaic Blur's aspect is from 0.001 to 1000",
    )?;
    for v in m.position.into_iter().flatten() {
        within(
            Some(v),
            MosaicBlur::PLACES,
            "a Mosaic Blur's position is from -1000000 to 1000000",
        )?;
    }
    Ok(MosaicBlur {
        pixel_frequency: m.pixel_frequency.filter(|v| *v != MosaicBlur::FREQUENCY),
        smooth_strength: m.smooth_strength.filter(|v| *v != 0.0),
        cell_shape: m.cell_shape.filter(|v| *v != CellShape::default()),
        aliasing: m.aliasing.filter(|v| *v != MosaicBlur::ALIASING),
        x_offset: m.x_offset.filter(|v| *v != 0.0),
        y_offset: m.y_offset.filter(|v| *v != 0.0),
        aspect: m.aspect.filter(|v| *v != MosaicBlur::STRETCH),
        position: m.position.filter(|v| *v != MosaicBlur::MIDDLE),
        border: m.border.filter(|v| *v != Border::default()),
        blur_alpha: m.blur_alpha.filter(|on| !*on),
        ..m.clone()
    })
}

fn glow_checked(g: &Glow) -> Result<Glow> {
    for (v, range, why) in [
        (g.threshold, Glow::UNIT, "a Glow's threshold is from 0 to 1"),
        (
            g.alpha_threshold,
            Glow::UNIT,
            "a Glow's alpha_threshold is from 0 to 1",
        ),
        (g.spread, Glow::SPREADS, "a Glow's spread is from 0 to 3"),
        (
            g.hv_ratio,
            Glow::RATIOS,
            "a Glow's hv_ratio is from -1 to 1",
        ),
        (g.gain, Glow::GAINS, "a Glow's gain is from 0 to 10"),
        (g.gamma, Glow::GAMMAS, "a Glow's gamma is from 0.1 to 10"),
        (
            g.saturation,
            Glow::SATURATIONS,
            "a Glow's saturation is from 0 to 10",
        ),
        (
            g.secondary_gain,
            Glow::GAINS,
            "a Glow's secondary_gain is from 0 to 10",
        ),
        (
            g.secondary_spread,
            Glow::SPREADS,
            "a Glow's secondary_spread is from 0 to 3",
        ),
        (
            g.atmosphere_strength,
            Glow::STRENGTHS,
            "a Glow's atmosphere_strength is from -5 to 5",
        ),
        (
            g.atmosphere_detail,
            Glow::UNIT,
            "a Glow's atmosphere_detail is from 0 to 1",
        ),
        (
            g.atmosphere_scale,
            Glow::SCALES,
            "a Glow's atmosphere_scale is from 0.01 to 100",
        ),
        (
            g.atmosphere_ratio,
            Glow::NOISE_RATIOS,
            "a Glow's atmosphere_ratio is from -0.99 to 0.99",
        ),
        (
            g.atmosphere_evolution,
            Glow::EVOLUTIONS,
            "a Glow's atmosphere_evolution is from 0 to 1000",
        ),
        (
            g.atmosphere_seethe,
            Glow::SEETHES,
            "a Glow's atmosphere_seethe is from 0 to 10",
        ),
        (g.opacity, Glow::UNIT, "a Glow's opacity is from 0 to 1"),
        (
            g.recovery,
            Glow::RECOVERIES,
            "a Glow's recovery is from 0 to 5",
        ),
    ] {
        within(v, range, why)?;
    }
    for v in g.relative_spread.iter().flatten() {
        within(
            Some(*v),
            Glow::RELATIVES,
            "a Glow's relative_spread values are from 0 to 2",
        )?;
    }
    for v in [g.color_filter, g.secondary_color]
        .iter()
        .flatten()
        .flatten()
    {
        within(
            Some(*v),
            Glow::COLORS,
            "a Glow's color values are from 0 to 1000000",
        )?;
    }
    let off = |v: Option<f64>, usual: f64| v.filter(|v| *v != usual);
    let white = |k: Option<[f64; 3]>| k.filter(|k| *k != Glow::WHITE);
    Ok(Glow {
        output: g.output.filter(|v| *v != GlowOutput::default()),
        input_alpha: g.input_alpha.filter(|v| *v != GlowAlpha::default()),
        threshold: off(g.threshold, Glow::THRESHOLD),
        alpha_threshold: off(g.alpha_threshold, Glow::ALPHA_THRESHOLD),
        spread: off(g.spread, Glow::SPREAD),
        hv_ratio: off(g.hv_ratio, 0.0),
        relative_spread: white(g.relative_spread),
        gain: off(g.gain, Glow::GAIN),
        gamma: off(g.gamma, 1.0),
        saturation: off(g.saturation, 1.0),
        color_filter: white(g.color_filter),
        secondary_gain: off(g.secondary_gain, Glow::SECONDARY_GAIN),
        secondary_spread: off(g.secondary_spread, Glow::SECONDARY_SPREAD),
        secondary_color: white(g.secondary_color),
        atmosphere_strength: off(g.atmosphere_strength, Glow::STRENGTH),
        atmosphere_detail: off(g.atmosphere_detail, Glow::DETAIL),
        atmosphere_scale: off(g.atmosphere_scale, Glow::SCALE),
        atmosphere_ratio: off(g.atmosphere_ratio, 0.0),
        atmosphere_evolution: off(g.atmosphere_evolution, 0.0),
        atmosphere_seethe: off(g.atmosphere_seethe, Glow::SEETHE),
        framing: g.framing.filter(|v| *v != GlowFraming::default()),
        composite_type: g.composite_type.filter(|v| *v != CompositeMode::Add),
        opacity: off(g.opacity, 1.0),
        recovery: off(g.recovery, 0.0),
        ..g.clone()
    })
}

fn grain_checked(g: &FilmGrain) -> Result<FilmGrain> {
    for (v, range, why) in [
        (
            g.opacity,
            FilmGrain::UNIT,
            "a Film Grain's opacity is from 0 to 1",
        ),
        (
            g.texture,
            FilmGrain::UNIT,
            "a Film Grain's texture is from 0 to 1",
        ),
        (
            g.grain_size,
            FilmGrain::UNIT,
            "a Film Grain's grain_size is from 0 to 1",
        ),
        (
            g.grain_aspect,
            FilmGrain::ASPECTS,
            "a Film Grain's grain_aspect is from 0.2 to 5",
        ),
        (
            g.grain_strength,
            FilmGrain::UNIT,
            "a Film Grain's grain_strength is from 0 to 1",
        ),
        (
            g.offset,
            FilmGrain::UNIT,
            "a Film Grain's offset is from 0 to 1",
        ),
        (
            g.symmetry,
            FilmGrain::UNIT,
            "a Film Grain's symmetry is from 0 to 1",
        ),
        (
            g.softness,
            FilmGrain::UNIT,
            "a Film Grain's softness is from 0 to 1",
        ),
        (
            g.saturation,
            FilmGrain::UNIT,
            "a Film Grain's saturation is from 0 to 1",
        ),
        (
            g.detail_loss,
            FilmGrain::DETAILS,
            "a Film Grain's detail_loss is from 0 to 2",
        ),
        (
            g.shadows,
            FilmGrain::UNIT,
            "a Film Grain's shadows is from 0 to 1",
        ),
        (
            g.midtones,
            FilmGrain::UNIT,
            "a Film Grain's midtones is from 0 to 1",
        ),
        (
            g.highlights,
            FilmGrain::UNIT,
            "a Film Grain's highlights is from 0 to 1",
        ),
    ] {
        within(v, range, why)?;
    }
    for v in g.channel_gains.iter().flatten() {
        within(
            Some(*v),
            FilmGrain::GAINS,
            "a Film Grain's channel_gains are from -10 to 10",
        )?;
    }
    let off = |v: Option<f64>, usual: f64| v.filter(|v| *v != usual);
    Ok(FilmGrain {
        grain_composite: g
            .grain_composite
            .filter(|v| *v != GrainComposite::default()),
        opacity: off(g.opacity, FilmGrain::OPACITY),
        texture: off(g.texture, FilmGrain::TEXTURE),
        grain_size: off(g.grain_size, FilmGrain::SIZE),
        grain_aspect: off(g.grain_aspect, 1.0),
        grain_strength: off(g.grain_strength, FilmGrain::STRENGTH),
        offset: off(g.offset, FilmGrain::OFFSET),
        symmetry: off(g.symmetry, FilmGrain::SYMMETRY),
        softness: off(g.softness, FilmGrain::SOFTNESS),
        saturation: off(g.saturation, 0.0),
        detail_loss: off(g.detail_loss, 0.0),
        channel_gains: g.channel_gains.filter(|k| *k != FilmGrain::WHITE),
        shadows: off(g.shadows, FilmGrain::TONE),
        midtones: off(g.midtones, FilmGrain::TONE),
        highlights: off(g.highlights, FilmGrain::TONE),
        ..g.clone()
    })
}

fn vignette_checked(v: &Vignette) -> Result<Vignette> {
    within(
        v.size,
        Vignette::SIZES,
        "a Vignette's size is from 0.01 to 2",
    )?;
    within(
        v.rotation,
        Vignette::TURNS,
        "a Vignette's rotation is from -360 to 360",
    )?;
    within(
        v.anamorphism,
        Vignette::STRETCHES,
        "a Vignette's anamorphism is from 0.2 to 5",
    )?;
    for c in v.center.into_iter().flatten() {
        within(
            Some(c),
            Vignette::PLACES,
            "a Vignette's center is from -1000000 to 1000000",
        )?;
    }
    within(
        v.softness,
        Vignette::SHARES,
        "a Vignette's softness is from 0 to 1",
    )?;
    within(
        v.transparency,
        Vignette::SHARES,
        "a Vignette's transparency is from 0 to 1",
    )?;
    if v.color.into_iter().flatten().any(|c| !c.is_finite()) {
        return Err(Error::invalid("a Vignette's color is three numbers"));
    }
    Ok(Vignette {
        operating_mode: v.operating_mode.filter(|m| *m != VignetteMode::default()),
        border_shape: v.border_shape.filter(|s| *s != VignetteShape::default()),
        size: v.size.filter(|s| *s != Vignette::SIZE),
        rotation: v.rotation.filter(|r| *r != 0.0),
        anamorphism: v.anamorphism.filter(|a| *a != Vignette::STRETCH),
        center: v.center.filter(|c| *c != Vignette::MIDDLE),
        softness: v.softness.filter(|s| *s != Vignette::SOFTNESS),
        transparency: v.transparency.filter(|t| *t != 0.0),
        color: v.color.filter(|c| *c != Vignette::BLACK),
        composite_type: v.composite_type.filter(|m| *m != Vignette::MIXING),
    })
}

fn sharpen_checked(s: &Sharpen) -> Result<Sharpen> {
    within(
        s.sharpen_amount,
        Sharpen::GAINS,
        "a Sharpen's amount is from 1 to 5",
    )?;
    within(
        s.fine_detail_size,
        Sharpen::SIZES,
        "a Sharpen's fine detail size is from 0.01 to 0.1",
    )?;
    within(
        s.fine_details,
        Sharpen::GAINS,
        "a Sharpen's fine details are from 1 to 5",
    )?;
    within(
        s.medium_details,
        Sharpen::GAINS,
        "a Sharpen's medium details are from 1 to 5",
    )?;
    within(
        s.large_details,
        Sharpen::GAINS,
        "a Sharpen's large details are from 1 to 5",
    )?;
    within(
        s.sharpen_chroma,
        Sharpen::GAINS,
        "a Sharpen's chroma is from 1 to 5",
    )?;
    let gain = |v: Option<f64>| v.filter(|g| *g != Sharpen::GAIN);
    Ok(Sharpen {
        sharpen_amount: s.sharpen_amount.filter(|a| *a != Sharpen::AMOUNT),
        fine_detail_size: s.fine_detail_size.filter(|z| *z != Sharpen::SIZE),
        fine_details: gain(s.fine_details),
        medium_details: gain(s.medium_details),
        large_details: gain(s.large_details),
        sharpen_chroma: gain(s.sharpen_chroma),
    })
}

fn keys_checked(e: &Effect) -> Result<Vec<EffectKeys>> {
    let mut keys = e.keys.clone();
    keys.retain(|k| !k.keys.is_empty());
    keys.sort_by_key(|k| k.value);
    if keys.windows(2).any(|w| w[0].value == w[1].value) {
        return Err(Error::invalid(
            "an effect's value has one list of keyframes",
        ));
    }
    let mut plain = e.clone();
    plain.keys.clear();
    for entry in &keys {
        let name = entry.value.name().replace('_', " ");
        if !e.takes(entry.value) {
            return Err(Error::invalid(format!(
                "a {} has no {name}",
                e.kind().name()
            )));
        }
        if entry.keys.windows(2).any(|w| w[0].frame >= w[1].frame) {
            return Err(Error::invalid(format!(
                "{name} keyframes out of order or on one frame"
            )));
        }
        for k in &entry.keys {
            for (h, before) in [(k.in_handle, true), (k.out_handle, false)] {
                if let Some(h) = h {
                    crate::keyframes::checked_handle(h, before)?;
                }
            }
            let mut one = plain.clone();
            one.write(entry.value, k.value);
            plain_effect_checked(&one).map_err(|err| {
                Error::invalid(format!(
                    "{name} keyframe at source frame {}: {}",
                    k.frame, err.message
                ))
            })?;
        }
    }
    Ok(keys)
}

pub fn effect_checked(e: &Effect) -> Result<Effect> {
    let keys = keys_checked(e)?;
    Ok(Effect {
        keys,
        ..plain_effect_checked(e)?
    })
}

fn plain_effect_checked(e: &Effect) -> Result<Effect> {
    within(e.blend, (0.0, 1.0), "an effect's blend is from 0 to 1")?;
    let kind = match &e.kind {
        EffectKind::GaussianBlur(b) => EffectKind::GaussianBlur(gaussian_checked(b)?),
        EffectKind::MosaicBlur(m) => EffectKind::MosaicBlur(mosaic_checked(m)?),
        EffectKind::Glow(g) => EffectKind::Glow(Box::new(glow_checked(g)?)),
        EffectKind::FilmGrain(g) => EffectKind::FilmGrain(Box::new(grain_checked(g)?)),
        EffectKind::Vignette(v) => EffectKind::Vignette(vignette_checked(v)?),
        EffectKind::Sharpen(s) => EffectKind::Sharpen(sharpen_checked(s)?),
    };
    Ok(Effect {
        kind,
        blend: e.blend.filter(|v| *v != 1.0),
        area: e.area.as_ref().map(area_checked).transpose()?,
        ..e.clone()
    })
}

fn area_checked(a: &EffectArea) -> Result<EffectArea> {
    for v in a.center.into_iter().flatten() {
        within(
            Some(v),
            EffectArea::PLACES,
            "an area's center is two fractions, each from -10 to 10",
        )?;
    }
    for v in a.size.into_iter().flatten() {
        within(
            Some(v),
            EffectArea::SIZES,
            "an area's size is two fractions, each from 0 to 10",
        )?;
    }
    within(
        a.rotation,
        EffectArea::TURNS,
        "an area's rotation is from -100000 to 100000 degrees",
    )?;
    within(
        a.softness,
        EffectArea::SHARES,
        "an area's softness is from 0 to 1",
    )?;
    let most = EffectArea::POINTS_KEPT;
    if !a.points.is_empty() && !(3..=most).contains(&a.points.len()) {
        return Err(Error::invalid(format!(
            "an area's points are from 3 to {most} corners, not {}",
            a.points.len()
        )));
    }
    for v in a.points.iter().flatten() {
        within(
            Some(*v),
            EffectArea::PLACES,
            "an area's points are fractions, each from -10 to 10",
        )?;
    }
    Ok(EffectArea {
        center: a.center.filter(|c| *c != EffectArea::MIDDLE),
        size: a.size.filter(|s| *s != EffectArea::WHOLE),
        rotation: a.rotation.filter(|r| *r != 0.0),
        softness: a.softness.filter(|s| *s != 0.0),
        ..a.clone()
    })
}

fn zoom_checked(z: &DynamicZoom) -> Result<()> {
    for b in [z.start, z.end] {
        if !b
            .center
            .iter()
            .all(|v| v.is_finite() && v.abs() <= DynamicZoom::REACH)
        {
            return Err(Error::invalid(
                "a Dynamic Zoom box's middle is two fractions of the picture, each from -100 to 100",
            ));
        }
        if !(DynamicZoom::SIZE.0..=DynamicZoom::SIZE.1).contains(&b.size) {
            return Err(Error::invalid(
                "a Dynamic Zoom box's size is a fraction of the picture, from 1/100 to 100",
            ));
        }
    }
    Ok(())
}

pub fn has_picture(c: &Clip) -> bool {
    c.video && c.title.is_none() && (c.generator.is_none() || c.draws())
}

pub fn takes_effects(c: &Clip) -> bool {
    has_picture(c) && c.generator.is_none()
}

pub(super) fn takes(p: &Project, ti: usize, c: &Clip) -> Result<()> {
    if c.title.is_some() {
        return Err(Error::invalid(format!(
            "{} is a title, placed by its own position, zoom and rotation (titles.set)",
            c.id
        )));
    }
    if c.generator.is_some() && !c.draws() {
        return Err(Error::invalid(format!(
            "{} is a block from another editor's file, which plays as a gap",
            c.id
        )));
    }
    if !c.video || p.tracks[ti].kind != TrackKind::Video {
        return Err(Error::invalid(format!("{} has no picture", c.id)));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PictureChange {
    pub position: Option<Option<[f64; 2]>>,
    pub zoom: Option<[f64; 2]>,
    pub rotation: Option<f64>,
    pub anchor: Option<Option<[f64; 2]>>,
    pub pitch: Option<f64>,
    pub yaw: Option<f64>,
    pub flip_horizontal: Option<bool>,
    pub flip_vertical: Option<bool>,
    pub transform_off: Option<bool>,
    pub crop: [Option<f64>; 4],
    pub crop_off: Option<bool>,
    pub crop_softness: Option<f64>,
    pub dynamic_zoom: Option<Option<DynamicZoomSet>>,
    pub composite_mode: Option<CompositeMode>,
    pub opacity: Option<f64>,
}

impl PictureChange {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn names(&self, v: PictureValue) -> bool {
        match v {
            PictureValue::Position => self.position.is_some(),
            PictureValue::Zoom => self.zoom.is_some(),
            PictureValue::Rotation => self.rotation.is_some(),
            PictureValue::Anchor => self.anchor.is_some(),
            PictureValue::Pitch => self.pitch.is_some(),
            PictureValue::Yaw => self.yaw.is_some(),
            PictureValue::CropLeft => self.crop[0].is_some(),
            PictureValue::CropRight => self.crop[1].is_some(),
            PictureValue::CropTop => self.crop[2].is_some(),
            PictureValue::CropBottom => self.crop[3].is_some(),
            PictureValue::CropSoftness => self.crop_softness.is_some(),
            PictureValue::Opacity => self.opacity.is_some(),
        }
    }
}

pub fn changed(old: &Picture, change: &PictureChange) -> Picture {
    let mut p = old.clone();
    if let Some(at) = change.position {
        p.position = at;
    }
    if let Some(z) = change.zoom {
        p.zoom = Some(z);
    }
    if let Some(r) = change.rotation {
        p.rotation = Some(r);
    }
    if let Some(at) = change.anchor {
        p.anchor = at;
    }
    if let Some(v) = change.pitch {
        p.pitch = Some(v);
    }
    if let Some(v) = change.yaw {
        p.yaw = Some(v);
    }
    if let Some(f) = change.flip_horizontal {
        p.flip_horizontal = f;
    }
    if let Some(f) = change.flip_vertical {
        p.flip_vertical = f;
    }
    if let Some(off) = change.transform_off {
        p.transform_off = off;
    }
    if let Some(off) = change.crop_off {
        p.crop_off = off;
    }
    let [left, right, top, bottom] = change.crop;
    let c = &mut p.crop;
    for (side, to) in [
        (&mut c.left, left),
        (&mut c.right, right),
        (&mut c.top, top),
        (&mut c.bottom, bottom),
    ] {
        if let Some(v) = to {
            *side = v;
        }
    }
    if let Some(s) = change.crop_softness {
        p.crop_softness = Some(s);
    }
    if let Some(z) = change.dynamic_zoom {
        p.dynamic_zoom = z.map(|z| z.on(p.dynamic_zoom.unwrap_or_default()));
    }
    if let Some(m) = change.composite_mode {
        p.composite_mode = Some(m);
    }
    if let Some(o) = change.opacity {
        p.opacity = Some(o);
    }
    p
}

const NEEDS: &str = "picture.set needs a position, zoom, rotation, anchor, pitch, yaw, flip, crop, crop softness, switch, Dynamic Zoom, composite mode or opacity";

pub fn set(p: &mut Project, ids: &[String], change: &PictureChange, at: Option<i64>) -> Result<()> {
    if change.is_empty() {
        return Err(Error::usage(NEEDS));
    }
    let mut applied = false;
    let mut missed = false;
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let c = &p.tracks[ti].clips[ci];
        takes(p, ti, c)?;
        let old = c.picture.clone().unwrap_or_default();
        let next = changed(&old, change);
        let frame = at.map(|t| c.content_at(t));
        let mut picture = old.clone();
        let mut wrote = false;
        for v in PictureValue::ALL {
            let was = crate::keyframes::plain_channel(&old, v);
            let now = crate::keyframes::plain_channel(&next, v);
            let named = change.names(v);
            if was == now && !(named && old.has_keys(v)) {
                wrote |= named;
                continue;
            }
            if old.has_keys(v) {
                match frame {
                    Some(f) => {
                        picture.put_channel(v, f, now);
                        wrote = true;
                    }
                    None => missed = true,
                }
            } else {
                crate::keyframes::write_channel(&mut picture, v, now);
                wrote = true;
            }
        }
        if next.flip_horizontal != old.flip_horizontal {
            picture.flip_horizontal = next.flip_horizontal;
            wrote = true;
        }
        if next.flip_vertical != old.flip_vertical {
            picture.flip_vertical = next.flip_vertical;
            wrote = true;
        }
        if (next.transform_off, next.crop_off) != (old.transform_off, old.crop_off) {
            picture.transform_off = next.transform_off;
            picture.crop_off = next.crop_off;
            wrote = true;
        }
        if next.dynamic_zoom != old.dynamic_zoom {
            picture.dynamic_zoom = next.dynamic_zoom;
            wrote = true;
        }
        if next.mode() != old.mode() {
            picture.composite_mode = next.composite_mode;
            wrote = true;
        }
        if !wrote {
            continue;
        }
        applied = true;
        let picture = checked(&picture)?;
        p.tracks[ti].clips[ci].picture = picture;
    }
    if !applied {
        return Err(if missed {
            Error::invalid("a value with keyframes needs `at` or a playhead")
        } else {
            Error::usage(NEEDS)
        });
    }
    Ok(())
}

pub fn clear(p: &mut Project, ids: &[String], tool: Option<PictureTool>) -> Result<()> {
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let c = &mut p.tracks[ti].clips[ci];
        let Some(old) = c.picture.take() else {
            continue;
        };
        let kept = match tool {
            Some(t) => cleared(&old, t),
            None => Picture {
                effects: old.effects,
                ..Picture::default()
            },
        };
        c.picture = (!kept.is_usual()).then_some(kept);
    }
    Ok(())
}

pub fn cleared(old: &Picture, tool: PictureTool) -> Picture {
    let old = old.clone();
    match tool {
        PictureTool::Transform => Picture {
            transform_off: old.transform_off,
            crop_off: old.crop_off,
            crop: old.crop,
            crop_left_keys: old.crop_left_keys,
            crop_right_keys: old.crop_right_keys,
            crop_top_keys: old.crop_top_keys,
            crop_bottom_keys: old.crop_bottom_keys,
            crop_softness: old.crop_softness,
            crop_softness_keys: old.crop_softness_keys,
            dynamic_zoom: old.dynamic_zoom,
            composite_mode: old.composite_mode,
            opacity: old.opacity,
            opacity_keys: old.opacity_keys,
            effects: old.effects,
            ..Picture::default()
        },
        PictureTool::Crop => {
            let mut kept = Picture {
                crop: Crop::default(),
                crop_softness: None,
                ..old
            };
            kept.clear_crop_keys();
            kept
        }
        PictureTool::DynamicZoom => Picture {
            dynamic_zoom: None,
            ..old
        },
        PictureTool::Composite => Picture {
            composite_mode: None,
            opacity: None,
            opacity_keys: Vec::new(),
            ..old
        },
    }
}

pub fn working(p: Picture) -> Picture {
    let mut p = p;
    if p.crop_off {
        p = cleared(&p, PictureTool::Crop);
    }
    if p.transform_off {
        p = cleared(&p, PictureTool::Transform);
    }
    p.transform_off = false;
    p.crop_off = false;
    p
}

pub fn shown(c: &Clip) -> Value {
    let p = c.picture.clone().unwrap_or_default();
    let crop = p.crop;
    json!({
        "clip": c.id,
        "position": p.place(),
        "zoom": p.scale(),
        "rotation": p.turn(),
        "anchor": p.pivot(),
        "pitch": p.pitch(),
        "yaw": p.yaw(),
        "flip_horizontal": p.flip_horizontal,
        "flip_vertical": p.flip_vertical,
        "transform_off": p.transform_off,
        "crop": {
            "left": crop.left, "right": crop.right, "top": crop.top, "bottom": crop.bottom,
            "softness": p.softness(),
        },
        "crop_off": p.crop_off,
        "dynamic_zoom": p.dynamic_zoom.as_ref().map(zoom_shown),
        "composite_mode": p.mode(),
        "opacity": p.opaque(),
        "effects": p.effects.iter().map(effect_shown).collect::<Vec<_>>(),
    })
}

pub fn effect_shown(e: &Effect) -> Value {
    let mut out = json!({
        "kind": e.kind(),
        "on": e.on,
        "blend": e.mixed(),
        "use_alpha": e.use_alpha,
        "render_cache": e.render_cache,
    });
    let more = match &e.kind {
        EffectKind::GaussianBlur(b) => json!({
            "strength": b.across(),
            "split": b.split,
            "vertical_strength": b.vertical_strength.unwrap_or(GaussianBlur::STRENGTH),
            "horizontal_channels": b.across_channels(),
            "vertical_channels": b.down_channels(),
            "border": b.edge(),
        }),
        EffectKind::MosaicBlur(m) => json!({
            "pixel_frequency": m.frequency(),
            "smooth_strength": m.smooth(),
            "cell_shape": m.shape(),
            "aliasing": m.box_size(),
            "x_offset": m.offsets()[0],
            "y_offset": m.offsets()[1],
            "aspect": m.stretch(),
            "position": m.place(),
            "border": m.edge(),
            "antialias": m.antialias,
            "blur_alpha": m.blurs_alpha(),
            "whole_cells": m.whole_cells,
        }),
        EffectKind::Glow(g) => json!({
            "output": g.shows(),
            "input_alpha": g.alpha(),
            "threshold": g.shine(),
            "alpha_threshold": g.alpha_shine(),
            "spread": g.wide(),
            "hv_ratio": g.ratio(),
            "relative_spread": g.relative(),
            "gain": g.bright(),
            "gamma": g.curve(),
            "saturation": g.colorful(),
            "color_filter": g.filter(),
            "secondary": g.secondary,
            "secondary_gain": g.second_bright(),
            "secondary_spread": g.second_wide(),
            "secondary_color": g.second_color(),
            "atmosphere": g.atmosphere,
            "atmosphere_strength": g.noise_strength(),
            "atmosphere_detail": g.noise_detail(),
            "atmosphere_scale": g.noise_scale(),
            "atmosphere_ratio": g.noise_ratio(),
            "atmosphere_evolution": g.noise_evolution(),
            "atmosphere_seethe": g.noise_seethe(),
            "atmosphere_tracked": g.atmosphere_tracked,
            "framing": g.frames(),
            "composite_type": g.over(),
            "opacity": g.shown(),
            "invert": g.invert,
            "recovery": g.recovers(),
        }),
        EffectKind::FilmGrain(g) => json!({
            "preset": g.preset(),
            "grain_composite": g.over(),
            "opacity": g.shown(),
            "grain_only": g.grain_only,
            "freeze": g.freeze,
            "texture": g.rough(),
            "grain_size": g.large(),
            "grain_aspect": g.stretch(),
            "grain_strength": g.strong(),
            "offset": g.middle(),
            "symmetry": g.lean(),
            "softness": g.soft(),
            "saturation": g.colorful(),
            "detail_loss": g.lost(),
            "channel_gains": g.gains(),
            "shadows": g.tones()[0],
            "midtones": g.tones()[1],
            "highlights": g.tones()[2],
            "animate": g.animate,
        }),
        EffectKind::Vignette(v) => json!({
            "operating_mode": v.mode(),
            "border_shape": v.shape(),
            "size": v.large(),
            "rotation": v.turn(),
            "anamorphism": v.stretch(),
            "center": v.middle(),
            "softness": v.soft(),
            "transparency": v.through(),
            "color": v.tint(),
            "composite_type": v.mixing(),
        }),
        EffectKind::Sharpen(s) => json!({
            "sharpen_amount": s.amount(),
            "fine_detail_size": s.size(),
            "fine_details": s.fine(),
            "medium_details": s.medium(),
            "large_details": s.large(),
            "sharpen_chroma": s.chroma(),
        }),
    };
    if let (Some(out), Some(more)) = (out.as_object_mut(), more.as_object()) {
        out.extend(more.clone());
    }
    out["area"] = e.area.as_ref().map_or(Value::Null, area_shown);
    out
}

fn area_shown(a: &EffectArea) -> Value {
    let points = (a.shape == AreaShape::Polygon).then(|| a.corners());
    json!({
        "shape": a.shape,
        "points": points,
        "center": a.middle(),
        "size": a.extent(),
        "rotation": a.turn(),
        "softness": a.soft(),
        "invert": a.invert,
        "follows": a.follows,
    })
}

pub fn zoom_shown(z: &DynamicZoom) -> Value {
    let shown = |b: &ZoomBox| json!({ "center": b.center, "size": b.size });
    json!({ "start": shown(&z.start), "end": shown(&z.end), "ease": z.ease })
}

pub fn get(p: &Project, id: &str) -> Result<Value> {
    let (ti, ci) = p
        .find_clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    let c = &p.tracks[ti].clips[ci];
    takes(p, ti, c)?;
    Ok(shown(c))
}

pub fn point(what: &str, raw: &[f64]) -> Result<Option<[f64; 2]>> {
    match raw {
        [] => Ok(None),
        [x, y] => Ok(Some([*x, *y])),
        _ => Err(Error::usage(format!(
            "a picture's {what} is two fractions, x and y, or none for the middle"
        ))),
    }
}

pub fn zoom(raw: &[f64]) -> Result<[f64; 2]> {
    match raw {
        [z] => Ok([*z, *z]),
        [x, y] => Ok([*x, *y]),
        _ => Err(Error::usage(
            "a picture's zoom is one number for both ways, or two, across and down",
        )),
    }
}

pub fn violation(p: &Project, ti: usize, c: &Clip) -> Option<String> {
    let picture = c.picture.as_ref()?;
    if let Err(e) = takes(p, ti, c) {
        return Some(e.message);
    }
    checked(picture)
        .err()
        .map(|e| format!("{}: {}", c.id, e.message))
}
