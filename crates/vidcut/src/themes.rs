use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};
use vidcut_core::{Error, Result};

use crate::look::Look;
use crate::paths::config_dir;

pub const DEFAULT: &str = "native";

const BUILT_IN: &[(&str, &str)] = &[
    ("starship", include_str!("../themes/starship.toml")),
    ("neon", include_str!("../themes/neon.toml")),
    ("cybertruck", include_str!("../themes/cybertruck.toml")),
    ("midnight", include_str!("../themes/midnight.toml")),
    ("studio", include_str!("../themes/studio.toml")),
    ("native", include_str!("../themes/native.toml")),
    ("omarchy", include_str!("../themes/omarchy.toml")),
    ("native-dark", include_str!("../themes/native-dark.toml")),
    ("native-light", include_str!("../themes/native-light.toml")),
    ("classic", include_str!("../themes/classic.toml")),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub title: String,
    pub light: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors_from: Option<ColorsFrom>,
    pub colors: Colors,
    pub shape: Shape,
    pub fonts: Fonts,
    pub decoration: Decoration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorsFrom {
    Omarchy,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    #[serde(with = "hex")]
    pub background: Color32,
    #[serde(with = "hex")]
    pub panel: Color32,
    #[serde(with = "hex")]
    pub panel_dark: Color32,
    #[serde(with = "hex")]
    pub track: Color32,
    #[serde(with = "hex")]
    pub track_target: Color32,
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
    #[serde(with = "hex")]
    pub offline: Color32,
    #[serde(with = "hex")]
    pub audio: Color32,
    #[serde(with = "hex")]
    pub generator: Color32,
    #[serde(with = "hex")]
    pub compound: Color32,
    #[serde(with = "hex")]
    pub marked: Color32,
    #[serde(with = "hex")]
    pub error: Color32,
    #[serde(with = "hex")]
    pub ok: Color32,
    #[serde(with = "hex")]
    pub selection: Color32,
    #[serde(with = "hex")]
    pub selected_text: Color32,
    #[serde(with = "hex")]
    pub field: Color32,
    #[serde(with = "hex")]
    pub faint: Color32,
    #[serde(with = "hex")]
    pub line: Color32,
    #[serde(with = "hex")]
    pub button: Color32,
    #[serde(with = "hex")]
    pub button_line: Color32,
    #[serde(with = "hex")]
    pub button_hovered: Color32,
    #[serde(with = "hex")]
    pub button_pressed: Color32,
    #[serde(with = "hex")]
    pub shadow: Color32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    pub corners: u8,
    pub windows: u8,
    pub menus: u8,
    pub panels: u8,
    pub clips: u8,
    pub gap: i8,
    pub padding: i8,
    pub bar_padding: [i8; 2],
    pub spacing: [f32; 2],
    pub button_padding: [f32; 2],
    pub button_height: f32,
    pub buttons: Buttons,
    pub panel_border: bool,
    pub shadow: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Buttons {
    Filled,
    Outlined,
    Flat,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decoration {
    pub style: Style,
    #[serde(default)]
    pub cut: f32,
    #[serde(with = "hex", default)]
    pub color: Color32,
    #[serde(with = "hex", default)]
    pub color2: Color32,
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
    Glow,
    Hud,
    Steel,
}

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

pub(crate) fn parse_color(text: &str) -> Option<Color32> {
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

pub fn default_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| load(DEFAULT).expect("the default theme loads"))
}

pub fn load(name: &str) -> Result<Theme> {
    load_with(name, &crate::look::current())
}

pub fn colors_from(name: &str) -> Option<ColorsFrom> {
    written(name, &crate::look::current()).ok()?.colors_from
}

fn load_with(name: &str, look: &Look) -> Result<Theme> {
    let mut theme = written(name, look)?;
    if theme.colors_from == Some(ColorsFrom::Omarchy) {
        crate::omarchy::apply(&mut theme)?;
    }
    Ok(theme)
}

fn written(name: &str, look: &Look) -> Result<Theme> {
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

#[derive(Debug, Clone, Serialize)]
pub struct Listed {
    pub name: String,
    pub title: String,
    pub path: Option<PathBuf>,
    pub error: Option<String>,
}

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

pub fn is_file(value: &str) -> bool {
    value.ends_with(".toml") || value.contains(['/', '\\'])
}

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
    fn the_omarchy_theme_takes_its_colors_from_omarchy() {
        assert_eq!(
            builtin("omarchy", &DARK).unwrap().colors_from,
            Some(ColorsFrom::Omarchy)
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
