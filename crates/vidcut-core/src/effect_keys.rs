use std::collections::HashSet;

use crate::keyframes::{
    drop_channel, ease_mark, hold_channel, put_pair, retarget_channel, retime_channel,
    scale_channel, split_channel, value_at, value_paired,
};
use crate::model::{
    Clip, Ease, Effect, EffectArea, EffectKeys, EffectKind, EffectValue, FilmGrain, GaussianBlur,
    Glow, Keyframe, MosaicBlur, Sharpen, Vignette,
};

type Range = (f64, f64);
type Slot<'a> = Option<(&'a mut f64, Range)>;
type Point<'a> = Option<(&'a mut [f64; 2], Range)>;

const BLEND: Range = (0.0, 1.0);
const FINITE: Range = (f64::MIN, f64::MAX);

fn gaussian_slot(b: &mut GaussianBlur, value: EffectValue) -> Slot<'_> {
    use EffectValue as V;
    let (channels, i) = match value {
        V::Strength => {
            return Some((
                b.strength.get_or_insert(GaussianBlur::STRENGTH),
                GaussianBlur::STRENGTHS,
            ));
        }
        V::VerticalStrength => {
            return Some((
                b.vertical_strength.get_or_insert(GaussianBlur::STRENGTH),
                GaussianBlur::STRENGTHS,
            ));
        }
        V::HorizontalRed => (&mut b.horizontal_channels, 0),
        V::HorizontalGreen => (&mut b.horizontal_channels, 1),
        V::HorizontalBlue => (&mut b.horizontal_channels, 2),
        V::HorizontalAlpha => (&mut b.horizontal_channels, 3),
        V::VerticalRed => (&mut b.vertical_channels, 0),
        V::VerticalGreen => (&mut b.vertical_channels, 1),
        V::VerticalBlue => (&mut b.vertical_channels, 2),
        V::VerticalAlpha => (&mut b.vertical_channels, 3),
        _ => return None,
    };
    Some((
        &mut channels.get_or_insert(GaussianBlur::SAME)[i],
        GaussianBlur::CHANNELS,
    ))
}

fn mosaic_slot(m: &mut MosaicBlur, value: EffectValue) -> Slot<'_> {
    use EffectValue as V;
    Some(match value {
        V::PixelFrequency => (
            m.pixel_frequency.get_or_insert(MosaicBlur::FREQUENCY),
            MosaicBlur::FREQUENCIES,
        ),
        V::SmoothStrength => (m.smooth_strength.get_or_insert(0.0), MosaicBlur::SMOOTHS),
        V::Aliasing => (
            m.aliasing.get_or_insert(MosaicBlur::ALIASING),
            MosaicBlur::ALIASINGS,
        ),
        V::XOffset => (m.x_offset.get_or_insert(0.0), MosaicBlur::OFFSETS),
        V::YOffset => (m.y_offset.get_or_insert(0.0), MosaicBlur::OFFSETS),
        V::Aspect => (
            m.aspect.get_or_insert(MosaicBlur::STRETCH),
            MosaicBlur::STRETCHES,
        ),
        _ => return None,
    })
}

fn glow_color(value: EffectValue) -> Option<usize> {
    use EffectValue as V;
    match value {
        V::RelativeSpreadRed | V::ColorFilterRed | V::SecondaryColorRed => Some(0),
        V::RelativeSpreadGreen | V::ColorFilterGreen | V::SecondaryColorGreen => Some(1),
        V::RelativeSpreadBlue | V::ColorFilterBlue | V::SecondaryColorBlue => Some(2),
        _ => None,
    }
}

fn glow_color_slot(g: &mut Glow, value: EffectValue) -> Slot<'_> {
    use EffectValue as V;
    let i = glow_color(value)?;
    let (color, range) = match value {
        V::RelativeSpreadRed | V::RelativeSpreadGreen | V::RelativeSpreadBlue => {
            (&mut g.relative_spread, Glow::RELATIVES)
        }
        V::ColorFilterRed | V::ColorFilterGreen | V::ColorFilterBlue => {
            (&mut g.color_filter, Glow::COLORS)
        }
        _ => (&mut g.secondary_color, Glow::COLORS),
    };
    Some((&mut color.get_or_insert(Glow::WHITE)[i], range))
}

