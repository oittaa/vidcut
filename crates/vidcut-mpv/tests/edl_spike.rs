//! M0 spike: how mpv's EDL and seeking behave, measured with barcode fixtures
//! rendered through the software renderer. Run with `--nocapture` to see the
//! findings; assertions pin down the behavior vidcut's time map relies on.
//!
//! Requires libmpv; tests skip (with a message) when it can't be loaded.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vidcut_av::decode::VideoReader;
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture, Sound, VideoCodec};
use vidcut_mpv::{Event, Format, Mpv, SwRenderer, Value, ffi::MPV_RENDER_UPDATE_FRAME};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A headless mpv with a software renderer and barcode readout.
struct Probe {
    mpv: Mpv,
    sw: SwRenderer,
    width: usize,
    height: usize,
    buf: Vec<u8>,
    /// Latest values of observed properties.
    props: std::collections::HashMap<String, Value>,
}

impl Probe {
    fn new(extra: &[(&str, &str)]) -> Option<Self> {
        if let Err(e) = vidcut_mpv::available() {
            eprintln!("skipping: {e}");
            return None;
        }
        let mut opts = vec![
            ("vo", "libmpv"),
            ("ao", "null"),
            ("hwdec", "no"),
            ("config", "no"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
            ("terminal", "no"),
            ("input-default-bindings", "no"),
            ("osc", "no"),
            ("hr-seek", "yes"),
            ("hr-seek-framedrop", "no"),
            ("keep-open", "always"),
            ("idle", "yes"),
            ("pause", "yes"),
            ("video-timing-offset", "0"),
        ];
        opts.extend_from_slice(extra);
        let mpv = Mpv::new(&opts).expect("mpv");
        let sw = SwRenderer::new(&mpv).expect("sw renderer");
        for (id, (name, format)) in [
            ("time-pos", Format::Double),
            ("track-list", Format::String),
            ("vid", Format::String),
            ("aid", Format::String),
        ]
        .into_iter()
        .enumerate()
        {
            mpv.observe_property(id as u64, name, format)
                .expect("observe");
        }
        Some(Self {
            mpv,
            sw,
            width: 320,
            height: 180,
            buf: vec![0; 320 * 4 * 180],
            props: Default::default(),
        })
    }

    /// Processes events until `pred` matches, keeping the video output fed.
    /// Like any mpv render thread, this one only uses render-thread-safe client
    /// calls (async commands, observed properties, `wait_event`).
    fn wait_for(&mut self, what: &str, pred: impl Fn(&Event) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(ev) = self.mpv.wait_event(0.01) {
                match &ev {
                    Event::EndFile { reason: 4, error } => {
                        panic!("mpv failed to play while waiting for {what}: error {error}")
                    }
                    Event::PropertyChange { name, value, .. } => {
                        self.props.insert(name.clone(), value.clone());
                    }
                    _ => {}
                }
                if pred(&ev) {
                    return;
                }
            }
            // Keep the VO fed; it may block until frames are consumed.
            if self.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
                self.render();
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn render(&mut self) {
        self.sw
            .render_rgb0(self.width, self.height, self.width * 4, &mut self.buf)
            .expect("render");
    }

    fn command(&self, args: &[&str]) {
        self.mpv.command_async(0, args).expect("command");
    }

    fn load(&mut self, url: &str) {
        self.command(&["loadfile", url, "replace"]);
        self.wait_for("file loaded", |e| matches!(e, Event::FileLoaded));
        self.wait_for("first frame", |e| matches!(e, Event::PlaybackRestart));
    }

    fn seek(&mut self, secs: f64) {
        // mpv treats a negative absolute seek as relative to the end of the file.
        let secs = secs.max(0.0);
        self.command(&["seek", &format!("{secs:.9}"), "absolute+exact"]);
        self.wait_for("seek", |e| matches!(e, Event::PlaybackRestart));
    }

    fn step(&mut self, back: bool) {
        self.command(&[if back {
            "frame-back-step"
        } else {
            "frame-step"
        }]);
        // A step ends paused with the new frame displayed; give mpv time to
        // deliver it, then read whatever is current.
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            self.wait_for_any(Duration::from_millis(10));
        }
    }

    /// Processes events for up to `timeout` without a predicate.
    fn wait_for_any(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(Event::PropertyChange { name, value, .. }) = self.mpv.wait_event(0.002) {
                self.props.insert(name, value);
            }
            if self.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
                self.render();
            }
        }
    }

    /// Renders the current frame and decodes its barcode.
    fn barcode(&mut self) -> Option<u32> {
        self.wait_for_any(Duration::from_millis(30));
        self.render();
        vidcut_av::barcode::read_packed(&self.buf, self.width, self.height, self.width * 4, 4, 1)
    }

    fn time_pos(&self) -> f64 {
        match self.props.get("time-pos") {
            Some(Value::Double(t)) => *t,
            _ => f64::NAN,
        }
    }

    fn prop_string(&self, name: &str) -> String {
        match self.props.get(name) {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        }
    }
}

