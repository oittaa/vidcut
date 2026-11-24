use crate::error::{Error, Result};
use crate::keyframes::{PictureValue, plain_channel, scaled};
use crate::model::{Clip, ClipRetime, Picture, Project};
use crate::render::FileClock;
use crate::request::{ClipAttribute, KeyTiming};

use super::locate;
use super::picture::takes;

type A = ClipAttribute;

fn channel(a: A) -> Option<(PictureValue, Option<usize>)> {
    Some(match a {
        A::Opacity => (PictureValue::Opacity, None),
        A::Position => (PictureValue::Position, None),
        A::Rotation => (PictureValue::Rotation, None),
        A::Anchor => (PictureValue::Anchor, None),
        A::Pitch => (PictureValue::Pitch, None),
        A::Yaw => (PictureValue::Yaw, None),
        A::ZoomX => (PictureValue::Zoom, Some(0)),
        A::ZoomY => (PictureValue::Zoom, Some(1)),
        A::CropLeft => (PictureValue::CropLeft, None),
        A::CropRight => (PictureValue::CropRight, None),
        A::CropTop => (PictureValue::CropTop, None),
        A::CropBottom => (PictureValue::CropBottom, None),
        A::CropSoftness => (PictureValue::CropSoftness, None),
        _ => return None,
    })
}

fn picture_only(a: A) -> bool {
    channel(a).is_some() || matches!(a, A::CompositeMode | A::FlipVertical | A::FlipHorizontal)
}

fn made_only(a: A) -> bool {
    matches!(a, A::VideoEffects | A::Grade | A::Composition)
}

pub fn picked_at_open(c: &Clip) -> Vec<ClipAttribute> {
    let usual = Picture::default();
    let picture = c.picture.clone().unwrap_or_default();
    let audio = c.audio.as_ref();
    let differs = |v: PictureValue, axis: Option<usize>| {
        let (now, was) = (plain_channel(&picture, v), plain_channel(&usual, v));
        picture.has_keys(v)
            || match axis {
                Some(i) => now[i] != was[i],
                None => now != was,
            }
    };
    let all = [
        A::ClipColor,
        A::CompositeMode,
        A::Opacity,
        A::Position,
        A::Rotation,
        A::Anchor,
        A::Pitch,
        A::Yaw,
        A::ZoomX,
        A::ZoomY,
        A::CropLeft,
        A::CropRight,
        A::CropTop,
        A::CropBottom,
        A::CropSoftness,
        A::FlipVertical,
        A::FlipHorizontal,
        A::RetimeProcess,
        A::VideoEffects,
        A::Grade,
        A::Composition,
        A::Volume,
        A::AudioEffects,
        A::Equalizer,
    ];
    all.into_iter()
        .filter(|a| match *a {
            A::ClipColor => true,
            A::CompositeMode => picture.composite_mode.is_some(),
            A::FlipVertical => picture.flip_vertical,
            A::FlipHorizontal => picture.flip_horizontal,
            A::RetimeProcess => c.retime != ClipRetime::Project,
            A::VideoEffects => !picture.effects.is_empty(),
            A::Grade => picture.grade.is_some(),
            A::Composition => picture.composition.is_some(),
            A::Volume => audio.is_some_and(|a| a.gain_db != 0.0 || !a.gain_keys.is_empty()),
            A::AudioEffects => {
                audio.is_some_and(|a| a.compressor.is_some() || a.noise_reduction.is_some())
            }
            A::Equalizer => audio.is_some_and(|a| a.equalizer.is_some()),
            a => channel(a).is_some_and(|(v, axis)| differs(v, axis)),
        })
        .collect()
}

fn opening(p: &Project, c: &Clip) -> i64 {
    FileClock::of(p, c).frame(c.content_at(c.start))
}

fn put(p: &Project, ti: usize, from: &Clip, to: &mut Clip, attributes: &[A], timing: KeyTiming) {
    let a = from.content_at(from.start);
    let b = to.content_at(to.start);
    let ratio = match timing {
        KeyTiming::Maintain => 1.0,
        KeyTiming::Stretch => to.duration as f64 / from.duration.max(1) as f64,
    };
    let conv = |f: i64| b + ((f - a) as f64 * ratio).round() as i64;
    let mut moved = from.picture.clone().unwrap_or_default();
    moved.scale_times(&conv, &|_| ratio);
    let shows = takes(p, ti, to).is_ok();
    let made = shows && !to.generates();
    let both_zooms = attributes.contains(&A::ZoomX) && attributes.contains(&A::ZoomY);
    let comp_start = opening(p, to) - opening(p, from);
    for &attribute in attributes {
        if (picture_only(attribute) && !shows) || (made_only(attribute) && !made) {
            continue;
        }
        let picture = to.picture.get_or_insert_with(Default::default);
        match (attribute, channel(attribute)) {
            (A::ZoomY, _) if both_zooms => {}
            (A::ZoomX, _) if both_zooms => picture.take_channel(&moved, PictureValue::Zoom),
            (_, Some((v, None))) => picture.take_channel(&moved, v),
            (_, Some((v, Some(axis)))) => picture.take_axis(&moved, v, axis),
            (A::CompositeMode, _) => picture.composite_mode = moved.composite_mode,
            (A::FlipVertical, _) => picture.flip_vertical = moved.flip_vertical,
            (A::FlipHorizontal, _) => picture.flip_horizontal = moved.flip_horizontal,
            (A::VideoEffects, _) => picture.effects.clone_from(&moved.effects),
            (A::Grade, _) => picture.grade.clone_from(&moved.grade),
            (A::Composition, _) => {
                picture.composition = moved.composition.clone().map(|mut c| {
                    c.start += comp_start;
                    c
                })
            }
            (A::ClipColor, _) => to.color = p.clip_color(from).map(str::to_owned),
            (A::RetimeProcess, _) => {
                if to.video && !to.media.is_empty() {
                    to.retime = from.retime;
                }
            }
            (A::Volume | A::AudioEffects | A::Equalizer, _) => {
                let (Some(had), Some(sound)) = (&from.audio, to.audio.as_mut()) else {
                    continue;
                };
                match attribute {
                    A::Volume => {
                        sound.gain_db = had.gain_db;
                        sound.gain_keys = scaled(&had.gain_keys, conv, ratio);
                    }
                    A::AudioEffects => {
                        sound.compressor.clone_from(&had.compressor);
                        sound.noise_reduction.clone_from(&had.noise_reduction);
                    }
                    _ => sound.equalizer.clone_from(&had.equalizer),
                }
            }
            _ => {}
        }
    }
}

