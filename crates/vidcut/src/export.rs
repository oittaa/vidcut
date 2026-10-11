//! Export as the session runs it: presets, outputs, plans, encoder cache.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};
use vidcut_av::export::encode::Availability;
use vidcut_av::export::{self as engine, Progress, Report};
use vidcut_core::export::{
    Container, EncoderChoice, ExportSettings, Preset, apply_overrides, builtin_presets, validate,
};
use vidcut_core::render::{self, RenderPlan, VideoPiece};
use vidcut_core::request::ExportParams;
use vidcut_core::time::{TimeContext, TimeSpec};
use vidcut_core::{Error, ErrorCode, Project, Result};

use crate::paths::{cache_dir, config_dir};

/// Built-in presets plus user presets from `~/.config/vidcut/presets/*.toml`.
pub fn presets() -> Result<Vec<Preset>> {
    let mut all = builtin_presets();
    let dir = config_dir().join("presets");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(all);
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
        let mut value: toml::Table =
            toml::from_str(&text).map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        value
            .entry("name")
            .or_insert_with(|| toml::Value::String(stem.clone()));
        value
            .entry("title")
            .or_insert_with(|| toml::Value::String(stem.clone()));
        let preset: Preset = toml::Value::Table(value)
            .try_into()
            .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
        validate(&preset.settings)
            .map_err(|e| Error::usage(format!("{}: {}", path.display(), e.message)))?;
        // A user preset may replace a built-in one of the same name.
        all.retain(|p| p.name != preset.name);
        all.push(preset);
    }
    Ok(all)
}

fn preset(name: &str) -> Result<Preset> {
    presets()?
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| Error::not_found(format!("no preset {name:?}; see `vidcut presets`")))
}

/// The default preset for an output file extension.
fn default_preset(output: &Path) -> &'static str {
    match output
        .extension()
        .and_then(|e| e.to_str())
        .and_then(Container::from_extension)
    {
        Some(Container::Wav) => "audio-wav",
        Some(Container::Flac) => "audio-flac",
        Some(Container::Mp3) => "audio-mp3",
        Some(Container::M4a) => "audio-aac",
        Some(Container::Ogg) => "audio-opus",
        Some(Container::Webm) => "vp9-webm",
        _ => "h264-master",
    }
}

/// One file to write.
pub struct Output {
    pub plan: RenderPlan,
    pub path: PathBuf,
}

/// A checked export: settings and the files to write.
pub struct Prepared {
    pub settings: ExportSettings,
    pub preset: Option<String>,
    pub outputs: Vec<Output>,
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\0' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "clip".into() } else { s }
}

fn file_name(template: &str, index: usize, name: &str, clip: &str, media: &str) -> String {
    let mut s = template
        .replace("{name}", name)
        .replace("{clip}", clip)
        .replace("{media}", media)
        .replace("{index}", &index.to_string());
    // {index:0N}
    while let Some(start) = s.find("{index:0") {
        let Some(end) = s[start..].find('}') else {
            break;
        };
        let width: usize = s[start + 8..start + end].parse().unwrap_or(1);
        s.replace_range(start..start + end + 1, &format!("{index:0width$}"));
    }
    sanitize(&s)
}

