//! mpv EDL for timeline playback (docs/decisions/preview.md).
//!
//! The timeline plays as one virtual file: a video stream of source segments
//! (black filler in gaps) and one audio stream per audio lane (silence filler
//! in gaps and for clips without audio). Each stream ends with a one-frame
//! filler entry marked `layout=this`, which pins it to exactly one track.
//! Video `start` values are raw source timestamps carrying the half-frame
//! bias, so timeline frame `j` is shown by seeking to exactly `j / fps`.
//!
//! mpv plays one audio track at a time, so several lanes are mixed by a
//! `lavfi-complex` graph ([`audio_graph`]), which also applies clip gains and
//! track faders by timeline time (preview.md §10).

use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::model::Project;
use crate::render::{AudioPiece, RenderPlan, VideoPiece};

/// Filler media used for gaps (generated once per format by the media layer).
#[derive(Debug, Clone, PartialEq)]
pub struct Fillers {
    /// Black video, no audio, first frame at timestamp 0.
    pub black: PathBuf,
    pub black_secs: f64,
    /// Silent audio, no video, first sample at timestamp 0.
    pub silence: PathBuf,
    pub silence_secs: f64,
}

/// One EDL entry: `%<bytes>%<path>,start=<s>,length=<l><extra>`.
fn entry(out: &mut String, path: &Path, start: f64, length: f64, extra: &str) {
    let p = path.to_string_lossy();
    let _ = writeln!(
        out,
        "%{}%{p},start={start:.9},length={length:.9}{extra}",
        p.len()
    );
}

/// Entries of `filler` covering `secs` seconds (chained if longer than the
/// file), each starting at `offset` seconds into the file.
///
/// Video fillers start half a frame in, like source segments: the segment then
/// holds exactly as many frames as it is long even when the container rounds
/// timestamps (Matroska: 1 ms), and its frames show half a frame late like
/// every other segment's.
fn filler(out: &mut String, path: &Path, available: f64, offset: f64, mut secs: f64) {
    // Stay a little inside the file so no entry runs past its end.
    let chunk = (available - offset - 0.1).max(0.1);
    while secs > 1e-9 {
        let len = secs.min(chunk);
        entry(out, path, offset, len, "");
        secs -= len;
    }
}

/// What a player loads: the EDL and the audio graph for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub edl: String,
    /// `lavfi-complex` for the file, if its audio needs mixing or gain.
    pub audio_graph: Option<String>,
    /// Sources on slow storage (a network share): the player reads ahead
    /// and keeps what it read for seeking back. Set by the host, which can
    /// ask the file system.
    pub read_ahead: bool,
    /// Interlaced sources: the player deinterlaces flagged frames, one
    /// frame per frame as export does. Set by the host.
    pub deinterlace: bool,
    /// Image lists (`mf://@list`, stills and sequences): the player shows
    /// them one file a frame at the project rate. Set by the host, which
    /// writes the lists.
    pub images: bool,
}

/// Plays fresh proxies in `plan` when the project asks for them (decision
/// D41). Video paths become the proxy file and image lists are dropped, so
/// a sequence plays as that video. `origin` and `source_frame` stay, which
/// keeps the EDL's frame mapping. Audio paths stay on the originals.
/// `exists` is whether the proxy file is there: a missing file plays the
/// original. Export plans never call this.
pub fn apply_proxies(plan: &mut RenderPlan, project: &Project, exists: impl Fn(&Path) -> bool) {
    if !project.settings.use_proxies {
        return;
    }
    for piece in &mut plan.video {
        let VideoPiece::Source {
            media, path, image, ..
        } = piece
        else {
            continue;
        };
        let Some(proxy) = project.media(media).and_then(|m| m.fresh_proxy()) else {
            continue;
        };
        if !exists(&proxy.path) {
            continue;
        }
        *path = proxy.path.clone();
        *image = None;
    }
}

/// The EDL and audio graph that play `plan` in mpv.
pub fn build(plan: &RenderPlan, fillers: &Fillers) -> Preview {
    Preview {
        edl: edl(plan, fillers),
        audio_graph: audio_graph(plan),
        read_ahead: false,
        deinterlace: false,
        images: false,
    }
}

fn linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// A lane's gain at timeline time `t` as an FFmpeg expression, from where
/// its gains start, `(start s, dB)` in order: a binary search over them.
/// It nests only log2(n) deep; FFmpeg's expression parser refuses some 100
/// levels, so one `if` per span stopped working past a few dozen changes.
/// A gain holds until the next start: between clips the lane is silent, so
/// it may reach over the silence.
fn gain_at(spans: &[(f64, f64)]) -> String {
    if let [(_, db)] = spans {
        return format!("{:.6}", linear(*db));
    }
    let mid = spans.len() / 2;
    format!(
        "if(lt(t,{:.6}),{},{})",
        spans[mid].0,
        gain_at(&spans[..mid]),
        gain_at(&spans[mid..])
    )
}

