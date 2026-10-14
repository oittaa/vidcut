//! The frame renderer behind the paused viewer and the two-up
//! (docs/decisions/preview.md §9): every picture exact, stepping forward
//! and back, beside prefetches and after jumps.

use vidcut_av::decode::read_barcode;
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
    let info = vidcut_av::probe::probe(&path, None).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    Src { path, origin }
}

fn v(s: &Src, source_frame: i64) -> VideoPiece {
    VideoPiece::Source {
        pace: Default::default(),
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
fn steps_both_ways_show_their_own_frames() {
    let fps = Rational::new(30, 1).unwrap();
    // Keyframes 120 frames apart, with B-frames.
    let long = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 300)
            .gop(120)
            .bframes(2),
    );

    // Forward a frame at a time.
    let mut r = StillRenderer::default();
    for f in 10..=60 {
        assert_eq!(
            code(&r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap()),
            Some(f as u32)
        );
    }

    // Back a frame at a time from late in the last GOP to the start.
    let mut r = StillRenderer::default();
    for f in (0..=299).rev() {
        let img = r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap();
        assert_eq!(code(&img), Some(f as u32), "frame {f} going back");
    }

    // Back through a prefetched window.
    let mut r = StillRenderer::default();
    r.rgba(&v(&long, 299), 0, fps, 320, 180).unwrap();
    r.rgba(&v(&long, 298), 0, fps, 320, 180).unwrap();
    let prefetch = r
        .take_prefetch()
        .expect("going back plans the window before");
    prefetch.run().unwrap();
    for f in (1..298).rev() {
        let img = r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap();
        assert_eq!(code(&img), Some(f as u32), "frame {f} after the prefetch");
    }

    // Back beside a prefetch running on another thread.
    let mut r = StillRenderer::default();
    r.rgba(&v(&long, 299), 0, fps, 320, 180).unwrap();
    r.rgba(&v(&long, 298), 0, fps, 320, 180).unwrap();
    let prefetch = r.take_prefetch().unwrap();
    let t = std::thread::spawn(move || prefetch.run().unwrap());
    for f in (100..=120).rev() {
        assert_eq!(
            code(&r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap()),
            Some(f as u32)
        );
    }
    t.join().unwrap();

    // Back from a jump, with the two seconds behind it rendered ahead.
    let mut r = StillRenderer::default();
    r.rgba(&v(&long, 250), 0, fps, 320, 180).unwrap();
    r.take_prefetch()
        .expect("a jump keeps two seconds behind")
        .run()
        .unwrap();
    for f in (190..250).rev() {
        let img = r.rgba(&v(&long, f), 0, fps, 320, 180).unwrap();
        assert_eq!(code(&img), Some(f as u32), "frame {f} behind a jump");
    }

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
