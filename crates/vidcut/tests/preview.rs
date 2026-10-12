//! Timeline preview: the product EDL (core) with generated fillers (vidcut-av)
//! plays frame-exactly in libmpv, gaps included. Skips without libmpv.

use std::time::{Duration, Instant};

use serde_json::json;
use vidcut::env::AvEnv;
use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::render::{VideoPiece, flatten};
use vidcut_core::time::Rational;
use vidcut_core::{Editor, Envelope, Project};
use vidcut_mpv::{Event, Format, Mpv, SwRenderer, Value, ffi::MPV_RENDER_UPDATE_FRAME};

/// Headless mpv using only render-thread-safe calls.
struct Player {
    mpv: Mpv,
    sw: SwRenderer,
    buf: Vec<u8>,
    tracks: String,
}

impl Player {
    fn new() -> Option<Self> {
        if let Err(e) = vidcut_mpv::available() {
            eprintln!("skipping: {e}");
            return None;
        }
        let mpv = Mpv::new(&[
            ("vo", "libmpv"),
            ("ao", "null"),
            ("hwdec", "no"),
            ("config", "no"),
            ("terminal", "no"),
            ("hr-seek", "yes"),
            ("hr-seek-framedrop", "no"),
            ("keep-open", "always"),
            ("idle", "yes"),
            ("pause", "yes"),
            ("video-timing-offset", "0"),
        ])
        .unwrap();
        mpv.observe_property(1, "track-list", Format::String)
            .unwrap();
        let sw = SwRenderer::new(&mpv).unwrap();
        Some(Self {
            mpv,
            sw,
            buf: vec![0; 320 * 4 * 180],
            tracks: String::new(),
        })
    }

    fn pump(&mut self, until: impl Fn(&Event) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(ev) = self.mpv.wait_event(0.01) {
                if let Event::PropertyChange {
                    value: Value::String(s),
                    ..
                } = &ev
                {
                    self.tracks = s.clone();
                }
                assert!(
                    !matches!(ev, Event::EndFile { reason: 4, .. }),
                    "mpv failed to play"
                );
                if until(&ev) {
                    return;
                }
            }
            if self.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
                self.sw
                    .render_rgb0(320, 180, 320 * 4, &mut self.buf)
                    .unwrap();
            }
        }
        panic!("timed out");
    }

    fn load(&mut self, path: &str) {
        self.mpv
            .command_async(0, &["loadfile", path, "replace"])
            .unwrap();
        self.pump(|e| matches!(e, Event::FileLoaded));
        self.pump(|e| matches!(e, Event::PlaybackRestart));
    }

    fn frame_at(&mut self, secs: f64) -> Option<u32> {
        self.mpv
            .command_async(
                0,
                &["seek", &format!("{:.9}", secs.max(0.0)), "absolute+exact"],
            )
            .unwrap();
        self.pump(|e| matches!(e, Event::PlaybackRestart));
        let settle = Instant::now() + Duration::from_millis(30);
        while Instant::now() < settle {
            self.pump_once();
        }
        self.sw
            .render_rgb0(320, 180, 320 * 4, &mut self.buf)
            .unwrap();
        vidcut_av::barcode::read_packed(&self.buf, 320, 180, 320 * 4, 4, 1)
    }

    fn pump_once(&mut self) {
        let _ = self.mpv.wait_event(0.002);
        if self.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
            self.sw
                .render_rgb0(320, 180, 320 * 4, &mut self.buf)
                .unwrap();
        }
    }
}

fn req(ed: &mut Editor, v: serde_json::Value) -> vidcut_core::Response {
    ed.apply(Envelope::from_json(v).unwrap(), &AvEnv::new(), None)
        .unwrap()
}

