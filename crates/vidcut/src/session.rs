//! Runs requests: headless against a project file (M1), live in the GUI (M3).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use vidcut_av::export::Progress;
use vidcut_core::store::{Store, state_dir};
use vidcut_core::{Editor, Envelope, Error, ErrorCode, Project, Request, Response, Result};

use crate::env::AvEnv;
use crate::export;

/// Cancellation and progress for long-running requests (exports).
pub struct Hooks<'a> {
    pub cancel: &'a Arc<AtomicBool>,
    /// Called as `(output index, output count, output path, progress)`.
    pub progress: &'a mut dyn FnMut(usize, usize, &Path, Progress),
}

/// Runs one request headless with no progress reporting.
pub fn run(project: Option<&Path>, envelope: Envelope) -> Result<Response> {
    let cancel = Arc::new(AtomicBool::new(false));
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
        Request::ExportPlan(_)
        | Request::ExportStart(_)
        | Request::FrameRender { .. }
        | Request::StoryboardRender { .. }
        | Request::AnalyzeSilence { .. }
        | Request::AnalyzeScenes { .. }
        | Request::AnalyzeBlack { .. }
        | Request::AnalyzeLoudness { .. } => {
            let store = open(project)?;
            let editor = store.load()?;
            envelope.check_rev(editor.project().rev)?;
            render(editor.project(), store.path(), &envelope, hooks)?
        }
        Request::Probe { .. } => {
            // Probing needs no project; an empty editor answers it.
            let mut editor = vidcut_core::Editor::new(
                vidcut_core::Project::new(Default::default()),
                Default::default(),
            );
            editor.apply(envelope, &AvEnv::new(), None)?
        }
        _ => {
            let store = open(project)?;
            let env = AvEnv::for_project(store.path());
            if envelope.request.is_mutation() {
                let (mut r, history_error) =
                    store.transact(|editor| editor.apply(envelope, &env, None))?;
                if let Some(e) = history_error {
                    r.warnings.push(history_unsaved(&e));
                }
                r
            } else {
                store.load()?.apply(envelope, &env, None)?
            }
        }
    };
    response.via = "headless".into();
    Ok(response)
}

/// Runs a request that reads `project` (saved at `path`) without changing
/// it, against the project as given: exports, export plans, frames,
/// storyboards and measurements. The caller has checked `if_rev`.
pub fn render(
    project: &Project,
    path: &Path,
    envelope: &Envelope,
    hooks: &mut Hooks<'_>,
) -> Result<Response> {
    // Publishing the flag lets decoders on this thread (and storyboard
    // workers, which copy it) stop when the client leaves, the way the
    // export frame loop loads the same flag.
    let _guard = vidcut_av::cancel::Guard::set(hooks.cancel.clone());
    if watches_cancel(&envelope.request) && hooks.cancel.load(Ordering::Relaxed) {
        return Err(Error::new(ErrorCode::Cancelled, "cancelled"));
    }
    let mut r = match &envelope.request {
        Request::ExportPlan(params) | Request::ExportStart(params)
            if export::is_otio(&params.output) =>
        {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let write = matches!(envelope.request, Request::ExportStart(_));
            answer(export::otio(project, &name, params, None, write)?)
        }
        Request::ExportPlan(params) => {
            let prepared = export::prepare(project, params, None)?;
            let available = if export::needs_gpu_info(&prepared.settings) {
                export::availability(false)
            } else {
                Default::default()
            };
            answer(export::describe(&prepared, &available))
        }
        Request::ExportStart(params) => export_media(project, params, hooks, &mut |_| {})?,
        Request::FrameRender {
            at,
            out,
            media,
            width,
        } => answer(export::frame(
            project,
            at,
            out,
            media.as_deref(),
            *width,
            None,
        )?),
        Request::StoryboardRender {
            out,
            columns,
            width,
            every,
            range,
        } => answer(crate::storyboard::render(
            project,
            out,
            *columns,
            *width,
            every.as_ref(),
            range.as_ref(),
            None,
        )?),
        // Measurements: queries, on a copy. Keep `if_rev`. A mutation that
        // lands here later must not apply on this throwaway editor and come
        // back as success: the copy is discarded, and `r.rev` below would
        // still report the snapshot revision.
        _ => {
            if envelope.request.is_mutation() {
                return Err(Error::internal(format!(
                    "{} changes the project; it cannot run on the render copy",
                    envelope.request.method()
                )));
            }
            Editor::new(project.clone(), Default::default()).apply(
                envelope.clone(),
                &AvEnv::for_project(path),
                None,
            )?
        }
    };
    r.rev = project.rev;
    Ok(r)
}

