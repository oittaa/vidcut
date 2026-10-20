use std::path::{Path, PathBuf};

use eframe::egui::Color32;
use notify::Watcher;

use crate::themes::{Colors, Theme};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Palette {
    pub light: bool,
    pub colors: Colors,
}

pub(crate) fn apply(theme: &mut Theme) {
    let Some(dir) = theme_dir() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(dir.join("colors.toml")) else {
        return;
    };
    let Some(painted) = palette(&text, dir.join("light.mode").is_file()) else {
        return;
    };
    theme.light = painted.light;
    theme.colors = painted.colors;
    let Some(root) = dir.parent() else {
        return;
    };
    if let Some(name) = std::fs::read_to_string(root.join("theme.name"))
        .ok()
        .filter(|name| !name.trim().is_empty())
    {
        theme.title = title(name.trim());
    }
}

pub(crate) fn palette(text: &str, light_marker: bool) -> Option<Palette> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let background = color(&table, &["background", "bg"])?;
    let foreground = color(&table, &["foreground", "fg"])?;
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
    let light = match text_value(&table, &["mode"]) {
        Some("light") => true,
        Some("dark") => false,
        _ if light_marker => true,
        _ => luminance(background) >= 0.6,
    };
    let wash = |c| crate::look::mix(c, background, 0.30);
    let [r, g, b, _] = selection.to_srgba_unmultiplied();
    Some(Palette {
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

fn luminance(c: Color32) -> f32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

fn title(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        "Omarchy".into()
    } else {
        format!("Omarchy: {}", words.join(" "))
    }
}

fn theme_dir() -> Option<PathBuf> {
    roots()
        .into_iter()
        .map(|root| root.join("theme"))
        .find(|dir| dir.join("colors.toml").is_file())
}

fn roots() -> [PathBuf; 2] {
    [
        xdg("XDG_STATE_HOME", ".local/state").join("omarchy/current"),
        xdg("XDG_CONFIG_HOME", ".config").join("omarchy/current"),
    ]
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var).filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => home().join(fallback),
    }
}

fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn watch(changed: impl Fn() + Send + 'static) -> Option<notify::RecommendedWatcher> {
    let dirs: Vec<PathBuf> = roots().into_iter().filter(|dir| dir.is_dir()).collect();
    if dirs.is_empty() {
        return None;
    }
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            return;
        };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        if event.paths.iter().any(|path| relevant(path)) {
            changed();
        }
    })
    .ok()?;
    let mut watching = false;
    for dir in &dirs {
        watching |= watcher.watch(dir, notify::RecursiveMode::Recursive).is_ok();
    }
    watching.then_some(watcher)
}

fn relevant(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("colors.toml" | "theme.name" | "light.mode" | "theme")
    )
}

#[cfg(test)]
pub(crate) const TOKYO_NIGHT: &str = r##"
mode = "dark"

accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"

background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"

foreground = "#a9b1d6"
dark_foreground = "#565f89"
light_foreground = "#b4bee6"
bright_foreground = "#c0caf5"

red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"

bright_red = "#ff7a93"
bright_green = "#b9f27c"
"##;

#[cfg(test)]
pub(crate) const CATPPUCCIN_LATTE: &str = r##"
mode = "light"

accent = "#1e66f5"
selection = "#ccd0da"
muted = "#acb0be"

background = "#eff1f5"
dark_background = "#e3e4e8"
darker_background = "#d7d8dc"
lighter_background = "#dce0e8"

foreground = "#4c4f69"
dark_foreground = "#9ca0b0"
light_foreground = "#5c5f77"
bright_foreground = "#4c4f69"

red = "#d20f39"
yellow = "#df8e1d"
green = "#40a02b"
blue = "#1e66f5"
magenta = "#ea76cb"

bright_red = "#d20f39"
bright_green = "#40a02b"
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_palette_without_a_background_is_none() {
        assert!(palette("foreground = \"#AABBCC\"\n", false).is_none());
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
    fn a_theme_name_is_its_title() {
        assert_eq!(title("tokyo-night"), "Omarchy: Tokyo Night");
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
