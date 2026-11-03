use crate::error::{Error, Result};
use crate::model::{Generator, GeneratorKind, Project};

use super::subtitles::checked_color;
use super::titles::add_block;
use super::{locate, unlocked};

fn rgb_of(color: &str) -> Result<[u8; 3]> {
    let shown = checked_color(Some(color), "generator")?.unwrap_or_default();
    crate::subtitles::rgb(&shown).ok_or_else(|| Error::invalid("a color is #rrggbb"))
}

pub fn made(kind: GeneratorKind, color: Option<&str>) -> Result<Generator> {
    match kind {
        GeneratorKind::SolidColor => Ok(Generator::SolidColor {
            color: color.map_or(Ok([0, 0, 0]), rgb_of)?,
        }),
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

pub fn set(p: &mut Project, id: &str, new_color: Option<&str>) -> Result<()> {
    let (ti, ci) = locate(p, id)?;
    unlocked(p, ti)?;
    let new_color = new_color.map(rgb_of).transpose()?;
    match &mut p.tracks[ti].clips[ci].generator {
        Some(Generator::SolidColor { color }) => {
            if let Some(new) = new_color {
                *color = new;
            }
            Ok(())
        }
        Some(Generator::Carried { .. }) => Err(Error::invalid(format!(
            "{id} is another editor's generator, which plays as a gap"
        ))),
        None => Err(Error::invalid(format!("{id} is not a generator"))),
    }
}
