use serde_json::{Value, json};

use super::effects::{key_frames, keyed, plain, ranged};
use super::picture::read_keys;
use super::{EDITOR, VIDCUT};
use crate::generators::{CENTER, checked_center};
use crate::keyframes::scaled;
use crate::model::{Generator, Gradient, Keyframe, hex};

const COLOR: &str = "color";
const BLACK: &str = "#000000";
const CHANNELS: &str = "channels";
const LINK: &str = "link";
const CENTER_X: &str = "centerX";
const CENTER_Y: &str = "centerY";
const UPPER_LEFT: &str = "upperLeft";
const UPPER_RIGHT: &str = "upperRight";
const LOWER_LEFT: &str = "lowerLeft";
const LOWER_RIGHT: &str = "lowerRight";

pub fn no_media() -> Value {
    json!({
        "OTIO_SCHEMA": "MissingReference.1",
        "metadata": {},
        "name": "",
        "available_range": null,
        "available_image_bounds": null,
    })
}

pub fn keep_ties(reference: &mut Value, editor: &Value) {
    for (key, from) in [(CHANNELS, "Channels"), (LINK, "Link Group ID")] {
        if !editor[from].is_null() {
            reference["metadata"][VIDCUT][key] = editor[from].clone();
        }
    }
}

fn kept<'a>(g: &'a Generator, key: &str) -> Option<&'a Value> {
    let Generator::Carried { reference } = g else {
        return None;
    };
    reference["metadata"][VIDCUT].get(key)
}

pub fn kept_channels(g: &Generator) -> Option<&Vec<Value>> {
    kept(g, CHANNELS)?.as_array()
}

pub fn kept_link(g: &Generator) -> Option<i64> {
    kept(g, LINK)?.as_i64()
}

fn made(kind: &str, shown: &str, effect: (&str, u32, u32), mut more: Vec<Value>) -> Value {
    let (effect, display, type_id) = effect;
    let mut parameters = vec![plain("Display Name", json!(""), json!(""), "String")];
    parameters.append(&mut more);
    json!({
        "OTIO_SCHEMA": "GeneratorReference.1",
        "metadata": { EDITOR: { "Generator Type": kind } },
        "name": shown,
        "available_range": null,
        "available_image_bounds": null,
        "generator_kind": kind,
        "parameters": { EDITOR: [{
            "Display Type": display,
            "Effect Name": effect,
            "Enabled": true,
            "Name": "Generator",
            "Parameters": parameters,
            "Type": type_id,
        }] },
    })
}

fn center(id: &str, value: f64, keys: &[Keyframe], origin: i64) -> Value {
    let first = keys.first().map_or(value, |k| k.value);
    let mut p = ranged(
        keyed(id, json!(first), json!(0.0), "Double"),
        *CENTER.start(),
        *CENTER.end(),
    );
    p["Key Frames"] = key_frames(keys, origin);
    p
}

fn gradient_reference(g: &Gradient, origin: i64) -> Value {
    let usual = Gradient::default();
    let color = |id: &str, value: [u8; 3], usual: [u8; 3]| {
        plain(id, json!(hex(value)), json!(hex(usual)), "Color")
    };
    let kind = Generator::FOUR_COLOR_GRADIENT;
    made(
        kind,
        kind,
        (kind, 0, 66),
        vec![
            center(CENTER_X, g.center_x, &g.center_x_keys, origin),
            center(CENTER_Y, g.center_y, &g.center_y_keys, origin),
            color(UPPER_LEFT, g.upper_left, usual.upper_left),
            color(UPPER_RIGHT, g.upper_right, usual.upper_right),
            color(LOWER_LEFT, g.lower_left, usual.lower_left),
            color(LOWER_RIGHT, g.lower_right, usual.lower_right),
        ],
    )
}

pub fn reference(g: &Generator, origin: i64) -> Value {
    match g {
        Generator::SolidColor { color } => {
            let kind = Generator::SOLID_COLOR;
            made(
                kind,
                kind,
                (kind, 0, 5),
                vec![plain(COLOR, json!(hex(*color)), json!(BLACK), "Color")],
            )
        }
        Generator::FourColorGradient(gradient) => gradient_reference(gradient, origin),
        Generator::Carried { reference } => {
            let mut written = (**reference).clone();
            if let Some(meta) = written["metadata"].as_object_mut() {
                meta.remove(VIDCUT);
            }
            written
        }
    }
}

fn parameter<'a>(reference: &'a Value, effect: &str, id: &str) -> Option<&'a Value> {
    reference["parameters"][EDITOR]
        .as_array()?
        .iter()
        .filter(|g| g["Effect Name"] == effect)
        .flat_map(|g| g["Parameters"].as_array().into_iter().flatten())
        .find(|p| p["Parameter ID"] == id)
}

fn color_of(reference: &Value, effect: &str, id: &str, usual: [u8; 3]) -> Option<[u8; 3]> {
    match parameter(reference, effect, id).and_then(|p| p["Parameter Value"].as_str()) {
        Some(color) => crate::subtitles::rgb(color),
        None => Some(usual),
    }
}

fn center_of(reference: &Value, id: &str, origin: i64) -> Option<(f64, Vec<Keyframe>)> {
    let Some(p) = parameter(reference, Generator::FOUR_COLOR_GRADIENT, id) else {
        return Some((0.0, Vec::new()));
    };
    let value = checked_center(p["Parameter Value"].as_f64().unwrap_or(0.0)).ok()?;
    let mut keys = read_keys(p, origin)?;
    for k in &mut keys {
        k.value = checked_center(k.value).ok()?;
    }
    Some((value, keys))
}

fn gradient_of(reference: &Value, origin: i64) -> Option<Generator> {
    let usual = Gradient::default();
    let color = |id, usual| color_of(reference, Generator::FOUR_COLOR_GRADIENT, id, usual);
    let (center_x, center_x_keys) = center_of(reference, CENTER_X, origin)?;
    let (center_y, center_y_keys) = center_of(reference, CENTER_Y, origin)?;
    Some(Generator::FourColorGradient(Gradient {
        center_x,
        center_x_keys,
        center_y,
        center_y_keys,
        upper_left: color(UPPER_LEFT, usual.upper_left)?,
        upper_right: color(UPPER_RIGHT, usual.upper_right)?,
        lower_left: color(LOWER_LEFT, usual.lower_left)?,
        lower_right: color(LOWER_RIGHT, usual.lower_right)?,
    }))
}

pub fn read(reference: &Value, origin: i64) -> Option<Generator> {
    let kind = reference["generator_kind"].as_str()?;
    match kind {
        Generator::SOLID_COLOR => Some(Generator::SolidColor {
            color: color_of(reference, kind, COLOR, [0, 0, 0])?,
        }),
        Generator::FOUR_COLOR_GRADIENT => gradient_of(reference, origin),
        _ => None,
    }
}

pub fn scale_keys(reference: &mut Value, frame: impl Fn(i64) -> i64, ratio: f64) {
    if reference["generator_kind"] != Generator::FOUR_COLOR_GRADIENT {
        return;
    }
    let Some(groups) = reference
        .pointer_mut(&format!("/parameters/{EDITOR}"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let parameters = groups
        .iter_mut()
        .filter_map(|g| g.get_mut("Parameters")?.as_array_mut())
        .flatten();
    for p in parameters {
        if let Some(keys) = read_keys(p, 0).filter(|keys| !keys.is_empty()) {
            p["Key Frames"] = key_frames(&scaled(&keys, &frame, ratio), 0);
        }
    }
}
