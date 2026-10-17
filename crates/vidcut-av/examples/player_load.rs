//! Plays PROJECT's timeline with the real player as the GUI does: a device
//! paced in real time, and a viewer presenting 60 times a second. First
//! alone, then beside the load the GUI puts next to it: four filmstrip
//! workers rendering tiles, and a job reading the timeline at 1280x720 (a
//! proxy's reading). The load runs on threads at the lowest priority that
//! yield to players, as the app's jobs do (the idle class on Linux only).
//!
//! For each run: the presents that showed the picture before the clock's
//! frame, the player's late presents and skipped frames and the pictures it
//! kept, what the load got
//! done, the process's threads at most (Linux) and its peak memory; and
//! between them, the load alone for as long: its tiles and frames a second. A
//! first run, not counted, reads what the others play into the system's
//! file cache; the filmstrip and the job go round short parts of their own,
//! so every run reads from memory, not the disk (4K PNG frames are 40 MB).
//!
//! `cargo run --release -p vidcut-av --example player_load -- PROJECT [SECONDS] [gpu]`

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use vidcut_av::export::renderer::FrameReader;
use vidcut_av::export::video::VideoFrames;
use vidcut_av::ffmpeg::format::Pixel;
use vidcut_av::player::{Config, PacedOutput, Player};
use vidcut_core::model::Project;
use vidcut_core::render::flatten_lanes;

/// Where the player starts, and the filmstrip's and the job's parts.
const PLAY_FROM: i64 = 600;
const TILES_FROM: i64 = 4000;
/// Each filmstrip worker's part: four tiles 2 s apart.
const TILES_EACH: i64 = 480;
const JOB_FROM: i64 = 8000;
const JOB_FRAMES: i64 = 200;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("PROJECT"));
    let rest: Vec<String> = args.collect();
    let gpu = rest.iter().any(|a| a == "gpu");
    let seconds: u64 = rest
        .iter()
        .find(|a| *a != "gpu")
        .map_or(5, |s| s.parse().expect("SECONDS"));
    let store = vidcut_core::store::Store::open(&path).expect("project");
    let project = store.load().expect("project").project().clone();
    let (plan, _) = flatten_lanes(&project, 0, 1).expect("a plan");
    println!(
        "{}: {}x{} at {} fps, {seconds} s from frame {PLAY_FROM}, decoding on {}",
        path.display(),
        plan.width,
        plan.height,
        plan.fps.as_f64(),
        if gpu {
            "the GPU where it can"
        } else {
            "the CPU"
        }
    );
    play(&project, gpu, seconds);
    let played = play(&project, gpu, seconds);
    println!("alone:  {played}");

    let stop = Arc::new(AtomicBool::new(false));
    let tiles = Arc::new(AtomicUsize::new(0));
    let job_frames = Arc::new(AtomicUsize::new(0));
    let mut load = Vec::new();
    for n in 0..4 {
        let (project, stop, tiles) = (project.clone(), stop.clone(), tiles.clone());
        load.push(std::thread::spawn(move || {
            as_a_job();
            filmstrip(&project, n, gpu, &stop, &tiles)
        }));
    }
    {
        let (project, stop, job_frames) = (project.clone(), stop.clone(), job_frames.clone());
        load.push(std::thread::spawn(move || {
            as_a_job();
            job(&project, &stop, &job_frames)
        }));
    }
    // The load once round its parts, read into the file cache, then alone
    // for as long as the player plays.
    let warming = Instant::now();
    while (tiles.load(Ordering::SeqCst) < 4 * (TILES_EACH / 120) as usize
        || job_frames.load(Ordering::SeqCst) < JOB_FRAMES as usize)
        && warming.elapsed() < Duration::from_secs(300)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    let (tiles_before, job_before) = (
        tiles.load(Ordering::SeqCst),
        job_frames.load(Ordering::SeqCst),
    );
    let mut threads = 0;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        threads = threads.max(threads_now());
        std::thread::sleep(Duration::from_millis(100));
    }
    let secs = started.elapsed().as_secs_f64();
    println!(
        "load:   {:.1} tiles a second, the job {:.1} frames a second; threads at most {threads}, \
         peak memory {} MB",
        (tiles.load(Ordering::SeqCst) - tiles_before) as f64 / secs,
        (job_frames.load(Ordering::SeqCst) - job_before) as f64 / secs,
        peak_memory_mb()
    );
    let tiles_before = tiles.load(Ordering::SeqCst);
    let job_before = job_frames.load(Ordering::SeqCst);
    let played = play(&project, gpu, seconds);
    let tiles_done = tiles.load(Ordering::SeqCst) - tiles_before;
    let job_done = job_frames.load(Ordering::SeqCst) - job_before;
    stop.store(true, Ordering::SeqCst);
    for t in load {
        let _ = t.join();
    }
    println!("loaded: {played}");
    println!("        meanwhile the load made {tiles_done} tiles and the job {job_done} frames");
}

