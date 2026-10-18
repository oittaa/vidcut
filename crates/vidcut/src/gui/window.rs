use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use serde::{Deserialize, Serialize};

const SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Kept {
    size: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<[f32; 2]>,
    #[serde(default)]
    maximized: bool,
}

fn path() -> PathBuf {
    crate::paths::state_dir().join("window.json")
}

fn read() -> Option<Kept> {
    let kept: Kept = serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()?;
    let size = kept
        .size
        .iter()
        .all(|v| v.is_finite() && (1.0..=16384.0).contains(v));
    let position = kept
        .position
        .is_none_or(|p| p.iter().all(|v| v.is_finite() && v.abs() <= 65536.0));
    (size && position).then_some(kept)
}

fn write(kept: &Kept) {
    let file = path();
    let part = file.with_extension("part");
    let written = serde_json::to_string_pretty(kept)
        .map_err(std::io::Error::other)
        .and_then(|text| {
            std::fs::create_dir_all(file.parent().expect("in the state dir"))?;
            std::fs::write(&part, text)?;
            std::fs::rename(&part, &file)
        });
    if let Err(e) = written {
        eprintln!("vidcut: {}: {e}", file.display());
    }
}

pub struct Window {
    kept: Option<Kept>,
    written: Option<Kept>,
    changed: Option<Instant>,
    placed: bool,
}

impl Window {
    pub fn load() -> Self {
        let kept = read();
        Self {
            kept,
            written: kept,
            changed: None,
            placed: false,
        }
    }

    pub fn builder(&self, builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        let Some(kept) = self.kept else {
            return builder;
        };
        let builder = builder
            .with_inner_size(kept.size)
            .with_maximized(kept.maximized);
        match kept.position.filter(|_| cfg!(windows)) {
            Some(at) => builder.with_position(at),
            None => builder,
        }
    }

    pub fn watch(&mut self, ctx: &egui::Context) {
        let (info, size) = ctx.input(|i| (i.viewport().clone(), i.viewport_rect().size()));
        let zoom = ctx.zoom_factor();
        if !self.placed {
            self.placed = true;
            keep_on_screen(ctx, &info, zoom, self.kept.is_some_and(|k| k.maximized));
        }
        if info.minimized == Some(true) || info.fullscreen == Some(true) {
            return;
        }
        let maximized = info.maximized == Some(true);
        let mut next = self.kept.unwrap_or(Kept {
            size: [size.x * zoom, size.y * zoom],
            position: None,
            maximized,
        });
        if !maximized {
            next.size = [size.x * zoom, size.y * zoom];
            next.position = info.outer_rect.map(|r| [r.min.x * zoom, r.min.y * zoom]);
        }
        next.maximized = maximized;
        if self.written.is_none() {
            self.written = Some(next);
        }
        if self.kept != Some(next) {
            self.kept = Some(next);
            self.changed = Some(Instant::now());
        }
        let Some(changed) = self.changed else {
            return;
        };
        if self.kept == self.written {
            self.changed = None;
        } else if changed.elapsed() >= SETTLE {
            self.flush();
        } else {
            ctx.request_repaint_after(SETTLE.saturating_sub(changed.elapsed()));
        }
    }

    pub fn flush(&mut self) {
        if let Some(kept) = self.kept.filter(|k| Some(*k) != self.written) {
            write(&kept);
            self.written = Some(kept);
        }
        self.changed = None;
    }
}

#[cfg(windows)]
fn keep_on_screen(ctx: &egui::Context, info: &egui::ViewportInfo, zoom: f32, maximized: bool) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect};
    let (Some(outer), Some(native)) = (info.outer_rect, info.native_pixels_per_point) else {
        return;
    };
    let px = |v: f32| (v * zoom * native).round() as i32;
    let rect = RECT {
        left: px(outer.min.x),
        top: px(outer.min.y),
        right: px(outer.max.x),
        bottom: px(outer.max.y),
    };
    // SAFETY: MonitorFromRect reads the RECT, which outlives the call.
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) };
    if monitor.is_null() {
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(64.0, 64.0)));
        if maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
    }
}

#[cfg(not(windows))]
fn keep_on_screen(_: &egui::Context, _: &egui::ViewportInfo, _: f32, _: bool) {}
