use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use vidcut_av::export::Progress;
use vidcut_core::looks;
use vidcut_core::model::Format;
use vidcut_core::store::{Store, state_dir};
use vidcut_core::{Editor, Envelope, Error, ErrorCode, Project, Request, Response, Result};

use crate::env::AvEnv;
use crate::threads::Work;

fn shortcuts() -> Value {
    use vidcut_core::shortcuts::{SHORTCUTS, key_name};
    let mac = cfg!(target_os = "macos");
    let list: Vec<Value> = SHORTCUTS
        .iter()
        .map(|s| {
            json!({
                "group": s.group,
                "group_title": s.group.title(),
                "action": s.action,
                "keys": s.keys.iter().map(|k| key_name(k, mac)).collect::<Vec<_>>(),
                "explanation": s.explanation.map(crate::gui::keys),
            })
        })
        .collect();
    json!({ "shortcuts": list })
}
use crate::export;

fn list_looks(kind: Option<looks::LookKind>) -> Value {
    let (mut found, mut refused) = (Vec::new(), Vec::new());
    for k in kind.map_or(looks::LookKind::ALL.to_vec(), |k| vec![k]) {
        let listing = looks::all(&AvEnv::new(), k);
        found.extend(listing.looks.iter().map(looks::Look::json));
        refused.extend(
            listing
                .refused
                .iter()
                .map(|r| json!({ "kind": k, "name": r.name, "path": r.path, "why": r.why })),
        );
    }
    json!({ "looks": found, "refused": refused })
}

pub struct Hooks<'a> {
    pub cancel: &'a Arc<AtomicBool>,
    pub progress: &'a mut dyn FnMut(usize, usize, &Path, Progress),
}

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

