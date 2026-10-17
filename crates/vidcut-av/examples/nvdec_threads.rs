//! The threads and NVDEC decoders a case leaves as `main` returns (Linux):
//! plays a barcode fixture on the GPU in use where it is NVDEC, and lists
//! the process's threads (`/proc/self/task`, with what each waits in) and
//! the driver's libraries loaded (`/proc/self/maps`), before and after.
//! NVDEC's library is loaded while any decoder lives, and each decoder has
//! a thread of the driver's, named after the thread that made it.
//!
//! `cargo run -p vidcut-av --example nvdec_threads -- [CASE] [CODEC] [gpu]`
//!
//! CASE, a player: `play` (dropped while it plays, the default), `idle`
//! (the first picture decoded, nothing played), `seek` (played, then
//! sought far away), `keep` (a picture shown kept past the drop), `ahead`
//! (dropped while the next file opens ahead, then the threads it left
//! joined; `nojoin` leaves them, and the exit hangs). A reader: `reader`
//! (dropped on a thread of its own), `leak` (never dropped), `frame` (a
//! frame of it kept past its drop), `closed` (another dropped and ended on
//! a thread of its own, the first leaked), `closing` (dropped on another
//! thread as `main` returns: the exit hangs, run it under `timeout`).
//! CODEC: `h264` (the default), `hevc`, `av1`. `gpu`: a player's pictures
//! stay in video memory, as the GUI's viewer keeps them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut_av::decode::{Input, VideoReader};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::player::{Config, PacedOutput, Picture, Player};
use vidcut_core::render::{AudioPiece, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

const FRAMES: u32 = 600;

const CASES: [&str; 10] = [
    "play", "idle", "seek", "keep", "ahead", "reader", "leak", "frame", "closed", "closing",
];

type Threads = Vec<(u32, String, String)>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    let case = CASES.into_iter().find(|c| has(c)).unwrap_or("play");
    let (codec, name) = if has("hevc") {
        (VideoCodec::Hevc10, "HEVC")
    } else if has("av1") {
        (VideoCodec::Av1, "AV1")
    } else {
        (VideoCodec::H264, "H.264")
    };
    let Some(gpu) = GpuDecoder::best().filter(|g| g.kind() == Kind::Nvdec) else {
        println!("the GPU decoder in use isn't NVDEC");
        return;
    };
    let spec = |frames| FixtureSpec::new(codec, Container::Mkv, 1280, 720, (30, 1), frames);
    let path = fixture::ensure(&spec(FRAMES).no_audio()).expect("the fixture");
    println!("{case}, {name} on {gpu}");
    let before = threads();
    list("the CUDA device open", &before, &[]);
    match case {
        "reader" => read(&path, &gpu),
        "leak" | "frame" | "closed" | "closing" => at_exit(&path, &gpu, case),
        _ => {
            let next = (case == "ahead")
                .then(|| fixture::ensure(&spec(FRAMES / 2).no_audio()).expect("the fixture"));
            let kept = played(&path, next.as_deref(), case, has("gpu"));
            list("the player dropped", &threads(), &before);
            if case == "ahead" && has("nojoin") {
                // Its opener closes the next file as the process exits.
                return;
            }
            if case == "ahead" {
                vidcut_av::ending::join();
                list("the threads it left ended", &threads(), &before);
            }
            if let Some(picture) = kept {
                drop(picture);
                list("the kept picture gone", &threads(), &before);
            }
        }
    }
    drop(gpu);
    std::thread::sleep(Duration::from_millis(300));
    list("returning from main", &threads(), &before);
}

/// Plays `path`, then `next` if given, as `case` says, drops the player,
/// and gives back the picture kept past it, if any.
fn played(path: &Path, next: Option<&Path>, case: &str, in_video: bool) -> Option<Arc<Picture>> {
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: if in_video {
            vec![Kind::Nvdec]
        } else {
            Vec::new()
        },
        ..Config::default()
    };
    let output = PacedOutput::new(Duration::from_millis(10), Duration::from_millis(20));
    let mut files = vec![(path, i64::from(FRAMES))];
    files.extend(next.map(|p| (p, i64::from(FRAMES / 2))));
    let player = Player::new(plan(&files), config, Box::new(output)).expect("a player");
    let start = if next.is_some() {
        i64::from(FRAMES) - 60
    } else {
        0
    };
    player.seek(start);
    assert!(
        player.wait_ready(start, Duration::from_secs(30)),
        "the first frame"
    );
    let mut kept = None;
    match case {
        "idle" => {}
        "play" | "keep" | "ahead" => {
            player.play(1.0);
            presenting(&player, Duration::from_millis(800));
            kept = (case == "keep").then(|| player.present(Instant::now()).unwrap().picture);
        }
        "seek" => {
            player.play(1.0);
            presenting(&player, Duration::from_millis(500));
            player.seek(i64::from(FRAMES) - 40);
            presenting(&player, Duration::from_millis(300));
        }
        _ => unreachable!(),
    }
    let stats = player.stats();
    println!(
        "dropping the player: {} readers open, {} pictures kept, {} MB in video memory",
        vidcut_av::decode::open_readers(),
        stats.kept,
        stats.video.0 >> 20
    );
    drop(player);
    kept
}

