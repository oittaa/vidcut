use crate::composition::Corner;
use crate::effect_keys::Keyed;
use crate::model::{
    ApplyMode, Background, BackgroundFill, BlendMode, Blur, BlurClipping, BlurFilter, Border,
    CellShape, CompositeMode, Composition, Effect, EffectKind, EffectType, FastNoise, FillMethod,
    FilmGrain, Glow, GlowAlpha, GlowFraming, GlowOutput, GradientRepeat, GradientStop,
    GradientType, Mask, Media, Merge, MergeEdges, MergeOperator, MosaicBlur, Node, NoiseColoring,
    Pattern, Picture, ShadingElement, Sharpen, TextPlus, Tool, Vignette, VignetteMode,
    VignetteShape,
};
use crate::motion::Area;
use crate::render::{Moving, Placed};
use crate::request::NodeInput;

pub type Areas<'a> = dyn Fn(&Pattern) -> Option<Area> + 'a;

const LEAST_ZOOM: f64 = 1e-9;
const LEAST_BAR: f64 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub back: [f64; 8],
    pub shown: [f64; 4],
    pub edge: [f64; 4],
    pub soft: Soft,
    pub opacity: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent {
    pub rect: [f64; 4],
    pub grid: (f64, f64),
}

impl Extent {
    pub fn whole((w, h): (f64, f64)) -> Self {
        Self {
            rect: [0.0, 0.0, w, h],
            grid: (w, h),
        }
    }

