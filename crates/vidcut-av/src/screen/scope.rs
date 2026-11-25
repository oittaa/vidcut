use vidcut_core::generators::pq_code;
use vidcut_core::model::Hdr;
use vidcut_core::preferences::{
    ChromaticityMode, HistogramMode, ParadeMode, Scope, ScopeScale, Scopes, WaveformMode,
};

use super::{Gpu, TARGET, Target, begin, make_target};
use crate::color::{
    Colors, D65, Matrix, bt709_primaries, shader_colors, timeline_from_xyz, timeline_to_xyz,
};

pub const SMALLEST: u32 = 16;

pub const LARGEST: u32 = 4096;

const LOW: f64 = -0.1;

const HIGH: f64 = 1.1;

const REACH: f64 = 0.55;

const CIRCLE: f64 = 0.5;

const BARS: f64 = 0.75;

const FULL_AT: u64 = 16;

const HISTOGRAM_SHARE: u64 = 64;

const PARAMS: usize = 1600;

const SKIN_DEGREES: f64 = 123.14;

const HDR_NITS: [f64; 6] = [0.0, 1.0, 10.0, 100.0, 1000.0, 10000.0];

const PLOT_TOP: f64 = 0.04;

const PLOT_BOTTOM: f64 = 0.07;

const PLOT_LEFT: f64 = 0.16;

const PLOT_LEAST: f64 = 0.5;

const TEXT_ROOM: f64 = 0.3;

const ACROSS_SHIFT: f32 = 1.2;

const UP_SHIFT: f32 = -2.0;

const TEXT_SHIFT: f32 = 1.8;

const CHART_STEP: f64 = 0.2;

const LABEL_GAP: f64 = 3.0;

const CURVE: usize = 78;

const WAVE_STEP: usize = 2;

const WAVE_TICK: f64 = 0.0095;

const GAMUT_NAME: &str = "Rec.709";

const TEXT_COLOR: [u8; 3] = [152, 152, 152];

const LOCUS: [[f64; 2]; 65] = [
    [0.1741, 0.0050],
    [0.1740, 0.0050],
    [0.1738, 0.0049],
    [0.1736, 0.0049],
    [0.1733, 0.0048],
    [0.1730, 0.0048],
    [0.1726, 0.0048],
    [0.1721, 0.0048],
    [0.1714, 0.0051],
    [0.1703, 0.0058],
    [0.1689, 0.0069],
    [0.1669, 0.0086],
    [0.1644, 0.0109],
    [0.1611, 0.0138],
    [0.1566, 0.0177],
    [0.1510, 0.0227],
    [0.1440, 0.0297],
    [0.1355, 0.0399],
    [0.1241, 0.0578],
    [0.1096, 0.0868],
    [0.0913, 0.1327],
    [0.0687, 0.2007],
    [0.0454, 0.2950],
    [0.0235, 0.4127],
    [0.0082, 0.5384],
    [0.0039, 0.6548],
    [0.0139, 0.7502],
    [0.0389, 0.8120],
    [0.0743, 0.8338],
    [0.1142, 0.8262],
    [0.1547, 0.8059],
    [0.1929, 0.7816],
    [0.2296, 0.7543],
    [0.2658, 0.7243],
    [0.3016, 0.6923],
    [0.3374, 0.6588],
    [0.3731, 0.6245],
    [0.4087, 0.5896],
    [0.4441, 0.5547],
    [0.4788, 0.5202],
    [0.5125, 0.4866],
    [0.5448, 0.4544],
    [0.5752, 0.4242],
    [0.6029, 0.3965],
    [0.6270, 0.3725],
    [0.6482, 0.3514],
    [0.6658, 0.3340],
    [0.6801, 0.3197],
    [0.6915, 0.3083],
    [0.7006, 0.2993],
    [0.7079, 0.2920],
    [0.7140, 0.2859],
    [0.7190, 0.2809],
    [0.7230, 0.2769],
    [0.7260, 0.2740],
    [0.7283, 0.2717],
    [0.7300, 0.2700],
    [0.7311, 0.2689],
    [0.7320, 0.2680],
    [0.7327, 0.2673],
    [0.7334, 0.2666],
    [0.7340, 0.2660],
    [0.7344, 0.2656],
    [0.7346, 0.2654],
    [0.7347, 0.2653],
];

