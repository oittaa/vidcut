use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Filmstrip, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};

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

fn start(project: &Path) -> (live::Handle, std::thread::JoinHandle<()>) {
    live::start(live::Config {
        project: Some(project.to_path_buf()),
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    })
}

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn thumbs(h: &live::Handle, media: &str) -> Arc<Filmstrip> {
    let started = Instant::now();
    loop {
        let snap = h.snapshot();
        if let Some(t) = snap.thumbs.get(media) {
            return t.clone();
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no thumbnails for {media}; notices: {:?}",
            snap.notices
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn cache_files(cache: &Path) -> Vec<String> {
    std::fs::read_dir(cache.join("vidcut/thumbs"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn thumbnails_come_back_from_the_disk_cache_for_each_project() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-thumbnails-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let cache = dir.join("cache");
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_CACHE_HOME", &cache);
    }
    let fixture = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ))
    .unwrap();
    let clip = dir.join("clip.mp4");
    std::fs::copy(&fixture, &clip).unwrap();
    let path = dir.join("thumbs.vidcut");
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

    let (h, session) = start(&path);
    call(&h, "media.add", json!({ "paths": [clip] }));
    let decoded = thumbs(&h, "m1");
    assert!(!decoded.frames.is_empty());
    let files = cache_files(&cache);
    assert_eq!(files.len(), 1, "{files:?}");
    h.shutdown();
    session.join().unwrap();

    std::fs::rename(&clip, dir.join("moved.mp4")).unwrap();
    let (h, session) = start(&path);
    let cached = thumbs(&h, "m1");
    assert_eq!(
        (cached.width, cached.height, cached.frames.len()),
        (decoded.width, decoded.height, decoded.frames.len())
    );
    assert_eq!(cached.times, decoded.times);
    for (a, b) in decoded.frames.iter().zip(&cached.frames) {
        let diff = a
            .iter()
            .zip(b)
            .map(|(x, y)| u64::from(x.abs_diff(*y)))
            .sum::<u64>() as f64
            / a.len() as f64;
        assert!(diff < 4.0, "JPEG kept the pictures: mean difference {diff}");
    }

    let longer = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        300,
    ))
    .unwrap();
    call(
        &h,
        "project.open",
        json!({ "path": dir.join("other.vidcut"), "create": true }),
    );
    call(&h, "media.add", json!({ "paths": [longer] }));
    let last = |strip: &Filmstrip| strip.times.last().copied().unwrap_or(0.0);
    let strip = thumbs(&h, "m1");
    assert!(last(&strip) > 5.0, "{:?}", strip.times);
    let started = Instant::now();
    let sound = loop {
        if let Some(w) = h.snapshot().waveforms.get("m1") {
            break w.clone();
        }
        assert!(started.elapsed() < Duration::from_secs(30), "no waveform");
        std::thread::sleep(Duration::from_millis(10));
    };
    let secs = sound.peaks.len() as f64 / f64::from(sound.per_second);
    assert!(
        (9.5..10.5).contains(&secs),
        "the 10 s file's sound: {secs} s"
    );
    call(&h, "project.open", json!({ "path": path }));
    let strip = thumbs(&h, "m1");
    assert!(last(&strip) < 3.0, "{:?}", strip.times);
    h.shutdown();
    let _ = session.join();
}