/// The `lavfi-complex` graph for the audio streams of [`edl`]: each lane's
/// gains (clip gain plus track fader, switched by timeline time `t`) and an
/// `amix` that sums the lanes. `None` when a single lane plays as it is.
pub fn audio_graph(plan: &RenderPlan) -> Option<String> {
    let rate = f64::from(plan.sample_rate);
    let mut filters = Vec::new();
    let mut inputs = Vec::new();
    for (i, lane) in plan.audio.iter().enumerate() {
        let n = i + 1;
        // Where the lane's source pieces change gain, as (start s, dB). A
        // gain holds until the next: between clips the lane is silent, so
        // it may reach over the silence.
        let mut spans: Vec<(f64, f64)> = Vec::new();
        let mut at = 0i64;
        for piece in lane {
            if let AudioPiece::Source { gain_db, .. } = piece
                && spans.last().is_none_or(|last| last.1 != *gain_db)
            {
                spans.push((at as f64 / rate, *gain_db));
            }
            at += piece.samples();
        }
        let first = spans.first().map_or(0.0, |s| s.1);
        let filter = if spans.iter().all(|s| s.1 == 0.0) {
            None
        } else if spans.iter().all(|s| s.1 == first) {
            Some(format!("volume={:.6}", linear(first)))
        } else {
            Some(format!("volume=volume='{}':eval=frame", gain_at(&spans)))
        };
        match filter {
            Some(f) => {
                filters.push(format!("[aid{n}]{f}[g{n}]"));
                inputs.push(format!("[g{n}]"));
            }
            None => inputs.push(format!("[aid{n}]")),
        }
    }
    match inputs.len() {
        0 => None,
        1 if filters.is_empty() => None,
        1 => Some(filters[0].replace("[g1]", "[ao]")),
        n => {
            filters.push(format!(
                "{}amix=inputs={n}:normalize=0[ao]",
                inputs.concat()
            ));
            Some(filters.join(";"))
        }
    }
}

