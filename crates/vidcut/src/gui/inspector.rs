use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Align, Layout, RichText, vec2};
use egui_phosphor::regular::{
    ARROW_COUNTER_CLOCKWISE, ARROW_DOWN, ARROW_UP, CARET_LEFT, CARET_RIGHT, CHECK, DIAMOND,
    FLIP_HORIZONTAL, FLIP_VERTICAL, LINK_SIMPLE, LINK_SIMPLE_BREAK, TRASH,
};
use serde_json::{Value, json};
use vidcut_core::generators::{Axis, CENTER};
use vidcut_core::keyframes::PictureValue;
use vidcut_core::model::{
    AreaShape, Border, CellShape, Clip, ClipRetime, CompositeMode, DynamicZoom, Ease, EbuLevel,
    Effect, EffectArea, EffectKind, EffectValue, FilmGrain, GaussianBlur, Generator, Glow,
    GlowAlpha, GlowFraming, GlowOutput, Gradient, GrainComposite, GrainPreset, Keyframe,
    MosaicBlur, MotionPath, Picture, PictureTool, Project, Retime, Sharpen, Speed, Subject,
    TrackKind, Upscaler, Vignette, VignetteMode, VignetteShape, ZoomBox,
};
use vidcut_core::motion::{REACH, TURN};
use vidcut_core::ops::picture::{cleared, effect_checked, has_picture};
use vidcut_core::place::fitted;
use vidcut_core::time::TimeSpec;
use vidcut_core::validate::{GAIN, PAN};

use super::dialog;
use super::export::Picked;
use super::isolation::{Isolate, heard};
use super::placing::{changed, ease_label, mode_label, target};
use super::theme;
use super::timeline::SpeedDialog;
use super::timeline::tools::landing;
use super::viewer::timecode_input;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

