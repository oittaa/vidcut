use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut_av::player::{Config, ManualOutput, Player, SubtitleView};
use vidcut_core::time::TimeSpec;
use vidcut_core::{Error, Project, Result};

use super::picture::{self, Gpu, Screen};

const WAIT: Duration = Duration::from_secs(60);

fn failed(what: &str) -> Error {
    Error::internal(format!("the viewer's frame: {what}"))
}

fn device() -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
        ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference:
            wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance),
        ..Default::default()
    }))
    .map_err(|e| failed(&format!("no GPU for wgpu: {e}")))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("vidcut viewer frame"),
        required_features: picture::features(&adapter),
        required_limits: wgpu::Limits {
            max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
            ..wgpu::Limits::default()
        },
        ..Default::default()
    }))
    .map_err(|e| failed(&format!("no wgpu device: {e}")))?;
    Ok((adapter, device, queue))
}

pub fn render(
    p: &Project,
    at: &TimeSpec,
    out: &Path,
    width: Option<u32>,
    playhead: Option<i64>,
) -> Result<Value> {
    let t = p.time_context(playhead).resolve(at)?;
    if t < 0 || t >= p.end().max(1) {
        return Err(Error::invalid(format!(
            "frame {t} is outside the timeline (0..{})",
            p.end()
        )));
    }
    let s = &p.settings;
    let w = width.unwrap_or(s.width).max(2);
    let h = ((u64::from(w) * u64::from(s.height) / u64::from(s.width.max(1))) as u32).max(2);
    let (adapter, device, queue) = device()?;
    let info = adapter.get_info();
    vidcut_av::player::set_viewer_gpu(vidcut_av::gpus::adapter_id(
        info.vendor,
        info.device,
        &info.device_pci_bus_id,
    ));
    let gpu = Gpu::new(&device, &queue);
    let (plan, _) = vidcut_core::render::flatten_lanes(p, t, t + 1)?;
    let config = Config {
        audio_scrubbing: false,
        hardware: crate::preferences::gpu_decoding().on,
        gpu_pictures: gpu.gpu_pictures(),
        ..Config::default()
    };
    let player = Player::new(plan, config, Box::new(ManualOutput::new()))
        .map_err(|e| failed(&e.to_string()))?;
    player.set_subtitles(p.subtitles.iter().find(|t| t.shown).map(|t| {
        let mut cues = t.cues.clone();
        for c in &mut cues {
            c.color = p.speakers.color_of(c);
        }
        SubtitleView {
            style: t.style.clone(),
            cues,
        }
    }));
    player.follow(t, 0.0);
    let started = Instant::now();
    let shown = loop {
        match player.present(Instant::now()) {
            Some(shown) if shown.exact => break shown,
            _ if started.elapsed() > WAIT => return Err(failed("the player gave no picture")),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    let preview = crate::preferences::load().is_ok_and(|p| p.viewer.super_scale);
    let kernels: Vec<u32> = match preview {
        true => shown
            .scales
            .iter()
            .map(|&k| super::video::kernel(k))
            .collect(),
        false => Vec::new(),
    };
    let mut screen = Screen::new();
    screen.draw(
        &gpu,
        Some(&shown.picture),
        &shown.titles,
        (&shown.placed, &shown.crosses, &kernels, shown.colors),
        w,
        h,
    );
    let image = screen
        .read(&gpu)
        .ok_or_else(|| failed("the GPU gave no picture back"))?;
    let png = vidcut_av::export::still::png_rgba(image.width, image.height, &image.rgba)
        .map_err(|e| failed(&e.to_string()))?;
    vidcut_core::temp::write(out, &png).map_err(|e| failed(&e.to_string()))?;
    let picture = &shown.picture;
    Ok(json!({
        "path": out,
        "frame": t,
        "width": image.width,
        "height": image.height,
        "viewer": true,
        "renderer": format!("{:?} on {}", info.backend, info.name),
        "picture": {
            "layout": format!("{:?}", picture.layout()),
            "held_in": picture.gpu().map_or("memory".into(), |k| format!("{k:?}")),
            "layers": 1 + picture.over().len(),
        },
    }))
}
