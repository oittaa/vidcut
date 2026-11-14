use std::collections::HashSet;

use crate::model::{
    Blur, BlurClipping, BlurFilter, Composition, EffectKind, EffectType, Mask, Node, Tool, ToolType,
};

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
            follows: None,
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
        }
    }

    pub fn is_mask(&self) -> bool {
        self.mask().is_some()
    }

    pub fn works(&self) -> bool {
        matches!(self, Tool::Blur(_) | Tool::Effect(_))
    }

    pub fn takes_input(&self) -> bool {
        self.works() || matches!(self, Tool::MediaOut)
    }

    pub fn takes_mask(&self) -> bool {
        self.works() || self.is_mask()
    }

    pub fn mask(&self) -> Option<&Mask> {
        match self {
            Tool::Ellipse(m) | Tool::Rectangle(m) | Tool::Polygon(m) => Some(m),
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
            effect_mask: None,
            pass_through: false,
            blend: None,
            blur_alpha: true,
            place,
            keys: Vec::new(),
        }
    }

    pub fn mixed(&self) -> f64 {
        self.blend.unwrap_or(1.0)
    }

    pub fn joins(&self, name: &str) -> bool {
        self.input.as_deref() == Some(name) || self.effect_mask.as_deref() == Some(name)
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
        }
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

    pub fn out(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| n.tool == Tool::MediaOut)
    }

    pub fn chain(&self) -> Option<Vec<&Node>> {
        let mut seen = HashSet::new();
        let mut at = self.node(self.out()?.input.as_deref()?)?;
        let mut tools = Vec::new();
        loop {
            if !seen.insert(at.name.as_str()) {
                return None;
            }
            match &at.tool {
                Tool::MediaIn => break,
                t if t.works() => tools.push(at),
                _ => return None,
            }
            at = self.node(at.input.as_deref()?)?;
        }
        tools.reverse();
        Some(tools)
    }

    pub fn masks_of(&self, node: &Node) -> Vec<&Node> {
        let mut seen = HashSet::new();
        let mut masks = Vec::new();
        let mut next = node.effect_mask.as_deref();
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
                for next in [n.input.as_deref(), n.effect_mask.as_deref()]
                    .into_iter()
                    .flatten()
                {
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
    pub const ADDED: [ToolType; 10] = [
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
            ToolType::MediaIn => Tool::MediaIn,
            ToolType::MediaOut => Tool::MediaOut,
        }
    }

    pub fn label(self) -> &'static str {
        self.made().label()
    }
}