fn presenting(player: &Player, how_long: Duration) {
    let started = Instant::now();
    while started.elapsed() < how_long {
        player.present(Instant::now());
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// A reader on the GPU, on a thread of its own, decoding `frames` frames.
fn reader_on(path: &Path, gpu: &GpuDecoder, frames: usize) -> std::thread::JoinHandle<()> {
    let (path, gpu) = (path.to_path_buf(), gpu.clone());
    std::thread::Builder::new()
        .name("vidcut-test-read".into())
        .spawn(move || {
            let mut reader = VideoReader::open_on(&Input::File(path), &gpu).expect("a reader");
            for _ in 0..frames {
                reader.next_frame().expect("a frame").expect("a frame");
            }
        })
        .expect("a thread")
}

/// Decodes 200 frames of `path` on a thread of its own, dropping the
/// reader there.
fn read(path: &Path, gpu: &GpuDecoder) {
    reader_on(path, gpu, 200).join().expect("the reader");
}

/// A reader as `case` says, NVDEC loaded as `main` returns: `leak`,
/// `frame`, `closed`, `closing` (see the top).
fn at_exit(path: &Path, gpu: &GpuDecoder, case: &str) {
    let mut reader = VideoReader::open_on(&Input::File(path.to_path_buf()), gpu).expect("a reader");
    for _ in 0..20 {
        reader.next_frame().expect("a frame").expect("a frame");
    }
    match case {
        "leak" => std::mem::forget(reader),
        "frame" => {
            let frame = reader.next_frame().expect("a frame").expect("a frame");
            drop(reader);
            std::mem::forget(frame);
        }
        "closed" => {
            std::mem::forget(reader);
            reader_on(path, gpu, 20).join().expect("the reader");
        }
        _ => {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("vidcut-test-close".into())
                .spawn(move || {
                    let _ = rx.recv();
                    drop(reader);
                })
                .expect("a thread");
            list("about to close it", &threads(), &[]);
            let _ = tx.send(());
        }
    }
}

/// A plan of `files` end to end, each whole (its frames given), at 30
/// fps, silent.
fn plan(files: &[(&Path, i64)]) -> RenderPlan {
    let fps = Rational::new(30, 1).unwrap();
    let pieces: Vec<(PathBuf, i64)> = files.iter().map(|(p, n)| (p.to_path_buf(), *n)).collect();
    let frames = pieces.iter().map(|(_, n)| n).sum();
    RenderPlan {
        fps,
        width: 1280,
        height: 720,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video: pieces
            .into_iter()
            .map(|(path, frames)| VideoPiece::Source {
                pace: Default::default(),
                media: path.display().to_string(),
                path,
                origin: 0.0,
                source_frame: 0,
                frames,
                image: None,
            })
            .collect(),
        audio: vec![vec![AudioPiece::Silence {
            samples: fps.frame_to_sample(frames, 48_000),
        }]],
        range: (0, frames),
        name: "threads".into(),
    }
}

/// The process's threads: id, name, what it waits in.
fn threads() -> Threads {
    let mut all: Threads = std::fs::read_dir("/proc/self/task")
        .expect("/proc/self/task")
        .filter_map(|e| {
            let dir: PathBuf = e.ok()?.path();
            let tid = dir.file_name()?.to_str()?.parse().ok()?;
            let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
            Some((tid, read("comm").trim().to_string(), read("wchan")))
        })
        .collect();
    all.sort();
    all
}

/// Says the threads `now` (those not in `before` marked) and NVIDIA's
/// libraries loaded, `when`.
fn list(when: &str, now: &[(u32, String, String)], before: &[(u32, String, String)]) {
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
    let mut libs: Vec<&str> = maps
        .lines()
        .filter_map(|l| l.split_whitespace().nth(5))
        .filter(|l| l.contains("libcuda") || l.contains("libnvcuvid"))
        .filter_map(|l| l.rsplit('/').next())
        .collect();
    libs.sort();
    libs.dedup();
    println!("{when}: {} threads; loaded: {}", now.len(), libs.join(", "));
    for (tid, name, wchan) in now {
        let new = !before.is_empty() && !before.iter().any(|t| t.0 == *tid);
        println!(
            "  {tid} {name:16} {wchan}{}",
            if new { "  (new)" } else { "" }
        );
    }
}