fn glow_slot(g: &mut Glow, value: EffectValue) -> Slot<'_> {
    use EffectValue as V;
    if glow_color(value).is_some() {
        return glow_color_slot(g, value);
    }
    Some(match value {
        V::Threshold => (g.threshold.get_or_insert(Glow::THRESHOLD), Glow::UNIT),
        V::AlphaThreshold => (
            g.alpha_threshold.get_or_insert(Glow::ALPHA_THRESHOLD),
            Glow::UNIT,
        ),
        V::Spread => (g.spread.get_or_insert(Glow::SPREAD), Glow::SPREADS),
        V::HvRatio => (g.hv_ratio.get_or_insert(0.0), Glow::RATIOS),
        V::Gain => (g.gain.get_or_insert(Glow::GAIN), Glow::GAINS),
        V::Gamma => (g.gamma.get_or_insert(1.0), Glow::GAMMAS),
        V::Saturation => (g.saturation.get_or_insert(1.0), Glow::SATURATIONS),
        V::SecondaryGain => (
            g.secondary_gain.get_or_insert(Glow::SECONDARY_GAIN),
            Glow::GAINS,
        ),
        V::SecondarySpread => (
            g.secondary_spread.get_or_insert(Glow::SECONDARY_SPREAD),
            Glow::SPREADS,
        ),
        V::AtmosphereStrength => (
            g.atmosphere_strength.get_or_insert(Glow::STRENGTH),
            Glow::STRENGTHS,
        ),
        V::AtmosphereDetail => (g.atmosphere_detail.get_or_insert(Glow::DETAIL), Glow::UNIT),
        V::AtmosphereScale => (g.atmosphere_scale.get_or_insert(Glow::SCALE), Glow::SCALES),
        V::AtmosphereRatio => (g.atmosphere_ratio.get_or_insert(0.0), Glow::NOISE_RATIOS),
        V::AtmosphereEvolution => (g.atmosphere_evolution.get_or_insert(0.0), Glow::EVOLUTIONS),
        V::AtmosphereSeethe => (
            g.atmosphere_seethe.get_or_insert(Glow::SEETHE),
            Glow::SEETHES,
        ),
        V::Opacity => (g.opacity.get_or_insert(1.0), Glow::UNIT),
        V::Recovery => (g.recovery.get_or_insert(0.0), Glow::RECOVERIES),
        _ => return None,
    })
}

fn grain_slot(g: &mut FilmGrain, value: EffectValue) -> Slot<'_> {
    use EffectValue as V;
    Some(match value {
        V::Opacity => (g.opacity.get_or_insert(FilmGrain::OPACITY), FilmGrain::UNIT),
        V::Texture => (g.texture.get_or_insert(FilmGrain::TEXTURE), FilmGrain::UNIT),
        V::GrainSize => (g.grain_size.get_or_insert(FilmGrain::SIZE), FilmGrain::UNIT),
        V::GrainAspect => (g.grain_aspect.get_or_insert(1.0), FilmGrain::ASPECTS),
        V::GrainStrength => (
            g.grain_strength.get_or_insert(FilmGrain::STRENGTH),
            FilmGrain::UNIT,
        ),
        V::Offset => (g.offset.get_or_insert(FilmGrain::OFFSET), FilmGrain::UNIT),
        V::Symmetry => (
            g.symmetry.get_or_insert(FilmGrain::SYMMETRY),
            FilmGrain::UNIT,
        ),
        V::Softness => (
            g.softness.get_or_insert(FilmGrain::SOFTNESS),
            FilmGrain::UNIT,
        ),
        V::Saturation => (g.saturation.get_or_insert(0.0), FilmGrain::UNIT),
        V::DetailLoss => (g.detail_loss.get_or_insert(0.0), FilmGrain::DETAILS),
        V::ChannelGainRed => (
            &mut g.channel_gains.get_or_insert(FilmGrain::WHITE)[0],
            FilmGrain::GAINS,
        ),
        V::ChannelGainGreen => (
            &mut g.channel_gains.get_or_insert(FilmGrain::WHITE)[1],
            FilmGrain::GAINS,
        ),
        V::ChannelGainBlue => (
            &mut g.channel_gains.get_or_insert(FilmGrain::WHITE)[2],
            FilmGrain::GAINS,
        ),
        V::Shadows => (g.shadows.get_or_insert(FilmGrain::TONE), FilmGrain::UNIT),
        V::Midtones => (g.midtones.get_or_insert(FilmGrain::TONE), FilmGrain::UNIT),
        V::Highlights => (g.highlights.get_or_insert(FilmGrain::TONE), FilmGrain::UNIT),
        _ => return None,
    })
}

