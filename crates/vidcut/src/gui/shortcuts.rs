use eframe::egui::{self, Event, Key, Modifiers};
use vidcut_core::shortcuts::{Action, Bindings, Combo, Held, Panel, key_named};

fn held_of(m: Modifiers) -> Held {
    Held {
        ctrl: m.command,
        control: cfg!(target_os = "macos") && m.ctrl,
        alt: m.alt,
        shift: m.shift,
    }
}

fn sign(key: Key) -> bool {
    matches!(
        key,
        Key::Comma
            | Key::Period
            | Key::Equals
            | Key::Plus
            | Key::Minus
            | Key::Slash
            | Key::Backslash
            | Key::OpenBracket
            | Key::CloseBracket
            | Key::Semicolon
            | Key::Quote
            | Key::Backtick
            | Key::Colon
            | Key::Questionmark
            | Key::Exclamationmark
            | Key::Pipe
            | Key::OpenCurlyBracket
            | Key::CloseCurlyBracket
    )
}

fn by_place(key: Key) -> bool {
    matches!(
        key,
        Key::Comma | Key::Period | Key::Backslash | Key::OpenBracket | Key::CloseBracket
    )
}

fn letter_or_digit(key: Key) -> bool {
    key.name().len() == 1 && !sign(key)
}

fn combo(key: Key, held: Held) -> Option<Combo> {
    key_named(key.name()).map(|key| Combo { held, key })
}

fn key_owner(
    b: &Bindings,
    panel: Panel,
    (key, place): (Key, Option<Key>),
    held: Held,
) -> Option<Action> {
    let owner = |key: Key, held: Held| combo(key, held).and_then(|c| b.owner(panel, c));
    let shifted = held.shift && place != Some(key);
    let bare = !held.ctrl && !held.control && !held.alt;
    let typed = sign(key) && !by_place(key) && bare && (!held.shift || shifted);
    let exact = (!typed).then(|| owner(key, held)).flatten();
    let without_shift = (sign(key) && shifted && !typed)
        .then(|| {
            owner(
                key,
                Held {
                    shift: false,
                    ..held
                },
            )
        })
        .flatten();
    let at_place = place
        .filter(|p| by_place(*p) && !letter_or_digit(key))
        .and_then(|p| owner(p, held));
    exact.or(without_shift).or(at_place)
}

fn text_owner(b: &Bindings, panel: Panel, text: &str) -> Option<Action> {
    let key = Key::from_name(text).filter(|k| sign(*k) && !by_place(*k))?;
    b.owner(panel, combo(key, Held::default())?)
}

fn clipboard_combo(event: &Event) -> Option<Combo> {
    let name = match event {
        Event::Copy => "Ctrl+C",
        Event::Cut => "Ctrl+X",
        Event::Paste(_) => "Ctrl+V",
        _ => return None,
    };
    name.parse().ok()
}

pub(super) fn owner(b: &Bindings, panel: Panel, event: &Event) -> Option<(Action, bool)> {
    match event {
        Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat,
            modifiers,
        } => key_owner(b, panel, (*key, *physical_key), held_of(*modifiers)).map(|a| (a, *repeat)),
        Event::Text(text) => text_owner(b, panel, text).map(|a| (a, false)),
        _ => clipboard_combo(event)
            .and_then(|c| b.owner(panel, c))
            .map(|a| (a, false)),
    }
}

pub(crate) struct Fired(Vec<(Action, bool)>);

impl Fired {
    pub(crate) fn of(b: &Bindings, input: &egui::InputState) -> Self {
        Self(
            input
                .events
                .iter()
                .filter_map(|e| owner(b, Panel::App, e))
                .collect(),
        )
    }

    pub(crate) fn pressed(&self, action: Action) -> bool {
        self.0.iter().any(|(a, _)| *a == action)
    }

    pub(crate) fn struck(&self, action: Action) -> bool {
        self.0.iter().any(|(a, repeat)| *a == action && !repeat)
    }
}

pub(super) fn take(b: &Bindings, input: &mut egui::InputState, action: Action) -> bool {
    let found = input
        .events
        .iter()
        .position(|e| owner(b, action.panel(), e).is_some_and(|(a, _)| a == action));
    if let Some(at) = found {
        input.events.remove(at);
    }
    found.is_some()
}

pub(super) fn held(b: &Bindings, input: &egui::InputState, action: Action) -> bool {
    b.keys(action).iter().any(|c| {
        c.held == held_of(input.modifiers)
            && Key::from_name(c.key).is_some_and(|k| input.key_down(k))
    })
}

pub(super) fn caught(b: &Bindings, panel: Panel, event: &Event) -> Option<(Combo, Option<Action>)> {
    let found = match event {
        Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat: false,
            modifiers,
        } => {
            let mut held = held_of(*modifiers);
            let at_place = key_owner(b, panel, (*key, *physical_key), held);
            held.shift &= !(sign(*key) && *physical_key != Some(*key));
            let c = combo(*key, held)?;
            (c, at_place.or_else(|| b.owner(panel, c)))
        }
        _ => {
            let c = clipboard_combo(event)?;
            (c, b.owner(panel, c))
        }
    };
    Some(found)
}
