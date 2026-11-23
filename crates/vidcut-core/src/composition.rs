use std::collections::HashSet;

use crate::effect_keys::Keyed;
use crate::model::{
    Appearance, ApplyMode, Background, BackgroundFill, Blur, BlurClipping, BlurFilter, BorderLevel,
    Clip, Composition, Connect, EffectKind, EffectType, EffectValue, FastNoise, GradientRepeat,
    GradientStop, GradientType, ImageFormat, JoinStyle, Mask, Merge, MergeEdges, MergeOperator,
    MotionPath, Node, NoiseColoring, Pattern, Project, ShadingElement, TextPlus, Tool, ToolType,
    Tracker,
};
use crate::motion::Area;
use crate::render::FileClock;
use crate::request::NodeInput;

pub const MEDIA_IN: &str = "MediaIn1";
pub const MEDIA_OUT: &str = "MediaOut1";

impl Default for Blur {
    fn default() -> Self {
        Self {
            filter: None,
            size: None,
            lock_xy: true,
            y_size: None,
            passes: None,
            clipping: None,
            channels: None,
        }
    }
}

impl Blur {
    pub const SIZE: f64 = 1.0;
    pub const SIZES: (f64, f64) = (0.0, 1e6);
    pub const PASSES: u32 = 4;
    pub const MOST_PASSES: u32 = 64;
    pub const ALL: [bool; 4] = [true; 4];
    pub const BOX: f64 = 0.002831;
    pub const BARTLETT: f64 = 0.001445;
    pub const GAUSSIAN: f64 = 0.000605;
    pub const FAST: f64 = 0.000598;

    pub fn filter(&self) -> BlurFilter {
        self.filter.unwrap_or_default()
    }

    pub fn across(&self) -> f64 {
        self.size.unwrap_or(Self::SIZE)
    }

    pub fn down(&self) -> f64 {
        match self.lock_xy {
            true => self.across(),
            false => self.y_size.unwrap_or(Self::SIZE),
        }
    }

    pub fn passes(&self) -> u32 {
        self.passes.unwrap_or(Self::PASSES)
    }

    pub fn clipping(&self) -> BlurClipping {
        self.clipping.unwrap_or_default()
    }

    pub fn channels(&self) -> [bool; 4] {
        self.channels.unwrap_or(Self::ALL)
    }
}

impl BlurFilter {
    pub const ALL: [BlurFilter; 5] = [
        BlurFilter::Box,
        BlurFilter::Bartlett,
        BlurFilter::MultiBox,
        BlurFilter::Gaussian,
        BlurFilter::FastGaussian,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Box => "Box",
            Self::Bartlett => "Bartlett",
            Self::MultiBox => "Multi-box",
            Self::Gaussian => "Gaussian",
            Self::FastGaussian => "Fast Gaussian",
        }
    }
}

impl BlurClipping {
    pub const ALL: [BlurClipping; 3] = [
        BlurClipping::None,
        BlurClipping::Domain,
        BlurClipping::Frame,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Domain => "Domain",
            Self::Frame => "Frame",
        }
    }
}

impl Default for Mask {
    fn default() -> Self {
        Self {
            show_view_controls: true,
            level: None,
            soft_edge: None,
            filter: None,
            passes: None,
            border_width: None,
            invert: false,
            solid: true,
            center: None,
            width: None,
            height: None,
            angle: None,
            size: None,
            z_rotation: None,
            x_rotation: None,
            y_rotation: None,
            corner_radius: None,
            fill_method: None,
            points: Vec::new(),
        }
    }
}

impl Pattern {
    pub const CENTER: [f64; 2] = [0.25, 0.75];
    pub const WIDTH: f64 = 0.03;
    pub const HEIGHT: f64 = 0.0534;
    pub const SIDES: (f64, f64) = (0.001, 0.5);
    pub const PLACES: (f64, f64) = (-crate::motion::REACH, crate::motion::REACH);

    pub fn new(name: String) -> Self {
        Self {
            name,
            enabled: true,
            path: None,
            center: None,
            width: None,
            height: None,
            offset: None,
        }
    }

    pub fn middle(&self) -> [f64; 2] {
        self.center.unwrap_or(Self::CENTER)
    }

    pub fn wide(&self) -> f64 {
        self.width.unwrap_or(Self::WIDTH)
    }

    pub fn high(&self) -> f64 {
        self.height.unwrap_or(Self::HEIGHT)
    }

    pub fn shift(&self) -> [f64; 2] {
        self.offset.unwrap_or([0.0, 0.0])
    }

    pub fn own(&self) -> Area {
        let [x, y] = self.middle();
        Area {
            center: [x, 1.0 - y],
            size: [self.wide(), self.high()],
            rotation: 0.0,
        }
    }

    pub fn area_at(&self, frame: f64) -> Option<Area> {
        match &self.path {
            None => Some(self.own()),
            Some(m) => m.base_at(frame),
        }
    }

    pub fn offset_position_at(&self, frame: f64) -> Option<[f64; 2]> {
        let a = self.area_at(frame)?;
        let [x, y] = self.shift();
        Some([a.center[0] + x, 1.0 - a.center[1] + y])
    }
}

impl Tracker {
    pub const PATTERNS_KEPT: usize = 256;
    pub const NAME: &'static str = "Pattern";

    pub fn free_name(&self) -> String {
        (1..)
            .map(|n| format!("{} {n}", Self::NAME))
            .find(|name| !self.patterns.iter().any(|p| p.name == *name))
            .unwrap_or_default()
    }

    pub fn made() -> Self {
        Self {
            patterns: vec![Pattern::new(format!("{} 1", Self::NAME))],
        }
    }
}

impl Merge {
    pub const SIZE: f64 = 1.0;
    pub const SLIDES: (f64, f64) = (0.0, 1.0);
    pub const Z_OFFSETS: (f64, f64) = (-1e6, 1e6);

    pub fn middle(&self) -> [f64; 2] {
        self.center.unwrap_or(Mask::MIDDLE)
    }

    pub fn scale(&self) -> f64 {
        self.size.unwrap_or(Self::SIZE)
    }

    pub fn turn(&self) -> f64 {
        self.angle.unwrap_or(0.0)
    }

    pub fn edges(&self) -> MergeEdges {
        self.edges.unwrap_or_default()
    }

    pub fn mode(&self) -> ApplyMode {
        self.apply_mode.unwrap_or_default()
    }

    pub fn operator(&self) -> MergeOperator {
        self.operator.unwrap_or_default()
    }

