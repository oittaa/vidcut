use crate::error::{Error, Result};
use crate::generators::{Axis, checked_center, name};
use crate::model::{Generator, GeneratorKind, Gradient, Project};
use crate::request::GeneratorSettings;

use super::keyframes::put_value;
use super::subtitles::checked_color;
use super::titles::add_block;
use super::{locate, unlocked};

const GRADIENT_TAKES: [&str; 6] = [
    "center_x",
    "center_y",
    "upper_left",
    "upper_right",
    "lower_left",
    "lower_right",
];

fn rgb_of(color: &str) -> Result<[u8; 3]> {
    let shown = checked_color(Some(color), "generator")?.unwrap_or_default();
    crate::subtitles::rgb(&shown).ok_or_else(|| Error::invalid("a color is #rrggbb"))
}

fn refuse(given: &GeneratorSettings, kind: &str, takes: &[&str]) -> Result<()> {
    for (name, there) in given.named() {
        if there && !takes.contains(&name) {
            return Err(Error::invalid(format!("{kind} has no `{name}`")));
        }
    }
    Ok(())
}

fn put_color(slot: &mut [u8; 3], color: Option<&str>) -> Result<()> {
    if let Some(color) = color {
        *slot = rgb_of(color)?;
    }
    Ok(())
}

fn put_center(g: &mut Gradient, axis: Axis, value: f64, frame: Option<i64>) -> Result<()> {
    let value = checked_center(value)?;
    if g.keys(axis).is_empty() {
        *g.plain_mut(axis) = value;
        return Ok(());
    }
    let frame = frame
        .ok_or_else(|| Error::usage("the center has keyframes: give `at` for the keyframe"))?;
    put_value(g.keys_mut(axis), frame, value);
    Ok(())
}

fn put_gradient(g: &mut Gradient, s: &GeneratorSettings, frame: Option<i64>) -> Result<()> {
    refuse(s, Generator::FOUR_COLOR_GRADIENT, &GRADIENT_TAKES)?;
    put_color(&mut g.upper_left, s.upper_left.as_deref())?;
    put_color(&mut g.upper_right, s.upper_right.as_deref())?;
    put_color(&mut g.lower_left, s.lower_left.as_deref())?;
    put_color(&mut g.lower_right, s.lower_right.as_deref())?;
    for (axis, value) in [(Axis::X, s.center_x), (Axis::Y, s.center_y)] {
        if let Some(v) = value {
            put_center(g, axis, v, frame)?;
        }
    }
    Ok(())
}

pub fn made(kind: GeneratorKind, s: &GeneratorSettings) -> Result<Generator> {
    let plain = |g: Generator| {
        refuse(s, name(&g), &[])?;
        Ok(g)
    };
    match kind {
        GeneratorKind::SolidColor => {
            refuse(s, Generator::SOLID_COLOR, &["color"])?;
            Ok(Generator::SolidColor {
                color: s.color.as_deref().map_or(Ok([0, 0, 0]), rgb_of)?,
            })
        }
        GeneratorKind::FourColorGradient => {
            let mut g = Gradient::default();
            put_gradient(&mut g, s, None)?;
            Ok(Generator::FourColorGradient(g))
        }
        GeneratorKind::TenStep => plain(Generator::TenStep),
        GeneratorKind::HundredMvSteps => plain(Generator::HundredMvSteps),
        GeneratorKind::GreyScale => plain(Generator::GreyScale),
        GeneratorKind::YcbcrRamp => plain(Generator::YcbcrRamp),
        GeneratorKind::EbuColorBar => {
            refuse(s, Generator::EBU_COLOR_BAR, &["level"])?;
            Ok(Generator::EbuColorBar {
                level: s.level.unwrap_or_default(),
            })
        }
        GeneratorKind::SmpteColorBar => plain(Generator::SmpteColorBar),
        GeneratorKind::Bt2111HlgNarrow => plain(Generator::Bt2111HlgNarrow),
        GeneratorKind::Bt2111PqFull => plain(Generator::Bt2111PqFull),
        GeneratorKind::Bt2111PqNarrow => plain(Generator::Bt2111PqNarrow),
        GeneratorKind::Window => plain(Generator::Window),
    }
}

pub fn add(
    p: &mut Project,
    track: Option<&str>,
    start: i64,
    duration: i64,
    generator: Generator,
) -> Result<(String, String)> {
    add_block(
        p,
        "generator",
        track,
        (start, duration),
        None,
        Some(generator),
    )
}

pub fn set(p: &mut Project, id: &str, s: &GeneratorSettings, at: Option<i64>) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    unlocked(p, ti)?;
    let c = &mut p.tracks[ti].clips[ci];
    let frame = at.map(|t| c.content_at(t));
    match &mut c.generator {
        Some(Generator::SolidColor { color }) => {
            refuse(s, Generator::SOLID_COLOR, &["color"])?;
            put_color(color, s.color.as_deref())
        }
        Some(Generator::FourColorGradient(g)) => {
            let mut next = g.clone();
            put_gradient(&mut next, s, frame)?;
            *g = next;
            Ok(())
        }
        Some(Generator::EbuColorBar { level }) => {
            refuse(s, Generator::EBU_COLOR_BAR, &["level"])?;
            if let Some(given) = s.level {
                *level = given;
            }
            Ok(())
        }
        Some(Generator::Carried { .. }) => Err(Error::invalid(format!(
            "{id} is a block from another editor's file, which plays as a gap"
        ))),
        Some(other) => refuse(s, name(other), &[]),
        None => Err(Error::invalid(format!("{id} is not a generator"))),
    }
}
