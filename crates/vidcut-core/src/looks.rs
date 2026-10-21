use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editor::Env;
use crate::error::{Error, Result};
use crate::model::{
    LetterCase, SubtitleBackground, SubtitleOutline, SubtitleStyle, SubtitleWords, Title,
    TitleAlign, TitleKind, TitleShadow, TitleStyle, WordsShow,
};

/// What a look styles: a subtitle track, or a title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LookKind {
    Subtitles,
    Titles,
}

impl LookKind {
    pub const ALL: [Self; 2] = [Self::Subtitles, Self::Titles];

    pub fn folder(self) -> &'static str {
        match self {
            Self::Subtitles => "subtitles",
            Self::Titles => "titles",
        }
    }
}

/// A subtitle track's look: its style, and how Create Subtitles from Audio
/// lays out the cues it makes with it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubtitleLook {
    /// Characters per line, 1 to 60. Usual: 42.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<u32>,
    /// Lines per cue: 1 or 2. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// The track's style, as `track.set` takes it.
    #[serde(default, skip_serializing_if = "SubtitleStyle::is_empty")]
    pub style: SubtitleStyle,
}

impl SubtitleLook {
    pub const CHARS: u32 = 42;
    pub const LINES: u32 = 1;

    pub fn chars(&self) -> u32 {
        self.chars.unwrap_or(Self::CHARS)
    }

    pub fn lines(&self) -> u32 {
        self.lines.unwrap_or(Self::LINES)
    }
}

/// A title's look: all of a title but its text, as `titles.add` takes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TitleLook {
    /// `text`, `lower_third` or `scroll`. Usual: `text`.
    #[serde(default, skip_serializing_if = "TitleKind::is_text")]
    pub kind: TitleKind,
    /// How a lower third or a scroll lines up: `left`, `middle` or `right`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TitleAlign>,
    /// `[x, y]` as fractions of the picture. Usual: the kind's place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    /// From 1/4 to 4. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    /// Degrees turned counterclockwise. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// From 0 to 1. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "TitleStyle::is_empty")]
    pub style: TitleStyle,
}

impl TitleLook {
    pub fn of(t: &Title) -> Self {
        Self {
            kind: t.kind,
            align: t.align,
            position: t.position,
            zoom: t.zoom,
            rotation: t.rotation,
            opacity: t.opacity,
            style: t.style.clone(),
        }
    }

    pub fn on(&self, text: &str) -> Title {
        Title {
            kind: self.kind,
            align: self.align,
            text: text.to_string(),
            style: self.style.clone(),
            position: self.position,
            zoom: self.zoom,
            rotation: self.rotation,
            opacity: self.opacity,
        }
    }
}

/// A look's content: a subtitle track's or a title's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Styled {
    Subtitles(SubtitleLook),
    Titles(TitleLook),
}

impl Styled {
    pub fn kind(&self) -> LookKind {
        match self {
            Self::Subtitles(_) => LookKind::Subtitles,
            Self::Titles(_) => LookKind::Titles,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub name: String,
    pub description: &'static str,
    pub built_in: bool,
    pub styled: Styled,
}

impl Look {
    pub fn json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "name": self.name,
            "kind": self.styled.kind(),
            "built_in": self.built_in,
        });
        if !self.description.is_empty() {
            v["description"] = self.description.into();
        }
        let (key, look) = match &self.styled {
            Styled::Subtitles(l) => ("subtitles", serde_json::to_value(l)),
            Styled::Titles(l) => ("titles", serde_json::to_value(l)),
        };
        v[key] = look.unwrap_or_default();
        v
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub path: PathBuf,
    pub name: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    pub looks: Vec<Look>,
    pub refused: Vec<Refused>,
}

pub fn dir() -> PathBuf {
    crate::dirs::config().join("looks")
}

fn subtitle(name: &str, description: &'static str, look: SubtitleLook) -> Look {
    Look {
        name: name.into(),
        description,
        built_in: true,
        styled: Styled::Subtitles(look),
    }
}

fn title(name: &str, description: &'static str, look: TitleLook) -> Look {
    Look {
        name: name.into(),
        description,
        built_in: true,
        styled: Styled::Titles(look),
    }
}

fn spoken(size: f64, show: WordsShow, pop: Option<f64>, upper: bool) -> SubtitleStyle {
    SubtitleStyle {
        size: Some(size),
        outline: Some(SubtitleOutline {
            color: None,
            size: Some(0.08),
        }),
        margin: Some(0.15),
        bold: Some(true),
        case: upper.then_some(LetterCase::Upper),
        shadow: Some(TitleShadow::default()),
        words: Some(SubtitleWords {
            show: Some(show),
            color: None,
            pop,
        }),
        ..SubtitleStyle::default()
    }
}

fn laid(chars: u32, style: SubtitleStyle) -> SubtitleLook {
    SubtitleLook {
        chars: Some(chars),
        lines: Some(1),
        style,
    }
}

fn bare() -> Option<SubtitleOutline> {
    Some(SubtitleOutline {
        color: None,
        size: Some(0.0),
    })
}