    pub fn additive(&self) -> f64 {
        self.subtractive_additive.unwrap_or(1.0)
    }

    pub fn gain(&self) -> f64 {
        self.alpha_gain.unwrap_or(1.0)
    }

    pub fn burn(&self) -> f64 {
        self.burn_in.unwrap_or(0.0)
    }

    pub fn z_offset(&self) -> f64 {
        self.foreground_z_offset.unwrap_or(0.0)
    }

    pub fn back_additive(&self) -> f64 {
        self.background_subtractive_additive.unwrap_or(1.0)
    }
}

impl ImageFormat {
    pub const PIXELS: (u32, u32) = (1, 32768);

    pub fn is_usual(&self) -> bool {
        *self == Self::default()
    }

    pub fn size(&self, (w, h): (f64, f64)) -> (f64, f64) {
        match self.auto_resolution {
            true => (w, h),
            false => (
                self.width.map_or(w, f64::from),
                self.height.map_or(h, f64::from),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Corner::TopLeft => "top_left",
            Corner::TopRight => "top_right",
            Corner::BottomLeft => "bottom_left",
            Corner::BottomRight => "bottom_right",
        }
    }

    pub fn values(self) -> [EffectValue; 4] {
        use EffectValue as V;
        match self {
            Corner::TopLeft => [
                V::TopLeftRed,
                V::TopLeftGreen,
                V::TopLeftBlue,
                V::TopLeftAlpha,
            ],
            Corner::TopRight => [
                V::TopRightRed,
                V::TopRightGreen,
                V::TopRightBlue,
                V::TopRightAlpha,
            ],
            Corner::BottomLeft => [
                V::BottomLeftRed,
                V::BottomLeftGreen,
                V::BottomLeftBlue,
                V::BottomLeftAlpha,
            ],
            Corner::BottomRight => [
                V::BottomRightRed,
                V::BottomRightGreen,
                V::BottomRightBlue,
                V::BottomRightAlpha,
            ],
        }
    }
}

impl Background {
    pub const COLOR: [f64; 4] = [0.0, 0.0, 0.0, 1.0];
    pub const COLORS: (f64, f64) = (-1e6, 1e6);
    pub const START: [f64; 2] = [0.0, 0.5];
    pub const END: [f64; 2] = [1.0, 0.5];
    pub const OFFSETS: (f64, f64) = (-1e6, 1e6);
    pub const STOPS: (usize, usize) = (2, 64);
    pub const STOPS_USUAL: [GradientStop; 2] = [
        GradientStop {
            position: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
        },
        GradientStop {
            position: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
        },
    ];

    pub fn filled(&self) -> BackgroundFill {
        self.fill.unwrap_or_default()
    }

    pub fn corner(&self, c: Corner) -> Option<[f64; 4]> {
        match c {
            Corner::TopLeft => self.top_left,
            Corner::TopRight => self.top_right,
            Corner::BottomLeft => self.bottom_left,
            Corner::BottomRight => self.bottom_right,
        }
    }

    pub fn corner_mut(&mut self, c: Corner) -> &mut Option<[f64; 4]> {
        match c {
            Corner::TopLeft => &mut self.top_left,
            Corner::TopRight => &mut self.top_right,
            Corner::BottomLeft => &mut self.bottom_left,
            Corner::BottomRight => &mut self.bottom_right,
        }
    }

    pub fn color(&self, c: Corner) -> [f64; 4] {
        self.corner(c).unwrap_or(Self::COLOR)
    }

    pub fn shown(&self) -> &'static [Corner] {
        match self.filled() {
            BackgroundFill::SolidColor => &[Corner::TopLeft],
            BackgroundFill::Horizontal => &[Corner::TopLeft, Corner::TopRight],
            BackgroundFill::Vertical => &[Corner::TopLeft, Corner::BottomLeft],
            BackgroundFill::FourCorner => &Corner::ALL,
            BackgroundFill::Gradient => &[],
        }
    }

    pub fn form(&self) -> GradientType {
        self.gradient_type.unwrap_or_default()
    }

    pub fn from(&self) -> [f64; 2] {
        self.start.unwrap_or(Self::START)
    }

    pub fn to(&self) -> [f64; 2] {
        self.end.unwrap_or(Self::END)
    }

    pub fn colors(&self) -> &[GradientStop] {
        match self.stops.is_empty() {
            true => &Self::STOPS_USUAL,
            false => &self.stops,
        }
    }

    pub fn shift(&self) -> f64 {
        self.offset.unwrap_or(0.0)
    }

    pub fn color_at(stops: &[GradientStop], t: f64) -> [f64; 4] {
        let Some(first) = stops.first() else {
            return Self::COLOR;
        };
        if t <= first.position {
            return first.color;
        }
        for pair in stops.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            if t <= b.position {
                let k = match b.position > a.position {
                    true => (t - a.position) / (b.position - a.position),
                    false => 0.0,
                };
                return std::array::from_fn(|i| a.color[i] + (b.color[i] - a.color[i]) * k);
            }
        }
        stops.last().map_or(Self::COLOR, |s| s.color)
    }

    pub fn repeats(&self) -> GradientRepeat {
        self.repeat.unwrap_or_default()
    }
}

impl Default for FastNoise {
    fn default() -> Self {
        Self {
            discontinuous: false,
            inverted: false,
            center: None,
            detail: None,
            brightness: None,
            contrast: None,
            lock_xy: true,
            scale: None,
            y_scale: None,
            angle: None,
            seethe: None,
            seethe_rate: None,
            coloring: None,
            color1: None,
            color2: None,
            stops: Vec::new(),
            image: ImageFormat::default(),
        }
    }
}

impl FastNoise {
    pub const DETAIL: f64 = 2.0;
    pub const DETAILS: (f64, f64) = (0.0, 10.0);
    pub const LEVELS: (f64, f64) = (-1000.0, 1000.0);
    pub const CONTRAST: f64 = 1.0;
    pub const SCALE: f64 = 2.0;
    pub const SCALES: (f64, f64) = (0.0, 1e6);
    pub const TURNS: (f64, f64) = (-1e6, 1e6);
    pub const SEETHES: (f64, f64) = (-1e6, 1e6);
    pub const COLOR1: [f64; 4] = [0.0, 0.0, 0.0, 0.0];
    pub const COLOR2: [f64; 4] = [1.0, 1.0, 1.0, 1.0];

    pub fn middle(&self) -> [f64; 2] {
        self.center.unwrap_or(Mask::MIDDLE)
    }