struct Source {
    path: PathBuf,
    spec: FixtureSpec,
    /// Raw pts of the first video frame, seconds (`o`).
    o: f64,
    /// Container start time, seconds (`fs`).
    fs: f64,
}

fn source(spec: FixtureSpec) -> Source {
    let path = fixture::ensure(&spec).expect("fixture");
    let reader = VideoReader::open(&path).expect("reader");
    let tb = reader.time_base();
    let o = reader.first_pts().unwrap() as f64 * f64::from(tb.numerator())
        / f64::from(tb.denominator());
    let fs = vidcut_av::probe::probe(&path)
        .unwrap()
        .start_time
        .unwrap_or(0.0);
    Source { path, spec, o, fs }
}

fn fps(spec: &FixtureSpec) -> f64 {
    f64::from(spec.fps.0) / f64::from(spec.fps.1)
}

fn edl_entry(path: &Path, start: f64, length: f64, extra: &str) -> String {
    let p = path.to_str().unwrap();
    format!(
        "%{}%{p},start={start:.9},length={length:.9}{extra}\n",
        p.len()
    )
}

fn write_edl(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vidcut-edl-spike-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("# mpv EDL v0\n{body}")).unwrap();
    path
}

fn ts_source() -> Source {
    source(
        FixtureSpec::new(VideoCodec::H264, Container::MpegTs, 320, 180, (30, 1), 90)
            .start_offset_ms(1400),
    )
}

/// The TS source as preview will see it: remuxed into MKV with timestamps kept.
fn ts_preview_source() -> Source {
    let ts = ts_source();
    let info = vidcut_av::probe::probe(&ts.path).unwrap();
    assert!(
        vidcut_av::remux::needs_index(&info.format),
        "{}",
        info.format
    );
    let proxy = ts.path.with_extension("index.mkv");
    if !proxy.exists() {
        vidcut_av::remux::to_mkv(&ts.path, &proxy).unwrap();
    }
    let proxied = Source {
        path: proxy,
        ..source(ts.spec.clone())
    };
    let reader = VideoReader::open(&proxied.path).unwrap();
    let tb = reader.time_base();
    let o = reader.first_pts().unwrap() as f64 * f64::from(tb.numerator())
        / f64::from(tb.denominator());
    eprintln!("TS o={:.4} -> MKV remux o={o:.4}", ts.o);
    Source { o, ..proxied }
}

fn offset_mkv_source() -> Source {
    source(
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 90)
            .start_offset_ms(1400),
    )
}

/// Seeks a single file for several frames; returns (frame, shown) mismatches.
fn seek_single(p: &mut Probe, src: &Source, origin: f64) -> Vec<(u32, Option<u32>)> {
    let f = fps(&src.spec);
    let mut bad = Vec::new();
    for k in [45u32, 0, 1, 29, 30, 31, 89, 2, 59] {
        p.seek(origin + f64::from(k) / f - 0.5 / f);
        let got = p.barcode();
        if got != Some(k) {
            bad.push((k, got));
        }
    }
    bad
}

