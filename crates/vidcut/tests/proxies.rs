//! Proxy encodes (decision D41) run beside the session, not in it: the GUI's
//! core thread keeps answering while one encodes, progress comes as an
//! export's does, each encode is a job the Jobs window and `jobs.*` show and
//! cancel, and a cancelled encode leaves no file and records nothing.
//! Headless, the encode happens before the project is locked. No players
//! and no display.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
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
        .tempdir()
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
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    ok(&h, "media.add", json!({ "paths": [media] }));
    let proxies = dir.join("proxies");

    // 1. While the proxy encodes, other requests are answered at once, and
    // progress arrives as an export's does.
    let started = Arc::new(AtomicBool::new(false));
    let encoding = {
        let (h, started) = (h.clone(), started.clone());
        std::thread::spawn(move || {
            let mut events = Vec::new();
            let r = h.call(
                envelope("proxy.generate", json!({ "media": "m1" })),
                Origin::Ipc,
                None,
                &mut |v| {
                    if let Some(v) = v {
                        started.store(true, Ordering::Relaxed);
                        events.push(v);
                    }
                    true
                },
            );
            (r, events)
        })
    };
    let wait = Instant::now();
    while !started.load(Ordering::Relaxed) {
        assert!(wait.elapsed() < Duration::from_secs(60), "no progress");
        std::thread::sleep(Duration::from_millis(5));
    }
    let asked = Instant::now();
    ok(&h, "project.get", json!({}));
    let took = asked.elapsed();
    assert!(
        !encoding.is_finished(),
        "the fixture encodes too fast to tell"
    );
    assert!(
        took < Duration::from_millis(500),
        "project.get took {took:?}"
    );
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
    // Switched off in the preferences: nothing by itself.
    ok(
        &h,
        "preferences.set",
        json!({ "proxies": { "auto": false } }),
    );
    ok(&h, "proxy.remove", json!({}));
    ok(&h, "project.set", json!({ "use_proxies": false }));
    ok(&h, "project.set", json!({ "use_proxies": true }));
    let third = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        1920,
        1080,
        (30, 1),
        30,
    ))
    .unwrap();
    ok(&h, "media.add", json!({ "paths": [third] }));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(jobs(&h), before + 2);
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

/// An image sequence's size and time are the whole run's: the total of its
/// pictures and the newest of their times. A render that rewrites a later
/// picture makes a new proxy name, though the first picture stays as it was.
#[test]
fn a_sequence_is_as_new_as_its_newest_picture() {
    use vidcut::env::AvEnv;
    use vidcut_av::fixture::ImageSpec;
    use vidcut_core::editor::Env;
    let made = fixture::ensure_images(&ImageSpec::sequence(1, 3, 4, 64, 36)).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("vidcut-sequence-times")
        .tempdir()
        .unwrap();
    let files: Vec<PathBuf> = made
        .iter()
        .map(|f| {
            let to = dir.path().join(f.file_name().unwrap());
            std::fs::copy(f, &to).unwrap();
            to
        })
        .collect();
    let at = |secs: u64| std::time::UNIX_EPOCH + Duration::from_secs(secs);
    for (k, f) in files.iter().enumerate() {
        let file = std::fs::File::options().write(true).open(f).unwrap();
        file.set_modified(at(1_000_000 + k as u64)).unwrap();
    }
    let env = AvEnv::new();
    let fps = Rational::new(25, 1).unwrap();
    let probed = env.probe_sequence(&files[0], fps).unwrap();
    let total: u64 = files.iter().map(|f| f.metadata().unwrap().len()).sum();
    assert_eq!(probed.size, total);
    assert_eq!(probed.mtime_ns, 1_000_002 * 1_000_000_000);
    // A later picture rendered again.
    let last = std::fs::File::options()
        .write(true)
        .open(&files[2])
        .unwrap();
    last.set_modified(at(2_000_000)).unwrap();
    let again = env.probe_sequence(&files[0], fps).unwrap();
    assert_eq!(again.mtime_ns, 2_000_000 * 1_000_000_000);
}
