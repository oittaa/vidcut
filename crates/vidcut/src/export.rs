//! Export as the session runs it: presets, outputs, plans, encoder cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};
use vidcut_av::export::encode::Availability;
use vidcut_av::export::{self as engine, Progress, Report};
use vidcut_av::light::Span;
use vidcut_core::export::color::{self, ColorChoice, PictureKind, PictureShare};
use vidcut_core::export::metadata::{Measured, PieceMeta};
use vidcut_core::export::{
    Container, EncoderChoice, ExportSettings, Preset, apply_overrides, builtin_presets, validate,
};
use vidcut_core::otio::{self, Locations};
use vidcut_core::render::{self, RenderPlan};
use vidcut_core::request::ExportParams;
use vidcut_core::time::{TimeContext, TimeSpec};
use vidcut_core::{Error, ErrorCode, Project, Result};

use crate::bundle;
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

/// Where a user preset lives.
fn user_preset_path(name: &str) -> PathBuf {
    config_dir().join("presets").join(format!("{name}.toml"))
}

/// Whether `name` is a user preset (which may shadow a built-in one).
pub fn is_user_preset(name: &str) -> bool {
    check_preset_name(name).is_ok() && user_preset_path(name).exists()
}

fn check_preset_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::usage(format!(
            "preset name {name:?}: use letters, digits, '-', '_' and '.'"
        )))
    }
}

/// What `presets.save` stores.
pub struct PresetSave<'a> {
    pub name: &'a str,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub quick_export: bool,
    pub settings: Option<&'a ExportSettings>,
    pub from: Option<&'a str>,
    pub set: &'a [String],
    pub overwrite: bool,
    /// Check and describe it without writing it.
    pub dry_run: bool,
}

/// Saves a user preset as TOML (`presets.save`).
pub fn save_preset(s: &PresetSave<'_>) -> Result<Value> {
    check_preset_name(s.name)?;
    let mut settings = match (s.settings, s.from) {
        (Some(settings), None) => settings.clone(),
        (None, Some(from)) => preset(from)?.settings,
        _ => {
            return Err(Error::usage(
                "presets.save needs exactly one of `settings` and `from`",
            ));
        }
    };
    apply_overrides(&mut settings, s.set)?;
    validate(&settings)?;
    let path = user_preset_path(s.name);
    let builtin = builtin_presets().iter().any(|p| p.name == s.name);
    if (path.exists() || builtin) && !s.overwrite {
        return Err(Error::invalid(format!(
            "preset {:?} exists{}; pass overwrite to replace it",
            s.name,
            if builtin { " (built in)" } else { "" }
        )));
    }
    let preset = Preset {
        name: s.name.into(),
        title: s.title.unwrap_or(s.name).into(),
        description: s.description.unwrap_or_default().into(),
        quick_export: s.quick_export,
        settings,
    };
    if s.dry_run {
        return Ok(json!({ "preset": preset, "path": path }));
    }
    let text = toml::to_string(&preset).map_err(|e| Error::internal(e.to_string()))?;
    let dir = path.parent().expect("has a parent");
    std::fs::create_dir_all(dir).map_err(|e| Error::usage(format!("{}: {e}", dir.display())))?;
    let part = path.with_extension("toml.part");
    std::fs::write(&part, text)
        .and_then(|()| std::fs::rename(&part, &path))
        .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
    Ok(json!({ "preset": preset, "path": path }))
}

/// Deletes a user preset (`presets.delete`); `dry_run` only checks.
pub fn delete_preset(name: &str, dry_run: bool) -> Result<Value> {
    check_preset_name(name)?;
    let path = user_preset_path(name);
    if !path.exists() {
        return Err(if builtin_presets().iter().any(|p| p.name == name) {
            Error::invalid(format!("{name:?} is built in and can't be deleted"))
        } else {
            Error::not_found(format!("no user preset {name:?}"))
        });
    }
    if !dry_run {
        std::fs::remove_file(&path)
            .map_err(|e| Error::usage(format!("{}: {e}", path.display())))?;
    }
    Ok(json!({ "deleted": name, "path": path }))
}

