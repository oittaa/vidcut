mod srt;
mod text;
mod ttml;
mod vtt;

use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{Place, Settings};
use crate::request::SubtitleFormat;

pub use text::{Encoding, plain};

#[derive(Debug, Clone, PartialEq)]
pub struct Timed {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub place: Place,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Read {
    pub cues: Vec<Timed>,
    pub warnings: Vec<String>,
}

pub fn clock(secs: f64, separator: char) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as i64;
    format!(
        "{:02}:{:02}:{:02}{separator}{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

pub fn format_of(path: &Path) -> Option<SubtitleFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "srt" => Some(SubtitleFormat::Srt),
        "vtt" | "webvtt" => Some(SubtitleFormat::Vtt),
        "ttml" | "dfxp" | "xml" => Some(SubtitleFormat::Ttml),
        _ => None,
    }
}

pub fn extension(format: SubtitleFormat) -> &'static str {
    match format {
        SubtitleFormat::Srt => "srt",
        SubtitleFormat::Vtt => "vtt",
        SubtitleFormat::Ttml => "ttml",
    }
}

pub fn sniff(source: &str) -> Option<SubtitleFormat> {
    let head = source.trim_start();
    if head.starts_with("WEBVTT") {
        Some(SubtitleFormat::Vtt)
    } else if head.starts_with('<') {
        Some(SubtitleFormat::Ttml)
    } else if head.lines().take(4).any(|l| l.contains("-->")) {
        Some(SubtitleFormat::Srt)
    } else {
        None
    }
}

pub fn parse(bytes: &[u8], path: &Path) -> Result<(Read, SubtitleFormat, Encoding)> {
    let (source, encoding) = text::decode(bytes);
    let format = sniff(&source).or_else(|| format_of(path)).ok_or_else(|| {
        Error::invalid(format!(
            "{}: not a subtitle file vidcut reads (SRT, WebVTT, TTML)",
            path.display()
        ))
    })?;
    let mut read = match format {
        SubtitleFormat::Srt => srt::read(&source),
        SubtitleFormat::Vtt => vtt::read(&source)?,
        SubtitleFormat::Ttml => ttml::read(&source)?,
    };
    read.cues.retain(|c| !c.text.trim().is_empty());
    Ok((read, format, encoding))
}

pub struct Writing<'a> {
    pub language: Option<&'a str>,
    pub frame_rate: Option<f64>,
    pub positions: bool,
}

pub fn write(cues: &[Timed], format: SubtitleFormat, how: &Writing) -> String {
    match format {
        SubtitleFormat::Srt => srt::write(cues, how.positions),
        SubtitleFormat::Vtt => vtt::write(cues),
        SubtitleFormat::Ttml => ttml::write(cues, how.language, how.frame_rate),
    }
}

pub fn normalize(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    self::text::render(&self::text::tokens(&lines.join("\n")))
}

pub fn origin(settings: &Settings) -> f64 {
    settings.start_tc as f64 / settings.fps.as_f64()
}

pub struct Beside {
    pub path: std::path::PathBuf,
    pub track: String,
    pub cues: usize,
    pub text: String,
}

pub fn beside(
    p: &crate::model::Project,
    output: &Path,
    range: (i64, i64),
    tracks: Option<&[String]>,
    format: Option<SubtitleFormat>,
    timeline: bool,
) -> Result<Vec<Beside>> {
    let format = format.unwrap_or(SubtitleFormat::Srt);
    let chosen: Vec<usize> = match tracks {
        Some(ids) => ids
            .iter()
            .map(|id| {
                p.subtitle_index(id)
                    .ok_or_else(|| Error::not_found(format!("no subtitle track {id}")))
            })
            .collect::<Result<_>>()?,
        None if timeline => (0..p.subtitles.len())
            .filter(|&i| !p.subtitles[i].cues.is_empty())
            .collect(),
        None => Vec::new(),
    };
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = output.parent().unwrap_or(Path::new(""));
    let several = chosen.len() > 1;
    let mut out: Vec<Beside> = Vec::new();
    for ti in chosen {
        let t = &p.subtitles[ti];
        let tag = match (&t.language, several) {
            (Some(l), _) => Some(l.clone()),
            (None, true) => Some(t.display_name()),
            (None, false) => None,
        };
        let ext = extension(format);
        let mut name = match &tag {
            Some(tag) => format!("{stem}.{tag}.{ext}"),
            None => format!("{stem}.{ext}"),
        };
        if out.iter().any(|b| b.path == dir.join(&name)) {
            name = format!("{stem}.{}.{ext}", t.id);
        }
        let cues = crate::ops::subtitles::collect(p, ti, range, timeline);
        let text = write(
            &cues,
            format,
            &Writing {
                language: t.language.as_deref(),
                frame_rate: Some(p.settings.fps.as_f64()),
                positions: !timeline,
            },
        );
        out.push(Beside {
            path: dir.join(name),
            track: t.id.clone(),
            cues: cues.len(),
            text,
        });
    }
    Ok(out)
}