pub fn run_with(
    project: Option<&Path>,
    envelope: Envelope,
    hooks: &mut Hooks<'_>,
) -> Result<Response> {
    let envelope = envelope.normalized();
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
    let dry_run = envelope.dry_run;
    let mut response = match &envelope.request {
        Request::Schema { method } => answer(schema(method.as_deref())?),
        Request::Doctor {} => answer(doctor()),
        Request::Reset {
            saved,
            keep_downloads,
        } => answer(crate::reset::run(
            *saved,
            *keep_downloads,
            envelope.dry_run,
        )?),
        Request::ProjectCreate {
            path,
            from,
            fps,
            width,
            height,
            sample_rate,
            channels,
            drop_frame,
            start_tc,
        } => {
            let format = Format {
                fps: fps.clone(),
                size: size(*width, *height)?,
                drop_frame: *drop_frame,
                start_tc: start_tc.clone(),
                sample_rate: *sample_rate,
                channels: *channels,
            };
            if let Some(from) = from {
                let (project, path, warnings) =
                    create_from_otio(path, from, &format, envelope.dry_run)?;
                answer(json!({
                    "project": path,
                    "settings": project.settings,
                    "warnings": warnings,
                }))
            } else {
                let settings = format.settings()?;
                let path = if envelope.dry_run {
                    path.clone()
                } else {
                    Store::create(path, &vidcut_core::Project::new(settings.clone()))?
                        .path()
                        .to_path_buf()
                };
                answer(json!({ "project": path, "settings": settings }))
            }
        }
        Request::PresetsList {} => {
            let (available, gpu) = (
                export::availability(false),
                crate::preferences::gpu_acceleration(),
            );
            answer(json!(
                export::presets()?
                    .iter()
                    .map(|p| json!({
                        "unavailable": export::unavailable(&p.settings, &available, gpu),
                        "name": p.name,
                        "title": p.title,
                        "description": p.description,
                        "quick_export": p.quick_export,
                        "container": p.settings.container,
                        "user": export::is_user_preset(&p.name),
                        "settings": p.settings,
                    }))
                    .collect::<Vec<_>>()
            ))
        }
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
        Request::LooksList { kind } => answer(list_looks(*kind)),
        Request::LooksSave {
            name,
            subtitles,
            titles,
            overwrite,
        } => {
            let styled = match (subtitles, titles) {
                (Some(s), None) => looks::Styled::Subtitles(s.clone()),
                (None, Some(t)) => looks::Styled::Titles(t.clone()),
                _ => {
                    return Err(Error::usage(
                        "looks.save needs exactly one of `subtitles` and `titles`",
                    ));
                }
            };
            let (look, path) =
                looks::save(&looks::dir(), name, &styled, *overwrite, envelope.dry_run)?;
            answer(json!({ "look": look.json(), "path": path }))
        }
        Request::LooksDelete { kind, name } => {
            let path = looks::delete(&looks::dir(), *kind, name, envelope.dry_run)?;
            answer(json!({ "deleted": name.trim(), "kind": kind, "path": path }))
        }
        Request::EncodersList { refresh } => {
            let detected = export::detected(*refresh);
            let mut answer_json = json!(detected);
            let can = export::encodable(&detected.in_use(), crate::preferences::gpu_acceleration());
            answer_json["video"] = can["video"].clone();
            answer_json["audio"] = can["audio"].clone();
            answer(answer_json)
        }
        Request::DecodersList {} => {
            let clips = vidcut_core::disk_cache::Kind::DecoderTests.dir();
            answer(json!({ "decoders": vidcut_av::hw::decoders(&clips) }))
        }
        Request::PreferencesGet {} => answer(crate::preferences::get()?),
        Request::KeysList {} => answer(shortcuts()),
        Request::ServicesSetKey { service, key } => answer(crate::services::set_key(
            *service,
            key.as_deref(),
            envelope.dry_run,
        )?),
        Request::ServicesRemoveKey { service } => {
            answer(crate::services::remove_key(*service, envelope.dry_run)?)
        }
        Request::PreferencesSet(patch) => answer(crate::preferences::set(patch, envelope.dry_run)?),
        Request::PreferencesReset {} => answer(crate::preferences::reset(envelope.dry_run)?),
        Request::ViewerGuides {
            enabled,
            framing,
            extents,
            action,
            title,
            center,
        } => {
            let patch = vidcut_core::preferences::GuidesPatch {
                enabled: *enabled,
                framing: *framing,
                extents: *extents,
                action: *action,
                title: *title,
                center: *center,
            };
            answer(crate::preferences::set_guides(patch, envelope.dry_run)?)
        }
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
            let editor = store.load_project()?;
            envelope.check_rev(editor.project().rev)?;
            render(editor.project(), store.path(), &envelope, hooks)?
        }
        Request::Probe { .. } => {
            let mut editor = vidcut_core::Editor::new(
                vidcut_core::Project::new(Default::default()),
                Default::default(),
            );
            editor.apply(envelope, &AvEnv::new(), None)?
        }
        _ => {
            let store = open(project)?;
            let env = AvEnv::for_project(store.path());
            match envelope.request {
                Request::ProxyGenerate { .. } if !envelope.dry_run => {
                    work_first(&store, &envelope, &env, hooks, "vidcut-proxies")?;
                }
                Request::SubtitlesTranscribe { .. }
                | Request::AnalyzeWords { .. }
                | Request::MediaTranscribe { .. } => {
                    work_first(&store, &envelope, &env, hooks, "vidcut-transcribe")?;
                }
                Request::SubtitlesRetime { .. } => {
                    work_first(&store, &envelope, &env, hooks, "vidcut-retime")?;
                }
                Request::SubtitlesTimeWords { .. } => {
                    work_first(&store, &envelope, &env, hooks, "vidcut-transcribe")?;
                }
                _ => {}
            }
            if envelope.request.is_mutation() {
                let (mut r, history_error) =
                    store.transact(|editor| editor.apply(envelope, &env, None))?;
                if let Some(e) = history_error {
                    r.warnings.push(history_unsaved(&e));
                }
                r
            } else {
                store.load_project()?.apply(envelope, &env, None)?
            }
        }
    };
    response.via = "headless".into();
    response.dry_run |= dry_run;
    Ok(response)
}

fn work_first(
    store: &Store,
    envelope: &Envelope,
    env: &AvEnv,
    hooks: &mut Hooks<'_>,
    thread: &str,
) -> Result<()> {
    let mut copy = store.load_project()?;
    let (tx, rx) = std::sync::mpsc::channel::<(std::path::PathBuf, Progress)>();
    let cancel = hooks.cancel.clone();
    std::thread::scope(|scope| {
        let work = crate::threads::scoped(scope, thread, Work::current(), move || {
            let _cancel = vidcut_av::cancel::Guard::set(cancel);
            let _report = vidcut_av::progress::Guard::set(Box::new(move |out: &Path, p| {
                let _ = tx.send((out.to_path_buf(), p));
            }));
            copy.apply(envelope.clone(), env, None).map(drop)
        })
        .map_err(|e| Error::internal(format!("starting {thread}: {e}")))?;
        for (out, p) in rx {
            (hooks.progress)(0, 1, &out, p);
        }
        work.join()
            .unwrap_or_else(|_| Err(Error::internal(format!("{thread} panicked"))))
    })
}