/// A timeline for other editors (deviation X9), as an output's extension
/// names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineFormat {
    /// `.otio`: the edit.
    Otio,
    /// `.otioz` or `.otiod`: the edit with its media.
    Bundle(bundle::Kind),
}

impl TimelineFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Otio => "otio",
            Self::Bundle(kind) => kind.name(),
        }
    }
}

/// The timeline format `output` names, if it names one.
pub fn timeline_format(output: &Path) -> Option<TimelineFormat> {
    match bundle::Kind::of(output) {
        Some(kind) => Some(TimelineFormat::Bundle(kind)),
        None => is_otio(output).then_some(TimelineFormat::Otio),
    }
}

/// Whether `output` is an `.otio` file: the edit alone, written at once.
pub fn is_otio(output: &Path) -> bool {
    output
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("otio"))
}

/// The name a timeline export gives the timeline: the project file's.
pub fn timeline_name(project_path: &Path) -> String {
    project_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Cancellation and byte progress for writing a bundle.
pub struct Writing<'a> {
    pub cancel: &'a AtomicBool,
    /// Called with bytes written and their total.
    pub progress: &'a mut dyn FnMut(u64, u64),
}

/// A timeline export (deviation X9): `.otio` (the edit), `.otioz` or
/// `.otiod` (the edit and its media, in a bundle). Unlike media exports it
/// covers the whole timeline unless given a range: the In/Out marks travel
/// inside the file. Without `write` it only describes the export.
pub fn timeline(
    p: &Project,
    name: &str,
    params: &ExportParams,
    playhead: Option<i64>,
    write: Option<Writing>,
) -> Result<Value> {
    let format =
        timeline_format(&params.output).ok_or_else(|| Error::internal("not a timeline export"))?;
    if params.per_clip
        || params.audio_only
        || params.preset.is_some()
        || params.settings.is_some()
        || !params.set.is_empty()
        || params.encoder.is_some()
        || params.name.is_some()
    {
        return Err(Error::usage(format!(
            "an .{} export writes the edit, not rendered media: it takes only a range and \
             overwrite",
            format.name()
        )));
    }
    // As media exports do: checked when planned, so a job doesn't start
    // only to fail.
    if params.output.exists() && !params.overwrite {
        return Err(Error::invalid(format!(
            "{} exists; pass overwrite to replace it",
            params.output.display()
        )));
    }
    let span = match &params.range {
        Some(r) => Some(p.time_context(playhead).range(r)?),
        None => None,
    };
    let (a, b) = span.unwrap_or((0, p.end()));
    let ids = otio::used_media(p, span);
    // Source ranges count from each media's timecode, which media probed
    // before vidcut kept them don't have yet.
    let mut project = p.clone();
    crate::env::fill_timecodes(&mut project, &ids);
    let items = match format {
        TimelineFormat::Otio => Vec::new(),
        TimelineFormat::Bundle(_) => bundle::plan(&project, &ids)?,
    };
    let names: HashMap<String, String> = items
        .iter()
        .map(|i| (i.media.clone(), i.location.clone()))
        .collect();
    let locations = match format {
        TimelineFormat::Otio => Locations::Absolute,
        TimelineFormat::Bundle(_) => Locations::Bundle(&names),
    };
    let doc = otio::export(&project, name, span, locations)?;
    let clips = p
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| {
            (c.video && c.start.max(a) < c.end().min(b))
                || c.audio_range().is_some_and(|(s, e)| s.max(a) < e.min(b))
        })
        .count();
    let mut summary = json!({
        "path": params.output, "format": format.name(), "range": [a, b], "frames": b - a,
        "clips": clips,
    });
    if let TimelineFormat::Bundle(_) = format {
        summary["media"] = json!(items.len());
        summary["files"] = json!(items.iter().map(|i| i.files.len()).sum::<usize>());
        summary["bytes"] = json!(items.iter().map(|i| i.bytes).sum::<u64>());
    }
    let Some(writing) = write else {
        return Ok(json!({ "preset": null, "format": format.name(), "outputs": [summary] }));
    };
    let text =
        serde_json::to_string_pretty(&doc).map_err(|e| Error::internal(e.to_string()))? + "\n";
    match format {
        TimelineFormat::Otio => {
            let part = vidcut_av::export::partial_path(&params.output);
            std::fs::write(&part, text)
                .and_then(|()| std::fs::rename(&part, &params.output))
                .map_err(|e| Error::usage(format!("{}: {e}", params.output.display())))?;
        }
        TimelineFormat::Bundle(kind) => bundle::write(
            kind,
            &params.output,
            &text,
            &items,
            params.overwrite,
            writing.cancel,
            writing.progress,
        )?,
    }
    Ok(json!({ "outputs": [summary] }))
}

