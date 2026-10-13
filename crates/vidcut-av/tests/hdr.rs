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
        let frames = hdr::read_frames(&path, false, usize::MAX).unwrap();
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
        let alone = hdr::read_frames(&out, false, usize::MAX).unwrap();
        assert_eq!(alone.len(), 20, "{what}");
        if in_band {
            for f in &alone {
                assert_eq!(f.metadata.light, want.light, "{what}: frame {}", f.pts);
                assert!(f.metadata.mastering.is_some(), "{what}: frame {}", f.pts);
            }
        }
        // Each frame's own HDR10+, from the source frame it shows.
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
        hdr::read_frames(&out, true, usize::MAX)
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
    for f in hdr::read_frames(&out, true, usize::MAX).unwrap() {
        assert_eq!(f.metadata, hdr::Metadata::default(), "frame {}", f.pts);
    }
}

// ---------------------------------------------------------------- real clips

/// A clip of `tests/media-manifest.toml`, if fetched.
fn media(name: &str, file: &str) -> Option<PathBuf> {
    let path = vidcut_av::testdata::root()
        .join("media")
        .join(name)
        .join(file);
    if !path.exists() {
        eprintln!("skipping {name}: run scripts/fetch-test-media.sh");
    }
    path.exists().then_some(path)
}

/// `frames` of `path` from its start, into a plan at `fps` and `w`×`h`.
fn clip_plan(path: &Path, fps: i64, frames: i64, w: u32, h: u32) -> RenderPlan {
    let mut p = plan(vec![piece(path, 0, frames)]);
    p.fps = Rational::new(fps, 1).unwrap();
    (p.width, p.height) = (w, h);
    p
}

#[test]
fn real_clips_keep_their_hdr_metadata() {
    vidcut_av::init();
    let dir = tempfile::tempdir().unwrap();

    // HDR10 values (1000 cd/m², MaxCLL 1000, MaxFALL 400) into x265.
    if let Some(bear) = media("hdr10-bear-hevc", "bear-1280x720-hevc-10bit-hdr10.mp4") {
        let out = dir.path().join("bear.mkv");
        let r = render(
            &clip_plan(&bear, 30, 30, 640, 360),
            &settings(VCodec::Hevc, OutContainer::Mkv, HdrMode::Auto),
            &out,
        );
        assert_eq!(r.metadata.unwrap().mode, HdrMetadata::Static);
        let boxes = stream_values(&out);
        let m = boxes.mastering.unwrap();
        assert!(
            close(m.max_nits, 1000.0) && close(m.min_nits, 0.05),
            "{m:?}"
        );
        assert_eq!(
            boxes.light,
            Some(LightLevel {
                max_cll: 1000,
                max_fall: 400
            })
        );
    }

    // HDR10+ on every frame of 4K AV1, scaled down: each frame keeps its
    // own. The clip has no static values: BT.2020 at 1000 cd/m² is assumed.
    if let Some(plus) = media("hdr10plus-croatia-av1", "B_1.mp4") {
        let out = dir.path().join("plus.webm");
        let r = render(
            &clip_plan(&plus, 60, 30, 640, 360),
            &settings(VCodec::Av1, OutContainer::Webm, HdrMode::Auto),
            &out,
        );
        let m = r.metadata.unwrap();
        assert_eq!(m.mode, HdrMetadata::Hdr10plus, "{:?}", m.why);
        assert!(close(m.mastering.unwrap().max_nits, 1000.0));
        assert_eq!(m.light, None);
        let source = hdr::read_frames(&plus, true, 30).unwrap();
        let output = hdr::read_frames(&out, true, 30).unwrap();
        assert_eq!(output.len(), 30);
        for (i, (s, o)) in source.iter().zip(&output).enumerate() {
            assert!(s.hdr10plus.is_some(), "source frame {i}");
            assert_eq!(o.hdr10plus, s.hdr10plus, "HDR10+ of frame {i}");
        }
    }

    // Dolby Vision 8.4: an HLG base layer kept, each frame's RPU with it,
    // through libx265, into Matroska and MP4.
    if let Some(dv) = media("dolby-vision-84-hevc", "Test Jellyfin 1080p DV P8.4.mp4") {
        for container in [OutContainer::Mkv, OutContainer::Mp4] {
            let out = dir.path().join(format!("dv.{}", container.extension()));
            let r = render(
                &clip_plan(&dv, 60, 30, 960, 540),
                &settings(VCodec::Hevc, container, HdrMode::Auto),
                &out,
            );
            let m = r.metadata.unwrap();
            assert_eq!(
                m.mode,
                HdrMetadata::DolbyVision,
                "{container:?}: {:?}",
                m.why
            );
            let input = vidcut_av::ffmpeg::format::input(&out).unwrap();
            let params = input
                .streams()
                .best(vidcut_av::ffmpeg::media::Type::Video)
                .unwrap()
                .parameters();
            assert_eq!(
                hdr::dolby_vision(&params),
                Some(vidcut_core::model::DolbyVision {
                    profile: 8,
                    compatibility: 4
                }),
                "{container:?}"
            );
            let frames = hdr::read_frames(&out, true, usize::MAX).unwrap();
            assert_eq!(frames.len(), 30);
            assert!(frames.iter().all(|f| f.dolby_vision), "{container:?}");
        }
    }
}