pub fn render(
    project: &Project,
    path: &Path,
    envelope: &Envelope,
    hooks: &mut Hooks<'_>,
) -> Result<Response> {
    let _guard = vidcut_av::cancel::Guard::set(hooks.cancel.clone());
    if watches_cancel(&envelope.request) && hooks.cancel.load(Ordering::Relaxed) {
        return Err(Error::new(ErrorCode::Cancelled, "cancelled"));
    }
    let mut r = match &envelope.request {
        Request::ExportPlan(params) if export::timeline_format(&params.output).is_some() => {
            let name = export::timeline_name(path);
            answer(export::timeline(project, &name, params, None, None)?)
        }
        Request::ExportStart(params) if export::timeline_format(&params.output).is_some() => {
            export_media(project, path, params, hooks, &mut |_| Ok(()))?
        }
        Request::ExportPlan(params) => {
            let mut prepared = export::prepare(project, params, None)?;
            let gpu = crate::preferences::gpu_acceleration();
            export::as_preferred(&mut prepared, gpu);
            let available = match gpu && prepared.settings.video.is_some() {
                true => export::availability(false),
                false => Default::default(),
            };
            export::choose_colors(&mut prepared, project, &available, None)?;
            let subtitles = export::subtitles_beside(project, &prepared, params)?;
            let mut plan = export::describe(&prepared, &available);
            plan["auto"] = export::auto_picks(&prepared, project, &available, gpu);
            if !subtitles.is_empty() {
                plan["subtitles"] = export::subtitle_files(&subtitles);
            }
            if let Some(notes) = plan["notes"].as_array_mut() {
                notes.extend(export::subtitle_notes(&subtitles));
            }
            answer(plan)
        }
        Request::ExportStart(params) => {
            export_media(project, path, params, hooks, &mut |_| Ok(()))?
        }
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

pub fn export_media(
    p: &Project,
    path: &Path,
    params: &vidcut_core::request::ExportParams,
    hooks: &mut Hooks<'_>,
    started: &mut dyn FnMut(Vec<(std::path::PathBuf, i64)>) -> Result<()>,
) -> Result<Response> {
    if export::timeline_format(&params.output).is_some() {
        let name = export::timeline_name(path);
        let plan = export::timeline(p, &name, params, None, None)?;
        let frames = plan["outputs"][0]["frames"].as_i64().unwrap_or(0);
        started(vec![(params.output.clone(), frames)])?;
        let clock = std::time::Instant::now();
        let output = params.output.clone();
        let progress = &mut *hooks.progress;
        let mut bytes = |done: u64, total: u64| {
            let share = if total == 0 {
                1.0
            } else {
                done as f64 / total as f64
            };
            progress(
                0,
                1,
                &output,
                Progress::single((frames as f64 * share) as i64, frames, clock.elapsed()),
            );
        };
        let written = export::timeline(
            p,
            &name,
            params,
            None,
            Some(export::Writing {
                cancel: hooks.cancel,
                progress: &mut bytes,
            }),
        )?;
        let mut r = answer(written);
        r.rev = p.rev;
        return Ok(r);
    }
    let mut prepared = export::prepare(p, params, None)?;
    let subtitles = export::subtitles_beside(p, &prepared, params)?;
    export::refuse_existing(&subtitles, params.overwrite)?;
    if prepared.settings.audio.is_some()
        && prepared
            .outputs
            .iter()
            .any(|o| vidcut_av::isolate::wanted(&o.plan.audio))
    {
        vidcut_av::isolate::get_ready(&crate::paths::models_dir(), params.download)
            .map_err(|e| export::av_error(&e, ErrorCode::Export, String::new()))?;
    }
    vidcut_av::ai::check(
        p,
        prepared.outputs.iter().map(|o| &o.plan),
        &crate::paths::models_dir(),
        params.download,
    )
    .map_err(|e| export::av_error(&e, ErrorCode::Export, String::new()))?;
    if prepared.settings.audio.is_some() {
        let env = AvEnv::for_project(path);
        for o in &mut prepared.outputs {
            vidcut_core::duck::hear_with(&mut o.plan, p, &env, params.download)?;
        }
    }
    export::as_preferred(&mut prepared, crate::preferences::gpu_acceleration());
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
    )?;
    let count = prepared.outputs.len();
    let clock = std::time::Instant::now();
    let first = prepared.outputs[0].path.clone();
    let progress = &mut *hooks.progress;
    let decoder = crate::preferences::gpu_decoding().own_decoder();
    export::choose_colors(
        &mut prepared,
        p,
        &available,
        Some(export::Measuring {
            decoder: decoder.as_ref(),
            cancel: hooks.cancel,
            progress: &mut |share| {
                progress(
                    0,
                    count,
                    &first,
                    Progress::measuring(share, clock.elapsed()),
                )
            },
        }),
    )?;
    let upscaled = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reports = {
        let (kept, before) = (upscaled.clone(), vidcut_av::progress::notes());
        let _noting = vidcut_av::progress::Noting::set(Arc::new(move |note| {
            if let vidcut_av::export::Note::Upscaled(u) = &note
                && let Ok(mut kept) = kept.lock()
            {
                kept.push(u.clone());
            }
            if let Some(before) = &before {
                before(note);
            }
        }));
        export::run(
            &prepared,
            &available,
            decoder.as_ref(),
            hooks.cancel,
            &mut |i, path, pr| (hooks.progress)(i, count, path, pr),
        )?
    };
    export::write_subtitles(&subtitles)?;
    let mut notes = export::notes(&prepared, &available);
    notes.extend(export::subtitle_notes(&subtitles));
    let upscaled = upscaled.lock().map(|u| u.clone()).unwrap_or_default();
    let mut r = answer(json!({
        "outputs": reports,
        "upscaled": upscaled,
        "notes": notes,
        "subtitles": export::subtitle_files(&subtitles),
    }));
    r.rev = p.rev;
    Ok(r)
}

pub fn history_unsaved(error: &str) -> String {
    format!(
        "the edit is saved, but its undo history isn't ({error}). The edits before it may \
         not be undoable"
    )
}

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

pub fn create_from_otio(
    path: &Path,
    from: &Path,
    format: &Format,
    dry_run: bool,
) -> Result<(Project, PathBuf, Vec<String>)> {
    if path.exists() {
        return Err(Error::invalid(format!("{} already exists", path.display())));
    }
    let env = AvEnv::for_project(path);
    let (project, warnings) = vidcut_core::editor::project_from_otio(from, format, !dry_run, &env)?;
    let path = if dry_run {
        path.to_path_buf()
    } else {
        Store::create(path, &project)?.path().to_path_buf()
    };
    Ok((project, path, warnings))
}

pub fn size(width: Option<u32>, height: Option<u32>) -> Result<Option<(u32, u32)>> {
    match (width, height) {
        (Some(w), Some(h)) => Ok(Some((w, h))),
        (None, None) => Ok(None),
        _ => Err(Error::usage("give both width and height, or neither")),
    }
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

pub fn schema(method: Option<&str>) -> Result<Value> {
    let root = vidcut_core::request::protocol_schema();
    let Some(method) = method else {
        return Ok(root.clone());
    };
    let variant = root
        .get("oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|v| vidcut_core::request::method_names(v).contains(&method))
        .ok_or_else(|| Error::not_found(format!("unknown method {method:?}")))?;
    Ok(json!({
        "method": method,
        "description": variant.get("description"),
        "params": variant["properties"].get("params"),
        "$defs": root.get("$defs"),
    }))
}

fn display_gpus() -> Vec<String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::PRIMARY))
        .iter()
        .map(|a| {
            let info = a.get_info();
            match info.driver.as_str() {
                "" => format!("{} ({:?})", info.name, info.backend),
                driver => format!("{} ({:?}, {driver})", info.name, info.backend),
            }
        })
        .collect()
}

pub fn doctor() -> Value {
    json!({
        "vidcut": env!("CARGO_PKG_VERSION"),
        "system": crate::system::version(),
        "ffmpeg": vidcut_av::library_versions(),
        "ffmpeg_missing": vidcut_av::parts::missing(),
        "ffmpeg_stand_ins": vidcut_av::parts::stand_ins().iter().map(|(p, name)| json!({
            "used_for": p.used_for, "name": name, "instead_of": p.names[0].name,
        })).collect::<Vec<_>>(),
        "audio_output": match vidcut_av::player::output_device() { Ok(d) => json!(d), Err(e) => json!(e.to_string()) },
        "audio_outputs": match vidcut_av::player::output_devices() { Ok(d) => json!(d), Err(e) => json!(e.to_string()) },
        "gpus": crate::system::gpus(),
        "display_gpus": display_gpus(),
        "state_dir": state_dir(),
        "test_data": vidcut_av::testdata::root(),
    })
}
