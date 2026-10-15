//! Themes: how the GUI looks (deviation X22). The built-in ones are the
//! TOML files in `crates/vidcut/themes`. A user's go in the themes folder
//! (`~/.config/vidcut/themes`) as `NAME.toml`, and one there replaces a
//! built-in of the same name. A theme file lists what it changes. The rest
//! comes from the theme it `extends`, Starship unless it names another.
//! A color written `system` is the computer's accent color (`look.rs`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};
use vidcut_core::{Error, Result};

use crate::look::Look;
use crate::paths::config_dir;

/// The theme a new install starts with, and the base of the others.
pub const DEFAULT: &str = "starship";

/// The built-in themes by name, in the order they are listed.
const BUILT_IN: &[(&str, &str)] = &[
    ("starship", include_str!("../themes/starship.toml")),
    ("neon", include_str!("../themes/neon.toml")),
    ("cybertruck", include_str!("../themes/cybertruck.toml")),
    ("midnight", include_str!("../themes/midnight.toml")),
    ("studio", include_str!("../themes/studio.toml")),
    ("native", include_str!("../themes/native.toml")),
    ("native-dark", include_str!("../themes/native-dark.toml")),
    ("native-light", include_str!("../themes/native-light.toml")),
    ("classic", include_str!("../themes/classic.toml")),
];

/// A look: colors, shapes, fonts and what is painted behind the panels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub title: String,
    /// Starts from light widgets instead of dark ones.
    pub light: bool,
    pub colors: Colors,
    pub shape: Shape,
    pub fonts: Fonts,
    pub decoration: Decoration,
    /// The theme shown instead while the computer is in light mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_mode: Option<String>,
}

/// A theme's colors, written `#RRGGBB` or `#RRGGBBAA`, or `system`,
/// `system-light` and `system-dark` for the computer's accent color and its
/// lighter and darker shades.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    /// Behind the panels, the top bar and the status bar.
    #[serde(with = "hex")]
    pub background: Color32,
    #[serde(with = "hex")]
    pub panel: Color32,
    /// The viewer and the timeline's well.
    #[serde(with = "hex")]
    pub panel_dark: Color32,
    #[serde(with = "hex")]
    pub track: Color32,
    /// The target track's row.
    #[serde(with = "hex")]
    pub track_target: Color32,
    /// The target track's header.
    #[serde(with = "hex")]
    pub panel_target: Color32,
    #[serde(with = "hex")]
    pub text: Color32,
    #[serde(with = "hex")]
    pub text_dim: Color32,
    #[serde(with = "hex")]
    pub accent: Color32,
    #[serde(with = "hex")]
    pub playhead: Color32,
    #[serde(with = "hex")]
    pub clip: Color32,
    #[serde(with = "hex")]
    pub clip_disabled: Color32,
    /// A clip whose file is missing, and the viewer's Media Offline frame.
    #[serde(with = "hex")]
    pub offline: Color32,
    #[serde(with = "hex")]
    pub audio: Color32,
    /// A generator or title carried from another editor (deviation X9).
    #[serde(with = "hex")]
    pub generator: Color32,
    /// A compound clip or nested timeline carried from another editor
    /// (deviation X9).
    #[serde(with = "hex")]
    pub compound: Color32,
    /// Over the marked range.
    #[serde(with = "hex")]
    pub marked: Color32,
    #[serde(with = "hex")]
    pub error: Color32,
    #[serde(with = "hex")]
    pub ok: Color32,
    /// Selected buttons and text.
    #[serde(with = "hex")]
    pub selection: Color32,
    #[serde(with = "hex")]
    pub selected_text: Color32,
    /// Text fields.
    #[serde(with = "hex")]
    pub field: Color32,
    /// Every other row of a list.
    #[serde(with = "hex")]
    pub faint: Color32,
    /// Separators and window edges.
    #[serde(with = "hex")]
    pub line: Color32,
    /// Buttons and check boxes.
    #[serde(with = "hex")]
    pub button: Color32,
    /// The edge of outlined buttons.
    #[serde(with = "hex")]
    pub button_line: Color32,
    #[serde(with = "hex")]
    pub button_hovered: Color32,
    #[serde(with = "hex")]
    pub button_pressed: Color32,
    #[serde(with = "hex")]
    pub shadow: Color32,
}

