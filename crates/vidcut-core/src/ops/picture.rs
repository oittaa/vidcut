use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::keyframes::PictureValue;
use crate::model::{
    Clip, CompositeMode, Crop, DynamicZoom, Picture, PictureTool, Project, TrackKind, ZoomBox,
};
use crate::request::DynamicZoomSet;

use super::locate;

const ZOOM: (f64, f64) = (0.01, 100.0);
const REACH: f64 = 100.0;
const TURNS: f64 = 3600.0;
const LEAN: f64 = 1.5;
const SOFT: f64 = 100.0;

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
    if let Some(at) = p.anchor
        && !at.iter().all(|v| v.is_finite() && v.abs() <= REACH)
    {
        return Err(Error::invalid(
            "a picture's anchor is two fractions of the picture, each from -100 to 100",
        ));
    }
    if let Some(v) = p.pitch
        && !(v.is_finite() && v.abs() <= LEAN)
    {
        return Err(Error::invalid("a picture's pitch is from -1.5 to 1.5"));
    }
    if let Some(v) = p.yaw
        && !(v.is_finite() && v.abs() <= LEAN)
    {
        return Err(Error::invalid("a picture's yaw is from -1.5 to 1.5"));
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
    if let Some(s) = p.crop_softness
        && !(s.is_finite() && s.abs() <= SOFT)
    {
        return Err(Error::invalid(
            "a picture's crop softness is from -100 to 100",
        ));
    }
    if let Some(z) = &p.dynamic_zoom {
        zoom_checked(z)?;
    }
    if let Some(o) = p.opacity
        && !(o.is_finite() && (0.0..=1.0).contains(&o))
    {
        return Err(Error::invalid("a picture's opacity is from 0 to 1"));
    }
    if let Some(why) = crate::keyframes::picture_problem(p) {
        return Err(Error::invalid(why));
    }
    let out = Picture {
        position: p.position.filter(|at| *at != Picture::MIDDLE),
        zoom: p.zoom.filter(|z| *z != [1.0, 1.0]),
        rotation: p.rotation.filter(|r| *r != 0.0),
        anchor: p.anchor.filter(|at| *at != Picture::MIDDLE),
        pitch: p.pitch.filter(|v| *v != 0.0),
        yaw: p.yaw.filter(|v| *v != 0.0),
        crop_softness: p.crop_softness.filter(|s| *s != 0.0),
        composite_mode: p.composite_mode.filter(|m| *m != CompositeMode::Normal),
        opacity: p.opacity.filter(|o| *o != 1.0),
        ..p.clone()
    };
    Ok((!out.is_usual()).then_some(out))
}

fn zoom_checked(z: &DynamicZoom) -> Result<()> {
    for b in [z.start, z.end] {
        if !b.center.iter().all(|v| v.is_finite() && v.abs() <= REACH) {
            return Err(Error::invalid(
                "a Dynamic Zoom box's middle is two fractions of the picture, each from -100 to 100",
            ));
        }
        if !(b.size.is_finite() && (ZOOM.0..=ZOOM.1).contains(&b.size)) {
            return Err(Error::invalid(
                "a Dynamic Zoom box's size is a fraction of the picture, from 1/100 to 100",
            ));
        }
    }
    Ok(())
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
    pub anchor: Option<Option<[f64; 2]>>,
    pub pitch: Option<f64>,
    pub yaw: Option<f64>,
    pub flip_horizontal: Option<bool>,
    pub flip_vertical: Option<bool>,
    pub crop: [Option<f64>; 4],
    pub crop_softness: Option<f64>,
    pub dynamic_zoom: Option<Option<DynamicZoomSet>>,
    pub composite_mode: Option<CompositeMode>,
    pub opacity: Option<f64>,
}