const PLANCK: [[f64; 2]; 21] = [
    [0.5998, 0.3850],
    [0.5857, 0.3931],
    [0.5551, 0.4064],
    [0.5267, 0.4133],
    [0.4770, 0.4137],
    [0.4369, 0.4041],
    [0.4053, 0.3907],
    [0.3804, 0.3767],
    [0.3451, 0.3516],
    [0.3221, 0.3318],
    [0.3135, 0.3236],
    [0.3064, 0.3165],
    [0.2952, 0.3048],
    [0.2806, 0.2883],
    [0.2718, 0.2776],
    [0.2637, 0.2673],
    [0.2565, 0.2576],
    [0.2525, 0.2522],
    [0.2472, 0.2447],
    [0.2426, 0.2380],
    [0.2399, 0.2340],
];

const DASHED_NITS: [f64; 3] = [26.0, 203.0, 2000.0];

const VIDEO_BLACK: f64 = 64.0 / 1023.0;

const VIDEO_SPAN: f64 = 876.0 / 1023.0;

const VIDEO_MIDDLE: f64 = 512.0 / 1023.0;

const VIDEO_CHROMA: f64 = 896.0 / 1023.0;

const MOST_ROWS: usize = 12;

pub(super) struct Pipes {
    waveform: wgpu::ComputePipeline,
    parade: wgpu::ComputePipeline,
    vectorscope: wgpu::ComputePipeline,
    histogram: wgpu::ComputePipeline,
    chromaticity: wgpu::ComputePipeline,
    painting: wgpu::RenderPipeline,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vidcut scope"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(include_str!("transfer.wgsl"), include_str!("scope.wgsl")).into(),
            ),
        });
        let gather = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("vidcut scope"),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let painting = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vidcut scope"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            waveform: gather("waveform"),
            parade: gather("parade"),
            vectorscope: gather("vectorscope"),
            histogram: gather("histogram"),
            chromaticity: gather("chromaticity"),
            painting,
        }
    }

    fn gathering(&self, scope: Scope) -> &wgpu::ComputePipeline {
        match scope {
            Scope::Waveform => &self.waveform,
            Scope::Parade => &self.parade,
            Scope::Vectorscope => &self.vectorscope,
            Scope::Histogram => &self.histogram,
            Scope::Chromaticity => &self.chromaticity,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub label: String,
    pub at: [f32; 2],
    pub align: [f32; 2],
    pub shift: [f32; 2],
    pub color: Option<[u8; 3]>,
}

fn nits_marks(nits: &[f64]) -> Vec<(String, f64)> {
    nits.iter().map(|&n| (n.to_string(), pq_code(n))).collect()
}

fn steps(count: u32, label: impl Fn(u32) -> u32) -> Vec<(String, f64)> {
    (0..=count)
        .map(|i| (label(i).to_string(), f64::from(i) / f64::from(count)))
        .collect()
}

fn levels(scale: ScopeScale) -> Vec<(String, f64)> {
    match scale {
        ScopeScale::TenBit => steps(8, |i| (i * 128).min(1023)),
        ScopeScale::TwelveBit => steps(8, |i| (i * 512).min(4095)),
        ScopeScale::Percent => steps(10, |i| i * 10),
        ScopeScale::Millivolts => steps(7, |i| i * 100),
        ScopeScale::Hdr => nits_marks(&HDR_NITS),
    }
}

fn dashed(scale: ScopeScale) -> Vec<f64> {
    match scale {
        ScopeScale::Hdr => DASHED_NITS.map(pq_code).to_vec(),
        _ => Vec::new(),
    }
}

fn sections(scope: Scope, prefs: Scopes) -> u32 {
    match scope {
        Scope::Parade if prefs.parade_mode == ParadeMode::Yrgb => 4,
        Scope::Histogram if prefs.histogram_mode == HistogramMode::Yrgb => 4,
        Scope::Parade | Scope::Histogram => 3,
        Scope::Waveform | Scope::Vectorscope | Scope::Chromaticity => 1,
    }
}

fn planes(scope: Scope, prefs: Scopes) -> u32 {
    match (scope, prefs.waveform_mode) {
        (Scope::Waveform, WaveformMode::Rgb) => 3,
        _ => 1,
    }
}

fn chart_xy(prefs: Scopes, [x, y]: [f64; 2]) -> [f64; 2] {
    match prefs.chromaticity_mode {
        ChromaticityMode::Xy => [x, y],
        ChromaticityMode::Uv => {
            let d = -2.0 * x + 12.0 * y + 3.0;
            [4.0 * x / d, 9.0 * y / d]
        }
    }
}

fn plot_rect((w, h): (u32, u32)) -> [f64; 4] {
    let (w, h) = (f64::from(w), f64::from(h));
    let text = h * TEXT_ROOM;
    let side = ((w - text) / (1.0 + PLOT_LEFT))
        .max(w * PLOT_LEAST)
        .min(h * (1.0 - PLOT_TOP - PLOT_BOTTOM));
    let left = ((w - side) / 2.0)
        .min(w - side - text)
        .max(side * PLOT_LEFT);
    let top = (h - side) * PLOT_TOP / (PLOT_TOP + PLOT_BOTTOM);
    [left, top, left + side, top + side]
}

fn on_chart(rect: [f64; 4], [u, v]: [f64; 2]) -> [f64; 2] {
    [
        rect[0] + u * (rect[2] - rect[0]),
        rect[3] - v * (rect[3] - rect[1]),
    ]
}

fn wave_ticks(locus: &[[f64; 2]], white: [f64; 2], side: f64) -> Vec<[f64; 2]> {
    let last = locus.len() - 1;
    (0..locus.len())
        .step_by(WAVE_STEP)
        .flat_map(|i| {
            let (before, at, after) = (
                locus[i.saturating_sub(1)],
                locus[i],
                locus[(i + 1).min(last)],
            );
            let mut out = [after[1] - before[1], before[0] - after[0]];
            if out[0] * (at[0] - white[0]) + out[1] * (at[1] - white[1]) < 0.0 {
                out = [-out[0], -out[1]];
            }
            let reach = side * WAVE_TICK / out[0].hypot(out[1]).max(f64::EPSILON);
            [at, [at[0] + out[0] * reach, at[1] + out[1] * reach]]
        })
        .collect()
}

fn curve_points(prefs: Scopes, size: (u32, u32)) -> Vec<[f64; 2]> {
    let rect = plot_rect(size);
    let gamut = bt709_primaries().map(|(x, y)| [x, y]);
    let white = [D65.0, D65.1];
    let place = |p: &[f64; 2]| on_chart(rect, chart_xy(prefs, *p));
    let locus: Vec<[f64; 2]> = LOCUS.iter().map(place).collect();
    let ticks = wave_ticks(&locus, place(&white), rect[2] - rect[0]);
    locus
        .into_iter()
        .chain(PLANCK.iter().chain(&gamut).chain([&white]).map(place))
        .chain(ticks)
        .collect()
}

fn chart_marks(prefs: Scopes, size: (u32, u32)) -> Vec<Mark> {
    let rect = plot_rect(size);
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let shifted = |label: String, [x, y]: [f64; 2], align: [f32; 2], shift: [f32; 2]| Mark {
        label,
        at: [(x / w) as f32, (y / h) as f32],
        align,
        shift,
        color: Some(TEXT_COLOR),
    };
    let mark = |label, at, align| shifted(label, at, align, [0.0, 0.0]);
    let axis = |label, at, align| Mark {
        color: None,
        ..mark(label, at, align)
    };
    let mut marks = Vec::new();
    for i in 0..=5 {
        let v = f64::from(i) * CHART_STEP;
        let label = match i {
            0 => "0".to_string(),
            _ => format!("{v:.1}"),
        };
        let below = [on_chart(rect, [v, 0.0])[0], rect[3] + LABEL_GAP];
        marks.push(axis(label.clone(), below, [0.5, 0.0]));
        if i > 0 {
            let left = [rect[0] - LABEL_GAP, on_chart(rect, [0.0, v])[1]];
            marks.push(axis(label, left, [1.0, 0.5]));
        }
    }
    let (across, up) = match prefs.chromaticity_mode {
        ChromaticityMode::Xy => ("X", "Y"),
        ChromaticityMode::Uv => ("U", "V"),
    };
    let corner = [rect[2], rect[3] + LABEL_GAP];
    marks.push(Mark {
        color: None,
        ..shifted(across.to_string(), corner, [0.0, 0.0], [ACROSS_SHIFT, 0.0])
    });
    let top = [rect[0] - LABEL_GAP, rect[1]];
    marks.push(Mark {
        color: None,
        ..shifted(up.to_string(), top, [1.0, 0.5], [UP_SHIFT, 0.0])
    });
    let name = GAMUT_NAME;
    let gamut = bt709_primaries();
    let green = on_chart(rect, chart_xy(prefs, [gamut[1].0, gamut[1].1]));
    let beside = [green[0] - LABEL_GAP, green[1]];
    marks.push(mark(name.to_string(), beside, [1.0, 0.5]));
    let coordinates = |(x, y): (f64, f64)| {
        let [u, v] = chart_xy(prefs, [x, y]);
        format!("{u:.4}, {v:.4}")
    };
    let text = format!(
        "{name} (D65)\nR: {}\nG: {}\nB: {}\nW: {}",
        coordinates(gamut[0]),
        coordinates(gamut[1]),
        coordinates(gamut[2]),
        coordinates(D65),
    );
    marks.push(shifted(
        text,
        [w - LABEL_GAP, rect[1]],
        [1.0, 0.0],
        [0.0, TEXT_SHIFT],
    ));
    marks
}

fn from_top(v: f64) -> f64 {
    (HIGH - v) / (HIGH - LOW)
}

fn luma_of(colors: Option<Hdr>) -> (f64, f64) {
    let matrix = match colors {
        Some(_) => Matrix::Bt2020,
        None => Matrix::Bt709,
    };
    let c = shader_colors(Colors {
        matrix,
        full_range: true,
        hdr: colors,
    });
    (c.kr, c.kb)
}

fn chroma_scales((kr, kb): (f64, f64)) -> (f64, f64) {
    (0.5 / (1.0 - kb), 0.5 / (1.0 - kr))
}

fn hue_points(colors: Option<Hdr>) -> [(&'static str, [f64; 2]); 6] {
    let (kr, kb) = luma_of(colors);
    let kg = 1.0 - kr - kb;
    let (cb, cr) = chroma_scales((kr, kb));
    [
        ("R", [BARS, 0.0, 0.0]),
        ("Mg", [BARS, 0.0, BARS]),
        ("B", [0.0, 0.0, BARS]),
        ("Cy", [0.0, BARS, BARS]),
        ("G", [0.0, BARS, 0.0]),
        ("Yl", [BARS, BARS, 0.0]),
    ]
    .map(|(name, [r, g, b])| {
        let y = kr * r + kg * g + kb * b;
        (name, [(b - y) * cb, (y - r) * cr])
    })
}

fn per_unit((w, h): (u32, u32)) -> f64 {
    f64::from(w.min(h)) / (2.0 * REACH)
}

pub fn marks(scope: Scope, (colors, prefs): (Option<Hdr>, Scopes), size: (u32, u32)) -> Vec<Mark> {
    match scope {
        Scope::Waveform | Scope::Parade => levels(prefs.scale)
            .into_iter()
            .map(|(label, v)| Mark {
                label,
                at: [0.0, from_top(v) as f32],
                align: [0.0, 1.0],
                shift: [0.0, 0.0],
                color: None,
            })
            .collect(),
        Scope::Histogram => levels(prefs.scale)
            .into_iter()
            .map(|(label, v)| Mark {
                label,
                at: [v as f32, 0.0],
                align: [0.5, 0.0],
                shift: [0.0, 0.0],
                color: None,
            })
            .collect(),
        Scope::Chromaticity => chart_marks(prefs, size),
        Scope::Vectorscope => {
            let unit = per_unit(size);
            hue_points(colors)
                .into_iter()
                .map(|(name, [x, y])| Mark {
                    label: name.to_string(),
                    at: [
                        (0.5 + x * unit / f64::from(size.0)) as f32,
                        (0.5 + y * unit / f64::from(size.1)) as f32,
                    ],
                    align: [0.5, 0.5],
                    shift: [0.0, 0.0],
                    color: None,
                })
                .collect()
        }
    }
}

fn params(
    scope: Scope,
    (colors, prefs): (Option<Hdr>, Scopes),
    canvas: (u32, u32),
    size: (u32, u32),
) -> [[u8; 4]; PARAMS / 4] {
    let (kr, kb) = luma_of(colors);
    let (cb, cr) = chroma_scales((kr, kb));
    let unit = per_unit(size);
    let pixels = u64::from(canvas.0) * u64::from(canvas.1);
    let columns = match scope {
        Scope::Waveform | Scope::Histogram => size.0,
        Scope::Parade => size.0 / sections(scope, prefs),
        Scope::Vectorscope | Scope::Chromaticity => size.0.min(size.1),
    };
    let full = match scope {
        Scope::Histogram => pixels / HISTOGRAM_SHARE,
        _ => pixels / u64::from(columns.max(1)) / FULL_AT,
    }
    .max(1);
    let f = |v: f64| (v as f32).to_le_bytes();
    let n = |v: u32| v.to_le_bytes();
    let mut rows = [f(-1.0); MOST_ROWS];
    let (labeled, lines) = match scope {
        Scope::Vectorscope | Scope::Chromaticity => (0, Vec::new()),
        _ => {
            let labeled: Vec<f64> = levels(prefs.scale).into_iter().map(|(_, v)| v).collect();
            let count = labeled.len();
            (count, [labeled, dashed(prefs.scale)].concat())
        }
    };
    for (row, v) in rows.iter_mut().zip(&lines) {
        let at = match scope {
            Scope::Histogram => (v * f64::from(size.0)).floor().min(f64::from(size.0 - 1)),
            _ => (from_top(*v) * f64::from(size.1))
                .floor()
                .min(f64::from(size.1 - 1)),
        };
        *row = f(at);
    }
    let middle = [f64::from(size.0) / 2.0, f64::from(size.1) / 2.0];
    let hues = hue_points(colors).map(|(_, [x, y])| {
        [
            f((middle[0] + x * unit).floor()),
            f((middle[1] + y * unit).floor()),
        ]
    });
    let (black, span, middle_code, chroma) = match prefs.video_levels {
        true => (VIDEO_BLACK, VIDEO_SPAN, VIDEO_MIDDLE, VIDEO_CHROMA),
        false => (0.0, 1.0, 0.5, 1.0),
    };
    let mode = match scope {
        Scope::Waveform => prefs.waveform_mode as u32,
        Scope::Parade => prefs.parade_mode as u32,
        Scope::Histogram => prefs.histogram_mode as u32,
        Scope::Chromaticity => prefs.chromaticity_mode as u32,
        Scope::Vectorscope => 0,
    };
    let channels = [
        prefs.waveform_red,
        prefs.waveform_green,
        prefs.waveform_blue,
    ]
    .iter()
    .enumerate()
    .filter(|(_, on)| **on)
    .fold(0, |mask, (i, _)| mask | 1 << i);
    let kind = match scope {
        Scope::Waveform => 0,
        Scope::Parade => 1,
        Scope::Vectorscope => 2,
        Scope::Histogram => 3,
        Scope::Chromaticity => 4,
    };
    let mut bytes = vec![
        n(canvas.0),
        n(canvas.1),
        n(size.0),
        n(size.1),
        f(kr),
        f(1.0 - kr - kb),
        f(kb),
        f(0.0),
        f(cb),
        f(cr),
        f(CIRCLE * unit),
        f((f64::from(size.0.min(size.1)) / 64.0).max(3.0)),
        f(LOW),
        f(HIGH - LOW),
        f(1.0 / full as f64),
        f(unit),
        f(black),
        f(span),
        f(middle_code),
        f(chroma),
    ];
    bytes.extend(rows);
    bytes.extend(hues.iter().flatten());
    bytes.extend([
        n(kind),
        n(lines.len().min(MOST_ROWS) as u32),
        n(labeled.min(MOST_ROWS) as u32),
        n(mode),
        n(sections(scope, prefs)),
        n(channels),
        n(planes(scope, prefs)),
        n(u32::from(prefs.skin_tone)),
    ]);
    let skin = SKIN_DEGREES.to_radians();
    bytes.extend([f(skin.cos()), f(-skin.sin()), f(0.0), f(0.0)]);
    bytes.extend(plot_rect(size).map(f));
    for row in timeline_to_xyz(colors)
        .iter()
        .chain(&timeline_from_xyz(colors))
    {
        bytes.extend([f(row[0]), f(row[1]), f(row[2]), f(0.0)]);
    }
    let transfer = match colors {
        None => 0,
        Some(Hdr::Pq) => 1,
        Some(Hdr::Hlg) => 2,
    };
    bytes.extend([
        n(transfer),
        n(LOCUS.len() as u32),
        n(PLANCK.len() as u32),
        n(LOCUS.len().div_ceil(WAVE_STEP) as u32),
    ]);
    let mut curve = vec![f(0.0); CURVE * 4];
    for (slot, p) in curve.chunks_mut(2).zip(curve_points(prefs, size)) {
        slot[0] = f(p[0]);
        slot[1] = f(p[1]);
    }
    bytes.extend(curve);
    bytes
        .try_into()
        .expect("the fields of the scope's parameters")
}

struct Plot {
    scope: Scope,
    size: (u32, u32),
    planes: u32,
    generation: u64,
    target: Target,
    params: wgpu::Buffer,
    hits: wgpu::Buffer,
    painted: wgpu::BindGroup,
    drawn: Option<u64>,
}

#[derive(Default)]
pub(super) struct Plots {
    plots: Vec<Plot>,
    made: u64,
    prefs: Scopes,
}

impl Plots {
    fn make(
        gpu: &Gpu,
        pipes: &Pipes,
        (scope, size, planes): (Scope, (u32, u32), u32),
        generation: u64,
    ) -> Plot {
        let params = crate::compute::buffer(
            &gpu.device,
            "vidcut scope",
            PARAMS,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let hits = crate::compute::buffer(
            &gpu.device,
            "vidcut scope hits",
            size.0 as usize * size.1 as usize * planes as usize * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let painted = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vidcut scope"),
            layout: &pipes.painting.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: hits.as_entire_binding(),
                },
            ],
        });
        Plot {
            scope,
            size,
            planes,
            generation,
            target: make_target(&gpu.device, size),
            params,
            hits,
            painted,
            drawn: None,
        }
    }

    pub(super) fn paint(
        &mut self,
        gpu: &Gpu,
        (canvas, canvas_size): (&wgpu::TextureView, (u32, u32)),
        (colors, draw): (Option<Hdr>, u64),
        wanted: &[(Scope, (u32, u32))],
        prefs: Scopes,
    ) -> bool {
        if self.prefs != prefs {
            self.prefs = prefs;
            for p in &mut self.plots {
                p.drawn = None;
            }
        }
        let pipes = gpu.scope.get_or_init(|| Pipes::new(&gpu.device));
        self.plots.retain(|p| {
            wanted
                .iter()
                .any(|&(s, z)| s == p.scope && sized(z) == p.size && planes(s, prefs) == p.planes)
        });
        for &(scope, size) in wanted {
            let size = sized(size);
            if !self.plots.iter().any(|p| p.scope == scope) {
                self.made += 1;
                let made = (scope, size, planes(scope, prefs));
                self.plots.push(Self::make(gpu, pipes, made, self.made));
            }
        }
        if self.plots.iter().all(|p| p.drawn == Some(draw)) {
            return false;
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vidcut scope"),
            });
        for plot in self.plots.iter_mut().filter(|p| p.drawn != Some(draw)) {
            let words = params(plot.scope, (colors, prefs), canvas_size, plot.size);
            gpu.queue
                .write_buffer(&plot.params, 0, words.as_flattened());
            encoder.clear_buffer(&plot.hits, 0, None);
            let gathering = pipes.gathering(plot.scope);
            let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vidcut scope"),
                layout: &gathering.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: plot.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(canvas),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: plot.hits.as_entire_binding(),
                    },
                ],
            });
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(gathering);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(canvas_size.0.div_ceil(16), canvas_size.1.div_ceil(16), 1);
            }
            {
                let mut pass = begin(&mut encoder, &plot.target.view, Some(wgpu::Color::BLACK));
                pass.set_pipeline(&pipes.painting);
                pass.set_bind_group(0, &plot.painted, &[]);
                pass.draw(0..3, 0..1);
            }
            plot.drawn = Some(draw);
        }
        gpu.submit([encoder.finish()]);
        true
    }

    pub(super) fn drop_all(&mut self) {
        self.plots.clear();
    }

    pub(super) fn views(&self) -> Vec<(Scope, u64, wgpu::TextureView)> {
        self.plots
            .iter()
            .map(|p| (p.scope, p.generation, p.target.view.clone()))
            .collect()
    }

    pub(super) fn target(&self, scope: Scope) -> Option<&Target> {
        self.plots
            .iter()
            .find(|p| p.scope == scope)
            .map(|p| &p.target)
    }
}

fn sized((w, h): (u32, u32)) -> (u32, u32) {
    (w.clamp(SMALLEST, LARGEST), h.clamp(SMALLEST, LARGEST))
}