#[test]
fn single_file_time_domain() {
    let mkv = offset_mkv_source();
    for rebase in ["yes", "no"] {
        let Some(mut p) = Probe::new(&[("rebase-start-time", rebase)]) else {
            return;
        };
        p.load(mkv.path.to_str().unwrap());
        let start_pos = p.time_pos();
        let origin = if rebase == "yes" {
            mkv.o - mkv.fs
        } else {
            mkv.o
        };
        let bad = seek_single(&mut p, &mkv, origin);
        eprintln!(
            "MKV +1.4 s, rebase-start-time={rebase}: time-pos after load {start_pos:.4} (o={:.4}, fs={:.4}); seek(origin + k/f - b) mismatches: {bad:?}",
            mkv.o, mkv.fs
        );
        assert!(bad.is_empty(), "rebase-start-time={rebase}");
    }
}

#[test]
fn mpegts_needs_demuxer_offset() {
    let ts = ts_source();
    for offset in ["0", "1.5"] {
        let Some(mut p) = Probe::new(&[
            ("rebase-start-time", "no"),
            ("hr-seek-demuxer-offset", offset),
        ]) else {
            return;
        };
        p.load(ts.path.to_str().unwrap());
        let bad = seek_single(&mut p, &ts, ts.o);
        eprintln!("MPEG-TS, hr-seek-demuxer-offset={offset}: mismatches {bad:?}");
        if offset != "0" {
            assert!(bad.is_empty(), "TS with demuxer offset");
        }
    }
}

#[test]
fn edl_start_is_raw_pts() {
    let src = offset_mkv_source();
    let f = fps(&src.spec);
    let Some(mut p) = Probe::new(&[]) else { return };
    // Same segment (source frames 10..30) described both ways.
    let raw = write_edl(
        "raw.edl",
        &edl_entry(&src.path, src.o + 10.0 / f - 0.5 / f, 20.0 / f, ""),
    );
    p.load(raw.to_str().unwrap());
    p.seek(0.0);
    let raw_first = p.barcode();
    p.seek(19.0 / f);
    let raw_last = p.barcode();
    let rel = write_edl(
        "rel.edl",
        &edl_entry(&src.path, 10.0 / f - 0.5 / f, 20.0 / f, ""),
    );
    p.load(rel.to_str().unwrap());
    p.seek(0.0);
    let rel_first = p.barcode();
    eprintln!(
        "EDL start=o+t-b shows {raw_first:?}..{raw_last:?} (want 10..29); start=t-b shows {rel_first:?}"
    );
    assert_eq!(
        (raw_first, raw_last),
        (Some(10), Some(29)),
        "EDL start is a raw source timestamp"
    );
}

/// A timeline of segments from several same-rate sources: (source, first frame, frames).
fn same_rate_timeline() -> Vec<(Source, u32, u32)> {
    use Container::*;
    use VideoCodec::*;
    vec![
        (
            source(FixtureSpec::new(H264, Mp4, 320, 180, (30, 1), 90)),
            15,
            20,
        ),
        (
            source(FixtureSpec::new(Hevc10, Mkv, 320, 180, (30, 1), 90)),
            40,
            12,
        ),
        (ts_preview_source(), 3, 25),
        (
            source(FixtureSpec::new(Vp9, Webm, 320, 180, (30, 1), 60)),
            31,
            9,
        ),
        (
            source(FixtureSpec::new(H264, Mp4, 320, 180, (30, 1), 90).no_audio()),
            60,
            30,
        ),
        (
            source(FixtureSpec::new(H264, Mp4, 320, 180, (30, 1), 90)),
            0,
            15,
        ),
    ]
}

fn video_entries(timeline: &[(Source, u32, u32)], f: f64) -> String {
    let b = 0.5 / f;
    timeline
        .iter()
        .map(|(s, a, n)| edl_entry(&s.path, s.o + f64::from(*a) / f - b, f64::from(*n) / f, ""))
        .collect()
}

