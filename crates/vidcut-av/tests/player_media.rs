//! The player on real footage, decoding on the CPU: how long a seek takes
//! to its picture, steps back, and whether 1080p30 and 2160p60 H.264 play
//! in real time, with the CPU they take. Prints the numbers
//! (`--nocapture`); skips without the clips (scripts/fetch-test-media.sh).

mod player_support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use player_support::cpu_seconds;
use vidcut_av::player::{Config, DEFAULT_BUDGET, PacedOutput, Player};
use vidcut_core::render::{AudioPiece, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

fn media(name: &str, file: &str) -> Option<PathBuf> {
    let path = vidcut_av::testdata::root()
        .join("media")
        .join(name)
        .join(file);
    path.exists().then_some(path)
}

fn plan(path: &Path, fps: i64, secs: i64) -> RenderPlan {
    let info = vidcut_av::probe::probe(path).unwrap();
    let origin = info.video.as_ref().unwrap().first_frame_time.unwrap();
    let fps = Rational::new(fps, 1).unwrap();
    let frames = fps.secs_to_frames(secs as f64);
    let samples = fps.frame_to_sample(frames, 48_000);
    RenderPlan {
        fps,
        width: 1920,
        height: 1080,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video: vec![VideoPiece::Source {
            media: "m1".into(),
            path: path.to_path_buf(),
            origin,
            source_frame: 0,
            frames,
            image: None,
        }],
        audio: vec![vec![AudioPiece::Source {
            media: "m1".into(),
            path: path.to_path_buf(),
            stream: 0,
            origin,
            source_sample: 0,
            samples,
            gain_db: 0.0,
        }]],
        range: (0, frames),
        name: "media".into(),
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
fn real_footage_seeks_steps_and_plays() {
    let clips = [
        ("bbb-1080p30", "bbb_sunflower_1080p_30fps_normal.mp4", 30),
        ("bbb-2160p60", "bbb_sunflower_2160p_60fps_normal.mp4", 60),
    ];
    for (name, file, fps) in clips {
        let Some(path) = media(name, file) else {
            eprintln!("skipping {name}: not in the test data (scripts/fetch-test-media.sh)");
            continue;
        };
        let p = plan(&path, fps, 540);
        let frames = p.frames;
        // Sound to a stand-in device in real time.
        let out = PacedOutput::new(Duration::from_millis(10), Duration::ZERO);
        // `VIDCUT_PLAYER_BUDGET_MB` measures another memory budget.
        let budget = std::env::var("VIDCUT_PLAYER_BUDGET_MB")
            .ok()
            .and_then(|mb| mb.parse::<usize>().ok())
            .map_or(DEFAULT_BUDGET, |mb| mb << 20);
        let config = Config {
            budget,
            ..Config::default()
        };
        let player = Player::new(p, config, Box::new(out)).unwrap();
        assert!(player.wait_ready(0, Duration::from_secs(20)));
        let pic = player.present(Instant::now()).unwrap().picture;
        eprintln!(
            "{name}: pictures {}×{} {:?}, {:.1} MB each; a window back holds up to {} (a third of the budget)",
            pic.width(),
            pic.height(),
            pic.layout(),
            pic.bytes() as f64 / 1e6,
            budget / 3 / pic.bytes()
        );

        // Seeks, paused, each waited for.
        let mut seeks = Vec::new();
        for k in 1..=12 {
            let t = (k * 7919 * fps) % frames;
            let start = Instant::now();
            player.seek(t);
            assert!(player.wait_ready(t, Duration::from_secs(20)), "{name} {t}");
            seeks.push(ms(start.elapsed()));
        }

        // Steps back from the middle of a GOP: the first decodes a window,
        // the rest are kept.
        player.seek(frames / 2);
        assert!(player.wait_ready(frames / 2, Duration::from_secs(20)));
        let mut steps = Vec::new();
        for k in 1..=40 {
            let start = Instant::now();
            player.step(-1);
            assert!(player.wait_ready(frames / 2 - k, Duration::from_secs(20)));
            steps.push(ms(start.elapsed()));
        }

        eprintln!(
            "{name}: seek to picture median {:.0} ms, max {:.0} ms; step back first {:.0} ms, \
             then median {:.1} ms, max {:.1} ms",
            median(seeks.clone()),
            seeks.iter().cloned().fold(0.0, f64::max),
            steps[0],
            median(steps[1..].to_vec()),
            steps[1..].iter().cloned().fold(0.0, f64::max),
        );

        // Five seconds of play forward, three backward.
        let forward = play(&player, frames / 3, 1.0, 5.0);
        eprintln!("{name}: forward {forward}");
        let backward = play(&player, frames / 3, -1.0, 3.0);
        eprintln!("{name}: backward {backward}");
        for p in [&forward, &backward] {
            assert!(
                p.due as f64 >= 0.95 * p.secs * fps as f64,
                "{name}: the clock ran slow: {p}"
            );
        }
        assert!(player.status().error.is_none(), "{:?}", player.status());
        if fps == 30 {
            // A 60 Hz display sees each 30 fps frame twice: every one of
            // them shows.
            assert!(
                forward.shown as f64 >= 0.95 * forward.due as f64,
                "{name}: {forward}"
            );
        }
    }
}

/// What a display saw of a stretch of play.
struct Played {
    secs: f64,
    /// Frames the clock went through, and how many of them were shown.
    due: i64,
    shown: usize,
    /// Presents, and those that showed the clock's own frame.
    presents: u32,
    exact: u32,
    late: u64,
    skipped: u64,
    /// From play until the picture changed.
    moved: Option<Duration>,
    cpu: f64,
}

impl std::fmt::Display for Played {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} frames in {:.2} s: {} shown, {} of {} presents on time ({} late), {} skipped; \
             the picture moved after {:.0} ms; {:.1} CPU-seconds ({:.1} cores)",
            self.due,
            self.secs,
            self.shown,
            self.exact,
            self.presents,
            self.late,
            self.skipped,
            self.moved.map_or(f64::NAN, ms),
            self.cpu,
            self.cpu / self.secs,
        )
    }
}

/// Plays from `from` at `speed` for `secs`, a display presenting at 60 Hz.
fn play(player: &Player, from: i64, speed: f64, secs: f64) -> Played {
    player.pause();
    player.seek(from);
    assert!(player.wait_ready(from, Duration::from_secs(20)));
    let before = player.status();
    let (cpu, wall) = (cpu_seconds(), Instant::now());
    player.play(speed);
    let mut moved = None;
    let mut shown = std::collections::BTreeSet::new();
    let (mut exact, mut presents) = (0, 0);
    while wall.elapsed() < Duration::from_secs_f64(secs) {
        if let Some(s) = player.present(Instant::now()) {
            presents += 1;
            if s.exact {
                exact += 1;
                shown.insert(s.frame);
            }
            if s.frame != from && moved.is_none() {
                moved = Some(wall.elapsed());
            }
        }
        std::thread::sleep(Duration::from_micros(16_667));
    }
    let status = player.status();
    let (cpu, secs) = (cpu_seconds() - cpu, wall.elapsed().as_secs_f64());
    player.pause();
    Played {
        secs,
        due: (status.position - from).abs() + 1,
        shown: shown.len(),
        presents,
        exact,
        late: status.late - before.late,
        skipped: status.skipped - before.skipped,
        moved,
        cpu,
    }
}