const SAME: f64 = 1e-9;
const FIELD_WIDTH: f32 = 76.0;
const NAME_WIDTH: f32 = 140.0;
const BLUR_SIZE: [f64; 2] = [1.3, 1.3];
pub(super) const SUBJECTS: [Subject; 2] = [Subject::Face, Subject::Text];
const BETTER_SIDE: u32 = 1920;
const FIND_VALUES: [(&str, &str, &str, &str); 7] = [
    (
        "size",
        "Search Picture Size (px)",
        "The model looks at the picture scaled to this size.",
        "Larger finds smaller things and takes much longer.",
    ),
    (
        "start",
        "Min Score To Start (0-1)",
        "How sure the model must be to start following a thing.",
        "Lower finds more things, and more wrong ones.",
    ),
    (
        "keep",
        "Min Score To Continue (0-1)",
        "How sure the model must be on later frames to keep following.",
        "Lower keeps a thing through blur and turns.",
    ),
    (
        "overlap",
        "Min Box Overlap (0-1)",
        "How much a box must overlap the box of the frame before.",
        "Lower follows faster things, and can mix up neighbors.",
    ),
    (
        "wait",
        "Max Time Hidden (s)",
        "A thing hidden for longer than this counts as a new thing.",
        "Longer follows a thing behind trees and people.",
    ),
    (
        "shortest",
        "Min Time Followed (s)",
        "A thing followed for less time than this is dropped.",
        "Longer removes more wrong finds, and more short ones.",
    ),
    (
        "steady",
        "Min Share Of Frames Seen (0-1)",
        "The share of its frames on which a thing must be seen.",
        "Higher removes things that flicker on and off.",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Field {
    Zoom,
    ZoomX,
    ZoomY,
    LinkedZoomY,
    X,
    Y,
    Rotation,
    AnchorX,
    AnchorY,
    Pitch,
    Yaw,
    Left,
    Right,
    Top,
    Bottom,
    Softness,
    Opacity,
    StartZoom,
    StartX,
    StartY,
    EndZoom,
    EndX,
    EndY,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Picture(Field),
    Effect(usize, Knob),
    Center(Axis),
    Volume,
    Pan,
    Speed,
    Rate,
    Length,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Knob {
    Gaussian(GaussianKnob),
    Mosaic(MosaicKnob),
    Glow(GlowKnob),
    Grain(GrainKnob),
    Vignette(VignetteKnob),
    Sharpen(SharpenKnob),
    Area(AreaKnob),
    Blend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AreaKnob {
    Softness,
    CenterX,
    CenterY,
    Width,
    Height,
    Rotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GrainKnob {
    Opacity,
    Texture,
    Size,
    Aspect,
    Strength,
    Offset,
    Symmetry,
    Softness,
    Saturation,
    Detail,
    Gain(usize),
    Tone(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GlowKnob {
    Threshold,
    AlphaThreshold,
    Spread,
    Ratio,
    Relative(usize),
    Gain,
    Gamma,
    Saturation,
    Filter(usize),
    SecondGain,
    SecondSpread,
    SecondColor(usize),
    Strength,
    Detail,
    Scale,
    NoiseRatio,
    Evolution,
    Seethe,
    Opacity,
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GaussianKnob {
    Strength,
    HorizontalStrength,
    VerticalStrength,
    Horizontal(usize),
    Vertical(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MosaicKnob {
    Frequency,
    Smooth,
    Aliasing,
    XOffset,
    YOffset,
    Aspect,
    PositionX,
    PositionY,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum VignetteKnob {
    Size,
    Rotation,
    Anamorphism,
    CenterX,
    CenterY,
    Softness,
    Transparency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SharpenKnob {
    Amount,
    Size,
    Fine,
    Medium,
    Large,
    Chroma,
}

struct Dial {
    label: &'static str,
    usual: f64,
    range: RangeInclusive<f64>,
    speed: f64,
}

const CHANNELS: [&str; 4] = ["Red", "Green", "Blue", "Alpha"];

const fn pair((low, high): (f64, f64)) -> RangeInclusive<f64> {
    low..=high
}

impl GaussianKnob {
    fn dial(self) -> Dial {
        let channel = |c: usize| Dial {
            label: CHANNELS[c],
            usual: 1.0,
            range: 0.0..=2.0,
            speed: 0.01,
        };
        let strength = |label| Dial {
            label,
            usual: GaussianBlur::STRENGTH,
            range: 0.0..=1.0,
            speed: 0.005,
        };
        match self {
            Self::Strength => strength("Strength"),
            Self::HorizontalStrength => strength("Horizontal Strength"),
            Self::VerticalStrength => strength("Vertical Strength"),
            Self::Horizontal(c) | Self::Vertical(c) => channel(c),
        }
    }

    fn read(self, g: &GaussianBlur) -> f64 {
        match self {
            Self::Strength | Self::HorizontalStrength => g.across(),
            Self::VerticalStrength => g.vertical_strength.unwrap_or(GaussianBlur::STRENGTH),
            Self::Horizontal(c) => g.across_channels()[c],
            Self::Vertical(c) => g.down_channels()[c],
        }
    }

    fn put(self, g: &mut GaussianBlur, v: f64) {
        match self {
            Self::Strength | Self::HorizontalStrength => g.strength = Some(v),
            Self::VerticalStrength => g.vertical_strength = Some(v),
            Self::Horizontal(c) => {
                let mut a = g.across_channels();
                a[c] = v;
                g.horizontal_channels = Some(a);
            }
            Self::Vertical(c) => {
                let mut a = g.down_channels();
                a[c] = v;
                g.vertical_channels = Some(a);
            }
        }
    }

    fn param(self, g: &GaussianBlur) -> (&'static str, Value) {
        match self {
            Self::Strength | Self::HorizontalStrength => ("strength", json!(g.across())),
            Self::VerticalStrength => ("vertical_strength", json!(self.read(g))),
            Self::Horizontal(_) => ("horizontal_channels", json!(g.across_channels())),
            Self::Vertical(_) => ("vertical_channels", json!(g.down_channels())),
        }
    }
}

impl MosaicKnob {
    fn dial(self) -> Dial {
        let dial = |label, usual, range, speed| Dial {
            label,
            usual,
            range,
            speed,
        };
        let offsets = pair(MosaicBlur::OFFSETS);
        let anywhere = pair(MosaicBlur::PLACES);
        match self {
            Self::Frequency => dial(
                "Pixel Frequency",
                MosaicBlur::FREQUENCY,
                pair(MosaicBlur::FREQUENCIES),
                0.5,
            ),
            Self::Smooth => dial("Smooth Strength", 0.0, pair(MosaicBlur::SMOOTHS), 0.005),
            Self::Aliasing => dial(
                "Aliasing",
                MosaicBlur::ALIASING,
                pair(MosaicBlur::ALIASINGS),
                0.005,
            ),
            Self::XOffset => dial("X Offset", 0.0, offsets, 0.005),
            Self::YOffset => dial("Y Offset", 0.0, offsets, 0.005),
            Self::Aspect => dial(
                "Aspect",
                MosaicBlur::STRETCH,
                pair(MosaicBlur::STRETCHES),
                0.005,
            ),
            Self::PositionX => dial("Position X", MosaicBlur::MIDDLE[0], anywhere, 0.001),
            Self::PositionY => dial("Position Y", MosaicBlur::MIDDLE[1], anywhere, 0.001),
        }
    }

    fn read(self, m: &MosaicBlur) -> f64 {
        match self {
            Self::Frequency => m.frequency(),
            Self::Smooth => m.smooth(),
            Self::Aliasing => m.box_size(),
            Self::XOffset => m.offsets()[0],
            Self::YOffset => m.offsets()[1],
            Self::Aspect => m.stretch(),
            Self::PositionX => m.place()[0],
            Self::PositionY => m.place()[1],
        }
    }

    fn put(self, m: &mut MosaicBlur, v: f64) {
        match self {
            Self::Frequency => m.pixel_frequency = Some(v),
            Self::Smooth => m.smooth_strength = Some(v),
            Self::Aliasing => m.aliasing = Some(v),
            Self::XOffset => m.x_offset = Some(v),
            Self::YOffset => m.y_offset = Some(v),
            Self::Aspect => m.aspect = Some(v),
            Self::PositionX => m.position = Some([v, m.place()[1]]),
            Self::PositionY => m.position = Some([m.place()[0], v]),
        }
    }

    fn param(self, m: &MosaicBlur) -> (&'static str, Value) {
        match self {
            Self::Frequency => ("pixel_frequency", json!(m.frequency())),
            Self::Smooth => ("smooth_strength", json!(m.smooth())),
            Self::Aliasing => ("aliasing", json!(m.box_size())),
            Self::XOffset => ("x_offset", json!(m.offsets()[0])),
            Self::YOffset => ("y_offset", json!(m.offsets()[1])),
            Self::Aspect => ("aspect", json!(m.stretch())),
            Self::PositionX | Self::PositionY => ("position", json!(m.place())),
        }
    }
}

impl GlowKnob {
    fn dial(self) -> Dial {
        let dial = |label, usual, range, speed| Dial {
            label,
            usual,
            range,
            speed,
        };
        let unit = pair(Glow::UNIT);
        let gains = pair(Glow::GAINS);
        let spreads = pair(Glow::SPREADS);
        let colors = pair(Glow::COLORS);
        match self {
            Self::Threshold => dial("Shine Threshold", Glow::THRESHOLD, unit, 0.005),
            Self::AlphaThreshold => {
                dial("Alpha Level Threshold", Glow::ALPHA_THRESHOLD, unit, 0.005)
            }
            Self::Spread => dial("Spread", Glow::SPREAD, spreads, 0.005),
            Self::Ratio => dial("H vs V Ratio", 0.0, pair(Glow::RATIOS), 0.005),
            Self::Relative(c) => dial(CHANNELS[c], 1.0, pair(Glow::RELATIVES), 0.005),
            Self::Gain => dial("Gain", Glow::GAIN, gains, 0.005),
            Self::Gamma => dial("Gamma", 1.0, pair(Glow::GAMMAS), 0.005),
            Self::Saturation => dial("Saturation", 1.0, pair(Glow::SATURATIONS), 0.005),
            Self::Filter(c) | Self::SecondColor(c) => dial(CHANNELS[c], 1.0, colors, 0.005),
            Self::SecondGain => dial("Gain", Glow::SECONDARY_GAIN, gains, 0.005),
            Self::SecondSpread => dial("Spread", Glow::SECONDARY_SPREAD, spreads, 0.005),
            Self::Strength => dial("Strength", Glow::STRENGTH, pair(Glow::STRENGTHS), 0.005),
            Self::Detail => dial("Detail", Glow::DETAIL, unit, 0.005),
            Self::Scale => dial("Scale", Glow::SCALE, pair(Glow::SCALES), 0.005),
            Self::NoiseRatio => dial("H vs V Ratio", 0.0, pair(Glow::NOISE_RATIOS), 0.005),
            Self::Evolution => dial("Evolution", 0.0, pair(Glow::EVOLUTIONS), 0.005),
            Self::Seethe => dial("Seethe", Glow::SEETHE, pair(Glow::SEETHES), 0.005),
            Self::Opacity => dial("Opacity", 1.0, unit, 0.005),
            Self::Recovery => dial("Source Recovery", 0.0, pair(Glow::RECOVERIES), 0.005),
        }
    }

    fn read(self, g: &Glow) -> f64 {
        match self {
            Self::Threshold => g.shine(),
            Self::AlphaThreshold => g.alpha_shine(),
            Self::Spread => g.wide(),
            Self::Ratio => g.ratio(),
            Self::Relative(c) => g.relative()[c],
            Self::Gain => g.bright(),
            Self::Gamma => g.curve(),
            Self::Saturation => g.colorful(),
            Self::Filter(c) => g.filter()[c],
            Self::SecondGain => g.second_bright(),
            Self::SecondSpread => g.second_wide(),
            Self::SecondColor(c) => g.second_color()[c],
            Self::Strength => g.noise_strength(),
            Self::Detail => g.noise_detail(),
            Self::Scale => g.noise_scale(),
            Self::NoiseRatio => g.noise_ratio(),
            Self::Evolution => g.noise_evolution(),
            Self::Seethe => g.noise_seethe(),
            Self::Opacity => g.shown(),
            Self::Recovery => g.recovers(),
        }
    }

    fn put(self, g: &mut Glow, v: f64) {
        let with = |mut all: [f64; 3], c: usize| {
            all[c] = v;
            Some(all)
        };
        match self {
            Self::Threshold => g.threshold = Some(v),
            Self::AlphaThreshold => g.alpha_threshold = Some(v),
            Self::Spread => g.spread = Some(v),
            Self::Ratio => g.hv_ratio = Some(v),
            Self::Relative(c) => g.relative_spread = with(g.relative(), c),
            Self::Gain => g.gain = Some(v),
            Self::Gamma => g.gamma = Some(v),
            Self::Saturation => g.saturation = Some(v),
            Self::Filter(c) => g.color_filter = with(g.filter(), c),
            Self::SecondGain => g.secondary_gain = Some(v),
            Self::SecondSpread => g.secondary_spread = Some(v),
            Self::SecondColor(c) => g.secondary_color = with(g.second_color(), c),
            Self::Strength => g.atmosphere_strength = Some(v),
            Self::Detail => g.atmosphere_detail = Some(v),
            Self::Scale => g.atmosphere_scale = Some(v),
            Self::NoiseRatio => g.atmosphere_ratio = Some(v),
            Self::Evolution => g.atmosphere_evolution = Some(v),
            Self::Seethe => g.atmosphere_seethe = Some(v),
            Self::Opacity => g.opacity = Some(v),
            Self::Recovery => g.recovery = Some(v),
        }
    }

    fn param(self, g: &Glow) -> (&'static str, Value) {
        match self {
            Self::Threshold => ("threshold", json!(g.shine())),
            Self::AlphaThreshold => ("alpha_threshold", json!(g.alpha_shine())),
            Self::Spread => ("spread", json!(g.wide())),
            Self::Ratio => ("hv_ratio", json!(g.ratio())),
            Self::Relative(_) => ("relative_spread", json!(g.relative())),
            Self::Gain => ("gain", json!(g.bright())),
            Self::Gamma => ("gamma", json!(g.curve())),
            Self::Saturation => ("saturation", json!(g.colorful())),
            Self::Filter(_) => ("color_filter", json!(g.filter())),
            Self::SecondGain => ("secondary_gain", json!(g.second_bright())),
            Self::SecondSpread => ("secondary_spread", json!(g.second_wide())),
            Self::SecondColor(_) => ("secondary_color", json!(g.second_color())),
            Self::Strength => ("atmosphere_strength", json!(g.noise_strength())),
            Self::Detail => ("atmosphere_detail", json!(g.noise_detail())),
            Self::Scale => ("atmosphere_scale", json!(g.noise_scale())),
            Self::NoiseRatio => ("atmosphere_ratio", json!(g.noise_ratio())),
            Self::Evolution => ("atmosphere_evolution", json!(g.noise_evolution())),
            Self::Seethe => ("atmosphere_seethe", json!(g.noise_seethe())),
            Self::Opacity => ("opacity", json!(g.shown())),
            Self::Recovery => ("recovery", json!(g.recovers())),
        }
    }
}

const TONES: [&str; 3] = ["Shadows", "Midtones", "Highlights"];

impl GrainKnob {
    fn dial(self) -> Dial {
        let dial = |label, usual, range, speed| Dial {
            label,
            usual,
            range,
            speed,
        };
        let unit = pair(FilmGrain::UNIT);
        match self {
            Self::Opacity => dial("Opacity", FilmGrain::OPACITY, unit, 0.005),
            Self::Texture => dial("Texture", FilmGrain::TEXTURE, unit, 0.005),
            Self::Size => dial("Grain Size", FilmGrain::SIZE, unit, 0.005),
            Self::Aspect => dial("Grain Aspect Ratio", 1.0, pair(FilmGrain::ASPECTS), 0.005),
            Self::Strength => dial("Grain Strength", FilmGrain::STRENGTH, unit, 0.005),
            Self::Offset => dial("Offset", FilmGrain::OFFSET, unit, 0.005),
            Self::Symmetry => dial("Symmetry", FilmGrain::SYMMETRY, unit, 0.005),
            Self::Softness => dial("Softness", FilmGrain::SOFTNESS, unit, 0.005),
            Self::Saturation => dial("Saturation", 0.0, unit, 0.005),
            Self::Detail => dial("Detail Loss", 0.0, pair(FilmGrain::DETAILS), 0.005),
            Self::Gain(c) => dial(CHANNELS[c], 1.0, pair(FilmGrain::GAINS), 0.005),
            Self::Tone(t) => dial(TONES[t], FilmGrain::TONE, unit, 0.005),
        }
    }

    fn read(self, g: &FilmGrain) -> f64 {
        match self {
            Self::Opacity => g.shown(),
            Self::Texture => g.rough(),
            Self::Size => g.large(),
            Self::Aspect => g.stretch(),
            Self::Strength => g.strong(),
            Self::Offset => g.middle(),
            Self::Symmetry => g.lean(),
            Self::Softness => g.soft(),
            Self::Saturation => g.colorful(),
            Self::Detail => g.lost(),
            Self::Gain(c) => g.gains()[c],
            Self::Tone(t) => g.tones()[t],
        }
    }

    fn put(self, g: &mut FilmGrain, v: f64) {
        match self {
            Self::Opacity => g.opacity = Some(v),
            Self::Texture => g.texture = Some(v),
            Self::Size => g.grain_size = Some(v),
            Self::Aspect => g.grain_aspect = Some(v),
            Self::Strength => g.grain_strength = Some(v),
            Self::Offset => g.offset = Some(v),
            Self::Symmetry => g.symmetry = Some(v),
            Self::Softness => g.softness = Some(v),
            Self::Saturation => g.saturation = Some(v),
            Self::Detail => g.detail_loss = Some(v),
            Self::Gain(c) => {
                let mut all = g.gains();
                all[c] = v;
                g.channel_gains = Some(all);
            }
            Self::Tone(0) => g.shadows = Some(v),
            Self::Tone(1) => g.midtones = Some(v),
            Self::Tone(_) => g.highlights = Some(v),
        }
    }

    fn param(self, g: &FilmGrain) -> (&'static str, Value) {
        match self {
            Self::Opacity => ("opacity", json!(g.shown())),
            Self::Texture => ("texture", json!(g.rough())),
            Self::Size => ("grain_size", json!(g.large())),
            Self::Aspect => ("grain_aspect", json!(g.stretch())),
            Self::Strength => ("grain_strength", json!(g.strong())),
            Self::Offset => ("offset", json!(g.middle())),
            Self::Symmetry => ("symmetry", json!(g.lean())),
            Self::Softness => ("softness", json!(g.soft())),
            Self::Saturation => ("saturation", json!(g.colorful())),
            Self::Detail => ("detail_loss", json!(g.lost())),
            Self::Gain(_) => ("channel_gains", json!(g.gains())),
            Self::Tone(0) => ("shadows", json!(g.tones()[0])),
            Self::Tone(1) => ("midtones", json!(g.tones()[1])),
            Self::Tone(_) => ("highlights", json!(g.tones()[2])),
        }
    }
}

impl VignetteKnob {
    fn dial(self) -> Dial {
        let dial = |label, usual, range, speed| Dial {
            label,
            usual,
            range,
            speed,
        };
        let anywhere = pair(Vignette::PLACES);
        let share = pair(Vignette::SHARES);
        match self {
            Self::Size => dial("Size", Vignette::SIZE, pair(Vignette::SIZES), 0.005),
            Self::Rotation => dial("Rotation", 0.0, pair(Vignette::TURNS), 0.5),
            Self::Anamorphism => dial(
                "Anamorphism",
                Vignette::STRETCH,
                pair(Vignette::STRETCHES),
                0.005,
            ),
            Self::CenterX => dial("Center X", Vignette::MIDDLE[0], anywhere, 0.001),
            Self::CenterY => dial("Center Y", Vignette::MIDDLE[1], anywhere, 0.001),
            Self::Softness => dial("Softness", Vignette::SOFTNESS, share, 0.005),
            Self::Transparency => dial("Transparency", 0.0, share, 0.005),
        }
    }

    fn read(self, v: &Vignette) -> f64 {
        match self {
            Self::Size => v.large(),
            Self::Rotation => v.turn(),
            Self::Anamorphism => v.stretch(),
            Self::CenterX => v.middle()[0],
            Self::CenterY => v.middle()[1],
            Self::Softness => v.soft(),
            Self::Transparency => v.through(),
        }
    }

    fn put(self, v: &mut Vignette, to: f64) {
        match self {
            Self::Size => v.size = Some(to),
            Self::Rotation => v.rotation = Some(to),
            Self::Anamorphism => v.anamorphism = Some(to),
            Self::CenterX => v.center = Some([to, v.middle()[1]]),
            Self::CenterY => v.center = Some([v.middle()[0], to]),
            Self::Softness => v.softness = Some(to),
            Self::Transparency => v.transparency = Some(to),
        }
    }

    fn param(self, v: &Vignette) -> (&'static str, Value) {
        match self {
            Self::Size => ("size", json!(v.large())),
            Self::Rotation => ("rotation", json!(v.turn())),
            Self::Anamorphism => ("anamorphism", json!(v.stretch())),
            Self::CenterX | Self::CenterY => ("center", json!(v.middle())),
            Self::Softness => ("softness", json!(v.soft())),
            Self::Transparency => ("transparency", json!(v.through())),
        }
    }
}

impl SharpenKnob {
    fn dial(self) -> Dial {
        let gain = |label, usual| Dial {
            label,
            usual,
            range: pair(Sharpen::GAINS),
            speed: 0.01,
        };
        match self {
            Self::Amount => gain("Sharpen Amount", Sharpen::AMOUNT),
            Self::Size => Dial {
                label: "Fine Detail Size",
                usual: Sharpen::SIZE,
                range: pair(Sharpen::SIZES),
                speed: 0.0005,
            },
            Self::Fine => gain("Fine Details", Sharpen::GAIN),
            Self::Medium => gain("Medium Details", Sharpen::GAIN),
            Self::Large => gain("Large Details", Sharpen::GAIN),
            Self::Chroma => gain("Sharpen Chroma", Sharpen::GAIN),
        }
    }

    fn read(self, s: &Sharpen) -> f64 {
        match self {
            Self::Amount => s.amount(),
            Self::Size => s.size(),
            Self::Fine => s.fine(),
            Self::Medium => s.medium(),
            Self::Large => s.large(),
            Self::Chroma => s.chroma(),
        }
    }

    fn put(self, s: &mut Sharpen, to: f64) {
        match self {
            Self::Amount => s.sharpen_amount = Some(to),
            Self::Size => s.fine_detail_size = Some(to),
            Self::Fine => s.fine_details = Some(to),
            Self::Medium => s.medium_details = Some(to),
            Self::Large => s.large_details = Some(to),
            Self::Chroma => s.sharpen_chroma = Some(to),
        }
    }

    fn param(self, s: &Sharpen) -> (&'static str, Value) {
        match self {
            Self::Amount => ("sharpen_amount", json!(s.amount())),
            Self::Size => ("fine_detail_size", json!(s.size())),
            Self::Fine => ("fine_details", json!(s.fine())),
            Self::Medium => ("medium_details", json!(s.medium())),
            Self::Large => ("large_details", json!(s.large())),
            Self::Chroma => ("sharpen_chroma", json!(s.chroma())),
        }
    }
}

impl AreaKnob {
    fn dial(self) -> Dial {
        let dial = |label, usual, range, speed| Dial {
            label,
            usual,
            range,
            speed,
        };
        let [x, y] = EffectArea::MIDDLE;
        let [w, h] = EffectArea::WHOLE;
        match self {
            Self::Softness => dial("Area Softness", 0.0, 0.0..=1.0, 0.005),
            Self::CenterX => dial("Center X", x, -REACH..=REACH, 0.001),
            Self::CenterY => dial("Center Y", y, -REACH..=REACH, 0.001),
            Self::Width => dial("Width", w, 0.0..=REACH, 0.001),
            Self::Height => dial("Height", h, 0.0..=REACH, 0.001),
            Self::Rotation => dial("Rotation", 0.0, -TURN..=TURN, 0.5),
        }
    }

    fn read(self, a: &EffectArea) -> f64 {
        match self {
            Self::Softness => a.soft(),
            Self::CenterX => a.middle()[0],
            Self::CenterY => a.middle()[1],
            Self::Width => a.extent()[0],
            Self::Height => a.extent()[1],
            Self::Rotation => a.turn(),
        }
    }

    fn put(self, a: &mut EffectArea, to: f64) {
        match self {
            Self::Softness => a.softness = Some(to),
            Self::CenterX => a.center = Some([to, a.middle()[1]]),
            Self::CenterY => a.center = Some([a.middle()[0], to]),
            Self::Width => a.size = Some([to, a.extent()[1]]),
            Self::Height => a.size = Some([a.extent()[0], to]),
            Self::Rotation => a.rotation = Some(to),
        }
    }

    fn param(self, a: &EffectArea) -> Value {
        match self {
            Self::Softness => json!({ "softness": a.soft() }),
            Self::CenterX | Self::CenterY => json!({ "center": a.middle() }),
            Self::Width | Self::Height => json!({ "size": a.extent() }),
            Self::Rotation => json!({ "rotation": a.turn() }),
        }
    }
}

impl Knob {
    fn dial(self) -> Dial {
        match self {
            Knob::Gaussian(k) => k.dial(),
            Knob::Mosaic(k) => k.dial(),
            Knob::Glow(k) => k.dial(),
            Knob::Grain(k) => k.dial(),
            Knob::Vignette(k) => k.dial(),
            Knob::Sharpen(k) => k.dial(),
            Knob::Area(k) => k.dial(),
            Knob::Blend => Dial {
                label: "Blend",
                usual: 1.0,
                range: 0.0..=1.0,
                speed: 0.005,
            },
        }
    }

    fn read(self, e: &Effect) -> f64 {
        match (self, &e.kind) {
            (Knob::Gaussian(k), EffectKind::GaussianBlur(g)) => k.read(g),
            (Knob::Mosaic(k), EffectKind::MosaicBlur(m)) => k.read(m),
            (Knob::Glow(k), EffectKind::Glow(g)) => k.read(g),
            (Knob::Grain(k), EffectKind::FilmGrain(g)) => k.read(g),
            (Knob::Vignette(k), EffectKind::Vignette(s)) => k.read(s),
            (Knob::Sharpen(k), EffectKind::Sharpen(s)) => k.read(s),
            (Knob::Area(k), _) => e.area.as_ref().map_or(k.dial().usual, |a| k.read(a)),
            (Knob::Blend, _) => e.mixed(),
            _ => self.dial().usual,
        }
    }

    fn written(self, e: &Effect, v: f64) -> Effect {
        let r = self.dial().range;
        let v = v.clamp(*r.start(), *r.end());
        let mut out = e.clone();
        match (self, &mut out.kind) {
            (Knob::Gaussian(k), EffectKind::GaussianBlur(g)) => k.put(g, v),
            (Knob::Mosaic(k), EffectKind::MosaicBlur(m)) => k.put(m, v),
            (Knob::Glow(k), EffectKind::Glow(g)) => k.put(g, v),
            (Knob::Grain(k), EffectKind::FilmGrain(g)) => k.put(g, v),
            (Knob::Vignette(k), EffectKind::Vignette(s)) => k.put(s, v),
            (Knob::Sharpen(k), EffectKind::Sharpen(s)) => k.put(s, v),
            (Knob::Area(k), _) => {
                if let Some(a) = out.area.as_mut() {
                    k.put(a, v);
                }
            }
            (Knob::Blend, _) => out.blend = Some(v),
            _ => {}
        }
        out
    }

    fn put(self, e: &Effect, v: f64) -> Effect {
        effect_checked(&self.written(e, v)).unwrap_or_else(|_| e.clone())
    }

    fn keyed_value(self, e: &Effect) -> Option<(EffectValue, usize)> {
        let values = e.values();
        let mut probe = e.clone();
        for value in &values {
            probe.write_point(*value, [f64::NAN; 2]);
        }
        let out = self.written(&probe, 0.0);
        values
            .into_iter()
            .filter(|value| !e.keys_of(*value).is_empty())
            .find_map(|value| {
                let axes = if value.pair() { 2 } else { 1 };
                let axis = out.point(value)[..axes].iter().position(|n| !n.is_nan())?;
                Some((value, axis))
            })
    }

    fn param(self, e: &Effect) -> (&'static str, Value) {
        match (self, &e.kind) {
            (Knob::Gaussian(k), EffectKind::GaussianBlur(g)) => k.param(g),
            (Knob::Mosaic(k), EffectKind::MosaicBlur(m)) => k.param(m),
            (Knob::Glow(k), EffectKind::Glow(g)) => k.param(g),
            (Knob::Grain(k), EffectKind::FilmGrain(g)) => k.param(g),
            (Knob::Vignette(k), EffectKind::Vignette(s)) => k.param(s),
            (Knob::Sharpen(k), EffectKind::Sharpen(s)) => k.param(s),
            (Knob::Area(k), _) => {
                let area = e.area.as_ref();
                ("area", area.map_or_else(|| json!({}), |a| k.param(a)))
            }
            _ => ("blend", json!(e.mixed())),
        }
    }
}

fn border_label(b: Border) -> &'static str {
    match b {
        Border::Black => "Black",
        Border::Replicate => "Replicate",
        Border::Reflect => "Reflect",
        Border::WrapAround => "Wrap-Around",
    }
}

fn knob_picture(c: &Chosen, (n, k): (usize, Knob), v: f64) -> Option<Picture> {
    let e = c.shown.effects.get(n)?;
    let mut p = c.shown.clone();
    p.effects[n] = match k.keyed_value(e) {
        Some((value, axis)) => {
            let mut keyed = e.clone();
            let r = k.dial().range;
            let mut point = e.point_at(value, c.frame as f64);
            point[axis] = v.clamp(*r.start(), *r.end());
            keyed.put_point(value, c.frame, point);
            keyed
        }
        None => k.put(e, v),
    };
    Some(p)
}

fn effect_key_times(c: &Clip, n: usize) -> Vec<i64> {
    let effects = c.picture.as_ref().map_or(&[][..], |p| &p.effects);
    effects.get(n).map_or_else(Vec::new, |e| {
        e.keys
            .iter()
            .filter(|k| !k.value.of_area())
            .flat_map(|k| k.keys.iter())
            .map(|k| c.key_time(k.frame).round() as i64)
            .collect()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Reverse,
    Freeze,
}

impl Direction {
    const ALL: [Direction; 3] = [Direction::Forward, Direction::Reverse, Direction::Freeze];

    fn of(speed: f64) -> Self {
        if speed == 0.0 {
            Direction::Freeze
        } else if speed < 0.0 {
            Direction::Reverse
        } else {
            Direction::Forward
        }
    }

    fn label(self) -> &'static str {
        match self {
            Direction::Forward => "Forward",
            Direction::Reverse => "Reverse",
            Direction::Freeze => "Freeze",
        }
    }

    fn turn(self, d: &mut SpeedDialog) {
        d.freeze = self == Direction::Freeze;
        if !d.freeze {
            d.reverse = self == Direction::Reverse;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Shape {
    pub(super) frame: [f64; 2],
    pub(super) fitted: [f64; 2],
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Zoom => "Zoom",
            Field::ZoomX => "Zoom X",
            Field::ZoomY | Field::LinkedZoomY => "Zoom Y",
            Field::X => "Position X",
            Field::Y => "Position Y",
            Field::Rotation => "Rotation",
            Field::AnchorX => "Anchor X",
            Field::AnchorY => "Anchor Y",
            Field::Pitch => "Pitch",
            Field::Yaw => "Yaw",
            Field::Left => "Left",
            Field::Right => "Right",
            Field::Top => "Top",
            Field::Bottom => "Bottom",
            Field::Softness => "Softness",
            Field::Opacity => "Opacity",
            Field::StartZoom => "Start Zoom",
            Field::StartX => "Start X",
            Field::StartY => "Start Y",
            Field::EndZoom => "End Zoom",
            Field::EndX => "End X",
            Field::EndY => "End Y",
        }
    }

    fn starts(self) -> Option<bool> {
        match self {
            Field::StartZoom | Field::StartX | Field::StartY => Some(true),
            Field::EndZoom | Field::EndX | Field::EndY => Some(false),
            _ => None,
        }
    }

    fn zoom_box(self, z: &DynamicZoom) -> ZoomBox {
        if self.starts() == Some(false) {
            z.end
        } else {
            z.start
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Field::Zoom
            | Field::ZoomX
            | Field::ZoomY
            | Field::LinkedZoomY
            | Field::StartZoom
            | Field::EndZoom
            | Field::Opacity => "%",
            Field::Rotation => "°",
            _ => "",
        }
    }

    fn speed(self) -> f64 {
        match self {
            Field::Zoom
            | Field::ZoomX
            | Field::ZoomY
            | Field::LinkedZoomY
            | Field::Rotation
            | Field::StartZoom
            | Field::EndZoom
            | Field::Softness
            | Field::Opacity => 0.5,
            Field::Pitch | Field::Yaw => 0.01,
            _ => 1.0,
        }
    }

    fn usual(self) -> f64 {
        match self {
            Field::Zoom
            | Field::ZoomX
            | Field::ZoomY
            | Field::LinkedZoomY
            | Field::EndZoom
            | Field::Opacity => 100.0,
            Field::StartZoom => 100.0 / DynamicZoom::default().start.size,
            _ => 0.0,
        }
    }

    fn axes(self) -> Vec<Field> {
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY | Field::LinkedZoomY => {
                vec![Field::ZoomX, Field::ZoomY]
            }
            Field::X | Field::Y => vec![Field::X, Field::Y],
            Field::AnchorX | Field::AnchorY => vec![Field::AnchorX, Field::AnchorY],
            f => vec![f],
        }
    }

    fn channel(self) -> Option<PictureValue> {
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY | Field::LinkedZoomY => {
                Some(PictureValue::Zoom)
            }
            Field::X | Field::Y => Some(PictureValue::Position),
            Field::Rotation => Some(PictureValue::Rotation),
            Field::AnchorX | Field::AnchorY => Some(PictureValue::Anchor),
            Field::Pitch => Some(PictureValue::Pitch),
            Field::Yaw => Some(PictureValue::Yaw),
            Field::Left => Some(PictureValue::CropLeft),
            Field::Right => Some(PictureValue::CropRight),
            Field::Top => Some(PictureValue::CropTop),
            Field::Bottom => Some(PictureValue::CropBottom),
            Field::Softness => Some(PictureValue::CropSoftness),
            Field::Opacity => Some(PictureValue::Opacity),
            _ => None,
        }
    }

    fn read(self, p: &Picture, s: &Shape, frame: Option<i64>) -> f64 {
        let owned = frame
            .filter(|_| self.channel().is_some_and(|v| p.has_keys(v)))
            .map(|f| p.sampled(f as f64));
        let p = owned.as_ref().unwrap_or(p);
        let ([x, y], [zx, zy], c) = (p.place(), p.scale(), p.crop);
        let ([w, h], [fw, fh], [ax, ay]) = (s.frame, s.fitted, p.pivot());
        let b = self.zoom_box(&p.dynamic_zoom.unwrap_or_default());
        match self {
            Field::Zoom | Field::ZoomX => zx * 100.0,
            Field::ZoomY | Field::LinkedZoomY => zy * 100.0,
            Field::X => (x - 0.5) * w,
            Field::Y => (0.5 - y) * h,
            Field::Rotation => p.turn(),
            Field::AnchorX => (ax - 0.5) * fw,
            Field::AnchorY => (0.5 - ay) * fh,
            Field::Pitch => p.pitch(),
            Field::Yaw => p.yaw(),
            Field::Left => c.left * fw,
            Field::Right => c.right * fw,
            Field::Top => c.top * fh,
            Field::Bottom => c.bottom * fh,
            Field::Softness => p.softness(),
            Field::Opacity => p.opaque() * 100.0,
            Field::StartZoom | Field::EndZoom => 100.0 / b.size,
            Field::StartX | Field::EndX => (0.5 - b.center[0]) / b.size * fw,
            Field::StartY | Field::EndY => (b.center[1] - 0.5) / b.size * fh,
        }
    }

    fn is_usual(self, p: &Picture) -> bool {
        let ([x, y], [zx, zy], [ax, ay], c) = (p.place(), p.scale(), p.pivot(), p.crop);
        let usual = self.zoom_box(&DynamicZoom::default());
        let b = p.dynamic_zoom.map_or(usual, |z| self.zoom_box(&z));
        match self {
            Field::Zoom | Field::LinkedZoomY => zx == 1.0 && zy == 1.0,
            Field::ZoomX => zx == 1.0,
            Field::ZoomY => zy == 1.0,
            Field::X => x == 0.5,
            Field::Y => y == 0.5,
            Field::Rotation => p.turn() == 0.0,
            Field::AnchorX => ax == 0.5,
            Field::AnchorY => ay == 0.5,
            Field::Pitch => p.pitch() == 0.0,
            Field::Yaw => p.yaw() == 0.0,
            Field::Left => c.left == 0.0,
            Field::Right => c.right == 0.0,
            Field::Top => c.top == 0.0,
            Field::Bottom => c.bottom == 0.0,
            Field::Softness => p.softness() == 0.0,
            Field::Opacity => p.opaque() == 1.0,
            Field::StartZoom | Field::EndZoom => b.size == usual.size,
            Field::StartX | Field::EndX => b.center[0] == usual.center[0],
            Field::StartY | Field::EndY => b.center[1] == usual.center[1],
        }
    }

    fn range(self, s: &Shape) -> RangeInclusive<f64> {
        let ([w, h], [fw, fh]) = (s.frame, s.fitted);
        let (reach, turns, lean, soft) =
            (Picture::REACH, Picture::TURNS, Picture::LEAN, Picture::SOFT);
        let (box_reach, box_size) = (DynamicZoom::REACH, DynamicZoom::SIZE);
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY | Field::LinkedZoomY => {
                Picture::ZOOM.0 * 100.0..=Picture::ZOOM.1 * 100.0
            }
            Field::X => -reach * w..=reach * w,
            Field::Y => -reach * h..=reach * h,
            Field::Rotation => -turns..=turns,
            Field::AnchorX => -reach * fw..=reach * fw,
            Field::AnchorY => -reach * fh..=reach * fh,
            Field::Pitch | Field::Yaw => -lean..=lean,
            Field::Left | Field::Right => 0.0..=fw,
            Field::Top | Field::Bottom => 0.0..=fh,
            Field::Softness => -soft..=soft,
            Field::Opacity => 0.0..=100.0,
            Field::StartZoom | Field::EndZoom => 100.0 / box_size.1..=100.0 / box_size.0,
            Field::StartX | Field::EndX => {
                -box_reach * fw / box_size.0..=box_reach * fw / box_size.0
            }
            Field::StartY | Field::EndY => {
                -box_reach * fh / box_size.0..=box_reach * fh / box_size.0
            }
        }
    }

    fn put_at(self, p: &Picture, s: &Shape, v: f64, frame: Option<i64>) -> Picture {
        let r = self.range(s);
        let v = v.clamp(*r.start(), *r.end());
        if (self.read(p, s, frame) - v).abs() < SAME {
            return p.clone();
        }
        let at = frame.unwrap_or(0) as f64;
        let place = frame
            .filter(|_| p.has_keys(PictureValue::Position))
            .map(|_| p.place_at(at))
            .unwrap_or_else(|| p.place());
        let scale = frame
            .filter(|_| p.has_keys(PictureValue::Zoom))
            .map(|_| p.scale_at(at))
            .unwrap_or_else(|| p.scale());
        let pivot = frame
            .filter(|_| p.has_keys(PictureValue::Anchor))
            .map(|_| p.pivot_at(at))
            .unwrap_or_else(|| p.pivot());
        let ([x, y], [zx, zy], [ax, ay]) = (place, scale, pivot);
        let ([w, h], [fw, fh]) = (s.frame, s.fitted);
        let zoom = |v: f64| (v / 100.0).clamp(Picture::ZOOM.0, Picture::ZOOM.1);
        let at = |v: f64| v.clamp(0.5 - Picture::REACH, 0.5 + Picture::REACH);
        let box_at = |v: f64| v.clamp(-DynamicZoom::REACH, DynamicZoom::REACH);
        let mut out = p.clone();
        match self {
            Field::Zoom | Field::LinkedZoomY => out.zoom = Some([zoom(v); 2]),
            Field::ZoomX => out.zoom = Some([zoom(v), zy]),
            Field::ZoomY => out.zoom = Some([zx, zoom(v)]),
            Field::X => out.position = Some([at(0.5 + v / w), y]),
            Field::Y => out.position = Some([x, at(0.5 - v / h)]),
            Field::Rotation => out.rotation = Some(v),
            Field::AnchorX => out.anchor = Some([at(0.5 + v / fw), ay]),
            Field::AnchorY => out.anchor = Some([ax, at(0.5 - v / fh)]),
            Field::Pitch => out.pitch = Some(v),
            Field::Yaw => out.yaw = Some(v),
            Field::Left => out.crop.left = v / fw,
            Field::Right => out.crop.right = v / fw,
            Field::Top => out.crop.top = v / fh,
            Field::Bottom => out.crop.bottom = v / fh,
            Field::Softness => out.crop_softness = Some(v),
            Field::Opacity => out.opacity = Some(v / 100.0),
            Field::StartZoom
            | Field::StartX
            | Field::StartY
            | Field::EndZoom
            | Field::EndX
            | Field::EndY => {
                let mut z = p.dynamic_zoom.unwrap_or_default();
                let b = if self.starts() == Some(true) {
                    &mut z.start
                } else {
                    &mut z.end
                };
                match self {
                    Field::StartZoom | Field::EndZoom => {
                        let size = (100.0 / v).clamp(DynamicZoom::SIZE.0, DynamicZoom::SIZE.1);
                        let keep = |c: f64| box_at(0.5 - (0.5 - c) / b.size * size);
                        b.center = [keep(b.center[0]), keep(b.center[1])];
                        b.size = size;
                    }
                    Field::StartX | Field::EndX => b.center[0] = box_at(0.5 - v / fw * b.size),
                    _ => b.center[1] = box_at(0.5 + v / fh * b.size),
                }
                out.dynamic_zoom = Some(z);
            }
        }
        out
    }
}

impl Key {
    fn label(self) -> &'static str {
        match self {
            Key::Picture(f) => f.label(),
            Key::Effect(_, k) => k.dial().label,
            Key::Center(Axis::X) => "Center X",
            Key::Center(Axis::Y) => "Center Y",
            Key::Volume => "Volume",
            Key::Pan => "Pan",
            Key::Speed => "Speed",
            Key::Rate => "Frames per Second",
            Key::Length => "Duration",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Key::Picture(Field::Zoom | Field::ZoomX | Field::X | Field::AnchorX) => "X ",
            Key::Picture(Field::LinkedZoomY | Field::ZoomY | Field::Y | Field::AnchorY) => "Y ",
            _ => "",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Key::Picture(f) => f.suffix(),
            Key::Volume => " dB",
            Key::Speed => "%",
            Key::Rate => " fps",
            Key::Effect(..) | Key::Center(_) | Key::Length | Key::Pan => "",
        }
    }

    fn speed(self) -> f64 {
        match self {
            Key::Picture(f) => f.speed(),
            Key::Effect(_, k) => k.dial().speed,
            Key::Center(_) => 0.005,
            Key::Volume => 0.1,
            Key::Pan => 0.5,
            Key::Speed | Key::Length => 1.0,
            Key::Rate => 0.1,
        }
    }

    fn decimals(self) -> (usize, usize) {
        match self {
            Key::Picture(Field::Pitch | Field::Yaw) => (0, 3),
            Key::Picture(_) => (0, 2),
            Key::Effect(..) | Key::Center(_) => (3, 3),
            Key::Volume | Key::Pan => (1, 1),
            Key::Speed => (0, 2),
            Key::Rate => (0, 3),
            Key::Length => (0, 0),
        }
    }

    fn usual(self) -> f64 {
        match self {
            Key::Picture(f) => f.usual(),
            Key::Effect(_, k) => k.dial().usual,
            Key::Center(_) | Key::Volume | Key::Pan => 0.0,
            Key::Speed => 100.0,
            Key::Rate | Key::Length => 0.0,
        }
    }
}

fn typed(text: &str) -> Option<f64> {
    let t = text
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '%' | '°'))
        .map(|c| if c == '−' { '-' } else { c })
        .collect::<String>()
        .to_ascii_lowercase();
    t.strip_suffix("db")
        .or_else(|| t.strip_suffix("fps"))
        .unwrap_or(&t)
        .parse()
        .ok()
}

fn frames(p: &Project, text: &str, shown: &str, dropped: i64) -> Option<f64> {
    let spec = TimeSpec::parse(&timecode_input(
        text,
        shown,
        dropped,
        p.settings.fps.frame_digits(),
    )?)
    .ok()?;
    let n = p.time_context(None).duration(&spec).ok()?;
    Some(n as f64)
}

fn rate(p: &Project, c: &Clip) -> f64 {
    p.media(&c.media)
        .and_then(|m| m.info.video.as_ref())
        .filter(|v| !v.still)
        .map_or(p.settings.fps, |v| v.fps)
        .as_f64()
}

pub(super) fn shape(p: &Project, c: &Clip) -> Shape {
    let (w, h) = (p.settings.width.max(1), p.settings.height.max(1));
    let frame = [f64::from(w), f64::from(h)];
    let size = p
        .media(&c.media)
        .and_then(|m| m.info.video.as_ref())
        .filter(|v| v.width > 0 && v.height > 0)
        .map_or(frame, |v| {
            let sar = (v.sar[0], v.sar[1]);
            let [_, _, fw, fh] = fitted((v.width, v.height), sar, v.rotation, (w, h));
            [fw.max(1.0), fh.max(1.0)]
        });
    Shape {
        frame,
        fitted: size,
    }
}

fn name(p: &Project, c: &Clip) -> String {
    match (&c.name, &c.generator) {
        (Some(n), _) if !n.is_empty() => n.clone(),
        (_, Some(g)) => vidcut_core::generators::name(g).to_string(),
        _ => p
            .media(&c.media)
            .map_or_else(|| c.id.clone(), |m| m.name.clone()),
    }
}

fn uses(p: &Picture, tool: PictureTool) -> bool {
    cleared(p, tool) != *p
}

fn merged(params: &[Value]) -> Option<Value> {
    let rest = |v: &Value| {
        let mut v = v.clone();
        v.as_object_mut()?.remove("clips");
        Some(v)
    };
    let first = rest(params.first()?)?;
    params
        .iter()
        .all(|p| rest(p).as_ref() == Some(&first))
        .then(|| {
            let mut one = first.clone();
            one["clips"] = json!(
                params
                    .iter()
                    .map(|p| p["clips"].clone())
                    .collect::<Vec<_>>()
            );
            one
        })
}

#[derive(Debug, Clone)]
struct Chosen {
    id: String,
    stored: Picture,
    shown: Picture,
    shape: Shape,
    frame: i64,
    effect_keys: Vec<KeyMark>,
    paths: Vec<PathRow>,
    finds: bool,
}

#[derive(Debug, Clone)]
struct PathRow {
    id: String,
    name: Option<String>,
    subject: Option<Subject>,
}

#[derive(Debug, Clone)]
struct Finding {
    method: &'static str,
    params: Value,
    subject: Option<Subject>,
}

impl PathRow {
    fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone)]
struct Sound {
    id: String,
    stored: f64,
    gain: f64,
    muted: bool,
    keyed: bool,
    pan: f64,
    pan_on: bool,
}

type Lists<'a> = (&'a [Chosen], &'a [Sound], &'a [Paced]);

type Pace = (f64, i64);

#[derive(Debug, Clone)]
struct Paced {
    id: String,
    stored: Pace,
    shown: Pace,
    rate: f64,
    retime: ClipRetime,
    project: Retime,
    at: i64,
    ramped: bool,
}

#[derive(Debug, Clone)]
struct Retimed {
    since: Instant,
    clips: Vec<(String, Pace, Pace)>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Look<'a> {
    off: bool,
    why: Option<&'static str>,
    timecode: Option<(&'a Project, i64)>,
    dead_reset: bool,
}

#[derive(Debug, Clone)]
struct Held {
    field: Key,
    value: f64,
    from: f64,
    start: Vec<Chosen>,
    dragging: bool,
}

#[derive(Debug, Clone)]
struct Landing {
    since: Instant,
    pictures: Vec<(String, Picture, Picture)>,
}

#[derive(Debug, Clone)]
struct Gained {
    since: Instant,
    from: Vec<(String, f64)>,
    to: f64,
}

#[derive(Debug, Default)]
pub struct Inspector {
    unlinked: bool,
    held: Option<Held>,
    landing: Option<Landing>,
    gained: Option<Gained>,
    retimed: Option<Retimed>,
    ripple: Option<bool>,
    seen: Option<(Vec<String>, Vec<String>)>,
    focus: Option<egui::Id>,
    moved: bool,
    commit: bool,
    recolored: Option<Recolored>,
    tinted: Option<Tinted>,
    lut_pick: Option<(Vec<String>, Picked)>,
    naming: Option<Naming>,
    subject: Subject,
    find_values: [String; 7],
    find_open: bool,
    find_better: bool,
    find_error: bool,
    finding: Option<Finding>,
    area_edit: Option<usize>,
}

#[derive(Debug)]
struct Naming {
    clip: String,
    path: String,
    text: String,
}

#[derive(Debug)]
struct Tinted {
    clip: String,
    effect: usize,
    from: [u8; 3],
    to: [u8; 3],
    since: Option<Instant>,
}

#[derive(Debug)]
struct Recolored {
    clip: String,
    param: &'static str,
    from: [u8; 3],
    to: [u8; 3],
    since: Option<Instant>,
}

struct ColorRow {
    label: &'static str,
    param: &'static str,
    stored: [u8; 3],
    usual: [u8; 3],
}

fn color_rows(c: &Clip) -> Vec<ColorRow> {
    let row = |label, param, stored, usual| ColorRow {
        label,
        param,
        stored,
        usual,
    };
    let u = Gradient::default();
    match &c.generator {
        Some(Generator::SolidColor { color }) => vec![row("Color", "color", *color, [0, 0, 0])],
        Some(Generator::FourColorGradient(g)) => vec![
            row("Upper Left Color", "upper_left", g.upper_left, u.upper_left),
            row(
                "Upper Right Color",
                "upper_right",
                g.upper_right,
                u.upper_right,
            ),
            row("Lower Left Color", "lower_left", g.lower_left, u.lower_left),
            row(
                "Lower Right Color",
                "lower_right",
                g.lower_right,
                u.lower_right,
            ),
        ],
        _ => Vec::new(),
    }
}

enum Act {
    Hold(Key, f64),
    Drag(Key, f64),
    Land(Key),
    Put(Key, f64),
    Edit(Key, f64),
    Release(Key),
    Compress(bool),
    Denoise(bool),
    Direction(Direction),
    SpeedEase(Ease),
    Ripple(bool),
    FullSpeed,
    Retime(ClipRetime),
    Mute(bool),
    PanOn(bool),
    Link(bool),
    Flips([Option<bool>; 2]),
    Zooming(bool),
    Boxes,
    AreaEdit(usize),
    AreaTrack(usize),
    Ease(Ease),
    Mode(CompositeMode),
    Swap,
    Clear(PictureTool),
    Switch(PictureTool, bool),
    Unkey,
    Step(Key, i64),
    Toggle(Key),
    KeyEase(Key, Ease),
    ResetChannel(Field),
    ResetCenter(Axis),
    EffectSet(usize, Value),
    EffectStep(usize, i64),
    EffectKey(usize, Option<Ease>),
    EffectReset(usize),
    EffectRemove(usize),
    EffectMove(usize, usize),
    Tint(usize, [u8; 3], [u8; 3]),
}

fn param_name(key: Key) -> Option<&'static str> {
    match key {
        Key::Volume => Some("volume"),
        Key::Picture(f) => f.channel().map(PictureValue::name),
        Key::Speed => Some("speed"),
        Key::Center(Axis::X) => Some("center_x"),
        Key::Center(Axis::Y) => Some("center_y"),
        _ => None,
    }
}

fn key_clips(key: Key, (chosen, sounds, paced): Lists<'_>) -> Vec<String> {
    match key {
        Key::Volume => sounds.iter().map(|s| s.id.clone()).collect(),
        Key::Picture(_) => chosen.iter().map(|c| c.id.clone()).collect(),
        Key::Center(_) => chosen.iter().take(1).map(|c| c.id.clone()).collect(),
        Key::Speed => paced.iter().map(|c| c.id.clone()).collect(),
        _ => Vec::new(),
    }
}

fn gradient_of(c: &Clip) -> Option<&Gradient> {
    match &c.generator {
        Some(Generator::FourColorGradient(g)) => Some(g),
        _ => None,
    }
}

fn ebu_level_of(c: &Clip) -> Option<EbuLevel> {
    match &c.generator {
        Some(Generator::EbuColorBar { level }) => Some(*level),
        _ => None,
    }
}

fn ebu_level_label(level: EbuLevel) -> &'static str {
    match level {
        EbuLevel::Percent75 => "75% (SMPTE)",
        EbuLevel::Percent100 => "100%",
    }
}

fn key_times(c: &Clip, key: Key) -> Vec<i64> {
    let keys = match key {
        Key::Volume => c.audio.as_ref().map(|a| a.gain_keys.clone()),
        Key::Picture(f) => f
            .channel()
            .and_then(|v| c.picture.as_ref().map(|pic| pic.keys_of(v).to_vec())),
        Key::Center(axis) => gradient_of(c).map(|g| g.keys(axis).to_vec()),
        Key::Speed => c.speed.as_ref().map(Speed::keyframes),
        _ => None,
    };
    keys.unwrap_or_default()
        .iter()
        .map(|k| c.key_time(k.frame).round() as i64)
        .collect()
}

pub(super) fn label(ui: &mut egui::Ui, text: &str, set: bool) {
    let c = theme::colors();
    ui.label(RichText::new(text).color(if set { c.accent } else { c.text_dim }));
}

fn reset(ui: &mut egui::Ui, shown: bool) -> bool {
    let r = ui.add_visible(
        shown,
        egui::Button::new(ARROW_COUNTER_CLOCKWISE).frame(false),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, shown, "Reset"));
    r.clicked()
}

fn number(
    ui: &mut egui::Ui,
    f: Field,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let first = &chosen[0];
    let several = chosen.len() > 1;
    let keyed = f
        .channel()
        .is_some_and(|v| chosen.iter().any(|c| c.stored.has_keys(v)));
    let off = several && keyed;
    let set = !off && (keyed || chosen.iter().any(|c| !f.is_usual(&c.stored)));
    let shown = f.read(&first.shown, &first.shape, Some(first.frame));
    let range = f.range(&first.shape);
    let keys = key_mark(f, p, at, chosen);
    field(
        ui,
        Key::Picture(f),
        i,
        (set, shown, range),
        keyed_look(off),
        keys,
        acts,
    );
}

fn key_mark(f: Field, p: &Project, at: i64, chosen: &[Chosen]) -> Option<KeyMark> {
    f.channel().filter(|_| chosen.len() == 1).and_then(|v| {
        let c = p.clip(&chosen[0].id)?;
        let picture = c.picture.clone().unwrap_or_default();
        Some(mark(c, picture.keys_of(v), at))
    })
}

fn pair_row(
    ui: &mut egui::Ui,
    (name, [a, b], link): (&str, [Field; 2], Option<bool>),
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let first = &chosen[0];
    let keyed = a
        .channel()
        .is_some_and(|v| chosen.iter().any(|c| c.stored.has_keys(v)));
    let look = keyed_look(chosen.len() > 1 && keyed);
    let moved = chosen
        .iter()
        .any(|c| !a.is_usual(&c.stored) || !b.is_usual(&c.stored));
    let set = !look.off && (keyed || moved);
    label(ui, name, set);
    ui.horizontal(|ui| {
        if look.off {
            ui.disable();
        }
        let number_box = |ui: &mut egui::Ui, f: Field, acts: &mut Vec<Act>| {
            let shown = f.read(&first.shown, &first.shape, Some(first.frame));
            let range = f.range(&first.shape);
            drag(ui, Key::Picture(f), i, (shown, range), &look, acts);
        };
        number_box(ui, a, acts);
        if let Some(lit) = link {
            link_button(ui, lit, acts);
        }
        number_box(ui, b, acts);
        if let Some(keys) = key_mark(a, p, at, chosen) {
            key_acts(ui, Key::Picture(a), &keys, acts);
        }
    });
    if reset(ui, set || look.dead_reset) && !look.dead_reset {
        acts.push(Act::ResetChannel(a));
    }
    ui.end_row();
}

fn link_button(ui: &mut egui::Ui, lit: bool, acts: &mut Vec<Act>) {
    let icon = if lit { LINK_SIMPLE } else { LINK_SIMPLE_BREAK };
    let b = ui.add(egui::Button::selectable(lit, icon));
    b.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, lit, "Link Zoom"));
    if b.clicked() {
        acts.push(Act::Link(!lit));
    }
}

fn keyed_look(off: bool) -> Look<'static> {
    Look {
        off,
        why: off.then_some("Select one clip to change its keyframes"),
        dead_reset: off,
        ..Look::default()
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct KeyMark {
    on: bool,
    prev: bool,
    next: bool,
}

pub(super) enum KeyClick {
    Step(i64),
    Toggle,
    Ease(Ease),
}

pub(super) fn key_diamond(on: bool) -> egui::Button<'static> {
    tinted_diamond(on, theme::KEY_ON)
}

fn tinted_diamond(on: bool, color: egui::Color32) -> egui::Button<'static> {
    let icon = RichText::new(DIAMOND);
    let icon = if on { icon.color(color) } else { icon };
    egui::Button::selectable(on, icon).frame(false)
}