/// Exports media: prepares the export, tells `started` the files it will
/// write and their lengths in frames, then encodes them.
pub fn export_media(
    p: &Project,
    params: &vidcut_core::request::ExportParams,
    hooks: &mut Hooks<'_>,
    started: &mut dyn FnMut(Vec<(std::path::PathBuf, i64)>),
) -> Result<Response> {
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

/// Says that the undo history wasn't saved with an edit, and what follows.
pub fn history_unsaved(error: &str) -> String {
    format!(
        "the edit is saved, but its undo history isn't ({error}): other vidcut runs see \
         the edit as an outside change, or start with no history"
    )
}

/// Work that keeps running after the client is gone unless it loads the
/// cancel flag: it reports no progress, so a failed progress write never
/// happens. A foreground export watches the same flag in its frame loop.
fn watches_cancel(request: &Request) -> bool {
    matches!(
        request,
        Request::FrameRender { .. }
            | Request::StoryboardRender { .. }
            | Request::AnalyzeSilence { .. }
            | Request::AnalyzeScenes { .. }
            | Request::AnalyzeBlack { .. }
            | Request::AnalyzeLoudness { .. }
    )
}

fn answer(result: Value) -> Response {
    Response {
        rev: 0,
        changes: Default::default(),
        result,
        dry_run: false,
        via: String::new(),
        warnings: Vec::new(),
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use serde_json::json;
    use vidcut_core::model::Settings;
    use vidcut_core::{Envelope, ErrorCode, Project};

    use super::{Hooks, render};

    fn render_req(req: serde_json::Value) -> vidcut_core::Result<vidcut_core::Response> {
        let project = Project::new(Settings::default());
        let envelope = Envelope::from_json(req).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        render(
            &project,
            Path::new("p.vidcut"),
            &envelope,
            &mut Hooks {
                cancel: &cancel,
                progress: &mut |_, _, _, _| {},
            },
        )
    }

    #[test]
    fn render_keeps_if_rev_and_refuses_mutations() {
        let stale = render_req(json!({ "method": "timeline.get", "if_rev": 1 })).unwrap_err();
        assert_eq!(stale.code, ErrorCode::Conflict);

        let ok = render_req(json!({ "method": "timeline.get", "if_rev": 0 })).unwrap();
        assert_eq!(ok.rev, 0);

        // A matching `if_rev` would pass the revision check and then apply
        // on the throwaway editor. That must still fail.
        for req in [
            json!({ "method": "project.set", "params": { "ripple_on": false }, "if_rev": 0 }),
            json!({ "method": "project.set", "params": { "ripple_on": false } }),
        ] {
            let err = render_req(req).unwrap_err();
            assert_eq!(err.code, ErrorCode::Internal);
            assert!(err.message.contains("project.set"), "{err}");
        }
    }

    #[test]
    fn a_set_cancel_flag_stops_frames_storyboards_and_analysis() {
        let project = Project::new(Settings::default());
        let path = Path::new("not-saved.vidcut");
        let cancel = Arc::new(AtomicBool::new(true));
        let calls = [
            ("frame.render", json!({ "at": "0f", "out": "f.png" })),
            ("storyboard.render", json!({ "out": "board.png" })),
            ("analyze.silence", json!({})),
            ("analyze.scenes", json!({})),
            ("analyze.black", json!({})),
            ("analyze.loudness", json!({})),
        ];
        for (method, params) in calls {
            let envelope =
                Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
            let err = render(
                &project,
                path,
                &envelope,
                &mut Hooks {
                    cancel: &cancel,
                    progress: &mut |_, _, _, _| {},
                },
            )
            .unwrap_err();
            assert_eq!(err.code, ErrorCode::Cancelled, "{method}: {err}");
        }
        // A clear flag is not a cancellation: an empty timeline has nothing
        // to measure, and that is the error.
        cancel.store(false, Ordering::Relaxed);
        let envelope =
            Envelope::from_json(json!({ "method": "analyze.loudness", "params": {} })).unwrap();
        let err = render(
            &project,
            path,
            &envelope,
            &mut Hooks {
                cancel: &cancel,
                progress: &mut |_, _, _, _| {},
            },
        )
        .unwrap_err();
        assert_ne!(err.code, ErrorCode::Cancelled, "{err}");
    }
}