fn vignette_slot(v: &mut Vignette, value: EffectValue) -> Slot<'_> {
    use EffectValue as V;
    Some(match value {
        V::Size => (v.size.get_or_insert(Vignette::SIZE), Vignette::SIZES),
        V::Rotation => (v.rotation.get_or_insert(0.0), Vignette::TURNS),
        V::Anamorphism => (
            v.anamorphism.get_or_insert(Vignette::STRETCH),
            Vignette::STRETCHES,
        ),
        V::Softness => (
            v.softness.get_or_insert(Vignette::SOFTNESS),
            Vignette::SHARES,
        ),
        V::Transparency => (v.transparency.get_or_insert(0.0), Vignette::SHARES),
        V::ColorRed => (&mut v.color.get_or_insert(Vignette::BLACK)[0], FINITE),
        V::ColorGreen => (&mut v.color.get_or_insert(Vignette::BLACK)[1], FINITE),
        V::ColorBlue => (&mut v.color.get_or_insert(Vignette::BLACK)[2], FINITE),
        _ => return None,
    })
}

fn sharpen_slot(s: &mut Sharpen, value: EffectValue) -> Slot<'_> {
    use EffectValue as V;
    Some(match value {
        V::SharpenAmount => (
            s.sharpen_amount.get_or_insert(Sharpen::AMOUNT),
            Sharpen::GAINS,
        ),
        V::FineDetailSize => (
            s.fine_detail_size.get_or_insert(Sharpen::SIZE),
            Sharpen::SIZES,
        ),
        V::FineDetails => (s.fine_details.get_or_insert(Sharpen::GAIN), Sharpen::GAINS),
        V::MediumDetails => (
            s.medium_details.get_or_insert(Sharpen::GAIN),
            Sharpen::GAINS,
        ),
        V::LargeDetails => (s.large_details.get_or_insert(Sharpen::GAIN), Sharpen::GAINS),
        V::SharpenChroma => (
            s.sharpen_chroma.get_or_insert(Sharpen::GAIN),
            Sharpen::GAINS,
        ),
        _ => return None,
    })
}

fn slot(kind: &mut EffectKind, value: EffectValue) -> Slot<'_> {
    match kind {
        EffectKind::GaussianBlur(b) => gaussian_slot(b, value),
        EffectKind::MosaicBlur(m) => mosaic_slot(m, value),
        EffectKind::Glow(g) => glow_slot(g, value),
        EffectKind::FilmGrain(g) => grain_slot(g, value),
        EffectKind::Vignette(v) => vignette_slot(v, value),
        EffectKind::Sharpen(s) => sharpen_slot(s, value),
    }
}

fn point_slot(kind: &mut EffectKind, value: EffectValue) -> Point<'_> {
    match (kind, value) {
        (EffectKind::MosaicBlur(m), EffectValue::Position) => Some((
            m.position.get_or_insert(MosaicBlur::MIDDLE),
            MosaicBlur::PLACES,
        )),
        (EffectKind::Vignette(v), EffectValue::Center) => {
            Some((v.center.get_or_insert(Vignette::MIDDLE), Vignette::PLACES))
        }
        _ => None,
    }
}