    pub fn layers(&self) -> f64 {
        self.detail.unwrap_or(Self::DETAIL)
    }

    pub fn lift(&self) -> f64 {
        self.brightness.unwrap_or(0.0)
    }

    pub fn spread(&self) -> f64 {
        self.contrast.unwrap_or(Self::CONTRAST)
    }

    pub fn scales(&self) -> [f64; 2] {
        let x = self.scale.unwrap_or(Self::SCALE);
        match self.lock_xy {
            true => [x, x],
            false => [x, self.y_scale.unwrap_or(Self::SCALE)],
        }
    }

    pub fn turn(&self) -> f64 {
        self.angle.unwrap_or(0.0)
    }

    pub fn depth(&self, frame: i64) -> f64 {
        self.seethe.unwrap_or(0.0) + self.seethe_rate.unwrap_or(0.0) * frame as f64
    }

    pub fn colored(&self) -> NoiseColoring {
        self.coloring.unwrap_or_default()
    }

    pub fn first(&self) -> [f64; 4] {
        self.color1.unwrap_or(Self::COLOR1)
    }

    pub fn second(&self) -> [f64; 4] {
        self.color2.unwrap_or(Self::COLOR2)
    }

    pub fn colors(&self) -> &[GradientStop] {
        match self.stops.is_empty() {
            true => &Background::STOPS_USUAL,
            false => &self.stops,
        }
    }

    pub fn color_values(second: bool) -> [EffectValue; 4] {
        use EffectValue as V;
        match second {
            false => [V::Color1Red, V::Color1Green, V::Color1Blue, V::Color1Alpha],
            true => [V::Color2Red, V::Color2Green, V::Color2Blue, V::Color2Alpha],
        }
    }
}

impl NoiseColoring {
    pub const ALL: [NoiseColoring; 2] = [NoiseColoring::TwoColor, NoiseColoring::Gradient];

    pub fn label(self) -> &'static str {
        match self {
            NoiseColoring::TwoColor => "Two Color",
            NoiseColoring::Gradient => "Gradient",
        }
    }
}

impl BackgroundFill {
    pub const ALL: [BackgroundFill; 5] = [
        BackgroundFill::SolidColor,
        BackgroundFill::Horizontal,
        BackgroundFill::Vertical,
        BackgroundFill::FourCorner,
        BackgroundFill::Gradient,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BackgroundFill::SolidColor => "Solid Color",
            BackgroundFill::Horizontal => "Horizontal",
            BackgroundFill::Vertical => "Vertical",
            BackgroundFill::FourCorner => "Four Corner",
            BackgroundFill::Gradient => "Gradient",
        }
    }
}

impl GradientType {
    pub const ALL: [GradientType; 6] = [
        GradientType::Linear,
        GradientType::Reflect,
        GradientType::Square,
        GradientType::Cross,
        GradientType::Radial,
        GradientType::Angle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GradientType::Linear => "Linear",
            GradientType::Reflect => "Reflect",
            GradientType::Square => "Square",
            GradientType::Cross => "Cross",
            GradientType::Radial => "Radial",
            GradientType::Angle => "Angle",
        }
    }
}

impl GradientRepeat {
    pub const ALL: [GradientRepeat; 3] = [
        GradientRepeat::Once,
        GradientRepeat::Repeat,
        GradientRepeat::PingPong,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GradientRepeat::Once => "Once",
            GradientRepeat::Repeat => "Repeat",
            GradientRepeat::PingPong => "Ping-pong",
        }
    }
}

impl TextPlus {
    pub const SIZE: f64 = 0.08;
    pub const STYLE: &str = "Bold";
    pub const SPACINGS: (f64, f64) = (-1e6, 1e6);
    pub const ANCHORS: (f64, f64) = (-1.0, 1.0);

    pub fn scale(&self) -> f64 {
        self.size.unwrap_or(Self::SIZE)
    }

    pub fn face(&self) -> &str {
        self.style.as_deref().unwrap_or(Self::STYLE)
    }

    pub fn track(&self) -> f64 {
        self.tracking.unwrap_or(1.0)
    }

    pub fn spacing(&self) -> f64 {
        self.line_spacing.unwrap_or(1.0)
    }

    pub fn anchors(&self) -> [f64; 2] {
        [self.horizontal_anchor, self.vertical_anchor].map(|a| a.unwrap_or(0.0))
    }

    pub fn middle(&self) -> [f64; 2] {
        self.center.unwrap_or(Mask::MIDDLE)
    }

    pub fn layout(&self) -> f64 {
        self.layout_size.unwrap_or(1.0)
    }

    pub fn turn(&self) -> f64 {
        self.angle.unwrap_or(0.0)
    }

    pub fn element(&self, k: u8) -> ShadingElement {
        let kept = self.shading.iter().find(|s| s.element == k);
        kept.cloned().unwrap_or_else(|| ShadingElement::new(k))
    }

    pub fn element_mut(&mut self, k: u8) -> &mut ShadingElement {
        let i = match self.shading.binary_search_by_key(&k, |s| s.element) {
            Ok(i) => i,
            Err(i) => {
                self.shading.insert(i, ShadingElement::new(k));
                i
            }
        };
        &mut self.shading[i]
    }

    pub fn drawn(&self) -> Vec<ShadingElement> {
        let mut on: Vec<ShadingElement> = (1..=ShadingElement::ELEMENTS)
            .map(|k| self.element(k))
            .filter(ShadingElement::on)
            .collect();
        on.sort_by(|a, b| (a.rank().total_cmp(&b.rank())).then(b.element.cmp(&a.element)));
        on
    }
}

impl Appearance {
    pub const ALL: [Appearance; 4] = [
        Appearance::TextFill,
        Appearance::TextOutline,
        Appearance::BorderFill,
        Appearance::BorderOutline,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::TextFill => "Text Fill",
            Appearance::TextOutline => "Text Outline",
            Appearance::BorderFill => "Border Fill",
            Appearance::BorderOutline => "Border Outline",
        }
    }
}

impl JoinStyle {
    pub const ALL: [JoinStyle; 3] = [JoinStyle::Sharp, JoinStyle::Rounded, JoinStyle::Beveled];

    pub fn label(self) -> &'static str {
        match self {
            JoinStyle::Sharp => "Sharp",
            JoinStyle::Rounded => "Rounded",
            JoinStyle::Beveled => "Beveled",
        }
    }
}

