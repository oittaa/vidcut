//! Runs requests: headless against a project file (M1), live in the GUI (M3).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};
use vidcut_av::export::Progress;
use vidcut_core::store::{Store, state_dir};
use vidcut_core::{Envelope, Error, Request, Response, Result};

use crate::env::AvEnv;
use crate::export;

/// Cancellation and progress for long-running requests (exports).
pub struct Hooks<'a> {
    pub cancel: &'a AtomicBool,
    /// Called as `(output index, output count, output path, progress)`.
    pub progress: &'a mut dyn FnMut(usize, usize, &Path, Progress),
}

/// Runs one request headless with no progress reporting.
pub fn run(project: Option<&Path>, envelope: Envelope) -> Result<Response> {
    let cancel = AtomicBool::new(false);
    run_with(
        project,
        envelope,
        &mut Hooks {
            cancel: &cancel,
            progress: &mut |_, _, _, _| {},
        },
    )
}

/// Runs one request headless. Project-less requests (`schema`, `doctor`,
/// `probe`, `presets.*`, `encoders.list`, `preferences.*`) ignore `project`.
pub fn run_with(
    project: Option<&Path>,
    envelope: Envelope,
    hooks: &mut Hooks<'_>,
) -> Result<Response> {
    let open = |project: Option<&Path>| -> Result<Store> {
        let path = project
            .ok_or_else(|| Error::usage("no project: pass -p FILE or set VIDCUT_PROJECT"))?;
        Store::open(path)
    };
    if envelope.request.is_live() {
        return Err(Error::no_instance(format!(
            "{} controls a running vidcut GUI, and none is reachable; start one with \
             `vidcut -p FILE` (the CLI forwards to it automatically)",
            envelope.request.method()
        )));
    }
    let mut response = match &envelope.request {
        Request::Schema { method } => answer(schema(method.as_deref())?),
        Request::Doctor {} => answer(doctor()),
        Request::PresetsList {} => answer(json!(
            export::presets()?
                .iter()
                .map(|p| json!({
                    "name": p.name,
                    "title": p.title,
                    "description": p.description,
                    "quick_export": p.quick_export,
                    "container": p.settings.container,
                    "user": export::is_user_preset(&p.name),
                    "settings": p.settings,
                }))
                .collect::<Vec<_>>()
        )),
        Request::PresetsGet { name } => {
            let p = export::presets()?
                .into_iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| Error::not_found(format!("no preset {name:?}")))?;
            answer(serde_json::to_value(p).map_err(|e| Error::internal(e.to_string()))?)
        }
        Request::PresetsSave {
            name,
            title,
            description,
            quick_export,
            settings,
            from,
            set,
            overwrite,
        } => answer(export::save_preset(&export::PresetSave {
            name,
            title: title.as_deref(),
            description: description.as_deref(),
            quick_export: *quick_export,
            settings: settings.as_ref(),
            from: from.as_deref(),
            set,
            overwrite: *overwrite,
            dry_run: envelope.dry_run,
        })?),
        Request::PresetsDelete { name } => answer(export::delete_preset(name, envelope.dry_run)?),
        Request::EncodersList { refresh } => answer(json!(export::availability(*refresh))),
        Request::PreferencesGet {} => answer(crate::preferences::get()?),
        Request::PreferencesSet(patch) => answer(crate::preferences::set(patch, envelope.dry_run)?),
        Request::ExportStart(params) if params.background => {
            return Err(Error::no_instance(
                "background exports run in a vidcut GUI (they would stop with this \
                 process); start one with `vidcut -p FILE`, or export without background",
            ));
        }
        Request::ExportPlan(params) | Request::ExportStart(params)
            if export::is_otio(&params.output) =>
        {
            let store = open(project)?;
            let name = store
                .path()
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let editor = store.load()?;
            let write = matches!(envelope.request, Request::ExportStart(_));
            let mut r = answer(export::otio(editor.project(), &name, params, None, write)?);
            r.rev = editor.project().rev;
            r
        }
        Request::ExportPlan(params) => {
            let editor = open(project)?.load()?;
            let p = editor.project();
            let prepared = export::prepare(p, params, None)?;
            let available = if export::needs_gpu_info(&prepared.settings) {
                export::availability(false)
            } else {
                Default::default()
            };
            let mut r = answer(export::describe(&prepared, &available));
            r.rev = p.rev;
            r
        }
        Request::ExportStart(params) => export_media(&open(project)?, params, hooks, &mut |_| {})?,
        Request::FrameRender {
            at,
            out,
            media,
            width,
        } => {
            let editor = open(project)?.load()?;
            let mut r = answer(export::frame(
                editor.project(),
                at,
                out,
                media.as_deref(),
                *width,
                None,
            )?);
            r.rev = editor.project().rev;
            r
        }
        Request::StoryboardRender {
            out,
            columns,
            width,
            every,
            range,
        } => {
            let editor = open(project)?.load()?;
            let mut r = answer(crate::storyboard::render(
                editor.project(),
                out,
                *columns,
                *width,
                every.as_ref(),
                range.as_ref(),
                None,
            )?);
            r.rev = editor.project().rev;
            r
        }
        Request::Probe { .. } => {
            // Probing needs no project; an empty editor answers it.
            let mut editor = vidcut_core::Editor::new(
                vidcut_core::Project::new(Default::default()),
                Default::default(),
            );
            editor.apply(envelope, &AvEnv, None)?
        }
        _ => {
            let store = open(project)?;
            if envelope.request.is_mutation() {
                store.transact(|editor| editor.apply(envelope, &AvEnv, None))?
            } else {
                store.load()?.apply(envelope, &AvEnv, None)?
            }
        }
    };
    response.via = "headless".into();
    Ok(response)
}