/// The EDL text that plays `plan` in mpv.
pub fn edl(plan: &RenderPlan, fillers: &Fillers) -> String {
    let fps = plan.fps;
    let one = fps.frames_to_secs(1);
    let mut out = String::from("# mpv EDL v0\n!new_stream\n");
    for piece in &plan.video {
        match piece {
            VideoPiece::Source {
                path,
                origin,
                source_frame,
                frames,
                ..
            } => {
                let start = origin + fps.frames_to_secs(*source_frame) - one / 2.0;
                entry(&mut out, path, start, fps.frames_to_secs(*frames), "");
            }
            VideoPiece::Black { frames } => {
                filler(
                    &mut out,
                    &fillers.black,
                    fillers.black_secs,
                    one / 2.0,
                    fps.frames_to_secs(*frames),
                );
            }
        }
    }
    entry(&mut out, &fillers.black, 0.0, one, ",layout=this");

    let rate = f64::from(plan.sample_rate);
    if plan.audio.is_empty() {
        out.push_str("!new_stream\n");
        filler(
            &mut out,
            &fillers.silence,
            fillers.silence_secs,
            0.0,
            fps.frames_to_secs(plan.frames),
        );
        entry(&mut out, &fillers.silence, 0.0, one, ",layout=this");
    }
    for lane in &plan.audio {
        out.push_str("!new_stream\n");
        for piece in lane {
            match piece {
                AudioPiece::Source {
                    path,
                    origin,
                    source_sample,
                    samples,
                    ..
                } => {
                    entry(
                        &mut out,
                        path,
                        origin + *source_sample as f64 / rate,
                        *samples as f64 / rate,
                        "",
                    );
                }
                AudioPiece::Silence { samples } => {
                    filler(
                        &mut out,
                        &fillers.silence,
                        fillers.silence_secs,
                        0.0,
                        *samples as f64 / rate,
                    );
                }
            }
        }
        entry(&mut out, &fillers.silence, 0.0, one, ",layout=this");
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::render::flatten;
    use crate::test_support::Fixture;

    fn fillers() -> Fillers {
        Fillers {
            black: "/f/black.mkv".into(),
            black_secs: 10.0,
            silence: "/f/silence.mkv".into(),
            silence_secs: 10.0,
        }
    }

    #[test]
    fn edl_layout_and_timing() {
        let mut f = Fixture::new(&[(10.0, true), (8.0, false)]);
        f.append("m1", 25, 75);
        f.append("m2", 0, 25);
        f.ok(json!({"method": "edit.delete", "params": {"clips": "c1", "lift": true}}));
        let plan = flatten(f.editor.project(), 0, 75).unwrap();
        let text = edl(&plan, &fillers());
        insta::assert_snapshot!(text, @r"
        # mpv EDL v0
        !new_stream
        %12%/f/black.mkv,start=0.020000000,length=2.000000000
        %12%/media/2.mp4,start=-0.020000000,length=1.000000000
        %12%/f/black.mkv,start=0.000000000,length=0.040000000,layout=this
        !new_stream
        %14%/f/silence.mkv,start=0.000000000,length=3.000000000
        %14%/f/silence.mkv,start=0.000000000,length=0.040000000,layout=this
        ");
    }

    #[test]
    fn long_gaps_chain_fillers_and_paths_are_byte_counted() {
        let mut f = Fixture::new(&[(10.0, true)]);
        f.append("m1", 0, 25);
        let mut plan = flatten(f.editor.project(), 0, 25).unwrap();
        plan.video.push(VideoPiece::Black { frames: 25 * 25 });
        if let VideoPiece::Source { path, .. } = &mut plan.video[0] {
            *path = "/media/kesä, öljy.mp4".into();
        }
        let text = edl(&plan, &fillers());
        let blacks = text
            .lines()
            .filter(|l| l.contains("black.mkv") && !l.contains("layout"))
            .count();
        assert_eq!(blacks, 3, "25 s of black in 9.9 s chunks:\n{text}");
        // Lengths count UTF-8 bytes: "ä" and "ö" are two bytes each.
        assert!(text.contains("%23%/media/kesä, öljy.mp4,start="), "{text}");
    }

    #[test]
    fn lanes_become_streams_mixed_with_their_gains() {
        let mut f = Fixture::new(&[(10.0, true), (4.0, true)]);
        f.append("m1", 0, 50);
        f.append("m2", 0, 25);
        let music = f.add_music("music.wav", 20.0);
        f.ok(json!({"method": "edit.overwrite", "params": {"media": music, "range": "0f..75f", "at": 0}}));
        let plan = flatten(f.editor.project(), 0, 75).unwrap();
        let text = edl(&plan, &fillers());
        assert_eq!(text.matches("!new_stream").count(), 3, "{text}");
        assert_eq!(
            audio_graph(&plan).unwrap(),
            "[aid1][aid2]amix=inputs=2:normalize=0[ao]"
        );
        // A fader is one gain for the lane; clip gains switch by time.
        f.ok(json!({"method": "track.set", "params": {"track": "A1", "volume_db": -6.0}}));
        f.ok(json!({"method": "clip.set", "params": {"clips": "c2", "gain_db": 6.0}}));
        let plan = flatten(f.editor.project(), 0, 75).unwrap();
        assert_eq!(
            audio_graph(&plan).unwrap(),
            "[aid1]volume=volume='if(lt(t,2.000000),1.000000,1.995262)':eval=frame[g1];\
             [aid2]volume=0.501187[g2];[g1][g2]amix=inputs=2:normalize=0[ao]"
        );
        // One plain lane needs no graph.
        f.ok(json!({"method": "track.remove", "params": {"track": "A1", "force": true}}));
        f.ok(json!({"method": "clip.set", "params": {"clips": "c2", "gain_db": 0.0}}));
        let plan = flatten(f.editor.project(), 0, 75).unwrap();
        assert_eq!(audio_graph(&plan), None);
    }

    /// The expressions [`gain_at`] makes: `if(lt(t,X),A,B)` and numbers.
    enum Gain {
        If(f64, Box<Gain>, Box<Gain>),
        Is(f64),
    }

    impl Gain {
        fn parse(s: &str) -> (Gain, &str) {
            if let Some(rest) = s.strip_prefix("if(lt(t,") {
                let (x, rest) = rest.split_once("),").unwrap();
                let (a, rest) = Gain::parse(rest);
                let (b, rest) = Gain::parse(rest.strip_prefix(',').unwrap());
                let node = Gain::If(x.parse().unwrap(), Box::new(a), Box::new(b));
                (node, rest.strip_prefix(')').unwrap())
            } else {
                let end = s.find([',', ')']).unwrap_or(s.len());
                (Gain::Is(s[..end].parse().unwrap()), &s[end..])
            }
        }

        fn at(&self, t: f64) -> f64 {
            match self {
                Gain::If(x, a, b) => {
                    if t < *x {
                        a.at(t)
                    } else {
                        b.at(t)
                    }
                }
                Gain::Is(v) => *v,
            }
        }

        fn depth(&self) -> usize {
            match self {
                Gain::If(_, a, b) => 1 + a.depth().max(b.depth()),
                Gain::Is(_) => 0,
            }
        }
    }

    #[test]
    fn gains_switch_by_binary_search_however_many_there_are() {
        // 2000 clips of 0.1 s cycling through four gains. One `if` per change,
        // as before, broke FFmpeg's parser past a few dozen.
        let db = [0.0, -6.0, -12.0, 3.0];
        let spans: Vec<(f64, f64)> = (0..2000).map(|i| (i as f64 * 0.1, db[i % 4])).collect();
        let expr = gain_at(&spans);
        let (gain, rest) = Gain::parse(&expr);
        assert!(rest.is_empty(), "left over: {rest:.40}");
        assert!(gain.depth() <= 11, "nests {} deep", gain.depth());
        for (i, s) in spans.iter().enumerate() {
            for t in [s.0, s.0 + 0.05, s.0 + 0.099] {
                let v = gain.at(t);
                assert!((v - linear(db[i % 4])).abs() < 1e-5, "span {i} at {t}: {v}");
            }
        }
        // Before the first span the first gain applies.
        assert!((gain.at(-1.0) - linear(db[0])).abs() < 1e-5);
        // A time after the last span still gets the last gain.
        assert!((gain.at(spans.last().unwrap().0 + 10.0) - linear(db[1999 % 4])).abs() < 1e-5);
        // Gaps are silent: a gain reaches over them to the next clip with it.
        assert_eq!(gain_at(&[(0.0, -6.0)]), "0.501187");
    }

    fn source(samples: i64, gain_db: f64) -> AudioPiece {
        AudioPiece::Source {
            media: "m1".into(),
            path: "/media/a.wav".into(),
            stream: 0,
            origin: 0.0,
            source_sample: 0,
            samples,
            gain_db,
        }
    }

    fn lane_plan(lane: Vec<AudioPiece>) -> RenderPlan {
        RenderPlan {
            fps: crate::time::Rational::new(25, 1).unwrap(),
            width: 320,
            height: 180,
            sample_rate: 48_000,
            channels: 1,
            frames: 1,
            video: Vec::new(),
            audio: vec![lane],
            range: (0, 1),
            name: "gains".into(),
        }
    }

    #[test]
    fn silence_holds_the_gain_until_the_next_start() {
        // 48 kHz. Leading silence, two -6 dB clips with a gap between them
        // (equal gains merge), a shorter gap, then +6 dB, then trailing
        // silence. The +6 dB clip therefore starts at 4.5 s.
        let sec = 48_000;
        let lane = vec![
            AudioPiece::Silence { samples: sec },
            source(sec, -6.0),
            AudioPiece::Silence { samples: sec },
            source(sec, -6.0),
            AudioPiece::Silence { samples: sec / 2 },
            source(sec, 6.0),
            AudioPiece::Silence { samples: sec },
        ];
        let graph = audio_graph(&lane_plan(lane)).unwrap();
        assert_eq!(
            graph,
            "[aid1]volume=volume='if(lt(t,4.500000),0.501187,1.995262)':eval=frame[ao]"
        );
        let (gain, rest) = Gain::parse(
            graph
                .strip_prefix("[aid1]volume=volume='")
                .unwrap()
                .strip_suffix("':eval=frame[ao]")
                .unwrap(),
        );
        assert!(rest.is_empty(), "{rest}");
        // Before the first span (the first clip starts at 1 s).
        assert!((gain.at(0.0) - linear(-6.0)).abs() < 1e-5);
        // Across the equal-gain gap, and in the gap before the next gain.
        assert!((gain.at(2.5) - linear(-6.0)).abs() < 1e-5);
        assert!((gain.at(4.0) - linear(-6.0)).abs() < 1e-5);
        // From the +6 dB clip on, including after the lane ends.
        assert!((gain.at(4.5) - linear(6.0)).abs() < 1e-5);
        assert!((gain.at(6.0) - linear(6.0)).abs() < 1e-5);
        assert!((gain.at(100.0) - linear(6.0)).abs() < 1e-5);

        // One gain is a constant, including over the leading silence.
        assert_eq!(
            audio_graph(&lane_plan(vec![
                AudioPiece::Silence { samples: sec },
                source(sec, -6.0),
            ]))
            .unwrap(),
            "[aid1]volume=0.501187[ao]"
        );
        // Silence alone needs no graph.
        assert_eq!(
            audio_graph(&lane_plan(vec![AudioPiece::Silence { samples: sec }])),
            None
        );
    }
}