fn area_slot(a: &mut EffectArea, value: EffectValue) -> Slot<'_> {
    use EffectValue as V;
    Some(match value {
        V::AreaCenterX => (
            &mut a.center.get_or_insert(EffectArea::MIDDLE)[0],
            EffectArea::PLACES,
        ),
        V::AreaCenterY => (
            &mut a.center.get_or_insert(EffectArea::MIDDLE)[1],
            EffectArea::PLACES,
        ),
        V::AreaWidth => (
            &mut a.size.get_or_insert(EffectArea::WHOLE)[0],
            EffectArea::SIZES,
        ),
        V::AreaHeight => (
            &mut a.size.get_or_insert(EffectArea::WHOLE)[1],
            EffectArea::SIZES,
        ),
        V::AreaRotation => (a.rotation.get_or_insert(0.0), EffectArea::TURNS),
        V::AreaSoftness => (a.softness.get_or_insert(0.0), EffectArea::SHARES),
        _ => return None,
    })
}

fn area_read(a: &EffectArea, value: EffectValue) -> Option<(f64, Range)> {
    let mut numbers = EffectArea {
        center: a.center,
        size: a.size,
        rotation: a.rotation,
        softness: a.softness,
        ..EffectArea::default()
    };
    area_slot(&mut numbers, value).map(|(n, range)| (*n, range))
}

impl EffectValue {
    pub const AREA: [EffectValue; 6] = [
        EffectValue::AreaCenterX,
        EffectValue::AreaCenterY,
        EffectValue::AreaWidth,
        EffectValue::AreaHeight,
        EffectValue::AreaRotation,
        EffectValue::AreaSoftness,
    ];

    pub const ALL: [EffectValue; 78] = [
        EffectValue::Blend,
        EffectValue::Strength,
        EffectValue::VerticalStrength,
        EffectValue::HorizontalRed,
        EffectValue::HorizontalGreen,
        EffectValue::HorizontalBlue,
        EffectValue::HorizontalAlpha,
        EffectValue::VerticalRed,
        EffectValue::VerticalGreen,
        EffectValue::VerticalBlue,
        EffectValue::VerticalAlpha,
        EffectValue::PixelFrequency,
        EffectValue::SmoothStrength,
        EffectValue::Aliasing,
        EffectValue::XOffset,
        EffectValue::YOffset,
        EffectValue::Aspect,
        EffectValue::Position,
        EffectValue::Threshold,
        EffectValue::AlphaThreshold,
        EffectValue::Spread,
        EffectValue::HvRatio,
        EffectValue::RelativeSpreadRed,
        EffectValue::RelativeSpreadGreen,
        EffectValue::RelativeSpreadBlue,
        EffectValue::Gain,
        EffectValue::Gamma,
        EffectValue::ColorFilterRed,
        EffectValue::ColorFilterGreen,
        EffectValue::ColorFilterBlue,
        EffectValue::SecondaryGain,
        EffectValue::SecondarySpread,
        EffectValue::SecondaryColorRed,
        EffectValue::SecondaryColorGreen,
        EffectValue::SecondaryColorBlue,
        EffectValue::AtmosphereStrength,
        EffectValue::AtmosphereDetail,
        EffectValue::AtmosphereScale,
        EffectValue::AtmosphereRatio,
        EffectValue::AtmosphereEvolution,
        EffectValue::AtmosphereSeethe,
        EffectValue::Recovery,
        EffectValue::Opacity,
        EffectValue::Texture,
        EffectValue::GrainSize,
        EffectValue::GrainAspect,
        EffectValue::GrainStrength,
        EffectValue::Offset,
        EffectValue::Symmetry,
        EffectValue::Softness,
        EffectValue::Saturation,
        EffectValue::DetailLoss,
        EffectValue::ChannelGainRed,
        EffectValue::ChannelGainGreen,
        EffectValue::ChannelGainBlue,
        EffectValue::Shadows,
        EffectValue::Midtones,
        EffectValue::Highlights,
        EffectValue::Size,
        EffectValue::Rotation,
        EffectValue::Anamorphism,
        EffectValue::Center,
        EffectValue::Transparency,
        EffectValue::ColorRed,
        EffectValue::ColorGreen,
        EffectValue::ColorBlue,
        EffectValue::SharpenAmount,
        EffectValue::FineDetailSize,
        EffectValue::FineDetails,
        EffectValue::MediumDetails,
        EffectValue::LargeDetails,
        EffectValue::SharpenChroma,
        EffectValue::AreaCenterX,
        EffectValue::AreaCenterY,
        EffectValue::AreaWidth,
        EffectValue::AreaHeight,
        EffectValue::AreaRotation,
        EffectValue::AreaSoftness,
    ];

