//! HDR metadata in files: the fixtures carry it as encoders and muxers
//! write it (the bitstream and the container), and the probe reads it.

use vidcut_av::fixture::{self, Container, FixtureSpec, HdrMeta, VideoCodec};
use vidcut_av::hdr;
use vidcut_core::model::{Hdr, LightLevel};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn hdr10_values_are_in_the_stream_the_container_and_the_probe() {
    vidcut_av::init();
    let meta = HdrMeta {
        max_nits: 1000,
        light: Some((1000, 400)),
        hdr10plus: true,
    };
    for container in [Container::Mp4, Container::Mkv] {
        let spec = FixtureSpec::new(VideoCodec::Hevc10, container, 320, 180, (30, 1), 30)
            .hdr(Hdr::Pq)
            .hdr_meta(meta);
        let path = fixture::ensure(&spec).unwrap();
        let want = meta.values();
        let m = want.mastering.unwrap();

        // The container's boxes.
        let input = vidcut_av::ffmpeg::format::input(&path).unwrap();
        let params = input
            .streams()
            .best(vidcut_av::ffmpeg::media::Type::Video)
            .unwrap()
            .parameters();
        let boxes = hdr::of_stream(&params);
        assert_eq!(boxes.light, want.light, "{container:?}");
        let got = boxes.mastering.expect("mastering display in the container");
        assert!(close(got.max_nits, m.max_nits) && close(got.min_nits, m.min_nits));
        assert!(
            got.primaries
                .iter()
                .flatten()
                .zip(m.primaries.iter().flatten())
                .all(|(a, b)| close(*a, *b))
        );

        // The bitstream alone: SEI at every keyframe, HDR10+ on every frame,
        // each its own.
        let frames = hdr::read_frames(&path, false).unwrap();
        assert_eq!(frames.len(), 30);
        for (k, f) in frames.iter().enumerate() {
            assert_eq!(f.metadata.light, want.light, "frame {k} of {container:?}");
            let fm = f.metadata.mastering.expect("mastering display in the SEI");
            assert!(close(fm.max_nits, 1000.0));
            assert_eq!(
                f.hdr10plus.as_deref(),
                Some(fixture::hdr10plus_t35(k as u32).as_slice()),
                "HDR10+ of frame {k}"
            );
        }

        // The probe, as the project keeps it.
        let info = vidcut_av::probe::probe(&path).unwrap();
        let v = info.video.unwrap();
        assert_eq!(
            v.hdr.light,
            Some(LightLevel {
                max_cll: 1000,
                max_fall: 400
            })
        );
        assert!(v.hdr.hdr10plus);
        assert!(close(v.hdr.mastering.unwrap().max_nits, 1000.0));
    }
}

// ---------------------------------------------------------------- exports

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
    let origin = vidcut_av::probe::probe(path)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    VideoPiece::Source {
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
    }
    .values();
    let p = plan(vec![piece(&source, 5, 20)]);
    let dir = tempfile::tempdir().unwrap();
    for (codec, container, in_band) in [
        (VCodec::Hevc, OutContainer::Mp4, true),
        (VCodec::Hevc, OutContainer::Mkv, true),
        (VCodec::Av1, OutContainer::Mp4, true),
        (VCodec::Av1, OutContainer::Webm, true),
        (VCodec::Vp9, OutContainer::Webm, false),
    ] {
        let what = format!("{codec:?} in {container:?}");
        let out = dir.path().join(format!("out.{}", container.extension()));
        let r = render(&p, &settings(codec, container, HdrMode::Auto), &out);
        let m = r.metadata.expect("chosen");
        assert_eq!(m.mode, HdrMetadata::Hdr10plus, "{what}: {:?}", m.why);
        // The container's boxes.
        let boxes = stream_values(&out);
        assert_eq!(boxes.light, want.light, "{what}");
        let got = boxes
            .mastering
            .expect("a mastering display in the container");
        assert!(
            close(got.max_nits, 1000.0) && close(got.min_nits, 0.0001),
            "{what}"
        );
        // The bitstream: HEVC's SEI, AV1's metadata OBUs.
        let alone = hdr::read_frames(&out, false).unwrap();
        assert_eq!(alone.len(), 20, "{what}");
        if in_band {
            for f in &alone {
                assert_eq!(f.metadata.light, want.light, "{what}: frame {}", f.pts);
                assert!(f.metadata.mastering.is_some(), "{what}: frame {}", f.pts);
            }
        }
        // Each frame's own HDR10+, from the source frame it shows.
        let frames = hdr::read_frames(&out, true).unwrap();
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
fn mixed_sources_give_the_widest_values_and_sdr_none() {
    vidcut_av::init();
    let a = hdr10_source(1000, (1000, 400), true);
    let b = hdr10_source(4000, (2000, 300), false);
    let p = plan(vec![piece(&a, 0, 10), piece(&b, 0, 10)]);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("mixed.mkv");
    let r = render(
        &p,
        &settings(VCodec::Hevc, OutContainer::Mkv, HdrMode::Pq),
        &out,
    );
    let m = r.metadata.unwrap();
    assert_eq!(m.mode, HdrMetadata::Static, "{:?}", m.why);
    assert!(
        m.why
            .iter()
            .any(|w| w == "No HDR10+: 50% of the picture has none"),
        "{:?}",
        m.why
    );
    let boxes = stream_values(&out);
    assert!(close(boxes.mastering.unwrap().max_nits, 4000.0));
    assert_eq!(
        boxes.light,
        Some(LightLevel {
            max_cll: 2000,
            max_fall: 400
        })
    );
    // No frame has HDR10+: neither the clip that had it, nor the other.
    assert!(
        hdr::read_frames(&out, true)
            .unwrap()
            .iter()
            .all(|f| f.hdr10plus.is_none())
    );

    // SDR: tone-mapped, nothing of the sources' metadata left.
    let out = dir.path().join("sdr.mkv");
    render(
        &p,
        &settings(VCodec::Hevc, OutContainer::Mkv, HdrMode::Sdr),
        &out,
    );
    assert_eq!(stream_values(&out), hdr::Static::default());
    for f in hdr::read_frames(&out, true).unwrap() {
        assert_eq!(f.metadata, hdr::Metadata::default(), "frame {}", f.pts);
    }
}
