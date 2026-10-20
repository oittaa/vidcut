use std::path::{Path, PathBuf};

use eframe::egui::Color32;
use vidcut_core::{Error, Result};

use crate::themes::{Colors, Theme};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Palette {
    pub light: bool,
    pub colors: Colors,
}

fn current() -> PathBuf {
    vidcut_core::dirs::home().join(".local/state/omarchy/current")
}

pub(crate) fn apply(theme: &mut Theme) -> Result<()> {
    let current = current();
    let dir = current.join("theme");
    if !dir.is_dir() {
        return Ok(());
    }
    let path = dir.join("colors.toml");
    let painted = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| palette(&text, dir.join("light.mode").is_file()))
        .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
    theme.light = painted.light;
    theme.colors = painted.colors;
    if let Ok(name) = std::fs::read_to_string(current.join("theme.name"))
        && !name.trim().is_empty()
    {
        theme.title = title(name.trim());
    }
    Ok(())
}

pub(crate) fn palette(text: &str, light_marker: bool) -> std::result::Result<Palette, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let needed = |keys: &[&str]| {
        color(&table, keys).ok_or_else(|| format!("{} is missing or isn't #RRGGBB", keys[0]))
    };
    let background = needed(&["background", "bg"])?;
    let foreground = needed(&["foreground", "fg"])?;
    let accent = color(&table, &["accent", "blue", "color4", "color12"]).unwrap_or(ACCENT);
    let selection = color(&table, &["selection", "selection_background"])
        .unwrap_or_else(|| crate::look::mix(background, accent, 0.22));
    let dark_foreground = color(&table, &["dark_foreground", "dark_fg"])
        .unwrap_or_else(|| crate::look::mix(foreground, background, 0.45));
    let bright_foreground =
        color(&table, &["bright_foreground", "bright_fg"]).unwrap_or(foreground);
    let muted = color(&table, &["muted", "color8"]).unwrap_or(dark_foreground);
    let dark_background = color(&table, &["dark_background", "dark_bg"])
        .unwrap_or_else(|| crate::look::mix(background, Color32::BLACK, 0.06));
    let darker_background = color(&table, &["darker_background", "darker_bg"])
        .unwrap_or_else(|| crate::look::mix(background, Color32::BLACK, 0.12));
    let lighter_background = color(&table, &["lighter_background", "lighter_bg"])
        .unwrap_or_else(|| crate::look::mix(background, foreground, 0.10));
    let red = color(&table, &["red", "color1"]).unwrap_or(RED);
    let green = color(&table, &["green", "color2"]).unwrap_or(GREEN);
    let yellow = color(&table, &["yellow", "color3", "orange"]).unwrap_or(YELLOW);
    let blue = color(&table, &["blue", "color4"]).unwrap_or(accent);
    let magenta = color(&table, &["magenta", "color5"]).unwrap_or(accent);
    let bright_red = color(&table, &["bright_red", "color9"]).unwrap_or(red);
    let mode = ["mode", "theme_type"]
        .iter()
        .find_map(|key| text_value(&table, &[key]).filter(|mode| !mode.is_empty()));
    let light = match mode {
        Some(mode) => mode == "light",
        None => light_marker || bright(background),
    };
    let wash = |c| crate::look::mix(c, background, 0.30);
    let [r, g, b, _] = selection.to_srgba_unmultiplied();
    Ok(Palette {
        light,
        colors: Colors {
            background: dark_background,
            panel: background,
            panel_dark: darker_background,
            track: dark_background,
            track_target: selection,
            panel_target: crate::look::mix(selection, accent, 0.45),
            text: foreground,
            text_dim: dark_foreground,
            accent,
            playhead: bright_red,
            clip: wash(blue),
            clip_disabled: muted,
            offline: red,
            audio: wash(green),
            generator: wash(magenta),
            compound: wash(yellow),
            marked: Color32::from_rgba_unmultiplied(r, g, b, 0x50),
            error: bright_red,
            ok: green,
            selection,
            selected_text: bright_foreground,
            field: darker_background,
            faint: lighter_background,
            line: muted,
            button: lighter_background,
            button_line: muted,
            button_hovered: crate::look::mix(lighter_background, foreground, 0.14),
            button_pressed: crate::look::mix(selection, accent, 0.28),
            shadow: if light {
                Color32::from_rgba_unmultiplied(0, 0, 0, 0x38)
            } else {
                Color32::from_rgba_unmultiplied(0, 0, 0, 0xA0)
            },
        },
    })
}

const ACCENT: Color32 = Color32::from_rgb(0x7A, 0xA2, 0xF7);
const RED: Color32 = Color32::from_rgb(0xF7, 0x76, 0x8E);
const GREEN: Color32 = Color32::from_rgb(0x9E, 0xCE, 0x6A);
const YELLOW: Color32 = Color32::from_rgb(0xE0, 0xAF, 0x68);

fn color(table: &toml::Table, keys: &[&str]) -> Option<Color32> {
    crate::themes::parse_color(text_value(table, keys)?)
}