    pub fn name(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    pub fn of_area(self) -> bool {
        Self::AREA.contains(&self)
    }

    pub fn pair(self) -> bool {
        matches!(self, EffectValue::Position | EffectValue::Center)
    }
}

impl EffectKeys {
    fn channel(&mut self) -> (&mut Vec<Keyframe>, Option<&mut Vec<f64>>) {
        let pair = self.value.pair();
        (&mut self.keys, pair.then_some(&mut self.y))
    }
}

impl Effect {
    fn read_point(&self, value: EffectValue) -> Option<([f64; 2], Range)> {
        if value == EffectValue::Blend {
            return Some(([self.mixed(), 0.0], BLEND));
        }
        if value.of_area() {
            return area_read(self.area.as_ref()?, value).map(|(n, range)| ([n, 0.0], range));
        }
        let mut kind = self.kind.clone();
        if value.pair() {
            return point_slot(&mut kind, value).map(|(p, range)| (*p, range));
        }
        slot(&mut kind, value).map(|(n, range)| ([*n, 0.0], range))
    }

    fn read(&self, value: EffectValue) -> Option<(f64, Range)> {
        self.read_point(value).map(|([x, _], range)| (x, range))
    }

    pub fn values(&self) -> Vec<EffectValue> {
        let mut kind = self.kind.clone();
        EffectValue::ALL
            .into_iter()
            .filter(|v| match *v {
                EffectValue::Blend => true,
                v if v.of_area() => self.area.is_some(),
                v if v.pair() => point_slot(&mut kind, v).is_some(),
                v => slot(&mut kind, v).is_some(),
            })
            .collect()
    }

    pub fn takes(&self, value: EffectValue) -> bool {
        self.range_of(value).is_some()
    }

    pub fn range_of(&self, value: EffectValue) -> Option<(f64, f64)> {
        self.read(value).map(|(_, range)| range)
    }

    pub fn has_keys(&self) -> bool {
        self.keys.iter().any(|k| !k.keys.is_empty())
    }

    pub fn keys_of(&self, value: EffectValue) -> &[Keyframe] {
        self.keys
            .iter()
            .find(|k| k.value == value)
            .map_or(&[], |k| k.keys.as_slice())
    }

    pub fn y_of(&self, value: EffectValue) -> &[f64] {
        self.keys
            .iter()
            .find(|k| k.value == value)
            .map_or(&[], |k| k.y.as_slice())
    }

    pub fn channel_mut(
        &mut self,
        value: EffectValue,
    ) -> Option<(&mut Vec<Keyframe>, Option<&mut Vec<f64>>)> {
        if !self.takes(value) {
            return None;
        }
        let i = match self.keys.binary_search_by_key(&value, |k| k.value) {
            Ok(i) => i,
            Err(i) => {
                self.keys.insert(
                    i,
                    EffectKeys {
                        value,
                        keys: Vec::new(),
                        y: Vec::new(),
                    },
                );
                i
            }
        };
        Some(self.keys[i].channel())
    }

    pub fn keys_mut(&mut self, value: EffectValue) -> Option<&mut Vec<Keyframe>> {
        if value.pair() {
            return None;
        }
        self.channel_mut(value).map(|(keys, _)| keys)
    }

    pub fn plain(&self, value: EffectValue) -> f64 {
        self.read(value).map_or(0.0, |(n, _)| n)
    }

    pub fn point(&self, value: EffectValue) -> [f64; 2] {
        self.read_point(value).map_or([0.0; 2], |(p, _)| p)
    }

    pub fn write(&mut self, value: EffectValue, to: f64) {
        let place = match value {
            EffectValue::Blend => Some((self.blend.get_or_insert(to), BLEND)),
            v if v.of_area() => self.area.as_mut().and_then(|a| area_slot(a, v)),
            v if v.pair() => point_slot(&mut self.kind, v).map(|(p, range)| (&mut p[0], range)),
            v => slot(&mut self.kind, v),
        };
        if let Some((n, _)) = place {
            *n = to;
        }
    }

