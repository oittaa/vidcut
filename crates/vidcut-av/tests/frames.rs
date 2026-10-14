//! Single frames as small pictures (the timeline's filmstrip tiles): each
//! one the frame export samples there, near or far from the one before,
//! forward or back.

use vidcut_av::decode::read_barcode;
use vidcut_av::export::renderer::FrameReader;
use vidcut_av::ffmpeg::{format::Pixel, frame};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::thumbs::Image;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

/// The barcode in a picture.
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
fn tiles_show_their_own_frames() {
    // Keyframes 120 frames apart, with B-frames.
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 300)
        .gop(120)
        .bframes(2);
    let path = fixture::ensure(&spec).unwrap();
    let origin = vidcut_av::probe::probe(&path, None)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    let fps = Rational::new(30, 1).unwrap();
    let piece = |source_frame| VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: path.clone(),
        origin,
        source_frame,
        frames: 1,
        image: None,
    };
    let mut reader = FrameReader::default();
    // Neighbors, a grid step apart, across keyframes, then back.
    for (w, h) in [(320, 180), (160, 90)] {
        for f in [0, 1, 2, 30, 64, 65, 119, 120, 128, 200, 299, 100, 5] {
            let img = reader.rgba(&piece(f), fps, w, h).unwrap();
            assert_eq!((img.width, img.height), (w, h));
            assert_eq!(code(&img), Some(f as u32), "frame {f} at {w}x{h}");
        }
    }
}
