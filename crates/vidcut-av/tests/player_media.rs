//! The player on real footage, decoding on the CPU, and with
//! `--features gpu-tests` on NVDEC too: how long a seek takes to its
//! picture, steps back, and whether 1080p30 and 2160p60 H.264 play in real
//! time, with the CPU they take; and what one player for a viewer's source
//! and timeline costs in its one memory budget. Prints the numbers
//! (`--nocapture`); skips without the clips (scripts/fetch-test-media.sh).
//! `VIDCUT_PLAYER_MEDIA=a.mkv,b.mp4` measures those files instead, and
//! `VIDCUT_PLAYER_BUDGET_MB` another budget.

mod player_support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vidcut_av::cpu_seconds;
use vidcut_av::player::{Budget, Config, DEFAULT_BUDGET, PacedOutput, Player};
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
    plan_from(path, fps, 0, secs)
}

/// `secs` of the file from source frame `from` (as far as it goes).
fn plan_from(path: &Path, fps: i64, from: i64, secs: i64) -> RenderPlan {
    let info = vidcut_av::probe::probe(path).unwrap();
    let origin = info.video.as_ref().unwrap().first_frame_time.unwrap();
    let fps = Rational::new(fps, 1).unwrap();
    let left = info.duration.unwrap_or(f64::MAX) - 0.5 - fps.frames_to_secs(from);
    let frames = fps.secs_to_frames((secs as f64).min(left));
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
            source_frame: from,
            frames,
            image: None,
        }],
        audio: vec![vec![AudioPiece::Source {
            media: "m1".into(),
            path: path.to_path_buf(),
            stream: 0,
            origin,
            source_sample: fps.frame_to_sample(from, 48_000),
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

/// A clip measured: its name, its file when it is there, its frame rate.
type Clip = (String, Option<PathBuf>, i64);

/// The clips measured, with their frame rates: the real 1080p30 and
/// 2160p60 ones, or `VIDCUT_PLAYER_MEDIA`'s; each with the decoders (true:
/// NVDEC).
fn clips() -> Vec<(Clip, bool)> {
    let mut clips = vec![
        (
            "bbb-1080p30".to_string(),
            media("bbb-1080p30", "bbb_sunflower_1080p_30fps_normal.mp4"),
            30,
        ),
        (
            "bbb-2160p60".to_string(),
            media("bbb-2160p60", "bbb_sunflower_2160p_60fps_normal.mp4"),
            60,
        ),
    ];
    if let Ok(files) = std::env::var("VIDCUT_PLAYER_MEDIA") {
        clips = files
            .split(',')
            .map(|f| {
                let path = PathBuf::from(f);
                let info = vidcut_av::probe::probe(&path).unwrap();
                let rate = info.video.as_ref().unwrap().avg_frame_rate;
                let fps = (f64::from(rate.0) / f64::from(rate.1)).round() as i64;
                (path.display().to_string(), Some(path), fps)
            })
            .collect();
    }
    let decoders: &[bool] = if cfg!(feature = "gpu-tests") {
        &[false, true]
    } else {
        &[false]
    };
    clips
        .iter()
        .flat_map(|c| decoders.iter().map(move |&d| (c.clone(), d)))
        .collect()
}

/// The memory budget: `VIDCUT_PLAYER_BUDGET_MB`, or the default.
fn budget() -> usize {
    std::env::var("VIDCUT_PLAYER_BUDGET_MB")
        .ok()
        .and_then(|mb| mb.parse::<usize>().ok())
        .map_or(DEFAULT_BUDGET.memory, |mb| mb << 20)
}

#[test]
fn real_footage_seeks_steps_and_plays() {
    for ((name, path, fps), hardware) in clips() {
        let Some(path) = path else {
            eprintln!("skipping {name}: not in the test data (scripts/fetch-test-media.sh)");
            continue;
        };
        let name = format!("{name} ({})", if hardware { "NVDEC" } else { "CPU" });
        let p = plan(&path, fps, 540);
        let frames = p.frames;
        // Sound to a stand-in device in real time.
        let out = PacedOutput::new(Duration::from_millis(10), Duration::ZERO);
        let budget = budget();
        let config = Config {
            budget: Budget::both(budget),
            hardware,
            ..Config::default()
        };
        let player = Player::new(p, config, Box::new(out)).unwrap();
        assert!(player.wait_ready(0, Duration::from_secs(20)));
        let pic = player.present(Instant::now()).unwrap().picture;
        eprintln!(
            "{name}: pictures {}×{} {:?}, {:.1} MB each; a window back holds up to {} (half the budget)",
            pic.width(),
            pic.height(),
            pic.layout(),
            pic.bytes() as f64 / 1e6,
            budget / 2 / pic.bytes()
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
        eprintln!(
            "{name}: forward {forward}; decoded on {:?}",
            player.status().decoder
        );
        let backward = play(&player, frames / 3, -1.0, 3.0);
        eprintln!(
            "{name}: backward {backward}; decoded on {:?}",
            player.status().decoder
        );
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

/// One player for a viewer's source and timeline (preview.md §1), and its
/// one memory budget. The timeline is stepped back through and played
/// backward, then the viewer switches to the source and back: right away,
/// as an engine of the timeline's own would have kept it, and after the
/// source played 5 s forward and 3 s backward elsewhere in the file. Each
/// time: how soon each frame shows after switching (the source's a seek
/// at first, the timeline's prepared), the first step back and those
/// after it, 3 s backward again, and the memory kept.
#[test]
fn one_player_for_the_source_and_the_timeline() {
    for ((name, path, fps), hardware) in clips() {
        let Some(path) = path else {
            eprintln!("skipping {name}: not in the test data (scripts/fetch-test-media.sh)");
            continue;
        };
        let name = format!("{name} ({})", if hardware { "NVDEC" } else { "CPU" });
        let timeline = plan(&path, fps, 540);
        let frames = timeline.frames;
        let source = plan_from(&path, fps, frames * 2 / 3 + fps * 7 / 3, 60);
        let budget = budget();
        for source_plays in [false, true] {
            let config = Config {
                budget: Budget::both(budget),
                hardware,
                audio_scrubbing: false,
                ..Config::default()
            };
            let out = PacedOutput::new(Duration::from_millis(10), Duration::ZERO);
            let player = Player::new(timeline.clone(), config, Box::new(out)).unwrap();
            // Editing on the timeline: steps back from the middle of a GOP,
            // then 3 s backward.
            let t = frames / 3;
            player.seek(t);
            assert!(player.wait_ready(t, Duration::from_secs(20)));
            for k in 1..=40 {
                player.step(-1);
                assert!(player.wait_ready(t - k, Duration::from_secs(20)));
                player.present(Instant::now());
            }
            play(&player, t, -1.0, 3.0);
            // Where it stopped, shown (backward 4K on NVDEC runs behind):
            // the picture a person sees before switching.
            let at = player.status().position;
            assert!(player.wait_ready(at, Duration::from_secs(20)));
            // To the source: a seek, the timeline's frame held.
            let started = Instant::now();
            player.prepare(Some((timeline.clone(), at)));
            let s = source.frames / 2;
            player.set_plan_at(source.clone(), s).unwrap();
            assert!(player.wait_ready(s, Duration::from_secs(20)));
            let to_source = started.elapsed();
            if source_plays {
                play(&player, s, 1.0, 5.0);
                play(&player, s, -1.0, 3.0);
            }
            let kept = player.status().kept;
            // Back to the timeline.
            let started = Instant::now();
            player.prepare(Some((source.clone(), player.status().position)));
            player.set_plan_at(timeline.clone(), at).unwrap();
            assert!(
                player.status().ready,
                "{name}: the timeline's frame was held"
            );
            assert!(player.wait_ready(at, Duration::from_secs(20)));
            let to_timeline = started.elapsed();
            let mut steps = Vec::new();
            for k in 1..=40 {
                let started = Instant::now();
                player.step(-1);
                assert!(player.wait_ready(at - k, Duration::from_secs(20)));
                steps.push(ms(started.elapsed()));
                // Shown, as a display shows it.
                player.present(Instant::now());
            }
            let backward = play(&player, at - 40, -1.0, 3.0);
            eprintln!(
                "{name}, {} MB: {}: switched to the source in {:.0} ms, back in {:.2} ms; \
                 {} pictures kept ({:.0} MB); then a step back {:.0} ms, the next 39 median \
                 {:.1} ms, max {:.1} ms; backward {backward}",
                budget >> 20,
                if source_plays {
                    "after the source played 8 s"
                } else {
                    "right away"
                },
                ms(to_source),
                ms(to_timeline),
                kept.0,
                kept.1 as f64 / 1e6,
                steps[0],
                median(steps[1..].to_vec()),
                steps[1..].iter().cloned().fold(0.0, f64::max),
            );
            assert!(player.status().error.is_none(), "{:?}", player.status());
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
