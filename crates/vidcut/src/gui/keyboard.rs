use eframe::egui::{self, RichText};
use vidcut_core::shortcuts::{Bindings, Combo, Held, Panel, key_name, key_named};

use super::theme;

#[derive(Default)]
pub(super) struct KeyboardUi {
    held: Held,
    key: Option<&'static str>,
}

#[derive(Clone, Copy)]
enum Cap {
    Key(&'static str, f32),
    Ctrl(f32),
    Control(f32),
    Alt(f32),
    Shift(f32),
    Gap(f32),
}

use Cap::{Alt, Control, Ctrl, Gap, Key, Shift};

const MAIN: [&[Cap]; 6] = [
    &[
        Key("Escape", 1.0),
        Gap(1.0),
        Key("F1", 1.0),
        Key("F2", 1.0),
        Key("F3", 1.0),
        Key("F4", 1.0),
        Gap(0.5),
        Key("F5", 1.0),
        Key("F6", 1.0),
        Key("F7", 1.0),
        Key("F8", 1.0),
        Gap(0.5),
        Key("F9", 1.0),
        Key("F10", 1.0),
        Key("F11", 1.0),
        Key("F12", 1.0),
    ],
    &[
        Key("`", 1.0),
        Key("1", 1.0),
        Key("2", 1.0),
        Key("3", 1.0),
        Key("4", 1.0),
        Key("5", 1.0),
        Key("6", 1.0),
        Key("7", 1.0),
        Key("8", 1.0),
        Key("9", 1.0),
        Key("0", 1.0),
        Key("-", 1.0),
        Key("=", 1.0),
        Key("Backspace", 2.0),
    ],
    &[
        Key("Tab", 1.5),
        Key("Q", 1.0),
        Key("W", 1.0),
        Key("E", 1.0),
        Key("R", 1.0),
        Key("T", 1.0),
        Key("Y", 1.0),
        Key("U", 1.0),
        Key("I", 1.0),
        Key("O", 1.0),
        Key("P", 1.0),
        Key("[", 1.0),
        Key("]", 1.0),
        Key("\\", 1.5),
    ],
    &[
        Gap(1.75),
        Key("A", 1.0),
        Key("S", 1.0),
        Key("D", 1.0),
        Key("F", 1.0),
        Key("G", 1.0),
        Key("H", 1.0),
        Key("J", 1.0),
        Key("K", 1.0),
        Key("L", 1.0),
        Key(";", 1.0),
        Key("'", 1.0),
        Key("Enter", 2.25),
    ],
    &[
        Shift(2.25),
        Key("Z", 1.0),
        Key("X", 1.0),
        Key("C", 1.0),
        Key("V", 1.0),
        Key("B", 1.0),
        Key("N", 1.0),
        Key("M", 1.0),
        Key(",", 1.0),
        Key(".", 1.0),
        Key("/", 1.0),
        Shift(2.75),
    ],
    &[
        Ctrl(1.5),
        Control(1.25),
        Alt(1.25),
        Key("Space", 6.25),
        Alt(1.25),
        Control(1.25),
        Ctrl(1.5),
    ],
];

const NAV: [&[Cap]; 6] = [
    &[Gap(3.0)],
    &[Key("Insert", 1.0), Key("Home", 1.0), Key("PageUp", 1.0)],
    &[Key("Delete", 1.0), Key("End", 1.0), Key("PageDown", 1.0)],
    &[Gap(3.0)],
    &[Gap(1.0), Key("Up", 1.0)],
    &[Key("Left", 1.0), Key("Down", 1.0), Key("Right", 1.0)],
];

const WIDE: f32 = 15.0 + 0.5 + 3.0;

fn label(key: &str) -> &str {
    match key {
        "Escape" => "Esc",
        "Backspace" => "⌫",
        "Enter" => "Enter",
        "Insert" => "Ins",
        "Delete" => "Del",
        "PageUp" => "PgUp",
        "PageDown" => "PgDn",
        "Left" => "←",
        "Right" => "→",
        "Up" => "↑",
        "Down" => "↓",
        k => k,
    }
}

impl KeyboardUi {
    pub(super) fn combo(&self) -> Option<Combo> {
        self.key.map(|key| Combo {
            held: self.held,
            key,
        })
    }

    fn cap(&mut self, ui: &mut egui::Ui, b: &Bindings, cap: Cap, unit: f32) {
        let c = theme::colors();
        let (width, text, on, used) = match cap {
            Gap(w) => {
                ui.add_space(w * unit - ui.spacing().item_spacing.x);
                return;
            }
            Key(name, w) => {
                let Some(key) = key_named(name) else {
                    return;
                };
                let combo = Combo {
                    held: self.held,
                    key,
                };
                let used = Panel::ALL.iter().any(|&p| b.owner(p, combo).is_some());
                (w, label(key).to_string(), self.key == Some(key), used)
            }
            Ctrl(w) => (w, key_name("Ctrl").to_string(), self.held.ctrl, false),
            Control(w) if cfg!(target_os = "macos") => {
                (w, "Control".to_string(), self.held.control, false)
            }
            Control(w) => {
                ui.add_space(w * unit - ui.spacing().item_spacing.x);
                return;
            }
            Alt(w) => (w, key_name("Alt").to_string(), self.held.alt, false),
            Shift(w) => (w, "Shift".to_string(), self.held.shift, false),
        };
        let size = egui::vec2(width * unit - ui.spacing().item_spacing.x, unit * 0.9);
        let fill = match (on, used) {
            (true, _) => c.selection,
            (false, true) => c.line,
            (false, false) => c.field,
        };
        let color = match on {
            true => c.selected_text,
            false => c.text,
        };
        let button = egui::Button::new(RichText::new(text).size(11.0).color(color))
            .fill(fill)
            .min_size(size);
        if !ui.add_sized(size, button).clicked() {
            return;
        }
        match cap {
            Key(name, _) => {
                let key = key_named(name);
                self.key = if self.key == key { None } else { key };
            }
            Ctrl(_) => self.held.ctrl = !self.held.ctrl,
            Control(_) => self.held.control = !self.held.control,
            Alt(_) => self.held.alt = !self.held.alt,
            Shift(_) => self.held.shift = !self.held.shift,
            Gap(_) => {}
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, b: &Bindings) {
        let unit = (ui.available_width() / WIDE).clamp(18.0, 34.0);
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                for row in MAIN {
                    ui.horizontal(|ui| {
                        for &cap in row {
                            self.cap(ui, b, cap, unit);
                        }
                    });
                }
            });
            ui.add_space(unit * 0.5);
            ui.vertical(|ui| {
                for row in NAV {
                    ui.horizontal(|ui| {
                        for &cap in row {
                            self.cap(ui, b, cap, unit);
                        }
                    });
                }
            });
        });
        ui.spacing_mut().item_spacing = spacing;
        self.active(ui, b);
    }

    fn active(&self, ui: &mut egui::Ui, b: &Bindings) {
        let c = theme::colors();
        let Some(combo) = self.combo() else {
            return;
        };
        let used = Panel::ALL.iter().any(|&p| b.owner(p, combo).is_some());
        ui.horizontal(|ui| {
            ui.label(RichText::new(combo.shown()).strong());
            if !used {
                ui.label(RichText::new("No Action").color(c.text_dim));
            }
        });
    }
}
