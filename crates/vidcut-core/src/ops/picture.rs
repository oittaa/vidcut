use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, Crop, Picture, PictureTool, Project, TrackKind};

use super::locate;

const ZOOM: (f64, f64) = (0.01, 100.0);
const REACH: f64 = 100.0;
const TURNS: f64 = 3600.0;

pub fn checked(p: &Picture) -> Result<Option<Picture>> {
    if let Some(at) = p.position
        && !at.iter().all(|v| v.is_finite() && v.abs() <= REACH)
    {
        return Err(Error::invalid(
            "a picture's position is two fractions of the frame, each from -100 to 100",
        ));
    }
    if let Some(z) = p.zoom
        && !z
            .iter()
            .all(|v| v.is_finite() && (ZOOM.0..=ZOOM.1).contains(v))
    {
        return Err(Error::invalid("a picture's zoom is from 1/100 to 100"));
    }
    if let Some(r) = p.rotation
        && !(r.is_finite() && r.abs() <= TURNS)
    {
        return Err(Error::invalid(
            "a picture's rotation is degrees, from -3600 to 3600",
        ));
    }
    let c = p.crop;
    if !c
        .sides()
        .iter()
        .all(|v| v.is_finite() && (0.0..1.0).contains(v))
        || c.left + c.right >= 1.0
        || c.top + c.bottom >= 1.0
    {
        return Err(Error::invalid(
            "a picture's crop is a fraction of it on each side, from 0, leaving some of it",
        ));
    }
    let out = Picture {
        position: p.position.filter(|at| *at != Picture::MIDDLE),
        zoom: p.zoom.filter(|z| *z != [1.0, 1.0]),
        rotation: p.rotation.filter(|r| *r != 0.0),
        ..p.clone()
    };
    Ok((!out.is_usual()).then_some(out))
}

pub fn has_picture(c: &Clip) -> bool {
    c.video && c.title.is_none() && c.generator.is_none()
}

fn takes(p: &Project, ti: usize, c: &Clip) -> Result<()> {
    if c.title.is_some() {
        return Err(Error::invalid(format!(
            "{} is a title, placed by its own position, zoom and rotation (titles.set)",
            c.id
        )));
    }
    if c.generator.is_some() {
        return Err(Error::invalid(format!(
            "{} is another editor's generator, which plays as a gap",
            c.id
        )));
    }
    if !c.video || p.tracks[ti].kind != TrackKind::Video {
        return Err(Error::invalid(format!("{} has no picture", c.id)));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PictureChange {
    pub position: Option<Option<[f64; 2]>>,
    pub zoom: Option<[f64; 2]>,
    pub rotation: Option<f64>,
    pub flip_horizontal: Option<bool>,
    pub flip_vertical: Option<bool>,
    pub crop: [Option<f64>; 4],
}

impl PictureChange {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

pub fn changed(old: &Picture, change: &PictureChange) -> Picture {
    let mut p = old.clone();
    if let Some(at) = change.position {
        p.position = at;
    }
    if let Some(z) = change.zoom {
        p.zoom = Some(z);
    }
    if let Some(r) = change.rotation {
        p.rotation = Some(r);
    }
    if let Some(f) = change.flip_horizontal {
        p.flip_horizontal = f;
    }
    if let Some(f) = change.flip_vertical {
        p.flip_vertical = f;
    }
    let [left, right, top, bottom] = change.crop;
    let c = &mut p.crop;
    for (side, to) in [
        (&mut c.left, left),
        (&mut c.right, right),
        (&mut c.top, top),
        (&mut c.bottom, bottom),
    ] {
        if let Some(v) = to {
            *side = v;
        }
    }
    p
}

pub fn set(p: &mut Project, ids: &[String], change: &PictureChange) -> Result<()> {
    if change.is_empty() {
        return Err(Error::usage(
            "picture.set needs a position, zoom, rotation, flip or crop",
        ));
    }
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let c = &p.tracks[ti].clips[ci];
        takes(p, ti, c)?;
        let old = c.picture.clone().unwrap_or_default();
        let picture = checked(&changed(&old, change))?;
        p.tracks[ti].clips[ci].picture = picture;
    }
    Ok(())
}

pub fn clear(p: &mut Project, ids: &[String], tool: Option<PictureTool>) -> Result<()> {
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let c = &mut p.tracks[ti].clips[ci];
        let Some(old) = c.picture.take() else {
            continue;
        };
        let kept = match tool {
            None => Picture::default(),
            Some(PictureTool::Transform) => Picture {
                crop: old.crop,
                ..Picture::default()
            },
            Some(PictureTool::Crop) => Picture {
                crop: Crop::default(),
                ..old
            },
        };
        c.picture = (!kept.is_usual()).then_some(kept);
    }
    Ok(())
}

pub fn shown(c: &Clip) -> Value {
    let p = c.picture.clone().unwrap_or_default();
    let crop = p.crop;
    json!({
        "clip": c.id,
        "position": p.place(),
        "zoom": p.scale(),
        "rotation": p.turn(),
        "flip_horizontal": p.flip_horizontal,
        "flip_vertical": p.flip_vertical,
        "crop": {
            "left": crop.left, "right": crop.right, "top": crop.top, "bottom": crop.bottom,
        },
    })
}

pub fn get(p: &Project, id: &str) -> Result<Value> {
    let (ti, ci) = p
        .find_clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    let c = &p.tracks[ti].clips[ci];
    takes(p, ti, c)?;
    Ok(shown(c))
}

pub fn position(raw: &[f64]) -> Result<Option<[f64; 2]>> {
    match raw {
        [] => Ok(None),
        [x, y] => Ok(Some([*x, *y])),
        _ => Err(Error::usage(
            "a picture's position is two fractions, x and y, or none for the middle",
        )),
    }
}

pub fn zoom(raw: &[f64]) -> Result<[f64; 2]> {
    match raw {
        [z] => Ok([*z, *z]),
        [x, y] => Ok([*x, *y]),
        _ => Err(Error::usage(
            "a picture's zoom is one number for both ways, or two, across and down",
        )),
    }
}

pub fn violation(p: &Project, ti: usize, c: &Clip) -> Option<String> {
    let picture = c.picture.as_ref()?;
    if let Err(e) = takes(p, ti, c) {
        return Some(e.message);
    }
    checked(picture)
        .err()
        .map(|e| format!("{}: {}", c.id, e.message))
}
