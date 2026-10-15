//! Themes: how the GUI looks (deviation X22). The built-in ones are the
//! TOML files in `crates/vidcut/themes`. A user's go in the themes folder
//! (`~/.config/vidcut/themes`) as `NAME.toml`, and one there replaces a
//! built-in of the same name. A theme file has every key and needs no other
//! file. Its `[light_mode]` table holds keys that replace the others while
//! the computer is in light mode, and a color written `system` is the
//! computer's accent color (`look.rs`). Native is the default.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};
use vidcut_core::{Error, Result};

use crate::look::Look;
use crate::paths::config_dir;

/// The theme a new install starts with, and the one shown when the chosen
/// one doesn't load.
pub const DEFAULT: &str = "native";

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

/// What is painted behind the panels. A style leaves out the keys it
/// doesn't use.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decoration {
    pub style: Style,
    /// How deep the cut corners are.
    #[serde(default)]
    pub cut: f32,
    /// `hud`: the edges and brackets. `glow`: the panels' glow. `steel`:
    /// the light bar.
    #[serde(with = "hex", default)]
    pub color: Color32,
    /// `hud`: the viewer's cut corners. `glow`: the viewer's glow.
    #[serde(with = "hex", default)]
    pub color2: Color32,
    /// `steel`: the slabs' shading from top to bottom, and their edge.
    #[serde(with = "hex", default)]
    pub top: Color32,
    #[serde(with = "hex", default)]
    pub bottom: Color32,
    #[serde(with = "hex", default)]
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

/// The default theme as it showed first, for what paints before the
/// session's theme comes.
pub fn default_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| load(DEFAULT).expect("the default theme loads"))
}

/// The theme called `name` as it shows now: the user's file of that name,
/// else the built-in one.
pub fn load(name: &str) -> Result<Theme> {
    load_with(name, &crate::look::current())
}

fn load_with(name: &str, look: &Look) -> Result<Theme> {
    check_name(name)?;
    let path = user_path(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            parse(&text, name, look).map_err(|e| Error::usage(format!("{}: {e}", path.display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => builtin(name, look),
        Err(e) => Err(Error::usage(format!("{}: {e}", path.display()))),
    }
}

fn builtin(name: &str, look: &Look) -> Result<Theme> {
    let Some((_, text)) = BUILT_IN.iter().find(|(n, _)| *n == name) else {
        return Err(Error::usage(format!(
            "there is no theme called {name:?}. `vidcut preferences get` lists them"
        )));
    };
    parse(text, name, look).map_err(|e| Error::internal(format!("theme {name}: {e}")))
}

/// A theme file's text as it shows in `look`: in light mode with its
/// `[light_mode]` keys, its `system` colors in the computer's accent.
fn parse(text: &str, name: &str, look: &Look) -> std::result::Result<Theme, String> {
    let mut table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    match table.remove("light_mode") {
        Some(toml::Value::Table(light)) if look.light => merge(&mut table, light),
        Some(toml::Value::Table(_)) | None => {}
        Some(_) => return Err("`light_mode` is a table of keys".into()),
    }
    table
        .entry("title")
        .or_insert_with(|| toml::Value::String(name.into()));
    system_colors(&mut table, look);
    toml::Value::Table(table)
        .try_into()
        .map_err(|e: toml::de::Error| {
            format!("{e}\nA theme file needs every key: start from a copy of a built-in one.")
        })
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

/// `over`'s keys replace `base`'s, table by table.
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
            match load_with(&name, &look) {
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
    parse(&text, &name, &crate::look::current())
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
    const LIGHT: Look = Look {
        light: true,
        ..crate::look::DEFAULT
    };

    fn text(name: &str) -> &'static str {
        BUILT_IN.iter().find(|(n, _)| *n == name).unwrap().1
    }

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
    fn every_built_in_theme_loads_in_dark_and_light_mode() {
        let failed: Vec<String> = BUILT_IN
            .iter()
            .flat_map(|(name, _)| [builtin(name, &DARK), builtin(name, &LIGHT)])
            .filter_map(|t| t.err().map(|e| e.message))
            .collect();
        assert_eq!(failed, Vec::<String>::new());
    }

    #[test]
    fn a_theme_file_without_every_key_fails() {
        assert!(parse("title = \"Mine\"\n", "mine", &DARK).is_err());
    }

    #[test]
    fn a_misspelled_key_fails() {
        let misspelled = text("neon").replace("accent =", "accnet =");
        assert!(parse(&misspelled, "mine", &DARK).is_err());
    }

    #[test]
    fn a_system_color_is_the_computers_accent() {
        let look = Look {
            accent: crate::look::Accent::from_base(Color32::from_rgb(200, 30, 90)),
            ..DARK
        };
        let t = parse(text("native"), "native", &look).unwrap();
        assert_eq!(t.colors.accent, look.accent.light);
    }

    #[test]
    fn native_takes_its_light_mode_keys_in_light_mode() {
        let t = parse(text("native"), "native", &LIGHT).unwrap();
        assert_eq!(
            (t.light, t.colors.panel),
            (true, Color32::from_rgb(0xFA, 0xFA, 0xFA))
        );
    }

    #[test]
    fn native_leaves_its_light_mode_keys_in_dark_mode() {
        let t = parse(text("native"), "native", &DARK).unwrap();
        assert_eq!(
            (t.light, t.colors.panel),
            (false, Color32::from_rgb(0x28, 0x28, 0x28))
        );
    }
}
