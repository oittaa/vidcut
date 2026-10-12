//! Pool thumbnails kept on disk (preference `thumbnails.disk_cache`): a
//! project opens with them shown even when its media can't be decoded any
//! more, and File > Preferences turns the cache off (emptying it) with one
//! request.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};
use vidcut::gui::workspace::Workspace;
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

/// The pool's filmstrip of `media`, once the session has it.
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
fn thumbnails_come_back_from_the_disk_cache_until_it_is_turned_off() {
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

    // Decoded the first time, and kept.
    let (h, session) = start(&path);
    call(&h, "media.add", json!({ "paths": [clip] }));
    let decoded = thumbs(&h, "m1");
    assert_eq!(decoded.frames.len(), 24);
    let files = cache_files(&cache);
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].ends_with("24-192x108.thumbs"), "{files:?}");
    h.shutdown();
    session.join().unwrap();

    // The clip can't be decoded any more (it moved), yet the project opens
    // with its thumbnails: they come from the cache.
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

    // Another project's m1 is another file: its own thumbnails and sound,
    // never those of the m1 before it. And back again.
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
    // 24 pictures over 10 s; the 3 s clip's end before 3 s.
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

    // File > Preferences: turning the cache off is one request, and empties it.
    let ws = Workspace::new(h.clone());
    let mut harness = Harness::builder()
        .with_size(vec2(1400.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    harness.step();
    harness.get_by_label("File").click();
    // A menu ignores input in the frame it opens in.
    harness.step();
    harness.step();
    harness.get_by_label("Preferences… Ctrl+,").click();
    let started = Instant::now();
    while harness.query_by_label("Keep thumbnails on disk").is_none() {
        assert!(started.elapsed() < Duration::from_secs(30), "no window");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    // A new window is placed in the frame after it is measured.
    harness.step();
    let sets = |h: &live::Handle| -> Vec<Value> {
        let log = call(h, "gui.log", json!({ "limit": 1000 })).result;
        log.as_array()
            .unwrap()
            .iter()
            .filter(|e| e["origin"] == "gui" && e["method"] == "preferences.set")
            .cloned()
            .collect()
    };
    // The size limit is a number typed in: Enter sends it, once.
    let field = harness.get_by_label("Size limit");
    field.focus();
    harness.step();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_by_label("Size limit").type_text("256");
    harness.key_press(egui::Key::Enter);
    let started = Instant::now();
    while sets(&h).is_empty() {
        assert!(started.elapsed() < Duration::from_secs(30), "no size sent");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    assert_eq!(
        sets(&h)[0]["params"],
        json!({ "thumbnails": { "cache_limit_mb": 256 } })
    );
    // Wait for the window to take the answer before the next change.
    while harness
        .query_by_label("Keep thumbnails on disk")
        .is_none_or(|n| n.accesskit_node().is_disabled())
    {
        assert!(started.elapsed() < Duration::from_secs(30), "no answer");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    harness.get_by_label("Keep thumbnails on disk").click();
    // The request runs as a task: the cache goes while it runs, and the
    // command log has it once the task has finished.
    let started = Instant::now();
    while cache.join("vidcut/thumbs").exists() || sets(&h).len() < 2 {
        assert!(started.elapsed() < Duration::from_secs(30), "cache kept");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    let made = sets(&h);
    assert_eq!(made.len(), 2, "{made:#?}");
    assert_eq!(
        made[1]["params"],
        json!({ "thumbnails": { "disk_cache": false } })
    );
    let saved = std::fs::read_to_string(dir.join("config/vidcut/preferences.toml")).unwrap();
    assert!(saved.contains("disk_cache = false"), "{saved}");
    assert!(saved.contains("cache_limit_mb = 256"), "{saved}");

    // GPU acceleration goes off with one click, and an export then plans
    // the CPU's encoder whatever the settings name; proxies too.
    const GPU: &str = "Decode, scale and encode on the GPU";
    let started = Instant::now();
    while harness
        .query_by_label(GPU)
        .is_none_or(|n| n.accesskit_node().is_disabled())
    {
        assert!(started.elapsed() < Duration::from_secs(30), "no answer");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    harness.get_by_label(GPU).click();
    while sets(&h).len() < 3 {
        assert!(started.elapsed() < Duration::from_secs(30), "GPU kept");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    assert_eq!(
        sets(&h)[2]["params"],
        json!({ "gpu": { "acceleration": false } })
    );
    let saved = std::fs::read_to_string(dir.join("config/vidcut/preferences.toml")).unwrap();
    assert!(saved.contains("acceleration = false"), "{saved}");
    call(&h, "edit.append", json!({ "media": "m1" }));
    let plan = call(
        &h,
        "export.plan",
        json!({ "output": dir.join("out.mp4"), "encoder": "nvenc" }),
    )
    .result;
    assert_eq!(plan["settings"]["video"]["encoder"], "cpu", "{plan}");
    assert_eq!(plan["video_encoder"]["kind"], "cpu", "{plan}");
    // Automatic proxies are H.264 with x264 then, not AV1 on NVENC.
    let prefs = call(&h, "preferences.get", json!({})).result;
    assert_eq!(prefs["proxy_codecs"][0]["codec"], "auto", "{prefs}");
    assert_eq!(prefs["proxy_codecs"][0]["encoder"], "libx264", "{prefs}");

    drop(harness);
    h.shutdown();
    let _ = session.join();
}
