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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<[f32; 2]>,
}

fn path(file: &str) -> PathBuf {
    crate::paths::state_dir().join(file)
}

fn read(file: &str) -> Option<Kept> {
    let kept: Kept = serde_json::from_str(&std::fs::read_to_string(path(file)).ok()?).ok()?;
    let size = kept.size.is_none_or(|s| {
        s.iter()
            .all(|v| v.is_finite() && (1.0..=16384.0).contains(v))
    });
    let position = kept
        .position
        .is_none_or(|p| p.iter().all(|v| v.is_finite()));
    (size && position).then_some(kept)
}

fn write(file: &str, kept: &Kept) {
    let file = path(file);
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

#[derive(Debug)]
pub struct Window {
    file: &'static str,
    kept: Option<Kept>,
    written: Option<Kept>,
    changed: Option<Instant>,
    placed: bool,
}

impl Window {
    pub fn load() -> Self {
        Self::of("window.json")
    }

    fn of(file: &'static str) -> Self {
        let kept = read(file);
        Self {
            file,
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
        self.follow(ctx, handle);
    }

    fn follow(&mut self, ctx: &egui::Context, handle: Option<RawWindowHandle>) {
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
        match handle {
            Some(handle) => native::observe(handle, &mut next),
            None if !maximized => {
                next.position = info.outer_rect.map(|r| [r.min.x * zoom, r.min.y * zoom]);
            }
            None => {}
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
            write(self.file, kept);
            self.written = Some(kept.clone());
        }
        self.changed = None;
    }

    fn tool_builder(&self, ctx: &egui::Context, spec: &Separate) -> egui::ViewportBuilder {
        let zoom = ctx.zoom_factor();
        let kept = self.kept.as_ref();
        let size = kept
            .and_then(|k| k.size)
            .map_or(egui::Vec2::from(spec.size), |s| egui::Vec2::from(s) / zoom);
        let builder = egui::ViewportBuilder::default()
            .with_title(spec.title)
            .with_inner_size(size)
            .with_active(false);
        let builder = match kept.and_then(|k| k.position) {
            Some(p) => builder.with_position(egui::Pos2::from(p) / zoom),
            None if kept.and_then(|k| k.windows).is_some() => builder,
            None => match ctx.input(|i| i.viewport().inner_rect) {
                Some(main) => builder.with_position(main.center() - size / 2.0),
                None => builder,
            },
        };
        if native::FINDS {
            builder
        } else {
            builder.with_maximized(kept.is_some_and(|k| k.maximized))
        }
    }
}

pub(super) struct Separate {
    pub(super) title: &'static str,
    pub(super) file: &'static str,
    pub(super) size: [f32; 2],
}

#[derive(Debug, Default)]
pub(super) struct Tool {
    window: Option<Window>,
    open: Option<Opened>,
}

#[derive(Debug)]
struct Opened {
    builder: egui::ViewportBuilder,
    handle: Option<RawWindowHandle>,
    adopted: bool,
}

impl Tool {
    pub(super) fn builder(
        &mut self,
        ctx: &egui::Context,
        spec: &Separate,
    ) -> egui::ViewportBuilder {
        let window = self.window.get_or_insert_with(|| Window::of(spec.file));
        let open = self.open.get_or_insert_with(|| Opened {
            builder: window.tool_builder(ctx, spec),
            handle: None,
            adopted: false,
        });
        open.builder.clone()
    }

    pub(super) fn watch(
        &mut self,
        ctx: &egui::Context,
        spec: &Separate,
        owner: Option<RawWindowHandle>,
    ) {
        let (Some(window), Some(open)) = (&mut self.window, &mut self.open) else {
            return;
        };
        if open.adopted {
            window.follow(ctx, open.handle);
            return;
        }
        open.adopted = true;
        open.handle = native::find(spec.title);
        if let Some(handle) = open.handle {
            native::adopt(handle, owner, window.kept.as_ref());
        }
    }

    pub(super) fn close(&mut self) {
        self.flush();
        self.open = None;
    }

    pub(super) fn flush(&mut self) {
        if let Some(window) = &mut self.window {
            window.flush();
        }
    }
}

pub(super) fn show_fullscreen(ctx: &egui::Context, on: bool) {
    let id = egui::Id::unique("vidcut-fullscreen");
    let sent = ctx.data(|d| d.get_temp::<bool>(id));
    if sent == Some(on) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, on));
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
}