/// Checks every timeline frame near each boundary; returns mismatches.
/// Timeline frame j is sought at exactly j/f: the EDL start already carries the
/// half-frame bias, which places each segment's frames half a frame late.
fn check_boundaries(p: &mut Probe, timeline: &[(Source, u32, u32)], f: f64) -> Vec<String> {
    let mut bad = Vec::new();
    let mut j0 = 0u32;
    for (_, a, n) in timeline {
        for off in [0, 1, n / 2, n - 2, n - 1] {
            let j = j0 + off;
            p.seek(f64::from(j) / f);
            let want = a + off;
            let got = p.barcode();
            if got != Some(want) {
                bad.push(format!("timeline frame {j}: got {got:?}, want {want}"));
            }
        }
        j0 += n;
    }
    bad
}

#[test]
fn edl_same_rate_boundaries_are_exact() {
    let timeline = same_rate_timeline();
    let f = 30.0;
    let Some(mut p) = Probe::new(&[]) else { return };
    let edl = write_edl("same-rate.edl", &video_entries(&timeline, f));
    p.load(edl.to_str().unwrap());
    let bad = check_boundaries(&mut p, &timeline, f);
    eprintln!(
        "same-rate single-stream EDL across 4 codecs/5 containers: {} mismatches {bad:?}",
        bad.len()
    );
    assert!(bad.is_empty(), "{bad:#?}");

    // Frame stepping across every boundary, forward then back.
    let mut j0 = 0u32;
    let mut step_bad = Vec::new();
    for (i, (_, _, n)) in timeline.iter().enumerate().take(timeline.len() - 1) {
        let last = j0 + n - 1;
        p.seek(f64::from(last) / f);
        let before = p.barcode();
        p.step(false);
        let after = p.barcode();
        let next = timeline[i + 1].1;
        if after != Some(next) {
            step_bad.push(format!(
                "step forward over boundary {i}: {before:?} -> {after:?}, want {next}"
            ));
        }
        p.step(true);
        let back = p.barcode();
        if back != before {
            step_bad.push(format!(
                "step back over boundary {i}: got {back:?}, want {before:?}"
            ));
        }
        j0 += n;
    }
    eprintln!(
        "frame-step across boundaries: {} problems {step_bad:?}",
        step_bad.len()
    );
    assert!(step_bad.is_empty(), "{step_bad:#?}");
}

fn fillers(f: (u32, u32)) -> (PathBuf, PathBuf) {
    let black = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, f, 30)
            .picture(Picture::Black)
            .no_audio(),
    )
    .unwrap();
    let silence = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, f, 30)
            .audio_only()
            .sound(Sound::Silence),
    )
    .unwrap();
    (black, silence)
}