/// Resolves settings, range and outputs for an export.
pub fn prepare(p: &Project, params: &ExportParams, playhead: Option<i64>) -> Result<Prepared> {
    let (mut settings, preset_name) = match (&params.settings, &params.preset) {
        (Some(s), None) => (s.clone(), None),
        (None, Some(name)) => (preset(name)?.settings, Some(name.clone())),
        (None, None) => {
            let name = if params.per_clip {
                "h264-master"
            } else {
                default_preset(&params.output)
            };
            (preset(name)?.settings, Some(name.to_string()))
        }
        (Some(_), Some(_)) => {
            return Err(Error::usage("give either a preset or settings, not both"));
        }
    };
    apply_overrides(&mut settings, &params.set)?;
    if let Some(enc) = params.encoder
        && let Some(v) = settings.video.as_mut()
    {
        v.encoder = enc;
    }
    if !params.per_clip
        && let Some(c) = params
            .output
            .extension()
            .and_then(|e| e.to_str())
            .and_then(Container::from_extension)
    {
        settings.container = c;
    }
    if params.audio_only || settings.container.audio_only() {
        settings.video = None;
    }
    validate(&settings)?;

    let end = p.end();
    if end == 0 {
        return Err(Error::invalid("the timeline is empty: nothing to export"));
    }
    let ctx: TimeContext = p.time_context(playhead);
    let (a, b) = match &params.range {
        Some(r) => ctx.range(r)?,
        None => match (p.marks.mark_in, p.marks.mark_out) {
            (Some(i), Some(o)) => (i, o),
            _ => (0, end),
        },
    };
    let (a, b) = (a.max(0), b.min(end));
    if b <= a {
        return Err(Error::invalid(format!("nothing to export in {a}..{b}")));
    }

    // Plan at the export's audio rate so sample math matches the encoder.
    let mut proj = p.clone();
    if let Some(audio) = &settings.audio {
        let enc = vidcut_av::export::encode::resolve_audio(
            audio,
            p.settings.sample_rate,
            p.settings.channels,
        )
        .map_err(|e| Error::new(ErrorCode::Export, e.to_string()))?;
        proj.settings.sample_rate = enc.rate;
        proj.settings.channels = enc.channels;
    }
    let ext = settings.container.extension();
    let mut outputs = Vec::new();
    if params.per_clip {
        std::fs::create_dir_all(&params.output)
            .map_err(|e| Error::usage(format!("{}: {e}", params.output.display())))?;
        let template = params.name.as_deref().unwrap_or("{index:03}-{name}");
        let clips: Vec<_> = proj
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.start < b && c.end() > a)
            .map(|c| (c.start, c.id.clone(), c.media.clone()))
            .collect();
        let mut clips = clips;
        clips.sort();
        for (i, (_, id, media)) in clips.iter().enumerate() {
            let plan = render::flatten_clip(&proj, id)?;
            let name = file_name(template, i + 1, &plan.name, id, media);
            outputs.push(Output {
                path: params.output.join(format!("{name}.{ext}")),
                plan,
            });
        }
        if outputs.is_empty() {
            return Err(Error::invalid("no clips in the export range"));
        }
    } else {
        outputs.push(Output {
            plan: render::flatten(&proj, a, b)?,
            path: params.output.clone(),
        });
    }
    for o in &outputs {
        if o.path.exists() && !params.overwrite {
            return Err(Error::invalid(format!(
                "{} exists; pass overwrite to replace it",
                o.path.display()
            )));
        }
    }
    Ok(Prepared {
        settings,
        preset: preset_name,
        outputs,
    })
}

/// A summary of a prepared export (the `export.plan` result).
pub fn describe(prepared: &Prepared, available: &Availability) -> Value {
    let video_encoder = prepared.settings.video.as_ref().map(|v| match vidcut_av::export::encode::resolve_video(v, available) {
        Ok(e) => json!({ "name": e.name, "kind": e.kind, "pixel": vidcut_av::export::encode::pixel_name(e.pixel), "device": e.device, "options": e.options }),
        Err(err) => json!({ "error": err.to_string() }),
    });
    json!({
        "preset": prepared.preset,
        "settings": prepared.settings,
        "video_encoder": video_encoder,
        "outputs": prepared.outputs.iter().map(|o| json!({
            "path": o.path,
            "frames": o.plan.frames,
            "range": o.plan.range,
            "video_pieces": o.plan.video.len(),
            "audio_lanes": o.plan.audio.len(),
            "plan": o.plan,
        })).collect::<Vec<_>>(),
    })
}