impl BorderLevel {
    pub const ALL: [BorderLevel; 4] = [
        BorderLevel::Text,
        BorderLevel::Line,
        BorderLevel::Word,
        BorderLevel::Character,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BorderLevel::Text => "Text",
            BorderLevel::Line => "Line",
            BorderLevel::Word => "Word",
            BorderLevel::Character => "Character",
        }
    }
}

struct Preset {
    name: &'static str,
    appearance: Appearance,
    color: [f64; 3],
    softness: f64,
    offset: [f64; 2],
}

const PLAIN: Preset = Preset {
    name: "",
    appearance: Appearance::TextFill,
    color: [1.0; 3],
    softness: 0.0,
    offset: [0.0; 2],
};

const PRESETS: [Preset; 4] = [
    Preset {
        name: "White Solid Fill",
        ..PLAIN
    },
    Preset {
        name: "Red Outline",
        appearance: Appearance::TextOutline,
        color: [1.0, 0.0, 0.0],
        ..PLAIN
    },
    Preset {
        name: "Black Shadow",
        color: [0.0; 3],
        softness: 5.0,
        offset: [0.05, -0.05],
        ..PLAIN
    },
    Preset {
        name: "Blue Border",
        appearance: Appearance::BorderFill,
        color: [0.0, 0.0, 1.0],
        ..PLAIN
    },
];

impl ShadingElement {
    pub const ELEMENTS: u8 = 8;
    pub const THICKNESS: f64 = 0.02;
    pub const MITER: f64 = 10.0;
    pub const EXTEND: f64 = 0.2;
    pub const SOFTNESS: f64 = 0.000589;
    pub const SOFT_EDGE: f64 = 0.3;
    pub const COLORS: (f64, f64) = (0.0, 1e6);
    pub const OPACITIES: (f64, f64) = (0.0, 1.0);
    pub const AMOUNTS: (f64, f64) = (0.0, 1e6);
    pub const SHIFTS: (f64, f64) = (-1e6, 1e6);

    pub fn new(element: u8) -> Self {
        Self {
            element,
            ..Self::default()
        }
    }

    fn preset(&self) -> &'static Preset {
        let i = usize::from(self.element).wrapping_sub(1);
        PRESETS.get(i).unwrap_or(&PLAIN)
    }

    pub fn on(&self) -> bool {
        self.enabled.unwrap_or(self.element == 1)
    }

    pub fn usual_name(&self) -> String {
        match self.preset().name {
            "" => format!("Element {}", self.element),
            name => name.to_string(),
        }
    }

    pub fn title(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.usual_name())
    }

    pub fn look(&self) -> Appearance {
        self.appearance.unwrap_or(self.preset().appearance)
    }

    pub fn usual_ink(&self) -> [f64; 3] {
        self.preset().color
    }

    pub fn ink(&self) -> [f64; 3] {
        self.color.unwrap_or(self.preset().color)
    }

    pub fn alpha(&self) -> f64 {
        self.opacity.unwrap_or(1.0)
    }

    pub fn width(&self) -> f64 {
        self.thickness.unwrap_or(Self::THICKNESS)
    }

    pub fn join(&self) -> JoinStyle {
        self.join_style.unwrap_or_default()
    }

    pub fn reach(&self) -> BorderLevel {
        self.border_level.unwrap_or_default()
    }

    pub fn extents(&self) -> [f64; 2] {
        [self.extend_horizontal, self.extend_vertical].map(|e| e.unwrap_or(0.0))
    }

    pub fn rounding(&self) -> f64 {
        self.round.unwrap_or(0.0)
    }

    pub fn usual_soft(&self) -> f64 {
        self.preset().softness
    }

    pub fn soft(&self) -> [f64; 2] {
        [self.softness_x, self.softness_y].map(|s| s.unwrap_or(self.usual_soft()))
    }

    pub fn usual_shift(&self) -> [f64; 2] {
        self.preset().offset
    }

    pub fn shift(&self) -> [f64; 2] {
        self.offset.unwrap_or(self.usual_shift())
    }

    pub fn usual_rank(&self) -> f64 {
        9.0 - f64::from(self.element)
    }

    pub fn rank(&self) -> f64 {
        self.priority.unwrap_or(self.usual_rank())
    }

    pub fn outlined(&self) -> bool {
        matches!(
            self.look(),
            Appearance::TextOutline | Appearance::BorderOutline
        )
    }

    pub fn boxed(&self) -> bool {
        matches!(
            self.look(),
            Appearance::BorderFill | Appearance::BorderOutline
        )
    }

    pub fn spread(soft: f64, width: f64) -> f64 {
        if soft <= 0.0 {
            return 0.0;
        }
        let seen = Self::SOFTNESS * width * soft + Self::SOFT_EDGE;
        (seen * seen - 1.0 / 12.0).max(0.0).sqrt()
    }

    pub fn plain(&self) -> bool {
        *self == Self::new(self.element)
    }
}

impl ApplyMode {
    pub const ALL: [ApplyMode; 27] = [
        ApplyMode::Normal,
        ApplyMode::Screen,
        ApplyMode::Dissolve,
        ApplyMode::Darken,
        ApplyMode::Multiply,
        ApplyMode::ColorBurn,
        ApplyMode::LinearBurn,
        ApplyMode::DarkerColor,
        ApplyMode::Lighten,
        ApplyMode::ColorDodge,
        ApplyMode::LinearDodge,
        ApplyMode::LighterColor,
        ApplyMode::Overlay,
        ApplyMode::SoftLight,
        ApplyMode::HardLight,
        ApplyMode::VividLight,
        ApplyMode::LinearLight,
        ApplyMode::PinLight,
        ApplyMode::HardMix,
        ApplyMode::Difference,
        ApplyMode::Exclusion,
        ApplyMode::Hue,
        ApplyMode::Saturation,
        ApplyMode::Color,
        ApplyMode::Luminosity,
        ApplyMode::Hypotenuse,
        ApplyMode::Geometric,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ApplyMode::Normal => "Normal",
            ApplyMode::Screen => "Screen",
            ApplyMode::Dissolve => "Dissolve",
            ApplyMode::Darken => "Darken",
            ApplyMode::Multiply => "Multiply",
            ApplyMode::ColorBurn => "Color Burn",
            ApplyMode::LinearBurn => "Linear Burn",
            ApplyMode::DarkerColor => "Darker Color",
            ApplyMode::Lighten => "Lighten",
            ApplyMode::ColorDodge => "Color Dodge",
            ApplyMode::LinearDodge => "Linear Dodge",
            ApplyMode::LighterColor => "Lighter Color",
            ApplyMode::Overlay => "Overlay",
            ApplyMode::SoftLight => "Soft Light",
            ApplyMode::HardLight => "Hard Light",
            ApplyMode::VividLight => "Vivid Light",
            ApplyMode::LinearLight => "Linear Light",
            ApplyMode::PinLight => "Pin Light",
            ApplyMode::HardMix => "Hard Mix",
            ApplyMode::Difference => "Difference",
            ApplyMode::Exclusion => "Exclusion",
            ApplyMode::Hue => "Hue",
            ApplyMode::Saturation => "Saturation",
            ApplyMode::Color => "Color",
            ApplyMode::Luminosity => "Luminosity",
            ApplyMode::Hypotenuse => "Hypotenuse",
            ApplyMode::Geometric => "Geometric",
        }
    }

    pub fn has_operator(self) -> bool {
        matches!(self, ApplyMode::Normal | ApplyMode::Screen)
    }
}

