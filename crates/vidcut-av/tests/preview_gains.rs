//! The preview's clip gains go through FFmpeg's `volume` filter as one
//! expression per lane (`vidcut_core::preview::audio_graph`). With thousands
//! of gain changes it still parses, and each clip gets its own gain.

use vidcut_av::ffmpeg::{self, filter, format, frame};
use vidcut_core::render::{AudioPiece, RenderPlan};
use vidcut_core::time::Rational;

#[test]
fn thousands_of_gain_changes_parse_and_apply() {
    // 2000 pieces of 0.1 s (4800 samples) cycling through four gains.
    let db = [0.0, -6.0, -12.0, 3.0];
    let pieces: Vec<AudioPiece> = (0..2000)
        .map(|i| AudioPiece::Source {
            media: "m1".into(),
            path: "/media/a.wav".into(),
            stream: 0,
            origin: 0.0,
            source_sample: 4800 * i as i64,
            samples: 4800,
            gain_db: db[i % 4],
        })
        .collect();
    let plan = RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: 320,
        height: 180,
        sample_rate: 48_000,
        channels: 1,
        frames: 6000,
        video: vec![],
        audio: vec![pieces],
        range: (0, 6000),
        name: "gains".into(),
    };
    let graph = vidcut_core::preview::audio_graph(&plan, None).unwrap();
    let spec = graph
        .strip_prefix("[aid1]")
        .and_then(|g| g.strip_suffix("[ao]"))
        .unwrap_or_else(|| panic!("one lane: {graph:.200}"));

    vidcut_av::init();
    let mut g = filter::Graph::new();
    g.add(
        &filter::find("abuffer").unwrap(),
        "in",
        "sample_rate=48000:sample_fmt=flt:channel_layout=mono:time_base=1/48000",
    )
    .unwrap();
    g.add(&filter::find("abuffersink").unwrap(), "out", "")
        .unwrap();
    g.output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(spec))
        .and_then(|_| g.validate())
        .expect("FFmpeg takes the expression");

    // A steady 0.5 through it, 1024 samples a frame.
    let total = 2000 * 4800;
    let mut out: Vec<f32> = Vec::with_capacity(total);
    let pulled = |g: &mut filter::Graph, out: &mut Vec<f32>| {
        let mut f = frame::Audio::empty();
        while g.get("out").unwrap().sink().frame(&mut f).is_ok() {
            let n = f.samples();
            out.extend_from_slice(&f.plane::<f32>(0)[..n]);
        }
    };
    for start in (0..total).step_by(1024) {
        let n = 1024.min(total - start);
        let mut f = frame::Audio::new(
            format::Sample::F32(format::sample::Type::Packed),
            n,
            ffmpeg::ChannelLayout::MONO,
        );
        f.set_rate(48_000);
        f.plane_mut::<f32>(0)[..n].fill(0.5);
        f.set_pts(Some(start as i64));
        g.get("in").unwrap().source().add(&f).unwrap();
        pulled(&mut g, &mut out);
    }
    g.get("in").unwrap().source().flush().unwrap();
    pulled(&mut g, &mut out);
    assert_eq!(out.len(), total);
    for i in 0..2000 {
        let mid = i * 4800 + 2400;
        let want = 0.5 * 10f32.powf(db[i % 4] as f32 / 20.0);
        assert!(
            (out[mid] - want).abs() < 1e-3,
            "piece {i}: {} instead of {want}",
            out[mid]
        );
    }
}

/// With live faders (preview.md §10) the graph's `azmq` binds to its
/// address: the escaping survives FFmpeg's two levels of unescaping. A
/// wrong one fails the graph, and the preview would play no sound.
#[test]
fn the_fader_control_address_binds() {
    if !vidcut_av::has_filter("azmq") {
        eprintln!("skipping: this FFmpeg has no azmq");
        return;
    }
    let dir = tempfile::Builder::new()
        .prefix("vidcut-azmq")
        .tempdir()
        .unwrap();
    let socket = dir.path().join("mix-1");
    let plan = RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: 320,
        height: 180,
        sample_rate: 48_000,
        channels: 1,
        frames: 30,
        video: vec![],
        audio: vec![vec![AudioPiece::Source {
            media: "m1".into(),
            path: "/media/a.wav".into(),
            stream: 0,
            origin: 0.0,
            source_sample: 0,
            samples: 48_000,
            gain_db: -6.0,
        }]],
        range: (0, 30),
        name: "faders".into(),
    };
    let lanes = [vidcut_core::render::LaneTrack {
        track: "V1".into(),
        volume_db: -6.0,
    }];
    let bind = format!("ipc://{}", socket.display());
    let control = vidcut_core::preview::FaderControl {
        lanes: &lanes,
        bind: &bind,
    };
    let graph = vidcut_core::preview::audio_graph(&plan, Some(control)).unwrap();
    let spec = graph
        .strip_prefix("[aid1]")
        .and_then(|g| g.strip_suffix("[ao]"))
        .unwrap_or_else(|| panic!("one lane: {graph}"));
    let mut g = filter::Graph::new();
    g.add(
        &filter::find("abuffer").unwrap(),
        "in",
        "sample_rate=48000:sample_fmt=flt:channel_layout=mono:time_base=1/48000",
    )
    .unwrap();
    g.add(&filter::find("abuffersink").unwrap(), "out", "")
        .unwrap();
    g.output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(spec))
        .and_then(|_| g.validate())
        .unwrap_or_else(|e| panic!("{spec}: {e}"));
    assert!(socket.exists(), "azmq bound {}", socket.display());
}