pub(super) fn key_buttons(
    ui: &mut egui::Ui,
    keys: &KeyMark,
    eases: &[Ease],
    now: Option<Ease>,
) -> Option<KeyClick> {
    let mut click = None;
    let prev = ui.add_enabled(keys.prev, egui::Button::new(CARET_LEFT).frame(false));
    if prev.clicked() {
        click = Some(KeyClick::Step(-1));
    }
    let diamond = ui.add(key_diamond(keys.on));
    if diamond.clicked() {
        click = Some(KeyClick::Toggle);
    }
    if keys.on && !eases.is_empty() {
        diamond.context_menu(|ui| {
            for &e in eases {
                let text = match now {
                    Some(n) if n == e => format!("{CHECK}  {}", ease_label(e)),
                    Some(_) => format!("     {}", ease_label(e)),
                    None => ease_label(e).to_string(),
                };
                if ui.button(text).clicked() {
                    click = Some(KeyClick::Ease(e));
                }
            }
        });
    }
    let next = ui.add_enabled(keys.next, egui::Button::new(CARET_RIGHT).frame(false));
    if next.clicked() {
        click = Some(KeyClick::Step(1));
    }
    click
}

pub(super) fn next_key(times: impl Iterator<Item = i64>, at: i64, dir: i64) -> Option<i64> {
    if dir < 0 {
        times.filter(|t| *t < at).max()
    } else {
        times.filter(|t| *t > at).min()
    }
}