fn shade(opacity: f64, offset: [f64; 2], blur: f64) -> Option<TitleShadow> {
    Some(TitleShadow {
        color: None,
        opacity: Some(opacity),
        offset: Some(offset),
        blur: Some(blur),
    })
}

fn boxed(color: &str, opacity: f64) -> Option<SubtitleBackground> {
    Some(SubtitleBackground {
        color: color.into(),
        opacity: Some(opacity),
    })
}

pub fn built_in(kind: LookKind) -> Vec<Look> {
    match kind {
        LookKind::Subtitles => vec![
            subtitle("Plain", "The usual style", SubtitleLook::default()),
            subtitle(
                "Pop",
                "The spoken word yellow and popping, in bold capitals",
                laid(20, spoken(0.075, WordsShow::Highlight, Some(1.25), true)),
            ),
            subtitle(
                "Highlight",
                "The spoken word yellow",
                laid(32, spoken(0.065, WordsShow::Highlight, None, false)),
            ),
            subtitle(
                "Reveal",
                "Each word shown as it is spoken",
                laid(32, spoken(0.065, WordsShow::Reveal, None, false)),
            ),
            subtitle(
                "Word",
                "Only the spoken word, large",
                laid(20, spoken(0.1, WordsShow::Word, Some(1.15), true)),
            ),
        ],
        LookKind::Titles => vec![
            title("Plain", "The usual title", TitleLook::default()),
            title(
                "Headline",
                "Large white words with a deep, soft shadow",
                TitleLook {
                    style: TitleStyle {
                        size: Some(0.12),
                        outline: bare(),
                        shadow: shade(1.0, [0.005, 0.009], 30.0),
                        ..TitleStyle::default()
                    },
                    ..TitleLook::default()
                },
            ),
            title(
                "Callout",
                "Large yellow words with a thick outline",
                TitleLook {
                    style: TitleStyle {
                        size: Some(0.11),
                        color: Some("#ffd400".into()),
                        outline: Some(SubtitleOutline {
                            color: None,
                            size: Some(0.1),
                        }),
                        shadow: shade(0.8, [0.004, 0.007], 15.0),
                        ..TitleStyle::default()
                    },
                    ..TitleLook::default()
                },
            ),
            title(
                "Caption",
                "Words on a dark box, low in the picture",
                TitleLook {
                    position: Some([0.5, 0.82]),
                    style: TitleStyle {
                        size: Some(0.055),
                        outline: bare(),
                        background: boxed("#000000", 0.6),
                        ..TitleStyle::default()
                    },
                    ..TitleLook::default()
                },
            ),
            title(
                "News",
                "A lower third on a blue bar",
                TitleLook {
                    kind: TitleKind::LowerThird,
                    style: TitleStyle {
                        size: Some(0.05),
                        outline: bare(),
                        background: boxed("#0b5cad", 0.92),
                        ..TitleStyle::default()
                    },
                    ..TitleLook::default()
                },
            ),
            title(
                "Soft",
                "A lower third with a soft shadow",
                TitleLook {
                    kind: TitleKind::LowerThird,
                    style: TitleStyle {
                        size: Some(0.055),
                        outline: bare(),
                        shadow: shade(1.0, [0.0025, 0.004], 15.0),
                        ..TitleStyle::default()
                    },
                    ..TitleLook::default()
                },
            ),
        ],
    }
}

pub fn checked(styled: &Styled) -> Result<Styled> {
    match styled {
        Styled::Subtitles(l) => {
            if l.chars.is_some_and(|c| !(1..=60).contains(&c)) {
                return Err(Error::invalid(
                    "a look's chars are 1 to 60 characters per line",
                ));
            }
            if l.lines.is_some_and(|n| !(1..=2).contains(&n)) {
                return Err(Error::invalid("a look's lines are 1 or 2"));
            }
            Ok(Styled::Subtitles(SubtitleLook {
                chars: l.chars,
                lines: l.lines,
                style: crate::ops::subtitles::checked_style(&l.style)?,
            }))
        }
        Styled::Titles(l) => Ok(Styled::Titles(TitleLook::of(&crate::ops::titles::checked(
            &l.on(""),
        )?))),
    }
}

const NAME_MOST: usize = 64;

fn reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
    };
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
}

pub fn checked_name(name: &str) -> Result<String> {
    let n = name.trim();
    let ok = !n.is_empty()
        && n.chars().count() <= NAME_MOST
        && !n.starts_with('.')
        && !n.ends_with('.')
        && !n.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        && !reserved(n);
    if ok {
        Ok(n.to_string())
    } else {
        Err(Error::usage(format!(
            "look name {name:?}: up to {NAME_MOST} characters, without / \\ : * ? \" < > |, \
             not starting or ending with a dot"
        )))
    }
}

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn parse(text: &str, kind: LookKind) -> std::result::Result<Styled, String> {
    let styled = match kind {
        LookKind::Subtitles => {
            Styled::Subtitles(toml::from_str(text).map_err(|e| e.message().to_string())?)
        }
        LookKind::Titles => {
            Styled::Titles(toml::from_str(text).map_err(|e| e.message().to_string())?)
        }
    };
    checked(&styled).map_err(|e| e.message)
}

