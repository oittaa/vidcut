use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::{Value, json};
use tempfile::TempDir;
use vidcut::gui::workspace::{TwoUp, TwoUpMode, Workspace};
use vidcut::live::{self, Capture, DynamicTrim, Origin, Presented, TrimEditor, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture as Content, VideoCodec};
use vidcut_av::player::{PacedOutput, Picture, Player};
use vidcut_av::thumbs::Image;
use vidcut_core::model::Settings;
use vidcut_core::request::{DynamicTrimMode, TrimSide};
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Store};

const W: u32 = 320;
const H: u32 = 180;

const PATIENCE: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Screen {
    exact: Option<i64>,
    log: Vec<(i64, Option<u32>)>,
}

#[derive(Default)]
struct Port {
    engine: Mutex<Option<Arc<Player>>>,
    presents: Mutex<Vec<Presented>>,
    screen: Mutex<Screen>,
}

impl live::Renderer for Port {
    fn attach(&self, engine: Option<Arc<Player>>) {
        *self.engine.lock().unwrap() = engine;
    }
    fn present(&self, done: Presented) {
        self.presents.lock().unwrap().push(done);
    }
    fn capture_viewer(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no captures in tests")));
    }
    fn capture_window(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no window in tests")));
    }
    fn window(&self, _: WindowChange) {}
    fn quit(&self) {}
}

fn code(img: &Image) -> Option<u32> {
    let (w, h) = (img.width as usize, img.height as usize);
    vidcut_av::barcode::read_packed(&img.rgba, w, h, w * 4, 4, 1)
}

fn picture_code(p: &Picture) -> Option<u32> {
    code(&p.to_rgba(W, H).unwrap())
}

static HOME: LazyLock<TempDir> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: every test forces `HOME` first, so no other thread touches the environment yet.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_CACHE_HOME", dir.path().join("cache"));
    }
    dir
});

static ONE_GUI: Mutex<()> = Mutex::new(());

struct Gui {
    h: live::Handle,
    session: Option<std::thread::JoinHandle<()>>,
    port: Arc<Port>,
    stop: Arc<AtomicBool>,
    render: Option<std::thread::JoinHandle<()>>,
    harness: Harness<'static, Workspace>,
    _dir: TempDir,
    _alone: MutexGuard<'static, ()>,
}