impl MergeOperator {
    pub const ALL: [MergeOperator; 10] = [
        MergeOperator::Over,
        MergeOperator::In,
        MergeOperator::HeldOut,
        MergeOperator::Atop,
        MergeOperator::XOr,
        MergeOperator::Conjoint,
        MergeOperator::Disjoint,
        MergeOperator::Mask,
        MergeOperator::Stencil,
        MergeOperator::Under,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MergeOperator::Over => "Over",
            MergeOperator::In => "In",
            MergeOperator::HeldOut => "Held Out",
            MergeOperator::Atop => "Atop",
            MergeOperator::XOr => "XOr",
            MergeOperator::Conjoint => "Conjoint",
            MergeOperator::Disjoint => "Disjoint",
            MergeOperator::Mask => "Mask",
            MergeOperator::Stencil => "Stencil",
            MergeOperator::Under => "Under",
        }
    }
}

impl MergeEdges {
    pub const ALL: [MergeEdges; 4] = [
        MergeEdges::Canvas,
        MergeEdges::Wrap,
        MergeEdges::Duplicate,
        MergeEdges::Mirror,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MergeEdges::Canvas => "Canvas",
            MergeEdges::Wrap => "Wrap",
            MergeEdges::Duplicate => "Duplicate",
            MergeEdges::Mirror => "Mirror",
        }
    }
}

impl Mask {
    pub const LEVEL: f64 = 1.0;
    pub const LEVELS: (f64, f64) = (0.0, 1.0);
    pub const SOFT_EDGES: (f64, f64) = (0.0, 1.0);
    pub const BORDERS: (f64, f64) = (-1.0, 1.0);
    pub const MIDDLE: [f64; 2] = [0.5, 0.5];
    pub const PLACES: (f64, f64) = (-crate::motion::REACH, crate::motion::REACH);
    pub const SIZE: f64 = 0.5;
    pub const SIZES: (f64, f64) = (0.0, 1e6);
    pub const SCALE: f64 = 1.0;
    pub const TURNS: (f64, f64) = (-1e6, 1e6);
    pub const CORNERS: (f64, f64) = (0.0, 1.0);
    pub const POINTS_KEPT: usize = 128;
    pub const LEAST_POINTS: usize = 3;
    pub const SOFT: f64 = 0.431;
    pub const SOFT_GAUSSIAN: f64 = 0.4276;
    pub const SOFT_BOXES: f64 = 0.4135;

    pub fn level(&self) -> f64 {
        self.level.unwrap_or(Self::LEVEL)
    }

    pub fn soft(&self) -> f64 {
        self.soft_edge.unwrap_or(0.0)
    }

    pub fn filter(&self) -> BlurFilter {
        self.filter.unwrap_or_default()
    }

    pub fn passes(&self) -> u32 {
        self.passes.unwrap_or(Blur::PASSES)
    }

    pub fn scale(&self) -> f64 {
        self.size.unwrap_or(Self::SCALE)
    }

    pub fn tilts(&self) -> [f64; 3] {
        [self.z_rotation, self.x_rotation, self.y_rotation].map(|t| t.unwrap_or(0.0))
    }

    pub fn border(&self) -> f64 {
        self.border_width.unwrap_or(0.0)
    }

    pub fn middle(&self) -> [f64; 2] {
        self.center.unwrap_or(Self::MIDDLE)
    }

    pub fn wide(&self) -> f64 {
        self.width.unwrap_or(Self::SIZE)
    }

    pub fn high(&self) -> f64 {
        self.height.unwrap_or(Self::SIZE)
    }

    pub fn turn(&self) -> f64 {
        self.angle.unwrap_or(0.0)
    }

    pub fn corner(&self) -> f64 {
        self.corner_radius.unwrap_or(0.0)
    }
}

impl Tool {
    pub const MASKS: [&'static str; 3] = ["ellipse", "rectangle", "polygon"];

    pub fn base(&self) -> &'static str {
        match self {
            Tool::MediaIn => "MediaIn",
            Tool::MediaOut => "MediaOut",
            Tool::Blur(_) => "Blur",
            Tool::Effect(kind) => match EffectType::of(kind) {
                EffectType::GaussianBlur => "GaussianBlur",
                EffectType::MosaicBlur => "MosaicBlur",
                EffectType::Glow => "Glow",
                EffectType::FilmGrain => "FilmGrain",
                EffectType::Vignette => "Vignette",
                EffectType::Sharpen => "Sharpen",
            },
            Tool::Ellipse(_) => "Ellipse",
            Tool::Rectangle(_) => "Rectangle",
            Tool::Polygon(_) => "Polygon",
            Tool::Tracker(_) => "Tracker",
            Tool::Merge(_) => "Merge",
            Tool::TextPlus(_) => "Text",
            Tool::Background(_) => "Background",
            Tool::FastNoise(_) => "FastNoise",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Tool::MediaIn => "Media In",
            Tool::MediaOut => "Media Out",
            Tool::Blur(_) => "Blur",
            Tool::Effect(kind) => EffectType::of(kind).name(),
            Tool::Ellipse(_) => "Ellipse",
            Tool::Rectangle(_) => "Rectangle",
            Tool::Polygon(_) => "Polygon",
            Tool::Tracker(_) => "Tracker",
            Tool::Merge(_) => "Merge",
            Tool::TextPlus(_) => "Text+",
            Tool::Background(_) => "Background",
            Tool::FastNoise(_) => "Fast Noise",
        }
    }