/// Corners, room and controls, in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    /// The corner radius of buttons and fields.
    pub corners: u8,
    pub windows: u8,
    pub menus: u8,
    pub panels: u8,
    /// The timeline's clips.
    pub clips: u8,
    /// Between panels.
    pub gap: i8,
    /// Inside panels.
    pub padding: i8,
    /// Inside the top bar and the status bar: across, down.
    pub bar_padding: [i8; 2],
    /// Between controls: across, down.
    pub spacing: [f32; 2],
    /// Inside buttons: across, down.
    pub button_padding: [f32; 2],
    /// The height of buttons and fields.
    pub button_height: f32,
    pub buttons: Buttons,
    /// A line around each panel.
    pub panel_border: bool,
    /// Window shadows: across, down, blur.
    pub shadow: [u8; 3],
}

/// How buttons look when the pointer isn't on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Buttons {
    Filled,
    Outlined,
    /// No box until the pointer is on one.
    Flat,
}

/// Fonts by family name, the first one installed wins. `system` is the
/// computer's own UI or monospace font. None installed: the built-in font.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fonts {
    pub ui: Vec<String>,
    pub mono: Vec<String>,
    pub size: f32,
    pub small: f32,
    pub heading: f32,
    pub mono_size: f32,
}

/// What is painted behind the panels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decoration {
    pub style: Style,
    /// How deep the cut corners are.
    pub cut: f32,
    /// `hud`: the edges and brackets. `glow`: the panels' glow. `steel`:
    /// the light bar.
    #[serde(with = "hex")]
    pub color: Color32,
    /// `hud`: the viewer's cut corners. `glow`: the viewer's glow.
    #[serde(with = "hex")]
    pub color2: Color32,
    /// `steel`: the slabs' shading from top to bottom, and their edge.
    #[serde(with = "hex")]
    pub top: Color32,
    #[serde(with = "hex")]
    pub bottom: Color32,
    #[serde(with = "hex")]
    pub edge: Color32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    None,
    /// A glow around each panel and a grid in the viewer.
    Glow,
    /// Cut corners, brackets and marks in the viewer.
    Hud,
    /// Brushed steel slabs with cut corners, and a light bar.
    Steel,
}

/// `#RRGGBB` or `#RRGGBBAA`, the opacity not multiplied in.
mod hex {
    use eframe::egui::Color32;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &Color32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(*c))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color32, D::Error> {
        let text = String::deserialize(d)?;
        super::parse_color(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{text:?} is no color: write it as #RRGGBB or #RRGGBBAA"
            ))
        })
    }
}

fn to_hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

/// `#RRGGBB` or `#RRGGBBAA` as a color.
fn parse_color(text: &str) -> Option<Color32> {
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    let a = if digits.len() == 8 { byte(6)? } else { 255 };
    Some(Color32::from_rgba_unmultiplied(
        byte(0)?,
        byte(2)?,
        byte(4)?,
        a,
    ))
}

/// The themes folder.
pub fn dir() -> PathBuf {
    config_dir().join("themes")
}

fn user_path(name: &str) -> PathBuf {
    dir().join(format!("{name}.toml"))
}

fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::usage(format!(
            "theme name {name:?}: use letters, digits, '-', '_' and '.'"
        )))
    }
}

/// The default theme, for whatever can't load the one chosen.
pub fn default_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| load(DEFAULT).expect("the default theme loads"))
}

/// The theme called `name`: the user's file of that name, else the
/// built-in one.
pub fn load(name: &str) -> Result<Theme> {
    load_from(name, 0, &crate::look::current())
}

