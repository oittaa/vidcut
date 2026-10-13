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
