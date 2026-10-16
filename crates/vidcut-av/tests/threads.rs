//! The threads FFmpeg's readers and graphs start (docs/decisions/threads.md):
//! a single-frame reader none, a job's stream decoder half the cores', a
//! graph none once the shared pool runs; and the readers open, a file left
//! lately among them. Linux only, which lists a process's threads in
//! `/proc`.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Mutex;

use vidcut_av::decode::{Reading, VideoReader, open_readers, read_barcode};
use vidcut_av::export::renderer::FrameReader;
use vidcut_av::export::video::VideoFrames;
use vidcut_av::ffmpeg::{filter, format::Pixel, frame};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

/// The tests count the process's threads: one at a time.
static ONE: Mutex<()> = Mutex::new(());

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

/// The names of the process's threads FFmpeg named (`av:h264:df0`).
fn ffmpegs_threads() -> Vec<String> {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|t| std::fs::read_to_string(t.ok()?.path().join("comm")).ok())
        .map(|name| name.trim_end().to_string())
        .filter(|name| name.starts_with("av:"))
        .collect()
}

/// A 320×180 H.264 file of `frames` frames at 30 fps, each with its number
/// as a barcode.
fn file_of(frames: u32) -> std::path::PathBuf {
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), frames);
    fixture::ensure(&spec).unwrap()
}

/// `frames` frames of `path` from its frame `source_frame`.
fn piece(path: &Path, source_frame: i64, frames: i64) -> VideoPiece {
    let origin = vidcut_av::probe::probe(path, None)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: path.to_path_buf(),
        origin,
        source_frame,
        frames,
        image: None,
    }
}

#[test]
fn a_reader_counts_while_it_is_open() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 1280, 720, (30, 1), 60);
    let path = fixture::ensure(&spec).unwrap();
    let before = open_readers();
    let reader = VideoReader::open(&path).unwrap();
    assert_eq!(open_readers(), before + 1);
    drop(reader);
    assert_eq!(open_readers(), before);
}

#[test]
fn a_single_frame_reader_starts_no_thread() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 1280, 720, (30, 1), 60);
    let path = fixture::ensure(&spec).unwrap();
    let origin = vidcut_av::probe::probe(&path, None)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    let piece = VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path,
        origin,
        source_frame: 30,
        frames: 1,
        image: None,
    };
    let fps = Rational::new(30, 1).unwrap();
    let before = threads();
    let mut reader = FrameReader::default();
    reader.rgba(&piece, fps, 160, 90).unwrap();
    // The reader stays open with its decoder and graph.
    assert!(
        threads() <= before,
        "{} threads, {before} before",
        threads()
    );
}

#[test]
fn a_jobs_stream_decoder_takes_half_the_cores() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 1280, 720, (30, 1), 60);
    let path = fixture::ensure(&spec).unwrap();
    let cores = std::thread::available_parallelism().unwrap().get();
    // At most 16, FFmpeg's automatic most; one thread starts none.
    let expected = match (cores / 2).clamp(1, 16) {
        1 => 0,
        n => n,
    };
    let (before, after) = std::thread::spawn(move || {
        vidcut_av::first::yield_to_players();
        let before = ffmpegs_threads();
        let _reader = VideoReader::open(&path).unwrap();
        (before, ffmpegs_threads())
    })
    .join()
    .unwrap();
    assert_eq!(
        after.len() - before.len(),
        expected,
        "{cores} cores: {after:?}, {before:?} before"
    );
}

#[test]
fn a_graph_runs_its_slices_on_the_pool() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    vidcut_av::init();
    vidcut_av::pool::run(2, &|_| {});
    let before = threads();
    let mut graph = vidcut_av::pool::graph(Reading::Stream);
    let args = format!(
        "video_size=640x360:pix_fmt={}:time_base=1/30:pixel_aspect=1/1",
        vidcut_av::ffmpeg::ffi::AVPixelFormat::from(Pixel::YUV420P) as i32
    );
    graph
        .add(&filter::find("buffer").unwrap(), "in", &args)
        .unwrap();
    graph
        .add(&filter::find("buffersink").unwrap(), "out", "")
        .unwrap();
    // `transpose` cuts its work into slices.
    graph
        .output("in", 0)
        .unwrap()
        .input("out", 0)
        .unwrap()
        .parse("transpose=clock")
        .unwrap();
    graph.validate().unwrap();
    let mut picture = frame::Video::new(Pixel::YUV420P, 640, 360);
    picture.set_pts(Some(0));
    graph.get("in").unwrap().source().add(&picture).unwrap();
    let mut turned = frame::Video::empty();
    graph.get("out").unwrap().sink().frame(&mut turned).unwrap();
    assert_eq!((turned.width(), turned.height()), (360, 640));
    assert!(
        threads() <= before,
        "{} threads, {before} before",
        threads()
    );
}

#[test]
fn a_file_left_stays_open_for_a_later_piece_of_it() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let (first, second) = (file_of(60), file_of(90));
    let fps = Rational::new(30, 1).unwrap();
    let mut frames = VideoFrames::new(&[], fps, 320, 180, Pixel::YUV420P, None);
    let before = open_readers();
    // A playhead dragged across a cut and back: a frame at a time.
    let mut show = |path: &Path, f| {
        frames.read_piece(&piece(path, f, 1));
        read_barcode(&frames.next_frame().unwrap().unwrap()).unwrap()
    };
    assert_eq!(show(&first, 10), Some(10));
    assert_eq!(open_readers(), before + 1);
    assert_eq!(show(&second, 20), Some(20));
    // The first file waits, open.
    assert_eq!(open_readers(), before + 2);
    assert_eq!(show(&first, 40), Some(40));
    // Taken back: no reader opened for it.
    assert_eq!(open_readers(), before + 2);
    drop(frames);
    assert_eq!(open_readers(), before);
}

#[test]
fn playing_across_a_cutaway_opens_each_file_once() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let (first, second) = (file_of(60), file_of(90));
    let fps = Rational::new(30, 1).unwrap();
    // A cutaway: the first file, the second, the first again further on.
    let pieces = [
        piece(&first, 10, 3),
        piece(&second, 20, 3),
        piece(&first, 50, 3),
    ];
    let mut frames = VideoFrames::new(&pieces, fps, 320, 180, Pixel::YUV420P, None);
    let before = open_readers();
    let mut codes = Vec::new();
    while let Some(f) = frames.next_frame().unwrap() {
        codes.push(read_barcode(&f).unwrap());
    }
    assert_eq!(codes, [10, 11, 12, 20, 21, 22, 50, 51, 52].map(Some));
    // The first file, sought ahead rather than opened again, and the
    // second, left.
    assert_eq!(open_readers(), before + 2);
}