/// The theme `name` as it shows now: its `light_mode` theme while the
/// computer is in light mode.
pub fn shown(name: &str) -> Result<Theme> {
    shown_with(name, &crate::look::current())
}

fn shown_with(name: &str, look: &Look) -> Result<Theme> {
    let theme = load_from(name, 0, look)?;
    match &theme.light_mode {
        Some(light) if look.light => load_from(light, 1, look),
        _ => Ok(theme),
    }
}

fn load_from(name: &str, depth: usize, look: &Look) -> Result<Theme> {
    if depth > 8 {
        return Err(Error::usage(format!(
            "theme {name:?}: its themes extend each other in a circle"
        )));
    }
    check_name(name)?;
    let path = user_path(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text, name, depth, look)
            .map_err(|e| Error::usage(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => builtin(name, depth, look),
        Err(e) => Err(Error::usage(format!("{}: {e}", path.display()))),
    }
}

/// The built-in theme called `name`. The default one has every key and
/// extends none.
fn builtin(name: &str, depth: usize, look: &Look) -> Result<Theme> {
    let Some((_, text)) = BUILT_IN.iter().find(|(n, _)| *n == name) else {
        return Err(Error::usage(format!(
            "there is no theme called {name:?}. `vidcut preferences get` lists them"
        )));
    };
    let theme = if name == DEFAULT {
        toml::from_str(text).map_err(|e| e.to_string())
    } else {
        parse(text, name, depth, look)
    };
    theme.map_err(|e| Error::internal(format!("theme {name}: {e}")))
}

/// A theme file's text, filled in from the theme it extends. A file
/// extending its own name extends the built-in theme of that name.
fn parse(text: &str, name: &str, depth: usize, look: &Look) -> std::result::Result<Theme, String> {
    let mut table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let base = match table.remove("extends") {
        Some(toml::Value::String(base)) => base,
        Some(_) => return Err("`extends` names a theme".into()),
        None => DEFAULT.to_string(),
    };
    let base = if base == name {
        builtin(&base, depth + 1, look)
    } else {
        load_from(&base, depth + 1, look)
    }
    .map_err(|e| e.message)?;
    table
        .entry("title")
        .or_insert_with(|| toml::Value::String(name.into()));
    let toml::Value::Table(mut full) = toml::Value::try_from(&base).map_err(|e| e.to_string())?
    else {
        unreachable!("a theme is a table");
    };
    merge(&mut full, table);
    system_colors(&mut full, look);
    toml::Value::Table(full)
        .try_into()
        .map_err(|e: toml::de::Error| e.to_string())
}

/// The colors written `system`, `system-light` and `system-dark` in
/// `look`'s accent color and its shades.
fn system_colors(theme: &mut toml::Table, look: &Look) {
    for group in ["colors", "decoration"] {
        let Some(toml::Value::Table(colors)) = theme.get_mut(group) else {
            continue;
        };
        for (_, value) in colors.iter_mut() {
            let color = match value.as_str() {
                Some("system") => look.accent.base,
                Some("system-light") => look.accent.light,
                Some("system-dark") => look.accent.dark,
                _ => continue,
            };
            *value = toml::Value::String(to_hex(color));
        }
    }
}

/// `over`'s keys replace `base`'s, group by group.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// A theme as `preferences.get` lists it.
#[derive(Debug, Clone, Serialize)]
pub struct Listed {
    pub name: String,
    pub title: String,
    /// The user's file, `None` for a built-in theme.
    pub path: Option<PathBuf>,
    /// Why it doesn't load, if it doesn't.
    pub error: Option<String>,
}

/// The built-in themes, then the user's, each name once.
pub fn list() -> Vec<Listed> {
    let look = crate::look::current();
    let mut names: Vec<String> = BUILT_IN.iter().map(|(n, _)| n.to_string()).collect();
    let mut user: Vec<String> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|n| check_name(n).is_ok() && !names.contains(n))
        .collect();
    user.sort();
    names.extend(user);
    names
        .into_iter()
        .map(|name| {
            let path = user_path(&name);
            let path = path.exists().then_some(path);
            match load_from(&name, 0, &look) {
                Ok(t) => Listed {
                    name,
                    title: t.title,
                    path,
                    error: None,
                },
                Err(e) => Listed {
                    title: name.clone(),
                    name,
                    path,
                    error: Some(e.message),
                },
            }
        })
        .collect()
}

