//! Times the player's reading under the load the GUI puts beside it: one
//! thread reads PROJECT's timeline frame by frame at the project's size, as
//! the player's decode thread does, while four filmstrip workers render
//! tiles of it and a job (a proxy's reading) reads it at 1280x720. The load
//! runs at the lowest priority, as the app's jobs do (Linux only here).
//! Prints the player's milliseconds a frame alone, then under the load.
//!
//! `cargo run --release -p vidcut-av --example player_load -- PROJECT [FRAMES]`

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use vidcut_av::export::renderer::FrameReader;
use vidcut_av::export::video::VideoFrames;
use vidcut_av::ffmpeg::format::Pixel;
use vidcut_core::model::Project;
use vidcut_core::render::flatten_lanes;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("PROJECT"));
    let frames: i64 = args.next().map_or(300, |n| n.parse().expect("FRAMES"));
    let store = vidcut_core::store::Store::open(&path).expect("project");
    let project = store.load().expect("project").project().clone();
    let (plan, _) = flatten_lanes(&project, 0, 1).expect("a plan");
    println!(
        "{}: {}x{} at {} fps, {frames} frames",
        path.display(),
        plan.width,
        plan.height,
        plan.fps.as_f64()
    );
    let alone = play(&project, 600, frames);
    println!("alone:     {}", summary(&alone));
    let stop = Arc::new(AtomicBool::new(false));
    let tiles = Arc::new(AtomicUsize::new(0));
    let mut load = Vec::new();
    for n in 0..4 {
        let (project, stop, tiles) = (project.clone(), stop.clone(), tiles.clone());
        load.push(std::thread::spawn(move || {
            lowest_priority();
            filmstrip(&project, n, &stop, &tiles)
        }));
    }
    let job_frames = Arc::new(AtomicUsize::new(0));
    {
        let (project, stop, job_frames) = (project.clone(), stop.clone(), job_frames.clone());
        load.push(std::thread::spawn(move || {
            lowest_priority();
            // A job's count, as the app's jobs take it.
            vidcut_av::threads::set(vidcut_av::threads::Threads::Quarter);
            job(&project, &stop, &job_frames)
        }));
    }
    // The load under way.
    std::thread::sleep(Duration::from_secs(2));
    let started = Instant::now();
    let (tiles_before, job_before) = (
        tiles.load(Ordering::SeqCst),
        job_frames.load(Ordering::SeqCst),
    );
    let loaded = play(&project, 600 + frames, frames);
    let secs = started.elapsed().as_secs_f64();
    let tiles_done = tiles.load(Ordering::SeqCst) - tiles_before;
    let job_done = job_frames.load(Ordering::SeqCst) - job_before;
    stop.store(true, Ordering::SeqCst);
    for t in load {
        let _ = t.join();
    }
    println!("loaded:    {}", summary(&loaded));
    println!(
        "meanwhile: {:.1} tiles a second, the job {:.1} frames a second",
        tiles_done as f64 / secs,
        job_done as f64 / secs
    );
}

/// Each frame's milliseconds, reading `frames` frames from `from` at the
/// project's size.
fn play(project: &Project, from: i64, frames: i64) -> Vec<f64> {
    let (plan, _) = flatten_lanes(project, from, from + frames).expect("a plan");
    let mut reader = VideoFrames::new(
        &plan.video,
        plan.fps,
        plan.width,
        plan.height,
        Pixel::YUV420P10LE,
        None,
    );
    let mut times = Vec::new();
    loop {
        let t = Instant::now();
        match reader.next_frame().expect("a frame") {
            Some(_) => times.push(t.elapsed().as_secs_f64() * 1e3),
            None => return times,
        }
    }
}

/// Filmstrip worker `n`: 160x90 tiles 2 s apart, each worker on a part of
/// its own, again and again.
fn filmstrip(project: &Project, n: i64, stop: &AtomicBool, done: &AtomicUsize) {
    let mut reader = FrameReader::default();
    let mut at = n * 2000;
    while !stop.load(Ordering::SeqCst) {
        let Ok((plan, _)) = flatten_lanes(project, at, at + 1) else {
            at = n * 2000;
            continue;
        };
        if let Some(piece) = plan.video.first() {
            let _ = reader.rgba(piece, plan.fps, 160, 90);
            done.fetch_add(1, Ordering::SeqCst);
        }
        at += 120;
        if at >= (n + 1) * 2000 {
            at = n * 2000;
        }
    }
}

/// A proxy's reading: the timeline from 8000 at 1280x720, frame by frame.
fn job(project: &Project, stop: &AtomicBool, done: &AtomicUsize) {
    let (plan, _) = flatten_lanes(project, 8000, 10000).expect("a plan");
    let mut reader = VideoFrames::new(&plan.video, plan.fps, 1280, 720, Pixel::YUV420P, None);
    while !stop.load(Ordering::SeqCst) {
        match reader.next_frame() {
            Ok(Some(_)) => {
                done.fetch_add(1, Ordering::SeqCst);
            }
            _ => return,
        }
    }
}

/// The idle scheduling class, as the app gives its jobs: the threads FFmpeg
/// starts from here inherit it.
fn lowest_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: a plain system call on this thread's own id.
    unsafe {
        let param = libc::sched_param { sched_priority: 0 };
        libc::sched_setscheduler(libc::gettid(), libc::SCHED_IDLE, &param);
    }
}

/// Mean, median, 95th percentile and worst.
fn summary(times: &[f64]) -> String {
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    format!(
        "{} frames, mean {:.1} ms, median {:.1}, p95 {:.1}, worst {:.1}",
        times.len(),
        times.iter().sum::<f64>() / times.len() as f64,
        at(0.5),
        at(0.95),
        at(1.0)
    )
}
