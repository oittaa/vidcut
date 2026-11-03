use serde_json::{Value, json};

use super::EDITOR;
use crate::model::{Generator, hex};

const COLOR: &str = "color";
const BLACK: &str = "#000000";

pub fn reference(g: &Generator) -> Value {
    let Generator::SolidColor { color } = g else {
        let Generator::Carried { reference } = g else {
            unreachable!("the two kinds");
        };
        return (**reference).clone();
    };
    let kind = Generator::SOLID_COLOR;
    let group = json!({
        "Display Type": 0,
        "Effect Name": kind,
        "Enabled": true,
        "Name": "Generator",
        "Parameters": [
            {
                "Default Parameter Value": "",
                "Parameter ID": "Display Name",
                "Parameter Value": "",
                "Variant Type": "String",
            },
            {
                "Default Parameter Value": BLACK,
                "Parameter ID": COLOR,
                "Parameter Value": hex(*color),
                "Variant Type": "Color",
            },
        ],
        "Type": 5,
    });
    json!({
        "OTIO_SCHEMA": "GeneratorReference.1",
        "metadata": { EDITOR: { "Generator Type": kind } },
        "name": kind,
        "available_range": null,
        "available_image_bounds": null,
        "generator_kind": kind,
        "parameters": { EDITOR: [group] },
    })
}

pub fn read(reference: &Value) -> Option<Generator> {
    if reference["generator_kind"] != Generator::SOLID_COLOR {
        return None;
    }
    let color = reference["parameters"][EDITOR]
        .as_array()?
        .iter()
        .filter(|g| g["Effect Name"] == Generator::SOLID_COLOR)
        .flat_map(|g| g["Parameters"].as_array().into_iter().flatten())
        .find(|p| p["Parameter ID"] == COLOR)
        .and_then(|p| p["Parameter Value"].as_str())
        .unwrap_or(BLACK);
    Some(Generator::SolidColor {
        color: crate::subtitles::rgb(color)?,
    })
}