fn text_value<'a>(table: &'a toml::Table, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| match table.get(*key) {
        Some(toml::Value::String(text)) => Some(text.as_str()),
        _ => None,
    })
}

fn bright(c: Color32) -> bool {
    u16::from(c.r()) + u16::from(c.g()) + u16::from(c.b()) > 382
}

fn title(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|word| {
            let mut chars = word.chars();
            let first = chars.next()?;
            Some(first.to_uppercase().chain(chars).collect())
        })
        .collect();
    if words.is_empty() {
        "Omarchy".into()
    } else {
        format!("Omarchy: {}", words.join(" "))
    }
}

pub(crate) fn watched() -> (PathBuf, notify::RecursiveMode) {
    let current = current();
    if current.is_dir() {
        return (current, notify::RecursiveMode::Recursive);
    }
    let dir = current
        .ancestors()
        .skip(1)
        .find(|dir| dir.is_dir())
        .map_or_else(|| current.clone(), Path::to_path_buf);
    (dir, notify::RecursiveMode::NonRecursive)
}

pub(crate) fn relevant(path: &Path) -> bool {
    let current = current();
    let theme = current.join("theme");
    current.starts_with(path)
        || path == theme
        || path == current.join("theme.name")
        || (path.parent() == Some(theme.as_path())
            && matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("colors.toml" | "light.mode")
            ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_palette_without_a_background_says_so() {
        assert_eq!(
            palette("foreground = \"#AABBCC\"\n", false),
            Err("background is missing or isn't #RRGGBB".to_string())
        );
    }

    #[test]
    fn canonical_names_win_over_the_legacy_ones() {
        let painted = palette(
            "background = \"#112233\"\nbg = \"#445566\"\nforeground = \"#AABBCC\"\n",
            false,
        )
        .unwrap();
        assert_eq!(painted.colors.panel, Color32::from_rgb(0x11, 0x22, 0x33));
    }

    #[test]
    fn legacy_names_still_paint() {
        let painted = palette(
            "bg = \"#112233\"\nfg = \"#AABBCC\"\nmode = \"dark\"\n",
            false,
        )
        .unwrap();
        assert_eq!(
            (painted.light, painted.colors.panel, painted.colors.text),
            (
                false,
                Color32::from_rgb(0x11, 0x22, 0x33),
                Color32::from_rgb(0xAA, 0xBB, 0xCC)
            )
        );
    }

    #[test]
    fn blue_is_the_accent_when_the_palette_names_none() {
        let painted = palette(
            "background = \"#101010\"\nforeground = \"#EEEEEE\"\ncolor4 = \"#123456\"\n",
            false,
        )
        .unwrap();
        assert_eq!(painted.colors.accent, Color32::from_rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn mode_says_whether_the_palette_is_light() {
        let painted = palette(
            "mode = \"light\"\nbackground = \"#101010\"\nforeground = \"#EEEEEE\"\n",
            false,
        )
        .unwrap();
        assert!(painted.light);
    }

    #[test]
    fn a_light_marker_applies_when_mode_is_absent() {
        let painted =
            palette("background = \"#101010\"\nforeground = \"#EEEEEE\"\n", true).unwrap();
        assert!(painted.light);
    }

    #[test]
    fn mode_wins_over_a_light_marker() {
        let painted = palette(
            "mode = \"dark\"\nbackground = \"#EEEEEE\"\nforeground = \"#101010\"\n",
            true,
        )
        .unwrap();
        assert!(!painted.light);
    }

    #[test]
    fn theme_type_says_whether_the_palette_is_light_when_mode_is_absent() {
        let painted = palette(
            "theme_type = \"light\"\nbackground = \"#101010\"\nforeground = \"#EEEEEE\"\n",
            false,
        )
        .unwrap();
        assert!(painted.light);
    }

    #[test]
    fn a_bright_background_is_light_when_nothing_says() {
        let painted = palette(
            "background = \"#808081\"\nforeground = \"#101010\"\n",
            false,
        )
        .unwrap();
        assert!(painted.light);
    }

    #[test]
    fn a_theme_name_is_its_title() {
        assert_eq!(title("tokyo-night"), "Omarchy: Tokyo Night");
    }

    #[test]
    fn a_theme_name_keeps_its_accents() {
        assert_eq!(title("rosé-pine"), "Omarchy: Rosé Pine");
    }

    #[test]
    fn a_short_palette_derives_its_ramp_from_the_background() {
        let painted = palette(
            "background = \"#101010\"\nforeground = \"#EEEEEE\"\nmode = \"dark\"\n",
            false,
        )
        .unwrap();
        assert_eq!(
            (
                painted.colors.background,
                painted.colors.panel_dark,
                painted.colors.faint,
                painted.colors.text_dim
            ),
            (
                Color32::from_rgb(0x0F, 0x0F, 0x0F),
                Color32::from_rgb(0x0E, 0x0E, 0x0E),
                Color32::from_rgb(0x26, 0x26, 0x26),
                Color32::from_rgb(0x8A, 0x8A, 0x8A)
            )
        );
    }
}
