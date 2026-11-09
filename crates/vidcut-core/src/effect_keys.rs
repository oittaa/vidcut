use crate::keyframes::{ease_mark, reshape, retimed, scaled_by, value_at};
use crate::model::{
    Clip, Ease, Effect, EffectKeys, EffectKind, EffectValue, FilmGrain, GaussianBlur, Glow,
    Keyframe, MosaicBlur, Sharpen, Vignette,
};

type Range = (f64, f64);
type Slot<'a> = Option<(&'a mut f64, Range)>;

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
        V::PositionX => (
            &mut m.position.get_or_insert(MosaicBlur::MIDDLE)[0],
            MosaicBlur::PLACES,
        ),
        V::PositionY => (
            &mut m.position.get_or_insert(MosaicBlur::MIDDLE)[1],
            MosaicBlur::PLACES,
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
        V::CenterX => (
            &mut v.center.get_or_insert(Vignette::MIDDLE)[0],
            Vignette::PLACES,
        ),
        V::CenterY => (
            &mut v.center.get_or_insert(Vignette::MIDDLE)[1],
            Vignette::PLACES,
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

impl EffectValue {
    pub const ALL: [EffectValue; 74] = [
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
        EffectValue::PositionX,
        EffectValue::PositionY,
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
        EffectValue::CenterX,
        EffectValue::CenterY,
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
    ];

    pub fn name(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

impl Effect {
    pub fn values(&self) -> Vec<EffectValue> {
        let mut kind = self.kind.clone();
        EffectValue::ALL
            .into_iter()
            .filter(|v| *v == EffectValue::Blend || slot(&mut kind, *v).is_some())
            .collect()
    }

    pub fn takes(&self, value: EffectValue) -> bool {
        self.range_of(value).is_some()
    }

    pub fn range_of(&self, value: EffectValue) -> Option<(f64, f64)> {
        if value == EffectValue::Blend {
            return Some(BLEND);
        }
        slot(&mut self.kind.clone(), value).map(|(_, range)| range)
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

    pub fn keys_mut(&mut self, value: EffectValue) -> Option<&mut Vec<Keyframe>> {
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
                    },
                );
                i
            }
        };
        Some(&mut self.keys[i].keys)
    }

    pub fn plain(&self, value: EffectValue) -> f64 {
        if value == EffectValue::Blend {
            return self.mixed();
        }
        slot(&mut self.kind.clone(), value).map_or(0.0, |(n, _)| *n)
    }

    pub fn write(&mut self, value: EffectValue, to: f64) {
        if value == EffectValue::Blend {
            self.blend = Some(to);
        } else if let Some((n, _)) = slot(&mut self.kind, value) {
            *n = to;
        }
    }

    pub fn value_at(&self, value: EffectValue, frame: f64) -> f64 {
        let (low, high) = self.range_of(value).unwrap_or(FINITE);
        value_at(self.keys_of(value), frame)
            .map_or_else(|| self.plain(value), |n| n.clamp(low, high))
    }

    pub fn put(&mut self, value: EffectValue, frame: i64, to: f64) {
        if let Some(keys) = self.keys_mut(value) {
            crate::ops::keyframes::put_value(keys, frame, to);
        }
    }

    pub fn adopted(&self, frame: f64) -> Effect {
        let mut e = self.clone();
        for k in &self.keys {
            e.write(k.value, self.value_at(k.value, frame));
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
            let to = edited.plain(k.value);
            out.write(k.value, self.plain(k.value));
            if to != baseline.plain(k.value) {
                out.put(k.value, frame, to);
            }
        }
        out
    }

    pub fn has_key_at(&self, frame: i64) -> bool {
        self.keys
            .iter()
            .any(|k| k.keys.iter().any(|key| key.frame == frame))
    }

    pub fn toggle_keys(&mut self, frame: i64) -> bool {
        if self.has_key_at(frame) {
            for k in &mut self.keys {
                reshape(&mut k.keys, |marked| {
                    marked.retain(|(key, _)| key.frame != frame);
                });
            }
            self.keys.retain(|k| !k.keys.is_empty());
            return false;
        }
        for value in self.values() {
            let to = self.value_at(value, frame as f64);
            self.put(value, frame, to);
        }
        true
    }

    pub fn ease_keys(&mut self, frame: i64, ease: Ease) {
        for k in &mut self.keys {
            ease_mark(&mut k.keys, frame, ease);
        }
    }

    pub fn scale_key_times(&mut self, conv: &impl Fn(i64) -> i64, ratio: &impl Fn(i64) -> f64) {
        for k in &mut self.keys {
            k.keys = scaled_by(&k.keys, conv, ratio);
        }
    }

    pub fn retime_keys(&mut self, old: &Clip, new: &Clip) {
        for k in &mut self.keys {
            k.keys = retimed(&k.keys, old, new);
        }
    }

    pub fn retarget_keys(&mut self, map: &impl Fn(i64) -> i64) {
        for k in &mut self.keys {
            for key in &mut k.keys {
                key.frame = map(key.frame);
            }
            k.keys.sort_by_key(|key| key.frame);
            k.keys.dedup_by_key(|key| key.frame);
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
    fn a_vignette_has_ten_values_and_the_blend() {
        assert_eq!(EffectType::Vignette.made().values().len(), 11);
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
        e.toggle_keys(10);
        assert_eq!(e.keys_of(EffectValue::Strength), [key(0, 0.2)]);
    }

    #[test]
    fn toggle_off_a_key_keys_every_value_at_its_value() {
        let mut e = blur_with_keys();
        e.toggle_keys(5);
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
