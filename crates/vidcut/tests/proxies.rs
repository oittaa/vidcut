//! Proxy encodes (decision D41) run beside the session, not in it: the GUI's
//! core thread keeps answering while one encodes, progress comes as an
//! export's does, each encode is a job the Jobs window and `jobs.*` show and
//! cancel, and a cancelled encode leaves no file and records nothing.
//! Headless, the encode happens before the project is locked. No player
//! and no display.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

fn envelope(method: &str, params: Value) -> Envelope {
    Envelope::from_json(json!({ "method": method, "params": params })).unwrap()
}

fn call(h: &live::Handle, method: &str, params: Value) -> vidcut_core::Result<Response> {
    h.call(envelope(method, params), Origin::Ipc, None, &mut |_| true)
}

fn ok(h: &live::Handle, method: &str, params: Value) -> Value {
    call(h, method, params.clone())
        .unwrap_or_else(|e| panic!("{method} {params}: {e}"))
        .result
}

/// The newest proxy job once `ready` accepts it, polling.
fn proxy_job(h: &live::Handle, ready: impl Fn(&Value) -> bool) -> Value {
    let wait = Instant::now();
    loop {
        let jobs = ok(h, "jobs.list", json!({}));
        if let Some(j) = jobs.as_array().unwrap().last()
            && j["kind"] == "proxy"
            && ready(j)
        {
            return j.clone();
        }
        assert!(wait.elapsed() < Duration::from_secs(60), "{jobs:#}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Files under `dir`, recursively.
fn files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|e| {
            let p = e.path();
            if p.is_dir() { files(&p) } else { vec![p] }
        })
        .collect()
}

#[test]
fn proxies_encode_beside_the_session() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxies-")
        .tempdir_in(vidcut_av::testdata::temp_dir())
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
    }
    // Big enough that its proxy takes a while: the session answers meanwhile.
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        1920,
        1080,
        (30, 1),
        900,
    ))
    .unwrap();
    let path = dir.join("p.vidcut");
    Store::create(
        &path,
        &Project::new(Settings {
            fps: Rational::new(30, 1).unwrap(),
            width: 1920,
            height: 1080,
            auto_format: false,
            ..Settings::default()
        }),
    )
    .unwrap();
    let (h, session) = live::start(live::Config {
        project: Some(path.clone()),
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    ok(&h, "media.add", json!({ "paths": [media] }));
    let proxies = dir.join("proxies");

    // 1. Progress arrives as an export's does.
    let encoding = {
        let h = h.clone();
        std::thread::spawn(move || {
            let mut events = Vec::new();
            let r = h.call(
                envelope("proxy.generate", json!({ "media": "m1" })),
                Origin::Ipc,
                None,
                &mut |v| {
                    if let Some(v) = v {
                        events.push(v);
                    }
                    true
                },
            );
            (r, events)
        })
    };
    let (r, events) = encoding.join().unwrap();
    r.unwrap_or_else(|e| panic!("proxy.generate: {e}"));
    let last = events.last().unwrap();
    assert_eq!(
        (&last["frames"], &last["total"], &last["job"]),
        (&json!(900), &json!(900), &json!("j1"))
    );
    // It was a job, titled by its media.
    let name = ok(&h, "media.get", json!({ "media": "m1" }))["name"].clone();
    let job = proxy_job(&h, |_| true);
    assert_eq!(
        (&job["id"], &job["state"], &job["title"], &job["origin"]),
        (&json!("j1"), &json!("done"), &name, &json!("ipc")),
        "{job:#}"
    );
    let proxy = ok(&h, "media.get", json!({ "media": "m1" }))["proxy"].clone();
    let file = PathBuf::from(proxy["path"].as_str().unwrap());
    assert!(file.is_file() && file.starts_with(&proxies), "{proxy}");

    // 2. Cancelled after its first progress: CANCELLED, nothing recorded,
    // no file left, not even a partial one.
    ok(&h, "proxy.remove", json!({ "media": "m1" }));
    assert!(files(&proxies).is_empty(), "{:?}", files(&proxies));
    let e = h
        .call(
            envelope("proxy.generate", json!({ "media": "m1" })),
            Origin::Ipc,
            None,
            &mut |v| v.is_none(),
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled, "{e}");
    assert!(ok(&h, "media.get", json!({ "media": "m1" }))["proxy"].is_null());
    assert!(files(&proxies).is_empty(), "{:?}", files(&proxies));
    assert_eq!(proxy_job(&h, |_| true)["state"], "cancelled");

    // 3. From the GUI, whose requests get no answer: the job shows the
    // progress, and its Cancel (`jobs.cancel`) stops the encode the same way.
    h.request("proxy.generate", json!({ "media": "m1" }));
    let job = proxy_job(&h, |j| j["id"] == "j3" && j["frames"].as_i64() > Some(0));
    assert_eq!(
        (&job["state"], &job["origin"]),
        (&json!("running"), &json!("gui"))
    );
    ok(&h, "jobs.cancel", json!({ "job": "j3" }));
    let e = call(&h, "jobs.wait", json!({ "job": "j3" })).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled, "{e}");
    assert!(ok(&h, "media.get", json!({ "media": "m1" }))["proxy"].is_null());
    assert!(files(&proxies).is_empty(), "{:?}", files(&proxies));
    // Left to finish, it records the proxy, and `jobs.wait` sees it done.
    h.request("proxy.generate", json!({ "media": "m1" }));
    proxy_job(&h, |j| j["id"] == "j4");
    assert_eq!(ok(&h, "jobs.wait", json!({ "job": "j4" }))["state"], "done");
    assert!(!ok(&h, "media.get", json!({ "media": "m1" }))["proxy"].is_null());
    ok(&h, "proxy.remove", json!({ "media": "m1" }));

    // 4. Automatic proxies, for what the rules pick (here: more than
    // 1280×720). Turning proxies on makes a job of the session's own for
    // the pool; an import makes one for what it added.
    ok(
        &h,
        "preferences.set",
        json!({ "proxies": { "larger_than": [1280, 720] } }),
    );
    let jobs = |h: &live::Handle| ok(h, "jobs.list", json!({})).as_array().unwrap().len();
    let before = jobs(&h);
    ok(&h, "project.set", json!({ "use_proxies": true }));
    let job = proxy_job(&h, |j| j["origin"] == "session");
    assert_eq!(job["params"]["media"], json!(["m1"]), "{job:#}");
    let id = job["id"].as_str().unwrap().to_string();
    assert_eq!(ok(&h, "jobs.wait", json!({ "job": id }))["state"], "done");
    assert!(!ok(&h, "media.get", json!({ "media": "m1" }))["proxy"].is_null());
    let second = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        1920,
        1080,
        (30, 1),
        60,
    ))
    .unwrap();
    ok(&h, "media.add", json!({ "paths": [second] }));
    let job = proxy_job(&h, |j| j["params"]["media"] == json!(["m2"]));
    let id = job["id"].as_str().unwrap().to_string();
    assert_eq!(ok(&h, "jobs.wait", json!({ "job": id }))["state"], "done");
    assert!(!ok(&h, "media.get", json!({ "media": "m2" }))["proxy"].is_null());
    assert_eq!(jobs(&h), before + 2);
    // Switched off, and the proxies removed, for what follows.
    ok(
        &h,
        "preferences.set",
        json!({ "proxies": { "auto": false } }),
    );
    ok(&h, "proxy.remove", json!({}));
    ok(&h, "project.set", json!({ "use_proxies": false }));
    ok(&h, "project.set", json!({ "use_proxies": true }));

    // 5. H.264 chosen makes a new file in place of the one made before,
    // named by its codec.
    ok(&h, "proxy.generate", json!({ "media": "m1" }));
    ok(
        &h,
        "preferences.set",
        json!({ "proxies": { "codec": "h264" } }),
    );
    ok(&h, "proxy.generate", json!({ "media": "m1" }));
    let h264 = ok(&h, "media.get", json!({ "media": "m1" }))["proxy"].clone();
    assert_eq!(h264["codec"], "h264", "{h264}");
    assert!(
        h264["path"].as_str().unwrap().ends_with("-h264.mkv"),
        "{h264}"
    );
    assert_eq!(
        files(&proxies).len(),
        1,
        "the replaced proxy's file went: {:?}",
        files(&proxies)
    );
    ok(
        &h,
        "preferences.set",
        json!({ "proxies": { "codec": "auto" } }),
    );
    ok(&h, "proxy.remove", json!({}));
    h.shutdown();
    let _ = session.join();

    // 4. Headless: progress goes to the hooks, and the cancel flag stops the
    // encode the same way.
    let cancel = Arc::new(AtomicBool::new(false));
    let mut frames = 0;
    let e = vidcut::session::run_with(
        Some(&path),
        envelope("proxy.generate", json!({ "media": "m1" })),
        &mut vidcut::session::Hooks {
            cancel: &cancel,
            progress: &mut |_, _, _, p| {
                frames = p.frames;
                cancel.store(true, Ordering::Relaxed);
            },
        },
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled, "{e}");
    assert!(frames > 0, "no progress headless");
    assert!(files(&proxies).is_empty(), "{:?}", files(&proxies));
}
