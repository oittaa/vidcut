use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut_av::player::{Config, ManualOutput, Player, Shown, SubtitleView};
use vidcut_av::screen::scope::{LARGEST, SMALLEST, marks};
use vidcut_av::screen::{Offscreen, Screen};
use vidcut_core::model::Hdr;
use vidcut_core::preferences::Scope;
use vidcut_core::time::TimeSpec;
use vidcut_core::{Error, Project, Result};

const WAIT: Duration = Duration::from_secs(60);

fn failed(what: &str) -> Error {
    Error::internal(format!("the viewer's frame: {what}"))
}

struct Frame {
    at: i64,
    offscreen: &'static Offscreen,
    shown: Shown,
    _player: Player,
}

impl Frame {
    fn of(p: &Project, at: &TimeSpec, playhead: Option<i64>) -> Result<Self> {
        let t = p.time_context(playhead).resolve(at)?;
        if t < 0 || t >= p.end().max(1) {
            return Err(Error::invalid(format!(
                "frame {t} is outside the timeline (0..{})",
                p.end()
            )));
        }
        vidcut_av::screen::retry_lost();
        let offscreen = vidcut_av::screen::offscreen().map_err(|e| failed(&e))?;
        let (info, gpu) = (&offscreen.adapter, &offscreen.gpu);
        vidcut_av::player::set_viewer_gpu(vidcut_av::gpus::adapter_id(
            info.vendor,
            info.device,
            &info.device_pci_bus_id,
        ));
        let (plan, _) = vidcut_core::render::flatten_lanes(p, t, t + 1)?;
        if let Some(lut) = &plan.output_lut {
            vidcut_av::screen::load_lut(lut).map_err(|e| Error::invalid(e.to_string()))?;
        }
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
            if let Some(shown) = player.present(Instant::now()).filter(|s| s.exact) {
                break shown;
            }
            let status = player.status();
            if status.failed.is_some() {
                return Err(Error::media(status.error.unwrap_or_default()));
            }
            if started.elapsed() > WAIT {
                return Err(failed("the player gave no picture"));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        Ok(Self {
            at: t,
            offscreen,
            shown,
            _player: player,
        })
    }

    fn draw(&self, screen: &mut Screen, width: u32, height: u32) {
        let preview = crate::preferences::load().is_ok_and(|p| p.viewer.super_scale);
        let shown = &self.shown;
        let kernels: Vec<u32> = match preview {
            true => shown
                .scales
                .iter()
                .map(|&k| vidcut_av::screen::kernel(k))
                .collect(),
            false => Vec::new(),
        };
        screen.set_output_lut(shown.output_lut.clone());
        screen.draw(
            &self.offscreen.gpu,
            Some(&shown.picture),
            &shown.titles,
            (&shown.placed, &shown.crosses, &kernels, shown.colors),
            width,
            height,
        );
    }

    fn renderer(&self) -> String {
        let info = &self.offscreen.adapter;
        format!("{:?} on {}", info.backend, info.name)
    }
}

fn write_png(out: &Path, image: &vidcut_av::thumbs::Image) -> Result<()> {
    let png = vidcut_av::export::still::png_rgba(image.width, image.height, &image.rgba)
        .map_err(|e| failed(&e.to_string()))?;
    vidcut_core::temp::write(out, &png).map_err(|e| failed(&e.to_string()))
}

pub fn render(
    p: &Project,
    at: &TimeSpec,
    out: &Path,
    width: Option<u32>,
    playhead: Option<i64>,
) -> Result<Value> {
    let frame = Frame::of(p, at, playhead)?;
    let s = &p.settings;
    let w = width.unwrap_or(s.width).max(2);
    let h = ((u64::from(w) * u64::from(s.height) / u64::from(s.width.max(1))) as u32).max(2);
    let mut screen = Screen::new();
    frame.draw(&mut screen, w, h);
    let image = screen
        .read(&frame.offscreen.gpu)
        .ok_or_else(|| failed("the GPU gave no picture back"))?;
    write_png(out, &image)?;
    let picture = &frame.shown.picture;
    let layers: Vec<Value> = std::iter::once(&**picture)
        .chain(picture.over().iter().map(|o| &*o.picture))
        .map(layer)
        .collect();
    Ok(json!({
        "path": out,
        "frame": frame.at,
        "width": image.width,
        "height": image.height,
        "viewer": true,
        "renderer": frame.renderer(),
        "layers": layers,
    }))
}

fn scope_size(scope: Scope, (width, height): (Option<u32>, Option<u32>)) -> Result<(u32, u32)> {
    let square = scope == Scope::Vectorscope;
    let (w, h) = match (width, height, square) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, true) => (w, w),
        (Some(w), None, false) => (w, w / 2),
        (None, Some(h), true) => (h, h),
        (None, Some(h), false) => (h.saturating_mul(2), h),
        (None, None, true) => (512, 512),
        (None, None, false) => (1024, 512),
    };
    if [w, h].iter().any(|v| !(SMALLEST..=LARGEST).contains(v)) {
        return Err(Error::invalid(format!(
            "a scope of {w}x{h}: each side is {SMALLEST} to {LARGEST} pixels"
        )));
    }
    Ok((w, h))
}

pub fn scope(
    p: &Project,
    at: &TimeSpec,
    out: &Path,
    scope: Scope,
    size: (Option<u32>, Option<u32>),
) -> Result<Value> {
    let size = scope_size(scope, size)?;
    let frame = Frame::of(p, at, None)?;
    let gpu = &frame.offscreen.gpu;
    let mut screen = Screen::new();
    screen.scoping(true);
    frame.draw(&mut screen, p.settings.width, p.settings.height);
    screen.scopes(gpu, &[(scope, size)]);
    let image = screen
        .read_scope(gpu, scope)
        .ok_or_else(|| failed("the GPU gave no scope back"))?;
    write_png(out, &image)?;
    let colors = frame.shown.colors;
    let marks: Vec<Value> = marks(scope, colors, size)
        .into_iter()
        .map(|m| json!({ "label": m.label, "x": m.at[0], "y": m.at[1] }))
        .collect();
    Ok(json!({
        "path": out,
        "scope": scope,
        "frame": frame.at,
        "width": image.width,
        "height": image.height,
        "colors": match colors {
            None => "sdr",
            Some(Hdr::Pq) => "pq",
            Some(Hdr::Hlg) => "hlg",
        },
        "marks": marks,
        "renderer": frame.renderer(),
    }))
}

fn layer(picture: &vidcut_av::player::Picture) -> Value {
    json!({
        "layout": format!("{:?}", picture.layout()),
        "held_in": picture.gpu().map_or("memory".into(), |k| format!("{k:?}")),
    })
}
