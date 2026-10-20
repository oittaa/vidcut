mod languages;
mod srt;
mod text;
mod ttml;
mod vtt;

use std::path::Path;

use crate::error::{Error, Result};
use crate::model::{Place, Settings, SubtitleStyle};
use crate::request::SubtitleFormat;

pub use languages::three_letter;
pub use text::{Encoding, Tag, Token, color, plain, rgb, tokens};

#[derive(Debug, Clone, PartialEq)]
pub struct Timed {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub place: Place,
    pub words: Vec<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Read {
    pub cues: Vec<Timed>,
    pub warnings: Vec<String>,
    pub language: Option<String>,
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

pub fn is_file(path: &Path) -> bool {
    format_of(path).is_some()
}

pub fn pool_item(
    bytes: &[u8],
    path: &Path,
) -> Result<(crate::model::MediaInfo, crate::model::SubtitleFile)> {
    let (read, format, encoding) = parse(bytes, path)?;
    let first = read
        .cues
        .iter()
        .min_by(|a, b| a.start.total_cmp(&b.start))
        .ok_or_else(|| Error::invalid(format!("{}: no cues", path.display())))?;
    let end = read.cues.iter().map(|c| c.end).fold(first.end, f64::max);
    let info = crate::model::MediaInfo {
        format: extension(format).to_string(),
        duration: end,
        start_time: first.start,
        video: None,
        audio: Vec::new(),
        timecode: None,
    };
    let file = crate::model::SubtitleFile {
        format,
        encoding: encoding.name().to_string(),
        cues: read.cues.len(),
        language: read.language.clone(),
        first: plain(&first.text),
        warnings: read.warnings.clone(),
    };
    Ok((info, file))
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
    pub style: &'a SubtitleStyle,
}

pub struct Written {
    pub text: String,
    pub warnings: Vec<String>,
}

pub fn write(cues: &[Timed], format: SubtitleFormat, how: &Writing) -> Written {
    let (text, warnings) = match format {
        SubtitleFormat::Srt => (srt::write(cues, how.positions), Vec::new()),
        SubtitleFormat::Vtt => (vtt::write(cues), Vec::new()),
        SubtitleFormat::Ttml => ttml::write(cues, how.language, how.frame_rate, how.style),
    };
    Written { text, warnings }
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

pub fn words(text: &str) -> Vec<String> {
    plain(text).split_whitespace().map(String::from).collect()
}

pub fn spread_words(text: &str, duration: i64) -> Vec<i64> {
    let words = words(text);
    let total: usize = words.iter().map(|w| w.chars().count() + 1).sum();
    let mut before = 0;
    words
        .iter()
        .map(|w| {
            let at = (before as f64 / total.max(1) as f64 * duration as f64).floor() as i64;
            before += w.chars().count() + 1;
            at.clamp(0, (duration - 1).max(0))
        })
        .collect()
}

pub fn fill_times(known: &[Option<f64>], weights: &[f64], start: f64, end: f64) -> Vec<f64> {
    let n = known.len();
    let mut at = vec![0.0; n + 1];
    for i in 0..n {
        at[i + 1] = at[i] + weights.get(i).copied().unwrap_or(1.0).max(0.0);
    }
    let mut out: Vec<f64> = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        if let Some(t) = known[i] {
            out.push(t);
            i += 1;
            continue;
        }
        let j = (i..n).find(|&k| known[k].is_some()).unwrap_or(n);
        let (t0, w0) = match i.checked_sub(1) {
            Some(b) => (out[b], at[b]),
            None => (start, 0.0),
        };
        let (t1, w1) = match known.get(j).copied().flatten() {
            Some(t) => (t, at[j]),
            None => (end, at[n]),
        };
        for &w in &at[i..j] {
            let share = if w1 > w0 { (w - w0) / (w1 - w0) } else { 0.0 };
            out.push(t0 + share * (t1 - t0));
        }
        i = j;
    }
    let mut last = f64::MIN;
    for t in &mut out {
        last = t.max(last);
        *t = last;
    }
    out
}

pub fn origin(settings: &Settings) -> f64 {
    settings.start_tc as f64 / settings.fps.as_f64()
}

pub struct Beside {
    pub path: std::path::PathBuf,
    pub track: String,
    pub cues: usize,
    pub text: String,
    pub warnings: Vec<String>,
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
        let written = write(
            &cues,
            format,
            &Writing {
                language: t.language.as_deref(),
                frame_rate: Some(p.settings.fps.as_f64()),
                positions: !timeline,
                style: &t.style,
            },
        );
        out.push(Beside {
            path: dir.join(name),
            track: t.id.clone(),
            cues: cues.len(),
            text: written.text,
            warnings: written.warnings,
        });
    }
    Ok(out)
}