pub fn read(dir: &Path, kind: LookKind) -> Listing {
    let mut listing = Listing::default();
    let Ok(entries) = std::fs::read_dir(dir.join(kind.folder())) else {
        return listing;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    for path in files {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let read = std::fs::read_to_string(&path).map_err(|e| e.to_string());
        let found = read.and_then(|text| parse(&text, kind)).and_then(|styled| {
            if built_in(kind).iter().any(|b| same(&b.name, &name)) {
                Err(format!("{name} is a built-in look's name"))
            } else {
                checked_name(&name).map(|_| styled).map_err(|e| e.message)
            }
        });
        match found {
            Ok(styled) => listing.looks.push(Look {
                name,
                description: "",
                built_in: false,
                styled,
            }),
            Err(why) => listing.refused.push(Refused { path, name, why }),
        }
    }
    listing
}

pub fn all(env: &dyn Env, kind: LookKind) -> Listing {
    let mine = env.looks(kind);
    Listing {
        looks: built_in(kind).into_iter().chain(mine.looks).collect(),
        refused: mine.refused,
    }
}

pub fn find(env: &dyn Env, kind: LookKind, name: &str) -> Result<Styled> {
    let listing = all(env, kind);
    if let Some(l) = listing.looks.iter().find(|l| same(&l.name, name.trim())) {
        return Ok(l.styled.clone());
    }
    if let Some(r) = listing.refused.iter().find(|r| same(&r.name, name.trim())) {
        return Err(Error::invalid(format!("{}: {}", r.path.display(), r.why)));
    }
    Err(Error::not_found(format!(
        "no {} look {name:?}; `vidcut looks list` lists them",
        kind.folder().trim_end_matches('s')
    )))
}

pub fn subtitle_look(env: &dyn Env, name: &str) -> Result<SubtitleLook> {
    match find(env, LookKind::Subtitles, name)? {
        Styled::Subtitles(l) => Ok(l),
        Styled::Titles(_) => Err(Error::internal("a title look among the subtitle looks")),
    }
}

pub fn title_look(env: &dyn Env, name: &str) -> Result<TitleLook> {
    match find(env, LookKind::Titles, name)? {
        Styled::Titles(l) => Ok(l),
        Styled::Subtitles(_) => Err(Error::internal("a subtitle look among the title looks")),
    }
}

fn path_of(dir: &Path, kind: LookKind, name: &str) -> PathBuf {
    dir.join(kind.folder()).join(format!("{name}.toml"))
}

fn mine(dir: &Path, kind: LookKind, name: &str) -> Option<PathBuf> {
    let listing = read(dir, kind);
    listing
        .looks
        .iter()
        .map(|l| (l.name.as_str(), path_of(dir, kind, &l.name)))
        .chain(
            listing
                .refused
                .iter()
                .map(|r| (r.name.as_str(), r.path.clone())),
        )
        .find(|(n, _)| same(n, name))
        .map(|(_, p)| p)
}

pub fn save(
    dir: &Path,
    name: &str,
    styled: &Styled,
    overwrite: bool,
    dry_run: bool,
) -> Result<(Look, PathBuf)> {
    let name = checked_name(name)?;
    let kind = styled.kind();
    if built_in(kind).iter().any(|b| same(&b.name, &name)) {
        return Err(Error::invalid(format!(
            "{name} is a built-in look and can't be changed; save it under another name"
        )));
    }
    let styled = checked(styled)?;
    let old = mine(dir, kind, &name);
    if old.is_some() && !overwrite {
        return Err(Error::invalid(format!(
            "you have a look called {name}; pass overwrite to replace it"
        )));
    }
    let path = path_of(dir, kind, &name);
    let look = Look {
        name,
        description: "",
        built_in: false,
        styled,
    };
    if dry_run {
        return Ok((look, path));
    }
    let text = match &look.styled {
        Styled::Subtitles(l) => toml::to_string(l),
        Styled::Titles(l) => toml::to_string(l),
    }
    .map_err(|e| Error::internal(e.to_string()))?;
    let folder = path.parent().expect("has a parent");
    let failed = |e: std::io::Error| Error::usage(format!("{}: {e}", path.display()));
    std::fs::create_dir_all(folder).map_err(failed)?;
    if let Some(old) = old.filter(|o| *o != path) {
        std::fs::rename(&old, &path).map_err(failed)?;
    }
    crate::temp::write(&path, text.as_bytes()).map_err(failed)?;
    Ok((look, path))
}

pub fn delete(dir: &Path, kind: LookKind, name: &str, dry_run: bool) -> Result<PathBuf> {
    if built_in(kind).iter().any(|b| same(&b.name, name.trim())) {
        return Err(Error::invalid(format!(
            "{} is a built-in look and can't be deleted",
            name.trim()
        )));
    }
    let Some(path) = mine(dir, kind, name.trim()) else {
        return Err(Error::not_found(format!(
            "you have no {} look {name:?}",
            kind.folder().trim_end_matches('s')
        )));
    };
    if !dry_run {
        std::fs::remove_file(&path)
            .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
    }
    Ok(path)
}