// ---------------------------------------------------------------- GPU

/// The GPU's encoders write the HDR metadata too (`gpu-tests`): NVENC on
/// frames scaled on the GPU, kept PQ with HDR10+, and with an SDR clip that
/// libplacebo converts; VAAPI, which reads the values from frames. Each where
/// this machine has it.
#[cfg(feature = "gpu-tests")]
#[test]
fn gpu_encoders_write_the_hdr_metadata() {
    vidcut_av::init();
    let available = export::encode::detect();
    let plus = hdr10_source(1000, (1000, 400), true);
    let sdr = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 30).no_audio(),
    )
    .unwrap();
    let want = HdrMeta {
        max_nits: 1000,
        light: Some((1000, 400)),
        hdr10plus: true,
    }
    .values();
    let dir = tempfile::tempdir().unwrap();
    let mut tried = 0;
    for (codec, kind, name, container) in [
        (
            VCodec::Hevc,
            EncoderChoice::Nvenc,
            "hevc_nvenc",
            OutContainer::Mp4,
        ),
        (
            VCodec::Av1,
            EncoderChoice::Nvenc,
            "av1_nvenc",
            OutContainer::Mkv,
        ),
        (
            VCodec::Hevc,
            EncoderChoice::Vaapi,
            "hevc_vaapi",
            OutContainer::Mkv,
        ),
    ] {
        if !available
            .encoders
            .iter()
            .any(|e| e.ok && e.encoder == name && e.pixel == "p010le")
        {
            eprintln!("skipping {name}: not on this machine");
            continue;
        }
        tried += 1;
        let mut s = settings(codec, container, HdrMode::Pq);
        let v = s.video.as_mut().unwrap();
        (v.encoder, v.speed) = (kind, None);

        // Kept PQ, scaled to 640×360: each frame's own HDR10+.
        let mut p = plan(vec![piece(&plus, 5, 20)]);
        (p.width, p.height) = (640, 360);
        let out = dir.path().join(format!("{name}.{}", container.extension()));
        let r = export::run(
            &p,
            &s,
            &out,
            &available,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(r.video_encoder.as_deref(), Some(name));
        assert_eq!(r.metadata.unwrap().mode, HdrMetadata::Hdr10plus, "{name}");
        let boxes = stream_values(&out);
        assert_eq!(boxes.light, want.light, "{name}: the container");
        assert!(boxes.mastering.is_some(), "{name}: the container");
        let alone = hdr::read_frames(&out, false, usize::MAX).unwrap();
        assert_eq!(alone.len(), 20, "{name}");
        let in_band = alone.iter().filter(|f| f.metadata.light.is_some()).count();
        eprintln!("{name}: static values in the bitstream on {in_band} of 20 frames");
        assert!(in_band > 0, "{name}: no static values in the bitstream");
        for f in alone.iter().filter(|f| f.metadata.light.is_some()) {
            assert_eq!(f.metadata.light, want.light, "{name}: frame {}", f.pts);
            let m = f.metadata.mastering.expect("a mastering display with them");
            let w = want.mastering.unwrap();
            assert!(
                close(m.max_nits, w.max_nits) && close(m.min_nits, w.min_nits),
                "{name}: {m:?}"
            );
            assert!(
                m.primaries
                    .iter()
                    .flatten()
                    .zip(w.primaries.iter().flatten())
                    .all(|(a, b)| close(*a, *b)),
                "{name}: {m:?}"
            );
        }
        for (i, f) in alone.iter().enumerate() {
            assert_eq!(
                f.hdr10plus.as_deref(),
                Some(fixture::hdr10plus_t35(5 + i as u32).as_slice()),
                "{name}: HDR10+ of frame {i}"
            );
        }

        // With an SDR clip (converted into PQ, by libplacebo on NVENC):
        // the widest values, and no HDR10+.
        let p = plan(vec![piece(&sdr, 0, 10), piece(&plus, 0, 10)]);
        let out = dir
            .path()
            .join(format!("{name}-mixed.{}", container.extension()));
        let r = export::run(
            &p,
            &s,
            &out,
            &available,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(r.metadata.unwrap().mode, HdrMetadata::Static, "{name}");
        assert_eq!(stream_values(&out).light, want.light, "{name}: mixed");
        let frames = hdr::read_frames(&out, true, usize::MAX).unwrap();
        assert!(
            frames.iter().all(|f| f.hdr10plus.is_none()),
            "{name}: mixed"
        );
    }
    assert!(tried > 0, "no GPU encoder of HEVC or AV1 here");
}