pub(super) fn mark(c: &Clip, keys: &[Keyframe], at: i64) -> KeyMark {
    let time = |k: &Keyframe| c.key_time(k.frame).round() as i64;
    KeyMark {
        on: keys.iter().any(|k| time(k) == at),
        prev: keys.iter().any(|k| time(k) < at),
        next: keys.iter().any(|k| time(k) > at),
    }
}

fn effect_marks(c: &Clip, at: i64) -> Vec<KeyMark> {
    let effects = c.picture.as_ref().map_or(&[][..], |p| &p.effects);
    effects
        .iter()
        .map(|e| {
            let own = e.keys.iter().filter(|k| !k.value.of_area());
            let keys: Vec<Keyframe> = own.flat_map(|k| k.keys.clone()).collect();
            mark(c, &keys, at)
        })
        .collect()
}

fn speed_mark(c: &Clip, keys: &[Keyframe], at: i64) -> KeyMark {
    let inside: Vec<Keyframe> = keys
        .iter()
        .copied()
        .filter(|k| (c.start..c.end()).contains(&(c.key_time(k.frame).round() as i64)))
        .collect();
    KeyMark {
        on: mark(c, keys, at).on,
        ..mark(c, &inside, at)
    }
}

fn drag(
    ui: &mut egui::Ui,
    f: Key,
    i: &Inspector,
    (shown, range): (f64, RangeInclusive<f64>),
    look: &Look<'_>,
    acts: &mut Vec<Act>,
) {
    let held = i.held.as_ref().filter(|h| h.field == f);
    let mut v = held.map_or(shown, |h| h.value);
    let before = v;
    let (fewest, most) = f.decimals();
    let r = ui
        .push_id(f, |ui| {
            let mut drag = egui::DragValue::new(&mut v)
                .update_while_editing(false)
                .clamp_existing_to_range(false)
                .range(range)
                .speed(f.speed())
                .min_decimals(fewest)
                .max_decimals(most)
                .prefix(f.prefix())
                .suffix(f.suffix())
                .custom_parser(typed)
                .min_size(vec2(FIELD_WIDTH, ui.spacing().interact_size.y));
            if let Some((p, dropped)) = look.timecode {
                let now = p.settings.length_timecode(before.round() as i64);
                drag = drag
                    .custom_formatter(|n, _| p.settings.length_timecode(n.round() as i64))
                    .custom_parser(move |t| frames(p, t, &now, dropped));
            }
            ui.add(drag)
        })
        .inner;
    let r = match look.why {
        Some(why) => r.on_disabled_hover_text(why),
        None => r,
    };
    if r.drag_started() {
        acts.push(Act::Hold(f, before));
    }
    if r.dragged() && r.changed() {
        acts.push(Act::Drag(f, v));
    }
    if r.drag_stopped() || (held.is_some_and(|h| h.dragging) && !r.dragged()) {
        acts.push(Act::Land(f));
    } else if r.changed() && !r.dragged() {
        acts.push(Act::Put(f, v));
    } else if r.has_focus() {
        acts.push(Act::Edit(f, v));
    } else if held.is_some() && !r.dragged() {
        acts.push(Act::Release(f));
    }
}

fn key_acts(ui: &mut egui::Ui, f: Key, keys: &KeyMark, acts: &mut Vec<Act>) {
    let linear = matches!(f, Key::Picture(Field::AnchorX | Field::AnchorY));
    let eases: &[Ease] = if linear { &[] } else { &Ease::ALL };
    match key_buttons(ui, keys, eases, None) {
        Some(KeyClick::Step(dir)) => acts.push(Act::Step(f, dir)),
        Some(KeyClick::Toggle) => acts.push(Act::Toggle(f)),
        Some(KeyClick::Ease(e)) => acts.push(Act::KeyEase(f, e)),
        None => {}
    }
}

fn field(
    ui: &mut egui::Ui,
    f: Key,
    i: &Inspector,
    (set, shown, range): (bool, f64, RangeInclusive<f64>),
    look: Look<'_>,
    keys: Option<KeyMark>,
    acts: &mut Vec<Act>,
) {
    label(ui, f.label(), set);
    field_rest(ui, f, i, (set, shown, range), look, keys, acts);
}

fn field_rest(
    ui: &mut egui::Ui,
    f: Key,
    i: &Inspector,
    (set, shown, range): (bool, f64, RangeInclusive<f64>),
    look: Look<'_>,
    keys: Option<KeyMark>,
    acts: &mut Vec<Act>,
) {
    ui.horizontal(|ui| {
        if look.off {
            ui.disable();
        }
        drag(ui, f, i, (shown, range), &look, acts);
        if let Some(keys) = keys {
            key_acts(ui, f, &keys, acts);
        }
    });
    if reset(ui, set || look.dead_reset) && !look.dead_reset {
        acts.push(match f {
            Key::Volume => Act::Unkey,
            Key::Picture(field) if field.channel().is_some() => Act::ResetChannel(field),
            Key::Center(axis) => Act::ResetCenter(axis),
            f => Act::Put(f, f.usual()),
        });
    }
    ui.end_row();
}

fn group<R>(
    ui: &mut egui::Ui,
    name: &str,
    header: impl FnOnce(&mut egui::Ui) -> R,
    rows: impl FnOnce(&mut egui::Ui),
) -> R {
    switched_group(ui, name, None, header, rows).1
}

pub(super) fn switched_group<R>(
    ui: &mut egui::Ui,
    name: &str,
    switch: Option<bool>,
    header: impl FnOnce(&mut egui::Ui) -> R,
    rows: impl FnOnce(&mut egui::Ui),
) -> (Option<bool>, R) {
    switched_fold(ui, name, switch, header, |ui| {
        egui::Grid::new(("inspector-rows", name))
            .num_columns(3)
            .min_col_width(84.0)
            .spacing([8.0, 6.0])
            .show(ui, rows);
    })
}

pub(super) fn switched_fold<R>(
    ui: &mut egui::Ui,
    name: &str,
    switch: Option<bool>,
    header: impl FnOnce(&mut egui::Ui) -> R,
    body: impl FnOnce(&mut egui::Ui),
) -> (Option<bool>, R) {
    let id = ui.make_persistent_id(("inspector", name));
    let (_, header, _) =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                let flipped = switch.and_then(|now| {
                    let mut on = now;
                    ui.checkbox(&mut on, "").clicked().then_some(on)
                });
                ui.label(RichText::new(name).strong());
                (flipped, header(ui))
            })
            .body(body);
    header.inner
}

fn section(
    ui: &mut egui::Ui,
    (name, tool): (&str, PictureTool),
    (set, switch): (bool, Option<bool>),
    acts: &mut Vec<Act>,
    rows: impl FnOnce(&mut egui::Ui, &mut Vec<Act>),
) {
    let reset = |ui: &mut egui::Ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled(set, egui::Button::new("Reset")).clicked()
        })
        .inner
    };
    let (flipped, cleared) = switched_group(ui, name, switch, reset, |ui| rows(ui, acts));
    if let Some(on) = flipped {
        acts.push(Act::Switch(tool, on));
    }
    if cleared {
        acts.push(Act::Clear(tool));
    }
}

fn transform(
    ui: &mut egui::Ui,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let first = &chosen[0];
    let set = chosen
        .iter()
        .any(|c| uses(&c.shown, PictureTool::Transform));
    section(
        ui,
        ("Transform", PictureTool::Transform),
        (set, Some(!first.shown.transform_off)),
        acts,
        |ui, acts| {
            let [zx, zy] = first.shown.sampled(first.frame as f64).scale();
            let linked = !i.unlinked && zx == zy;
            let zoom = if linked {
                [Field::Zoom, Field::LinkedZoomY]
            } else {
                [Field::ZoomX, Field::ZoomY]
            };
            let rows = [
                ("Zoom", zoom, Some(linked)),
                ("Position", [Field::X, Field::Y], None),
            ];
            for row in rows {
                pair_row(ui, row, i, p, at, chosen, acts);
            }
            number(ui, Field::Rotation, i, p, at, chosen, acts);
            let anchor = ("Anchor Point", [Field::AnchorX, Field::AnchorY], None);
            pair_row(ui, anchor, i, p, at, chosen, acts);
            number(ui, Field::Pitch, i, p, at, chosen, acts);
            number(ui, Field::Yaw, i, p, at, chosen, acts);
            let flipped = chosen
                .iter()
                .any(|c| c.shown.flip_horizontal || c.shown.flip_vertical);
            label(ui, "Flip", flipped);
            ui.horizontal(|ui| {
                let p = &first.shown;
                for (k, icon, text, on) in [
                    (0, FLIP_HORIZONTAL, "Horizontal", p.flip_horizontal),
                    (1, FLIP_VERTICAL, "Vertical", p.flip_vertical),
                ] {
                    let b = egui::Button::selectable(on, format!("{icon} {text}"));
                    if ui.add(b).clicked() {
                        let mut to = [None; 2];
                        to[k] = Some(!on);
                        acts.push(Act::Flips(to));
                    }
                }
            });
            if reset(ui, flipped) {
                acts.push(Act::Flips([Some(false); 2]));
            }
            ui.end_row();
        },
    );
}

fn crop(
    ui: &mut egui::Ui,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let set = chosen.iter().any(|c| uses(&c.shown, PictureTool::Crop));
    let switch = Some(!chosen[0].shown.crop_off);
    section(
        ui,
        ("Crop", PictureTool::Crop),
        (set, switch),
        acts,
        |ui, acts| {
            for f in [
                Field::Left,
                Field::Right,
                Field::Top,
                Field::Bottom,
                Field::Softness,
            ] {
                number(ui, f, i, p, at, chosen, acts);
            }
        },
    );
}

fn composite(
    ui: &mut egui::Ui,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let set = chosen
        .iter()
        .any(|c| uses(&c.shown, PictureTool::Composite));
    let section_name = ("Composite", PictureTool::Composite);
    section(ui, section_name, (set, None), acts, |ui, acts| {
        let now = chosen[0].shown.mode();
        let mixed = chosen
            .iter()
            .any(|c| c.shown.mode() != CompositeMode::Normal);
        label(ui, "Composite Mode", mixed);
        egui::ComboBox::from_id_salt("inspector-mode")
            .selected_text(mode_label(now))
            .show_ui(ui, |ui| {
                for m in CompositeMode::ALL {
                    if ui.selectable_label(m == now, mode_label(m)).clicked() {
                        acts.push(Act::Mode(m));
                    }
                }
            });
        if reset(ui, mixed) {
            acts.push(Act::Mode(CompositeMode::Normal));
        }
        ui.end_row();
        number(ui, Field::Opacity, i, p, at, chosen, acts);
    });
}

fn dynamic_zoom(
    ui: &mut egui::Ui,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    boxes: bool,
    acts: &mut Vec<Act>,
) {
    let zooms: Vec<DynamicZoom> = chosen.iter().filter_map(|c| c.shown.dynamic_zoom).collect();
    let on = !zooms.is_empty();
    let first = chosen[0].shown.dynamic_zoom;
    let section_name = ("Dynamic Zoom", PictureTool::DynamicZoom);
    section(ui, section_name, (on, None), acts, |ui, acts| {
        label(ui, "Dynamic Zoom", on);
        dialog::switch(ui, |ui| {
            if ui
                .add(egui::Button::selectable(first.is_none(), "Off"))
                .clicked()
                && on
            {
                acts.push(Act::Zooming(false));
            }
            if ui
                .add(egui::Button::selectable(first.is_some(), "On"))
                .clicked()
                && zooms.len() < chosen.len()
            {
                acts.push(Act::Zooming(true));
            }
        });
        if reset(ui, on) {
            acts.push(Act::Zooming(false));
        }
        ui.end_row();
        for f in [
            Field::StartZoom,
            Field::StartX,
            Field::StartY,
            Field::EndZoom,
            Field::EndX,
            Field::EndY,
        ] {
            number(ui, f, i, p, at, chosen, acts);
        }
        let eased = zooms.iter().any(|z| z.ease != Ease::Linear);
        label(ui, "Ease", eased);
        ui.add_enabled_ui(first.is_some(), |ui| {
            let now = first.map_or(Ease::Linear, |z| z.ease);
            egui::ComboBox::from_id_salt("inspector-ease")
                .selected_text(ease_label(now))
                .show_ui(ui, |ui| {
                    for e in Ease::ALL {
                        if ui.selectable_label(e == now, ease_label(e)).clicked() {
                            acts.push(Act::Ease(e));
                        }
                    }
                });
        });
        if reset(ui, eased) {
            acts.push(Act::Ease(Ease::Linear));
        }
        ui.end_row();
        label(ui, "", false);
        ui.horizontal(|ui| {
            let show = egui::Button::selectable(boxes, "Show Boxes");
            if ui.add(show).clicked() {
                acts.push(Act::Boxes);
            }
            if ui.add_enabled(on, egui::Button::new("Swap")).clicked() {
                acts.push(Act::Swap);
            }
        });
        ui.end_row();
    });
}

fn rows(ui: &mut egui::Ui, salt: (usize, &str), rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(("inspector-effect-rows", salt))
        .num_columns(3)
        .min_col_width(84.0)
        .spacing([8.0, 6.0])
        .show(ui, rows);
}

fn fold(ui: &mut egui::Ui, (n, name): (usize, &str), body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(name)
        .id_salt(("inspector-effect", n, name))
        .default_open(false)
        .show(ui, |ui| rows(ui, (n, name), body));
}

fn icon(ui: &mut egui::Ui, text: &str, name: &str, on: bool) -> bool {
    ui.add_enabled(on, egui::Button::new(text).frame(false))
        .on_hover_text(name)
        .clicked()
}

fn knob(ui: &mut egui::Ui, i: &Inspector, (n, k): (usize, Knob), e: &Effect, acts: &mut Vec<Act>) {
    let shown = k.read(e);
    let dial = k.dial();
    let values = (shown != dial.usual, shown, dial.range);
    field(
        ui,
        Key::Effect(n, k),
        i,
        values,
        Look::default(),
        None,
        acts,
    );
}

fn check(
    ui: &mut egui::Ui,
    (n, name, now, usual): (usize, &str, bool, bool),
    put: &str,
    acts: &mut Vec<Act>,
) {
    let set = |on: bool| {
        let mut params = json!({});
        params[put] = json!(on);
        Act::EffectSet(n, params)
    };
    label(ui, name, now != usual);
    let mut on = now;
    if ui.checkbox(&mut on, "").clicked() {
        acts.push(set(on));
    }
    if reset(ui, now != usual) {
        acts.push(set(usual));
    }
    ui.end_row();
}

fn border_row(ui: &mut egui::Ui, n: usize, now: Border, acts: &mut Vec<Act>) {
    label(ui, "Border Type", now != Border::default());
    egui::ComboBox::from_id_salt(("inspector-border", n))
        .selected_text(border_label(now))
        .show_ui(ui, |ui| {
            for b in Border::ALL {
                if ui.selectable_label(b == now, border_label(b)).clicked() && b != now {
                    acts.push(Act::EffectSet(n, json!({ "border": b })));
                }
            }
        });
    if reset(ui, now != Border::default()) {
        let b = Border::default();
        acts.push(Act::EffectSet(n, json!({ "border": b })));
    }
    ui.end_row();
}

fn gaussian_rows(
    ui: &mut egui::Ui,
    i: &Inspector,
    (n, e, g): (usize, &Effect, &GaussianBlur),
    acts: &mut Vec<Act>,
) {
    let gaussian = |ui: &mut egui::Ui, k: GaussianKnob, acts: &mut Vec<Act>| {
        knob(ui, i, (n, Knob::Gaussian(k)), e, acts);
    };
    rows(ui, (n, "main"), |ui| {
        label(ui, "Split Horizontal and Vertical", g.split);
        let mut split = g.split;
        if ui.checkbox(&mut split, "").clicked() {
            acts.push(Act::EffectSet(n, json!({ "split": split })));
        }
        if reset(ui, g.split) {
            acts.push(Act::EffectSet(n, json!({ "split": false })));
        }
        ui.end_row();
        let strengths: &[GaussianKnob] = if g.split {
            &[
                GaussianKnob::HorizontalStrength,
                GaussianKnob::VerticalStrength,
            ]
        } else {
            &[GaussianKnob::Strength]
        };
        for &k in strengths {
            gaussian(ui, k, acts);
        }
    });
    fold(ui, (n, "Horizontal Channel Adjustment"), |ui| {
        for c in 0..CHANNELS.len() {
            gaussian(ui, GaussianKnob::Horizontal(c), acts);
        }
    });
    fold(ui, (n, "Vertical Channel Adjustment"), |ui| {
        for c in 0..CHANNELS.len() {
            gaussian(ui, GaussianKnob::Vertical(c), acts);
        }
    });
    fold(ui, (n, "Advanced Controls"), |ui| {
        border_row(ui, n, g.edge(), acts);
    });
}

fn cell_label(s: CellShape) -> &'static str {
    match s {
        CellShape::Square => "Square",
        CellShape::Hexagon => "Hexagon",
        CellShape::Triangle => "Triangle",
    }
}

fn mosaic_rows(
    ui: &mut egui::Ui,
    i: &Inspector,
    (n, e, m): (usize, &Effect, &MosaicBlur),
    acts: &mut Vec<Act>,
) {
    let mosaic = |ui: &mut egui::Ui, k: MosaicKnob, acts: &mut Vec<Act>| {
        knob(ui, i, (n, Knob::Mosaic(k)), e, acts);
    };
    rows(ui, (n, "main"), |ui| {
        mosaic(ui, MosaicKnob::Frequency, acts);
        mosaic(ui, MosaicKnob::Smooth, acts);
        let now = m.shape();
        label(ui, "Cell Shape", now != CellShape::default());
        dialog::switch(ui, |ui| {
            for s in CellShape::ALL {
                if ui.selectable_label(s == now, cell_label(s)).clicked() && s != now {
                    acts.push(Act::EffectSet(n, json!({ "cell_shape": s })));
                }
            }
        });
        if reset(ui, now != CellShape::default()) {
            let s = CellShape::default();
            acts.push(Act::EffectSet(n, json!({ "cell_shape": s })));
        }
        ui.end_row();
        mosaic(ui, MosaicKnob::Aliasing, acts);
    });
    fold(ui, (n, "Advanced Controls"), |ui| {
        for k in [
            MosaicKnob::XOffset,
            MosaicKnob::YOffset,
            MosaicKnob::Aspect,
            MosaicKnob::PositionX,
            MosaicKnob::PositionY,
        ] {
            mosaic(ui, k, acts);
        }
        border_row(ui, n, m.edge(), acts);
        check(
            ui,
            (n, "Antialias Result", m.antialias, false),
            "antialias",
            acts,
        );
        check(
            ui,
            (n, "Blur Alpha", m.blurs_alpha(), true),
            "blur_alpha",
            acts,
        );
        check(
            ui,
            (n, "Whole Cells Only", m.whole_cells, false),
            "whole_cells",
            acts,
        );
    });
}

fn choice_row<T: Copy + PartialEq + serde::Serialize>(
    ui: &mut egui::Ui,
    (n, name, put): (usize, &str, &str),
    (now, usual): (T, T),
    (all, text): (&[T], fn(T) -> &'static str),
    acts: &mut Vec<Act>,
) {
    let set = |v: T| {
        let mut params = json!({});
        params[put] = json!(v);
        Act::EffectSet(n, params)
    };
    label(ui, name, now != usual);
    egui::ComboBox::from_id_salt(("inspector-effect-choice", n, put))
        .selected_text(text(now))
        .show_ui(ui, |ui| {
            for v in all {
                if ui.selectable_label(*v == now, text(*v)).clicked() && *v != now {
                    acts.push(set(*v));
                }
            }
        });
    if reset(ui, now != usual) {
        acts.push(set(usual));
    }
    ui.end_row();
}

fn glow_output_label(o: GlowOutput) -> &'static str {
    match o {
        GlowOutput::ShinyRegions => "Shiny Regions",
        GlowOutput::GlowAlone => "Glow Alone",
        GlowOutput::GlowingImage => "Glowing Image",
    }
}

fn glow_alpha_label(a: GlowAlpha) -> &'static str {
    match a {
        GlowAlpha::MasksSources => "Alpha Masks Sources",
        GlowAlpha::Usual => "Usual Behavior",
        GlowAlpha::IsSource => "Alpha Is Source",
    }
}