/// Whether a value of `appearance.theme` is a theme file to import rather
/// than a name.
pub fn is_file(value: &str) -> bool {
    value.ends_with(".toml") || value.contains(['/', '\\'])
}

/// Copies the theme file at `path` into the themes folder, after checking
/// that it loads. Its name is the file's, and it replaces a theme of the
/// user's by that name. Returns the name. A dry run only checks it.
pub fn import(path: &Path, dry_run: bool) -> Result<String> {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    check_name(&name)?;
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
    parse(&text, &name, 0, &crate::look::current())
        .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
    let to = user_path(&name);
    if !dry_run && std::fs::canonicalize(path).ok() != std::fs::canonicalize(&to).ok() {
        std::fs::create_dir_all(dir())
            .and_then(|()| std::fs::write(&to, text))
            .map_err(|e| Error::internal(format!("{}: {e}", to.display())))?;
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: Look = crate::look::DEFAULT;

    #[test]
    fn colors_are_written_in_hex() {
        assert_eq!(
            parse_color("#FF2EC4"),
            Some(Color32::from_rgb(255, 46, 196))
        );
    }

    #[test]
    fn hex_colors_take_an_opacity() {
        assert_eq!(
            parse_color("#00789640"),
            Some(Color32::from_rgba_unmultiplied(0, 120, 150, 64))
        );
    }

    #[test]
    fn a_color_without_its_hash_is_none() {
        assert_eq!(parse_color("FF2EC4"), None);
    }

    #[test]
    fn every_built_in_theme_loads() {
        let failed: Vec<String> = BUILT_IN
            .iter()
            .filter_map(|(name, _)| builtin(name, 0, &DARK).err().map(|e| e.message))
            .collect();
        assert_eq!(failed, Vec::<String>::new());
    }

    #[test]
    fn a_theme_takes_what_it_leaves_out_from_the_one_it_extends() {
        let t = parse(
            "extends = \"neon\"\n[colors]\naccent = \"#123456\"\n",
            "mine",
            0,
            &DARK,
        )
        .unwrap();
        assert_eq!(
            (t.title.as_str(), t.colors.accent, t.colors.panel),
            (
                "mine",
                Color32::from_rgb(0x12, 0x34, 0x56),
                Color32::from_rgb(0x13, 0x0C, 0x22)
            )
        );
    }

    #[test]
    fn a_theme_extends_starship_unless_it_says_otherwise() {
        let t = parse("title = \"Mine\"\n", "mine", 0, &DARK).unwrap();
        assert_eq!(t.colors, default_theme().colors);
    }

    #[test]
    fn a_misspelled_key_fails() {
        assert!(parse("[colors]\naccnet = \"#123456\"\n", "mine", 0, &DARK).is_err());
    }

    #[test]
    fn a_system_color_is_the_computers_accent() {
        let look = Look {
            accent: crate::look::Accent::from_base(Color32::from_rgb(200, 30, 90)),
            ..DARK
        };
        let t = parse("[colors]\naccent = \"system\"\n", "mine", 0, &look).unwrap();
        assert_eq!(t.colors.accent, Color32::from_rgb(200, 30, 90));
    }

    #[test]
    fn native_shows_its_light_theme_in_light_mode() {
        let light = Look {
            light: true,
            ..DARK
        };
        assert_eq!(shown_with("native", &light).unwrap().title, "Native Light");
    }

    #[test]
    fn native_shows_its_dark_theme_in_dark_mode() {
        assert_eq!(shown_with("native", &DARK).unwrap().title, "Native");
    }
}