#[cfg(windows)]
mod native {
    use std::num::NonZeroIsize;

    use eframe::wgpu::rwh::{RawWindowHandle, Win32WindowHandle};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GWL_EXSTYLE, GWLP_HWNDPARENT, GetWindowLongPtrW, GetWindowPlacement,
        GetWindowTextW, IsIconic, IsZoomed, SHOW_WINDOW_CMD, SW_HIDE, SW_SHOWMAXIMIZED,
        SW_SHOWNOACTIVATE, SetWindowLongPtrW, SetWindowPlacement, ShowWindow, WINDOWPLACEMENT,
        WS_EX_APPWINDOW,
    };
    use windows_sys::core::BOOL;

    use super::Kept;

    pub const PLACES: bool = true;

    pub const FINDS: bool = true;

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
        if let (Some(window), Some(rect)) = (hwnd(handle), kept.windows) {
            set_placement(window, rect, SW_HIDE);
        }
    }

    fn set_placement(
        window: HWND,
        [left, top, right, bottom]: [i32; 4],
        show: SHOW_WINDOW_CMD,
    ) -> bool {
        let Some(mut p) = placement(window) else {
            return false;
        };
        p.flags = 0;
        p.showCmd = show as u32;
        p.rcNormalPosition = RECT {
            left,
            top,
            right,
            bottom,
        };
        // SAFETY: the window is eframe's, open for this frame, and p a whole WINDOWPLACEMENT.
        unsafe { SetWindowPlacement(window, &p) != 0 }
    }

    struct Search {
        title: Vec<u16>,
        found: isize,
    }

    extern "system" fn each(window: HWND, search: LPARAM) -> BOOL {
        // SAFETY: search is the Search of find, alive and borrowed by nothing else during the enumeration.
        let search = unsafe { &mut *(search as *mut Search) };
        let mut text = [0u16; 64];
        // SAFETY: the window is this thread's, and text a buffer of the length given.
        let n = unsafe { GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32) };
        if text.get(..usize::try_from(n).unwrap_or(0)) == Some(&search.title[..]) {
            search.found = window as isize;
            return 0;
        }
        1
    }

    pub fn find(title: &str) -> Option<RawWindowHandle> {
        let mut search = Search {
            title: title.encode_utf16().collect(),
            found: 0,
        };
        // SAFETY: each runs only during this call, and search outlives it.
        unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(each),
                (&raw mut search) as LPARAM,
            )
        };
        NonZeroIsize::new(search.found).map(|h| RawWindowHandle::Win32(Win32WindowHandle::new(h)))
    }

    pub fn adopt(handle: RawWindowHandle, owner: Option<RawWindowHandle>, kept: Option<&Kept>) {
        let Some(window) = hwnd(handle) else {
            return;
        };
        // SAFETY: the window is eframe's, open for this frame.
        unsafe { ShowWindow(window, SW_HIDE) };
        if let Some(owner) = owner.and_then(hwnd) {
            // SAFETY: both windows are eframe's, open for this frame.
            unsafe {
                SetWindowLongPtrW(window, GWLP_HWNDPARENT, owner as isize);
                let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
                SetWindowLongPtrW(window, GWL_EXSTYLE, style & !(WS_EX_APPWINDOW as isize));
            }
        }
        let show = match kept {
            Some(k) if k.maximized => SW_SHOWMAXIMIZED,
            _ => SW_SHOWNOACTIVATE,
        };
        let placed = kept
            .and_then(|k| k.windows)
            .is_some_and(|rect| set_placement(window, rect, show));
        if !placed {
            // SAFETY: the window is eframe's, open for this frame.
            unsafe { ShowWindow(window, show) };
        }
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

    pub const FINDS: bool = false;

    pub fn find(_: &str) -> Option<RawWindowHandle> {
        None
    }

    pub fn adopt(_: RawWindowHandle, _: Option<RawWindowHandle>, _: Option<&Kept>) {}
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

    pub const FINDS: bool = false;

    pub fn find(_: &str) -> Option<RawWindowHandle> {
        None
    }

    pub fn adopt(_: RawWindowHandle, _: Option<RawWindowHandle>, _: Option<&Kept>) {}
}