fn glow_framing_label(f: GlowFraming) -> &'static str {
    match f {
        GlowFraming::OnlyAsSeen => "Only As Seen",
        GlowFraming::ReflectInCamera => "Reflect In Camera",
    }
}

fn glow_rows(
    ui: &mut egui::Ui,
    i: &Inspector,
    (n, e, g): (usize, &Effect, &Glow),
    acts: &mut Vec<Act>,
) {
    let glow = |ui: &mut egui::Ui, k: GlowKnob, acts: &mut Vec<Act>| {
        knob(ui, i, (n, Knob::Glow(k)), e, acts);
    };
    rows(ui, (n, "main"), |ui| {
        choice_row(
            ui,
            (n, "Select Output", "output"),
            (g.shows(), GlowOutput::default()),
            (&GlowOutput::ALL, glow_output_label),
            acts,
        );
        choice_row(
            ui,
            (n, "Input Alpha", "input_alpha"),
            (g.alpha(), GlowAlpha::default()),
            (&GlowAlpha::ALL, glow_alpha_label),
            acts,
        );
        glow(ui, GlowKnob::Threshold, acts);
        glow(ui, GlowKnob::AlphaThreshold, acts);
    });
    fold(ui, (n, "Shape And Spread"), |ui| {
        glow(ui, GlowKnob::Spread, acts);
        glow(ui, GlowKnob::Ratio, acts);
        label(ui, "Relative Spread", false);
        ui.end_row();
        for c in 0..3 {
            glow(ui, GlowKnob::Relative(c), acts);
        }
    });
    fold(ui, (n, "Color and Composite"), |ui| {
        glow(ui, GlowKnob::Gain, acts);
        glow(ui, GlowKnob::Gamma, acts);
        glow(ui, GlowKnob::Saturation, acts);
        label(ui, "Color Filter", false);
        ui.end_row();
        for c in 0..3 {
            glow(ui, GlowKnob::Filter(c), acts);
        }
    });
    fold(ui, (n, "Secondary Glow"), |ui| {
        check(
            ui,
            (n, "Add Secondary Glow", g.secondary, false),
            "secondary",
            acts,
        );
        glow(ui, GlowKnob::SecondGain, acts);
        glow(ui, GlowKnob::SecondSpread, acts);
        for c in 0..3 {
            glow(ui, GlowKnob::SecondColor(c), acts);
        }
    });
    fold(ui, (n, "Atmosphere"), |ui| {
        check(
            ui,
            (n, "Add Atmosphere", g.atmosphere, false),
            "atmosphere",
            acts,
        );
        for k in [
            GlowKnob::Strength,
            GlowKnob::Detail,
            GlowKnob::Scale,
            GlowKnob::NoiseRatio,
            GlowKnob::Evolution,
            GlowKnob::Seethe,
        ] {
            glow(ui, k, acts);
        }
        check(
            ui,
            (n, "Follow FX Tracker", g.atmosphere_tracked, false),
            "atmosphere_tracked",
            acts,
        );
    });
    fold(ui, (n, "Advanced Controls"), |ui| {
        choice_row(
            ui,
            (n, "Glow Framing", "framing"),
            (g.frames(), GlowFraming::default()),
            (&GlowFraming::ALL, glow_framing_label),
            acts,
        );
        choice_row(
            ui,
            (n, "Composite Type", "composite_type"),
            (g.over(), CompositeMode::Add),
            (&CompositeMode::ALL, mode_label),
            acts,
        );
        glow(ui, GlowKnob::Opacity, acts);
        check(ui, (n, "Invert Effect", g.invert, false), "invert", acts);
        glow(ui, GlowKnob::Recovery, acts);
    });
}

fn grain_preset_label(p: GrainPreset) -> &'static str {
    match p {
        GrainPreset::S1At8 => "8mm S1",
        GrainPreset::D50At8 => "8mm 50D",
        GrainPreset::D100At8 => "8mm 100D",
        GrainPreset::D250At8 => "8mm 250D",
        GrainPreset::T200At8 => "8mm 200T",
        GrainPreset::T500At8 => "8mm 500T",
        GrainPreset::S1At16 => "16mm S1",
        GrainPreset::D50At16 => "16mm 50D",
        GrainPreset::D250At16 => "16mm 250D",
        GrainPreset::T500At16 => "16mm 500T",
        GrainPreset::ArchivalAt16 => "16mm Archival",
        GrainPreset::ArchivalPrintAt16 => "16mm Archival Print",
        GrainPreset::ReversalAt16 => "16mm Reversal",
        GrainPreset::S1At35 => "35mm S1",
        GrainPreset::T200At35 => "35mm 200T",
        GrainPreset::T400At35 => "35mm 400T",
        GrainPreset::ReversalBwAt35 => "35mm Reversal BW",
        GrainPreset::Custom => "Custom",
    }
}

fn grain_composite_label(c: GrainComposite) -> &'static str {
    match c {
        GrainComposite::Custom => "Custom",
        _ => mode_label(c.mode()),
    }
}

fn grain_rows(
    ui: &mut egui::Ui,
    i: &Inspector,
    (n, e, g): (usize, &Effect, &FilmGrain),
    acts: &mut Vec<Act>,
) {
    let grain = |ui: &mut egui::Ui, k: GrainKnob, acts: &mut Vec<Act>| {
        knob(ui, i, (n, Knob::Grain(k)), e, acts);
    };
    rows(ui, (n, "main"), |ui| {
        choice_row(
            ui,
            (n, "Film Grain Presets", "preset"),
            (g.preset(), GrainPreset::S1At16),
            (&GrainPreset::ALL, grain_preset_label),
            acts,
        );
        choice_row(
            ui,
            (n, "Composite Type", "grain_composite"),
            (g.over(), GrainComposite::default()),
            (&GrainComposite::ALL, grain_composite_label),
            acts,
        );
        grain(ui, GrainKnob::Opacity, acts);
        check(
            ui,
            (n, "Grain Only", g.grain_only, false),
            "grain_only",
            acts,
        );
        check(ui, (n, "Freeze", g.freeze, false), "freeze", acts);
    });
    fold(ui, (n, "Grain Params"), |ui| {
        for k in [
            GrainKnob::Texture,
            GrainKnob::Size,
            GrainKnob::Aspect,
            GrainKnob::Strength,
            GrainKnob::Offset,
            GrainKnob::Symmetry,
            GrainKnob::Softness,
            GrainKnob::Saturation,
            GrainKnob::Detail,
        ] {
            grain(ui, k, acts);
        }
    });
    fold(ui, (n, "Advanced Controls"), |ui| {
        for c in 0..3 {
            grain(ui, GrainKnob::Gain(c), acts);
        }
        for t in 0..3 {
            grain(ui, GrainKnob::Tone(t), acts);
        }
        check(
            ui,
            (n, "Animate On Every Refresh", g.animate, false),
            "animate",
            acts,
        );
    });
}

fn vignette_mode_label(m: VignetteMode) -> &'static str {
    match m {
        VignetteMode::Basic => "Basic",
        VignetteMode::Advanced => "Advanced",
    }
}

fn vignette_shape_label(s: VignetteShape) -> &'static str {
    match s {
        VignetteShape::Oval => "Oval",
        VignetteShape::Triangle => "Triangle",
        VignetteShape::Diamond => "Diamond",
        VignetteShape::Rectangle => "Rectangle",
        VignetteShape::Hexagon => "Hexagon",
        VignetteShape::Octagon => "Octagon",
    }
}

fn tint_bytes(color: [f64; 3]) -> [u8; 3] {
    color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn vignette_rows(
    ui: &mut egui::Ui,
    i: &Inspector,
    (n, e, v): (usize, &Effect, &Vignette),
    acts: &mut Vec<Act>,
) {
    let vignette = |ui: &mut egui::Ui, k: VignetteKnob, acts: &mut Vec<Act>| {
        knob(ui, i, (n, Knob::Vignette(k)), e, acts);
    };
    let advanced = v.mode() == VignetteMode::Advanced;
    rows(ui, (n, "main"), |ui| {
        let mode = v.mode();
        label(ui, "Operating Mode", mode != VignetteMode::default());
        dialog::switch(ui, |ui| {
            for m in VignetteMode::ALL {
                if ui
                    .selectable_label(m == mode, vignette_mode_label(m))
                    .clicked()
                    && m != mode
                {
                    acts.push(Act::EffectSet(n, json!({ "operating_mode": m })));
                }
            }
        });
        if reset(ui, mode != VignetteMode::default()) {
            let m = VignetteMode::default();
            acts.push(Act::EffectSet(n, json!({ "operating_mode": m })));
        }
        ui.end_row();
        if advanced {
            let shape = v.shape();
            label(ui, "Border Shape", shape != VignetteShape::default());
            egui::ComboBox::from_id_salt(("inspector-vignette-shape", n))
                .selected_text(vignette_shape_label(shape))
                .show_ui(ui, |ui| {
                    for s in VignetteShape::ALL {
                        let name = vignette_shape_label(s);
                        if ui.selectable_label(s == shape, name).clicked() && s != shape {
                            acts.push(Act::EffectSet(n, json!({ "border_shape": s })));
                        }
                    }
                });
            if reset(ui, shape != VignetteShape::default()) {
                let s = VignetteShape::default();
                acts.push(Act::EffectSet(n, json!({ "border_shape": s })));
            }
            ui.end_row();
        }
        vignette(ui, VignetteKnob::Size, acts);
        if advanced {
            vignette(ui, VignetteKnob::Rotation, acts);
        }
        vignette(ui, VignetteKnob::Anamorphism, acts);
        if advanced {
            vignette(ui, VignetteKnob::CenterX, acts);
            vignette(ui, VignetteKnob::CenterY, acts);
        }
        vignette(ui, VignetteKnob::Softness, acts);
        if advanced {
            vignette(ui, VignetteKnob::Transparency, acts);
        }
        let stored = tint_bytes(v.tint());
        let held = i.tinted.as_ref().filter(|t| t.effect == n);
        let mut rgb = held.map_or(stored, |t| t.to);
        let tinted = v.tint() != Vignette::BLACK;
        label(ui, "Color", tinted);
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            acts.push(Act::Tint(n, stored, rgb));
        }
        if reset(ui, tinted) {
            let black = Vignette::BLACK;
            acts.push(Act::EffectSet(n, json!({ "color": black })));
        }
        ui.end_row();
        if advanced {
            let now = v.mixing();
            label(ui, "Composite Type", now != Vignette::MIXING);
            egui::ComboBox::from_id_salt(("inspector-vignette-mode", n))
                .selected_text(mode_label(now))
                .show_ui(ui, |ui| {
                    for m in CompositeMode::ALL {
                        if ui.selectable_label(m == now, mode_label(m)).clicked() && m != now {
                            acts.push(Act::EffectSet(n, json!({ "composite_type": m })));
                        }
                    }
                });
            if reset(ui, now != Vignette::MIXING) {
                let m = Vignette::MIXING;
                acts.push(Act::EffectSet(n, json!({ "composite_type": m })));
            }
            ui.end_row();
        }
    });
}

fn sharpen_rows(ui: &mut egui::Ui, i: &Inspector, (n, e): (usize, &Effect), acts: &mut Vec<Act>) {
    let sharpen = |ui: &mut egui::Ui, k: SharpenKnob, acts: &mut Vec<Act>| {
        knob(ui, i, (n, Knob::Sharpen(k)), e, acts);
    };
    rows(ui, (n, "main"), |ui| {
        sharpen(ui, SharpenKnob::Amount, acts);
    });
    fold(ui, (n, "Detail Levels"), |ui| {
        sharpen(ui, SharpenKnob::Size, acts);
        sharpen(ui, SharpenKnob::Fine, acts);
        sharpen(ui, SharpenKnob::Medium, acts);
        sharpen(ui, SharpenKnob::Large, acts);
    });
    fold(ui, (n, "Chroma"), |ui| {
        sharpen(ui, SharpenKnob::Chroma, acts);
    });
}

fn path_rows(c: &Clip) -> Vec<PathRow> {
    let row = |m: &MotionPath| PathRow {
        id: m.id.clone(),
        name: m.name.clone(),
        subject: m.subject,
    };
    c.motion.iter().map(row).collect()
}

fn found(c: &Chosen, subject: Subject) -> bool {
    c.paths.iter().any(|p| p.subject == Some(subject))
}

fn find_extra(values: &[String; 7]) -> Option<Value> {
    let mut out = serde_json::Map::new();
    for ((key, _, _, _), text) in FIND_VALUES.iter().zip(values) {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let number: f64 = text.replace(',', ".").parse().ok()?;
        let value = match *key {
            "size" => json!(number.round() as u32),
            _ => json!(number),
        };
        out.insert((*key).to_string(), value);
    }
    Some(Value::Object(out))
}

fn with_extra(mut params: Value, extra: &Value) -> Value {
    for (key, value) in extra.as_object().into_iter().flatten() {
        params[key.as_str()] = value.clone();
    }
    params
}

fn find_request(c: &Chosen, subject: Subject, extra: &Value) -> (&'static str, Value) {
    let mut params = json!({ "clips": [c.id], "subject": subject });
    if found(c, subject) {
        params["replace"] = json!(true);
    }
    ("motion.find", with_extra(params, extra))
}

fn blur_request(
    id: &str,
    found: bool,
    effects: &[Effect],
    (subject, extra): (Subject, &Value),
) -> Option<(&'static str, Value)> {
    let follows =
        |e: &Effect| e.area.as_ref().and_then(|a| a.follows.as_deref()) == Some(subject.name());
    let mut commands = Vec::new();
    if !found {
        let params = with_extra(json!({ "clips": [id], "subject": subject }), extra);
        commands.push(json!({ "method": "motion.find", "params": params }));
    }
    if !effects.iter().any(follows) {
        let area = json!({ "follows": subject.name(), "size": BLUR_SIZE });
        let add = json!({ "clips": [id], "kind": "mosaic_blur" });
        let set = json!({ "clip": id, "effect": effects.len(), "area": area });
        commands.push(json!({ "method": "effects.add", "params": add }));
        commands.push(json!({ "method": "effects.set", "params": set }));
    }
    let label = format!("Blur {}", subject.many());
    (!commands.is_empty()).then(|| ("apply", json!({ "commands": commands, "label": label })))
}

fn finds(method: &str, params: &Value) -> bool {
    let commands = params["commands"].as_array();
    method == "motion.find"
        || commands.is_some_and(|c| c.iter().any(|c| c["method"] == "motion.find"))
}

fn model_needs(subject: Option<Subject>) -> Vec<vidcut_av::models::Need> {
    let dir = crate::paths::models_dir();
    match subject {
        Some(subject) => vidcut_av::subjects::needed(&dir, subject),
        None => vidcut_av::tracking::needed(&dir),
    }
}

fn with_model(method: &str, params: &mut Value) {
    if method == "motion.find" || method == "motion.track" {
        params["download"] = json!(true);
    }
    for c in params["commands"].as_array_mut().into_iter().flatten() {
        if c["method"] == "motion.find" {
            c["params"]["download"] = json!(true);
        }
    }
}

fn area_label(shape: Option<AreaShape>) -> &'static str {
    match shape {
        None => "Off",
        Some(AreaShape::Ellipse) => "Ellipse",
        Some(AreaShape::Box) => "Box",
        Some(AreaShape::Polygon) => "Polygon",
    }
}

fn area_set(n: usize, change: Value) -> Act {
    Act::EffectSet(n, json!({ "area": change }))
}

pub(super) fn all_of(subject: Subject) -> String {
    format!("All {}", subject.many())
}

fn follow_row(
    ui: &mut egui::Ui,
    (n, a): (usize, &EffectArea),
    paths: &[PathRow],
    acts: &mut Vec<Act>,
) {
    let now = a.follows.as_deref();
    let subject = now.and_then(Subject::named);
    let followed = paths.iter().find(|p| Some(p.id.as_str()) == now);
    let shown = match (subject, followed) {
        (Some(s), _) => all_of(s),
        (None, Some(p)) => p.label().to_string(),
        (None, None) => "None".to_string(),
    };
    label(ui, "Follow", now.is_some());
    egui::ComboBox::from_id_salt(("inspector-area-follow", n))
        .selected_text(shown)
        .show_ui(ui, |ui| {
            if ui.selectable_label(now.is_none(), "None").clicked() && now.is_some() {
                acts.push(area_set(n, json!({ "follows": null })));
            }
            for s in SUBJECTS {
                let on = subject == Some(s);
                if ui.selectable_label(on, all_of(s)).clicked() && !on {
                    acts.push(area_set(n, json!({ "follows": s.name() })));
                }
            }
            for p in paths {
                let on = Some(p.id.as_str()) == now;
                if ui.selectable_label(on, p.label()).clicked() && !on {
                    acts.push(area_set(n, json!({ "follows": p.id })));
                }
            }
        });
    if reset(ui, now.is_some()) {
        acts.push(area_set(n, json!({ "follows": null })));
    }
    ui.end_row();
}

fn area_rows(
    ui: &mut egui::Ui,
    i: &Inspector,
    (n, e): (usize, &Effect),
    paths: &[PathRow],
    acts: &mut Vec<Act>,
) {
    let area = |ui: &mut egui::Ui, k: AreaKnob, acts: &mut Vec<Act>| {
        knob(ui, i, (n, Knob::Area(k)), e, acts);
    };
    rows(ui, (n, "area"), |ui| {
        let now = e.area.as_ref().map(|a| a.shape);
        label(ui, "Area", now.is_some());
        let shapes = [
            None,
            Some(AreaShape::Ellipse),
            Some(AreaShape::Box),
            Some(AreaShape::Polygon),
        ];
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("inspector-area-shape", n))
                .selected_text(area_label(now))
                .show_ui(ui, |ui| {
                    for shape in shapes {
                        let name = area_label(shape);
                        if ui.selectable_label(shape == now, name).clicked() && shape != now {
                            let change = shape.map_or(Value::Null, |s| json!({ "shape": s }));
                            acts.push(area_set(n, change));
                        }
                    }
                });
            let edit = egui::Button::selectable(i.area_edit == Some(n), "Edit Shape");
            if ui.add_enabled(now.is_some(), edit).clicked() {
                acts.push(Act::AreaEdit(n));
            }
            if ui
                .add_enabled(now.is_some(), egui::Button::new("Track"))
                .clicked()
            {
                acts.push(Act::AreaTrack(n));
            }
        });
        if reset(ui, now.is_some()) {
            acts.push(area_set(n, Value::Null));
        }
        ui.end_row();
        let Some(a) = &e.area else {
            return;
        };
        label(ui, "Invert Area", a.invert);
        let mut invert = a.invert;
        if ui.checkbox(&mut invert, "").clicked() {
            acts.push(area_set(n, json!({ "invert": invert })));
        }
        if reset(ui, a.invert) {
            acts.push(area_set(n, json!({ "invert": false })));
        }
        ui.end_row();
        area(ui, AreaKnob::Softness, acts);
        follow_row(ui, (n, a), paths, acts);
    });
    if e.area.is_some() {
        fold(ui, (n, "Area Advanced Controls"), |ui| {
            for k in [
                AreaKnob::CenterX,
                AreaKnob::CenterY,
                AreaKnob::Width,
                AreaKnob::Height,
                AreaKnob::Rotation,
            ] {
                area(ui, k, acts);
            }
        });
    }
}

