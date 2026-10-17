use vidcut_av::fixture::{self, Container, FixtureSpec, HdrMeta, Picture, VideoCodec};
use vidcut_av::hdr;
use vidcut_av::light::{self, Span};
use vidcut_core::model::Hdr;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::export::{self, encode::Availability};
use vidcut_core::export::{
    Container as OutContainer, EncoderChoice, ExportSettings, HdrMetadata, HdrMode,
    VideoCodec as VCodec, VideoSettings,
};
use vidcut_core::render::{RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

fn piece(path: &Path, source_frame: i64, frames: i64) -> VideoPiece {
    let origin = vidcut_av::probe::probe(path, None)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    VideoPiece::Source {
        pace: Default::default(),
        media: path.display().to_string(),
        path: path.to_path_buf(),
        origin,
        source_frame,
        frames,
        image: None,
    }
}

fn plan(video: Vec<VideoPiece>) -> RenderPlan {
    let frames = video.iter().map(VideoPiece::frames).sum();
    RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: 320,
        height: 180,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video,
        audio: vec![],
        range: (0, frames),
        name: "t".into(),
    }
}

fn settings(codec: VCodec, container: OutContainer, hdr: HdrMode) -> ExportSettings {
    ExportSettings {
        container,
        video: Some(VideoSettings {
            codec,
            encoder: EncoderChoice::Cpu,
            quality: None,
            bitrate: None,
            speed: Some(
                match codec {
                    VCodec::Av1 => "10",
                    VCodec::Vp9 => "8",
                    _ => "ultrafast",
                }
                .into(),
            ),
            profile: None,
            bit_depth: None,
            width: None,
            height: None,
            hdr,
            passes: None,
            first_pass: Default::default(),
            multipass: Default::default(),
            lookahead: None,
            hdr_metadata: HdrMetadata::Auto,
            mastering_display: None,
            max_cll: None,
        }),
        audio: None,
        declick: false,
    }
}

fn render(plan: &RenderPlan, settings: &ExportSettings, out: &Path) -> export::Report {
    export::run(
        plan,
        settings,
        out,
        &Availability::default(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap()
}

fn stream_values(path: &Path) -> hdr::Static {
    let input = vidcut_av::ffmpeg::format::input(&path).unwrap();
    let params = input
        .streams()
        .best(vidcut_av::ffmpeg::media::Type::Video)
        .unwrap()
        .parameters();
    hdr::of_stream(&params)
}

fn hdr10_source(max_nits: u32, light: (u32, u32), hdr10plus: bool) -> PathBuf {
    fixture::ensure(
        &FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 30)
            .hdr(Hdr::Pq)
            .hdr_meta(HdrMeta {
                max_nits,
                light: Some(light),
                hdr10plus,
                hdr10plus_frames: None,
            })
            .no_audio(),
    )
    .unwrap()
}

#[test]
fn pq_exports_keep_hdr10_values_and_each_frames_hdr10plus() {
    vidcut_av::init();
    let source = hdr10_source(1000, (1000, 400), true);
    let want = HdrMeta {
        max_nits: 1000,
        light: Some((1000, 400)),
        hdr10plus: true,
        hdr10plus_frames: None,
    }
    .values();
    let p = plan(vec![piece(&source, 5, 20)]);
    let dir = tempfile::tempdir().unwrap();
    for (codec, container, in_band) in [
        (VCodec::Hevc, OutContainer::Mp4, true),
        (VCodec::Av1, OutContainer::Mp4, true),
        (VCodec::Av1, OutContainer::Webm, true),
        (VCodec::Vp9, OutContainer::Webm, false),
    ] {
        let what = format!("{codec:?} in {container:?}");
        let out = dir.path().join(format!("out.{}", container.extension()));
        let r = render(&p, &settings(codec, container, HdrMode::Auto), &out);
        let m = r.metadata.expect("chosen");
        assert_eq!(m.mode, HdrMetadata::Hdr10plus, "{what}: {:?}", m.why);
        let boxes = stream_values(&out);
        assert_eq!(boxes.light, want.light, "{what}");
        let got = boxes
            .mastering
            .expect("a mastering display in the container");
        assert!(
            close(got.max_nits, 1000.0) && close(got.min_nits, 0.0001),
            "{what}"
        );
        let alone = hdr::read_frames(&out, false, usize::MAX).unwrap();
        assert_eq!(alone.len(), 20, "{what}");
        if in_band {
            for f in &alone {
                assert_eq!(f.metadata.light, want.light, "{what}: frame {}", f.pts);
                assert!(f.metadata.mastering.is_some(), "{what}: frame {}", f.pts);
            }
        }
        let frames = hdr::read_frames(&out, true, usize::MAX).unwrap();
        for (i, f) in frames.iter().enumerate() {
            assert_eq!(
                f.hdr10plus.as_deref(),
                Some(fixture::hdr10plus_t35(5 + i as u32).as_slice()),
                "{what}: HDR10+ of frame {i}"
            );
        }
    }
}

#[test]
fn light_is_measured_as_a_pq_export_shows_it() {
    vidcut_av::init();
    for (what, spec) in [
        (
            "PQ",
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 33)
                .hdr(Hdr::Pq),
        ),
        (
            "HLG",
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 33)
                .hdr(Hdr::Hlg),
        ),
        (
            "SDR",
            FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 33),
        ),
    ] {
        let path = fixture::ensure(&spec.no_audio()).unwrap();
        let span = |from, to| Span {
            input: vidcut_av::decode::Input::File(path.clone()),
            from,
            to,
        };
        let a = light::analysis(&span(0.0, 2.0), true, None, None, &mut |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(a.frames.len(), 33, "{what}");
        let level = a.level((0..30).map(|k| f64::from(k) / 30.0)).unwrap();
        let f = a.at(0.5).unwrap();
        assert!((195.0..=215.0).contains(&f.percentiles[8]), "{what}: {f:?}");
        assert!(f.percentiles[0] < 1.0, "{what}: black, {f:?}");
        assert!((200..=300).contains(&level.max_cll), "{what}: {level:?}");
        assert!((90..=115).contains(&level.max_fall), "{what}: {level:?}");
    }
}