impl Gui {
    fn new(codec: VideoCodec) -> Self {
        let _ = &*HOME;
        let alone = ONE_GUI.lock().unwrap_or_else(|e| e.into_inner());
        let media = fixture::ensure(
            &FixtureSpec::new(codec, Container::Mkv, 3840, 2160, (60, 1), 240)
                .gop(120)
                .picture(Content::Busy),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.vidcut");
        Store::create(
            &path,
            &Project::new(Settings {
                fps: Rational::new(60, 1).unwrap(),
                width: 3840,
                height: 2160,
                auto_format: false,
                ..Settings::default()
            }),
        )
        .unwrap();
        let port = Arc::new(Port::default());
        let stop = Arc::new(AtomicBool::new(false));
        let render = {
            let (port, stop) = (port.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut last: Option<Arc<Picture>> = None;
                while !stop.load(Ordering::Relaxed) {
                    let engine = port.engine.lock().unwrap().clone();
                    let presents: Vec<_> = port.presents.lock().unwrap().drain(..).collect();
                    let shown = engine.as_ref().and_then(|e| e.present(Instant::now()));
                    let exact = shown.filter(|s| s.exact);
                    if let Some(s) = &exact
                        && last.as_ref().is_none_or(|l| !Arc::ptr_eq(l, &s.picture))
                    {
                        let read = picture_code(&s.picture);
                        port.screen.lock().unwrap().log.push((s.frame, read));
                        last = Some(s.picture.clone());
                    }
                    let frame = exact.map(|s| s.frame);
                    port.screen.lock().unwrap().exact = frame;
                    for done in presents {
                        done(frame);
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        let audio = Arc::new(|| {
            Box::new(PacedOutput::new(
                Duration::from_millis(10),
                Duration::from_millis(30),
            )) as Box<dyn vidcut_av::player::AudioOutput>
        });
        let (h, session) = live::start(live::Config {
            project: Some(path),
            create: false,
            player: Some(live::PlayerConfig {
                audio,
                budget: vidcut_av::player::DEFAULT_BUDGET,
                gpu_pictures: Vec::new(),
            }),
            preview_error: None,
            renderer: port.clone(),
            notify: Arc::new(|| {}),
            socket: None,
        });
        call(&h, "media.add", json!({ "paths": [media] }));
        for range in ["0f..80f", "100f..180f", "190f..236f"] {
            call(&h, "edit.append", json!({ "media": "m1", "range": range }));
        }
        call(&h, "viewer.set", json!({ "mode": "timeline" }));
        let mut ws = Workspace::new(h.clone());
        ws.timeline_options_mut().snapping = false;
        ws.timeline_options_mut().ppf = 4.0;
        let harness = Harness::builder()
            .with_size(vec2(1600.0, 1000.0))
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(50)
            .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
        let mut gui = Self {
            h,
            session: Some(session),
            port,
            stop,
            render: Some(render),
            harness,
            _dir: dir,
            _alone: alone,
        };
        gui.settle();
        gui
    }

    fn call(&self, method: &str, params: Value) -> Value {
        call(&self.h, method, params)
    }

    fn settle(&mut self) {
        self.call("player.state", json!({}));
        self.harness.step();
        self.harness.step();
    }

    fn key(&mut self, key: Key) {
        self.harness.key_press(key);
        self.settle();
    }

    fn hold(&mut self, key: Key, tap: Key) {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        self.harness.event(event(true));
        self.harness.step();
        self.harness.key_press(tap);
        self.harness.step();
        self.harness.event(event(false));
        self.settle();
    }

    fn double_click(&mut self, at: Pos2) {
        for _ in 0..2 {
            for pressed in [true, false] {
                self.harness.event(egui::Event::PointerMoved(at));
                self.harness.event(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
                self.harness.step();
            }
        }
        self.settle();
    }

    fn at(&self, track: &str, frame: i64, dy: f32) -> Pos2 {
        let l = self.harness.state().timeline_layout().unwrap();
        let row = l.lower.get(track).unwrap().rect.rect;
        pos2(l.lower_scale.xf(frame), row.top() + dy)
    }

    fn wait(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let started = Instant::now();
        while started.elapsed() < PATIENCE {
            self.harness.step();
            if done(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn position(&self) -> i64 {
        self.h.snapshot().timeline.position
    }

    fn clips(&self) -> Vec<Value> {
        let tl = self.call("timeline.get", json!({}));
        let track = tl["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == "V1")
            .unwrap()
            .clone();
        track["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["type"] == "clip")
            .cloned()
            .collect()
    }

    fn source_at(&self, t: i64) -> i64 {
        let n = |c: &Value, k: &str| c[k].as_i64().unwrap();
        let clips = self.clips();
        let clip = clips
            .iter()
            .find(|c| n(c, "start") <= t && t < n(c, "end"))
            .unwrap_or_else(|| panic!("no clip at frame {t}"));
        n(clip, "source_in") + t - n(clip, "start")
    }

    fn clip(&self, id: &str) -> (i64, i64) {
        let c = self
            .clips()
            .into_iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("no clip {id}"));
        (c["start"].as_i64().unwrap(), c["end"].as_i64().unwrap())
    }

    fn shows(&mut self, t: i64) {
        self.wait(&format!("the viewer to show frame {t}"), |g| {
            g.position() == t && g.port.screen.lock().unwrap().exact == Some(t)
        });
        let read = self
            .port
            .screen
            .lock()
            .unwrap()
            .log
            .iter()
            .rev()
            .find(|(f, _)| *f == t)
            .map(|(_, c)| *c);
        let want = self.source_at(t) as u32;
        assert_eq!(read, Some(Some(want)), "the picture at frame {t}");
    }

    fn clear_log(&self) {
        self.port.screen.lock().unwrap().log.clear();
    }

    fn wrong_since_cleared(&self) -> (Vec<(i64, Option<u32>)>, usize) {
        let log = self.port.screen.lock().unwrap().log.clone();
        let wrong = log
            .iter()
            .filter(|(f, c)| *c != Some(self.source_at(*f) as u32))
            .copied()
            .collect();
        (wrong, log.len())
    }

    fn two_up(&self) -> Option<TwoUp> {
        self.harness.state().two_up().cloned()
    }

    fn two_up_shows(&mut self, want: &[i64]) {
        self.wait(&format!("the two-up to show source frames {want:?}"), |g| {
            g.two_up().is_some_and(|t| {
                t.cells.len() == want.len()
                    && t.cells.iter().zip(want).all(|(c, &w)| {
                        c.as_ref()
                            .is_some_and(|c| c.exact && c.source_frame == w && c.picture.is_some())
                    })
            })
        });
        let t = self.two_up().unwrap();
        let read: Vec<_> = t
            .cells
            .into_iter()
            .map(|c| c.and_then(|c| c.picture).and_then(|p| picture_code(&p)))
            .collect();
        let want: Vec<_> = want.iter().map(|&w| Some(w as u32)).collect();
        assert_eq!(read, want, "the two-up's pictures");
    }

    fn two_up_wrong(&self) -> Vec<(i64, Option<u32>)> {
        let Some(t) = self.two_up() else {
            return Vec::new();
        };
        t.cells
            .into_iter()
            .flatten()
            .filter(|c| c.exact)
            .filter_map(|c| {
                let read = c.picture.as_deref().and_then(picture_code);
                (read != Some(c.source_frame as u32)).then_some((c.source_frame, read))
            })
            .collect()
    }

    fn run_for(&mut self, frames: usize) {
        let mut wrong = Vec::new();
        for _ in 0..frames {
            self.harness.step();
            wrong.extend(self.two_up_wrong());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            wrong,
            [],
            "cells shown as their frame's with another frame's picture"
        );
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        self.h.shutdown();
        if let Some(t) = self.session.take() {
            let _ = t.join();
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.render.take() {
            let _ = t.join();
        }
    }
}

fn call(h: &live::Handle, method: &str, params: Value) -> Value {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
        .result
}

fn viewer(codec: VideoCodec) {
    let mut gui = Gui::new(codec);
    for at in [130, 20, 170, 85, 5, 200, 119] {
        gui.call("player.seek", json!({ "at": format!("{at}f") }));
        gui.shows(at);
    }
    gui.call("player.seek", json!({ "at": "100f" }));
    gui.key(Key::ArrowUp);
    gui.shows(80);
    gui.key(Key::ArrowDown);
    gui.shows(160);
    gui.call("player.seek", json!({ "at": "75f" }));
    gui.shows(75);
    for at in 76..=86 {
        gui.key(Key::ArrowRight);
        gui.shows(at);
    }
    for at in (66..=85).rev() {
        gui.key(Key::ArrowLeft);
        gui.shows(at);
    }
    gui.hold(Key::K, Key::L);
    gui.shows(67);
    gui.hold(Key::K, Key::J);
    gui.shows(66);

    gui.clear_log();
    gui.key(Key::L);
    gui.wait("playing past frame 140", |g| g.position() >= 140);
    gui.key(Key::K);
    let (wrong, shown) = gui.wrong_since_cleared();
    assert_eq!(
        wrong,
        [],
        "frames played forward with another frame's picture"
    );
    assert!(shown > 1, "playing forward showed {shown} frames");
    let at = gui.position();
    gui.shows(at);

    gui.clear_log();
    gui.key(Key::J);
    gui.wait("playing back past frame 40", |g| g.position() <= 40);
    gui.key(Key::K);
    let (wrong, shown) = gui.wrong_since_cleared();
    assert_eq!(
        wrong,
        [],
        "frames played backward with another frame's picture"
    );
    assert!(shown > 1, "playing backward showed {shown} frames");
    let at = gui.position();
    gui.shows(at);
}

fn trim_editor(codec: VideoCodec) {
    let mut gui = Gui::new(codec);
    let cut = gui.at("V1", 80, 20.0);
    gui.double_click(cut);
    gui.two_up_shows(&[79, 100]);
    gui.key(Key::Space);
    gui.wait(
        "Space to play the timeline in place of the Trim Editor",
        |g| g.h.snapshot().timeline.playing && g.two_up().is_none(),
    );
    gui.call("player.pause", json!({}));
    gui.two_up_shows(&[79, 100]);
    let by = |gui: &mut Gui, side, by| {
        gui.harness
            .state_mut()
            .trim_editor_request(TrimEditor::Open {
                at: None,
                track: "V1".into(),
                by: Some(by),
                side,
                transition: false,
            });
        gui.settle();
    };
    by(&mut gui, TrimSide::Both, 1);
    gui.two_up_shows(&[80, 101]);
    by(&mut gui, TrimSide::Both, -1);
    gui.two_up_shows(&[79, 100]);
    by(&mut gui, TrimSide::Out, 1);
    gui.two_up_shows(&[80, 100]);
    by(&mut gui, TrimSide::In, -1);
    gui.two_up_shows(&[80, 99]);
    by(&mut gui, TrimSide::In, 1);
    gui.two_up_shows(&[80, 100]);
    by(&mut gui, TrimSide::Out, -1);
    gui.two_up_shows(&[79, 100]);
    gui.harness
        .state_mut()
        .trim_editor_request(TrimEditor::Open {
            at: Some(160),
            track: "V1".into(),
            by: None,
            side: TrimSide::Both,
            transition: false,
        });
    gui.settle();
    gui.two_up_shows(&[179, 190]);
    gui.harness
        .state_mut()
        .trim_editor_request(TrimEditor::Open {
            at: Some(80),
            track: "V1".into(),
            by: None,
            side: TrimSide::Both,
            transition: false,
        });
    gui.settle();
    gui.two_up_shows(&[79, 100]);
    gui.key(Key::Escape);
    assert!(gui.two_up().is_none(), "Esc left the Trim Editor open");
}

fn dynamic_trim_roll(codec: VideoCodec) {
    let mut gui = Gui::new(codec);
    gui.call("player.seek", json!({ "at": "78f" }));
    gui.key(Key::W);
    gui.wait("W to take the playhead to the cut", |g| g.position() == 80);
    gui.two_up_shows(&[79, 100]);
    gui.hold(Key::K, Key::L);
    gui.two_up_shows(&[80, 101]);
    gui.hold(Key::K, Key::J);
    gui.two_up_shows(&[79, 100]);

    for presses in [vec![Key::L], vec![Key::L, Key::L], vec![Key::J]] {
        let (_, before) = gui.clip("c1");
        for k in presses {
            gui.harness.key_press(k);
        }
        gui.run_for(20);
        assert_eq!(
            gui.two_up().map(|t| t.mode),
            Some(TwoUpMode::DynamicTrim),
            "a run's two-up"
        );
        gui.key(Key::K);
        gui.wait("the trim to land", |g| g.clip("c1").1 != before);
        let by = gui.clip("c1").1 - 80;
        gui.two_up_shows(&[79 + by, 100 + by]);
        gui.wait("the playhead to follow the cut", |g| {
            g.position() == 80 + by
        });
    }

    let by = gui.clip("c1").1 - 80;
    gui.key(Key::ArrowDown);
    let (_, end) = gui.clip("c2");
    assert_eq!(end, 160);
    gui.two_up_shows(&[179, 190]);
    gui.key(Key::ArrowUp);
    gui.two_up_shows(&[79 + by, 100 + by]);
    gui.key(Key::W);
    assert!(
        gui.two_up().is_none(),
        "W left the cut selected in the Trim Editor"
    );
}

fn dynamic_trim_clip(codec: VideoCodec, slide: bool) {
    let mut gui = Gui::new(codec);
    let mode = if slide {
        DynamicTrimMode::Slide
    } else {
        DynamicTrimMode::Slip
    };
    gui.harness.state_mut().dynamic_trim_request(
        DynamicTrim {
            on: None,
            side: None,
            mode: Some(mode),
            run: None,
            step: None,
        },
        0.0,
    );
    gui.call("selection.set", json!({ "clips": ["c2"] }));
    gui.settle();
    gui.key(Key::W);
    gui.wait("W to take the playhead to the clip", |g| g.position() == 80);
    assert!(gui.two_up().is_none(), "a two-up before the trim ran");
    let cells = |gui: &Gui| {
        let (start, end) = gui.clip("c2");
        [
            gui.source_at(start),
            gui.source_at(end - 1),
            gui.source_at(start - 1),
            gui.source_at(end),
        ]
    };
    assert_eq!(cells(&gui), [100, 179, 79, 190]);
    gui.hold(Key::K, Key::L);
    let stepped = if slide {
        [100, 179, 80, 191]
    } else {
        [99, 178, 79, 190]
    };
    gui.wait("K and L to trim a frame", |g| cells(g) == stepped);
    for k in [Key::L, Key::J] {
        let before = cells(&gui);
        gui.harness.key_press(k);
        gui.run_for(15);
        assert_eq!(gui.two_up().map(|t| t.cells.len()), Some(4), "a run's 4-up");
        gui.key(Key::K);
        gui.wait("the trim to land", |g| cells(g) != before);
        assert!(gui.two_up().is_none(), "the 4-up stayed after K");
    }
}

#[test]
fn k_and_l_during_space_leave_the_playhead_on_the_trimmed_cut() {
    let mut gui = Gui::new(VideoCodec::Vp9);
    gui.call("player.seek", json!({ "at": "78f" }));
    gui.key(Key::W);
    gui.wait("W to take the playhead to the cut", |g| g.position() == 80);
    gui.key(Key::Space);
    gui.wait("Space to play", |g| g.h.snapshot().timeline.playing);
    gui.hold(Key::K, Key::L);
    gui.wait("the trim to land", |g| g.clip("c1").1 == 81);
    gui.wait("the playhead to stop on the cut", |g| {
        !g.h.snapshot().timeline.playing && g.position() == 81
    });
    gui.run_for(30);
    assert_eq!(gui.position(), 81, "the playhead left the cut");
}

#[test]
fn space_in_dynamic_trim_plays_around_the_playhead_and_goes_back() {
    let mut gui = Gui::new(VideoCodec::Vp9);
    gui.call("player.seek", json!({ "at": "78f" }));
    gui.key(Key::W);
    gui.wait("W to take the playhead to the cut", |g| g.position() == 80);
    gui.key(Key::Space);
    let (mut first, mut furthest) = (None, 0);
    gui.wait("Space to play past the cut and come back", |g| {
        let playing = g.h.snapshot().timeline.playing;
        if playing {
            first.get_or_insert(g.position());
            furthest = furthest.max(g.position());
        }
        first.is_some() && !playing && g.position() == 80
    });
    assert!(first.is_some_and(|f| f < 20), "it started at {first:?}");
    assert!(furthest >= 180, "it went only as far as {furthest}");
}

macro_rules! codec_tests {
    ($($codec:ident: $viewer:ident, $trim:ident, $roll:ident, $slip:ident, $slide:ident;)*) => {$(
        #[test]
        fn $viewer() {
            viewer(VideoCodec::$codec);
        }
        #[test]
        fn $trim() {
            trim_editor(VideoCodec::$codec);
        }
        #[test]
        fn $roll() {
            dynamic_trim_roll(VideoCodec::$codec);
        }
        #[test]
        fn $slip() {
            dynamic_trim_clip(VideoCodec::$codec, false);
        }
        #[test]
        fn $slide() {
            dynamic_trim_clip(VideoCodec::$codec, true);
        }
    )*};
}

codec_tests! {
    Av1: av1_viewer, av1_trim_editor, av1_dynamic_trim_roll, av1_dynamic_trim_slip, av1_dynamic_trim_slide;
    Hevc10: hevc_viewer, hevc_trim_editor, hevc_dynamic_trim_roll, hevc_dynamic_trim_slip, hevc_dynamic_trim_slide;
    Vp9: vp9_viewer, vp9_trim_editor, vp9_dynamic_trim_roll, vp9_dynamic_trim_slip, vp9_dynamic_trim_slide;
}
