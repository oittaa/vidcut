use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::wgpu::rwh::{HasWindowHandle, RawWindowHandle};
use serde::{Deserialize, Serialize};

const SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Kept {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size: Option<[f32; 2]>,
    #[serde(default)]
    maximized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    windows: Option<[i32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    macos: Option<String>,
}

fn path() -> PathBuf {
    crate::paths::state_dir().join("window.json")
}

fn read() -> Option<Kept> {
    let kept: Kept = serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()?;
    let size = kept.size.is_none_or(|s| {
        s.iter()
            .all(|v| v.is_finite() && (1.0..=16384.0).contains(v))
    });
    size.then_some(kept)
}

fn write(kept: &Kept) {
    let file = path();
    let written = serde_json::to_string_pretty(kept)
        .map_err(std::io::Error::other)
        .and_then(|text| {
            std::fs::create_dir_all(file.parent().expect("in the state dir"))?;
            vidcut_core::temp::write(&file, text.as_bytes())
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
            written: kept.clone(),
            kept,
            changed: None,
            placed: false,
        }
    }

    pub fn builder(&self, builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        let Some(kept) = &self.kept else {
            return builder;
        };
        let builder = match kept.size {
            Some(size) => builder.with_inner_size(size),
            None => builder,
        };
        if native::PLACES {
            builder
        } else {
            builder.with_maximized(kept.maximized)
        }
    }

    pub fn watch(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let handle = frame.window_handle().ok().map(|h| h.as_raw());
        if !self.placed {
            self.placed = true;
            if let (Some(handle), Some(kept)) = (handle, &self.kept)
                && native::PLACES
            {
                native::place(handle, kept);
                if kept.maximized {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
            }
            return;
        }
        let (info, size) = ctx.input(|i| (i.viewport().clone(), i.viewport_rect().size()));
        let state = |native: fn(RawWindowHandle) -> Option<bool>, egui: Option<bool>| {
            handle.and_then(native).or(egui).unwrap_or(false)
        };
        let maximized = state(native::maximized, info.maximized);
        if state(native::minimized, info.minimized) || info.fullscreen == Some(true) {
            return;
        }
        let zoom = ctx.zoom_factor();
        let mut next = self.kept.clone().unwrap_or_default();
        next.maximized = maximized;
        if !maximized {
            next.size = Some([size.x * zoom, size.y * zoom]);
        }
        if let Some(handle) = handle {
            native::observe(handle, &mut next);
        }
        if self.written.is_none() {
            self.written = Some(next.clone());
        }
        if self.kept.as_ref() != Some(&next) {
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
        if let Some(kept) = self
            .kept
            .as_ref()
            .filter(|&k| Some(k) != self.written.as_ref())
        {
            write(kept);
            self.written = Some(kept.clone());
        }
        self.changed = None;
    }
}

#[cfg(windows)]
mod native {
    use eframe::wgpu::rwh::RawWindowHandle;
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, IsIconic, IsZoomed, SW_HIDE, SetWindowPlacement, WINDOWPLACEMENT,
    };

    use super::Kept;

    pub const PLACES: bool = true;

    fn hwnd(handle: RawWindowHandle) -> Option<HWND> {
        match handle {
            RawWindowHandle::Win32(w) => Some(w.hwnd.get() as HWND),
            _ => None,
        }
    }

    fn placement(hwnd: HWND) -> Option<WINDOWPLACEMENT> {
        // SAFETY: WINDOWPLACEMENT is plain data, all zeros a valid value.
        let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        p.length = size_of::<WINDOWPLACEMENT>() as u32;
        // SAFETY: the window is eframe's, open for this frame, and p is a WINDOWPLACEMENT of its length.
        (unsafe { GetWindowPlacement(hwnd, &mut p) } != 0).then_some(p)
    }

    pub fn maximized(handle: RawWindowHandle) -> Option<bool> {
        // SAFETY: IsZoomed only reads the state of eframe's window, open for this frame.
        hwnd(handle).map(|h| unsafe { IsZoomed(h) } != 0)
    }

    pub fn minimized(handle: RawWindowHandle) -> Option<bool> {
        // SAFETY: IsIconic only reads the state of eframe's window, open for this frame.
        hwnd(handle).map(|h| unsafe { IsIconic(h) } != 0)
    }

    pub fn observe(handle: RawWindowHandle, kept: &mut Kept) {
        if let Some(p) = hwnd(handle).and_then(placement) {
            let r = p.rcNormalPosition;
            kept.windows = Some([r.left, r.top, r.right, r.bottom]);
        }
    }

    pub fn place(handle: RawWindowHandle, kept: &Kept) {
        let (Some(hwnd), Some([left, top, right, bottom])) = (hwnd(handle), kept.windows) else {
            return;
        };
        let Some(mut p) = placement(hwnd) else {
            return;
        };
        p.flags = 0;
        p.showCmd = SW_HIDE as u32;
        p.rcNormalPosition = RECT {
            left,
            top,
            right,
            bottom,
        };
        // SAFETY: the window is eframe's, still hidden before its first frame, and p a whole WINDOWPLACEMENT.
        unsafe { SetWindowPlacement(hwnd, &p) };
    }
}

#[cfg(target_os = "macos")]
mod native {
    use eframe::wgpu::rwh::RawWindowHandle;
    use objc2_app_kit::{NSView, NSWindow};
    use objc2_foundation::NSString;

    use super::Kept;

    pub const PLACES: bool = true;

    fn with_window<T>(handle: RawWindowHandle, f: impl FnOnce(&NSWindow) -> T) -> Option<T> {
        let RawWindowHandle::AppKit(a) = handle else {
            return None;
        };
        // SAFETY: ns_view is the NSView of eframe's window, alive for this frame, on the main thread.
        let view = unsafe { a.ns_view.cast::<NSView>().as_ref() };
        let window = view.window()?;
        Some(f(&window))
    }

    pub fn maximized(handle: RawWindowHandle) -> Option<bool> {
        with_window(handle, |w| w.isZoomed())
    }

    pub fn minimized(handle: RawWindowHandle) -> Option<bool> {
        with_window(handle, |w| w.isMiniaturized())
    }

    pub fn observe(handle: RawWindowHandle, kept: &mut Kept) {
        if let Some(Some(frame)) = with_window(handle, |w| {
            (!w.isZoomed()).then(|| NSWindow::stringWithSavedFrame(w).to_string())
        }) {
            kept.macos = Some(frame);
        }
    }

    pub fn place(handle: RawWindowHandle, kept: &Kept) {
        if let Some(frame) = &kept.macos {
            with_window(handle, |w| {
                NSWindow::setFrameFromString(w, &NSString::from_str(frame))
            });
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod native {
    use eframe::wgpu::rwh::RawWindowHandle;

    use super::Kept;

    pub const PLACES: bool = false;

    pub fn maximized(_: RawWindowHandle) -> Option<bool> {
        None
    }

    pub fn minimized(_: RawWindowHandle) -> Option<bool> {
        None
    }

    pub fn observe(_: RawWindowHandle, _: &mut Kept) {}

    pub fn place(_: RawWindowHandle, _: &Kept) {}
}
