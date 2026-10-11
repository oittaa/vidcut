//! Render plans: exactly what an export produces, frame by frame for video and
//! sample by sample for audio.
//!
//! A plan covers a timeline range `[a, b)` at the project frame rate. Video is
//! one contiguous list of pieces (source frames or black). Audio is one or more
//! lanes, each a contiguous list of pieces (source samples or silence),
//! that the engine mixes. Every boundary is computed with exact integer math
//! (see [`Rational::frame_to_sample`]), so lengths never drift.
//!
//! **Sampling rule** (docs/decisions/export.md): output frame `j` of a video
//! piece shows the first source frame with timestamp ≥
//! `origin + (source_frame + j) / fps − ½ frame`, i.e. the nearest source
//! frame, ties to the later one. Frame-exact for sources at the project rate.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Clip, Project, TrackKind};
use crate::time::Rational;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderPlan {
    pub fps: Rational,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Output length in frames.
    pub frames: i64,
    pub video: Vec<VideoPiece>,
    /// Audio lanes to mix; each covers all of [`samples`](Self::samples).
    pub audio: Vec<Vec<AudioPiece>>,
    /// Timeline range this plan renders, `[start, end)`.
    pub range: (i64, i64),
    /// A name for the output (per-clip exports: the clip's name).
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum VideoPiece {
    Source {
        media: String,
        path: PathBuf,
        /// Raw timestamp (s) of the media's first video frame (`o`).
        origin: f64,
        /// First source position, in project frames from `origin`.
        source_frame: i64,
        frames: i64,
    },
    Black {
        frames: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AudioPiece {
    Source {
        media: String,
        path: PathBuf,
        /// Which of the media's audio streams.
        stream: u32,
        /// Raw timestamp (s) that source position 0 refers to (`o`).
        origin: f64,
        /// First source sample, counted at the plan's sample rate from `origin`.
        source_sample: i64,
        samples: i64,
        gain_db: f64,
    },
    Silence {
        samples: i64,
    },
}

impl VideoPiece {
    pub fn frames(&self) -> i64 {
        match self {
            Self::Source { frames, .. } | Self::Black { frames } => *frames,
        }
    }
}

impl AudioPiece {
    pub fn samples(&self) -> i64 {
        match self {
            Self::Source { samples, .. } | Self::Silence { samples } => *samples,
        }
    }
}

impl RenderPlan {
    /// Total audio samples (per channel).
    pub fn samples(&self) -> i64 {
        self.fps.frame_to_sample(self.frames, self.sample_rate)
    }
}

fn push_video(out: &mut Vec<VideoPiece>, piece: VideoPiece) {
    if piece.frames() <= 0 {
        return;
    }
    match (out.last_mut(), &piece) {
        (Some(VideoPiece::Black { frames }), VideoPiece::Black { frames: more }) => *frames += more,
        (
            Some(VideoPiece::Source {
                media,
                source_frame,
                frames,
                ..
            }),
            VideoPiece::Source {
                media: m2,
                source_frame: s2,
                frames: f2,
                ..
            },
        ) if media == m2 && *source_frame + *frames == *s2 => *frames += f2,
        _ => out.push(piece),
    }
}

fn push_audio(out: &mut Vec<AudioPiece>, piece: AudioPiece) {
    if piece.samples() <= 0 {
        return;
    }
    match (out.last_mut(), &piece) {
        (Some(AudioPiece::Silence { samples }), AudioPiece::Silence { samples: more }) => {
            *samples += more
        }
        (
            Some(AudioPiece::Source {
                media,
                stream,
                source_sample,
                samples,
                gain_db,
                ..
            }),
            AudioPiece::Source {
                media: m2,
                stream: st2,
                source_sample: s2,
                samples: n2,
                gain_db: g2,
                ..
            },
        ) if media == m2 && stream == st2 && gain_db == g2 && *source_sample + *samples == *s2 => {
            *samples += n2
        }
        _ => out.push(piece),
    }
}

impl Project {
    fn media_origin(&self, media: &str) -> Result<(PathBuf, f64)> {
        let m = self
            .media(media)
            .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
        let origin = m
            .info
            .video
            .as_ref()
            .map(|v| v.first_pts)
            .or_else(|| m.info.audio.first().map(|a| a.first_pts))
            .unwrap_or(0.0);
        Ok((m.path.clone(), origin))
    }
}

/// Plans the timeline range `[a, b)`.
pub fn flatten(p: &Project, a: i64, b: i64) -> Result<RenderPlan> {
    if a < 0 || b <= a {
        return Err(Error::invalid(format!("nothing to render in {a}..{b}")));
    }
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t - a, s.sample_rate);

    // Video: the topmost enabled video track with a clip shows (spec §6).
    // M1/M2 timelines have only V1, so this walks V1.
    let mut video = Vec::new();
    let tracks: Vec<_> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .collect();
    let mut t = a;
    let v1 = tracks
        .first()
        .ok_or_else(|| Error::internal("no video track"))?;
    for c in v1.clips.iter().filter(|c| c.end() > a && c.start < b) {
        let (s0, e0) = (c.start.max(a), c.end().min(b));
        push_video(&mut video, VideoPiece::Black { frames: s0 - t });
        if c.enabled && v1.video_enabled && c.video {
            let (path, origin) = p.media_origin(&c.media)?;
            push_video(
                &mut video,
                VideoPiece::Source {
                    media: c.media.clone(),
                    path,
                    origin,
                    source_frame: c.source_at(s0),
                    frames: e0 - s0,
                },
            );
        } else {
            push_video(&mut video, VideoPiece::Black { frames: e0 - s0 });
        }
        t = e0;
    }
    push_video(&mut video, VideoPiece::Black { frames: b - t });

    // Audio: one lane per track that carries audio.
    let mut audio = Vec::new();
    for track in &p.tracks {
        if track.clips.iter().all(|c| c.audio.is_none()) {
            continue;
        }
        let mut lane = Vec::new();
        let mut t = a;
        for c in &track.clips {
            let Some((ra, rb)) = c.audio_range() else {
                continue;
            };
            let (x, y) = (ra.max(a), rb.min(b));
            if y <= x {
                continue;
            }
            push_audio(
                &mut lane,
                AudioPiece::Silence {
                    samples: sample(x) - sample(t),
                },
            );
            let audio = c.audio.as_ref().expect("has audio");
            if audio.muted || track.muted {
                push_audio(
                    &mut lane,
                    AudioPiece::Silence {
                        samples: sample(y) - sample(x),
                    },
                );
            } else {
                push_audio(&mut lane, source_audio(p, c, x, sample(y) - sample(x))?);
            }
            t = y;
        }
        push_audio(
            &mut lane,
            AudioPiece::Silence {
                samples: sample(b) - sample(t),
            },
        );
        audio.push(lane);
    }

    Ok(RenderPlan {
        fps: s.fps,
        width: s.width,
        height: s.height,
        sample_rate: s.sample_rate,
        channels: s.channels,
        frames: b - a,
        video,
        audio,
        range: (a, b),
        name: "timeline".into(),
    })
}

/// Audio of clip `c` from timeline frame `x`, `samples` long.
fn source_audio(p: &Project, c: &Clip, x: i64, samples: i64) -> Result<AudioPiece> {
    let audio = c.audio.as_ref().expect("has audio");
    let (path, origin) = p.media_origin(&c.media)?;
    Ok(AudioPiece::Source {
        media: c.media.clone(),
        path,
        stream: audio.stream,
        origin,
        source_sample: p
            .settings
            .fps
            .frame_to_sample(c.source_at(x), p.settings.sample_rate),
        samples,
        gain_db: audio.gain_db,
    })
}

/// Plans one clip on its own: its video extent with its own audio (J/L
/// offsets and neighbors are left out), for per-clip export.
pub fn flatten_clip(p: &Project, clip: &str) -> Result<RenderPlan> {
    let c = p
        .clip(clip)
        .ok_or_else(|| Error::not_found(format!("no clip {clip}")))?
        .clone();
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.retain(|x| x.id == c.id);
        for x in &mut t.clips {
            if let Some(a) = x.audio.as_mut() {
                a.head = 0;
                a.tail = 0;
            }
        }
    }
    let mut plan = flatten(&alone, c.start, c.end())?;
    plan.name = c
        .name
        .clone()
        .or_else(|| p.media(&c.media).map(|m| stem(&m.name)))
        .unwrap_or_else(|| c.id.clone());
    Ok(plan)
}

fn stem(name: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::Fixture;

    fn fixture() -> Fixture {
        // 25 fps project; m1 10 s with audio, m2 4 s with audio, m3 8 s silent.
        Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)])
    }

    fn req(f: &mut Fixture, method: &str, params: serde_json::Value) {
        f.ok(json!({"method": method, "params": params}));
    }

    #[test]
    fn video_pieces_follow_clips_gaps_and_disabled_clips() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        f.append("m3", 0, 50);
        f.append("m1", 100, 150);
        req(&mut f, "edit.delete", json!({"clips": "c2", "lift": true}));
        req(&mut f, "clip.set", json!({"clips": "c3", "enabled": false}));
        let plan = flatten(f.editor.project(), 0, 200).unwrap();
        let kinds: Vec<(String, i64)> = plan
            .video
            .iter()
            .map(|v| match v {
                VideoPiece::Source {
                    media,
                    source_frame,
                    frames,
                    ..
                } => (format!("{media}@{source_frame}"), *frames),
                VideoPiece::Black { frames } => ("black".into(), *frames),
            })
            .collect();
        // c1 = m1 0..100; the lifted gap and disabled c3 merge into one black piece.
        assert_eq!(kinds, vec![("m1@0".into(), 100), ("black".into(), 100)]);
        assert_eq!(plan.frames, 200);
    }

    #[test]
    fn contiguous_source_pieces_merge() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        req(&mut f, "edit.split", json!({"at": [30, 60]}));
        let plan = flatten(f.editor.project(), 0, 100).unwrap();
        assert_eq!(plan.video.len(), 1, "a split clip renders as one read");
        assert_eq!(plan.audio[0].len(), 1);
    }

    #[test]
    fn audio_is_sample_exact_with_jl_cuts_and_silence() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        f.append("m3", 0, 25); // no audio
        f.append("m2", 10, 60);
        let p = f.editor.project();
        let plan = flatten(p, 0, 175).unwrap();
        let lane = &plan.audio[0];
        // 25 fps at 48 kHz: 1920 samples per frame.
        assert_eq!(
            lane.iter().map(AudioPiece::samples).sum::<i64>(),
            plan.samples()
        );
        assert_eq!(plan.samples(), 175 * 1920);
        assert!(matches!(
            lane[0],
            AudioPiece::Source {
                source_sample: 0,
                samples: 192_000,
                ..
            }
        ));
        assert!(matches!(lane[1], AudioPiece::Silence { samples: 48_000 }));
        assert!(matches!(
            lane[2],
            AudioPiece::Source {
                source_sample: 19_200,
                samples: 96_000,
                ..
            }
        ));
    }

    #[test]
    fn ranges_crop_and_ntsc_sample_counts_stay_exact() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        let plan = flatten(f.editor.project(), 10, 40).unwrap();
        assert!(matches!(
            plan.video[0],
            VideoPiece::Source {
                source_frame: 10,
                frames: 30,
                ..
            }
        ));
        assert!(matches!(
            plan.audio[0][0],
            AudioPiece::Source {
                source_sample: 19_200,
                samples: 57_600,
                ..
            }
        ));
        // 29.97 fps: 1601.6 samples per frame, rounded per boundary, never drifting.
        let mut p = f.editor.project().clone();
        p.settings.fps = Rational::new(30000, 1001).unwrap();
        let plan = flatten(&p, 1, 6).unwrap();
        assert_eq!(
            plan.audio[0].iter().map(AudioPiece::samples).sum::<i64>(),
            plan.samples()
        );
        assert_eq!(plan.samples(), 8008); // 5 frames
    }

    #[test]
    fn per_clip_plans_leave_out_neighbors() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        f.append("m2", 10, 60);
        req(
            &mut f,
            "edit.audio_roll",
            json!({"clip": "c1", "edge": "tail", "by": 5}),
        );
        let plan = flatten_clip(f.editor.project(), "c2").unwrap();
        assert_eq!(plan.range, (100, 150));
        assert!(matches!(
            plan.video[0],
            VideoPiece::Source {
                source_frame: 10,
                frames: 50,
                ..
            }
        ));
        assert!(matches!(
            plan.audio[0][0],
            AudioPiece::Source {
                source_sample: 19_200,
                samples: 96_000,
                ..
            }
        ));
        assert_eq!(plan.name, "2");
    }
}