    pub fn within(&self, [fx, fy, fw, fh]: [f64; 4]) -> [f64; 4] {
        let [ex, ey, ew, eh] = self.rect;
        [(ex - fx) / fw, (ey - fy) / fh, ew / fw, eh / fh]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Soft {
    pub inner: [f64; 4],
    pub reach: [f64; 2],
}

impl Soft {
    const KNEE: f64 = 0.5;
    const WIDTH: f64 = 7.0 / 8.0;

    fn of([u0, v0, u1, v1]: [f64; 4], softness: f64, [fw, fh]: [f64; 2]) -> ([f64; 4], Self) {
        let ramp = (softness.abs() * fw / 400.0).floor();
        if ramp <= 0.0 || fw <= 0.0 || fh <= 0.0 {
            return ([u0, v0, u1, v1], Self::default());
        }
        let (du, dv) = (1.0 / fw, 1.0 / fh);
        let reach = [ramp * du, ramp * dv];
        let inset = if softness > 0.0 { 0.5 } else { ramp - 0.5 };
        let inner = [
            u0 + inset * du,
            v0 + inset * dv,
            u1 - inset * du,
            v1 - inset * dv,
        ];
        if softness < 0.0 {
            return ([u0, v0, u1, v1], Self { inner, reach });
        }
        let (ou, ov) = ((ramp + 1.0) * du, (ramp + 1.0) * dv);
        let shown = [
            (u0 - ou).max(0.0),
            (v0 - ov).max(0.0),
            (u1 + ou).min(1.0),
            (v1 + ov).min(1.0),
        ];
        (shown, Self { inner, reach })
    }

    pub fn alpha(&self, [u, v]: [f64; 2]) -> f64 {
        let [ru, rv] = self.reach;
        if ru <= 0.0 || rv <= 0.0 {
            return 1.0;
        }
        let [u0, v0, u1, v1] = self.inner;
        let a = (u0 - u).max(u - u1).max(0.0) / ru;
        let b = (v0 - v).max(v - v1).max(0.0) / rv;
        let h = (Self::KNEE - (a - b).abs()).max(0.0) / Self::KNEE;
        let m = a.max(b) + h * h * Self::KNEE / 4.0;
        let t = ((1.0 - m) / Self::WIDTH).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placing {
    pub level: Level,
    pub mode: CompositeMode,
    pub effects: Vec<Working>,
    pub composed: Program,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Working {
    pub work: Work,
    pub blend: f64,
    pub use_alpha: bool,
    pub masks: Vec<Shaping>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outline {
    Ellipse {
        half: [f64; 2],
        turn: f64,
    },
    Rectangle {
        half: [f64; 2],
        turn: f64,
        corner: f64,
    },
    Polygon {
        points: Vec<[f64; 2]>,
        winding: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Softening {
    Gaussian(f64),
    Boxes { width: f64, count: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    pub outline: Outline,
    pub middle: [f64; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskBase {
    pub size: [f64; 2],
    pub round: f64,
    pub turn: f64,
}

pub fn turned([x, y]: [f64; 2], turn: f64) -> [f64; 2] {
    let (s, c) = turn.sin_cos();
    [x * c + y * s, -x * s + y * c]
}

impl MaskBase {
    pub fn whole((w, h): (f64, f64)) -> Self {
        Self {
            size: [w, h],
            round: w,
            turn: 0.0,
        }
    }

    pub fn of(a: &Area, (w, h): (f64, f64)) -> Self {
        let size = [a.size[0] * w, a.size[1] * h];
        Self {
            size,
            round: size[1],
            turn: a.rotation.to_radians(),
        }
    }

    pub fn all(n: &Node, comp: &Composition, (w, h): (f64, f64), areas: &Areas) -> Vec<Self> {
        let Some(c) = &n.connect else {
            return vec![Self::whole((w, h))];
        };
        let Some(a) = comp.pattern(c).and_then(areas) else {
            return Vec::new();
        };
        match c.size {
            true => vec![Self::of(&a, (w, h))],
            false => vec![Self::whole((w, h))],
        }
    }

    pub fn unturned(&self) -> Self {
        Self { turn: 0.0, ..*self }
    }

    pub fn screen(&self, [x, y]: [f64; 2], [mx, my]: [f64; 2]) -> [f64; 2] {
        let [dx, dy] = turned([x - mx, y - my], self.turn);
        [mx + dx, my + dy]
    }

    pub fn own(&self, [x, y]: [f64; 2], [mx, my]: [f64; 2]) -> [f64; 2] {
        let [dx, dy] = turned([x - mx, y - my], -self.turn);
        [mx + dx, my + dy]
    }
}

impl Figure {
    pub const NONE: Self = Self {
        outline: Outline::Polygon {
            points: Vec::new(),
            winding: false,
        },
        middle: [0.0, 0.0],
    };
    const ROUND: u32 = 72;

    pub fn line(&self) -> Vec<[f64; 2]> {
        let at = |[x, y]: [f64; 2], turn: f64| {
            let [dx, dy] = turned([x, y], turn);
            [self.middle[0] + dx, self.middle[1] + dy]
        };
        match &self.outline {
            Outline::Ellipse { half, turn } => (0..=Self::ROUND)
                .map(|i| {
                    let a = f64::from(i) / f64::from(Self::ROUND) * std::f64::consts::TAU;
                    at([half[0] * a.cos(), half[1] * a.sin()], *turn)
                })
                .collect(),
            Outline::Rectangle { half, turn, .. } => [
                [-1.0, -1.0],
                [1.0, -1.0],
                [1.0, 1.0],
                [-1.0, 1.0],
                [-1.0, -1.0],
            ]
            .iter()
            .map(|[x, y]| at([x * half[0], y * half[1]], *turn))
            .collect(),
            Outline::Polygon { points, .. } => {
                let mut out = points.clone();
                out.extend(points.first());
                out
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shaping {
    pub figures: Vec<Figure>,
    pub border: f64,
    pub solid: bool,
    pub level: f64,
    pub invert: bool,
    pub soft: Option<Softening>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Boxing {
    pub across: f64,
    pub down: f64,
    pub count: u32,
    pub channels: [bool; 4],
    pub border: Border,
}

fn boxes_of(filter: BlurFilter, passes: u32) -> Option<u32> {
    match filter {
        BlurFilter::Box => Some(1),
        BlurFilter::Bartlett => Some(2),
        BlurFilter::MultiBox => Some(passes.max(1)),
        BlurFilter::Gaussian | BlurFilter::FastGaussian => None,
    }
}

fn box_width(sigma: f64, count: u32) -> f64 {
    sigma * (12.0 / f64::from(count)).sqrt()
}

impl Shaping {
    pub fn figure(m: &Mask, tool: &Tool, b: &MaskBase, (fw, fh): (f64, f64)) -> Figure {
        let [cx, cy] = m.middle();
        let [w, h] = b.size;
        let own = [cx * fw, (1.0 - cy) * fh];
        let turn = m.turn().to_radians() + b.turn;
        let outline = match tool {
            Tool::Ellipse(_) => Outline::Ellipse {
                half: [m.wide() * w / 2.0, m.high() * b.round / 2.0],
                turn,
            },
            Tool::Rectangle(_) => {
                let half = [m.wide() * w / 2.0, m.high() * h / 2.0];
                Outline::Rectangle {
                    half,
                    turn,
                    corner: m.corner() * half[0].min(half[1]),
                }
            }
            _ => Outline::Polygon {
                points: m
                    .points
                    .iter()
                    .map(|p| b.screen(Self::tilted(m, *p, own, (w, h)), own))
                    .collect(),
                winding: m.fill_method == Some(FillMethod::NonZeroWinding),
            },
        };
        Figure {
            outline,
            middle: own,
        }
    }

    pub fn of(m: &Mask, tool: &Tool, (w, h): (f64, f64), bases: &[MaskBase]) -> Self {
        let s = m.soft();
        let soft = (s > 0.0).then(|| match boxes_of(m.filter(), m.passes()) {
            Some(count) => Softening::Boxes {
                width: box_width(Mask::SOFT_BOXES * s * w, count),
                count,
            },
            None => Softening::Gaussian(match m.filter() {
                BlurFilter::Gaussian => Mask::SOFT_GAUSSIAN * s * w,
                _ => Mask::SOFT * s * w,
            }),
        });
        Self {
            figures: bases
                .iter()
                .map(|b| Self::figure(m, tool, b, (w, h)))
                .collect(),
            border: m.border() * w / 2.0,
            solid: m.solid,
            level: m.level(),
            invert: m.invert,
            soft,
        }
    }

    pub fn tilted(m: &Mask, [x, y]: [f64; 2], [mx, my]: [f64; 2], (w, h): (f64, f64)) -> [f64; 2] {
        let k = m.scale();
        let [z, tx, ty] = m.tilts().map(f64::to_radians);
        let (qx, qy) = (x * w * k, y * h * k);
        let (sz, cz) = z.sin_cos();
        let (rx, ry) = (qx * cz - qy * sz, qx * sz + qy * cz);
        let (sx, cx) = tx.sin_cos();
        let (sy, cy) = ty.sin_cos();
        [mx + rx * cy + ry * sx * sy, my - ry * cx]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Work {
    Boxes(Boxing),
    Blur(Blurring),
    Mosaic(Tiling),
    Glow(Box<Glowing>),
    Grain(Box<Graining>),
    Vignette(Shading),
    Sharpen(Sharpening),
    Grade {
        graded: Box<crate::grade::Graded>,
        back: [f64; 8],
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blurring {
    pub across: [f64; 4],
    pub down: [f64; 4],
    pub border: Border,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tiling {
    pub shape: CellShape,
    pub cell: [f64; 2],
    pub step: f64,
    pub origin: [f64; 2],
    pub reach: [u32; 2],
    pub samples: u32,
    pub blur_alpha: bool,
    pub whole: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlowSpread {
    pub across: [f64; 3],
    pub down: [f64; 3],
    pub grid: [u32; 2],
}

impl GlowSpread {
    const GRID: u32 = 8;
    const MOST_GRID: u32 = 1 << 16;
    const WIDE: f64 = 600.0;

    fn grid(spreads: &[f64; 3]) -> u32 {
        let most = spreads.iter().copied().fold(0.0, f64::max);
        let (mut grid, mut wide) = (Self::GRID, Self::WIDE);
        while most >= wide && grid < Self::MOST_GRID {
            grid *= 2;
            wide *= 2.0;
        }
        grid
    }

    fn of(spread: f64, ratio: f64, relative: [f64; 3], area: f64) -> Self {
        let sigma = Glow::SIGMA * spread * area;
        let across = relative.map(|r| sigma * r * (1.0 + ratio));
        let down = relative.map(|r| sigma * r * (1.0 - ratio));
        Self {
            across,
            down,
            grid: [Self::grid(&across), Self::grid(&down)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlowNoise {
    pub strength: f64,
    pub detail: f64,
    pub cell: [f64; 2],
    pub depth: f64,
}

impl GlowNoise {
    const CELL: f64 = 340.0 / (0.6 * 1440.0);
    const EVOLUTION: f64 = 1.8;
    const SEETHE: f64 = 0.0208;
    const SEETHE_POWER: f64 = 1.9;
    const DEPTHS: f64 = 4096.0;

    fn of(g: &Glow, area: f64, frame: i64) -> Self {
        let cell = Self::CELL * g.noise_scale() * area;
        let moved = frame.max(0) as f64 * Self::SEETHE * g.noise_seethe().powf(Self::SEETHE_POWER);
        Self {
            strength: g.noise_strength(),
            detail: g.noise_detail(),
            cell: [
                cell * (1.0 + g.noise_ratio()),
                cell * (1.0 - g.noise_ratio()),
            ],
            depth: (g.noise_evolution() * Self::EVOLUTION + moved).rem_euclid(Self::DEPTHS),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Glowing {
    pub output: GlowOutput,
    pub alpha: GlowAlpha,
    pub shine: f64,
    pub alpha_shine: f64,
    pub first: GlowSpread,
    pub second: Option<GlowSpread>,
    pub gain: f64,
    pub second_gain: f64,
    pub filter: [f64; 3],
    pub second_color: [f64; 3],
    pub gamma: f64,
    pub saturation: f64,
    pub noise: Option<GlowNoise>,
    pub seen_only: bool,
    pub mode: BlendMode,
    pub opacity: f64,
    pub invert: bool,
    pub recovery: f64,
}

impl Glowing {
    const GAIN: f64 = 4.0;

    fn of(g: &Glow, on: &OnFrame) -> Self {
        let area = (on.size[0] * on.size[1]).sqrt();
        let second = g.secondary.then(|| {
            let wide = Glow::SECOND * g.second_wide();
            GlowSpread::of(wide, g.ratio(), g.relative(), area)
        });
        Self {
            output: g.shows(),
            alpha: g.alpha(),
            shine: g.shine(),
            alpha_shine: g.alpha_shine(),
            first: GlowSpread::of(g.wide(), g.ratio(), g.relative(), area),
            second,
            gain: Self::GAIN * g.bright(),
            second_gain: Self::GAIN * g.second_bright(),
            filter: g.filter(),
            second_color: g.second_color(),
            gamma: g.curve(),
            saturation: g.colorful(),
            noise: g.atmosphere.then(|| GlowNoise::of(g, area, on.frame)),
            seen_only: g.frames() == GlowFraming::OnlyAsSeen,
            mode: g.over(),
            opacity: g.shown(),
            invert: g.invert,
            recovery: g.recovers(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrainTile {
    pub ring: f64,
    pub width: f64,
    pub scale: f64,
    pub aspect: f64,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Graining {
    pub tile: GrainTile,
    pub frame: i64,
    pub amount: f64,
    pub saturation: f64,
    pub curve: [f64; 4],
    pub tails: [f64; 3],
    pub gains: [f64; 3],
    pub offset: f64,
    pub softness: f64,
    pub detail: f64,
    pub tones: [f64; 3],
    pub opacity: f64,
    pub mode: BlendMode,
    pub only: bool,
}

fn between<const N: usize>(rows: &[[f64; N]], x: f64) -> [f64; N] {
    let i = rows.partition_point(|r| r[0] <= x).clamp(1, rows.len() - 1);
    let (a, b) = (&rows[i - 1], &rows[i]);
    let t = ((x - a[0]) / (b[0] - a[0])).clamp(0.0, 1.0);
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

impl Graining {
    const AMOUNT: f64 = 1.334 * 0.1986;
    const HIGH: f64 = 1080.0;
    const WIDE: f64 = 1920.0;
    const SOFT: f64 = 5.0;
    const DETAIL: f64 = 4.32;
    const SHIFT: f64 = 0.13;
    const NEAR: f64 = 0.09;
    const TEXTURES: [[f64; 3]; 12] = [
        [0.0, 0.1416, 0.2009],
        [0.25, 0.0021, 0.2106],
        [0.5, 0.0023, 0.1823],
        [0.6, 0.1173, 0.1407],
        [0.65, 0.1396, 0.1239],
        [0.704, 0.1615, 0.1044],
        [0.75, 0.1768, 0.0855],
        [0.8, 0.1864, 0.0675],
        [0.85, 0.1924, 0.0552],
        [0.9, 0.1948, 0.0433],
        [0.95, 0.1971, 0.0323],
        [1.0, 0.1989, 0.0212],
    ];
    const SIZES: [[f64; 2]; 12] = [
        [0.0, 0.4120],
        [0.1, 0.4606],
        [0.2156, 0.5003],
        [0.25, 0.5402],
        [0.4, 0.6505],
        [0.6, 0.9114],
        [0.7, 1.1413],
        [0.8, 1.5220],
        [0.85, 1.8248],
        [0.9, 2.2788],
        [0.95, 3.0311],
        [1.0, 4.5242],
    ];
    const SYMMETRIES: [[f64; 5]; 30] = [
        [0.0, -0.07744, 0.6358, -1.06414, 0.34923],
        [0.04, -0.07739, 0.64081, -0.9797, 0.44705],
        [0.08, -0.0772, 0.65078, -0.86977, 0.25285],
        [0.12, -0.07669, 0.66729, -0.80699, -0.88817],
        [0.16, -0.07646, 0.68175, -0.6498, -1.37177],
        [0.2, -0.07522, 0.71945, -0.6292, -5.30187],
        [0.24, -0.07351, 0.76562, -0.61471, -9.55647],
        [0.28, -0.07081, 0.81804, -0.72264, -13.95116],
        [0.32, -0.06693, 0.86831, -0.98022, -16.54602],
        [0.36, -0.06155, 0.90966, -1.2118, -15.31277],
        [0.38, -0.05757, 0.91704, -1.34923, -10.89731],
        [0.42, -0.05541, 0.86577, -0.12053, -0.17395],
        [0.44, -0.04579, 0.90897, -0.04096, 0.35627],
        [0.46, -0.03272, 0.95123, -0.08668, 0.91194],
        [0.48, -0.01564, 1.00156, -0.10991, -1.77354],
        [0.5, 0.0, 1.0, 0.0, 0.0],
        [0.52, 0.01956, 1.00131, 0.12804, -1.93273],
        [0.54, 0.03684, 0.94974, 0.0717, 1.2161],
        [0.56, 0.05007, 0.90897, 0.03813, 0.28897],
        [0.58, 0.05985, 0.86518, 0.12339, -0.2245],
        [0.62, 0.06091, 0.87297, 1.62573, -3.21686],
        [0.66, 0.06804, 0.86244, 1.51316, -12.42271],
        [0.7, 0.07347, 0.83585, 1.04878, -15.03239],
        [0.74, 0.07704, 0.78627, 0.77186, -11.99199],
        [0.78, 0.07925, 0.74308, 0.70712, -8.5182],
        [0.82, 0.08084, 0.69763, 0.68734, -3.58237],
        [0.86, 0.08169, 0.67401, 0.76786, -2.2161],
        [0.9, 0.08217, 0.65444, 0.8672, -0.72808],
        [0.94, 0.08259, 0.63968, 0.93542, 0.30383],
        [1.0, 0.0828, 0.62865, 1.05907, 0.47028],
    ];
    const TAILS: [[f64; 3]; 4] = [
        [0.0, 1.0, 1.0],
        [0.2, 0.594, 0.845],
        [0.3, 0.486, 0.847],
        [0.4, 0.395, 0.842],
    ];

    fn of(g: &FilmGrain, on: &OnFrame) -> Self {
        let [_, ring, width] = between(&Self::TEXTURES, g.rough());
        let sizes = Self::SIZES.map(|[s, k]| [s, k.ln()]);
        let scale = between(&sizes, g.large())[1].exp() * on.fit[3] / Self::HIGH;
        let [across, down] = on.canvas;
        let tall = down / Self::HIGH;
        let spread = (across * down / (Self::WIDE * Self::HIGH)).sqrt();
        let seed = g.rough().to_bits()
            ^ g.large().to_bits().rotate_left(17)
            ^ g.stretch().to_bits().rotate_left(34)
            ^ 1
            ^ (u64::from(g.animate) << 1);
        let lean = g.lean();
        let [_, c0, c1, c2, c3] = between(&Self::SYMMETRIES, lean);
        let strong = g.strong();
        let [_, steep, gentle] = between(&Self::TAILS, (lean - 0.5).abs());
        let (low, high) = if lean < 0.5 {
            (gentle, steep)
        } else {
            (steep, gentle)
        };
        Self {
            tile: GrainTile {
                ring,
                width,
                scale,
                aspect: g.stretch(),
                seed,
            },
            frame: if g.freeze { 0 } else { on.frame },
            amount: Self::AMOUNT * strong,
            saturation: g.colorful(),
            curve: [c0 * (1.0 + Self::SHIFT * strong * strong), c1, c2, c3],
            tails: [low, high, Self::NEAR],
            gains: g.gains(),
            offset: g.middle(),
            softness: Self::SOFT * g.soft().powf(1.5) * tall,
            detail: Self::DETAIL * g.lost() * g.lost() * spread,
            tones: g.tones(),
            opacity: g.shown(),
            mode: g.over().mode(),
            only: g.grain_only,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shading {
    pub shape: VignetteShape,
    pub radius: f64,
    pub stretch: f64,
    pub turn: f64,
    pub cells: [u32; 2],
    pub boxes: [u32; 3],
    pub first: [f64; 2],
    pub color: [f64; 3],
    pub strength: f64,
    pub mode: BlendMode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sharpening {
    pub size: f64,
    pub light: [f64; 3],
    pub color: [f64; 3],
}

impl Sharpening {
    fn of(s: &Sharpen) -> Self {
        let bands = [s.fine(), s.medium(), s.large()];
        let chroma = s.chroma();
        Self {
            size: s.size(),
            light: bands.map(|g| s.amount() * g - 1.0),
            color: match chroma == Sharpen::GAIN {
                true => [0.0; 3],
                false => bands.map(|g| chroma * g - 1.0),
            },
        }
    }

    pub fn colors(&self) -> bool {
        self.color.iter().any(|k| *k != 0.0)
    }

    fn still(&self) -> bool {
        !self.colors() && self.light.iter().all(|k| *k == 0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OnFrame {
    width: f64,
    size: [f64; 2],
    middle: [f64; 2],
    turn: f64,
    frame: i64,
    fit: [f64; 4],
    canvas: [f64; 2],
    back: [f64; 8],
}

impl Tiling {
    const LEAST: f64 = 1e-3;
    const MOST_REACH: f64 = 1e9;

    fn of(m: &MosaicBlur, on: &OnFrame) -> Self {
        let samples = if m.antialias { 2.0 } else { 1.0 };
        let [w, h] = on.size;
        let [mx, my] = on.middle;
        let high = (samples * MosaicBlur::CELL * (w * h).sqrt() / m.frequency()).max(Self::LEAST);
        let cell = [(high * m.stretch()).max(Self::LEAST), high];
        let step = match m.shape() {
            CellShape::Square => cell[0],
            CellShape::Hexagon => (cell[0] * 3.0_f64.sqrt() / 2.0).max(Self::LEAST),
            CellShape::Triangle => (cell[0] / 3.0_f64.sqrt()).max(Self::LEAST),
        };
        let [px, py] = m.place();
        let [dx, dy] = m.offsets();
        let (ax, ay) = ((px - 0.5) * w, (0.5 - py) * h);
        let (sin, cos) = on.turn.to_radians().sin_cos();
        let (x, y) = (mx + cos * ax + sin * ay, my - sin * ax + cos * ay);
        let origin = [
            (samples * x + dx * 0.5 * cell[0]).rem_euclid(2.0 * step),
            (samples * y - dy * 0.5 * cell[1]).rem_euclid(2.0 * cell[1]),
        ];
        let reach = cell.map(|c| (m.box_size() * c / 2.0 + 1e-9).min(Self::MOST_REACH) as u32);
        Self {
            shape: m.shape(),
            cell,
            step,
            origin,
            reach,
            samples: samples as u32,
            blur_alpha: m.blurs_alpha(),
            whole: m.whole_cells,
        }
    }
}

impl Shading {
    const SPREAD: f64 = 966.0 / 1080.0;
    const RADIUS: f64 = 0.7;
    const LEAST_SIZE: f64 = 0.01;
    const LEAST_SOFT: f64 = 0.05;
    const CELL: f64 = 2.0;
    const EXACT: f64 = 1e-6;

    fn boxes(sigma: f64) -> [u32; 3] {
        let wide = 12.0 * sigma * sigma;
        let floor = (wide / 3.0 + 1.0).sqrt().floor() as u32;
        let low = if floor.is_multiple_of(2) {
            floor - 1
        } else {
            floor
        };
        let l = f64::from(low);
        let narrow = ((wide - 3.0 * l * l - 12.0 * l - 9.0) / (-4.0 * l - 4.0))
            .round_ties_even()
            .clamp(0.0, 3.0) as usize;
        std::array::from_fn(|i| if i + narrow < 3 { low + 2 } else { low })
    }

    fn of(v: &Vignette, on: &OnFrame) -> Self {
        let [x, y, fw, fh] = on.fit;
        let (w, h) = ((fw - Self::EXACT).ceil(), (fh - Self::EXACT).ceil());
        let advanced = v.mode() == VignetteMode::Advanced;
        let [cx, cy] = if advanced {
            v.middle()
        } else {
            Vignette::MIDDLE
        };
        let size = v.large();
        let soft = v.soft();
        let sigma = Self::SPREAD * h * size.sqrt() * (Self::LEAST_SOFT + soft * soft);
        Self {
            shape: if advanced {
                v.shape()
            } else {
                VignetteShape::Oval
            },
            radius: Self::RADIUS * (size + Self::LEAST_SIZE) * h,
            stretch: v.stretch(),
            turn: if advanced { v.turn() } else { 0.0 },
            cells: [w as u32, h as u32],
            boxes: Self::boxes(sigma / Self::CELL),
            first: [
                (x + fw / 2.0).floor() + (cx - 0.5) * w - w,
                (y + fh / 2.0).floor() + (0.5 - cy) * h - h,
            ],
            color: v.tint(),
            strength: if advanced { 1.0 - v.through() } else { 1.0 },
            mode: if advanced {
                v.mixing()
            } else {
                BlendMode::Normal
            },
        }
    }
}

impl Working {
    fn of(e: &Effect, on: &OnFrame) -> Option<Self> {
        if !e.on || e.mixed() <= 0.0 {
            return None;
        }
        let work = match &e.kind {
            EffectKind::GaussianBlur(b) => {
                let (across, down) = b.spreads(on.width);
                let still = across.iter().chain(&down).all(|s| *s <= 0.0);
                if still && e.use_alpha {
                    return None;
                }
                Work::Blur(Blurring {
                    across,
                    down,
                    border: b.edge(),
                })
            }
            EffectKind::MosaicBlur(m) => Work::Mosaic(Tiling::of(m, on)),
            EffectKind::Glow(g) => Work::Glow(Box::new(Glowing::of(g, on))),
            EffectKind::FilmGrain(g) => Work::Grain(Box::new(Graining::of(g, on))),
            EffectKind::Vignette(v) => Work::Vignette(Shading::of(v, on)),
            EffectKind::Sharpen(s) => {
                let sharpening = Sharpening::of(s);
                if sharpening.still() && e.use_alpha {
                    return None;
                }
                Work::Sharpen(sharpening)
            }
        };
        Some(Self {
            work,
            blend: e.mixed(),
            use_alpha: e.use_alpha,
            masks: Vec::new(),
        })
    }

    fn plain(work: Work, blend: f64) -> Self {
        Self {
            work,
            blend,
            use_alpha: true,
            masks: Vec::new(),
        }
    }

    pub fn graded(g: &crate::model::Grade, back: [f64; 8]) -> Option<Self> {
        crate::grade::Graded::of(g).map(|graded| Self {
            work: Work::Grade {
                graded: Box::new(graded),
                back,
            },
            blend: 1.0,
            use_alpha: false,
            masks: Vec::new(),
        })
    }

    fn of_blur(b: &Blur, blend: f64, w: f64) -> Option<Self> {
        let [x, y] = [b.across(), b.down()].map(|s| s * w);
        let channels = b.channels();
        if blend <= 0.0 || x + y <= 0.0 || !channels.contains(&true) {
            return None;
        }
        let border = match b.clipping() {
            BlurClipping::None => Border::Black,
            BlurClipping::Domain | BlurClipping::Frame => Border::Replicate,
        };
        let work = match boxes_of(b.filter(), b.passes()) {
            Some(count) => {
                let wide = match b.filter() {
                    BlurFilter::Box => Blur::BOX,
                    BlurFilter::Bartlett => Blur::BARTLETT,
                    _ => box_width(Blur::FAST, count),
                };
                Work::Boxes(Boxing {
                    across: wide * x,
                    down: wide * y,
                    count,
                    channels,
                    border,
                })
            }
            None => {
                let k = match b.filter() {
                    BlurFilter::Gaussian => Blur::GAUSSIAN,
                    _ => Blur::FAST,
                };
                let on = |s: f64| channels.map(|c| if c { k * s } else { 0.0 });
                Work::Blur(Blurring {
                    across: on(x),
                    down: on(y),
                    border,
                })
            }
        };
        Some(Self::plain(work, blend))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Program {
    pub stages: Vec<Stage>,
    pub out: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub op: Op,
    pub from: Vec<usize>,
    pub to: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Clear,
    Work(Working),
    Merge(Merging),
    Text(Box<Inking>),
    Make(Box<Making>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Making {
    Background(Painting),
    Noise(Noising),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Noising {
    pub middle: [f64; 2],
    pub cells: [f64; 2],
    pub turn: f64,
    pub detail: f64,
    pub depth: f64,
    pub brightness: f64,
    pub contrast: f64,
    pub discontinuous: bool,
    pub inverted: bool,
    pub coloring: NoiseColoring,
    pub colors: [[f64; 4]; 2],
    pub stops: Vec<GradientStop>,
    pub maps: [bool; 2],
}

impl Noising {
    pub fn of(f: &FastNoise, (w, h): (f64, f64), frame: i64, maps: [bool; 2]) -> Self {
        let [cx, cy] = f.middle();
        let [sx, sy] = f.scales();
        Self {
            middle: [cx * w, (1.0 - cy) * h],
            cells: [sx / w, sy / w],
            turn: f.turn().to_radians(),
            detail: f.layers(),
            depth: f.depth(frame),
            brightness: f.lift(),
            contrast: f.spread(),
            discontinuous: f.discontinuous,
            inverted: f.inverted,
            coloring: f.colored(),
            colors: [f.first(), f.second()],
            stops: f.colors().to_vec(),
            maps,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Painting {
    pub fill: BackgroundFill,
    pub corners: [[f64; 4]; 4],
    pub form: GradientType,
    pub line: [[f64; 2]; 2],
    pub stops: Vec<GradientStop>,
    pub offset: f64,
    pub repeat: GradientRepeat,
}

impl Painting {
    pub fn of(b: &Background, (w, h): (f64, f64)) -> Self {
        let pixel = |[x, y]: [f64; 2]| [x * w, (1.0 - y) * h];
        Self {
            fill: b.filled(),
            corners: Corner::ALL.map(|c| b.color(c)),
            form: b.form(),
            line: [pixel(b.from()), pixel(b.to())],
            stops: b.colors().to_vec(),
            offset: b.shift(),
            repeat: b.repeats(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inking {
    pub text: TextPlus,
    pub element: ShadingElement,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merging {
    pub back: Option<[f64; 6]>,
    pub edges: MergeEdges,
    pub mode: ApplyMode,
    pub operator: MergeOperator,
    pub additive: f64,
    pub gain: f64,
    pub burn: f64,
    pub blend: f64,
    pub masks: Vec<Shaping>,
}

impl Merging {
    pub fn of(m: &Merge, blend: f64, (w, h): (f64, f64), masks: Vec<Shaping>) -> Self {
        Self {
            back: m.lookup((w, h)),
            edges: m.edges(),
            mode: m.mode(),
            operator: m.operator(),
            additive: m.additive(),
            gain: m.gain(),
            burn: m.burn(),
            blend,
            masks,
        }
    }
}

impl Merge {
    pub fn lookup(&self, (w, h): (f64, f64)) -> Option<[f64; 6]> {
        let s = self.scale();
        if s <= 0.0 || !s.is_finite() {
            return None;
        }
        let [cx, cy] = self.middle();
        let c = [cx * w, (1.0 - cy) * h];
        let m = [w / 2.0, h / 2.0];
        let (sin, cos) = self.turn().to_radians().sin_cos();
        let [fx, fy] =
            [self.flip_horizontal, self.flip_vertical].map(|f| if f { -1.0 } else { 1.0 });
        let (a, from, to) = match self.invert_transform {
            false => (
                [[fx * cos / s, -fx * sin / s], [fy * sin / s, fy * cos / s]],
                c,
                m,
            ),
            true => (
                [[fx * cos * s, fy * sin * s], [-fx * sin * s, fy * cos * s]],
                m,
                c,
            ),
        };
        let row = |k: usize| {
            [
                a[k][0],
                a[k][1],
                to[k] - a[k][0] * from[0] - a[k][1] * from[1],
            ]
        };
        let [x, y] = [row(0), row(1)];
        Some([x[0], x[1], x[2], y[0], y[1], y[2]])
    }
}

impl Program {
    pub fn clear() -> Self {
        Self {
            stages: vec![Stage {
                op: Op::Clear,
                from: Vec::new(),
                to: 0,
            }],
            out: 0,
        }
    }

    pub fn is_plain(&self) -> bool {
        self.stages.is_empty() && self.out == 0
    }

    pub fn registers(&self) -> usize {
        let used = self
            .stages
            .iter()
            .flat_map(|s| s.from.iter().chain([&s.to]));
        used.chain([&self.out]).max().map_or(1, |r| r + 1)
    }
}

struct Registers {
    left: Vec<usize>,
    free: Vec<usize>,
}

impl Registers {
    fn new(media: bool) -> Self {
        Self {
            left: vec![0],
            free: if media { Vec::new() } else { vec![0] },
        }
    }

    fn hold(&mut self, r: usize, uses: usize) {
        self.left[r] += uses;
    }

    fn read(&mut self, r: usize) -> bool {
        self.left[r] = self.left[r].saturating_sub(1);
        self.left[r] == 0
    }

    fn taken(&mut self) -> usize {
        self.free.pop().unwrap_or_else(|| {
            self.left.push(0);
            self.left.len() - 1
        })
    }

    fn freed(&mut self, r: usize, last: bool) {
        if last {
            self.free.push(r);
        }
    }

    fn written(&mut self, from: usize) -> usize {
        match self.read(from) {
            true => from,
            false => self.taken(),
        }
    }
}

fn masked(
    stages: &mut Vec<Stage>,
    regs: &mut Registers,
    made: usize,
    (shaped, mixed): (Vec<Shaping>, f64),
    size: (f64, f64),
) -> usize {
    if shaped.is_empty() && mixed == 1.0 {
        return made;
    }
    let clear = regs.taken();
    stages.push(Stage {
        op: Op::Clear,
        from: Vec::new(),
        to: clear,
    });
    let out = regs.taken();
    stages.push(Stage {
        op: Op::Merge(Merging::of(&Merge::default(), mixed, size, shaped)),
        from: vec![clear, made],
        to: out,
    });
    regs.freed(clear, true);
    regs.freed(made, true);
    out
}

pub fn composed(c: &Composition, (w, h): (f64, f64), frame: i64, areas: &Areas) -> Program {
    let Some((order, uses)) = c.picture_order() else {
        return Program::clear();
    };
    let on = OnFrame {
        width: 4.0 / 3.0 * (w * h).sqrt(),
        size: [w, h],
        middle: [w / 2.0, h / 2.0],
        turn: 0.0,
        frame,
        fit: [0.0, 0.0, w, h],
        canvas: [w, h],
        back: frame_back((w, h)),
    };
    let masks_on = |n: &Node, input: NodeInput| -> Vec<Shaping> {
        c.masks_on(n, input)
            .into_iter()
            .filter(|m| !m.pass_through)
            .filter_map(|m| {
                let mask = m.tool.mask()?;
                let bases = MaskBase::all(m, c, (w, h), areas);
                Some(Shaping::of(mask, &m.tool, (w, h), &bases))
            })
            .collect()
    };
    let masks = |n: &Node| masks_on(n, NodeInput::EffectMask);
    let work = |n: &Node| {
        let mut working = match &n.tool {
            _ if n.pass_through => return None,
            Tool::Blur(b) => Working::of_blur(b, n.mixed(), w)?,
            Tool::Effect(kind) => {
                let effect = Effect {
                    kind: kind.clone(),
                    blend: n.blend,
                    ..EffectType::of(kind).made()
                };
                let mut working = Working::of(&effect, &on)?;
                if let (Work::Blur(b), false) = (&mut working.work, n.blur_alpha) {
                    b.across[3] = 0.0;
                    b.down[3] = 0.0;
                }
                working
            }
            _ => return None,
        };
        working.masks = masks(n);
        Some(working)
    };
    let media = order.iter().any(|&i| c.nodes[i].tool == Tool::MediaIn);
    let mut regs = Registers::new(media);
    let mut made: Vec<Option<usize>> = vec![None; c.nodes.len()];
    let mut stages = Vec::new();
    let mut out = None;
    for i in order {
        let n = &c.nodes[i];
        let came: Vec<Option<usize>> = n
            .pictures()
            .map(|from| from.and_then(|name| c.index(name)).and_then(|j| made[j]))
            .collect();
        let first = came.first().copied().flatten();
        let gives: Vec<bool> = came.iter().map(Option::is_some).collect();
        let result = match &n.tool {
            Tool::MediaIn if n.pass_through => Some(0),
            Tool::MediaIn => {
                let shaped = masks(n);
                Some(masked(
                    &mut stages,
                    &mut regs,
                    0,
                    (shaped, n.mixed()),
                    (w, h),
                ))
            }
            Tool::MediaOut => {
                out = first;
                None
            }
            Tool::TextPlus(_) if n.pass_through => None,
            Tool::TextPlus(t) => {
                let mut stack = None;
                for element in t.drawn() {
                    let spread = element.soft().map(|s| ShadingElement::spread(s, w));
                    let to = regs.taken();
                    let text = t.clone();
                    stages.push(Stage {
                        op: Op::Text(Box::new(Inking { text, element })),
                        from: Vec::new(),
                        to,
                    });
                    if spread != [0.0; 2] {
                        let work = Work::Blur(Blurring {
                            across: [spread[0]; 4],
                            down: [spread[1]; 4],
                            border: Border::Black,
                        });
                        stages.push(Stage {
                            op: Op::Work(Working::plain(work, 1.0)),
                            from: vec![to],
                            to,
                        });
                    }
                    stack = Some(match stack {
                        None => to,
                        Some(back) => {
                            let out = regs.taken();
                            let over = Merging::of(&Merge::default(), 1.0, (w, h), Vec::new());
                            stages.push(Stage {
                                op: Op::Merge(over),
                                from: vec![back, to],
                                to: out,
                            });
                            regs.freed(back, true);
                            regs.freed(to, true);
                            out
                        }
                    });
                }
                let to = stack.unwrap_or_else(|| {
                    let to = regs.taken();
                    stages.push(Stage {
                        op: Op::Clear,
                        from: Vec::new(),
                        to,
                    });
                    to
                });
                let shaped = masks(n);
                Some(masked(
                    &mut stages,
                    &mut regs,
                    to,
                    (shaped, n.mixed()),
                    (w, h),
                ))
            }
            Tool::Background(_) if n.pass_through => None,
            Tool::Background(b) => {
                let to = regs.taken();
                let painting = Painting::of(b, (w, h));
                stages.push(Stage {
                    op: Op::Make(Box::new(Making::Background(painting))),
                    from: Vec::new(),
                    to,
                });
                let how = (masks(n), n.mixed());
                Some(masked(&mut stages, &mut regs, to, how, (w, h)))
            }
            Tool::FastNoise(_) if n.pass_through => None,
            Tool::FastNoise(f) => {
                let maps = [NodeInput::DetailMap, NodeInput::BrightnessMap].map(|input| {
                    let shaped = masks_on(n, input);
                    if shaped.is_empty() {
                        return None;
                    }
                    let white = regs.taken();
                    let lit = Background {
                        top_left: Some([1.0; 4]),
                        ..Background::default()
                    };
                    stages.push(Stage {
                        op: Op::Make(Box::new(Making::Background(Painting::of(&lit, (w, h))))),
                        from: Vec::new(),
                        to: white,
                    });
                    Some(masked(&mut stages, &mut regs, white, (shaped, 1.0), (w, h)))
                });
                let to = regs.taken();
                let from: Vec<usize> = maps.iter().flatten().copied().collect();
                let noising = Noising::of(f, (w, h), frame, maps.map(|m| m.is_some()));
                stages.push(Stage {
                    op: Op::Make(Box::new(Making::Noise(noising))),
                    from: from.clone(),
                    to,
                });
                for r in from {
                    regs.freed(r, true);
                }
                let how = (masks(n), n.mixed());
                Some(masked(&mut stages, &mut regs, to, how, (w, h)))
            }
            t if !t.gives_from(&gives) => None,
            Tool::Merge(m) => first.map(|b| match came.get(1).copied().flatten() {
                Some(f) if !n.pass_through => {
                    let last = [regs.read(b), regs.read(f)];
                    let to = regs.taken();
                    let merging = Merging::of(m, n.mixed(), (w, h), masks(n));
                    stages.push(Stage {
                        op: Op::Merge(merging),
                        from: vec![b, f],
                        to,
                    });
                    regs.freed(b, last[0] && f != b);
                    regs.freed(f, last[1]);
                    to
                }
                other => {
                    regs.read(b);
                    if let Some(f) = other {
                        let last = regs.read(f);
                        regs.freed(f, last && f != b);
                    }
                    b
                }
            }),
            _ => first.map(|r| match work(n) {
                None => {
                    regs.read(r);
                    r
                }
                Some(working) => {
                    let to = regs.written(r);
                    stages.push(Stage {
                        op: Op::Work(working),
                        from: vec![r],
                        to,
                    });
                    to
                }
            }),
        };
        if let Some(r) = result {
            regs.hold(r, uses[i]);
        }
        made[i] = result;
    }
    match out {
        Some(out) => Program { stages, out },
        None => Program::clear(),
    }
}

pub fn flipped(p: &Picture, [x, y]: [f64; 2]) -> [f64; 2] {
    let sx = if p.flip_horizontal { -1.0 } else { 1.0 };
    let sy = if p.flip_vertical { -1.0 } else { 1.0 };
    [0.5 + sx * (x - 0.5), 0.5 + sy * (y - 0.5)]
}

pub fn anchored(p: &Picture, (w, h): (f64, f64), [fx, fy, fw, fh]: [f64; 4]) -> [f64; 2] {
    let ([px, py], [ax, ay]) = (p.place(), p.pivot());
    [fx + ax * fw + (px - 0.5) * w, fy + ay * fh + (py - 0.5) * h]
}

fn whole_pixels(part: f64, pixels: f64) -> f64 {
    (part * pixels + 1e-6).floor() / pixels.max(1.0)
}

fn level(placed: &Placed, frame: (f64, f64), fitted: [f64; 4], extent: Extent) -> Level {
    let sampled = placed.picture.sampled(placed.source as f64);
    let p = &sampled;
    let [_, _, fw, fh] = fitted;
    let [zx, zy] = p.scale().map(|z| z.max(LEAST_ZOOM));
    let [ax, ay] = p.pivot();
    let (sin, cos) = p.turn().to_radians().sin_cos();
    let sx = if p.flip_horizontal { -1.0 } else { 1.0 };
    let sy = if p.flip_vertical { -1.0 } else { 1.0 };
    let [qx, qy] = anchored(p, frame, fitted);
    let (dx, dy) = (zx * (ax - 0.5) * fw, zy * (ay - 0.5) * fh);
    let (ox, oy) = (qx - cos * dx - sin * dy, qy + sin * dx - cos * dy);
    let (k, [u0, v0]) = placed
        .zoomed()
        .map_or((1.0, Picture::MIDDLE), |b| (b.size, flipped(p, b.center)));
    let (a, b) = (k * sx * cos / (zx * fw), -k * sx * sin / (zx * fw));
    let (c, d) = (k * sy * sin / (zy * fh), k * sy * cos / (zy * fh));
    let c0 = p.crop;
    let [cx, cy, cw, ch] = extent.within(fitted);
    let (gw, gh) = extent.grid;
    let crop = [
        cx + whole_pixels(c0.left, gw) * cw,
        cy + whole_pixels(c0.top, gh) * ch,
        cx + (1.0 - whole_pixels(c0.right, gw)) * cw,
        cy + (1.0 - whole_pixels(c0.bottom, gh)) * ch,
    ];
    let gone = p.scale().contains(&0.0) || crop[0] >= crop[2] || crop[1] >= crop[3];
    let cut = |part: f64, grid: f64, at: f64, open: f64| match whole_pixels(part, grid) > 0.0 {
        true => at,
        false => open,
    };
    let cuts = [
        cut(c0.left, gw, crop[0], 0.0),
        cut(c0.top, gh, crop[1], 0.0),
        cut(c0.right, gw, crop[2], 1.0),
        cut(c0.bottom, gh, crop[3], 1.0),
    ];
    let (mut edge, soft) = Soft::of(cuts, p.softness(), [fw, fh]);
    if gone {
        edge = [1.0, 1.0, 0.0, 0.0];
    }
    let affine = [
        a,
        b,
        u0 - a * ox - b * oy,
        c,
        d,
        v0 - c * ox - d * oy,
        0.0,
        0.0,
    ];
    Level {
        back: leaned(affine, [u0, v0], [k * sx, k * sy], p.pitch(), p.yaw()),
        shown: crop,
        edge,
        soft,
        opacity: match gone {
            true => 0.0,
            false => {
                let dissolve = placed.ramp.filter(|r| !r.moved);
                p.opaque() * dissolve.map_or(1.0, |r| r.level(0))
            }
        },
    }
}

fn leaned(
    affine: [f64; 8],
    [u0, v0]: [f64; 2],
    [su, sv]: [f64; 2],
    pitch: f64,
    yaw: f64,
) -> [f64; 8] {
    if pitch == 0.0 && yaw == 0.0 {
        return affine;
    }
    let (g, h) = (-2.0 * yaw / su, 2.0 * pitch / sv);
    let i = 1.0 - g * u0 - h * v0;
    let lean = unit([
        1.0 + u0 * g,
        u0 * h,
        u0 * (i - 1.0),
        v0 * g,
        1.0 + v0 * h,
        v0 * (i - 1.0),
        g,
        h,
        i,
    ]);
    chain(&affine, &lean)
}

fn unit([a, b, e, c, d, f, g, h, i]: [f64; 9]) -> [f64; 8] {
    let s = if i.abs() < 1e-8 {
        if i.is_sign_negative() { -1e-8 } else { 1e-8 }
    } else {
        i
    };
    [a / s, b / s, e / s, c / s, d / s, f / s, g / s, h / s]
}

pub const SAME: [f64; 8] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

pub fn chain(first: &[f64; 8], next: &[f64; 8]) -> [f64; 8] {
    let [a1, b1, e1, c1, d1, f1, g1, h1] = *first;
    let [a2, b2, e2, c2, d2, f2, g2, h2] = *next;
    unit([
        a2 * a1 + b2 * c1 + e2 * g1,
        a2 * b1 + b2 * d1 + e2 * h1,
        a2 * e1 + b2 * f1 + e2,
        c2 * a1 + d2 * c1 + f2 * g1,
        c2 * b1 + d2 * d1 + f2 * h1,
        c2 * e1 + d2 * f1 + f2,
        g2 * a1 + h2 * c1 + g1,
        g2 * b1 + h2 * d1 + h1,
        g2 * e1 + h2 * f1 + 1.0,
    ])
}

pub fn window_back(back: &[f64; 8], [fx, fy, fw, fh]: [f64; 4], (w, h): (f64, f64)) -> [f64; 8] {
    chain(back, &[fw / w, 0.0, fx / w, 0.0, fh / h, fy / h, 0.0, 0.0])
}

pub fn frame_back((w, h): (f64, f64)) -> [f64; 8] {
    [1.0 / w, 0.0, 0.0, 0.0, 1.0 / h, 0.0, 0.0, 0.0]
}

pub fn apply(m: &[f64; 8], [x, y]: [f64; 2]) -> [f64; 2] {
    let [a, b, e, c, d, f, g, h] = *m;
    let w = g * x + h * y + 1.0;
    [(a * x + b * y + e) / w, (c * x + d * y + f) / w]
}

pub fn ahead(m: &[f64; 8], [u, v]: [f64; 2]) -> [f64; 2] {
    let [a, b, e, c, d, f, g, h] = *m;
    let (a, b, c, d) = (a - u * g, b - u * h, c - v * g, d - v * h);
    let det = a * d - b * c;
    let (u, v) = (u - e, v - f);
    [(d * u - b * v) / det, (a * v - c * u) / det]
}

pub fn lettered([px, py]: [f64; 2], zoom: f64, radians: f64, [ox, oy]: [f64; 2]) -> [f64; 8] {
    let (sin, cos) = radians.sin_cos();
    let (a, b, c, d) = (cos / zoom, -sin / zoom, sin / zoom, cos / zoom);
    [
        a,
        b,
        px - a * px - b * py - ox,
        c,
        d,
        py - c * px - d * py - oy,
        0.0,
        0.0,
    ]
}

impl Level {
    const OPEN: f64 = 1e6;

    fn keeps(&self, [fx, fy, fw, fh]: [f64; 4], extent: Extent) -> bool {
        let still = [1.0 / fw, 0.0, -fx / fw, 0.0, 1.0 / fh, -fy / fh, 0.0, 0.0];
        let [cx, cy, cw, ch] = extent.within([fx, fy, fw, fh]);
        let all = [cx, cy, cx + cw, cy + ch];
        let near = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-9);
        near(&self.back, &still) && near(&self.shown, &all)
    }

    pub fn forward(&self, uv: [f64; 2]) -> [f64; 2] {
        ahead(&self.back, uv)
    }

    pub fn open(&mut self) {
        self.edge = [-Self::OPEN, -Self::OPEN, Self::OPEN, Self::OPEN];
        self.soft = Soft::default();
    }

    pub fn centered(from: (u32, u32), to: (u32, u32)) -> Self {
        let (fw, fh) = (f64::from(from.0), f64::from(from.1));
        let (tw, th) = (f64::from(to.0), f64::from(to.1));
        let by = (tw / fw).min(th / fh);
        let (w, h) = (fw * by, fh * by);
        let (x, y) = ((tw - w) / 2.0, (th - h) / 2.0);
        Self {
            back: [1.0 / w, 0.0, -x / w, 0.0, 1.0 / h, -y / h, 0.0, 0.0],
            shown: [0.0, 0.0, 1.0, 1.0],
            edge: [0.0, 0.0, 1.0, 1.0],
            soft: Soft::default(),
            opacity: 1.0,
        }
    }
}

impl Placing {
    pub fn new(placed: &[Placed], frame: (f64, f64), fitted: [f64; 4], pixels: (f64, f64)) -> Self {
        let extent = Extent {
            rect: fitted,
            grid: pixels,
        };
        Self::over(placed, frame, (fitted, pixels), extent)
    }

    pub fn whole(placed: &[Placed], frame: (f64, f64)) -> Self {
        Self::within(placed, frame, Extent::whole(frame))
    }

    pub fn within(placed: &[Placed], frame: (f64, f64), extent: Extent) -> Self {
        Self::over(placed, frame, ([0.0, 0.0, frame.0, frame.1], frame), extent)
    }

    pub fn over(
        placed: &[Placed],
        frame: (f64, f64),
        (fitted, pixels): ([f64; 4], (f64, f64)),
        extent: Extent,
    ) -> Self {
        let p = &placed[0];
        let [zx, zy] = p.picture.scale_at(p.source as f64);
        let mut own = level(p, frame, fitted, extent);
        let unzoomed = p
            .zoomed()
            .map_or(Picture::MIDDLE, |b| flipped(&p.picture, b.center));
        let on = OnFrame {
            width: frame.0 * (zx * zy).abs().sqrt(),
            size: [fitted[2] * zx.abs(), fitted[3] * zy.abs()],
            middle: ahead(&own.back, unzoomed),
            turn: p.picture.turn_at(p.source as f64),
            frame: p.frame,
            fit: fitted,
            canvas: [frame.0, frame.1],
            back: own.back,
        };
        let mut effects: Vec<Working> = p
            .picture
            .effects
            .iter()
            .filter_map(|e| Working::of(&e.sampled(p.source as f64), &on))
            .collect();
        let windows = window_back(&own.back, fitted, frame);
        let graded = (p.picture.grade.as_ref())
            .and_then(|g| Working::graded(&g.at(p.source as f64), windows));
        effects.extend(graded);
        let [u0, v0, u1, v1] = own.edge;
        let quad = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|c| ahead(&own.back, c));
        if u0 < u1 && v0 < v1 && own.soft == Soft::default() && no_bars(quad, frame) {
            own.edge = [-1.0, -1.0, 2.0, 2.0];
        }
        let composed = p
            .picture
            .composition
            .as_ref()
            .map_or_else(Program::default, |c| {
                let frame = p.clock.frame(p.source);
                let sampled = c.sampled((frame - c.start) as f64, frame as f64);
                let areas = |pattern: &Pattern| pattern.area_at(frame as f64);
                composed(&sampled, pixels, p.frame, &areas)
            });
        Self {
            level: own,
            mode: placing_mode(p),
            effects,
            composed,
        }
    }

    pub fn at(&self, x: f64, y: f64) -> Option<([f64; 2], f64)> {
        let l = &self.level;
        let [a, b, e, c, d, f, g, h] = l.back;
        let w = g * x + h * y + 1.0;
        if !w.is_finite() || w.abs() < 1e-12 {
            return None;
        }
        let (u, v) = ((a * x + b * y + e) / w, (c * x + d * y + f) / w);
        if !u.is_finite() || !v.is_finite() {
            return None;
        }
        let (du, dv) = (
            (a - u * g).hypot(b - u * h) / w.abs(),
            (c - v * g).hypot(d - v * h) / w.abs(),
        );
        let [u0, v0, u1, v1] = l.edge;
        let mut cover = l.soft.alpha([u, v]) * l.opacity;
        for edge in [(u - u0) / du, (u1 - u) / du, (v - v0) / dv, (v1 - v) / dv] {
            cover *= (edge + 0.5).clamp(0.0, 1.0);
        }
        (cover > 0.0).then_some(([u, v], cover))
    }

    pub fn step(&self) -> [f64; 4] {
        let [a, b, _, c, d, _, _, _] = self.level.back;
        [a, b, c, d]
    }

    pub fn share(&self, (w, h): (f64, f64)) -> f64 {
        let l = &self.level;
        let [u0, v0, u1, v1] = l.edge;
        let [a, b, _, c, d, _, g, k] = l.back;
        let corners = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|p| ahead(&l.back, p));
        let quad = if g == 0.0 && k == 0.0 {
            (u1 - u0) * (v1 - v0) / (a * d - b * c).abs()
        } else {
            quad_area(corners).unwrap_or(0.0)
        };
        let boxed = self.bounds((w, h)).map_or(0.0, |[x0, y0, x1, y1]| {
            f64::from(x1 - x0) * f64::from(y1 - y0)
        });
        (quad.min(boxed) / (w * h)).clamp(0.0, 1.0)
    }

    pub fn bounds(&self, (w, h): (f64, f64)) -> Option<[u32; 4]> {
        let [u0, v0, u1, v1] = self.level.edge;
        let corners = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|c| ahead(&self.level.back, c));
        let mut out = [0.0, 0.0, w, h];
        if corners.iter().all(|c| c[0].is_finite() && c[1].is_finite()) {
            let xs = corners.map(|c| c[0]);
            let ys = corners.map(|c| c[1]);
            let lo = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min) - 1.0;
            let hi = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max) + 1.0;
            out = [
                lo(xs).max(0.0),
                lo(ys).max(0.0),
                hi(xs).min(w),
                hi(ys).min(h),
            ];
        }
        let [x0, y0, x1, y1] = out.map(|v| v.clamp(0.0, w.max(h)));
        let (x0, y0) = (x0.floor() as u32, y0.floor() as u32);
        let (x1, y1) = (x1.ceil().min(w) as u32, y1.ceil().min(h) as u32);
        (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MixPart {
    Layer(usize),
    Mix(Box<Mix>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mix {
    pub parts: Vec<MixPart>,
    pub placing: Placing,
    pub extent: Extent,
    pub added: bool,
    pub moving: Option<Moving>,
    pub outgoing: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OuterMix {
    pub mix: Mix,
    pub layers: (usize, usize),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mixes {
    pub outer: Vec<OuterMix>,
    pub deepest: usize,
    pub blurs: bool,
    pub moved: usize,
    layers: Vec<bool>,
}

struct Member<'a> {
    index: usize,
    placed: &'a [Placed],
    extent: Extent,
}

impl Member<'_> {
    fn chain(&self) -> &[Placed] {
        self.placed.get(1..).unwrap_or_default()
    }

    fn around(&self, depth: usize) -> Option<&Placed> {
        self.chain().iter().rev().nth(depth)
    }

    fn part(&self) -> MixPart {
        MixPart::Layer(self.index)
    }

    fn shown(&self, frame: (f64, f64)) -> Extent {
        match self.placed.first() {
            Some(own) => kept(own, frame, self.extent.rect, self.extent),
            None => Extent::whole(frame),
        }
    }
}

fn kept(placed: &Placed, frame: (f64, f64), fitted: [f64; 4], extent: Extent) -> Extent {
    let still = placed.moving.is_none() && placed.zoomed().is_none();
    match still && level(placed, frame, fitted, extent).keeps(fitted, extent) {
        true => extent,
        false => Extent::whole(frame),
    }
}

fn parts_of(members: &[Member<'_>], depth: usize, frame: (f64, f64)) -> Vec<(MixPart, Extent)> {
    let mut parts = Vec::new();
    let mut rest = members;
    while let Some(first) = rest.first() {
        let Some(outer) = first.around(depth) else {
            parts.push((first.part(), first.shown(frame)));
            rest = &rest[1..];
            continue;
        };
        let n = rest
            .iter()
            .take_while(|m| m.around(depth) == Some(outer))
            .count();
        let (run, after) = rest.split_at(n);
        let (inner, shown): (Vec<MixPart>, Vec<Extent>) =
            parts_of(run, depth + 1, frame).into_iter().unzip();
        let extent = match shown.as_slice() {
            [only] => *only,
            _ => Extent::whole(frame),
        };
        let mut placing = Placing::within(std::slice::from_ref(outer), frame, extent);
        if let (CompositeMode::Normal, [only]) = (placing.mode, inner.as_slice()) {
            placing.mode = run_mode(only, run);
        }
        let whole = [0.0, 0.0, frame.0, frame.1];
        let out = kept(outer, frame, whole, extent);
        parts.push((
            MixPart::Mix(Box::new(Mix {
                parts: inner,
                placing,
                extent,
                added: outer.crossing,
                moving: outer
                    .moving
                    .as_ref()
                    .zip(outer.ramp)
                    .map(|(m, r)| m.at(&r, 0)),
                outgoing: outer
                    .ramp
                    .filter(|r| r.moved && !outer.crossing)
                    .map(|r| r.falling),
            })),
            out,
        ));
        rest = after;
    }
    parts
}

fn placing_mode(p: &Placed) -> CompositeMode {
    let mode = p.picture.mode();
    if (p.ramp.is_some() || p.crossing) && mode.is_matte() {
        CompositeMode::Normal
    } else {
        mode
    }
}

fn run_mode(part: &MixPart, run: &[Member<'_>]) -> CompositeMode {
    match part {
        MixPart::Mix(m) if !m.added => m.placing.mode,
        MixPart::Mix(_) => CompositeMode::Normal,
        MixPart::Layer(i) => run
            .iter()
            .find(|m| m.index == *i)
            .map_or(CompositeMode::Normal, |m| placing_mode(&m.placed[0])),
    }
}

fn layers_in(parts: &[MixPart]) -> usize {
    parts
        .iter()
        .map(|p| match p {
            MixPart::Layer(_) => 1,
            MixPart::Mix(m) => layers_in(&m.parts),
        })
        .sum()
}

impl Mixes {
    pub fn of(layers: &[(&[Placed], Extent)], frame: (f64, f64)) -> Self {
        let n = layers.len();
        if n == 0 || layers.iter().all(|(p, _)| p.len() < 2) {
            return Self::default();
        }
        let members: Vec<Member<'_>> = layers
            .iter()
            .enumerate()
            .map(|(index, (placed, extent))| Member {
                index,
                placed,
                extent: *extent,
            })
            .collect();
        let parts: Vec<MixPart> = parts_of(&members, 0, frame)
            .into_iter()
            .map(|(part, _)| part)
            .collect();
        if !parts.iter().any(|p| matches!(p, MixPart::Mix(_))) {
            return Self::default();
        }
        let mut out = Self {
            layers: vec![false; n],
            ..Self::default()
        };
        let mut next = 0;
        for part in parts {
            match part {
                MixPart::Layer(i) => next = i + 1,
                MixPart::Mix(mix) => {
                    out.mark(&mix, 1);
                    let to = next + layers_in(&mix.parts);
                    out.outer.push(OuterMix {
                        mix: *mix,
                        layers: (next, to),
                    });
                    next = to;
                }
            }
        }
        out
    }

    fn mark(&mut self, mix: &Mix, depth: usize) {
        self.deepest = self.deepest.max(depth);
        self.blurs |= !mix.placing.effects.is_empty();
        self.moved += usize::from(mix.moving.is_some());
        for part in &mix.parts {
            match part {
                MixPart::Layer(i) => self.layers[*i] = true,
                MixPart::Mix(m) => self.mark(m, depth + 1),
            }
        }
    }

    pub fn layer_in_mix(&self, i: usize) -> bool {
        self.layers.get(i).copied().unwrap_or(false)
    }

    pub fn starts_at(&self, i: usize) -> bool {
        self.outer.iter().any(|m| m.layers.0 == i)
    }
}

fn quad_area(c: [[f64; 2]; 4]) -> Option<f64> {
    if c.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return None;
    }
    let mut s = 0.0;
    for i in 0..4 {
        let [x0, y0] = c[i];
        let [x1, y1] = c[(i + 1) % 4];
        s += x0 * y1 - x1 * y0;
    }
    Some((s * 0.5).abs())
}

pub fn fit(w: u32, h: u32, sar: (i32, i32), out_w: u32, out_h: u32) -> (u32, u32) {
    let sar = if sar.0 > 0 && sar.1 > 0 {
        f64::from(sar.0) / f64::from(sar.1)
    } else {
        1.0
    };
    let dar = f64::from(w) * sar / f64::from(h);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if dar > f64::from(out_w) / f64::from(out_h) {
        (out_w, even(f64::from(out_w) / dar).min(out_h))
    } else {
        (even(f64::from(out_h) * dar).min(out_w), out_h)
    }
}

pub fn upright_fit(
    (w, h): (u32, u32),
    sar: (i32, i32),
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> (u32, u32) {
    if rotation % 180 == 90 {
        fit(h, w, (sar.1, sar.0), out_w, out_h)
    } else {
        fit(w, h, sar, out_w, out_h)
    }
}

pub fn upright_pixels((w, h): (u32, u32), rotation: u16) -> (f64, f64) {
    let (w, h) = (f64::from(w), f64::from(h));
    if rotation % 180 == 90 { (h, w) } else { (w, h) }
}

pub fn rounded_place(
    size: (u32, u32),
    sar: (i32, i32),
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> [f64; 4] {
    let (w, h) = upright_fit(size, sar, rotation, (out_w, out_h));
    [
        f64::from((out_w - w.min(out_w)) / 2),
        f64::from((out_h - h.min(out_h)) / 2),
        f64::from(w),
        f64::from(h),
    ]
}

pub fn fitted(
    (w, h): (u32, u32),
    sar: (i32, i32),
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> [f64; 4] {
    let square = sar.0 == sar.1 || sar.0 <= 0 || sar.1 <= 0;
    if !square || rotation != 0 || w == 0 || h == 0 {
        return rounded_place((w, h), sar, rotation, (out_w, out_h));
    }
    let (pw, ph) = (f64::from(w), f64::from(h));
    let (fw, fh) = (f64::from(out_w), f64::from(out_h));
    let (sw, sh) = if pw * fh >= ph * fw {
        (fw, fw * ph / pw)
    } else {
        (fh * pw / ph, fh)
    };
    [(fw - sw) / 2.0, (fh - sh) / 2.0, sw, sh]
}

fn uncovered(quad: &[[f64; 2]; 4], [x, y]: [f64; 2]) -> f64 {
    let (mut left, mut right, mut near) = (false, false, f64::INFINITY);
    for i in 0..4 {
        let [ax, ay] = quad[i];
        let [bx, by] = quad[(i + 1) % 4];
        let (dx, dy) = (bx - ax, by - ay);
        let (px, py) = (x - ax, y - ay);
        let side = dx * py - dy * px;
        left |= side > 0.0;
        right |= side < 0.0;
        let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        near = near.min((px - t * dx).hypot(py - t * dy));
    }
    if left && right { near } else { 0.0 }
}

pub fn no_bars(quad: [[f64; 2]; 4], (w, h): (f64, f64)) -> bool {
    quad.iter().flatten().all(|v| v.is_finite())
        && [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
            .iter()
            .all(|c| uncovered(&quad, *c) < LEAST_BAR - 1e-9)
}

pub fn no_bars_at([x, y, w, h]: [f64; 4], frame: (f64, f64)) -> bool {
    no_bars([[x, y], [x + w, y], [x + w, y + h], [x, y + h]], frame)
}

pub fn fills(m: &Media, frame: (u32, u32)) -> bool {
    m.opaque()
        && m.info.video.as_ref().is_some_and(|v| {
            let sar = (v.sar[0], v.sar[1]);
            let place = fitted((v.width, v.height), sar, v.rotation, frame);
            no_bars_at(place, (f64::from(frame.0), f64::from(frame.1)))
        })
}

pub fn detail(placed: &[Placed]) -> f64 {
    placed.first().map_or(1.0, |p| {
        let near = p.picture.dynamic_zoom.map_or(1.0, |z| z.closest());
        p.picture.largest_zoom() / near
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing(picture: Picture, uv: [f64; 2]) -> [f64; 2] {
        let placed = Placed {
            clip: "c1".into(),
            picture,
            run: None,
            source: 0,
            frame: 0,
            start: 0,
            ready: true,
            ramp: None,
            crossing: false,
            moving: None,
            clock: Default::default(),
            adjustment: false,
        };
        let frame = (1920.0, 1080.0);
        let level = Placing::whole(&[placed], frame).level;
        level.forward(uv).map(|v| (v * 1e6).round() / 1e6)
    }

    #[test]
    fn a_keyed_blur_takes_its_strength_on_the_source_frame() {
        let mut effect = crate::model::EffectType::GaussianBlur.made();
        effect.keys = vec![crate::model::EffectKeys {
            value: crate::model::EffectValue::Strength,
            keys: vec![crate::model::Keyframe {
                frame: 30,
                value: 0.6,
                in_handle: None,
                out_handle: None,
            }],
            y: Vec::new(),
        }];
        let placed = Placed {
            clip: "c1".into(),
            picture: Picture {
                effects: vec![effect],
                ..Picture::default()
            },
            run: None,
            source: 30,
            frame: 0,
            start: 0,
            ready: true,
            ramp: None,
            crossing: false,
            moving: None,
            clock: Default::default(),
            adjustment: false,
        };
        let placing = Placing::whole(&[placed], (1920.0, 1080.0));
        let Work::Blur(b) = &placing.effects[0].work else {
            panic!("a blur");
        };
        assert_eq!((b.across[0] * 100.0).round(), 3888.0);
    }

    #[test]
    fn the_largest_zoom_key_sets_the_detail() {
        let picture = Picture {
            zoom_keys: vec![
                crate::model::Keyframe {
                    frame: 0,
                    value: 0.1,
                    in_handle: None,
                    out_handle: None,
                },
                crate::model::Keyframe {
                    frame: 30,
                    value: 0.4,
                    in_handle: None,
                    out_handle: None,
                },
            ],
            zoom_y: vec![0.1, 0.4],
            ..Picture::default()
        };
        let placed = Placed {
            clip: "c1".into(),
            picture,
            run: None,
            source: 0,
            frame: 0,
            start: 0,
            ready: true,
            ramp: None,
            crossing: false,
            moving: None,
            clock: Default::default(),
            adjustment: false,
        };
        assert_eq!(detail(&[placed]), 0.4);
    }

    #[test]
    fn a_zoom_works_about_the_anchor() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.5, 0.5]), [480.0, 540.0]);
    }

    #[test]
    fn a_rotation_works_about_the_anchor() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            rotation: Some(90.0),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.5, 0.5]), [1440.0, 1020.0]);
    }

    #[test]
    fn the_anchor_stays_where_the_position_puts_it() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(90.0),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.75, 0.5]), [1440.0, 540.0]);
    }

    #[test]
    fn the_anchor_is_a_point_of_the_flipped_picture() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            flip_horizontal: true,
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.25, 0.5]), [1440.0, 540.0]);
    }

    #[test]
    fn a_yaw_of_one_half_lands_the_top_left_corner() {
        let p = Picture {
            yaw: Some(0.5),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.0, 0.0]), [-960.0, -540.0]);
    }

    #[test]
    fn a_pitch_of_one_half_lands_the_bottom_right_corner() {
        let p = Picture {
            pitch: Some(0.5),
            ..Picture::default()
        };
        assert_eq!(landing(p, [1.0, 1.0]), [2880.0, 1620.0]);
    }
}