/// Encoder availability, from the cache when it matches this system.
pub fn availability(refresh: bool) -> Availability {
    let key = {
        let read = |p: &str| {
            std::fs::read_to_string(p)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let nodes: Vec<String> = vidcut_av::export::encode::render_nodes()
            .into_iter()
            .map(|(n, d)| format!("{n}={d}"))
            .collect();
        format!(
            "{:?}|nvidia {}|kernel {}|{}",
            vidcut_av::library_versions(),
            read("/sys/module/nvidia/version"),
            read("/proc/sys/kernel/osrelease"),
            nodes.join(",")
        )
    };
    let path = cache_dir().join("encoders.json");
    if !refresh
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cached) = serde_json::from_str::<Value>(&text)
        && cached["key"] == key
        && let Ok(a) = serde_json::from_value::<Availability>(cached["availability"].clone())
    {
        return a;
    }
    let a = vidcut_av::export::encode::detect();
    if std::fs::create_dir_all(cache_dir()).is_ok() {
        let _ = std::fs::write(&path, json!({ "key": key, "availability": a }).to_string());
    }
    a
}

/// Whether the settings may pick a GPU encoder (so availability matters).
pub fn needs_gpu_info(settings: &ExportSettings) -> bool {
    settings
        .video
        .as_ref()
        .is_some_and(|v| v.encoder != EncoderChoice::Cpu)
}

/// Runs a prepared export, one output after another.
pub fn run(
    prepared: &Prepared,
    available: &Availability,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, &Path, Progress),
) -> Result<Vec<Report>> {
    let mut reports = Vec::new();
    for (i, o) in prepared.outputs.iter().enumerate() {
        if let Some(dir) = o.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::usage(format!("{}: {e}", dir.display())))?;
        }
        let report = engine::run(
            &o.plan,
            &prepared.settings,
            &o.path,
            available,
            cancel,
            &mut |pr| progress(i, &o.path, pr),
        )
        .map_err(|e| {
            if engine::is_cancelled(&e) {
                Error::new(ErrorCode::Cancelled, "export cancelled")
            } else {
                Error::new(ErrorCode::Export, format!("{}: {e}", o.path.display()))
            }
        })?;
        reports.push(report);
    }
    Ok(reports)
}

/// Renders the timeline (or a media item) at `at` to a PNG.
pub fn frame(
    p: &Project,
    at: &TimeSpec,
    out: &Path,
    media: Option<&str>,
    width: Option<u32>,
    playhead: Option<i64>,
) -> Result<Value> {
    let fps = p.settings.fps;
    let (piece, (w, h), frame) = match media {
        None => {
            let t = p.time_context(playhead).resolve(at)?;
            if t < 0 || t >= p.end().max(1) {
                return Err(Error::invalid(format!(
                    "frame {t} is outside the timeline (0..{})",
                    p.end()
                )));
            }
            let plan = render::flatten(p, t, t + 1)?;
            (
                plan.video[0].clone(),
                (p.settings.width, p.settings.height),
                t,
            )
        }
        Some(id) => {
            let m = p
                .media(id)
                .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
            let v = m
                .info
                .video
                .as_ref()
                .ok_or_else(|| Error::invalid(format!("{id} has no video")))?;
            let frames = p.media_frames(m);
            let ctx = TimeContext {
                fps,
                start_tc: 0,
                end: frames,
                mark_in: None,
                mark_out: None,
                playhead: None,
            };
            let t = ctx.resolve(at)?;
            if t < 0 || t >= frames {
                return Err(Error::invalid(format!(
                    "frame {t} is outside {id} (0..{frames})"
                )));
            }
            let sar = if v.sar[0] > 0 && v.sar[1] > 0 {
                f64::from(v.sar[0]) / f64::from(v.sar[1])
            } else {
                1.0
            };
            let piece = VideoPiece::Source {
                media: id.into(),
                path: m.path.clone(),
                origin: v.first_pts,
                source_frame: t,
                frames: 1,
            };
            (
                piece,
                (
                    ((f64::from(v.width) * sar).round() as u32) & !1,
                    v.height & !1,
                ),
                t,
            )
        }
    };
    let (w, h) = match width {
        Some(nw) => (
            nw & !1,
            ((f64::from(nw) * f64::from(h) / f64::from(w)).round() as u32).max(2) & !1,
        ),
        None => (w, h),
    };
    vidcut_av::export::still::write_png(&piece, fps, w.max(2), h.max(2), out)
        .map_err(|e| Error::new(ErrorCode::Export, e.to_string()))?;
    Ok(json!({ "path": out, "frame": frame, "width": w, "height": h }))
}
