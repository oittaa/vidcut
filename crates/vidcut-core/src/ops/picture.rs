use serde_json::{Value, json};

use crate::composition::named_well;
use crate::effect_keys::Keyed;
use crate::error::{Error, Result};
use crate::keyframes::PictureValue;
use crate::model::{
    Blur, BlurClipping, BlurFilter, Border, CellShape, Clip, CompositeMode, Composition, Crop,
    DynamicZoom, Effect, EffectKeys, EffectKind, EffectValue, FillMethod, FilmGrain, GaussianBlur,
    Glow, GlowAlpha, GlowFraming, GlowOutput, GrainComposite, Mask, MosaicBlur, Node, Pattern,
    Picture, PictureTool, Project, Sharpen, Tool, TrackKind, Tracker, Vignette, VignetteMode,
    VignetteShape, ZoomBox,
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
        composition: p
            .composition
            .as_ref()
            .map(composition_checked)
            .transpose()?,
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

fn keys_checked<T: Keyed>(
    e: &T,
    label: &str,
    plain_checked: impl Fn(&T) -> Result<T>,
) -> Result<Vec<EffectKeys>> {
    let mut keys = e.key_list().to_vec();
    keys.retain(|k| !k.keys.is_empty());
    keys.sort_by_key(|k| k.value);
    if keys.windows(2).any(|w| w[0].value == w[1].value) {
        return Err(Error::invalid(
            "an effect's value has one list of keyframes",
        ));
    }
    let mut plain = e.clone();
    plain.key_list_mut().clear();
    for entry in &keys {
        let name = entry.value.name().replace('_', " ");
        if !e.takes(entry.value) {
            return Err(Error::invalid(format!("a {label} has no {name}")));
        }
        if entry.keys.windows(2).any(|w| w[0].frame >= w[1].frame) {
            return Err(Error::invalid(format!(
                "{name} keyframes out of order or on one frame"
            )));
        }
        let ys = match entry.value.pair() {
            true => entry.keys.len(),
            false => 0,
        };
        if entry.y.len() != ys {
            return Err(Error::invalid(format!(
                "{name} keyframes need one y each on a point, and none on a number"
            )));
        }
        for (i, k) in entry.keys.iter().enumerate() {
            for (h, before) in [(k.in_handle, true), (k.out_handle, false)] {
                if let Some(h) = h {
                    crate::keyframes::checked_handle(h, before)?;
                }
            }
            let mut one = plain.clone();
            let y = entry.y.get(i).copied().unwrap_or(0.0);
            one.write_point(entry.value, [k.value, y]);
            plain_checked(&one).map_err(|err| {
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
    let keys = keys_checked(e, e.kind().name(), plain_effect_checked)?;
    Ok(Effect {
        keys,
        ..plain_effect_checked(e)?
    })
}

fn plain_effect_checked(e: &Effect) -> Result<Effect> {
    within(e.blend, (0.0, 1.0), "an effect's blend is from 0 to 1")?;
    Ok(Effect {
        kind: kind_checked(&e.kind)?,
        blend: e.blend.filter(|v| *v != 1.0),
        ..e.clone()
    })
}

fn kind_checked(kind: &EffectKind) -> Result<EffectKind> {
    Ok(match kind {
        EffectKind::GaussianBlur(b) => EffectKind::GaussianBlur(gaussian_checked(b)?),
        EffectKind::MosaicBlur(m) => EffectKind::MosaicBlur(mosaic_checked(m)?),
        EffectKind::Glow(g) => EffectKind::Glow(Box::new(glow_checked(g)?)),
        EffectKind::FilmGrain(g) => EffectKind::FilmGrain(Box::new(grain_checked(g)?)),
        EffectKind::Vignette(v) => EffectKind::Vignette(vignette_checked(v)?),
        EffectKind::Sharpen(s) => EffectKind::Sharpen(sharpen_checked(s)?),
    })
}

pub fn node_checked(n: &Node) -> Result<Node> {
    let keys = keys_checked(n, n.tool.label(), plain_node_checked)?;
    Ok(Node {
        keys,
        ..plain_node_checked(n)?
    })
}

fn plain_node_checked(n: &Node) -> Result<Node> {
    within(n.blend, (0.0, 1.0), "a node's blend is from 0 to 1")?;
    if !n.place.iter().all(|v| v.is_finite()) {
        return Err(Error::invalid("a node's place is two finite numbers"));
    }
    let tool = match &n.tool {
        Tool::Blur(b) => Tool::Blur(blur_checked(b)?),
        Tool::Effect(kind) => Tool::Effect(kind_checked(kind)?),
        Tool::Ellipse(m) => Tool::Ellipse(mask_checked(m, "ellipse")?),
        Tool::Rectangle(m) => Tool::Rectangle(mask_checked(m, "rectangle")?),
        Tool::Polygon(m) => Tool::Polygon(mask_checked(m, "polygon")?),
        Tool::Tracker(t) => Tool::Tracker(tracker_checked(t)?),
        t => t.clone(),
    };
    Ok(Node {
        tool,
        blend: n.blend.filter(|v| *v != 1.0),
        ..n.clone()
    })
}

fn blur_checked(b: &Blur) -> Result<Blur> {
    for v in [b.size, b.y_size] {
        within(v, Blur::SIZES, "a blur's size is from 0 to 1000000")?;
    }
    if b.passes
        .is_some_and(|n| !(1..=Blur::MOST_PASSES).contains(&n))
    {
        return Err(Error::invalid("a blur's passes are from 1 to 64"));
    }
    Ok(Blur {
        filter: b.filter.filter(|f| *f != BlurFilter::default()),
        size: b.size.filter(|v| *v != Blur::SIZE),
        y_size: b.y_size.filter(|v| *v != Blur::SIZE),
        passes: b.passes.filter(|v| *v != Blur::PASSES),
        clipping: b.clipping.filter(|c| *c != BlurClipping::default()),
        channels: b.channels.filter(|c| *c != Blur::ALL),
        ..b.clone()
    })
}

fn tracker_checked(t: &Tracker) -> Result<Tracker> {
    if t.patterns.len() > Tracker::PATTERNS_KEPT {
        return Err(Error::invalid(format!(
            "a tracker has {} patterns at most",
            Tracker::PATTERNS_KEPT
        )));
    }
    let patterns = t.patterns.iter().map(pattern_checked);
    Ok(Tracker {
        patterns: patterns.collect::<Result<Vec<_>>>()?,
    })
}

fn pattern_checked(p: &Pattern) -> Result<Pattern> {
    let name = p.name.trim().to_string();
    if name.is_empty() {
        return Err(Error::invalid("a pattern of a tracker has a name"));
    }
    for v in p.center.into_iter().flatten() {
        let why = "a pattern's center is two fractions, each from -10 to 10";
        within(Some(v), Pattern::PLACES, why)?;
    }
    for v in p.offset.into_iter().flatten() {
        let why = "a pattern's offset is two fractions, each from -10 to 10";
        within(Some(v), Pattern::PLACES, why)?;
    }
    let why = "a pattern's width and height are from 0.001 to 0.5";
    within(p.width, Pattern::SIDES, why)?;
    within(p.height, Pattern::SIDES, why)?;
    Ok(Pattern {
        name,
        center: p.center.filter(|c| *c != Pattern::CENTER),
        width: p.width.filter(|v| *v != Pattern::WIDTH),
        height: p.height.filter(|v| *v != Pattern::HEIGHT),
        offset: p.offset.filter(|o| *o != [0.0, 0.0]),
        ..p.clone()
    })
}

fn connect_checked(c: &Composition, n: &Node) -> Result<()> {
    let Some(k) = n.tool.mask().and_then(|m| m.connect.as_ref()) else {
        return Ok(());
    };
    if c.node(&k.tracker).and_then(|t| t.tool.tracker()).is_none() {
        return Err(Error::invalid(format!(
            "{} follows {}, but the composition has no tracker {}",
            n.name, k.tracker, k.tracker
        )));
    }
    if c.pattern(k).is_none() {
        return Err(Error::invalid(format!(
            "{} follows pattern {} of {}, but {} has no pattern {}",
            n.name, k.pattern, k.tracker, k.tracker, k.pattern
        )));
    }
    let own = n.tool.mask().and_then(|m| m.center);
    if own.is_some_and(|v| v != Mask::MIDDLE) || !n.keys_of(EffectValue::Center).is_empty() {
        return Err(Error::invalid(format!(
            "{} follows a tracker, so its own center is [0.5, 0.5], with no keyframes",
            n.name
        )));
    }
    Ok(())
}

fn mask_checked(m: &Mask, tool: &str) -> Result<Mask> {
    let polygon = tool == "polygon";
    within(m.level, Mask::LEVELS, "a mask's level is from 0 to 1")?;
    within(
        m.soft_edge,
        Mask::SOFT_EDGES,
        "a mask's soft edge is from 0 to 1",
    )?;
    if m.passes
        .is_some_and(|n| !(1..=Blur::MOST_PASSES).contains(&n))
    {
        return Err(Error::invalid("a mask's passes are from 1 to 64"));
    }
    within(
        m.border_width,
        Mask::BORDERS,
        "a mask's border width is from -1 to 1",
    )?;
    for v in m.center.into_iter().flatten() {
        within(
            Some(v),
            Mask::PLACES,
            "a mask's center is two fractions, each from -10 to 10",
        )?;
    }
    let shaped = [("width", m.width), ("height", m.height), ("angle", m.angle)];
    let tilted = [
        ("size", m.size),
        ("z rotation", m.z_rotation),
        ("x rotation", m.x_rotation),
        ("y rotation", m.y_rotation),
    ];
    let shaped: &[(&str, Option<f64>)] = &shaped;
    let tilted: &[(&str, Option<f64>)] = &tilted;
    let (own, other) = match polygon {
        true => (tilted, shaped),
        false => (shaped, tilted),
    };
    if let Some((what, _)) = other.iter().find(|(_, v)| v.is_some()) {
        return Err(Error::invalid(format!("a {tool} has no {what}")));
    }
    for &(what, v) in own {
        let (range, unit) = match what {
            "width" | "height" | "size" => (Mask::SIZES, ""),
            _ => (Mask::TURNS, " degrees"),
        };
        within(
            v,
            range,
            &format!("a mask's {what} is from {} to {}{unit}", range.0, range.1),
        )?;
    }
    if m.corner_radius.is_some() && tool != "rectangle" {
        return Err(Error::invalid("only a rectangle has a corner radius"));
    }
    within(
        m.corner_radius,
        Mask::CORNERS,
        "a corner radius is from 0 to 1",
    )?;
    if !polygon && !m.points.is_empty() {
        return Err(Error::invalid("only a polygon has points"));
    }
    if !polygon && m.fill_method.is_some() {
        return Err(Error::invalid("only a polygon has a fill method"));
    }
    let most = Mask::POINTS_KEPT;
    if m.points.len() > most {
        return Err(Error::invalid(format!(
            "a polygon has at most {most} points, not {}",
            m.points.len()
        )));
    }
    for v in m.points.iter().flatten() {
        within(
            Some(*v),
            Mask::PLACES,
            "a polygon's points are fractions, each from -10 to 10",
        )?;
    }
    Ok(Mask {
        level: m.level.filter(|v| *v != Mask::LEVEL),
        soft_edge: m.soft_edge.filter(|v| *v != 0.0),
        filter: m.filter.filter(|f| *f != BlurFilter::default()),
        passes: m.passes.filter(|v| *v != Blur::PASSES),
        border_width: m.border_width.filter(|v| *v != 0.0),
        center: m.center.filter(|c| *c != Mask::MIDDLE),
        width: m.width.filter(|v| *v != Mask::SIZE),
        height: m.height.filter(|v| *v != Mask::SIZE),
        angle: m.angle.filter(|v| *v != 0.0),
        size: m.size.filter(|v| *v != Mask::SCALE),
        z_rotation: m.z_rotation.filter(|v| *v != 0.0),
        x_rotation: m.x_rotation.filter(|v| *v != 0.0),
        y_rotation: m.y_rotation.filter(|v| *v != 0.0),
        corner_radius: m.corner_radius.filter(|v| *v != 0.0),
        fill_method: m.fill_method.filter(|f| *f != FillMethod::default()),
        ..m.clone()
    })
}

pub fn composition_checked(c: &Composition) -> Result<Composition> {
    for (i, n) in c.nodes.iter().enumerate() {
        if !named_well(&n.name) {
            return Err(Error::invalid(format!(
                "{:?} is no node name: letters, digits and _, with no digit first",
                n.name
            )));
        }
        if c.nodes[..i].iter().any(|m| m.name == n.name) {
            return Err(Error::invalid(format!("two nodes are named {}", n.name)));
        }
        let once = matches!(n.tool, Tool::MediaIn | Tool::MediaOut);
        if once && c.nodes[..i].iter().any(|m| m.tool == n.tool) {
            return Err(Error::invalid(format!(
                "a composition has one {} node",
                n.tool.label()
            )));
        }
        let joined = |join: &Option<String>, fits: fn(&Tool) -> bool, what: &str| match join {
            None => Ok(()),
            Some(from) => match c.node(from) {
                None => Err(Error::invalid(format!(
                    "{}'s {what} is {from}, which is no node",
                    n.name
                ))),
                Some(m) if !fits(&m.tool) => Err(Error::invalid(format!(
                    "{}'s {what} is {from}, a {}, which doesn't fit there",
                    n.name,
                    m.tool.label()
                ))),
                Some(_) => Ok(()),
            },
        };
        if n.input.is_some() && !n.tool.takes_input() {
            return Err(Error::invalid(format!("{} has no picture input", n.name)));
        }
        if n.effect_mask.is_some() && !n.tool.takes_mask() {
            return Err(Error::invalid(format!(
                "{} has no effect mask input",
                n.name
            )));
        }
        joined(&n.input, |t| t.flows() || *t == Tool::MediaIn, "input")?;
        joined(&n.effect_mask, Tool::is_mask, "effect mask")?;
        connect_checked(c, n)?;
    }
    if let Some(at) = c.loops() {
        return Err(Error::invalid(format!(
            "the composition's joins make a loop through {at}"
        )));
    }
    Ok(Composition {
        nodes: c
            .nodes
            .iter()
            .map(node_checked)
            .collect::<Result<Vec<_>>>()?,
        loop_play: c.loop_play,
        ping_pong: c.ping_pong,
        start: c.start,
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
                effects: old.effects.clone(),
                composition: old.composition.clone(),
                ..Picture::default()
            },
        };
        let trackers = old
            .composition
            .iter()
            .filter(|_| kept.composition.is_none());
        let nodes = trackers
            .flat_map(|k| &k.nodes)
            .filter_map(|n| n.tool.tracker());
        let gone: Vec<String> = nodes
            .flat_map(|t| t.patterns.iter().filter_map(|x| x.path.clone()))
            .collect();
        c.picture = (!kept.is_usual()).then_some(kept);
        super::motion::unused(p, id, &gone);
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
            composition: old.composition,
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
        PictureTool::Composition => Picture {
            composition: None,
            ..old
        },
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
    out
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
    if c.generator.is_some() && picture.composition.is_some() {
        return Some(format!("{} is a generator, which has no composition", c.id));
    }
    checked(picture)
        .err()
        .map(|e| format!("{}: {}", c.id, e.message))
}