    pub fn is_mask(&self) -> bool {
        self.mask().is_some()
    }

    pub fn works(&self) -> bool {
        matches!(
            self,
            Tool::Blur(_)
                | Tool::Effect(_)
                | Tool::Merge(_)
                | Tool::TextPlus(_)
                | Tool::Background(_)
                | Tool::FastNoise(_)
        )
    }

    pub fn inputs(&self) -> &'static [NodeInput] {
        match self {
            Tool::MediaIn => &[NodeInput::EffectMask],
            Tool::MediaOut => &[NodeInput::Input],
            Tool::Tracker(_) | Tool::Blur(_) | Tool::Effect(_) => {
                &[NodeInput::Input, NodeInput::EffectMask]
            }
            Tool::Merge(_) => &[
                NodeInput::Input,
                NodeInput::Foreground,
                NodeInput::EffectMask,
            ],
            Tool::Ellipse(_)
            | Tool::Rectangle(_)
            | Tool::Polygon(_)
            | Tool::TextPlus(_)
            | Tool::Background(_) => &[NodeInput::EffectMask],
            Tool::FastNoise(_) => &[
                NodeInput::EffectMask,
                NodeInput::DetailMap,
                NodeInput::BrightnessMap,
            ],
        }
    }

    pub fn takes(&self, input: NodeInput) -> bool {
        self.inputs().contains(&input)
    }

    pub fn gives_picture(&self) -> bool {
        !self.is_mask()
    }

    pub fn is_media(&self) -> bool {
        matches!(self, Tool::MediaIn | Tool::MediaOut)
    }

    pub fn multiplies(&self) -> bool {
        !self.is_mask() && !self.is_media()
    }

    pub fn operates(&self) -> bool {
        !matches!(self, Tool::Tracker(_))
    }

    pub fn flows(&self) -> bool {
        self.gives_picture() && self.takes(NodeInput::Input)
    }

    pub fn makes(&self) -> bool {
        self.gives_picture() && !self.takes(NodeInput::Input)
    }

    pub fn gives_from(&self, came: &[bool]) -> bool {
        match self {
            t if t.makes() => true,
            t if !t.gives_picture() => false,
            _ => came.first().copied().unwrap_or(false),
        }
    }

    pub fn feeds(&self, input: NodeInput) -> bool {
        match input.carries_picture() {
            true => self.gives_picture(),
            false => self.is_mask(),
        }
    }

    pub fn tracker(&self) -> Option<&Tracker> {
        match self {
            Tool::Tracker(t) => Some(t),
            _ => None,
        }
    }

    pub fn tracker_mut(&mut self) -> Option<&mut Tracker> {
        match self {
            Tool::Tracker(t) => Some(t),
            _ => None,
        }
    }

    pub fn mask(&self) -> Option<&Mask> {
        match self {
            Tool::Ellipse(m) | Tool::Rectangle(m) | Tool::Polygon(m) => Some(m),
            _ => None,
        }
    }

    pub fn merge(&self) -> Option<&Merge> {
        match self {
            Tool::Merge(m) => Some(m),
            _ => None,
        }
    }

    pub fn text_plus(&self) -> Option<&TextPlus> {
        match self {
            Tool::TextPlus(t) => Some(t),
            _ => None,
        }
    }

    pub fn background(&self) -> Option<&Background> {
        match self {
            Tool::Background(b) => Some(b),
            _ => None,
        }
    }

    pub fn noise(&self) -> Option<&FastNoise> {
        match self {
            Tool::FastNoise(f) => Some(f),
            _ => None,
        }
    }

    pub fn mask_mut(&mut self) -> Option<&mut Mask> {
        match self {
            Tool::Ellipse(m) | Tool::Rectangle(m) | Tool::Polygon(m) => Some(m),
            _ => None,
        }
    }

    pub fn made(&self) -> Tool {
        match self {
            Tool::MediaIn => Tool::MediaIn,
            Tool::MediaOut => Tool::MediaOut,
            Tool::Blur(_) => Tool::Blur(Blur::default()),
            Tool::Effect(kind) => Tool::Effect(EffectType::of(kind).made().kind),
            Tool::Ellipse(_) => Tool::Ellipse(Mask::default()),
            Tool::Rectangle(_) => Tool::Rectangle(Mask::default()),
            Tool::Polygon(m) => Tool::Polygon(Mask {
                points: m.points.clone(),
                ..Mask::default()
            }),
            Tool::Tracker(t) => Tool::Tracker(Tracker {
                patterns: t
                    .patterns
                    .iter()
                    .map(|p| Pattern {
                        offset: None,
                        ..p.clone()
                    })
                    .collect(),
            }),
            Tool::Merge(_) => Tool::Merge(Merge::default()),
            Tool::TextPlus(_) => Tool::TextPlus(TextPlus::default()),
            Tool::Background(_) => Tool::Background(Background::default()),
            Tool::FastNoise(_) => Tool::FastNoise(FastNoise::default()),
        }
    }
}

impl EffectType {
    pub fn of(kind: &EffectKind) -> EffectType {
        match kind {
            EffectKind::GaussianBlur(_) => EffectType::GaussianBlur,
            EffectKind::MosaicBlur(_) => EffectType::MosaicBlur,
            EffectKind::Glow(_) => EffectType::Glow,
            EffectKind::FilmGrain(_) => EffectType::FilmGrain,
            EffectKind::Vignette(_) => EffectType::Vignette,
            EffectKind::Sharpen(_) => EffectType::Sharpen,
        }
    }
}

impl Node {
    pub fn new(name: &str, tool: Tool, place: [f64; 2]) -> Self {
        Self {
            name: name.to_string(),
            tool,
            input: None,
            foreground: None,
            effect_mask: None,
            detail_map: None,
            brightness_map: None,
            pass_through: false,
            blend: None,
            blur_alpha: true,
            multiply_by_mask: false,
            place,
            keys: Vec::new(),
            connect: None,
        }
    }

    pub fn mixed(&self) -> f64 {
        self.blend.unwrap_or(1.0)
    }

    pub fn usual_point(&self, value: EffectValue) -> Option<[f64; 2]> {
        let usual = Node::new(&self.name, self.tool.made(), self.place);
        usual.read_point(value).map(|(p, _)| p)
    }

    pub fn center_to_usual(&mut self) {
        if let Some(to) = self.usual_point(EffectValue::Center) {
            self.write_point(EffectValue::Center, to);
        }
    }