fn effect_block(
    ui: &mut egui::Ui,
    i: &Inspector,
    (n, count): (usize, usize),
    (e, marks, paths): (&Effect, &KeyMark, &[PathRow]),
    acts: &mut Vec<Act>,
) {
    let mut fresh = e.kind().made();
    fresh.on = e.on;
    fresh.area = e.area.clone();
    fresh.keys = e.area_keys().cloned().collect();
    let changed = *e != fresh;
    let header = |ui: &mut egui::Ui| {
        let mut act = None;
        let mut on = e.on;
        let r = ui.checkbox(&mut on, "");
        r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, "On"));
        if r.clicked() {
            act = Some(Act::EffectSet(n, json!({ "on": on })));
        }
        ui.label(RichText::new(e.kind().name()).strong());
        match key_buttons(ui, marks, &Ease::ALL, None) {
            Some(KeyClick::Step(dir)) => act = Some(Act::EffectStep(n, dir)),
            Some(KeyClick::Toggle) => act = Some(Act::EffectKey(n, None)),
            Some(KeyClick::Ease(ease)) => act = Some(Act::EffectKey(n, Some(ease))),
            None => {}
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon(ui, TRASH, "Delete", true) {
                act = Some(Act::EffectRemove(n));
            }
            if ui
                .add_enabled(changed, egui::Button::new("Reset"))
                .clicked()
            {
                act = Some(Act::EffectReset(n));
            }
            if icon(ui, ARROW_DOWN, "Move Down", n + 1 < count) {
                act = Some(Act::EffectMove(n, n + 1));
            }
            if icon(ui, ARROW_UP, "Move Up", n > 0) {
                act = Some(Act::EffectMove(n, n - 1));
            }
        });
        act
    };
    let id = ui.make_persistent_id(("inspector-effect", n));
    let (_, header, _) =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, header)
            .body(|ui| {
                match &e.kind {
                    EffectKind::GaussianBlur(g) => gaussian_rows(ui, i, (n, e, g), acts),
                    EffectKind::MosaicBlur(m) => mosaic_rows(ui, i, (n, e, m), acts),
                    EffectKind::Glow(g) => glow_rows(ui, i, (n, e, g), acts),
                    EffectKind::FilmGrain(g) => grain_rows(ui, i, (n, e, g), acts),
                    EffectKind::Vignette(v) => vignette_rows(ui, i, (n, e, v), acts),
                    EffectKind::Sharpen(_) => sharpen_rows(ui, i, (n, e), acts),
                }
                area_rows(ui, i, (n, e), paths, acts);
                fold(ui, (n, "Global Blend"), |ui| {
                    knob(ui, i, (n, Knob::Blend), e, acts);
                    check(ui, (n, "Use Alpha", e.use_alpha, true), "use_alpha", acts);
                });
            });
    acts.extend(header.inner);
}

fn effects(ui: &mut egui::Ui, i: &Inspector, c: &Chosen, acts: &mut Vec<Act>) {
    let count = c.shown.effects.len();
    for (n, e) in c.shown.effects.iter().enumerate() {
        let shown = e.adopted(c.frame as f64);
        let marks = c.effect_keys.get(n).cloned().unwrap_or_default();
        effect_block(ui, i, (n, count), (&shown, &marks, &c.paths), acts);
    }
}

fn section_speed(c: &Paced, to: f64, ripple: bool) -> Option<(&'static str, Value)> {
    let (from, clips, at) = (c.shown.0, [&c.id], c.at);
    if to == 0.0 {
        let params = json!({ "clips": clips, "at": at, "ripple": ripple });
        return Some(("speed.freeze", params));
    }
    if (to < 0.0) != (from < 0.0) && from != 0.0 {
        let params = json!({ "clips": clips, "at": at, "backward": to < 0.0 });
        return Some(("speed.reverse", params));
    }
    let params = json!({ "clips": clips, "at": at, "speed": to, "ripple": ripple });
    (from > 0.0).then_some(("speed.set", params))
}

fn speed_keys(ui: &mut egui::Ui, (keys, inside): (KeyMark, bool)) -> Option<Act> {
    let mut act = None;
    let prev = ui.add_enabled(keys.prev, egui::Button::new(CARET_LEFT).frame(false));
    if prev.clicked() {
        act = Some(Act::Step(Key::Speed, -1));
    }
    let diamond = tinted_diamond(keys.on, theme::SPEED_KEY_ON);
    if ui.add_enabled(inside || keys.on, diamond).clicked() {
        act = Some(Act::Toggle(Key::Speed));
    }
    let next = ui.add_enabled(keys.next, egui::Button::new(CARET_RIGHT).frame(false));
    if next.clicked() {
        act = Some(Act::Step(Key::Speed, 1));
    }
    act
}

fn speed_section(
    ui: &mut egui::Ui,
    (i, p, dropped): (&Inspector, &Project, i64),
    (paced, at): (&[Paced], i64),
    ripple: bool,
    acts: &mut Vec<Act>,
) {
    let (speed, length) = paced[0].shown;
    let rate = paced[0].rate;
    let frozen = speed == 0.0;
    let ids: Vec<String> = paced.iter().map(|c| c.id.clone()).collect();
    let count = vidcut_core::ops::linked::leads(p, &ids).len();
    let turned = paced.iter().any(|c| c.shown.0 <= 0.0);
    let sped = paced
        .iter()
        .any(|c| c.shown.0 != 0.0 && c.shown.0.abs() != 1.0);
    let ramped = paced.iter().any(|c| c.ramped);
    let stuck = paced.iter().any(|c| c.ramped && c.shown.0 == 0.0);
    let back = paced.iter().any(|c| c.ramped && c.shown.0 < 0.0);
    let keys = p.clip(&paced[0].id).filter(|_| count == 1).map(|c| {
        let keys = c.speed.as_ref().map(Speed::keyframes).unwrap_or_default();
        (speed_mark(c, &keys, at), c.start < at && at < c.end())
    });
    let header = |ui: &mut egui::Ui| {
        let mut act = keys.and_then(|k| speed_keys(ui, k));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let on = turned || sped || ramped;
            if ui.add_enabled(on, egui::Button::new("Reset")).clicked() {
                act = Some(Act::FullSpeed);
            }
            if count > 1 {
                ui.label(dim(format!("{count} clips")));
            }
        });
        act
    };
    let picked = group(ui, "Speed", header, |ui| {
        label(ui, "Direction", turned);
        dialog::switch(ui, |ui| {
            for d in Direction::ALL {
                let on = Direction::of(speed) == d;
                let free = !stuck || d == Direction::Freeze;
                let b = ui
                    .add_enabled(free, egui::Button::selectable(on, d.label()))
                    .on_disabled_hover_text("A freeze between speed keyframes has no direction");
                if b.clicked() && !on {
                    acts.push(Act::Direction(d));
                }
            }
        });
        if reset(ui, turned && !stuck) {
            acts.push(Act::Direction(Direction::Forward));
        }
        ui.end_row();
        let eases = p
            .clip(&paced[0].id)
            .and_then(|c| vidcut_core::ops::speed::section_eases(c, paced[0].at));
        if let Some((now, options)) = eases {
            label(ui, "Ease", !now.is_linear());
            ui.add_enabled_ui(options.len() > 1, |ui| {
                egui::ComboBox::from_id_salt("inspector-speed-ease")
                    .selected_text(ease_label(now))
                    .show_ui(ui, |ui| {
                        for &e in &options {
                            if ui.selectable_label(e == now, ease_label(e)).clicked() && e != now {
                                acts.push(Act::SpeedEase(e));
                            }
                        }
                    });
            });
            if reset(ui, !now.is_linear()) {
                acts.push(Act::SpeedEase(Ease::Linear));
            }
            ui.end_row();
        }
        let moving = Look {
            off: frozen || back,
            why: (back && !frozen).then_some("Only a forward section takes a speed"),
            ..Look::default()
        };
        let (least, most) = (
            *SpeedDialog::PERCENT.start() / 100.0,
            *SpeedDialog::PERCENT.end() / 100.0,
        );
        let typed_range = f64::NEG_INFINITY..=*SpeedDialog::PERCENT.end();
        let percent = (sped, speed.abs() * 100.0, typed_range);
        field(ui, Key::Speed, i, percent, moving, None, acts);
        let fps = (false, speed.abs() * rate, least * rate..=most * rate);
        field(ui, Key::Rate, i, fps, moving, None, acts);
        let span = length as f64 * speed.abs();
        let lengths = (span / most).ceil().max(1.0)..=(span / least).floor().max(1.0);
        let timed = Look {
            off: frozen || !ripple || ramped,
            why: if ramped {
                Some("Off on a clip with speed keyframes")
            } else {
                (!frozen && !ripple).then_some("Needs Ripple Timeline")
            },
            timecode: Some((p, dropped)),
            dead_reset: false,
        };
        field(
            ui,
            Key::Length,
            i,
            (false, length as f64, lengths),
            timed,
            None,
            acts,
        );
        label(ui, "Ripple Timeline", false);
        let mut on = ripple;
        if ui.checkbox(&mut on, "").clicked() {
            acts.push(Act::Ripple(on));
        }
        ui.end_row();
        let (retime, project) = (paced[0].retime, paced[0].project);
        let text = |r| super::retime_process::clip_text(r, project);
        label(ui, "Retime Process", !retime.is_project());
        egui::ComboBox::from_id_salt("inspector-retime")
            .selected_text(text(retime))
            .show_ui(ui, |ui| {
                for r in super::retime_process::PROCESSES {
                    if ui.selectable_label(retime == r, text(r)).clicked() && r != retime {
                        acts.push(Act::Retime(r));
                    }
                }
            });
        if reset(ui, !retime.is_project()) {
            acts.push(Act::Retime(ClipRetime::Project));
        }
        ui.end_row();
    });
    acts.extend(picked);
}

impl Workspace {
    fn chosen(&self) -> Vec<Chosen> {
        let Some(p) = self.project_shared() else {
            return Vec::new();
        };
        let wanted = self.picture_wanted();
        self.snap
            .selection
            .clips
            .iter()
            .filter_map(|id| {
                let (ti, ci) = p.find_clip(id)?;
                let c = &p.tracks[ti].clips[ci];
                (p.tracks[ti].kind == TrackKind::Video && has_picture(c)).then(|| {
                    let stored = c.picture.clone().unwrap_or_default();
                    let shown = wanted
                        .iter()
                        .find(|(w, _)| *w == c.id)
                        .map_or_else(|| stored.clone(), |(_, x)| x.clone());
                    Chosen {
                        id: c.id.clone(),
                        stored,
                        shown,
                        shape: shape(&p, c),
                        frame: c.content_at(self.snap.timeline.position),
                        effect_keys: effect_marks(c, self.snap.timeline.position),
                        paths: path_rows(c),
                        finds: vidcut_core::ops::motion::takes(c),
                    }
                })
            })
            .collect()
    }

    fn inspector_landed(&self) -> Vec<(String, Picture)> {
        let (Some(l), Some(p)) = (&self.inspector.landing, self.project()) else {
            return Vec::new();
        };
        l.pictures
            .iter()
            .filter_map(|(id, from, to)| {
                let stored = p.clip(id)?.picture.clone().unwrap_or_default();
                landing(to, l.since.elapsed(), from, &stored).map(|t| (id.clone(), t.clone()))
            })
            .collect()
    }

    fn sounds(&self) -> Vec<Sound> {
        let Some(p) = self.project() else {
            return Vec::new();
        };
        let gained = self.inspector.gained.as_ref();
        let at = self.snap.timeline.position as f64;
        self.snap
            .selection
            .clips
            .iter()
            .filter_map(|id| {
                let c = p.clip(id)?;
                let a = c.audio.as_ref()?;
                let stored = c.gain_at(at)?;
                let gain = gained
                    .and_then(|g| {
                        let (_, from) = g.from.iter().find(|(f, _)| f == id)?;
                        landing(g.to, g.since.elapsed(), *from, stored)
                    })
                    .unwrap_or(stored);
                Some(Sound {
                    id: c.id.clone(),
                    stored,
                    gain,
                    muted: a.muted,
                    keyed: a.is_keyed(),
                    pan: a.pan,
                    pan_on: a.pan_on,
                })
            })
            .collect()
    }

    fn paced(&self) -> Vec<Paced> {
        let Some(p) = self.project() else {
            return Vec::new();
        };
        let retimed = self.inspector.retimed.as_ref();
        let position = self.snap.timeline.position;
        self.snap
            .selection
            .clips
            .iter()
            .filter_map(|id| {
                let c = p
                    .clip(id)
                    .filter(|c| !c.is_generator() && c.title.is_none())?;
                let at = position.clamp(c.start, c.end() - 1);
                let stored = (c.speed_at(at), c.duration);
                let shown = retimed
                    .and_then(|r| {
                        let (_, from, to) = r.clips.iter().find(|(f, _, _)| f == id)?;
                        landing(*to, r.since.elapsed(), *from, stored)
                    })
                    .unwrap_or(stored);
                Some(Paced {
                    id: c.id.clone(),
                    stored,
                    shown,
                    rate: rate(p, c),
                    retime: c.retime,
                    project: p.media(&c.media).map_or(Retime::Nearest, |m| {
                        vidcut_core::render::retime_at(
                            ClipRetime::Project,
                            c.speed_at(at),
                            m,
                            &p.settings,
                        )
                    }),
                    at,
                    ramped: c.speed.as_ref().is_some_and(|s| s.sections.len() > 1),
                })
            })
            .collect()
    }

    fn ripple(&self) -> bool {
        self.inspector
            .ripple
            .unwrap_or_else(|| self.project().is_some_and(|p| p.settings.ripple_on))
    }

