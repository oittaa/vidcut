use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, ErrorCode, Project, Store};

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

fn ok(h: &live::Handle, method: &str, params: Value) -> Value {
    h.call(
        envelope(method, params.clone()),
        Origin::Ipc,
        None,
        &mut |_| true,
    )
    .unwrap_or_else(|e| panic!("{method} {params}: {e}"))
    .result
}

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

fn automatic_job(h: &live::Handle, seen: &[String]) -> Option<String> {
    for _ in 0..1000 {
        let job = h
            .snapshot()
            .jobs
            .iter()
            .find(|j| j.origin == Origin::Session.name() && !seen.contains(&j.id))
            .map(|j| j.id.clone());
        if job.is_some() {
            return job;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn cancelled_and_automatic_proxies() {
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
        project: Some(path),
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    ok(&h, "media.add", json!({ "paths": [media] }));

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
    let proxies = dir.join("proxies");
    assert!(files(&proxies).is_empty(), "{:?}", files(&proxies));

    let sequence = fixture::ensure_images(&ImageSpec::sequence(1, 30, 6, 320, 180)).unwrap();
    let still = fixture::ensure_images(&ImageSpec::still(7, 320, 180, "png")).unwrap();
    ok(&h, "project.set", json!({ "use_proxies": true }));
    ok(
        &h,
        "apply",
        json!({ "commands": [
            { "method": "media.add", "params": { "paths": [sequence[0]], "sequence": true } },
            { "method": "media.add", "params": { "paths": [still[0]] } },
        ] }),
    );
    let first = automatic_job(&h, &[]).expect("an automatic proxy");
    ok(&h, "jobs.wait", json!({ "job": first }));
    let made = ok(&h, "media.get", json!({ "media": "m2" }))["proxy"]["path"].clone();
    let made = PathBuf::from(made.as_str().expect("a proxy"));

    std::fs::remove_file(&made).unwrap();
    let again = automatic_job(&h, &[first]).expect("the lost proxy made again");
    ok(&h, "jobs.wait", json!({ "job": again }));
    assert!(made.is_file());

    h.shutdown();
    let _ = session.join();
}
