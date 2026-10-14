//! Exports in the live session. Cancelling one leaves no file. An edit sent
//! right after one starts doesn't change what it encodes. A second one waits
//! for the first. Quitting during one leaves no partial file. No player and
//! no display.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, ErrorCode, Project, Response, Store};

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, done: Presented) {
        done(None);
    }
    fn capture_viewer(&self, done: Capture) {
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
fn cancel_edit_and_quit_during_exports() {
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
        player: None,
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

    // 1. A slow background export (4K, slowest x264): cancelling ends it
    // without leaving files.
    let b = dir.join("b.mkv");
    let mut slow = quick(&b);
    slow["set"] = json!([
        "video.width=3840",
        "video.height=2160",
        "video.speed=veryslow"
    ]);
    slow["background"] = json!(true);
    let job = ok(&h, "export.start", slow)["id"].clone();
    ok(&h, "jobs.cancel", json!({ "job": job }));
    let e = call(&h, "jobs.wait", json!({ "job": job })).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled, "{e}");
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("b."))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // A long timeline keeps the exports below running.
    for _ in 0..10 {
        ok(&h, "edit.append", json!({ "media": "m1" }));
    }

    // 2. An export encodes the project as it was when asked for: an edit
    // right after it (another client's, say) doesn't change what it encodes.
    let frames = ok(&h, "project.summary", json!({}))["duration"].clone();
    let now = quick(&dir.join("g.mkv"));
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

    // 3. Exports take turns: a second one waits while the first runs, and
    // runs once the first ends.
    let mut slow = quick(&dir.join("q1.mkv"));
    slow["set"] = json!([
        "video.width=3840",
        "video.height=2160",
        "video.speed=veryslow"
    ]);
    slow["background"] = json!(true);
    let first = ok(&h, "export.start", slow)["id"].clone();
    let mut next = quick(&dir.join("q2.mkv"));
    next["background"] = json!(true);
    let second = ok(&h, "export.start", next);
    assert_eq!(second["state"], "queued", "{second}");
    ok(&h, "jobs.cancel", json!({ "job": first }));
    let done = ok(&h, "jobs.wait", json!({ "job": second["id"] }));
    assert_eq!(done["frames"], done["total"], "{done}");
    assert!(dir.join("q2.mkv").exists());

    // 4. Quitting while an export runs leaves no partial file.
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