    pub fn join(&self, input: NodeInput) -> Option<&str> {
        match input {
            NodeInput::Input => self.input.as_deref(),
            NodeInput::EffectMask => self.effect_mask.as_deref(),
            NodeInput::Foreground => self.foreground.as_deref(),
            NodeInput::DetailMap => self.detail_map.as_deref(),
            NodeInput::BrightnessMap => self.brightness_map.as_deref(),
        }
    }

    pub fn join_mut(&mut self, input: NodeInput) -> &mut Option<String> {
        match input {
            NodeInput::Input => &mut self.input,
            NodeInput::EffectMask => &mut self.effect_mask,
            NodeInput::Foreground => &mut self.foreground,
            NodeInput::DetailMap => &mut self.detail_map,
            NodeInput::BrightnessMap => &mut self.brightness_map,
        }
    }

    pub fn joins(&self, name: &str) -> bool {
        NodeInput::ALL.iter().any(|i| self.join(*i) == Some(name))
    }

    pub fn pictures(&self) -> impl Iterator<Item = Option<&str>> {
        let inputs = self.tool.inputs().iter().filter(|i| i.carries_picture());
        inputs.map(|i| self.join(*i))
    }
}

impl NodeInput {
    pub const ALL: [NodeInput; 5] = [
        NodeInput::Input,
        NodeInput::EffectMask,
        NodeInput::Foreground,
        NodeInput::DetailMap,
        NodeInput::BrightnessMap,
    ];

    pub fn carries_picture(self) -> bool {
        match self {
            NodeInput::Input | NodeInput::Foreground => true,
            NodeInput::EffectMask | NodeInput::DetailMap | NodeInput::BrightnessMap => false,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NodeInput::Input => "picture input",
            NodeInput::EffectMask => "effect mask input",
            NodeInput::Foreground => "foreground input",
            NodeInput::DetailMap => "detail map input",
            NodeInput::BrightnessMap => "brightness map input",
        }
    }
}

pub fn cleaned(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    kept.trim_start_matches(|c: char| c.is_ascii_digit())
        .to_string()
}

pub fn named_well(name: &str) -> bool {
    !name.is_empty() && cleaned(name) == name
}

impl Default for Composition {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            loop_play: true,
            ping_pong: false,
            start: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frames {
    pub clock: FileClock,
    pub zero: i64,
    pub first: i64,
    pub last: i64,
    pub mark_in: i64,
    pub mark_out: i64,
    start: i64,
    end: i64,
    content: i64,
}

impl Frames {
    pub fn of(p: &Project, c: &Clip) -> Self {
        let clock = FileClock::of(p, c);
        let content = c.content_at(c.start);
        let opening = clock.frame(content);
        let closing = clock.frame(c.content_at(c.end() - 1));
        let zero = c
            .picture
            .as_ref()
            .and_then(|x| x.composition.as_ref())
            .map_or(opening, |x| x.start);
        let (first, last) = match p.source_range(c) {
            Some(have) if have.end > have.start => {
                (clock.file(have.start), clock.file_end(have.end) - 1)
            }
            _ => (opening.min(closing), opening.max(closing)),
        };
        let (mark_in, mark_out) = (opening.min(closing) - zero, opening.max(closing) - zero);
        Self {
            clock,
            zero,
            first: (first - zero).min(mark_in),
            last: (last - zero).max(mark_out),
            mark_in,
            mark_out,
            start: c.start,
            end: c.end(),
            content,
        }
    }

    pub fn at(&self, t: i64) -> i64 {
        self.clock.frame(self.content + t - self.start) - self.zero
    }

    pub fn time(&self, f: i64) -> i64 {
        let (a, b) = (self.start, (self.end - 1).max(self.start));
        let rising = self.at(b) >= self.at(a);
        let below = |t: i64| {
            if rising {
                self.at(t) < f
            } else {
                self.at(t) > f
            }
        };
        let (mut lo, mut hi) = (a, b);
        if !below(lo) {
            return lo;
        }
        if below(hi) {
            return hi;
        }
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if below(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        match (self.at(lo) - f).abs() < (self.at(hi) - f).abs() {
            true => lo,
            false => hi,
        }
    }

    pub fn inside(&self, f: i64) -> bool {
        (self.mark_in..=self.mark_out).contains(&f)
    }

    pub fn file(&self, f: i64) -> i64 {
        f + self.zero
    }
}

impl Composition {
    pub const STEP: [f64; 2] = [1.0, 1.0];

    pub fn plain() -> Self {
        let mut out = Node::new(MEDIA_OUT, Tool::MediaOut, [3.0, 0.0]);
        out.input = Some(MEDIA_IN.to_string());
        Self {
            nodes: vec![Node::new(MEDIA_IN, Tool::MediaIn, [0.0, 0.0]), out],
            ..Self::default()
        }
    }

    pub fn node(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name == name)
    }

    pub fn free_name(&self, base: &str) -> String {
        (1..)
            .map(|n| format!("{base}{n}"))
            .find(|name| self.node(name).is_none())
            .unwrap_or_default()
    }

    pub fn copy_name(&self, of: &str) -> String {
        (1..)
            .map(|n| format!("{of}_{n}"))
            .find(|name| self.node(name).is_none())
            .unwrap_or_default()
    }

    pub fn cut_at(&mut self, input: &str) {
        let mut kept: HashSet<String> = HashSet::new();
        let mut next = vec![input.to_string()];
        while let Some(name) = next.pop() {
            let Some(n) = self.node(&name) else {
                continue;
            };
            if !kept.insert(name) {
                continue;
            }
            next.extend(
                NodeInput::ALL
                    .iter()
                    .filter_map(|i| n.join(*i))
                    .map(str::to_string),
            );
            next.extend(n.connect.as_ref().map(|c| c.tracker.clone()));
        }
        self.nodes
            .retain(|n| n.tool == Tool::MediaOut || kept.contains(&n.name));
        if let Some(out) = self.nodes.iter_mut().find(|n| n.tool == Tool::MediaOut) {
            out.input = Some(input.to_string());
        }
    }

    pub fn out(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| n.tool == Tool::MediaOut)
    }

    pub fn pattern(&self, c: &Connect) -> Option<&Pattern> {
        let t = self.node(&c.tracker)?.tool.tracker()?;
        t.patterns.get((c.pattern as usize).checked_sub(1)?)
    }