fn preset(name: &str) -> Result<Preset> {
    presets()?
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| Error::not_found(format!("no preset {name:?}; see `vidcut presets list`")))
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
    /// What it is encoded with: the export's settings with `video.hdr`
    /// and an unset bit depth chosen from its pictures ([`choose_colors`]).
    pub settings: ExportSettings,
    /// What was chosen, and why (decision D44).
    pub color: Option<ColorChoice>,
    /// The HDR metadata it writes, and why (decision D46).
    pub metadata: Option<vidcut_core::export::metadata::MetadataChoice>,
}

/// A checked export: settings and the files to write.
pub struct Prepared {
    /// The settings as asked for.
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
            let ext = params.output.extension().and_then(|e| e.to_str());
            if !params.per_clip
                && let Some(ext) = ext.filter(|e| Container::from_extension(e).is_none())
            {
                return Err(Error::usage(format!(
                    "{}: vidcut doesn't export .{ext} files; name the file .mp4, .mov, .mkv, \
                     .webm, .m4a, .mp3, .flac, .wav or .ogg (or .otio, .otioz or .otiod for a \
                     timeline), or give a preset",
                    params.output.display()
                )));
            }
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
        // The folder is made when the export runs: a plan changes nothing.
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
                settings: settings.clone(),
                color: None,
                metadata: None,
            });
        }
        if outputs.is_empty() {
            return Err(Error::invalid("no clips in the export range"));
        }
    } else {
        outputs.push(Output {
            plan: render::flatten(&proj, a, b)?,
            path: params.output.clone(),
            settings: settings.clone(),
            color: None,
            metadata: None,
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

/// What a media item's pictures are: its HDR transfer and bit depth, as
/// probed when it was added.
fn picture_kind(p: &Project, piece: &render::VideoPiece) -> PictureKind {
    let render::VideoPiece::Source { media, .. } = piece else {
        return PictureKind::default();
    };
    p.media(media)
        .and_then(|m| m.info.video.as_ref())
        .map_or(PictureKind::default(), |v| PictureKind {
            hdr: v.hdr,
            depth: vidcut_av::probe::pixel_depth(&v.pix_fmt),
        })
}

/// What a media item's pictures carry: its transfer and HDR metadata, as
/// probed when it was added (or filled in, [`fill_hdr_metadata`]).
fn piece_meta(p: &Project, piece: &render::VideoPiece) -> PieceMeta {
    let render::VideoPiece::Source { media, .. } = piece else {
        return PieceMeta::default();
    };
    p.media(media)
        .and_then(|m| m.info.video.as_ref())
        .map_or(PieceMeta::default(), |v| PieceMeta {
            hdr: v.hdr,
            mastering: v.mastering,
            light: v.light,
            hdr10plus: v.hdr10plus,
            dolby_vision: v.dolby_vision,
            measured: Measured::No,
        })
}

/// Probes again the HDR media among `ids` that were probed before vidcut
/// kept their metadata: those with none of it.
pub fn fill_hdr_metadata(p: &mut Project, ids: &[String]) {
    for m in &mut p.media {
        let Some(v) = m.info.video.as_mut() else {
            continue;
        };
        let bare = v.hdr.is_some()
            && v.mastering.is_none()
            && v.light.is_none()
            && !v.hdr10plus
            && v.dolby_vision.is_none();
        if !bare || !ids.contains(&m.id) || m.sequence.is_some() {
            continue;
        }
        if let Some(probed) = vidcut_av::probe::probe(&m.path).ok().and_then(|i| i.video) {
            v.mastering = probed.hdr.mastering;
            v.light = probed.hdr.light;
            v.hdr10plus = probed.hdr.hdr10plus;
            v.dolby_vision = probed.hdr.dolby_vision;
        }
    }
}

/// Light measuring for [`choose_colors`] as an export starts: its cancel
/// flag, and what hears the share measured.
pub struct Measuring<'a> {
    pub cancel: &'a AtomicBool,
    pub progress: &'a mut dyn FnMut(f64),
}

/// Chooses each output's dynamic range and bit depth from the pictures it
/// shows, where the settings leave them to the export (decision D44): with
/// the encoders the export may use (`settings` as the preferences allow).
/// Then its HDR metadata (decision D46), with the light measured before;
/// with `measure`, what the metadata needs and isn't measured is measured
/// first: light levels the sources don't say, frames HDR10+ is made for.
pub fn choose_colors(
    prepared: &mut Prepared,
    p: &Project,
    available: &Availability,
    measure: Option<Measuring<'_>>,
) -> Result<()> {
    let todo = choose_once(prepared, p, available, false);
    if let Some(m) = measure
        && !todo.is_empty()
    {
        measure_spans(&todo, m)?;
        choose_once(prepared, p, available, true);
    }
    Ok(())
}

/// [`choose_colors`] with the light measured before; returns what the
/// metadata needs measured and isn't. `measured`: what it needed was just
/// measured, so light levels still unknown couldn't be.
fn choose_once(
    prepared: &mut Prepared,
    p: &Project,
    available: &Availability,
    measured: bool,
) -> Vec<Span> {
    let ids: Vec<String> = prepared
        .outputs
        .iter()
        .flat_map(|o| &o.plan.video)
        .filter_map(|piece| match piece {
            render::VideoPiece::Source { media, .. } => Some(media.clone()),
            _ => None,
        })
        .collect();
    let mut filled = p.clone();
    fill_hdr_metadata(&mut filled, &ids);
    let mut todo: Vec<Span> = Vec::new();
    for o in &mut prepared.outputs {
        o.settings = prepared.settings.clone();
        o.color = vidcut_av::export::choose_color(&o.plan, &o.settings, available, |piece| {
            picture_kind(p, piece)
        });
        if let (Some(c), Some(v)) = (&o.color, o.settings.video.as_mut()) {
            c.apply(v);
        }
        let fps = o.plan.fps;
        let colored = o.settings.video.clone();
        o.metadata = vidcut_av::export::choose_metadata(&o.plan, &o.settings, |piece| {
            let mut m = piece_meta(&filled, piece);
            if let Some(v) = &colored {
                m.measured = engine::measured_light(piece, fps, v, &m, false, None);
                if m.measured == Measured::Pending {
                    if measured {
                        m.measured = Measured::Failed;
                    } else {
                        todo.extend(Span::of(piece, fps));
                    }
                }
            }
            m
        });
        if let (Some(c), Some(v)) = (&o.metadata, o.settings.video.as_mut()) {
            c.apply(v);
        }
        let Some(v) = o.settings.video.as_ref() else {
            continue;
        };
        for piece in &o.plan.video {
            if !engine::makes_hdr10plus(v, &piece_meta(&filled, piece)) {
                continue;
            }
            if let Some(span) = Span::of(piece, fps)
                && !matches!(
                    vidcut_av::light::analysis(&span, false, None, &mut |_| {}),
                    Ok(Some(_))
                )
            {
                todo.push(span);
            }
        }
    }
    let mut unique: Vec<Span> = Vec::new();
    for s in todo {
        if !unique.contains(&s) {
            unique.push(s);
        }
    }
    unique
}

/// Measures `todo`, telling `m` the share done, by length.
fn measure_spans(todo: &[Span], m: Measuring<'_>) -> Result<()> {
    // A still is one frame.
    let weight = |s: &Span| {
        let d = s.to - s.from;
        if d.is_finite() { d.max(0.04) } else { 0.04 }
    };
    let total: f64 = todo.iter().map(weight).sum();
    let mut done = 0.0;
    for s in todo {
        let w = weight(s);
        let measured = vidcut_av::light::analysis(s, true, Some(m.cancel), &mut |share| {
            (m.progress)((done + share * w) / total)
        });
        // What can't be read is left out of the metadata, and said.
        if let Err(e) = measured
            && engine::is_cancelled(&e)
        {
            return Err(Error::new(ErrorCode::Cancelled, "export cancelled"));
        }
        done += w;
        (m.progress)(done / total);
    }
    Ok(())
}

/// Quiet notes about a prepared export: HDR picture tone-mapped into SDR
/// when SDR wasn't asked for (decision D44). Each is `{code, message,
/// set}`, `set` the `--set` overrides that keep the picture HDR, empty
/// when no encoder `available` allows writes that.
pub fn notes(prepared: &Prepared, available: &Availability) -> Vec<Value> {
    let noted: Vec<&ColorChoice> = prepared
        .outputs
        .iter()
        .filter_map(|o| o.color.as_ref())
        .filter(|c| c.worth_a_note())
        .collect();
    let (Some(first), Some(v)) = (noted.first(), prepared.settings.video.as_ref()) else {
        return Vec::new();
    };
    let mut share = PictureShare::default();
    for c in &noted {
        share.add(&c.picture);
    }
    let (set, hdr) = color::keep_hdr_settings(v, share.transfer());
    let set = if vidcut_av::export::encode::abilities(&hdr, available).ten_bit {
        set
    } else {
        Vec::new()
    };
    let fps = prepared.outputs[0].plan.fps.as_f64();
    let seconds = share.hdr() as f64 / fps;
    let length = if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{}:{:02}", (seconds / 60.0) as u64, seconds as u64 % 60)
    };
    // The reason, when every file has the same one.
    let why = if noted.iter().all(|c| c.why[0] == first.why[0]) {
        format!(" ({})", first.why[0].trim_start_matches("SDR: "))
    } else {
        String::new()
    };
    let message = if prepared.outputs.len() == 1 {
        format!("{length} of HDR picture will be tone-mapped to SDR{why}")
    } else {
        format!(
            "HDR picture in {} of {} files ({length} in all) will be tone-mapped to SDR{why}",
            noted.len(),
            prepared.outputs.len()
        )
    };
    vec![json!({ "code": "hdr_tone_mapped", "message": message, "set": set })]
}

/// A metadata choice in words: `HDR10: P3 D65, 0.0001–1000 cd/m², MaxCLL
/// 1000, MaxFALL 400; HDR10+`.
pub fn metadata_words(m: &Value) -> String {
    let mut parts = Vec::new();
    if let Ok(d) =
        serde_json::from_value::<vidcut_core::model::MasteringDisplay>(m["mastering"].clone())
    {
        parts.push(vidcut_core::export::metadata::describe_mastering(&d));
    }
    if m["light"].is_object() {
        parts.push(format!(
            "MaxCLL {}, MaxFALL {}",
            m["light"]["max_cll"], m["light"]["max_fall"]
        ));
    }
    let mut words = if parts.is_empty() {
        String::new()
    } else {
        format!("HDR10: {}", parts.join(", "))
    };
    let mode = m["mode"].as_str().unwrap_or_default();
    for (kept, name) in [
        (matches!(mode, "hdr10plus" | "all"), "HDR10+"),
        (matches!(mode, "dolby_vision" | "all"), "Dolby Vision"),
    ] {
        if kept {
            if !words.is_empty() {
                words += "; ";
            }
            words += name;
        }
    }
    words
}

/// A summary of a prepared export (the `export.plan` result).
pub fn describe(prepared: &Prepared, available: &Availability) -> Value {
    // The encoder of the first file; with `auto` color, files may differ
    // in depth (`outputs[].color`).
    let video = prepared
        .outputs
        .first()
        .map_or(&prepared.settings, |o| &o.settings)
        .video
        .as_ref();
    let video_encoder = video.map(|v| match vidcut_av::export::encode::resolve_video(v, available) {
        Ok(e) => json!({ "name": e.name, "kind": e.kind, "pixel": vidcut_av::export::encode::pixel_name(e.pixel), "device": e.device, "options": e.options, "passes": e.passes }),
        Err(err) => json!({ "error": err.to_string() }),
    });
    json!({
        "preset": prepared.preset,
        "settings": prepared.settings,
        "video_encoder": video_encoder,
        "notes": notes(prepared, available),
        "outputs": prepared.outputs.iter().map(|o| json!({
            "path": o.path,
            "frames": o.plan.frames,
            "range": o.plan.range,
            "video_pieces": o.plan.video.len(),
            "audio_lanes": o.plan.audio.len(),
            "color": o.color,
            "metadata": o.metadata,
            "plan": o.plan,
        })).collect::<Vec<_>>(),
    })
}

/// Encoder availability, from the cache when it matches this system.
pub fn availability(refresh: bool) -> Availability {
    // FFmpeg, the system, the GPUs' drivers and what is tested: a change of
    // any of them tests the encoders again.
    let key = format!(
        "{:?}|{}|{}|{}",
        vidcut_av::library_versions(),
        crate::system::version(),
        Value::from(crate::system::gpus()),
        vidcut_av::export::encode::DETECTS
    );
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

/// The settings as the preferences allow: without GPU acceleration every
/// video encoder choice is the CPU's, so a preset naming NVENC still
/// exports, in software.
pub fn as_preferred(settings: &mut ExportSettings, gpu: bool) {
    if !gpu && let Some(v) = settings.video.as_mut() {
        v.encoder = EncoderChoice::Cpu;
    }
}

/// Whether the settings may pick a GPU encoder (so availability matters).
pub fn needs_gpu_info(settings: &ExportSettings) -> bool {
    settings
        .video
        .as_ref()
        .is_some_and(|v| v.encoder != EncoderChoice::Cpu)
}

/// Runs a prepared export, one output after another, its pieces decoding
/// as `decoding` says (export.md §5).
pub fn run(
    prepared: &Prepared,
    available: &Availability,
    decoding: vidcut_av::hw::Decoding<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, &Path, Progress),
) -> Result<Vec<Report>> {
    let mut reports = Vec::new();
    for (i, o) in prepared.outputs.iter().enumerate() {
        if let Some(dir) = o.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::usage(format!("{}: {e}", dir.display())))?;
        }
        let report = engine::run_decoding_on(
            &o.plan,
            &o.settings,
            &o.path,
            available,
            decoding,
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
        reports.push(Report {
            color: report.color.or_else(|| o.color.clone()),
            metadata: report.metadata.or_else(|| o.metadata.clone()),
            ..report
        });
    }
    Ok(reports)
}

/// A still render's failure. The cancel flag is the export's, so a client
/// that left is `CANCELLED`.
fn from_av(e: vidcut_av::error::Error) -> Error {
    if vidcut_av::export::is_cancelled(&e) {
        Error::new(ErrorCode::Cancelled, "cancelled")
    } else {
        Error::new(ErrorCode::Export, e.to_string())
    }
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
                drop_frame: p.settings.counts_drop_frame(),
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
            (render::media_frame(p, id, t)?, v.display_size(), t)
        }
    };
    let (w, h) = match width {
        Some(nw) => (
            nw & !1,
            ((f64::from(nw) * f64::from(h) / f64::from(w)).round() as u32).max(2) & !1,
        ),
        None => (w, h),
    };
    vidcut_av::export::still::write_png(&piece, fps, w.max(2), h.max(2), out).map_err(from_av)?;
    Ok(json!({ "path": out, "frame": frame, "width": w, "height": h }))
}