#[test]
fn light_levels_asked_to_be_measured_are_measured() {
    vidcut_av::init();
    let dir = tempfile::tempdir().unwrap();
    let pq = hdr10_source(1000, (1000, 400), false);
    let mut measure = settings(VCodec::Hevc, OutContainer::Mkv, HdrMode::Pq);
    measure.video.as_mut().unwrap().max_cll = Some("measure".into());
    let out = dir.path().join("measure.mkv");
    let r = render(&plan(vec![piece(&pq, 0, 20)]), &measure, &out);
    let m = r.metadata.unwrap();
    let l = m.light.expect("measured");
    assert!((200..=300).contains(&l.max_cll), "{l:?}");
    assert!((90..=115).contains(&l.max_fall), "{l:?}");
    assert_eq!(stream_values(&out).light, Some(l));
}

#[test]
fn a_highlight_of_two_pixels_is_the_peak() {
    vidcut_av::init();
    for (what, spec, peak) in [
        (
            "PQ",
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 1920, 1080, (30, 1), 3)
                .hdr(Hdr::Pq),
            20.0..=230.0,
        ),
        (
            "SDR",
            FixtureSpec::new(VideoCodec::Ffv1, Container::Mkv, 1920, 1080, (30, 1), 3),
            202.9..=203.1,
        ),
    ] {
        let path = fixture::ensure(&spec.picture(Picture::Highlight).no_audio()).unwrap();
        let span = Span {
            input: vidcut_av::decode::Input::File(path),
            from: 0.0,
            to: 0.1,
        };
        let a = light::analysis(&span, true, None, None, &mut |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(a.frames.len(), 3, "{what}");
        for (t, f) in &a.frames {
            assert!(peak.contains(&f.max), "{what} at {t}: {f:?}");
            for v in f.hdr10plus().maxscl {
                assert!(peak.contains(&(v as f32)), "{what} at {t}: {f:?}");
            }
            assert!(
                f.average < 0.1 && f.percentiles[8] < 0.1,
                "{what} at {t}: {f:?}"
            );
        }
        let level = a.level((0..3).map(|k| f64::from(k) / 30.0)).unwrap();
        assert!(peak.contains(&(level.max_cll as f32)), "{what}: {level:?}");
    }
}
