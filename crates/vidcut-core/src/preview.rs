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
}

/// The EDL and audio graph that play `plan` in mpv.
pub fn build(plan: &RenderPlan, fillers: &Fillers) -> Preview {
    Preview {
        edl: edl(plan, fillers),
        audio_graph: audio_graph(plan),
        read_ahead: false,
    }
}

/// Clip gain changes in one lane that preview follows (more fall back to
/// the lane's most common gain).
const MAX_GAIN_CHANGES: usize = 64;

fn linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
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
        // Gains of the lane's source pieces as (start s, end s, dB).
        let mut spans: Vec<(f64, f64, f64)> = Vec::new();
        let mut at = 0i64;
        for piece in lane {
            let len = piece.samples();
            if let AudioPiece::Source { gain_db, .. } = piece {
                let (a, b) = (at as f64 / rate, (at + len) as f64 / rate);
                match spans.last_mut() {
                    Some(last) if last.2 == *gain_db && (last.1 - a).abs() < 1e-9 => last.1 = b,
                    _ => spans.push((a, b, *gain_db)),
                }
            }
            at += len;
        }
        let first = spans.first().map_or(0.0, |s| s.2);
        let filter = if spans.iter().all(|s| s.2 == 0.0) {
            None
        } else if spans.iter().all(|s| s.2 == first) {
            Some(format!("volume={:.6}", linear(first)))
        } else if spans.len() <= MAX_GAIN_CHANGES {
            let mut expr = String::from("1");
            for (a, b, db) in spans.iter().rev().filter(|s| s.2 != 0.0) {
                expr = format!("if(between(t,{a:.6},{b:.6}),{:.6},{expr})", linear(*db));
            }
            Some(format!("volume=volume='{expr}':eval=frame"))
        } else {
            // Too many changes: the most common gain for the whole lane.
            let mut counts: Vec<(f64, usize)> = Vec::new();
            for s in &spans {
                match counts.iter_mut().find(|c| c.0 == s.2) {
                    Some(c) => c.1 += 1,
                    None => counts.push((s.2, 1)),
                }
            }
            let db = counts.iter().max_by_key(|c| c.1).map_or(0.0, |c| c.0);
            (db != 0.0).then(|| format!("volume={:.6}", linear(db)))
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
            "[aid1]volume=volume='if(between(t,2.000000,3.000000),1.995262,1)':eval=frame[g1];\
             [aid2]volume=0.501187[g2];[g1][g2]amix=inputs=2:normalize=0[ao]"
        );
        // One plain lane needs no graph.
        f.ok(json!({"method": "track.remove", "params": {"track": "A1", "force": true}}));
        f.ok(json!({"method": "clip.set", "params": {"clips": "c2", "gain_db": 0.0}}));
        let plan = flatten(f.editor.project(), 0, 75).unwrap();
        assert_eq!(audio_graph(&plan), None);
    }
}
