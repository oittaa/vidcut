use serde_json::{Value, json};

use crate::model::{Generator, Hdr, hex};

const BT709_TO_BT2020: [[f64; 3]; 3] = [
    [0.62740389, 0.32928304, 0.04331307],
    [0.06909729, 0.91954040, 0.01136232],
    [0.01639144, 0.08801331, 0.89559525],
];

const BT2020_LUMA: [f64; 3] = [0.2627, 0.6780, 0.0593];

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 32.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 128.0;
const PQ_C3: f64 = 2392.0 / 128.0;
const PQ_WHITE: f64 = 0.01;

const HLG_A: f64 = 0.17883277;
const HLG_B: f64 = 0.28466892;
const HLG_C: f64 = 0.55991073;
const HLG_GAMMA: f64 = 1.2;
const HLG_WHITE: f64 = 0.1;

fn pq_encode(light: f64) -> f64 {
    let p = light.max(0.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * p) / (1.0 + PQ_C3 * p)).powf(PQ_M2)
}

fn pq_decode(code: f64) -> f64 {
    let p = code.max(0.0).powf(1.0 / PQ_M2);
    ((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1)
}

fn hlg_encode(light: f64) -> f64 {
    match light.max(0.0) {
        l if l <= 1.0 / 12.0 => (3.0 * l).sqrt(),
        l => HLG_A * (12.0 * l - HLG_B).ln() + HLG_C,
    }
}

fn hlg_decode(code: f64) -> f64 {
    match code.max(0.0) {
        c if c <= 0.5 => c * c / 3.0,
        c => (((c - HLG_C) / HLG_A).exp() + HLG_B) / 12.0,
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn to_bt2020(c: [f64; 3]) -> [f64; 3] {
    BT709_TO_BT2020.map(|row| dot(row, c))
}

fn hlg_display(scene: [f64; 3]) -> [f64; 3] {
    let gain = dot(BT2020_LUMA, scene).max(0.0).powf(HLG_GAMMA - 1.0);
    scene.map(|v| v * gain)
}

fn hlg_scene(display: [f64; 3]) -> [f64; 3] {
    let luma = dot(BT2020_LUMA, display);
    if luma <= 0.0 {
        return [0.0; 3];
    }
    let gain = luma.powf((1.0 - HLG_GAMMA) / HLG_GAMMA);
    display.map(|v| v * gain)
}

pub fn timeline_code(color: [u8; 3], colors: Option<Hdr>) -> [f64; 3] {
    let c = color.map(|v| f64::from(v) / 255.0);
    match colors {
        None => c,
        Some(Hdr::Pq) => {
            let white = pq_encode(PQ_WHITE);
            to_bt2020(c.map(pq_decode)).map(|v| white * pq_encode(v))
        }
        Some(Hdr::Hlg) => {
            let white = hlg_encode(HLG_WHITE.powf(1.0 / HLG_GAMMA));
            hlg_scene(to_bt2020(hlg_display(c.map(hlg_decode)))).map(|v| white * hlg_encode(v))
        }
    }
}

pub fn name(g: &Generator) -> &str {
    match g {
        Generator::SolidColor { .. } => Generator::SOLID_COLOR,
        Generator::Carried { .. } if g.is_offline() => "Media Offline",
        Generator::Carried { reference } => [&reference["generator_kind"], &reference["name"]]
            .into_iter()
            .filter_map(Value::as_str)
            .find(|n| !n.is_empty())
            .unwrap_or("generator"),
    }
}

pub fn shown(g: &Generator) -> Value {
    match g {
        Generator::SolidColor { color } => {
            json!({ "kind": "solid_color", "name": name(g), "color": hex(*color) })
        }
        Generator::Carried { .. } => json!({ "kind": "carried", "name": name(g) }),
    }
}
