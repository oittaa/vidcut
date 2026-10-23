use serde_json::{Map, Value, json};

use super::EDITOR;
use super::effects::{effect, group, keyed, plain, ranged, rounded};
use crate::model::{Crop, DynamicZoom, Ease, Picture, VideoInfo, ZoomBox};

const TRANSFORM: &str = "Transform";
const CROPPING: &str = "Cropping";
const SCALING: &str = "Retime and Scaling";
const ZOOMING: &str = "Dynamic Zoom";
const NAMES: [&str; 4] = [TRANSFORM, CROPPING, SCALING, ZOOMING];

const ZOOM_X: &str = "transformationZoomX";
const ZOOM_Y: &str = "transformationZoomY";
const ZOOM_LINK: &str = "transformationZoomLink";
const PAN: &str = "transformationPan";
const TILT: &str = "transformationTilt";
const ROTATION: &str = "transformationRotationAngle";
const ANCHOR: &str = "transformationAnchorPoint";
const FLIP_X: &str = "transformationFlipX";
const FLIP_Y: &str = "transformationFlipY";
const CROPS: [&str; 4] = ["cropLeft", "cropRight", "cropTop", "cropBottom"];
const SCALE: &str = "scale";
const EASE: &str = "dynamicZoomEase";
const CENTER: &str = "dynamicZoomCenter";
const SIZE: &str = "dynamicZoomScale";

const FIT: u8 = 2;
const REACH: f64 = 4.0;
const TURNS: f64 = 3600.0;
const LATER: i64 = 1000;
const SIZES: (f64, f64) = (0.01, 100.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    frame: [f64; 2],
    fitted: [f64; 2],
    source: [f64; 2],
}

impl Shape {
    pub fn of(v: &VideoInfo, (w, h): (u32, u32)) -> Self {
        let sar = (v.sar[0], v.sar[1]);
        let (fw, fh) = crate::place::upright_fit((v.width, v.height), sar, v.rotation, (w, h));
        let (dw, dh) = v.display_size();
        Self {
            frame: [w.into(), h.into()],
            fitted: [fw.into(), fh.into()],
            source: [dw.into(), dh.into()],
        }
    }

    pub fn whole((w, h): (u32, u32)) -> Self {
        let frame = [w.into(), h.into()];
        Self {
            frame,
            fitted: frame,
            source: frame,
        }
    }

    fn picture(&self, scale: u8) -> [f64; 2] {
        let ([w, h], [dw, dh]) = (self.frame, self.source);
        match scale {
            1 => self.source,
            3 => {
                let s = (w / dw).max(h / dh);
                [dw * s, dh * s]
            }
            4 => self.frame,
            _ => self.fitted,
        }
    }
}

fn kind(e: &Value) -> Option<&'static str> {
    let name = e["metadata"][EDITOR]["Effect Name"].as_str()?;
    NAMES.into_iter().find(|n| *n == name)
}

pub fn is_placing(e: &Value) -> bool {
    kind(e).is_some()
}

fn known(name: &str) -> &'static [&'static str] {
    match name {
        TRANSFORM => &[
            ZOOM_X, ZOOM_Y, ZOOM_LINK, PAN, TILT, ROTATION, ANCHOR, FLIP_X, FLIP_Y,
        ],
        CROPPING => &CROPS,
        ZOOMING => &[EASE, CENTER, SIZE],
        _ => &[SCALE],
    }
}

fn animated(name: &str, id: &str) -> bool {
    name == ZOOMING && (id == CENTER || id == SIZE)
}

fn keyframed(p: &Value) -> bool {
    match &p["Key Frames"] {
        Value::Null => false,
        Value::Object(keys) => !keys.is_empty(),
        Value::Array(keys) => !keys.is_empty(),
        _ => true,
    }
}

type Parameters<'a> = (Vec<&'static str>, Vec<(&'a str, &'a Value)>);

fn parameters(effects: &[Value]) -> Option<Parameters<'_>> {
    let (mut found, mut values) = (Vec::new(), Vec::new());
    for name in NAMES {
        let mut of = effects
            .iter()
            .filter(|e| kind(e) == Some(name) && e["metadata"][EDITOR]["Enabled"] != false);
        let first = of.next();
        if of.next().is_some() {
            return None;
        }
        let Some(e) = first else {
            continue;
        };
        found.push(name);
        for p in e["metadata"][EDITOR]["Parameters"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let id = p["Parameter ID"].as_str()?;
            if !known(name).contains(&id) || keyframed(p) != animated(name, id) {
                return None;
            }
            values.push((id, p));
        }
    }
    (!found.is_empty()).then_some((found, values))
}

fn ease_code(e: Ease) -> u8 {
    match e {
        Ease::Linear => 0,
        Ease::EaseIn => 1,
        Ease::EaseOut => 2,
        Ease::EaseInOut => 3,
    }
}