    fn send_speeds(
        &mut self,
        (paced, whole): (&[Paced], bool),
        length: Option<i64>,
        change: impl Fn(&mut SpeedDialog, &Paced),
    ) {
        let ripple = self.ripple();
        let mut groups: Vec<(SpeedDialog, Vec<String>)> = Vec::new();
        let mut sections = Vec::new();
        let mut clips = Vec::new();
        for c in paced {
            let mut d = SpeedDialog::of(c.shown.0, ripple);
            change(&mut d, c);
            let to = d.speed();
            let section = c.ramped && !whole;
            if (to - c.shown.0).abs() < SAME && (section || !c.ramped) {
                continue;
            }
            let (from, frames) = c.shown;
            let length = length.unwrap_or(if ripple && to != 0.0 && from != 0.0 && !c.ramped {
                ((frames as f64 * from.abs() / to.abs()) + 1e-6)
                    .floor()
                    .max(1.0) as i64
            } else {
                frames
            });
            if section {
                let Some(command) = section_speed(c, to, ripple) else {
                    continue;
                };
                sections.push(command);
            } else {
                match groups.iter_mut().find(|(g, _)| g.speed() == to) {
                    Some((_, ids)) => ids.push(c.id.clone()),
                    None => groups.push((d, vec![c.id.clone()])),
                }
            }
            clips.push((c.id.clone(), c.stored, (to, length)));
        }
        let mut commands: Vec<(&'static str, Value)> = groups
            .iter()
            .map(|(d, ids)| ("edit.change_speed", d.params(ids)))
            .chain(sections)
            .collect();
        match commands.len() {
            0 => return,
            1 => {
                let (method, params) = commands.remove(0);
                self.send_asking(method, params);
            }
            _ => {
                let commands: Vec<Value> = commands
                    .into_iter()
                    .map(|(m, p)| json!({ "method": m, "params": p }))
                    .collect();
                self.send_asking("apply", json!({ "commands": commands }));
            }
        }
        self.inspector.retimed = Some(Retimed {
            since: Instant::now(),
            clips,
        });
    }

    fn ease_sections(&mut self, paced: &[Paced], ease: Ease) {
        let mut commands: Vec<Value> = paced
            .iter()
            .filter(|c| c.ramped)
            .map(|c| {
                let params = json!({ "clips": [c.id], "at": c.at, "ease": ease });
                json!({ "method": "speed.ease", "params": params })
            })
            .collect();
        match commands.len() {
            0 => {}
            1 => {
                let c = commands.remove(0);
                self.send("speed.ease", c["params"].clone());
            }
            _ => self.send("apply", json!({ "commands": commands })),
        }
    }

    fn send_freeze(&mut self, paced: &[Paced], speed: f64) {
        let clips: Vec<&String> = paced.iter().map(|c| &c.id).collect();
        let at = self.snap.timeline.position;
        self.send(
            "speed.set",
            json!({ "clips": clips, "at": at, "speed": speed }),
        );
    }

    fn send_percent(
        &mut self,
        paced: &[Paced],
        length: Option<i64>,
        percent: impl Fn(&Paced) -> f64,
    ) {
        self.send_speeds((paced, false), length, |d, c| {
            if !d.freeze {
                d.set_percent(percent(c));
            }
        });
    }

    pub(super) fn inspector_wanted(&self) -> Vec<(String, Picture)> {
        match self.inspector.held.as_ref().filter(|h| h.dragging) {
            Some(Held {
                field: Key::Picture(f),
                value,
                start,
                ..
            }) => start
                .iter()
                .map(|c| {
                    (
                        c.id.clone(),
                        f.put_at(&c.shown, &c.shape, *value, Some(c.frame)),
                    )
                })
                .collect(),
            Some(Held {
                field: Key::Effect(n, k),
                value,
                start,
                ..
            }) => start
                .first()
                .and_then(|c| Some((c.id.clone(), knob_picture(c, (*n, *k), *value)?)))
                .into_iter()
                .collect(),
            _ => self.inspector_landed(),
        }
    }

    fn send_knob(&mut self, chosen: &[Chosen], (n, k): (usize, Knob), value: f64) {
        let Some(c) = chosen.first() else {
            return;
        };
        let Some(to) = knob_picture(c, (n, k), value).filter(|p| *p != c.shown) else {
            return;
        };
        let at_frame = c.shown.effects[n].adopted(c.frame as f64);
        let (name, v) = k.param(&k.put(&at_frame, value));
        let mut params = json!({ "clip": c.id, "effect": n });
        params[name] = v;
        if c.stored.effects.get(n).is_some_and(Effect::has_keys) {
            params["at"] = json!(self.snap.timeline.position);
        }
        self.send("effects.set", params);
        self.inspector.landing = Some(Landing {
            since: Instant::now(),
            pictures: vec![(c.id.clone(), c.stored.clone(), to)],
        });
    }

    fn send_effect(&mut self, method: &str, chosen: &[Chosen], n: usize, mut params: Value) {
        let Some(c) = chosen.first() else {
            return;
        };
        params["clip"] = json!(c.id);
        params["effect"] = json!(n);
        if method == "effects.set" && c.stored.effects.get(n).is_some_and(Effect::has_keys) {
            params["at"] = json!(self.snap.timeline.position);
        }
        self.send(method, params);
    }

    fn step_effect(&mut self, n: usize, dir: i64, chosen: &[Chosen]) {
        let at = self.snap.timeline.position;
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(c) = chosen.first().and_then(|c| p.clip(&c.id)) else {
            return;
        };
        if let Some(t) = next_key(effect_key_times(c, n).into_iter(), at, dir) {
            self.seek(PlayerId::Timeline, t, false);
        }
    }

    fn key_effect(&mut self, n: usize, ease: Option<Ease>, chosen: &[Chosen]) {
        let at = self.snap.timeline.position;
        let params = match ease {
            Some(ease) => json!({ "at": at, "ease": ease }),
            None => json!({ "at": at }),
        };
        self.send_effect("effects.key", chosen, n, params);
    }

    pub(super) fn settle_inspector(&mut self) {
        if self.inspector.landing.is_some() && self.inspector_landed().is_empty() {
            self.inspector.landing = None;
        }
    }

    pub(super) fn leave_inspector(&mut self) {
        self.inspector.held = None;
        self.inspector.seen = None;
        self.close_forms();
    }

    pub(super) fn close_forms(&mut self) {
        self.close_title_form();
        self.close_cue_form();
        self.close_track_form();
    }

    fn send_pictures(&mut self, chosen: &[Chosen], to: impl Fn(&Chosen) -> Option<Picture>) {
        let list: Vec<(String, Picture, Picture)> = chosen
            .iter()
            .filter_map(|c| {
                let frame = c.frame as f64;
                let now = Chosen {
                    stored: c.stored.sampled(frame),
                    shown: c.shown.sampled(frame),
                    ..c.clone()
                };
                let p = to(&now).filter(|p| *p != now.shown)?;
                Some((now.id, now.stored, p))
            })
            .collect();
        let at = self.snap.timeline.position;
        let params: Vec<Value> = list
            .iter()
            .filter_map(|(id, from, to)| {
                changed(id, from, to).map(|mut p| {
                    p["at"] = json!(at);
                    p
                })
            })
            .collect();
        if params.is_empty() {
            return;
        }
        self.end_picture_wheel();
        match merged(&params) {
            Some(one) => self.send("picture.set", one),
            None => {
                let commands: Vec<Value> = params
                    .into_iter()
                    .map(|p| json!({ "method": "picture.set", "params": p }))
                    .collect();
                self.send("apply", json!({ "commands": commands }));
            }
        }
        self.inspector.landing = Some(Landing {
            since: Instant::now(),
            pictures: list,
        });
    }

    fn clear_pictures(&mut self, chosen: &[Chosen], tool: PictureTool) {
        let used: Vec<&Chosen> = chosen.iter().filter(|c| uses(&c.shown, tool)).collect();
        if used.is_empty() {
            return;
        }
        let ids: Vec<&str> = used.iter().map(|c| c.id.as_str()).collect();
        self.end_picture_wheel();
        self.timeline.picture_grab.cancel_landing();
        self.send("picture.clear", json!({ "clips": ids, "tool": tool }));
        self.inspector.landing = Some(Landing {
            since: Instant::now(),
            pictures: used
                .iter()
                .map(|c| (c.id.clone(), c.stored.clone(), cleared(&c.shown, tool)))
                .collect(),
        });
    }

    fn send_volume(&mut self, sounds: &[Sound], to: f64) {
        let to = to.clamp(*GAIN.start(), *GAIN.end());
        let from: Vec<(String, f64)> = sounds
            .iter()
            .filter(|s| s.gain != to)
            .map(|s| (s.id.clone(), s.stored))
            .collect();
        if from.is_empty() {
            return;
        }
        let ids: Vec<&str> = from.iter().map(|(id, _)| id.as_str()).collect();
        let at = self.snap.timeline.position;
        self.send("clip.set", json!({ "clips": ids, "gain_db": to, "at": at }));
        self.inspector.gained = Some(Gained {
            since: Instant::now(),
            from,
            to,
        });
    }

    fn send_pan(&mut self, sounds: &[Sound], to: f64) {
        let to = to.clamp(*PAN.start(), *PAN.end());
        let ids: Vec<&str> = sounds
            .iter()
            .filter(|s| s.pan != to)
            .map(|s| s.id.as_str())
            .collect();
        if !ids.is_empty() {
            self.send("clip.set", json!({ "clips": ids, "pan": to }));
        }
    }

    fn step_key(&mut self, key: Key, dir: i64, lists: Lists<'_>) {
        let at = self.snap.timeline.position;
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(c) = key_clips(key, lists).first().and_then(|id| p.clip(id)) else {
            return;
        };
        let times = key_times(c, key)
            .into_iter()
            .filter(|t| key != Key::Speed || (c.start..c.end()).contains(t));
        if let Some(t) = next_key(times, at, dir) {
            self.seek(PlayerId::Timeline, t, false);
        }
    }

    fn toggle_key(&mut self, key: Key, lists: Lists<'_>) {
        let Some(param) = param_name(key) else {
            return;
        };
        let at = self.snap.timeline.position;
        let Some(p) = self.project_shared() else {
            return;
        };
        let ids = key_clips(key, lists);
        let times: Vec<(&str, Vec<i64>)> = ids
            .iter()
            .filter_map(|id| Some((id.as_str(), key_times(p.clip(id)?, key))))
            .collect();
        if times.iter().any(|(_, t)| t.contains(&at)) {
            let keyed: Vec<&str> = times
                .iter()
                .filter(|(_, t)| !t.is_empty())
                .map(|(id, _)| *id)
                .collect();
            self.send(
                "keyframes.remove",
                json!({ "clips": keyed, "param": param, "at": at }),
            );
        } else if !ids.is_empty() {
            self.send(
                "keyframes.add",
                json!({ "clips": ids, "param": param, "at": at }),
            );
        }
    }

    fn ease_key(&mut self, key: Key, ease: Ease, chosen: &[Chosen], sounds: &[Sound]) {
        let Some(param) = param_name(key) else {
            return;
        };
        let at = self.snap.timeline.position;
        let Some(p) = self.project_shared() else {
            return;
        };
        let ids: Vec<&str> = match key {
            Key::Volume => sounds.iter().map(|s| s.id.as_str()).collect(),
            Key::Picture(_) => chosen.iter().map(|c| c.id.as_str()).collect(),
            Key::Center(_) => chosen.iter().take(1).map(|c| c.id.as_str()).collect(),
            _ => return,
        };
        let ids: Vec<&str> = ids
            .into_iter()
            .filter(|id| p.clip(id).is_some_and(|c| key_times(c, key).contains(&at)))
            .collect();
        if !ids.is_empty() {
            self.send(
                "keyframes.set",
                json!({ "clips": ids, "param": param, "at": at, "ease": ease }),
            );
        }
    }

    fn reset_channel(&mut self, field: Field, chosen: &[Chosen]) {
        let Some(v) = field.channel() else {
            return;
        };
        if v == PictureValue::Zoom {
            self.inspector.unlinked = false;
        }
        let keyed: Vec<&str> = chosen
            .iter()
            .filter(|c| c.stored.has_keys(v))
            .map(|c| c.id.as_str())
            .collect();
        let at = self.snap.timeline.position;
        let sets: Vec<Value> = chosen
            .iter()
            .filter_map(|c| {
                let mut plain = c.stored.clone();
                plain.clear_channel(v);
                let mut base = plain.clone();
                let now = c.stored.channel_at(v, c.frame as f64);
                vidcut_core::keyframes::write_channel(&mut base, v, now);
                let to = field.axes().iter().fold(base, |b, f| {
                    f.put_at(&b, &c.shape, f.usual(), Some(c.frame))
                });
                changed(&c.id, &plain, &to).map(|mut p| {
                    p["at"] = json!(at);
                    p
                })
            })
            .collect();
        let mut commands = Vec::new();
        if !keyed.is_empty() {
            commands.push(json!({
                "method": "keyframes.remove",
                "params": { "clips": keyed, "param": v.name() }
            }));
        }
        for params in sets {
            commands.push(json!({ "method": "picture.set", "params": params }));
        }
        match commands.len() {
            0 => {}
            1 => {
                let one = commands.remove(0);
                self.send(
                    one["method"].as_str().unwrap_or("picture.set"),
                    one["params"].clone(),
                );
            }
            _ => self.send("apply", json!({ "commands": commands })),
        }
    }

    fn put(&mut self, key: Key, value: f64, (chosen, sounds, paced): Lists<'_>) {
        match key {
            Key::Picture(f) => {
                self.send_pictures(chosen, |c| {
                    Some(f.put_at(&c.shown, &c.shape, value, Some(c.frame)))
                });
            }
            Key::Effect(n, k) => self.send_knob(chosen, (n, k), value),
            Key::Center(axis) => self.send_center(axis, value, chosen),
            Key::Volume => self.send_volume(sounds, value),
            Key::Pan => self.send_pan(sounds, value),
            Key::Speed if Speed::freezes(value / 100.0) && !paced.iter().any(|c| c.ramped) => {
                self.send_freeze(paced, value / 100.0);
            }
            Key::Speed => self.send_percent(paced, None, |_| value),
            Key::Rate => self.send_percent(paced, None, |c| value / c.rate * 100.0),
            Key::Length => {
                let frames = value.round().max(1.0);
                self.send_percent(paced, Some(frames as i64), |c| {
                    c.shown.1 as f64 * c.shown.0.abs() / frames * 100.0
                });
            }
        }
    }

    fn send_center(&mut self, axis: Axis, value: f64, chosen: &[Chosen]) {
        let (Some(c), Some(param)) = (chosen.first(), param_name(Key::Center(axis))) else {
            return;
        };
        let at = self.snap.timeline.position;
        let mut params = json!({ "clip": c.id, "at": at });
        params[param] = json!(value);
        self.send("generators.set", params);
    }

    fn reset_center(&mut self, axis: Axis, chosen: &[Chosen]) {
        let key = Key::Center(axis);
        let (Some(c), Some(param)) = (chosen.first(), param_name(key)) else {
            return;
        };
        let mut plain = json!({ "clip": c.id });
        plain[param] = json!(key.usual());
        let keyed = self
            .project_shared()
            .and_then(|p| p.clip(&c.id).map(|clip| !key_times(clip, key).is_empty()));
        if keyed == Some(true) {
            let commands = json!([
                { "method": "keyframes.remove", "params": { "clips": [c.id], "param": param } },
                { "method": "generators.set", "params": plain },
            ]);
            self.send("apply", json!({ "commands": commands }));
        } else {
            self.send("generators.set", plain);
        }
    }

    fn generator_section(&mut self, ui: &mut egui::Ui, c: &Clip, acts: &mut Vec<Act>) {
        let rows = color_rows(c);
        let gradient = gradient_of(c);
        let level = ebu_level_of(c);
        if rows.is_empty() && gradient.is_none() && level.is_none() {
            return;
        }
        let mut new_level = None;
        let stored_of = |param: &str| rows.iter().find(|r| r.param == param).map(|r| r.stored);
        let mut held = self.inspector.recolored.take().filter(|r| {
            r.clip == c.id
                && stored_of(r.param).is_some_and(|stored| {
                    r.since
                        .is_none_or(|s| landing(r.to, s.elapsed(), r.from, stored).is_some())
                })
        });
        let at = self.snap.timeline.position;
        let inspector = &self.inspector;
        group(
            ui,
            "Generator",
            |_| (),
            |ui| {
                if let Some(now) = level {
                    label(ui, "Level", now != EbuLevel::default());
                    egui::ComboBox::from_id_salt("inspector-ebu-level")
                        .selected_text(ebu_level_label(now))
                        .show_ui(ui, |ui| {
                            for l in EbuLevel::ALL {
                                if ui.selectable_label(l == now, ebu_level_label(l)).clicked()
                                    && l != now
                                {
                                    new_level = Some(l);
                                }
                            }
                        });
                    ui.end_row();
                }
                for axis in [Axis::X, Axis::Y] {
                    let Some(g) = gradient else {
                        break;
                    };
                    let key = Key::Center(axis);
                    let shown = g.center_at(axis, c.content_at(at) as f64);
                    let keys = g.keys(axis);
                    let set = !keys.is_empty() || shown != key.usual();
                    let value = (set, shown, *CENTER.start()..=*CENTER.end());
                    let marks = Some(mark(c, keys, at));
                    field(ui, key, inspector, value, keyed_look(false), marks, acts);
                }
                for row in &rows {
                    let mine = held.as_ref().filter(|r| r.param == row.param);
                    let mut rgb = mine.map_or(row.stored, |r| r.to);
                    label(ui, row.label, rgb != row.usual);
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        held = Some(Recolored {
                            clip: c.id.clone(),
                            param: row.param,
                            from: row.stored,
                            to: rgb,
                            since: None,
                        });
                    }
                    ui.end_row();
                }
            },
        );
        if let Some(r) = held.as_mut().filter(|r| r.since.is_none())
            && !ui.input(|i| i.pointer.any_down())
        {
            let mut params = json!({ "clip": c.id });
            params[r.param] = json!(vidcut_core::model::hex(r.to));
            self.send("generators.set", params);
            r.since = Some(Instant::now());
        }
        self.inspector.recolored = held;
        if let Some(l) = new_level {
            self.send("generators.set", json!({ "clip": c.id, "level": l }));
        }
    }

