//! The frame renderer behind the paused viewer and the two-up
//! (docs/decisions/preview.md §9): every picture exact, and each frame
//! decoded about once going back as well as forward. One test, so the
//! process-wide decode count is this test's alone.

use vidcut_av::decode::{frames_decoded, read_barcode};
use vidcut_av::export::renderer::StillRenderer;
use vidcut_av::ffmpeg::{format::Pixel, frame};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::thumbs::Image;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

struct Src {
    path: std::path::PathBuf,
    origin: f64,
}

fn src(spec: FixtureSpec) -> Src {
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    Src { path, origin }
}

fn v(s: &Src, source_frame: i64) -> VideoPiece {
    VideoPiece::Source {
        media: "m".into(),
        path: s.path.clone(),
        origin: s.origin,
        source_frame,
        frames: 1,
        image: None,
    }
}

/// The barcode in a rendered picture.
fn code(img: &Image) -> Option<u32> {
    let mut f = frame::Video::new(Pixel::RGBA, img.width, img.height);
    let stride = f.stride(0);
    let row = img.width as usize * 4;
    for y in 0..img.height as usize {
        f.data_mut(0)[y * stride..y * stride + row]
            .copy_from_slice(&img.rgba[y * row..(y + 1) * row]);
    }
    read_barcode(&f).unwrap()
}

#[test]
fn frames_decode_once_both_ways() {
    let fps = Rational::new(30, 1).unwrap();
    // Keyframes 120 frames apart, with B-frames.
    let long = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 300)
            .gop(120)
            .bframes(2),
    );

    // Forward a frame at a time decodes one frame a step.
    let mut r = StillRenderer::default();
    assert_eq!(
        code(&r.rgba(&v(&long, 10), 0, fps, 320, 180).unwrap()),
        Some(10)
    );
    let before = frames_decoded();
    for f in 11..=60 {
        assert_eq!(
            code(&r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap()),
            Some(f as u32)
        );
    }
    let decoded = frames_decoded() - before;
    eprintln!("50 steps forward: {decoded} frames decoded");
    assert!(
        decoded <= 50 + 2,
        "{decoded} frames decoded for 50 steps forward"
    );

    // Back a frame at a time from late in the last GOP to the start: windows
    // start on keyframes, so the whole way decodes about each frame once
    // (seeking from a keyframe for each frame would be thousands).
    let mut r = StillRenderer::default();
    assert_eq!(
        code(&r.rgba(&v(&long, 299), 0, fps, 320, 180).unwrap()),
        Some(299)
    );
    let before = frames_decoded();
    for f in (0..299).rev() {
        let img = r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap();
        assert_eq!(code(&img), Some(f as u32), "frame {f} going back");
    }
    let decoded = frames_decoded() - before;
    eprintln!("299 steps back: {decoded} frames decoded");
    assert!(
        decoded < 400,
        "{decoded} frames decoded stepping back through 299"
    );

    // A prefetched window: the steps back through it decode nothing.
    let mut r = StillRenderer::default();
    r.rgba(&v(&long, 299), 0, fps, 320, 180).unwrap();
    r.rgba(&v(&long, 298), 0, fps, 320, 180).unwrap();
    let prefetch = r
        .take_prefetch()
        .expect("going back plans the window before");
    prefetch.run().unwrap();
    let before = frames_decoded();
    for f in (1..298).rev() {
        let img = r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap();
        assert_eq!(code(&img), Some(f as u32), "frame {f} after the prefetch");
        // Prefetches past the start aren't run here: dropped untaken.
    }
    assert_eq!(
        frames_decoded() - before,
        0,
        "all of it was rendered already"
    );

    // A prefetch running on another thread: asked for a frame it is
    // rendering, the renderer waits for it instead of rendering it too.
    let mut r = StillRenderer::default();
    r.rgba(&v(&long, 299), 0, fps, 320, 180).unwrap();
    r.rgba(&v(&long, 298), 0, fps, 320, 180).unwrap();
    let prefetch = r.take_prefetch().unwrap();
    let t = std::thread::spawn(move || prefetch.run().unwrap());
    let before = frames_decoded();
    for f in (100..=120).rev() {
        assert_eq!(
            code(&r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap()),
            Some(f as u32)
        );
    }
    t.join().unwrap();
    let decoded = frames_decoded() - before;
    eprintln!("21 steps back beside a running prefetch: {decoded} frames decoded");
    assert!(
        decoded < 200,
        "{decoded}: the prefetch's window decoded once"
    );

    // After a jump, two seconds behind the position render ahead of need:
    // the first steps back decode nothing.
    let mut r = StillRenderer::default();
    r.rgba(&v(&long, 250), 0, fps, 320, 180).unwrap();
    r.take_prefetch()
        .expect("a jump keeps two seconds behind")
        .run()
        .unwrap();
    let before = frames_decoded();
    for f in (190..250).rev() {
        let img = r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap();
        assert_eq!(code(&img), Some(f as u32), "frame {f} behind a jump");
    }
    assert_eq!(frames_decoded() - before, 0, "the two seconds were ready");

    // Nothing behind the floor, where the shown stretch of the file starts.
    let mut r = StillRenderer::default();
    r.rgba(&v(&long, 205), 200, fps, 320, 180).unwrap();
    r.take_prefetch()
        .expect("frames 200 to 204 are worth keeping")
        .run()
        .unwrap();
    let before = frames_decoded();
    for f in (200..205).rev() {
        let img = r.rgba(&v(&long, f), 200, fps, 320, 180).unwrap();
        assert_eq!(code(&img), Some(f as u32));
        assert!(
            r.take_prefetch().is_none(),
            "nothing behind the floor at {f}"
        );
    }
    assert_eq!(frames_decoded() - before, 0);

    // Between two files on a tight budget: what was looked at last stays
    // whole, and the oldest pictures go. (Going back in the second file
    // renders its windows, which pushes the budget over.)
    let other =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 300).gop(30));
    let mut r = StillRenderer::with_budget(100 * 320 * 180 * 4);
    for f in 10..=60 {
        r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap();
    }
    for f in 100..=130 {
        r.rgba(&v(&other, f), 0, fps, 320, 180).unwrap();
    }
    for f in (100..=129).rev() {
        r.rgba(&v(&other, f), 0, fps, 320, 180).unwrap();
        if let Some(p) = r.take_prefetch() {
            p.run().unwrap();
        }
    }
    let before = frames_decoded();
    for f in (45..=60).rev() {
        let img = r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap();
        assert_eq!(
            code(&img),
            Some(f as u32),
            "frame {f} back in the first file"
        );
    }
    assert_eq!(
        frames_decoded() - before,
        0,
        "the first file's frames looked at last were kept"
    );

    // A 24 fps file in a 30 fps project: some frames repeat. Stepping back
    // gives exactly the frames rendered one by one.
    let slow = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (24, 1), 240)
            .gop(48)
            .bframes(2),
    );
    let mut r = StillRenderer::default();
    for f in (0..290).rev() {
        let stepped = code(&r.rgba(&v(&slow, f), 0, fps, 320, 180).unwrap());
        let alone = code(&vidcut_av::export::still::rgba(&v(&slow, f), fps, 320, 180).unwrap());
        assert_eq!(stepped, alone, "project frame {f} of a 24 fps file");
    }
}
