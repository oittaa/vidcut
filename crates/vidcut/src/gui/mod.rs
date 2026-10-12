//! The GUI: eframe (glow) on the UI thread; the project, players and
//! everything else in the live session (crate::live). See
//! docs/decisions/gui.md.

mod app;
pub mod export;
mod folders;
mod import;
mod mixer;
mod pool;
mod preferences;
mod stills;
mod theme;
mod timeline;
mod video;
mod viewer;
pub mod workspace;

use std::path::PathBuf;

use eframe::egui;
use vidcut_core::{Error, ErrorCode, Result};

/// Runs the GUI until its window closes, optionally opening `project`
/// (created if it doesn't exist).
pub fn run(project: Option<PathBuf>) -> Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        // With vsync, NVIDIA's EGL on Wayland blocks eglSwapBuffers until the
        // compositor sends a frame callback, which GNOME withholds while the
        // window is hidden: the UI (= mpv render) thread would hang
        // (docs/decisions/preview.md §6). Repaints follow mpv's frames anyway.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        viewport: egui::ViewportBuilder::default()
            .with_title("vidcut")
            .with_app_id("vidcut")
            .with_inner_size([1600.0, 980.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vidcut",
        options,
        Box::new(move |cc| {
            app::App::new(cc, project)
                .map(|a| Box::new(a) as Box<dyn eframe::App>)
                .map_err(Into::into)
        }),
    )
    .map_err(|e| {
        Error::new(
            ErrorCode::Dependency,
            format!("can't start the GUI (is a Wayland or X11 display available?): {e}"),
        )
    })
}
