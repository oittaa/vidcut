#[cfg(any(target_os = "linux", windows))]
mod acquire;
mod app;
mod cells;
pub mod export;
mod file_manager;
mod fillers;
mod folders;
mod gate;
mod help;
mod history;
mod import;
mod mixer;
mod onscreen;
pub mod picture;
mod pool;
mod preferences;
mod project;
mod relink;
mod retime;
mod shared;
mod shuttle;
mod speech;
mod subtitles;
mod theme;
mod tiles;
mod timeline;
mod titles;
mod transcription;
mod trim_strips;
mod video;
mod viewer;
#[cfg(any(target_os = "linux", windows))]
mod vulkan;
mod window;
pub mod workspace;

use std::path::PathBuf;

use eframe::egui;
use vidcut_core::{Error, ErrorCode, Result};

pub fn run(project: Option<PathBuf>) -> Result<()> {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu;
    let window = window::Window::load();
    let viewport = window.builder(
        egui::ViewportBuilder::default()
            .with_title("vidcut")
            .with_app_id("vidcut")
            .with_inner_size([1600.0, 980.0])
            .with_min_inner_size([960.0, 600.0]),
    );
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: WgpuConfiguration {
            surface: eframe::SurfaceConfig {
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: Some(1),
            },
            wgpu_setup: WgpuSetup::CreateNew(WgpuSetupCreateNew {
                instance_descriptor: wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
                    ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
                },
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
        viewport,
        ..Default::default()
    };
    let ran = eframe::run_native(
        "vidcut",
        options,
        Box::new(move |cc| {
            app::App::new(cc, project, window)
                .map(|a| Box::new(a) as Box<dyn eframe::App>)
                .map_err(Into::into)
        }),
    );
    crate::threads::ending("the closing threads");
    crate::threads::join_closing();
    crate::threads::ending("the process's exit");
    if ran.is_ok() {
        crate::threads::exit_now(0);
    }
    ran.map_err(|e| {
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