#[test]
fn edl_new_stream_layout_with_fillers() {
    let timeline = same_rate_timeline();
    let f = 30.0;
    let (black, silence) = fillers((30, 1));
    let one = 1.0 / f;
    // Video group: the clips, then a one-frame black filler that pins the layout
    // to video-only. Audio group: each clip's audio (silence for the no-audio
    // clip), then a one-frame silence filler pinning the layout to audio-only.
    let mut body = String::from("!new_stream\n");
    body += &video_entries(&timeline, f);
    body += &edl_entry(&black, 0.0, one, ",layout=this");
    body += "!new_stream\n";
    for (s, a, n) in &timeline {
        let (start, len) = (s.o + f64::from(*a) / f, f64::from(*n) / f);
        if s.spec.audio.is_some() {
            body += &edl_entry(&s.path, start, len, "");
        } else {
            body += &edl_entry(&silence, 0.0, len, "");
        }
    }
    body += &edl_entry(&silence, 0.0, one, ",layout=this");
    let edl = write_edl("new-stream.edl", &body);

    let Some(mut p) = Probe::new(&[]) else { return };
    p.load(edl.to_str().unwrap());
    p.wait_for_any(Duration::from_millis(50));
    let tracks = p.prop_string("track-list");
    let count = |kind: &str| tracks.matches(&format!("\"type\":\"{kind}\"")).count();
    let (vt, at) = (count("video"), count("audio"));
    let vid = p.prop_string("vid");
    let aid = p.prop_string("aid");
    eprintln!("!new_stream layout: {vt} video / {at} audio tracks; vid={vid} aid={aid}");
    assert_eq!(
        (vt, at),
        (1, 1),
        "layout fillers should expose exactly one video and one audio track: {tracks}"
    );

    let bad = check_boundaries(&mut p, &timeline, f);
    eprintln!("!new_stream boundaries: {} mismatches {bad:?}", bad.len());
    assert!(bad.is_empty(), "{bad:#?}");

    // Real-time playback across all boundaries must not stall or error out.
    p.seek(0.0);
    p.command(&["set", "pause", "no"]);
    let total: u32 = timeline.iter().map(|t| t.2).sum();
    let end = f64::from(total) / f;
    let started = Instant::now();
    while p.time_pos() < end - 2.0 / f && started.elapsed() < Duration::from_secs(15) {
        if let Some(Event::EndFile { reason: 4, error }) = p.mpv.wait_event(0.02) {
            panic!("playback error {error}");
        }
        if p.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
            let _ = p.barcode();
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    eprintln!(
        "played {end:.2} s of timeline in {elapsed:.2} s wall time (time-pos {:.3})",
        p.time_pos()
    );
    assert!(
        p.time_pos() >= end - 3.0 / f,
        "playback stalled at {}",
        p.time_pos()
    );
}

#[test]
fn edl_reload_latency() {
    let a = source(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let c = source(FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        320,
        180,
        (30, 1),
        90,
    ));
    let f = 30.0;
    let Some(mut p) = Probe::new(&[]) else { return };
    for entries in [20usize, 100, 400] {
        let mut body = String::new();
        for i in 0..entries {
            let s = if i % 2 == 0 { &a } else { &c };
            let first = (i * 7 % 80) as f64;
            body += &edl_entry(&s.path, s.o + first / f - 0.5 / f, 5.0 / f, "");
        }
        let edl = write_edl(&format!("reload-{entries}.edl"), &body);
        let started = Instant::now();
        p.load(edl.to_str().unwrap());
        let loaded = started.elapsed();
        // Seek near the end, as a reload "at the playhead" would.
        let seek_started = Instant::now();
        p.seek((entries * 5 - 3) as f64 / f);
        let got = p.barcode();
        eprintln!(
            "{entries} entries: load+first frame {:.0} ms, seek near end {:.0} ms (frame {got:?})",
            loaded.as_secs_f64() * 1000.0,
            seek_started.elapsed().as_secs_f64() * 1000.0
        );
        assert!(got.is_some());
    }
}

#[test]
fn edl_reverse_play() {
    let timeline = same_rate_timeline();
    let f = 30.0;
    let Some(mut p) = Probe::new(&[]) else { return };
    let edl = write_edl("reverse.edl", &video_entries(&timeline[..3], f));
    p.load(edl.to_str().unwrap());
    let total: u32 = timeline[..3].iter().map(|t| t.2).sum();
    p.seek(f64::from(total - 1) / f);
    let start = p.time_pos();
    p.command(&["set", "play-direction", "backward"]);
    p.command(&["set", "pause", "no"]);
    let started = Instant::now();
    let mut frames = Vec::new();
    while started.elapsed() < Duration::from_millis(1500) {
        let _ = p.mpv.wait_event(0.02);
        if p.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
            frames.extend(p.barcode());
        }
    }
    p.command(&["set", "pause", "yes"]);
    p.wait_for_any(Duration::from_millis(50));
    let end = p.time_pos();
    eprintln!(
        "reverse play: time-pos {start:.3} -> {end:.3} in 1.5 s; {} frames decoded, first few {:?}",
        frames.len(),
        &frames[..frames.len().min(8)]
    );
}

#[test]
fn demuxer_offset_seek_cost() {
    // Cost of the TS workaround on an indexed long-GOP source (H.264, GOP 4 s).
    let src =
        source(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 360, (30, 1), 600).gop(120));
    let f = fps(&src.spec);
    for offset in ["0", "1.5"] {
        let Some(mut p) = Probe::new(&[
            ("rebase-start-time", "no"),
            ("hr-seek-demuxer-offset", offset),
        ]) else {
            return;
        };
        p.width = 640;
        p.height = 360;
        p.buf = vec![0; 640 * 4 * 360];
        p.load(src.path.to_str().unwrap());
        let started = Instant::now();
        let mut bad = 0;
        for k in (0..600u32).step_by(37) {
            p.seek(src.o + f64::from(k) / f - 0.5 / f);
            let got = p.barcode();
            if got != Some(k) {
                bad += 1;
                eprintln!(
                    "  offset={offset}: frame {k} showed {got:?}, time-pos {:.4}",
                    p.time_pos()
                );
            }
        }
        eprintln!(
            "hr-seek-demuxer-offset={offset}: 17 exact seeks on 640x360 GOP-4s H.264 took {:.0} ms ({bad} wrong)",
            started.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(bad, 0);
    }
}