#[test]
fn product_edl_plays_frame_exactly_with_gaps() {
    let Some(mut player) = Player::new() else {
        return;
    };
    let a = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ))
    .unwrap();
    let b = fixture::ensure(&FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        320,
        180,
        (30, 1),
        90,
    ))
    .unwrap();
    let c = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 90).no_audio(),
    )
    .unwrap();
    let fps = Rational::new(30, 1).unwrap();
    let settings = Settings {
        fps,
        width: 320,
        height: 180,
        auto_format: false,
        ..Settings::default()
    };
    let mut ed = Editor::new(Project::new(settings), Default::default());
    req(
        &mut ed,
        json!({"method": "media.add", "params": {"paths": [a, b, c]}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.append", "params": {"media": "m1", "range": "10f..40f"}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.append", "params": {"media": "m3", "range": "0f..10f"}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.append", "params": {"media": "m2", "range": "50f..80f"}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.append", "params": {"media": "m3", "range": "60f..70f"}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.delete", "params": {"clips": "c2", "lift": true}}),
    );
    let p = ed.project();
    let plan = flatten(p, 0, p.end()).unwrap();

    let tmp = tempfile::Builder::new()
        .prefix("vidcut-preview-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let fillers = vidcut_av::filler::ensure(&dir.join("fillers"), 320, 180, fps).unwrap();
    let edl = dir.join("timeline.edl");
    std::fs::write(&edl, vidcut_core::preview::edl(&plan, &fillers)).unwrap();

    player.load(edl.to_str().unwrap());
    player.pump_once();
    let videos = player.tracks.matches("\"type\":\"video\"").count();
    let audios = player.tracks.matches("\"type\":\"audio\"").count();
    assert_eq!((videos, audios), (1, 1), "{}", player.tracks);

    // Expected frame at every timeline position near piece boundaries.
    let mut expected = Vec::new();
    for piece in &plan.video {
        match piece {
            VideoPiece::Source {
                source_frame,
                frames,
                ..
            } => expected.extend((0..*frames).map(|k| Some((source_frame + k) as u32))),
            VideoPiece::Black { frames } => expected.extend((0..*frames).map(|_| None)),
        }
    }
    let mut bad = Vec::new();
    for j in [
        0, 1, 15, 28, 29, 30, 31, 38, 39, 40, 41, 55, 68, 69, 70, 71, 78, 79,
    ] {
        let got = player.frame_at(j as f64 / 30.0);
        if got != expected[j] {
            bad.push(format!(
                "timeline frame {j}: got {got:?}, want {:?}",
                expected[j]
            ));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Proxies play frame-exactly in the timeline's EDL, whichever encoder made
/// them (decision D41): the preview swaps the path and keeps the source
/// frame mapping.
#[test]
fn proxies_play_frame_exactly() {
    use vidcut_av::export::encode::Availability;
    use vidcut_core::preferences::ProxyCodec;
    let Some(mut player) = Player::new() else {
        return;
    };
    // B-frames and keyframes 30 frames apart in the source.
    let a = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        150,
    ))
    .unwrap();
    let fps = Rational::new(30, 1).unwrap();
    let settings = Settings {
        fps,
        width: 320,
        height: 180,
        auto_format: false,
        ..Settings::default()
    };
    let mut ed = Editor::new(Project::new(settings), Default::default());
    req(
        &mut ed,
        json!({"method": "media.add", "params": {"paths": [a.clone()]}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.append", "params": {"media": "m1", "range": "10f..70f"}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.append", "params": {"media": "m1", "range": "100f..140f"}}),
    );
    let p = ed.project();
    let plan = flatten(p, 0, p.end()).unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-preview-proxies-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let fillers = vidcut_av::filler::ensure(&dir.join("fillers"), 320, 180, fps).unwrap();
    let none = Availability::default();
    let mut encoders = vidcut_av::proxy::encoders(ProxyCodec::H264, &none);
    encoders.extend(vidcut_av::proxy::encoders(ProxyCodec::Av1, &none));
    for enc in encoders {
        let proxy = dir.join(format!("proxy-{}.mkv", enc.name));
        vidcut_av::proxy::write(
            &vidcut_av::decode::Input::File(a.clone()),
            320,
            180,
            &proxy,
            None,
            std::slice::from_ref(&enc),
        )
        .unwrap();
        // As `preview::apply_proxies` swaps it.
        let mut plan = plan.clone();
        for piece in &mut plan.video {
            if let VideoPiece::Source { path, image, .. } = piece {
                *path = proxy.clone();
                *image = None;
            }
        }
        let edl = dir.join(format!("timeline-{}.edl", enc.name));
        std::fs::write(&edl, vidcut_core::preview::edl(&plan, &fillers)).unwrap();
        player.load(edl.to_str().unwrap());
        let mut bad = Vec::new();
        for (j, want) in [
            (0, 10),
            (1, 11),
            (19, 29),
            (20, 30),
            (21, 31),
            (49, 59),
            (50, 60),
            (59, 69),
            (60, 100),
            (61, 101),
            (79, 119),
            (99, 139),
        ] {
            let got = player.frame_at(j as f64 / 30.0);
            if got != Some(want) {
                bad.push(format!("timeline frame {j}: got {got:?}, want {want}"));
            }
        }
        assert!(bad.is_empty(), "{}: {bad:#?}", enc.name);
    }
}

/// Playing across a cut into a clip with other audio (5.1 at 44.1 kHz after
/// stereo at 48 kHz) keeps the one audio output, in the project's format:
/// reopening it stalls the picture, then drops frames to catch up.
#[test]
fn a_cut_into_other_audio_keeps_the_audio_output() {
    let Some(mut player) = Player::new() else {
        return;
    };
    let spec = || FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 60);
    let a = fixture::ensure(&spec()).unwrap();
    let b = fixture::ensure(&spec().with_audio(AudioCodec::Aac, 44_100, 6)).unwrap();
    let fps = Rational::new(30, 1).unwrap();
    let settings = Settings {
        fps,
        width: 320,
        height: 180,
        auto_format: false,
        ..Settings::default()
    };
    let mut ed = Editor::new(Project::new(settings), Default::default());
    req(
        &mut ed,
        json!({"method": "media.add", "params": {"paths": [a, b]}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.append", "params": {"media": "m1", "range": "0f..30f"}}),
    );
    req(
        &mut ed,
        json!({"method": "edit.append", "params": {"media": "m2", "range": "0f..60f"}}),
    );
    let p = ed.project();
    let plan = flatten(p, 0, p.end()).unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-preview-audio-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let fillers = vidcut_av::filler::ensure(&dir.join("fillers"), 320, 180, fps).unwrap();
    let preview = vidcut_core::preview::build(&plan, &fillers, None);
    let edl = dir.join("timeline.edl");
    std::fs::write(&edl, &preview.edl).unwrap();

    player.mpv.request_log_messages("info").unwrap();
    player
        .mpv
        .observe_property(2, "time-pos", Format::Double)
        .unwrap();
    // Playing from half a second before the cut to half a second after it.
    let options = vidcut::live::file_options(&preview, "0.5", true, fps);
    player
        .mpv
        .command_async(
            0,
            &["loadfile", edl.to_str().unwrap(), "replace", "-1", &options],
        )
        .unwrap();
    let mut opened = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < deadline,
            "playback stalled; audio outputs: {opened:#?}"
        );
        match player.mpv.wait_event(0.002) {
            Some(Event::LogMessage { text, .. }) if text.starts_with("AO: [") => {
                opened.push(text.trim().to_string());
            }
            Some(Event::PropertyChange {
                id: 2,
                value: Value::Double(t),
                ..
            }) if t > 1.5 => break,
            Some(Event::EndFile { reason: 4, .. }) => panic!("mpv failed to play"),
            _ => {}
        }
        if player.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
            player
                .sw
                .render_rgb0(320, 180, 320 * 4, &mut player.buf)
                .unwrap();
        }
    }
    assert_eq!(opened.len(), 1, "{opened:#?}");
    assert!(opened[0].contains("48000Hz stereo 2ch"), "{opened:#?}");
}
