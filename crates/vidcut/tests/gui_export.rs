//! The export windows under simulated input (egui_kittest, no window):
//! Quick Export, the Export dialog, progress and jobs, timeline interchange.
//! The dialog's command makes the same request, and so the
//! same plan, as the GUI.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};
use vidcut::cli::{Action, Cli};
use vidcut::gui::export::Dialogs;
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Request, Response, Store};

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, _: PlayerId, done: Presented) {
        done(None);
    }
    fn capture_viewer(&self, _: PlayerId, done: Capture) {
        done(Err(vidcut_core::Error::usage("no renderer in tests")));
    }
    fn capture_window(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no window in tests")));
    }
    fn window(&self, _: WindowChange) {}
    fn quit(&self) {}
}

/// File dialogs that answer at once: saves go to `dir` as `quick-<name>`,
/// opening picks the last file saved, folders are `folder`.
struct FakeDialogs {
    dir: PathBuf,
    saved: Arc<Mutex<Vec<PathBuf>>>,
    folder: Arc<Mutex<PathBuf>>,
}

impl Dialogs for FakeDialogs {
    fn save_file(&self, _: &str, _: &Path, name: &str, _: &[&str]) -> Option<PathBuf> {
        let path = self.dir.join(format!("quick-{name}"));
        self.saved.lock().unwrap().push(path.clone());
        Some(path)
    }
    fn pick_file(&self, _: &str, _: &Path, _: &[&str]) -> Option<PathBuf> {
        self.saved.lock().unwrap().last().cloned()
    }
    fn pick_folder(&self, _: &str, _: &Path) -> Option<PathBuf> {
        Some(self.folder.lock().unwrap().clone())
    }
}

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// Steps the GUI until `ready` holds (helper threads answer the dialogs).
fn until(
    harness: &mut Harness<'_, Workspace>,
    what: &str,
    ready: impl Fn(&Harness<'_, Workspace>) -> bool,
) {
    let started = Instant::now();
    while !ready(harness) {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
}

fn has(harness: &Harness<'_, Workspace>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

/// What the GUI's last `export.start` asked for.
fn last_gui_export(h: &live::Handle) -> Value {
    let log = call(h, "gui.log", json!({ "limit": 1000 })).result;
    log.as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|e| e["origin"] == "gui" && e["method"] == "export.start")
        .expect("an export from the GUI")["params"]
        .clone()
}

#[test]
fn export_windows() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-export-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
    }
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ))
    .unwrap();
    let project = dir.join("p.vidcut");
    Store::create(
        &project,
        &Project::new(Settings {
            fps: Rational::new(30, 1).unwrap(),
            width: 320,
            height: 180,
            auto_format: false,
            ..Settings::default()
        }),
    )
    .unwrap();
    let (h, session) = live::start(live::Config {
        project: Some(project.clone()),
        create: false,
        players: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    call(&h, "media.add", json!({ "paths": [media] }));
    call(
        &h,
        "edit.append",
        json!({ "media": "m1", "range": "0f..30f" }),
    );
    call(
        &h,
        "edit.append",
        json!({ "media": "m1", "range": "60f..90f" }),
    );
    call(&h, "marks.set", json!({ "in": 10, "out": 40 }));
    let mut ws = Workspace::new(h.clone());
    let saved = Arc::new(Mutex::new(Vec::new()));
    let folder = Arc::new(Mutex::new(dir.to_path_buf()));
    ws.set_dialogs(Arc::new(FakeDialogs {
        dir: dir.to_path_buf(),
        saved: saved.clone(),
        folder: folder.clone(),
    }));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    harness.step();

    // 1. Quick Export: a preset tile, Export…, the file dialog, then
    // a progress window. The marks are set, so it exports In–Out.
    harness.get_by_label("Quick Export").click();
    until(&mut harness, "preset tiles", |h| has(h, "ProRes 422 HQ\n"));
    // New windows are measured in their first frame and placed in the next.
    harness.step();
    harness.get_by_label_contains("ProRes 422 HQ\n").click();
    harness.step();
    harness.get_by_label("Export…").click();
    until(&mut harness, "the progress window", |h| {
        has(h, "Export quick-p.mov")
    });
    let j1 = call(&h, "jobs.wait", json!({ "job": "j1" })).result;
    assert_eq!(j1["frames"], 30, "In–Out is 30 frames");
    assert_eq!(j1["params"]["preset"], "prores-422hq");
    until(&mut harness, "done", |h| has(h, "Done in"));
    assert!(dir.join("quick-p.mov").exists());
    harness.get_by_label("Close").click();
    harness.step();

    // 2. The Export dialog: switch to a bitrate and the whole timeline, then
    // compare its command with what the GUI sends.
    harness.get_by_label("Export").click();
    until(&mut harness, "the dialog", |h| has(h, "Start Export"));
    harness.step();
    harness.get_by_label("Bitrate").click();
    harness.get_by_label("Whole timeline").click();
    harness.get_by_label("Replace existing files").click();
    until(&mut harness, "a plan", |h| {
        h.query_by_label("Start Export")
            .is_some_and(|b| !b.accesskit_node().is_disabled())
    });
    // Labels keep their text in the AccessKit value.
    let command = harness
        .get_by_label_contains("vidcut -p ")
        .value()
        .expect("the command's text");
    let args = shlex::split(&command).expect("a shell command");
    let cli = Cli::try_parse_from(&args).expect("the CLI parses it");
    let Some(Action::Run(envelope)) = cli.action().unwrap() else {
        panic!("{command}: not a request");
    };
    harness.get_by_label("Start Export").click();
    until(&mut harness, "the second job", |h| has(h, "Export p.mov"));
    let mut gui = last_gui_export(&h);
    assert_eq!(gui["background"], true);
    gui["background"] = json!(false);
    let gui_request = Request::from_parts("export.start", Some(gui.clone())).unwrap();
    assert_eq!(gui_request, envelope.request, "{command}");
    assert!(
        args.windows(4).any(|w| w
            == [
                "--set",
                "video.bitrate=20000000",
                "--set",
                "video.quality=null"
            ]),
        "{command}"
    );
    // The same plan headless, from the command as a shell would run it.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_vidcut"))
        .args(["--headless", "--json", "--dry-run"])
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let headless: Value = serde_json::from_slice(&out.stdout).unwrap();
    let live_plan = call(&h, "export.plan", gui.clone()).result;
    assert_eq!(headless["result"], live_plan);
    assert_eq!(live_plan["outputs"][0]["frames"], 60);
    let j2 = call(&h, "jobs.wait", json!({ "job": "j2" })).result;
    assert_eq!(j2["state"], "done");
    until(&mut harness, "done", |h| has(h, "Done in"));
    harness.get_by_label("Close").click();

    // 3. The jobs window lists both.
    until(&mut harness, "no running jobs", |h| {
        h.query_by_label("Jobs").is_some()
    });
    harness.get_by_label("Jobs").click();
    until(&mut harness, "jobs", |h| {
        h.query_all_by_label_contains("Done in").count() >= 2
    });
    // Closed again (it may cover the menu bar): the toggle comes first.
    harness.query_all_by_label("Jobs").next().unwrap().click();
    harness.step();

    // 4. Timeline interchange: export as OTIO, import it back (appended).
    harness.get_by_label("File").click();
    until(&mut harness, "the File menu", |h| {
        has(h, "Export Timeline (OTIO)…")
    });
    harness.get_by_label("Export Timeline (OTIO)…").click();
    until(&mut harness, "the .otio file", |_| {
        dir.join("quick-p.otio").exists()
    });
    harness.get_by_label("File").click();
    until(&mut harness, "the File menu", |h| {
        has(h, "Import Timeline (OTIO)…")
    });
    harness.get_by_label("Import Timeline (OTIO)…").click();
    until(&mut harness, "the choice", |h| {
        has(h, "Append to the Timeline")
    });
    harness.step();
    harness.get_by_label("Append to the Timeline").click();
    until(&mut harness, "the import", |_| {
        h.snapshot()
            .project()
            .is_some_and(|p| p.tracks[0].clips.len() == 4)
    });

    // 5. Media that moved: the import asks for a folder and looks in its
    // subfolders.
    let fixtures = media.parent().unwrap();
    let moved = dir.join("moved.otio");
    let text = std::fs::read_to_string(dir.join("quick-p.otio"))
        .unwrap()
        .replace(fixtures.to_str().unwrap(), "/nonexistent/footage");
    std::fs::write(&moved, text).unwrap();
    saved.lock().unwrap().push(moved);
    *folder.lock().unwrap() = fixtures.parent().unwrap().to_path_buf();
    harness.get_by_label("File").click();
    until(&mut harness, "the File menu", |h| {
        has(h, "Import Timeline (OTIO)…")
    });
    harness.get_by_label("Import Timeline (OTIO)…").click();
    until(&mut harness, "the choice", |h| {
        has(h, "Append to the Timeline")
    });
    harness.step();
    harness.get_by_label("Append to the Timeline").click();
    until(&mut harness, "the question", |h| has(h, "Choose Folder…"));
    assert!(has(&harness, "/nonexistent/footage/"));
    harness.step();
    harness.get_by_label("Choose Folder…").click();
    until(&mut harness, "the relinked import", |_| {
        h.snapshot()
            .project()
            .is_some_and(|p| p.tracks[0].clips.len() == 6)
    });

    // 6. With its media: an .otioz bundle, written by a job; imported, its
    // media unpack next to it.
    harness.get_by_label("File").click();
    until(&mut harness, "the File menu", |h| {
        has(h, "Export Timeline with Media (OTIOZ)…")
    });
    harness
        .get_by_label("Export Timeline with Media (OTIOZ)…")
        .click();
    until(&mut harness, "the bundle", |_| {
        dir.join("quick-p.otioz").exists()
    });
    assert!(
        h.snapshot()
            .jobs
            .iter()
            .any(|j| j.title == "quick-p.otioz" && j.origin == "gui"),
        "a job"
    );
    harness.get_by_label("File").click();
    until(&mut harness, "the File menu", |h| {
        has(h, "Import Timeline (OTIO)…")
    });
    harness.get_by_label("Import Timeline (OTIO)…").click();
    until(&mut harness, "the choice", |h| {
        has(h, "Replace the Timeline")
    });
    harness.step();
    harness.get_by_label("Replace the Timeline").click();
    until(&mut harness, "the bundle's import", |_| {
        h.snapshot().project().is_some_and(|p| {
            p.tracks[0].clips.len() == 6
                && p.media
                    .iter()
                    .any(|m| m.path.starts_with(dir.join("quick-p/media")))
        })
    });

    // 7. HDR picture into H.264: one quiet line under the plan, never in
    // the way, whose action keeps it HDR on HEVC (decision D44).
    let pq = fixture::ensure(
        &FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 60)
            .hdr(vidcut_core::model::Hdr::Pq),
    )
    .unwrap();
    let added = call(&h, "media.add", json!({ "paths": [pq] })).result;
    let end = h.snapshot().project().unwrap().end();
    call(&h, "edit.append", json!({ "media": added["media"][0] }));
    call(&h, "marks.set", json!({ "in": end, "out": end + 60 }));
    harness.get_by_label("Export").click();
    until(&mut harness, "the dialog", |h| has(h, "Start Export"));
    harness.step();
    harness.get_by_label("Preset").click();
    harness.step();
    harness.step();
    harness.get_by_label("H.264 Master").click();
    harness.step();
    // p.mov is there from the second export.
    harness.get_by_label("Replace existing files").click();
    until(&mut harness, "the note", |h| {
        has(h, "2.0 s of HDR picture will be tone-mapped to SDR")
    });
    assert!(has(&harness, "SDR, 8-bit"));
    // The export can start as it is.
    assert!(
        !harness
            .get_by_label("Start Export")
            .accesskit_node()
            .is_disabled()
    );
    harness.get_by_label("Export HDR").click();
    until(&mut harness, "HDR", |h| {
        has(h, "HDR (PQ), 10-bit") && !has(h, "tone-mapped")
    });
    let command = harness
        .get_by_label_contains("vidcut -p ")
        .value()
        .expect("the command's text");
    assert!(
        command.contains("video.codec=hevc") && command.contains("video.hdr=pq"),
        "{command}"
    );

    drop(harness);
    h.shutdown();
    let _ = session.join();
    std::thread::sleep(Duration::from_millis(10));
}