/// Exports media: prepares the export, tells `started` the files it will
/// write and their lengths in frames, then encodes them.
pub fn export_media(
    store: &Store,
    params: &vidcut_core::request::ExportParams,
    hooks: &mut Hooks<'_>,
    started: &mut dyn FnMut(Vec<(std::path::PathBuf, i64)>),
) -> Result<Response> {
    let editor = store.load()?;
    let p = editor.project();
    let prepared = export::prepare(p, params, None)?;
    let available = if export::needs_gpu_info(&prepared.settings) {
        export::availability(false)
    } else {
        Default::default()
    };
    started(
        prepared
            .outputs
            .iter()
            .map(|o| (o.path.clone(), o.plan.frames))
            .collect(),
    );
    let count = prepared.outputs.len();
    let reports = export::run(&prepared, &available, hooks.cancel, &mut |i, path, pr| {
        (hooks.progress)(i, count, path, pr)
    })?;
    let mut r = answer(json!({ "outputs": reports }));
    r.rev = p.rev;
    Ok(r)
}

fn answer(result: Value) -> Response {
    Response {
        rev: 0,
        changes: Default::default(),
        result,
        dry_run: false,
        via: String::new(),
    }
}

/// JSON Schema of the whole protocol, or of one method's params.
pub fn schema(method: Option<&str>) -> Result<Value> {
    let root = serde_json::to_value(schemars::schema_for!(Request))
        .map_err(|e| Error::internal(e.to_string()))?;
    let Some(method) = method else {
        return Ok(root);
    };
    let variants = root
        .get("oneOf")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let names = |v: &Value| -> Vec<String> {
        let m = &v["properties"]["method"];
        m.get("const")
            .and_then(Value::as_str)
            .map(|s| vec![s.to_string()])
            .or_else(|| {
                m.get("enum").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default()
    };
    let variant = variants
        .iter()
        .find(|v| names(v).iter().any(|n| n == method))
        .ok_or_else(|| Error::not_found(format!("unknown method {method:?}")))?;
    Ok(json!({
        "method": method,
        "description": variant.get("description"),
        "params": variant["properties"].get("params"),
        "$defs": root.get("$defs"),
    }))
}

/// Dependencies and environment, for humans and agents debugging setups.
pub fn doctor() -> Value {
    let gpus: Vec<Value> = std::fs::read_dir("/sys/class/drm")
        .map(|dir| {
            let mut nodes: Vec<Value> = dir
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
                .map(|e| {
                    let driver = std::fs::read_link(e.path().join("device/driver"))
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
                    json!({ "node": format!("/dev/dri/{}", e.file_name().to_string_lossy()), "driver": driver })
                })
                .collect();
            nodes.sort_by_key(|v| v["node"].as_str().unwrap_or_default().to_string());
            nodes
        })
        .unwrap_or_default();
    json!({
        "vidcut": env!("CARGO_PKG_VERSION"),
        "ffmpeg": vidcut_av::library_versions(),
        "libmpv": match vidcut_mpv::available() { Ok(()) => json!("ok"), Err(e) => json!(e) },
        "gpus": gpus,
        "state_dir": state_dir(),
        "test_data": vidcut_av::testdata::root(),
    })
}