/// What a run showed.
struct Played {
    presents: usize,
    behind: usize,
    late: u64,
    skipped: u64,
    /// Pictures the player kept at the end, and their megabytes.
    kept: usize,
    kept_mb: usize,
    threads: usize,
    peak_mb: u64,
}

impl std::fmt::Display for Played {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} presents, {} of them the picture before ({:.1} %); the player's late {}, \
             skipped {}; {} pictures kept ({} MB); threads at most {}, peak memory {} MB",
            self.presents,
            self.behind,
            100.0 * self.behind as f64 / self.presents.max(1) as f64,
            self.late,
            self.skipped,
            self.kept,
            self.kept_mb,
            self.threads,
            self.peak_mb
        )
    }
}

/// Plays `seconds` from [`PLAY_FROM`], presenting as the viewer does.
fn play(project: &Project, gpu: bool, seconds: u64) -> Played {
    let fps = flatten_lanes(project, 0, 1).expect("a plan").0.fps;
    let frames = (seconds as f64 * fps.as_f64()).ceil() as i64 + 60;
    let (plan, _) = flatten_lanes(project, PLAY_FROM, PLAY_FROM + frames).expect("a plan");
    let config = Config {
        audio_scrubbing: false,
        hardware: gpu,
        ..Config::default()
    };
    let output = PacedOutput::new(Duration::from_millis(10), Duration::from_millis(20));
    let player = Player::new(plan, config, Box::new(output)).expect("a player");
    assert!(
        player.wait_ready(PLAY_FROM, Duration::from_secs(30)),
        "the first frame"
    );
    player.play(1.0);
    let (mut presents, mut behind, mut threads) = (0, 0, 0);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        if let Some(shown) = player.present(Instant::now()) {
            presents += 1;
            behind += usize::from(!shown.exact);
        }
        threads = threads.max(threads_now());
        std::thread::sleep(Duration::from_micros(16_667));
    }
    let stats = player.stats();
    Played {
        presents,
        behind,
        late: stats.late,
        skipped: stats.skipped,
        kept: stats.kept,
        kept_mb: (stats.memory.0 + stats.video.0) >> 20,
        threads,
        peak_mb: peak_memory_mb(),
    }
}

/// Filmstrip worker `n`: 160x90 tiles 2 s apart, each worker on a part of
/// its own, again and again.
fn filmstrip(project: &Project, n: i64, gpu: bool, stop: &AtomicBool, done: &AtomicUsize) {
    let mut reader = match gpu.then(vidcut_av::hw::GpuDecoder::best).flatten() {
        Some(d) => FrameReader::decoding_on(d),
        None => FrameReader::default(),
    };
    let first = TILES_FROM + n * TILES_EACH;
    let mut at = first;
    while !stop.load(Ordering::SeqCst) {
        let Ok((plan, _)) = flatten_lanes(project, at, at + 1) else {
            at = first;
            continue;
        };
        if let Some(piece) = plan.video.first() {
            let _ = reader.rgba(piece, plan.fps, 160, 90);
            done.fetch_add(1, Ordering::SeqCst);
        }
        at += 120;
        if at >= first + TILES_EACH {
            at = first;
        }
    }
}

/// A proxy's reading: [`JOB_FRAMES`] of the timeline from [`JOB_FROM`] at
/// 1280x720, again and again.
fn job(project: &Project, stop: &AtomicBool, done: &AtomicUsize) {
    let (plan, _) = flatten_lanes(project, JOB_FROM, JOB_FROM + JOB_FRAMES).expect("a plan");
    while !stop.load(Ordering::SeqCst) {
        let mut reader = VideoFrames::new(&plan.video, plan.fps, 1280, 720, Pixel::YUV420P, None);
        while !stop.load(Ordering::SeqCst) {
            match reader.next_frame() {
                Ok(Some(_)) => {
                    done.fetch_add(1, Ordering::SeqCst);
                }
                _ => break,
            }
        }
    }
}

/// This thread as the app's jobs are: the lowest priority (the idle class,
/// which the threads it starts inherit; Linux only here), its decoding
/// waiting while a player plays.
fn as_a_job() {
    #[cfg(target_os = "linux")]
    // SAFETY: a plain system call on this thread's own id.
    unsafe {
        let param = libc::sched_param { sched_priority: 0 };
        libc::sched_setscheduler(libc::gettid(), libc::SCHED_IDLE, &param);
    }
    vidcut_av::first::yield_to_players();
}

/// The process's threads now (Linux; 0 elsewhere).
fn threads_now() -> usize {
    std::fs::read_dir("/proc/self/task").map_or(0, |d| d.count())
}

/// The process's peak resident memory so far (Unix; 0 elsewhere).
#[cfg(unix)]
fn peak_memory_mb() -> u64 {
    // SAFETY: getrusage fills the struct given.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    // Kilobytes on Linux, bytes on macOS.
    let peak = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        peak >> 20
    } else {
        peak >> 10
    }
}

#[cfg(not(unix))]
fn peak_memory_mb() -> u64 {
    0
}