fn ease_of(v: &Value) -> Option<Ease> {
    let code = v.as_f64().filter(|c| c.fract() == 0.0)?;
    Ease::ALL
        .into_iter()
        .find(|e| f64::from(ease_code(*e)) == code)
}

fn ends(p: &Value) -> Option<[&Value; 2]> {
    let keys = p["Key Frames"].as_object().filter(|k| k.len() == 2)?;
    let mut at: Vec<(f64, &Value)> = keys
        .iter()
        .map(|(t, k)| {
            Some((
                t.parse::<f64>().ok().filter(|t| t.is_finite())?,
                &k["Value"],
            ))
        })
        .collect::<Option<_>>()?;
    at.sort_by(|a, b| a.0.total_cmp(&b.0));
    (at[0].0 < at[1].0).then(|| [at[0].1, at[1].1])
}

fn zoom_of(ease: Option<&Value>, center: &Value, size: &Value) -> Option<DynamicZoom> {
    let ease = match ease {
        None => Ease::Linear,
        Some(p) => ease_of(&p["Parameter Value"])?,
    };
    let [c0, c1] = ends(center)?;
    let [s0, s1] = ends(size)?;
    let at = |c: &Value, s: &Value| -> Option<ZoomBox> {
        let a = c.as_array().filter(|a| a.len() == 2)?;
        let x = |i: usize| a[i].as_f64().filter(|v| v.is_finite());
        Some(ZoomBox {
            center: [rounded(0.5 + x(0)?), rounded(0.5 - x(1)?)],
            size: rounded(s.as_f64().filter(|v| v.is_finite())?),
        })
    };
    Some(DynamicZoom {
        start: at(c0, s0)?,
        end: at(c1, s1)?,
        ease,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    zoom: [f64; 2],
    moved: [f64; 2],
    turn: f64,
    anchor: [f64; 2],
    flips: [bool; 2],
    crop: [f64; 4],
    scale: u8,
    dynamic: Option<DynamicZoom>,
}

fn placement(effects: &[Value]) -> Option<Placement> {
    let (found, values) = parameters(effects)?;
    let param = |id: &str| values.iter().find(|(k, _)| *k == id).map(|(_, p)| *p);
    let given = |id: &str| param(id).map(|p| &p["Parameter Value"]);
    let number = |id: &str, usual: f64| match given(id) {
        None => Some(usual),
        Some(v) => v.as_f64().filter(|v| v.is_finite()),
    };
    let switch = |id: &str| match given(id) {
        None => Some(false),
        Some(v) => v.as_bool().or_else(|| v.as_f64().map(|x| x != 0.0)),
    };
    let anchor = match given(ANCHOR) {
        None => [0.0, 0.0],
        Some(v) => {
            let a = v.as_array().filter(|a| a.len() == 2)?;
            let x = |i: usize| a[i].as_f64().filter(|v| v.is_finite());
            [x(0)?, x(1)?]
        }
    };
    let scale = match given(SCALE) {
        None => 0,
        Some(v) => {
            let s = v
                .as_f64()
                .filter(|s| s.fract() == 0.0 && (0.0..=4.0).contains(s))?;
            s as u8
        }
    };
    let crop = [
        number(CROPS[0], 0.0)?,
        number(CROPS[1], 0.0)?,
        number(CROPS[2], 0.0)?,
        number(CROPS[3], 0.0)?,
    ];
    let dynamic = match found.contains(&ZOOMING) {
        true => Some(zoom_of(param(EASE), param(CENTER)?, param(SIZE)?)?),
        false => None,
    };
    Some(Placement {
        zoom: [number(ZOOM_X, 1.0)?, number(ZOOM_Y, 1.0)?],
        moved: [number(PAN, 0.0)?, number(TILT, 0.0)?],
        turn: number(ROTATION, 0.0)?,
        anchor,
        flips: [switch(FLIP_X)?, switch(FLIP_Y)?],
        crop,
        scale,
        dynamic,
    })
}

impl Placement {
    fn picture(&self, shape: &Shape) -> Picture {
        let [pw, ph] = shape.picture(self.scale);
        let ([w, h], [fw, fh]) = (shape.frame, shape.fitted);
        let ([zx, zy], [ax, ay]) = (self.zoom, self.anchor);
        let d = [
            ax * (pw - fw) + self.moved[0] * pw,
            -ay * (ph - fh) - self.moved[1] * ph,
        ];
        let turn = if self.turn.abs() > TURNS {
            self.turn % 360.0
        } else {
            self.turn
        };
        let [left, right, top, bottom] = self.crop.map(rounded);
        Picture {
            position: Some([rounded(0.5 + d[0] / w), rounded(0.5 + d[1] / h)]),
            zoom: Some([rounded(pw / fw * zx), rounded(ph / fh * zy)]),
            rotation: Some(rounded(turn)),
            anchor: Some([rounded(0.5 + ax), rounded(0.5 - ay)]),
            flip_horizontal: self.flips[0],
            flip_vertical: self.flips[1],
            crop: Crop {
                left,
                right,
                top,
                bottom,
            },
            dynamic_zoom: self.dynamic,
        }
    }
}

pub fn taken(effects: &[Value], ours: bool) -> bool {
    ours || placement(effects).is_some()
}

pub fn take(
    effects: &[Value],
    ours: Option<&Picture>,
    shape: &Shape,
) -> Result<(Vec<Value>, Option<Picture>), Vec<String>> {
    let rest = || -> Vec<Value> { effects.iter().filter(|e| !is_placing(e)).cloned().collect() };
    if let Some(p) = ours {
        return Ok((rest(), Some(p.clone())));
    }
    let Some(read) = placement(effects) else {
        return Ok((effects.to_vec(), None));
    };
    match crate::ops::picture::checked(&read.picture(shape)) {
        Ok(picture) => Ok((rest(), picture)),
        Err(_) => {
            let mut names: Vec<String> = Vec::new();
            for n in effects
                .iter()
                .filter(|e| is_placing(e))
                .map(super::effects::name)
            {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            Err(names)
        }
    }
}

fn moved(p: &Picture, shape: &Shape) -> [f64; 2] {
    let [x, y] = p.place();
    let ([w, h], [fw, fh]) = (shape.frame, shape.fitted);
    [rounded((x - 0.5) * w / fw), rounded(-(y - 0.5) * h / fh)]
}

pub fn reaches(p: &Picture, shape: &Shape) -> bool {
    moved(p, shape).iter().all(|v| v.abs() <= REACH)
}

fn double(id: &str, value: f64, usual: f64, (low, high): (f64, f64)) -> Option<Value> {
    (value != usual).then(|| ranged(keyed(id, json!(value), json!(usual), "Double"), low, high))
}

fn boolean(id: &str, value: bool, usual: bool) -> Option<Value> {
    (value != usual).then(|| keyed(id, json!(value), json!(usual), "Bool"))
}

fn point(id: &str, value: [f64; 2], usual: [f64; 2]) -> Option<Value> {
    (value != usual).then(|| keyed(id, json!(value), json!(usual), "POINTF"))
}

fn zooming(z: &DynamicZoom, g: i64) -> Vec<Value> {
    let keys = |a: Value, b: Value, variant: &str| {
        let mut m = Map::new();
        for (at, v) in [(-g, a), (LATER - g, b)] {
            m.insert(
                at.to_string(),
                json!({ "Value": v, "Variant Type": variant }),
            );
        }
        Value::Object(m)
    };
    let middle = |b: &ZoomBox| json!([rounded(b.center[0] - 0.5), rounded(0.5 - b.center[1])]);
    let mut center = plain(CENTER, json!([0.0, 0.0]), json!([0.0, 0.0]), "POINTF");
    center["Key Frames"] = keys(middle(&z.start), middle(&z.end), "POINTF");
    let mut size = ranged(
        plain(SIZE, json!(1.0), json!(1.0), "Double"),
        SIZES.0,
        SIZES.1,
    );
    size["Key Frames"] = keys(json!(z.start.size), json!(z.end.size), "Double");
    let ease = keyed(EASE, json!(ease_code(z.ease)), json!(0), "UInt");
    vec![ease, center, size]
}

pub fn effects(p: &Picture, shape: &Shape, g: i64) -> Vec<Value> {
    let [zx, zy] = p.scale();
    let [ax, ay] = p.pivot();
    let [pan, tilt] = moved(p, shape).map(|v| v.clamp(-REACH, REACH));
    let (zoom, reach) = ((0.0, 100.0), (-REACH, REACH));
    let transform: Vec<Value> = [
        double(ZOOM_X, zx, 1.0, zoom),
        double(ZOOM_Y, zy, 1.0, zoom),
        boolean(ZOOM_LINK, zx == zy, true),
        double(PAN, pan, 0.0, reach),
        double(TILT, tilt, 0.0, reach),
        double(ROTATION, p.turn(), 0.0, (-100000.0, 100000.0)),
        point(ANCHOR, [rounded(ax - 0.5), rounded(0.5 - ay)], [0.0, 0.0]),
        boolean(FLIP_X, p.flip_horizontal, false),
        boolean(FLIP_Y, p.flip_vertical, false),
    ]
    .into_iter()
    .flatten()
    .collect();
    let cropping: Vec<Value> = CROPS
        .into_iter()
        .zip(p.crop.sides())
        .filter_map(|(id, v)| double(id, v, 0.0, (0.0, 1.0)))
        .collect();
    let scaling = vec![keyed(SCALE, json!(FIT), json!(0), "UInt")];
    let dynamic = p.dynamic_zoom.map_or_else(Vec::new, |z| zooming(&z, g));
    [
        (TRANSFORM, 2, transform),
        (CROPPING, 3, cropping),
        (SCALING, 22, scaling),
        (ZOOMING, 59, dynamic),
    ]
    .into_iter()
    .filter(|(_, _, list)| !list.is_empty())
    .map(|(name, kind, list)| effect(group(name, kind, 1, list)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::Placing;
    use crate::render::Placed;
    use crate::test_support::still_info;

    fn given(transform: &[(&str, Value)], scale: u8) -> Vec<Value> {
        let param = |(id, v): &(&str, Value)| json!({ "Parameter ID": id, "Parameter Value": v });
        let made = |name: &str, kind: u32, params: Vec<Value>| {
            json!({
                "OTIO_SCHEMA": "Effect.1",
                "metadata": { EDITOR: {
                    "Effect Name": name, "Enabled": true, "Name": name,
                    "Parameters": params, "Type": kind,
                } },
            })
        };
        vec![
            made(TRANSFORM, 2, transform.iter().map(param).collect()),
            made(SCALING, 22, vec![param(&(SCALE, json!(scale)))]),
        ]
    }

    fn filled() -> Shape {
        let v = still_info(1440, 1080).video.expect("video");
        Shape::of(&v, (1920, 1080))
    }

    fn filled_turned() -> Vec<Value> {
        let transform = [
            (ZOOM_X, json!(1.5)),
            (ZOOM_Y, json!(1.5)),
            (ROTATION, json!(90.0)),
            (ANCHOR, json!([0.25, 0.1])),
        ];
        given(&transform, 3)
    }

    fn read(effects: &[Value], shape: &Shape) -> Option<Picture> {
        take(effects, None, shape).expect("read").1
    }

    #[test]
    fn a_fitted_pictures_anchor_leaves_its_position_to_the_pan() {
        let transform = [
            (ZOOM_X, json!(2.0)),
            (ZOOM_Y, json!(2.0)),
            (ROTATION, json!(30.0)),
            (PAN, json!(0.1)),
            (ANCHOR, json!([0.25, 0.1])),
        ];
        let got = read(&given(&transform, FIT), &Shape::whole((1920, 1080)));
        let want = Picture {
            position: Some([0.6, 0.5]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(30.0),
            anchor: Some([0.75, 0.4]),
            ..Picture::default()
        };
        assert_eq!(got, Some(want));
    }

    #[test]
    fn a_filled_pictures_scaling_move_goes_into_its_position() {
        let want = Picture {
            position: Some([0.5625, 0.466667]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(90.0),
            anchor: Some([0.75, 0.4]),
            ..Picture::default()
        };
        assert_eq!(read(&filled_turned(), &filled()), Some(want));
    }

    #[test]
    fn a_filled_pictures_middle_lands_where_the_file_puts_it() {
        let picture = read(&filled_turned(), &filled()).expect("a picture");
        let placed = Placed {
            clip: "c1".into(),
            picture,
            run: None,
        };
        let fitted = [240.0, 0.0, 1440.0, 1080.0];
        let level = Placing::new(&[placed], (1920.0, 1080.0), fitted).levels[0];
        let middle = level
            .forward([0.5, 0.5])
            .map(|v| (v * 100.0).round() / 100.0);
        assert_eq!(middle, [1656.0, 1116.0]);
    }

    #[test]
    fn a_transform_without_an_anchor_reads_without_one() {
        let transform = [(ZOOM_X, json!(2.0)), (ZOOM_Y, json!(2.0))];
        let got = read(&given(&transform, FIT), &Shape::whole((1920, 1080)));
        let want = Picture {
            zoom: Some([2.0, 2.0]),
            ..Picture::default()
        };
        assert_eq!(got, Some(want));
    }

    #[test]
    fn the_anchor_is_written_from_the_middle_with_y_up() {
        let p = Picture {
            anchor: Some([0.75, 0.4]),
            ..Picture::default()
        };
        let written = effects(&p, &Shape::whole((1920, 1080)), 0);
        let anchor = written[0]["metadata"][EDITOR]["Parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|q| q["Parameter ID"] == ANCHOR)
            .map(|q| q["Parameter Value"].clone());
        assert_eq!(anchor, Some(json!([0.25, 0.1])));
    }

    #[test]
    fn a_written_anchor_reads_back_the_same() {
        let p = Picture {
            position: Some([0.6, 0.5]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(30.0),
            anchor: Some([0.75, 0.4]),
            ..Picture::default()
        };
        let shape = Shape::whole((1920, 1080));
        assert_eq!(read(&effects(&p, &shape, 0), &shape), Some(p));
    }
}