    pub fn followed(&self) -> Vec<(&str, u32, &MotionPath)> {
        let connects = self.nodes.iter().filter_map(|n| n.connect.as_ref());
        let path = |k: &Connect| self.pattern(k)?.path.as_ref();
        connects
            .filter_map(|k| Some((k.tracker.as_str(), k.pattern, path(k)?)))
            .collect()
    }

    pub fn hold(&mut self, frame: Option<f64>) {
        let connects = self.nodes.iter().filter_map(|n| n.connect.as_ref());
        let on: Vec<(String, u32)> = connects.map(|k| (k.tracker.clone(), k.pattern)).collect();
        for n in &mut self.nodes {
            let name = n.name.clone();
            let Tool::Tracker(t) = &mut n.tool else {
                continue;
            };
            for (i, pattern) in t.patterns.iter_mut().enumerate() {
                let followed = on.iter().any(|(t, k)| *t == name && *k as usize == i + 1);
                let path = pattern.path.take().filter(|_| followed);
                pattern.path = match frame {
                    Some(frame) => path.map(|m| m.held(frame)),
                    None => path,
                };
            }
        }
    }

    pub fn sampled(&self, at: f64, frame: f64) -> Composition {
        let mut nodes: Vec<Node> = self.nodes.iter().map(|n| n.sampled(at)).collect();
        for n in &mut nodes {
            let to = n.connect.as_ref().and_then(|c| self.pattern(c));
            if let Some(to) = to.and_then(|p| p.offset_position_at(frame)) {
                n.write_point(EffectValue::Center, to);
            }
        }
        Composition {
            nodes,
            loop_play: self.loop_play,
            ping_pong: self.ping_pong,
            start: self.start,
        }
    }

    pub fn shows(&self) -> bool {
        let Some((order, _)) = self.picture_order() else {
            return false;
        };
        let mut gives = vec![false; self.nodes.len()];
        for i in order {
            let n = &self.nodes[i];
            let came: Vec<bool> = n
                .pictures()
                .map(|from| {
                    from.and_then(|name| self.index(name))
                        .is_some_and(|j| gives[j])
                })
                .collect();
            if n.tool == Tool::MediaOut {
                return came.first().copied().unwrap_or(false);
            }
            gives[i] = n.tool.gives_from(&came);
        }
        false
    }

    pub fn picture_order(&self) -> Option<(Vec<usize>, Vec<usize>)> {
        if self.loops().is_some() {
            return None;
        }
        let out = self.nodes.iter().position(|n| n.tool == Tool::MediaOut)?;
        let mut uses = vec![0; self.nodes.len()];
        let mut seen = vec![false; self.nodes.len()];
        let mut order = Vec::new();
        let mut stack = vec![(out, false)];
        while let Some((i, done)) = stack.pop() {
            if done {
                order.push(i);
                continue;
            }
            if seen[i] {
                continue;
            }
            seen[i] = true;
            stack.push((i, true));
            let from = self.nodes[i]
                .pictures()
                .flatten()
                .filter_map(|n| self.index(n));
            for j in from.collect::<Vec<_>>().into_iter().rev() {
                uses[j] += 1;
                stack.push((j, false));
            }
        }
        Some((order, uses))
    }

    pub fn masks_of(&self, node: &Node) -> Vec<&Node> {
        self.masks_on(node, NodeInput::EffectMask)
    }

    pub fn masks_on(&self, node: &Node, input: NodeInput) -> Vec<&Node> {
        let mut seen = HashSet::new();
        let mut masks = Vec::new();
        let mut next = node.join(input);
        while let Some(m) = next.and_then(|n| self.node(n)) {
            if !m.tool.is_mask() || !seen.insert(m.name.as_str()) {
                break;
            }
            masks.push(m);
            next = m.effect_mask.as_deref();
        }
        masks.reverse();
        masks
    }

    pub fn loops(&self) -> Option<&str> {
        self.nodes.iter().find_map(|start| {
            let mut seen = HashSet::new();
            let mut stack = vec![start.name.as_str()];
            while let Some(name) = stack.pop() {
                let Some(n) = self.node(name) else { continue };
                for next in NodeInput::ALL.iter().filter_map(|i| n.join(*i)) {
                    if next == start.name {
                        return Some(start.name.as_str());
                    }
                    if seen.insert(next) {
                        stack.push(next);
                    }
                }
            }
            None
        })
    }

    pub fn moves_by_itself(&self) -> bool {
        self.nodes.iter().any(|n| match &n.tool {
            Tool::Effect(kind) => crate::model::Effect {
                kind: kind.clone(),
                ..EffectType::of(kind).made()
            }
            .moves_by_itself(),
            _ => false,
        })
    }
}

impl ToolType {
    pub const ADDED: [ToolType; 15] = [
        ToolType::Blur,
        ToolType::GaussianBlur,
        ToolType::MosaicBlur,
        ToolType::Glow,
        ToolType::FilmGrain,
        ToolType::Vignette,
        ToolType::Sharpen,
        ToolType::Ellipse,
        ToolType::Rectangle,
        ToolType::Polygon,
        ToolType::Tracker,
        ToolType::Merge,
        ToolType::TextPlus,
        ToolType::Background,
        ToolType::FastNoise,
    ];

    pub fn made(self) -> Tool {
        let effect = |t: EffectType| Tool::Effect(t.made().kind);
        match self {
            ToolType::Blur => Tool::Blur(Blur::default()),
            ToolType::GaussianBlur => effect(EffectType::GaussianBlur),
            ToolType::MosaicBlur => effect(EffectType::MosaicBlur),
            ToolType::Glow => effect(EffectType::Glow),
            ToolType::FilmGrain => effect(EffectType::FilmGrain),
            ToolType::Vignette => effect(EffectType::Vignette),
            ToolType::Sharpen => effect(EffectType::Sharpen),
            ToolType::Ellipse => Tool::Ellipse(Mask::default()),
            ToolType::Rectangle => Tool::Rectangle(Mask::default()),
            ToolType::Polygon => Tool::Polygon(Mask::default()),
            ToolType::Tracker => Tool::Tracker(Tracker::made()),
            ToolType::Merge => Tool::Merge(Merge::default()),
            ToolType::TextPlus => Tool::TextPlus(TextPlus::default()),
            ToolType::Background => Tool::Background(Background::default()),
            ToolType::FastNoise => Tool::FastNoise(FastNoise::default()),
            ToolType::MediaIn => Tool::MediaIn,
            ToolType::MediaOut => Tool::MediaOut,
        }
    }

    pub fn label(self) -> &'static str {
        self.made().label()
    }
}