    pub fn write_point(&mut self, value: EffectValue, to: [f64; 2]) {
        match point_slot(&mut self.kind, value) {
            Some((p, _)) => *p = to,
            None => self.write(value, to[0]),
        }
    }

    pub fn area_keys(&self) -> impl Iterator<Item = &EffectKeys> {
        self.keys.iter().filter(|k| k.value.of_area())
    }

    pub fn value_at(&self, value: EffectValue, frame: f64) -> f64 {
        self.point_at(value, frame)[0]
    }

    pub fn point_at(&self, value: EffectValue, frame: f64) -> [f64; 2] {
        let (low, high) = self.range_of(value).unwrap_or(FINITE);
        let keys = self.keys_of(value);
        let x = value_at(keys, frame);
        let y = match value.pair() {
            true => {
                let ys = self.y_of(value);
                if ys.len() != keys.len() {
                    return self.point(value);
                }
                value_paired(keys, frame, |i| ys[i])
            }
            false => Some(0.0),
        };
        match (x, y) {
            (Some(x), Some(y)) if value.pair() => [x.clamp(low, high), y.clamp(low, high)],
            (Some(x), _) => [x.clamp(low, high), 0.0],
            _ => self.point(value),
        }
    }

    pub fn put(&mut self, value: EffectValue, frame: i64, to: f64) {
        if let Some(keys) = self.keys_mut(value) {
            crate::ops::keyframes::put_value(keys, frame, to);
        }
    }

    pub fn put_point(&mut self, value: EffectValue, frame: i64, to: [f64; 2]) {
        match self.channel_mut(value) {
            Some((keys, Some(y))) => put_pair(keys, y, frame, to),
            Some((keys, None)) => crate::ops::keyframes::put_value(keys, frame, to[0]),
            None => {}
        }
    }

    pub fn adopted(&self, frame: f64) -> Effect {
        let mut e = self.clone();
        for k in &self.keys {
            e.write_point(k.value, self.point_at(k.value, frame));
        }
        e
    }

    pub fn sampled(&self, frame: f64) -> Effect {
        let mut e = self.adopted(frame);
        e.keys.clear();
        e
    }

    pub fn previewed(&self, baseline: &Effect, edited: &Effect, frame: i64) -> Effect {
        let mut out = edited.clone();
        for k in &self.keys {
            let to = edited.point(k.value);
            out.write_point(k.value, self.point(k.value));
            if to != baseline.point(k.value) {
                out.put_point(k.value, frame, to);
            }
        }
        out
    }

    pub fn has_key_at(&self, frame: i64, area: bool) -> bool {
        self.keys
            .iter()
            .filter(|k| k.value.of_area() == area)
            .any(|k| k.keys.iter().any(|key| key.frame == frame))
    }

    pub fn toggle_keys(&mut self, frame: i64, area: bool) -> bool {
        if self.has_key_at(frame, area) {
            let at = HashSet::from([frame]);
            for k in self.keys.iter_mut().filter(|k| k.value.of_area() == area) {
                let (keys, y) = k.channel();
                drop_channel(keys, y, &at);
            }
            self.keys.retain(|k| !k.keys.is_empty());
            return false;
        }
        for value in self.values().into_iter().filter(|v| v.of_area() == area) {
            let to = self.point_at(value, frame as f64);
            self.put_point(value, frame, to);
        }
        true
    }

    pub fn ease_keys(&mut self, frame: i64, ease: Ease, area: bool) {
        for k in self.keys.iter_mut().filter(|k| k.value.of_area() == area) {
            ease_mark(&mut k.keys, frame, ease);
        }
    }

    pub fn split_keys(&mut self, frame: i64) {
        for k in &mut self.keys {
            let (keys, y) = k.channel();
            split_channel(keys, y, frame);
        }
    }

    pub fn hold_keys(&mut self, at: i64, until: i64) {
        for k in &mut self.keys {
            let (keys, y) = k.channel();
            hold_channel(keys, y, at, until);
        }
    }