/// M6: one `!new_stream` per audio track, mixed by `lavfi-complex` (an
/// `amix` with only audio pads). The video must stay frame-exact, and both
/// streams must be heard: lane 2 is lane 1's beeps half a second later.
#[test]
fn edl_audio_streams_mix_with_lavfi_complex() {
    let src = source(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let f = 30.0;
    let one = 1.0 / f;
    let (black, silence) = fillers((30, 1));
    let mut body = String::from("!new_stream\n");
    body += &edl_entry(&src.path, src.o - 0.5 / f, 3.0, "");
    body += &edl_entry(&black, 0.0, one, ",layout=this");
    body += "!new_stream\n";
    body += &edl_entry(&src.path, src.o, 3.0, "");
    body += &edl_entry(&silence, 0.0, one, ",layout=this");
    body += "!new_stream\n";
    body += &edl_entry(&silence, 0.0, 0.5, "");
    body += &edl_entry(&src.path, src.o, 2.5, "");
    body += &edl_entry(&silence, 0.0, one, ",layout=this");
    let edl = write_edl("two-audio.edl", &body);
    let wav = edl.with_file_name("mix.wav");
    let _ = std::fs::remove_file(&wav);
    let wav_opt = wav.to_str().unwrap().to_string();
    let Some(mut p) = Probe::new(&[
        ("ao", "pcm"),
        ("ao-pcm-file", &wav_opt),
        ("audio-format", "s16"),
        ("audio-samplerate", "48000"),
        ("audio-channels", "stereo"),
    ]) else {
        return;
    };
    let graph = "[aid1][aid2]amix=inputs=2:normalize=0[ao]";
    let options = format!("lavfi-complex=%{}%{graph}", graph.len());
    p.command(&["loadfile", edl.to_str().unwrap(), "replace", "-1", &options]);
    p.wait_for("file loaded", |e| matches!(e, Event::FileLoaded));
    p.wait_for("first frame", |e| matches!(e, Event::PlaybackRestart));
    p.wait_for_any(Duration::from_millis(50));
    let tracks = p.prop_string("track-list");
    let count = |kind: &str| tracks.matches(&format!("\"type\":\"{kind}\"")).count();
    assert_eq!((count("video"), count("audio")), (1, 2), "{tracks}");
    for j in [0u32, 1, 45, 89] {
        p.seek(f64::from(j) / f);
        assert_eq!(p.barcode(), Some(j), "frame {j} with lavfi-complex");
    }
    p.seek(0.0);
    p.command(&["set", "pause", "no"]);
    let started = Instant::now();
    let mut last_report = Instant::now();
    while p.time_pos() < 3.0 - 2.0 / f && started.elapsed() < Duration::from_secs(15) {
        match p.mpv.wait_event(0.02) {
            Some(Event::EndFile { reason: 4, error }) => panic!("playback error {error}"),
            Some(Event::PropertyChange { name, value, .. }) => {
                p.props.insert(name, value);
            }
            _ => {}
        }
        if p.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
            p.render();
        }
        if last_report.elapsed() > Duration::from_millis(500) {
            last_report = Instant::now();
            eprintln!(
                "  t={:.2}s time-pos {:.3}",
                started.elapsed().as_secs_f64(),
                p.time_pos()
            );
        }
    }
    eprintln!(
        "played 3 s with two mixed audio streams in {:.2} s (time-pos {:.3})",
        started.elapsed().as_secs_f64(),
        p.time_pos()
    );
    drop(p);
    let bytes = std::fs::read(&wav).expect("mpv wrote the mix");
    let data = bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("a WAV data chunk")
        + 8;
    let left: Vec<f32> = bytes[data..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[lo, hi, ..]| f32::from(i16::from_le_bytes([lo, hi])) / 32768.0)
        .collect();
    let found = vidcut_av::beep::onsets(&left, 0.1, 4800);
    eprintln!("beep onsets in the mix (samples): {found:?}");
    // Playback starts at the first video frame, half a frame (800 samples)
    // in: the beep at 0 s is before it, and the beeps of both lanes follow
    // every half second. Audio segment edges inside an EDL are exact to
    // about 10 ms (lane 2's first beep comes after a filler).
    let want = [23_200, 47_200, 71_200, 95_200, 119_200];
    assert_eq!(found.len(), want.len(), "{found:?}");
    for (w, got) in want.iter().zip(&found) {
        assert!(
            (*got as i64 - w).abs() < 480,
            "onset {got}, want {w}: {found:?}"
        );
    }
}

