//! Export jobs in the live session: foreground exports with progress,
//! background exports, waiting and cancelling, as `vidcut jobs` commands see them.
//! No players and no display.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use vidcut::live::{self, Capture, JobState, Origin, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, ErrorCode, Project, Response, Store};

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, _: PlayerId, done: Presented) {
        done();
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

fn call(h: &live::Handle, method: &str, params: Value) -> vidcut_core::Result<Response> {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
}

fn ok(h: &live::Handle, method: &str, params: Value) -> Value {
    call(h, method, params.clone())
        .unwrap_or_else(|e| panic!("{method} {params}: {e}"))
        .result
}

/// Fast, exact settings for small test exports.
fn quick(out: &Path) -> Value {
    json!({
        "output": out, "encoder": "cpu",
        "set": ["video.quality=0", "video.speed=ultrafast"],
    })
}

#[test]
fn export_jobs() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-jobs-")
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
    let path = dir.join("p.vidcut");
    Store::create(
        &path,
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
        project: Some(path),
        create: false,
        players: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    ok(&h, "media.add", json!({ "paths": [media] }));
    ok(
        &h,
        "edit.append",
        json!({ "media": "m1", "range": "0f..30f" }),
    );
    ok(
        &h,
        "edit.append",
        json!({ "media": "m1", "range": "60f..90f" }),
    );

    // 1. A foreground export: progress carries the job id, the reply the
    // outputs, and the job stays listed.
    let a = dir.join("a.mkv");
    let mut events = Vec::new();
    let e = Envelope::from_json(json!({ "method": "export.start", "params": quick(&a) })).unwrap();
    let r = h
        .call(e, Origin::Ipc, None, &mut |v| {
            if let Some(v) = v {
                events.push(v);
            }
            true
        })
        .unwrap();
    assert_eq!(r.result["outputs"][0]["frames"], 60);
    assert!(!events.is_empty() && events.iter().all(|v| v["job"] == "j1"));
    let jobs = ok(&h, "jobs.list", json!({}));
    let j1 = &jobs[0];
    assert_eq!(
        (j1["id"].as_str(), j1["state"].as_str()),
        (Some("j1"), Some("done"))
    );
    assert_eq!(
        (j1["frames"].as_i64(), j1["total"].as_i64()),
        (Some(60), Some(60))
    );
    assert_eq!(j1["outputs"][0]["path"], json!(a));

    // 2. A slow background export (4K, slowest x264): answered at once with
    // the planned job; a zero timeout returns it running; cancelling ends it
    // without leaving files.
    let b = dir.join("b.mkv");
    let mut slow = quick(&b);
    slow["set"] = json!([
        "video.width=3840",
        "video.height=2160",
        "video.speed=veryslow"
    ]);
    slow["background"] = json!(true);
    let j2 = ok(&h, "export.start", slow);
    assert_eq!(
        (j2["id"].as_str(), j2["state"].as_str()),
        (Some("j2"), Some("running"))
    );
    assert_eq!(j2["total"], 60);
    let now = ok(&h, "jobs.wait", json!({ "job": "j2", "timeout": 0 }));
    assert_eq!(now["state"], "running");
    ok(&h, "jobs.cancel", json!({ "job": "j2" }));
    let e = call(&h, "jobs.wait", json!({ "job": "j2" })).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled, "{e}");
    assert_eq!(
        ok(&h, "jobs.get", json!({ "job": "j2" }))["state"],
        "cancelled"
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("b."))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // 3. The GUI's exports are jobs too (it never waits for them).
    let c = dir.join("c.mkv");
    let e = Envelope::from_json(json!({ "method": "export.start", "params": quick(&c) })).unwrap();
    h.send(e);
    let mut snapshot_jobs = 0;
    for _ in 0..500 {
        snapshot_jobs = h.snapshot().jobs.len();
        if snapshot_jobs == 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(snapshot_jobs, 3);
    let j3 = ok(&h, "jobs.wait", json!({ "job": "j3" }));
    assert_eq!(
        (j3["origin"].as_str(), j3["state"].as_str()),
        (Some("gui"), Some("done"))
    );
    assert!(c.exists());
    let snap = h.snapshot();
    assert_eq!(
        snap.jobs.iter().map(|j| j.state).collect::<Vec<_>>(),
        vec![JobState::Done, JobState::Cancelled, JobState::Done]
    );

    // 4. Mistakes fail the request itself, even in the background.
    let mut bad = quick(&dir.join("d.mkv"));
    bad["preset"] = json!("no-such-preset");
    bad["background"] = json!(true);
    let e = call(&h, "export.start", bad).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "{e}");
    assert_eq!(
        ok(&h, "jobs.get", json!({ "job": "j4" }))["state"],
        "failed"
    );
    let e = call(&h, "jobs.get", json!({ "job": "j99" })).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    // `.otio` exports are instant: no job.
    let r = ok(
        &h,
        "export.start",
        json!({ "output": dir.join("cut.otio") }),
    );
    assert_eq!(r["outputs"][0]["clips"], 2);
    assert_eq!(ok(&h, "jobs.list", json!({})).as_array().unwrap().len(), 4);

    // 5. An export the person at the GUI started can be cancelled from the
    // CLI. A long timeline keeps it running meanwhile.
    for _ in 0..10 {
        ok(&h, "edit.append", json!({ "media": "m1" }));
    }
    let mut long = quick(&dir.join("e.mkv"));
    long["set"] = json!([
        "video.width=3840",
        "video.height=2160",
        "video.speed=veryslow"
    ]);
    let e = Envelope::from_json(json!({ "method": "export.start", "params": long })).unwrap();
    h.send(e);
    for _ in 0..500 {
        if h.snapshot().jobs.len() == 5 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let j5 = ok(&h, "jobs.cancel", json!({ "job": "j5" }));
    assert_eq!(j5["origin"], "gui");
    let e = call(&h, "jobs.wait", json!({ "job": "j5" })).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled, "{e}");
    assert!(!dir.join("e.mkv").exists());

    // 6. An export encodes the project as it was when asked for: a stale
    // `if_rev` fails it up front, and an edit right after it (another
    // client's, say) doesn't change what it encodes.
    let r = call(&h, "project.summary", json!({})).unwrap();
    let (rev, frames) = (r.rev, r.result["duration"].clone());
    let mut stale = quick(&dir.join("f.mkv"));
    stale["if_rev"] = json!(rev - 1);
    let e = call(&h, "export.start", stale).unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict, "{e}");
    assert!(!dir.join("f.mkv").exists());
    let mut now = quick(&dir.join("g.mkv"));
    now["if_rev"] = json!(rev);
    let e = Envelope::from_json(json!({ "method": "export.start", "params": now })).unwrap();
    h.send(e);
    let e = Envelope::from_json(json!({ "method": "edit.delete", "params": { "clips": "c1" } }))
        .unwrap();
    h.send(e);
    let mut id = None;
    for _ in 0..500 {
        id = h
            .snapshot()
            .jobs
            .iter()
            .find(|j| j.outputs.first().is_some_and(|o| o.path.ends_with("g.mkv")))
            .map(|j| j.id.clone());
        if id.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let j = ok(
        &h,
        "jobs.wait",
        json!({ "job": id.expect("the export's job") }),
    );
    assert_eq!((&j["frames"], &j["total"]), (&frames, &frames), "{j}");
    assert_ne!(
        call(&h, "project.summary", json!({})).unwrap().result["duration"],
        frames
    );

    // 7. A bundle copies media: a job like a media export, and its media
    // unpack (on a worker) when it is imported.
    let bundle = dir.join("cut.otioz");
    let job = ok(
        &h,
        "export.start",
        json!({ "output": bundle, "background": true }),
    );
    let j = ok(&h, "jobs.wait", json!({ "job": job["id"] }));
    assert_eq!(j["state"], "done", "{j}");
    assert_eq!(j["result"]["outputs"][0]["format"], "otioz");
    assert!(bundle.is_file());
    let clips = call(&h, "project.summary", json!({})).unwrap().result["clips"].clone();
    let r = ok(
        &h,
        "timeline.import",
        json!({ "path": bundle, "replace": true }),
    );
    assert_eq!(r["bundle"]["folder"], json!(dir.join("cut")), "{r}");
    assert!(dir.join("cut/media").is_dir());
    assert_eq!(
        call(&h, "project.summary", json!({})).unwrap().result["clips"],
        clips
    );

    // 8. Quitting while an export runs leaves no partial file.
    let mut slow = quick(&dir.join("h.mkv"));
    slow["set"] = json!([
        "video.width=3840",
        "video.height=2160",
        "video.speed=veryslow"
    ]);
    slow["background"] = json!(true);
    ok(&h, "export.start", slow);
    let part = dir.join(".h.mkv.part");
    for _ in 0..500 {
        if part.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(part.exists(), "the export started writing");
    h.shutdown();
    let _ = session.join();
    assert!(!part.exists() && !dir.join("h.mkv").exists());
}
