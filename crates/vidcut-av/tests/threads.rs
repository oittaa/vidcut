//! The threads FFmpeg's readers and graphs start (docs/decisions/threads.md):
//! a single-frame reader none, a graph none once the shared pool runs.
//! Linux only, which lists a process's threads in `/proc`.
#![cfg(target_os = "linux")]

use std::sync::Mutex;

use vidcut_av::decode::Reading;
use vidcut_av::export::renderer::FrameReader;
use vidcut_av::ffmpeg::{filter, format::Pixel, frame};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

/// The tests count the process's threads: one at a time.
static ONE: Mutex<()> = Mutex::new(());

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
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