impl PictureChange {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn names(&self, v: PictureValue) -> bool {
        match v {
            PictureValue::Position => self.position.is_some(),
            PictureValue::Zoom => self.zoom.is_some(),
            PictureValue::Rotation => self.rotation.is_some(),
            PictureValue::Anchor => self.anchor.is_some(),
            PictureValue::Pitch => self.pitch.is_some(),
            PictureValue::Yaw => self.yaw.is_some(),
            PictureValue::CropLeft => self.crop[0].is_some(),
            PictureValue::CropRight => self.crop[1].is_some(),
            PictureValue::CropTop => self.crop[2].is_some(),
            PictureValue::CropBottom => self.crop[3].is_some(),
            PictureValue::CropSoftness => self.crop_softness.is_some(),
            PictureValue::Opacity => self.opacity.is_some(),
        }
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
    if let Some(at) = change.anchor {
        p.anchor = at;
    }
    if let Some(v) = change.pitch {
        p.pitch = Some(v);
    }
    if let Some(v) = change.yaw {
        p.yaw = Some(v);
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
    if let Some(s) = change.crop_softness {
        p.crop_softness = Some(s);
    }
    if let Some(z) = change.dynamic_zoom {
        p.dynamic_zoom = z.map(|z| z.on(p.dynamic_zoom.unwrap_or_default()));
    }
    if let Some(m) = change.composite_mode {
        p.composite_mode = Some(m);
    }
    if let Some(o) = change.opacity {
        p.opacity = Some(o);
    }
    p
}

const NEEDS: &str = "picture.set needs a position, zoom, rotation, anchor, pitch, yaw, flip, crop, crop softness, Dynamic Zoom, composite mode or opacity";

pub fn set(p: &mut Project, ids: &[String], change: &PictureChange, at: Option<i64>) -> Result<()> {
    if change.is_empty() {
        return Err(Error::usage(NEEDS));
    }
    let mut applied = false;
    let mut missed = false;
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let c = &p.tracks[ti].clips[ci];
        takes(p, ti, c)?;
        let old = c.picture.clone().unwrap_or_default();
        let next = changed(&old, change);
        let frame = at
            .filter(|&t| c.start <= t && t < c.end())
            .map(|t| c.content_at(t));
        let mut picture = old.clone();
        let mut wrote = false;
        for v in PictureValue::ALL {
            let was = crate::keyframes::plain_channel(&old, v);
            let now = crate::keyframes::plain_channel(&next, v);
            if was == now {
                wrote |= !old.has_keys(v) && change.names(v);
                continue;
            }
            if old.has_keys(v) {
                match frame {
                    Some(f) => {
                        picture.put_channel(v, f, now);
                        wrote = true;
                    }
                    None => missed = true,
                }
            } else {
                crate::keyframes::write_channel(&mut picture, v, now);
                wrote = true;
            }
        }
        if next.flip_horizontal != old.flip_horizontal {
            picture.flip_horizontal = next.flip_horizontal;
            wrote = true;
        }
        if next.flip_vertical != old.flip_vertical {
            picture.flip_vertical = next.flip_vertical;
            wrote = true;
        }
        if next.dynamic_zoom != old.dynamic_zoom {
            picture.dynamic_zoom = next.dynamic_zoom;
            wrote = true;
        }
        if next.mode() != old.mode() {
            picture.composite_mode = next.composite_mode;
            wrote = true;
        }
        if !wrote {
            continue;
        }
        applied = true;
        let picture = checked(&picture)?;
        p.tracks[ti].clips[ci].picture = picture;
    }
    if !applied {
        return Err(if missed {
            Error::invalid("no clip with keyframes on that value shows at the frame")
        } else {
            Error::usage(NEEDS)
        });
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
        let kept = tool.map_or_else(Picture::default, |t| cleared(&old, t));
        c.picture = (!kept.is_usual()).then_some(kept);
    }
    Ok(())
}

pub fn cleared(old: &Picture, tool: PictureTool) -> Picture {
    let old = old.clone();
    match tool {
        PictureTool::Transform => Picture {
            crop: old.crop,
            crop_left_keys: old.crop_left_keys,
            crop_right_keys: old.crop_right_keys,
            crop_top_keys: old.crop_top_keys,
            crop_bottom_keys: old.crop_bottom_keys,
            crop_softness: old.crop_softness,
            crop_softness_keys: old.crop_softness_keys,
            dynamic_zoom: old.dynamic_zoom,
            composite_mode: old.composite_mode,
            opacity: old.opacity,
            opacity_keys: old.opacity_keys,
            ..Picture::default()
        },
        PictureTool::Crop => {
            let mut kept = Picture {
                crop: Crop::default(),
                crop_softness: None,
                ..old
            };
            kept.clear_crop_keys();
            kept
        }
        PictureTool::DynamicZoom => Picture {
            dynamic_zoom: None,
            ..old
        },
        PictureTool::Composite => Picture {
            composite_mode: None,
            opacity: None,
            opacity_keys: Vec::new(),
            ..old
        },
    }
}

pub fn shown(c: &Clip) -> Value {
    let p = c.picture.clone().unwrap_or_default();
    let crop = p.crop;
    json!({
        "clip": c.id,
        "position": p.place(),
        "zoom": p.scale(),
        "rotation": p.turn(),
        "anchor": p.pivot(),
        "pitch": p.pitch(),
        "yaw": p.yaw(),
        "flip_horizontal": p.flip_horizontal,
        "flip_vertical": p.flip_vertical,
        "crop": {
            "left": crop.left, "right": crop.right, "top": crop.top, "bottom": crop.bottom,
            "softness": p.softness(),
        },
        "dynamic_zoom": p.dynamic_zoom.as_ref().map(zoom_shown),
        "composite_mode": p.mode(),
        "opacity": p.opaque(),
    })
}

pub fn zoom_shown(z: &DynamicZoom) -> Value {
    let shown = |b: &ZoomBox| json!({ "center": b.center, "size": b.size });
    json!({ "start": shown(&z.start), "end": shown(&z.end), "ease": z.ease })
}

pub fn get(p: &Project, id: &str) -> Result<Value> {
    let (ti, ci) = p
        .find_clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    let c = &p.tracks[ti].clips[ci];
    takes(p, ti, c)?;
    Ok(shown(c))
}

pub fn point(what: &str, raw: &[f64]) -> Result<Option<[f64; 2]>> {
    match raw {
        [] => Ok(None),
        [x, y] => Ok(Some([*x, *y])),
        _ => Err(Error::usage(format!(
            "a picture's {what} is two fractions, x and y, or none for the middle"
        ))),
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
