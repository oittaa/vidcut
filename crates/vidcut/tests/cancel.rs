//! A client that leaves cancels work which reports no progress. No players,
//! no display, no encode: the flag is set before the work starts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
use vidcut_core::model::Settings;
use vidcut_core::{Envelope, ErrorCode, Project, Store};

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

#[test]
fn a_gone_client_cancels_frames_storyboards_and_analysis() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-cancel-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
        std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
    }
    let path = dir.join("p.vidcut");
    Store::create(&path, &Project::new(Settings::default())).unwrap();
    let (h, session) = live::start(live::Config {
        project: Some(path),
        create: false,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    let ready = (0..200).any(|_| {
        if h.snapshot().project.is_some() {
            true
        } else {
            std::thread::sleep(Duration::from_millis(10));
            false
        }
    });
    assert!(ready, "the project did not open");

    // A client that is still there is not cancelled.
    let get = Envelope::from_json(json!({ "method": "timeline.get", "params": {} })).unwrap();
    h.call(get, Origin::Ipc, None, &mut |_| true).unwrap();

    let out = dir.join("out.png");
    let calls: [(&str, Value); 6] = [
        ("frame.render", json!({ "at": "0f", "out": out })),
        ("storyboard.render", json!({ "out": out })),
        ("analyze.silence", json!({})),
        ("analyze.scenes", json!({})),
        ("analyze.black", json!({})),
        ("analyze.loudness", json!({})),
    ];
    for (method, params) in calls {
        let envelope = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
        let started = Instant::now();
        // Returning false is what a closed socket does to the progress probe.
        let err = h
            .call(envelope, Origin::Ipc, None, &mut |_| false)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled, "{method}: {err}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{method} kept running for {:?}",
            started.elapsed()
        );
        assert!(!out.exists(), "{method} wrote {out:?}");
    }
    h.shutdown();
    session.join().unwrap();
}
