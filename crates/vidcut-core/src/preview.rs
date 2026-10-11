//! mpv EDL for timeline playback (docs/decisions/preview.md).
//!
//! The timeline plays as one virtual file: a video stream of source segments
//! (black filler in gaps) and an audio stream (silence filler in gaps and for
//! clips without audio). Each stream ends with a one-frame filler entry marked
//! `layout=this`, which pins it to exactly one track. Video `start` values are
//! raw source timestamps carrying the half-frame bias, so timeline frame `j` is
//! shown by seeking to exactly `j / fps`.

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

    out.push_str("!new_stream\n");
    let rate = f64::from(plan.sample_rate);
    // M2 timelines have one audio lane (V1); several lanes are mixed in M6.
    match plan.audio.first() {
        Some(lane) => {
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
        }
        None => filler(
            &mut out,
            &fillers.silence,
            fillers.silence_secs,
            0.0,
            fps.frames_to_secs(plan.frames),
        ),
    }
    entry(&mut out, &fillers.silence, 0.0, one, ",layout=this");
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
}