/// M6: per-clip gain in preview: a `volume` filter whose expression switches
/// on the EDL timeline's time `t`, also after a seek.
#[test]
fn edl_volume_follows_timeline_time() {
    let src = source(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let f = 30.0;
    let one = 1.0 / f;
    let (black, silence) = fillers((30, 1));
    let mut body = String::from("!new_stream\n");
    body += &edl_entry(&src.path, src.o - 0.5 / f, 3.0, "");
    body += &edl_entry(&black, 0.0, one, ",layout=this");
    body += "!new_stream\n";
    body += &edl_entry(&src.path, src.o, 3.0, "");
    body += &edl_entry(&silence, 0.0, one, ",layout=this");
    let edl = write_edl("volume.edl", &body);
    let wav = edl.with_file_name("volume.wav");
    let _ = std::fs::remove_file(&wav);
    let wav_opt = wav.to_str().unwrap().to_string();
    let Some(mut p) = Probe::new(&[
        ("ao", "pcm"),
        ("ao-pcm-file", &wav_opt),
        ("audio-format", "s16"),
        ("audio-samplerate", "48000"),
        ("audio-channels", "stereo"),
    ]) else {
        return;
    };
    let graph = "[aid1]volume=volume='if(between(t,1.9,2.5),0.25,1)':eval=frame[ao]";
    let options = format!("lavfi-complex=%{}%{graph}", graph.len());
    p.command(&["loadfile", edl.to_str().unwrap(), "replace", "-1", &options]);
    p.wait_for("file loaded", |e| matches!(e, Event::FileLoaded));
    p.wait_for("first frame", |e| matches!(e, Event::PlaybackRestart));
    // Start from a seek, as preview does.
    p.seek(0.5);
    p.command(&["set", "pause", "no"]);
    let started = Instant::now();
    while p.time_pos() < 3.0 - 2.0 / f && started.elapsed() < Duration::from_secs(15) {
        match p.mpv.wait_event(0.02) {
            Some(Event::EndFile { reason: 4, error }) => panic!("playback error {error}"),
            Some(Event::PropertyChange { name, value, .. }) => {
                p.props.insert(name, value);
            }
            _ => {}
        }
        if p.sw.update() & MPV_RENDER_UPDATE_FRAME != 0 {
            p.render();
        }
    }
    drop(p);
    let bytes = std::fs::read(&wav).expect("mpv wrote the audio");
    let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
    let left: Vec<f32> = bytes[data..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[lo, hi, ..]| f32::from(i16::from_le_bytes([lo, hi])) / 32768.0)
        .collect();
    // Peak of each beep: at 1 s and 2 s of the timeline, i.e. 0.5 s and
    // 1.5 s into the recording (it starts at the seek).
    let peak = |at: usize| left[at..at + 2400].iter().fold(0f32, |m, s| m.max(s.abs()));
    let (one_s, two_s) = (peak(23_000), peak(71_000));
    eprintln!("beep peaks: 1 s {one_s:.3}, 2 s {two_s:.3}");
    assert!((one_s - 0.5).abs() < 0.05, "{one_s}");
    assert!((two_s - 0.125).abs() < 0.03, "{two_s}");
}