    pub fn scale_key_times(&mut self, conv: &impl Fn(i64) -> i64, ratio: &impl Fn(i64) -> f64) {
        for k in &mut self.keys {
            let (keys, y) = k.channel();
            scale_channel(keys, y, conv, ratio);
        }
    }

    pub fn retime_keys(&mut self, old: &Clip, new: &Clip) {
        for k in &mut self.keys {
            let (keys, y) = k.channel();
            retime_channel(keys, y, old, new);
        }
    }

    pub fn retarget_keys(&mut self, map: &impl Fn(i64) -> i64) {
        for k in &mut self.keys {
            let (keys, y) = k.channel();
            retarget_channel(keys, y, map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EffectType;

    fn key(frame: i64, value: f64) -> Keyframe {
        Keyframe {
            frame,
            value,
            in_handle: None,
            out_handle: None,
        }
    }

    fn blur_with_keys() -> Effect {
        let mut e = EffectType::GaussianBlur.made();
        e.keys = vec![EffectKeys {
            value: EffectValue::Strength,
            keys: vec![key(0, 0.2), key(10, 0.6)],
            y: Vec::new(),
        }];
        e
    }

    fn strength(e: &Effect) -> Option<f64> {
        match &e.kind {
            EffectKind::GaussianBlur(b) => b.strength,
            _ => None,
        }
    }

    #[test]
    fn a_keyed_value_is_linear_between_two_keys() {
        assert_eq!(blur_with_keys().value_at(EffectValue::Strength, 5.0), 0.4);
    }

    #[test]
    fn a_curve_past_the_range_stops_at_its_end() {
        let mut e = EffectType::GaussianBlur.made();
        let mut first = key(0, 0.9);
        first.out_handle = Some(crate::model::Handle {
            frames: 10.0,
            value: 50.0,
        });
        e.keys = vec![EffectKeys {
            value: EffectValue::Strength,
            keys: vec![first, key(30, 0.9)],
            y: Vec::new(),
        }];
        assert_eq!(e.value_at(EffectValue::Strength, 15.0), 1.0);
    }

    #[test]
    fn a_value_with_no_keys_stays_plain() {
        assert_eq!(blur_with_keys().value_at(EffectValue::Blend, 5.0), 1.0);
    }

    #[test]
    fn sampled_writes_the_value_and_drops_the_keys() {
        let e = blur_with_keys().sampled(10.0);
        assert_eq!((strength(&e), e.keys.len()), (Some(0.6), 0));
    }

    #[test]
    fn adopted_writes_the_value_and_keeps_the_keys() {
        let e = blur_with_keys().adopted(0.0);
        assert_eq!((strength(&e), e.keys.len()), (Some(0.2), 1));
    }

    #[test]
    fn a_value_of_another_kind_has_no_place() {
        assert!(!blur_with_keys().takes(EffectValue::Gamma));
    }

    #[test]
    fn a_vignette_has_nine_values_and_the_blend() {
        assert_eq!(EffectType::Vignette.made().values().len(), 10);
    }

    #[test]
    fn a_channel_keeps_the_usual_of_the_others() {
        let mut e = EffectType::GaussianBlur.made();
        e.write(EffectValue::HorizontalGreen, 1.5);
        assert_eq!(e.plain(EffectValue::HorizontalRed), 1.0);
    }

    #[test]
    fn toggle_on_a_key_removes_the_keys_of_that_frame() {
        let mut e = blur_with_keys();
        e.toggle_keys(10, false);
        assert_eq!(e.keys_of(EffectValue::Strength), [key(0, 0.2)]);
    }

    #[test]
    fn toggle_off_a_key_keys_every_value_at_its_value() {
        let mut e = blur_with_keys();
        e.toggle_keys(5, false);
        assert_eq!(e.keys.iter().filter(|k| !k.keys.is_empty()).count(), 11);
    }

    #[test]
    fn a_change_at_a_keyed_value_lands_on_the_frame() {
        let stored = blur_with_keys();
        let baseline = stored.adopted(5.0);
        let mut edited = baseline.clone();
        edited.write(EffectValue::Strength, 0.9);
        let out = stored.previewed(&baseline, &edited, 5);
        assert_eq!(out.keys_of(EffectValue::Strength)[1], key(5, 0.9));
    }
}
