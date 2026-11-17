use std::sync::{Arc, Mutex, MutexGuard};

use eframe::{EframeWinitApplication, UserEvent, egui};
use vidcut_core::request::Page;
use vidcut_core::shortcuts::{Action, Combo, Held, Panel, key_named, on_keypad};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::WindowId;

use super::shortcuts::Pads;
use super::workspace::Workspace;

#[derive(Default)]
struct Shared {
    ctx: Option<egui::Context>,
    watch: Watch,
    pads: Pads,
}

#[derive(Default)]
pub(super) struct Watch {
    root: Option<WindowId>,
    free: bool,
    capture: bool,
    taken: Vec<Combo>,
}

#[derive(Clone, Default)]
pub(super) struct Keypad(Arc<Mutex<Shared>>);

impl Keypad {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn start(&self, ctx: egui::Context) {
        self.lock().ctx = Some(ctx);
    }

    pub(super) fn take(&self) -> Pads {
        let mut s = self.lock();
        Pads {
            struck: std::mem::take(&mut s.pads.struck),
            down: s.pads.down.clone(),
        }
    }

    pub(super) fn watch(&self, watch: Watch) {
        self.lock().watch = watch;
    }

    fn takes(&self, window: WindowId, key: &KeyEvent, synthetic: bool, held: Held) -> bool {
        let PhysicalKey::Code(code) = key.physical_key else {
            return false;
        };
        let Some(name) = pad_name(code) else {
            return false;
        };
        let mut s = self.lock();
        let down = s.pads.down.iter().position(|k| *k == name);
        let combo = Combo { held, key: name };
        let take = match (key.state, key.repeat) {
            (ElementState::Released, _) => {
                if let Some(at) = down {
                    s.pads.down.remove(at);
                }
                return down.is_some();
            }
            (ElementState::Pressed, true) => down.is_some(),
            (ElementState::Pressed, false) => {
                let w = &s.watch;
                !synthetic
                    && (w.capture || (w.root == Some(window) && w.free && w.taken.contains(&combo)))
            }
        };
        if take {
            if down.is_none() {
                s.pads.down.push(name);
            }
            s.pads.struck.push((combo, key.repeat));
            if let Some(ctx) = &s.ctx {
                ctx.request_repaint();
            }
        }
        take
    }
}

fn pad_name(code: KeyCode) -> Option<&'static str> {
    let name = match code {
        KeyCode::Numpad0 => "Num0",
        KeyCode::Numpad1 => "Num1",
        KeyCode::Numpad2 => "Num2",
        KeyCode::Numpad3 => "Num3",
        KeyCode::Numpad4 => "Num4",
        KeyCode::Numpad5 => "Num5",
        KeyCode::Numpad6 => "Num6",
        KeyCode::Numpad7 => "Num7",
        KeyCode::Numpad8 => "Num8",
        KeyCode::Numpad9 => "Num9",
        KeyCode::NumpadAdd => "Num+",
        KeyCode::NumpadSubtract => "Num-",
        KeyCode::NumpadMultiply => "Num*",
        KeyCode::NumpadDivide => "Num/",
        KeyCode::NumpadDecimal => "Num.",
        KeyCode::NumpadEnter => "NumEnter",
        _ => return None,
    };
    key_named(name)
}

fn held_of(m: ModifiersState) -> Held {
    let mac = cfg!(target_os = "macos");
    Held {
        ctrl: if mac { m.super_key() } else { m.control_key() },
        control: mac && m.control_key(),
        alt: m.alt_key(),
        shift: m.shift_key(),
    }
}

struct Errors(Mutex<Option<String>>);

static ERRORS: Errors = Errors(Mutex::new(None));

impl Errors {
    fn take(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl log::Log for Errors {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() == log::Level::Error && metadata.target().starts_with("eframe")
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let text = record.args().to_string();
        let mut kept = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if kept.is_none() {
            *kept = Some(text);
        }
    }

    fn flush(&self) {}
}

pub(super) fn keep_errors() -> Result<(), String> {
    log::set_logger(&ERRORS).map_err(|e| e.to_string())?;
    log::set_max_level(log::LevelFilter::Error);
    Ok(())
}

pub(super) struct Loop<'a> {
    app: EframeWinitApplication<'a>,
    keypad: Keypad,
    modifiers: ModifiersState,
    failed: Option<String>,
}

impl<'a> Loop<'a> {
    pub(super) fn new(app: EframeWinitApplication<'a>, keypad: Keypad) -> Self {
        Self {
            app,
            keypad,
            modifiers: ModifiersState::empty(),
            failed: None,
        }
    }

    pub(super) fn failed(&self) -> Option<String> {
        self.failed.clone()
    }

    fn pass(
        &mut self,
        event_loop: &ActiveEventLoop,
        call: impl FnOnce(&mut EframeWinitApplication<'a>, &ActiveEventLoop),
    ) {
        let open = !event_loop.exiting();
        ERRORS.take();
        call(&mut self.app, event_loop);
        let logged = ERRORS.take();
        if open && event_loop.exiting() {
            self.failed = logged;
        }
    }
}

impl ApplicationHandler<UserEvent> for Loop<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.pass(event_loop, |app, l| app.resumed(l));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window: WindowId, event: WindowEvent) {
        match &event {
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput {
                event: key,
                is_synthetic,
                ..
            } if self
                .keypad
                .takes(window, key, *is_synthetic, held_of(self.modifiers)) =>
            {
                return;
            }
            _ => {}
        }
        self.pass(event_loop, |app, l| app.window_event(l, window, event));
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.pass(event_loop, |app, l| app.new_events(l, cause));
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.pass(event_loop, |app, l| app.user_event(l, event));
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, device: DeviceId, event: DeviceEvent) {
        self.pass(event_loop, |app, l| app.device_event(l, device, event));
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.pass(event_loop, |app, l| app.about_to_wait(l));
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.pass(event_loop, |app, l| app.suspended(l));
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.pass(event_loop, |app, l| app.exiting(l));
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.pass(event_loop, |app, l| app.memory_warning(l));
    }
}

impl Workspace {
    pub(super) fn keypad_watch(&self, root: Option<WindowId>, ctx: &egui::Context) -> Watch {
        let page = self.effects_page.page;
        let works = |panel: Panel| match panel {
            Panel::App => true,
            Panel::Transcription => self.transcription.over,
            Panel::Effects => page == Page::Effects,
            Panel::Color => page == Page::Color && self.effects_page.printer_keys,
        };
        let b = &self.snap.bindings;
        Watch {
            root,
            free: !ctx.egui_wants_keyboard_input(),
            capture: self.preferences.capturing(),
            taken: Action::ALL
                .iter()
                .filter(|a| works(a.panel()))
                .flat_map(|a| b.keys(*a))
                .filter(|c| on_keypad(c.key))
                .copied()
                .collect(),
        }
    }
}
