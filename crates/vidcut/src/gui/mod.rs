mod app;
mod attributes;
mod bar;
mod cache_manager;
mod cells;
mod clip_info;
mod color_page;
mod compound;
mod compressor;
mod dialog;
mod ducking;
mod effect_browser;
mod effects_page;
mod equalizer;
pub mod export;
mod export_page;
mod file_manager;
mod fillers;
mod folders;
pub mod frame;
mod generator_browser;
mod grade_keys;
mod grade_windows;
mod help;
mod history;
mod import;
mod inspector;
mod isolation;
mod keyframe_editor;
mod leveler;
mod look_tiles;
mod looks;
mod luts;
mod mixer;
mod node_editor;
mod noise;
mod normalize;
mod onscreen;
mod placing;
mod pool;
mod preferences;
mod project;
mod relink;
mod retime;
mod retime_process;
mod row;
mod scopes;
pub(crate) mod shortcuts;
mod shuttle;
mod speech;
mod stabilize;
mod subclip;
mod subtitles;
mod sync_bin;
mod sync_clips;
mod theme;
mod thumbnail_timeline;
mod tiles;
mod timeline;
mod timelines;
mod title_browser;
mod titles;
mod transcription;
mod transition_browser;
mod trim_strips;
mod upscale;
mod video;
mod viewer;
mod window;
mod window_overlay;
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
                device_descriptor: std::sync::Arc::new(|adapter| {
                    let most = adapter.limits();
                    wgpu::DeviceDescriptor {
                        label: Some("vidcut"),
                        required_features: vidcut_av::screen::features(adapter),
                        required_limits: wgpu::Limits {
                            max_texture_dimension_2d: most.max_texture_dimension_2d,
                            max_storage_buffer_binding_size: most.max_storage_buffer_binding_size,
                            max_buffer_size: most.max_buffer_size,
                            ..wgpu::Limits::default()
                        },
                        ..Default::default()
                    }
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