fn cleared(p: &Project, ti: usize, to: &mut Clip, attributes: &[A]) {
    let usual = Picture::default();
    let shows = takes(p, ti, to).is_ok();
    let both_zooms = attributes.contains(&A::ZoomX) && attributes.contains(&A::ZoomY);
    for &attribute in attributes {
        if picture_only(attribute) && !shows {
            continue;
        }
        let picture = to.picture.get_or_insert_with(Default::default);
        match (attribute, channel(attribute)) {
            (A::ZoomY, _) if both_zooms => {}
            (A::ZoomX, _) if both_zooms => picture.take_channel(&usual, PictureValue::Zoom),
            (_, Some((v, None))) => picture.take_channel(&usual, v),
            (_, Some((v, Some(axis)))) => picture.take_axis(&usual, v, axis),
            (A::CompositeMode, _) => picture.composite_mode = None,
            (A::FlipVertical, _) => picture.flip_vertical = false,
            (A::FlipHorizontal, _) => picture.flip_horizontal = false,
            (A::VideoEffects, _) => picture.effects.clear(),
            (A::Grade, _) => picture.grade = None,
            (A::Composition, _) => picture.composition = None,
            (A::ClipColor, _) => to.color = None,
            (A::RetimeProcess, _) => to.retime = ClipRetime::Project,
            (_, _) => {
                let Some(sound) = to.audio.as_mut() else {
                    continue;
                };
                match attribute {
                    A::Volume => {
                        sound.gain_db = 0.0;
                        sound.gain_keys.clear();
                    }
                    A::AudioEffects => {
                        sound.compressor = None;
                        sound.noise_reduction = None;
                    }
                    A::Equalizer => sound.equalizer = None,
                    A::VoiceIsolation => sound.voice_isolation = None,
                    A::DialogueLeveler => sound.dialogue_leveler = None,
                    _ => {}
                }
            }
        }
    }
}

fn speeds(p: &mut Project, ids: &[String], speed: f64, ripple: bool) -> Result<()> {
    let takers: Vec<String> = (ids.iter())
        .filter(|id| p.clip(id).is_some_and(Clip::takes_speed))
        .cloned()
        .collect();
    match takers.is_empty() {
        true => Ok(()),
        false => super::change_speed(p, &takers, speed, Some(ripple), None),
    }
}

fn save(p: &mut Project, ti: usize, ci: usize, mut c: Clip) -> Result<()> {
    c.picture = match &c.picture {
        Some(x) => super::picture::checked(x)?,
        None => None,
    };
    p.tracks[ti].clips[ci] = c;
    Ok(())
}

pub fn paste(
    p: &mut Project,
    from: &Clip,
    ids: &[String],
    attributes: &[ClipAttribute],
    timing: KeyTiming,
    ripple: bool,
) -> Result<()> {
    if (attributes.iter()).any(|a| matches!(a, A::VoiceIsolation | A::DialogueLeveler)) {
        return Err(Error::usage(
            "voice_isolation and dialogue_leveler go only to clip.remove_attributes",
        ));
    }
    if attributes.contains(&A::Speed) && from.takes_speed() {
        let speed = from.constant_speed().ok_or_else(|| {
            Error::invalid("the copied clip's speed has keyframes: it does not paste")
        })?;
        speeds(p, ids, speed, ripple)?;
    }
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let mut c = p.tracks[ti].clips[ci].clone();
        put(p, ti, from, &mut c, attributes, timing);
        save(p, ti, ci, c)?;
    }
    Ok(())
}

pub fn remove(
    p: &mut Project,
    ids: &[String],
    attributes: &[ClipAttribute],
    ripple: bool,
) -> Result<()> {
    if attributes.contains(&A::Speed) {
        speeds(p, ids, 1.0, ripple)?;
    }
    for id in ids {
        let (ti, ci) = locate(p, id)?;
        let mut c = p.tracks[ti].clips[ci].clone();
        cleared(p, ti, &mut c, attributes);
        save(p, ti, ci, c)?;
    }
    Ok(())
}
