//! The GUI: eframe on wgpu (Vulkan on Linux, Vulkan or Direct3D 12 on
//! Windows) on the UI thread; the project, the player and everything else
//! in the live session (crate::live). See docs/decisions/gui.md.

#[cfg(any(target_os = "linux", windows))]
mod acquire;
mod app;
mod cells;
mod close;
pub mod export;
mod file_manager;
mod folders;
mod help;
mod import;
mod mixer;
pub mod picture;
mod pool;
mod preferences;
mod project;
mod relink;
mod shared;
mod theme;
mod tiles;
mod timeline;
mod video;
mod viewer;
#[cfg(any(target_os = "linux", windows))]
mod vulkan;
pub mod workspace;

use std::path::PathBuf;

use eframe::egui;
use vidcut_core::{Error, ErrorCode, Result};

/// Runs the GUI until its window closes, optionally opening `project`
/// (created if it doesn't exist).
pub fn run(project: Option<PathBuf>) -> Result<()> {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu;
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: WgpuConfiguration {
            // Mailbox (or Immediate where there is one): a new picture shows
            // at the next refresh, no frames queued. Presenting a hidden
            // window can still block the UI thread (NVIDIA on Wayland), so
            // the viewer presents on a thread of its own
            // (docs/decisions/preview.md §4).
            surface: eframe::SurfaceConfig {
                present_mode: wgpu::PresentMode::AutoNoVsync,
                desired_maximum_frame_latency: Some(1),
            },
            wgpu_setup: WgpuSetup::CreateNew(WgpuSetupCreateNew {
                instance_descriptor: wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
                    ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
                },
                // 16-bit textures for P010 pictures, and pictures as large
                // as the GPU takes.
                device_descriptor: std::sync::Arc::new(|adapter| wgpu::DeviceDescriptor {
                    label: Some("vidcut"),
                    required_features: picture::features(adapter),
                    required_limits: wgpu::Limits {
                        max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
                        ..wgpu::Limits::default()
                    },
                    ..Default::default()
                }),
                ..WgpuSetupCreateNew::without_display_handle()
            }),
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
        #[cfg(all(unix, not(target_os = "macos")))]
        let hint = " (is a Wayland or X11 display available?)";
        #[cfg(any(windows, target_os = "macos"))]
        let hint = "";
        Error::new(
            ErrorCode::Dependency,
            format!("can't start the GUI{hint}: {e}"),
        )
    })
}

/// Shortcuts as this system names its keys: what Linux and Windows call
/// Ctrl and Alt are Cmd and Option on macOS (the spec's key names). The
/// keys themselves are egui's `command` and `alt` everywhere.
pub fn keys(text: &str) -> String {
    keys_on(text, cfg!(target_os = "macos"))
}

fn keys_on(text: &str, mac: bool) -> String {
    if mac {
        text.replace("Ctrl+", "Cmd+").replace("Alt+", "Option+")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn macos_names_ctrl_and_alt_cmd_and_option() {
        let text = "Lift (Alt+Delete), Shift+Ctrl+N";
        assert_eq!(super::keys_on(text, false), text);
        assert_eq!(
            super::keys_on(text, true),
            "Lift (Option+Delete), Shift+Cmd+N"
        );
    }
}