    pub(super) fn lut_folder(&self, now: Option<&Path>) -> PathBuf {
        now.and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.project_folder())
            .unwrap_or_default()
    }

    pub(super) fn input_lut_picked(&mut self) {
        let Some((media, picked)) = self.inspector.lut_pick.take() else {
            return;
        };
        match picked.take() {
            None => self.inspector.lut_pick = Some((media, picked)),
            Some(None) => {}
            Some(Some(path)) => {
                self.send("media.set", json!({ "media": media, "input_lut": path }));
            }
        }
    }

    pub(super) fn ask_input_lut(
        &mut self,
        ctx: &egui::Context,
        media: Vec<String>,
        now: Option<&Path>,
    ) {
        let (dialogs, folder) = (self.dialogs.clone(), self.lut_folder(now));
        let picked = Picked::ask(ctx, move || {
            dialogs.pick_file("Input LUT", &folder, &["cube"])
        });
        self.inspector.lut_pick = Some((media, picked));
    }

    fn input_lut_section(&mut self, ui: &mut egui::Ui, p: &Project, chosen: &[Chosen]) {
        let mut media: Vec<String> = chosen
            .iter()
            .filter_map(|c| p.clip(&c.id))
            .filter(|c| c.video && !c.media.is_empty())
            .map(|c| c.media.clone())
            .collect();
        media.sort_unstable();
        media.dedup();
        let Some(first) = media.first().and_then(|id| p.media(id)) else {
            return;
        };
        let now = first.input_lut.as_deref();
        let any = media
            .iter()
            .any(|id| p.media(id).is_some_and(|m| m.input_lut.is_some()));
        let mut choose = false;
        let header = |ui: &mut egui::Ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled(any, egui::Button::new("Reset")).clicked()
            })
            .inner
        };
        let cleared = group(ui, "LUT", header, |ui| {
            label(ui, "Input LUT", any);
            ui.horizontal(|ui| {
                choose = ui.button("Choose…").clicked();
                let name = now
                    .and_then(Path::file_name)
                    .map_or("None".into(), |n| n.to_string_lossy().into_owned());
                let shown = ui.add(egui::Label::new(name).truncate());
                if let Some(path) = now {
                    shown.on_hover_text(path.display().to_string());
                }
            });
            ui.end_row();
        });
        if cleared {
            self.send("media.set", json!({ "media": media, "input_lut": "" }));
        }
        if choose {
            self.ask_input_lut(ui.ctx(), media, now);
        }
    }

    pub(super) fn subject_blur(
        &self,
        clip: &str,
        subject: Subject,
    ) -> Option<(&'static str, Value)> {
        let p = self.project_shared()?;
        let c = p.clip(clip)?;
        if !vidcut_core::ops::motion::takes(c) {
            return None;
        }
        let found = c.motion.iter().any(|m| m.subject == Some(subject));
        let effects = c.picture.as_ref().map_or(&[][..], |x| &x.effects);
        blur_request(clip, found, effects, (subject, &Value::Null))
    }

    pub(super) fn find(&mut self, method: &'static str, params: Value, subject: Subject) {
        if finds(method, &params) && !model_needs(Some(subject)).is_empty() {
            self.inspector.finding = Some(Finding {
                method,
                params,
                subject: Some(subject),
            });
        } else {
            self.send(method, params);
        }
    }

    pub(super) fn track(&mut self, clip: &str, effect: usize) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(c) = p.clip(clip) else {
            return;
        };
        let effects = c.picture.iter().flat_map(|x| &x.effects);
        let area = effects
            .into_iter()
            .nth(effect)
            .and_then(|e| e.area.as_ref());
        let follows = area.and_then(|a| a.follows.as_ref());
        let at = self.snap.timeline.position;
        let followed = c.motion.iter().find(|m| Some(&m.id) == follows);
        let first = followed.and_then(|m| m.span()).map(|(first, _)| first);
        let way = match first {
            None => "both",
            Some(first) if c.source_at(at) <= first => "backward",
            Some(_) => "forward",
        };
        let params = json!({ "clip": clip, "effect": effect, "at": at, "way": way });
        if model_needs(None).is_empty() {
            self.send("motion.track", params);
        } else {
            self.inspector.finding = Some(Finding {
                method: "motion.track",
                params,
                subject: None,
            });
        }
    }

    pub(super) fn motion_prompt(&mut self, ctx: &egui::Context) {
        let Some(finding) = self.inspector.finding.clone() else {
            return;
        };
        let (method, mut params) = (finding.method, finding.params);
        let needs = model_needs(finding.subject);
        if needs.is_empty() {
            self.inspector.finding = None;
            self.send(method, params);
            return;
        }
        match super::speech::ask_download(ctx, "motion-download", &needs, "Download") {
            Some(true) => {
                self.inspector.finding = None;
                with_model(method, &mut params);
                self.send(method, params);
            }
            Some(false) => self.inspector.finding = None,
            None => {}
        }
    }

    fn motion_section(&mut self, ui: &mut egui::Ui, c: &Chosen) {
        if !c.finds {
            return;
        }
        let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let mut naming = self.inspector.naming.take();
        let mut subject = self.inspector.subject;
        let mut values = std::mem::take(&mut self.inspector.find_values);
        let mut open = self.inspector.find_open;
        let mut better = self.inspector.find_better;
        let wrong = self.inspector.find_error;
        let extra = find_extra(&values);
        let mut tried = false;
        let mut sends = Vec::new();
        let mut asked = None;
        group(
            ui,
            "Motion",
            |_| (),
            |ui| {
                label(ui, "Subject", false);
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("inspector-motion-subject")
                        .selected_text(subject.many())
                        .show_ui(ui, |ui| {
                            for s in Subject::ALL {
                                ui.selectable_value(&mut subject, s, s.many());
                            }
                        });
                    let mut given = extra.clone().unwrap_or(Value::Null);
                    if better && given.is_object() && given.get("size").is_none() {
                        given["size"] = json!(BETTER_SIDE);
                    }
                    if ui.button("Find").clicked() {
                        tried = true;
                        asked = Some(find_request(c, subject, &given));
                    }
                    let had = found(c, subject);
                    let blur = blur_request(&c.id, had, &c.stored.effects, (subject, &given));
                    let button = egui::Button::new("Blur");
                    if ui.add_enabled(blur.is_some(), button).clicked() {
                        tried = true;
                        asked = blur;
                    }
                    if ui.add(egui::Button::selectable(open, "Advanced")).clicked() {
                        open = !open;
                    }
                });
                ui.end_row();
                label(ui, "Quality", false);
                dialog::switch(ui, |ui| {
                    let quick = ui.selectable_label(!better, "Faster");
                    if quick
                        .on_hover_text("The model looks at a small picture. It is quick.")
                        .clicked()
                    {
                        better = false;
                    }
                    let slow = ui.selectable_label(better, "Better");
                    let tip = [
                        "The model looks at a larger picture.",
                        "It finds smaller things and takes much longer.",
                    ]
                    .join("\n");
                    if slow.on_hover_text(tip.as_str()).clicked() {
                        better = true;
                    }
                });
                ui.end_row();
                if open {
                    let usual = vidcut_core::motion::Join::of(subject);
                    let side = match better {
                        true => BETTER_SIDE,
                        false => vidcut_av::subjects::side(subject),
                    };
                    let hints = [
                        side.to_string(),
                        usual.start.to_string(),
                        usual.keep.to_string(),
                        usual.overlap.to_string(),
                        usual.wait_secs.to_string(),
                        usual.shortest_secs.to_string(),
                        usual.steady.to_string(),
                    ];
                    for (i, (_, name, what, effect)) in FIND_VALUES.iter().enumerate() {
                        let tip = format!("{what}\n{effect}");
                        let set = !values[i].trim().is_empty();
                        let c = theme::colors();
                        let color = if set { c.accent } else { c.text_dim };
                        ui.label(RichText::new(*name).color(color))
                            .on_hover_text(tip.as_str());
                        ui.add(
                            egui::TextEdit::singleline(&mut values[i])
                                .hint_text(hints[i].as_str())
                                .desired_width(NAME_WIDTH),
                        )
                        .on_hover_text(tip.as_str());
                        ui.end_row();
                    }
                    if wrong {
                        label(ui, "", false);
                        label(ui, "A Value Is Not A Number", true);
                        ui.end_row();
                    }
                }
                for (n, row) in c.paths.iter().enumerate() {
                    let now = row.name.as_deref().unwrap_or("");
                    let typed = naming
                        .as_ref()
                        .filter(|n| n.clip == c.id && n.path == row.id)
                        .map(|n| n.text.clone());
                    let mut text = typed.clone().unwrap_or_else(|| now.to_string());
                    label(ui, if n == 0 { "Found" } else { "" }, false);
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .hint_text("Name")
                            .desired_width(NAME_WIDTH),
                    );
                    if field.lost_focus() && !cancelled && text.trim() != now {
                        let params = json!({ "clip": c.id, "path": row.id, "name": text });
                        sends.push(("motion.set", params));
                    }
                    if field.has_focus() {
                        naming = Some(Naming {
                            clip: c.id.clone(),
                            path: row.id.clone(),
                            text,
                        });
                    } else if typed.is_some() {
                        naming = None;
                    }
                    if icon(ui, TRASH, "Delete", true) {
                        let params = json!({ "clip": c.id, "paths": [row.id] });
                        sends.push(("motion.remove", params));
                    }
                    ui.end_row();
                }
            },
        );
        self.inspector.naming = naming;
        self.inspector.subject = subject;
        self.inspector.find_values = values;
        self.inspector.find_open = open;
        self.inspector.find_better = better;
        if tried || extra.is_some() {
            self.inspector.find_error = extra.is_none();
        }
        if extra.is_none() {
            asked = None;
        }
        for (method, params) in sends {
            self.send(method, params);
        }
        if let Some((method, params)) = asked {
            self.find(method, params, subject);
        }
    }

    fn super_scale_section(&mut self, ui: &mut egui::Ui, p: &Project, chosen: &[Chosen]) {
        use super::super_scale::{SHARPNESS, UPSCALERS, label_of, unfit, upscaler_text};
        let mut media: Vec<String> = chosen
            .iter()
            .filter_map(|c| p.clip(&c.id))
            .map(|c| c.media.clone())
            .collect();
        media.sort_unstable();
        media.dedup();
        let Some(now) = media
            .first()
            .and_then(|id| p.media(id))
            .map(|m| m.super_scale)
        else {
            return;
        };
        let mut put = None;
        let header = |ui: &mut egui::Ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled(!now.is_off(), egui::Button::new("Reset"))
                    .clicked()
            })
            .inner
        };
        let shown = vidcut_core::render::super_scale_of(now, &p.settings);
        let cleared = group(ui, "Super Scale", header, |ui| {
            let on = !now.is_off();
            label(ui, "Super Scale", on);
            dialog::switch(ui, |ui| {
                for (value, text) in [(false, "Off"), (true, "On")] {
                    if ui.selectable_label(on == value, text).clicked() && on != value {
                        put = Some(json!({ "super_scale": value }));
                    }
                }
            });
            ui.end_row();
            label(ui, "Upscaler", on);
            ui.add_enabled_ui(on, |ui| {
                let project = p.settings.upscaler;
                let text = |u, l| upscaler_text(u, l, project);
                egui::ComboBox::from_id_salt("inspector-upscaler")
                    .selected_text(text(now.upscaler, label_of(UPSCALERS, now.upscaler)))
                    .show_ui(ui, |ui| {
                        for &(upscaler, l) in UPSCALERS {
                            if ui
                                .selectable_label(now.upscaler == upscaler, text(upscaler, l))
                                .clicked()
                            {
                                put = Some(json!({ "upscaler": upscaler }));
                            }
                        }
                    });
            });
            ui.end_row();
            let classic = on && shown.upscaler == Upscaler::Classic;
            let row = |ui: &mut egui::Ui,
                       name: &str,
                       enabled: bool,
                       pick: &mut dyn FnMut(&mut egui::Ui)| {
                label(ui, name, enabled);
                ui.add_enabled_ui(enabled, |ui| dialog::switch(ui, |ui| pick(ui)));
                ui.end_row();
            };
            let files = media.iter().filter_map(|id| p.media(id));
            if let Some(text) = unfit(shown, files, self.timeline_colors()) {
                ui.label("");
                dialog::warn(ui, text);
                ui.end_row();
            }
            row(ui, "Sharpness", classic, &mut |ui| {
                for (sharpness, text) in SHARPNESS {
                    if ui
                        .selectable_label(now.sharpness == sharpness, text)
                        .clicked()
                    {
                        put = Some(json!({ "sharpness": sharpness }));
                    }
                }
            });
        });
        if cleared {
            put = Some(json!({ "super_scale": false }));
        }
        if let Some(params) = put {
            self.set_super_scale(&media, params);
        }
    }

    fn audio_section(
        &mut self,
        ui: &mut egui::Ui,
        p: &Project,
        sounds: &[Sound],
        acts: &mut Vec<Act>,
    ) {
        let count = sounds.len();
        let header = |ui: &mut egui::Ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let normalize = ui.button("Normalize…").clicked();
                if count > 1 {
                    ui.label(dim(format!("{count} clips")));
                }
                normalize
            })
            .inner
        };
        let mut edit_compressor = false;
        let mut edit_noise = false;
        let normalize = group(ui, "Audio", header, |ui| {
            let first = &sounds[0];
            let off = count > 1 && sounds.iter().any(|s| s.keyed);
            let set = !off
                && sounds
                    .iter()
                    .any(|s| s.keyed || s.gain != Key::Volume.usual());
            let volume = (set, first.gain, *GAIN.start()..=*GAIN.end());
            let keys = p.clip(&first.id).filter(|_| count == 1).and_then(|c| {
                c.audio
                    .as_ref()
                    .map(|a| mark(c, &a.gain_keys, self.snap.timeline.position))
            });
            field(
                ui,
                Key::Volume,
                &self.inspector,
                volume,
                keyed_look(off),
                keys,
                acts,
            );
            let panned = sounds.iter().any(|s| s.pan != Key::Pan.usual());
            let pan = (panned, first.pan, *PAN.start()..=*PAN.end());
            ui.horizontal(|ui| {
                let mut on = first.pan_on;
                if ui.checkbox(&mut on, "").clicked() {
                    acts.push(Act::PanOn(on));
                }
                label(ui, Key::Pan.label(), panned);
            });
            field_rest(
                ui,
                Key::Pan,
                &self.inspector,
                pan,
                keyed_look(false),
                None,
                acts,
            );
            label(ui, "Mute", sounds.iter().any(|s| s.muted));
            let mut on = first.muted;
            if ui.checkbox(&mut on, "").clicked() {
                acts.push(Act::Mute(on));
            }
            ui.end_row();
            let compressor = |id: &str| {
                p.clip(id)
                    .and_then(|c| c.audio.as_ref())
                    .and_then(|a| a.compressor)
            };
            let compressed = sounds
                .iter()
                .any(|s| compressor(&s.id).is_some_and(|c| c.heard()));
            label(ui, "Compressor", compressed);
            ui.horizontal(|ui| {
                let mut on = compressor(&first.id).is_some_and(|c| c.on);
                if ui.checkbox(&mut on, "").clicked() {
                    acts.push(Act::Compress(on));
                }
                edit_compressor = ui.button("Edit…").clicked();
            });
            ui.end_row();
            let denoise = |id: &str| {
                p.clip(id)
                    .and_then(|c| c.audio.as_ref())
                    .and_then(|a| a.noise_reduction)
            };
            let denoised = sounds
                .iter()
                .any(|s| denoise(&s.id).is_some_and(|n| n.heard()));
            label(ui, "Noise Reduction", denoised);
            ui.horizontal(|ui| {
                let mut on = denoise(&first.id).is_some_and(|n| n.on);
                if ui.checkbox(&mut on, "").clicked() {
                    acts.push(Act::Denoise(on));
                }
                edit_noise = ui.button("Edit…").clicked();
            });
            ui.end_row();
            let isolated = sounds.iter().any(|s| heard(p, &s.id).flatten().is_some());
            label(ui, "Voice Isolation", isolated);
            ui.horizontal(|ui| {
                let on = Isolate::Clips(sounds.iter().map(|s| s.id.clone()).collect());
                self.isolation_controls(ui, on, heard(p, &first.id).flatten(), "");
            });
            ui.end_row();
        });
        if normalize {
            self.open_normalize(sounds.iter().map(|s| s.id.clone()).collect());
        }
        if edit_compressor {
            self.open_compressor(sounds.iter().map(|s| s.id.clone()).collect());
        }
        if edit_noise {
            self.open_noise(sounds.iter().map(|s| s.id.clone()).collect());
        }
    }

    fn show_boxes(&mut self, chosen: &[Chosen]) {
        self.timeline.picture_grab.tool = Some(PictureTool::DynamicZoom);
        let Some(p) = self.project_shared() else {
            return;
        };
        let at = self.snap.player(PlayerId::Timeline).position;
        let ids: Vec<String> = chosen.iter().map(|c| c.id.clone()).collect();
        if target(&p, &ids, at).is_none()
            && let Some(c) = chosen.first().and_then(|c| p.clip(&c.id))
        {
            self.seek(PlayerId::Timeline, c.start, false);
        }
    }

    fn inspect(&mut self, act: Act, (chosen, sounds, paced): Lists<'_>) {
        let held_on = |i: &Inspector, f: Key| i.held.as_ref().is_some_and(|h| h.field == f);
        match act {
            Act::Denoise(on) => {
                let ids: Vec<&str> = sounds.iter().map(|s| s.id.as_str()).collect();
                self.send(
                    "clip.set",
                    json!({ "clips": ids, "noise_reduction": { "on": on } }),
                );
            }
            Act::Compress(on) => {
                let ids: Vec<&str> = sounds.iter().map(|s| s.id.as_str()).collect();
                self.send(
                    "clip.set",
                    json!({ "clips": ids, "compressor": { "on": on } }),
                );
            }
            Act::Hold(field, value) => {
                self.inspector.held = Some(Held {
                    field,
                    value,
                    from: value,
                    start: chosen.to_vec(),
                    dragging: true,
                });
            }
            Act::Drag(field, value) => {
                if let Some(h) = self.inspector.held.as_mut().filter(|h| h.field == field) {
                    h.value = match field {
                        Key::Speed => value.max(*SpeedDialog::PERCENT.start()),
                        _ => value,
                    };
                }
            }
            Act::Land(field) => {
                let Some(h) = self
                    .inspector
                    .held
                    .take_if(|h| h.field == field && h.dragging)
                else {
                    return;
                };
                if h.value != h.from {
                    self.put(field, h.value, (&h.start, sounds, paced));
                }
            }
            Act::Put(field, value) => {
                if held_on(&self.inspector, field) {
                    self.inspector.held = None;
                }
                self.put(field, value, (chosen, sounds, paced));
            }
            Act::Edit(field, value) => {
                if !held_on(&self.inspector, field) {
                    self.inspector.held = Some(Held {
                        field,
                        value,
                        from: value,
                        start: Vec::new(),
                        dragging: false,
                    });
                }
            }
            Act::Release(field) => {
                if self
                    .inspector
                    .held
                    .as_ref()
                    .is_some_and(|h| h.field == field && !h.dragging)
                {
                    self.inspector.held = None;
                }
            }
            Act::Direction(to) => self.send_speeds((paced, false), None, |d, _| to.turn(d)),
            Act::SpeedEase(ease) => self.ease_sections(paced, ease),
            Act::Ripple(on) => self.inspector.ripple = Some(on),
            Act::Retime(r) => {
                let ids: Vec<String> = paced.iter().map(|c| c.id.clone()).collect();
                self.set_retime(&ids, r);
            }
            Act::FullSpeed => self.send_speeds((paced, true), None, |d, _| {
                Direction::Forward.turn(d);
                d.set_percent(Key::Speed.usual());
            }),
            Act::Link(on) => {
                self.inspector.unlinked = !on;
                if on {
                    self.send_pictures(chosen, |c| {
                        let [zx, zy] = c.shown.scale();
                        (zx != zy).then(|| Picture {
                            zoom: Some([zx, zx]),
                            ..c.shown.clone()
                        })
                    });
                }
            }
            Act::Flips([across, down]) => self.send_pictures(chosen, |c| {
                Some(Picture {
                    flip_horizontal: across.unwrap_or(c.shown.flip_horizontal),
                    flip_vertical: down.unwrap_or(c.shown.flip_vertical),
                    ..c.shown.clone()
                })
            }),
            Act::Switch(tool, on) => self.send_pictures(chosen, |c| {
                let mut p = c.shown.clone();
                match tool {
                    PictureTool::Transform => p.transform_off = !on,
                    PictureTool::Crop => p.crop_off = !on,
                    PictureTool::DynamicZoom | PictureTool::Composite => return None,
                }
                Some(p)
            }),
            Act::Zooming(on) => {
                self.send_pictures(chosen, |c| {
                    let z = c.shown.dynamic_zoom;
                    Some(Picture {
                        dynamic_zoom: if on {
                            z.or(Some(DynamicZoom::default()))
                        } else {
                            None
                        },
                        ..c.shown.clone()
                    })
                });
                if on {
                    self.show_boxes(chosen);
                }
            }
            Act::AreaEdit(n) => {
                let grab = &mut self.timeline.picture_grab;
                grab.area = (grab.area != Some(n)).then_some(n);
                grab.tool = None;
            }
            Act::AreaTrack(n) => {
                if let Some(c) = chosen.first() {
                    let id = c.id.clone();
                    self.track(&id, n);
                }
            }
            Act::Boxes => {
                if self.timeline.picture_grab.tool == Some(PictureTool::DynamicZoom) {
                    self.timeline.picture_grab.tool = None;
                } else {
                    self.show_boxes(chosen);
                }
            }
            Act::Ease(ease) => self.send_pictures(chosen, |c| {
                let z = c.shown.dynamic_zoom?;
                Some(Picture {
                    dynamic_zoom: Some(DynamicZoom { ease, ..z }),
                    ..c.shown.clone()
                })
            }),
            Act::Mode(mode) => self.send_pictures(chosen, |c| {
                Some(Picture {
                    composite_mode: Some(mode).filter(|m| *m != CompositeMode::Normal),
                    ..c.shown.clone()
                })
            }),
            Act::Swap => self.send_pictures(chosen, |c| {
                let z = c.shown.dynamic_zoom?;
                Some(Picture {
                    dynamic_zoom: Some(z.swapped()),
                    ..c.shown.clone()
                })
            }),
            Act::Clear(tool) => self.clear_pictures(chosen, tool),
            Act::PanOn(on) => {
                let ids: Vec<&str> = sounds
                    .iter()
                    .filter(|s| s.pan_on != on)
                    .map(|s| s.id.as_str())
                    .collect();
                if !ids.is_empty() {
                    self.send("clip.set", json!({ "clips": ids, "pan_on": on }));
                }
            }
            Act::Mute(on) => {
                let ids: Vec<&str> = sounds
                    .iter()
                    .filter(|s| s.muted != on)
                    .map(|s| s.id.as_str())
                    .collect();
                if !ids.is_empty() {
                    self.send("clip.set", json!({ "clips": ids, "muted": on }));
                }
            }
            Act::Step(key, dir) => self.step_key(key, dir, (chosen, sounds, paced)),
            Act::Toggle(key) => self.toggle_key(key, (chosen, sounds, paced)),
            Act::KeyEase(key, ease) => self.ease_key(key, ease, chosen, sounds),
            Act::ResetChannel(field) => self.reset_channel(field, chosen),
            Act::ResetCenter(axis) => self.reset_center(axis, chosen),
            Act::EffectSet(n, params) => self.send_effect("effects.set", chosen, n, params),
            Act::EffectStep(n, dir) => self.step_effect(n, dir, chosen),
            Act::EffectKey(n, ease) => self.key_effect(n, ease, chosen),
            Act::EffectReset(n) => self.send_effect("effects.reset", chosen, n, json!({})),
            Act::EffectRemove(n) => self.send_effect("effects.remove", chosen, n, json!({})),
            Act::EffectMove(n, to) => {
                self.send_effect("effects.move", chosen, n, json!({ "to": to }));
            }
            Act::Tint(effect, from, to) => {
                if let Some(c) = chosen.first() {
                    self.inspector.tinted = Some(Tinted {
                        clip: c.id.clone(),
                        effect,
                        from,
                        to,
                        since: None,
                    });
                }
            }
            Act::Unkey => {
                let keyed: Vec<&str> = sounds
                    .iter()
                    .filter(|s| s.keyed)
                    .map(|s| s.id.as_str())
                    .collect();
                if keyed.is_empty() {
                    self.send_volume(sounds, Key::Volume.usual());
                    return;
                }
                let ids: Vec<&str> = sounds.iter().map(|s| s.id.as_str()).collect();
                let commands = json!([
                    { "method": "keyframes.remove", "params": { "clips": keyed } },
                    { "method": "clip.set", "params": { "clips": ids, "gain_db": 0.0 } },
                ]);
                self.send("apply", json!({ "commands": commands }));
            }
        }
    }

    fn settle_tint(&mut self, chosen: &[Chosen], held: bool) {
        let Some(t) = self.inspector.tinted.as_mut() else {
            return;
        };
        let stored = chosen
            .first()
            .filter(|c| c.id == t.clip)
            .and_then(|c| c.shown.effects.get(t.effect))
            .and_then(|e| match &e.kind {
                EffectKind::Vignette(v) => Some(tint_bytes(v.tint())),
                _ => None,
            });
        let Some(stored) = stored else {
            self.inspector.tinted = None;
            return;
        };
        match t.since {
            Some(since) if landing(t.to, since.elapsed(), t.from, stored).is_none() => {
                self.inspector.tinted = None;
            }
            None if !held => {
                t.since = Some(Instant::now());
                let (n, to) = (t.effect, t.to);
                let color = to.map(|c| f64::from(c) / 255.0);
                self.send_effect("effects.set", chosen, n, json!({ "color": color }));
            }
            _ => {}
        }
    }

    pub(super) fn inspector_saw_selection(&mut self) {
        let s = &self.snap.selection;
        self.inspector.seen = Some((s.clips.clone(), s.cues.clone()));
    }

    fn follow_selection(&mut self) {
        let s = &self.snap.selection;
        let now = (s.clips.clone(), s.cues.clone());
        if self.inspector.seen.as_ref() == Some(&now) {
            return;
        }
        let title = self.project_shared().and_then(|p| {
            let is_title = |id: &String| p.clip(id).is_some_and(|c| c.title.is_some());
            let several = now.0.len() > 1 && now.1.is_empty() && now.0.iter().all(is_title);
            now.0
                .iter()
                .find(|id| is_title(id))
                .filter(|_| !several)
                .cloned()
        });
        if let Some(id) = title {
            self.inspect_title(&id, false);
        } else if let Some(id) = now.1.first().cloned() {
            self.inspect_cue(&id, false);
        } else {
            self.close_forms();
        }
        self.inspector.seen = Some(now);
    }

    pub(super) fn inspector_commits(&self) -> bool {
        self.inspector.commit
    }

    pub(super) fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let focus = ui.ctx().memory(|m| m.focused());
        let i = &mut self.inspector;
        i.moved |= focus != i.focus;
        i.focus = focus;
        i.commit = (focus.is_none() || i.moved) && !ui.input(|i| i.pointer.any_down());
        i.moved &= !i.commit;
        self.follow_selection();
        self.input_lut_picked();
        let chosen = self.chosen();
        let sounds = self.sounds();
        if sounds.iter().all(|s| s.gain == s.stored) {
            self.inspector.gained = None;
        }
        let paced = self.paced();
        if paced.iter().all(|c| c.shown == c.stored) {
            self.inspector.retimed = None;
        }
        let ripple = self.ripple();
        let dropped = self.dropped();
        let gone = |k: Key| match k {
            Key::Picture(_) | Key::Center(_) => chosen.is_empty(),
            Key::Effect(n, _) => chosen.first().is_none_or(|c| c.shown.effects.len() <= n),
            Key::Volume | Key::Pan => sounds.is_empty(),
            Key::Speed | Key::Rate | Key::Length => paced.is_empty(),
        };
        if self.inspector.held.as_ref().is_some_and(|h| gone(h.field)) {
            self.inspector.held = None;
        }
        let mut acts = Vec::new();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - dialog::scroll_room(ui));
                let forms = self.title_form(ui)
                    | self.cue_form(ui)
                    | self.track_form(ui)
                    | self.transition_form(ui);
                let Some(p) = self.project_shared() else {
                    return;
                };
                let top = chosen.first().map(|c| &c.id);
                let Some(first) = top
                    .or(sounds.first().map(|s| &s.id))
                    .or(paced.first().map(|c| &c.id))
                    .and_then(|id| p.clip(id))
                else {
                    if !forms {
                        ui.label(dim("Select a clip with a picture or sound"));
                    }
                    return;
                };
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if chosen.len() > 1 {
                            ui.label(dim(format!("{} clips selected", chosen.len())));
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let text = RichText::new(name(&p, first)).strong();
                            ui.add(egui::Label::new(text).truncate());
                        });
                    });
                });
                self.generator_section(ui, first, &mut acts);
                if !chosen.is_empty() {
                    let boxes = self.timeline.picture_grab.tool == Some(PictureTool::DynamicZoom);
                    let i = &self.inspector;
                    let at = self.snap.timeline.position;
                    transform(ui, i, &p, at, &chosen, &mut acts);
                    crop(ui, i, &p, at, &chosen, &mut acts);
                    dynamic_zoom(ui, i, &p, at, &chosen, boxes, &mut acts);
                    composite(ui, i, &p, at, &chosen, &mut acts);
                    self.input_lut_section(ui, &p, &chosen);
                    self.super_scale_section(ui, &p, &chosen);
                    self.motion_section(ui, &chosen[0]);
                    self.inspector.area_edit = self.timeline.picture_grab.area;
                    effects(ui, &self.inspector, &chosen[0], &mut acts);
                }
                if !paced.is_empty() {
                    let state = (&self.inspector, &*p, dropped);
                    let at = self.snap.timeline.position;
                    speed_section(ui, state, (&paced, at), ripple, &mut acts);
                }
                if !sounds.is_empty() {
                    self.audio_section(ui, &p, &sounds, &mut acts);
                    let ids: Vec<String> = sounds.iter().map(|s| s.id.clone()).collect();
                    self.equalizer_section(ui, &p, &ids);
                    self.leveler_section(ui, &p, &ids);
                }
            });
        for act in acts {
            self.inspect(act, (&chosen, &sounds, &paced));
        }
        self.settle_tint(&chosen, ui.input(|i| i.pointer.any_down()));
        self.sync_picture_preview();
    }
}
