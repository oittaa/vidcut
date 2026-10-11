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

use std::path::{Path, PathBuf};

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
        /// Pictures that aren't a video file's frames.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<Image>,
    },
    Black {
        frames: i64,
    },
}

/// What a source piece shows when it isn't a video file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Image {
    /// One picture (the piece's `path`), for every frame.
    Still,
    /// Numbered pictures, one a frame at `rate`: picture `k` is the file
    /// `pattern` (a full path, FFmpeg's image2 form) names with the number
    /// `first + k`. The piece's `path` is the first.
    Sequence {
        pattern: PathBuf,
        first: u64,
        count: u64,
        rate: Rational,
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
    pub(crate) fn media_origin(&self, media: &str) -> Result<(PathBuf, f64)> {
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

    /// How `media`'s pictures are read, if not as a video file's frames.
    pub fn media_image(&self, media: &str) -> Option<Image> {
        let m = self.media(media)?;
        let v = m.info.video.as_ref()?;
        match &m.sequence {
            Some(s) => Some(Image::Sequence {
                pattern: m.path.parent().unwrap_or(Path::new("")).join(&s.pattern),
                first: s.first,
                count: s.count,
                rate: v.fps,
            }),
            None if v.still => Some(Image::Still),
            None => None,
        }
    }
}

/// Plans the timeline range `[a, b)`.
pub fn flatten(p: &Project, a: i64, b: i64) -> Result<RenderPlan> {
    if a < 0 || b <= a {
        return Err(Error::invalid(format!("nothing to render in {a}..{b}")));
    }
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t - a, s.sample_rate);

    // Video: at every frame the highest video track with a visible clip
    // there shows; clips above cover the ones below.
    let visible = |t: &crate::model::Track, c: &Clip| t.video_enabled && c.enabled && c.video;
    let tracks: Vec<_> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .collect();
    let mut cuts = vec![a, b];
    for t in &tracks {
        for c in t
            .clips
            .iter()
            .filter(|c| c.end() > a && c.start < b && visible(t, c))
        {
            cuts.push(c.start.max(a));
            cuts.push(c.end().min(b));
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut video = Vec::new();
    for w in cuts.windows(2) {
        let (x, y) = (w[0], w[1]);
        let top = tracks.iter().rev().find_map(|t| {
            t.clip_covering(x)
                .map(|ci| &t.clips[ci])
                .filter(|c| visible(t, c))
        });
        match top {
            Some(c) => {
                let (path, origin) = p.media_origin(&c.media)?;
                push_video(
                    &mut video,
                    VideoPiece::Source {
                        media: c.media.clone(),
                        path,
                        origin,
                        source_frame: c.source_at(x),
                        frames: y - x,
                        image: p.media_image(&c.media),
                    },
                );
            }
            None => push_video(&mut video, VideoPiece::Black { frames: y - x }),
        }
    }

    // Audio: one lane per heard track (mute, solo), with its fader.
    let any_solo = p.any_solo();
    let mut audio = Vec::new();
    for track in &p.tracks {
        if !track.audible(any_solo) || track.clips.iter().all(|c| c.audio.is_none()) {
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
            if audio.muted {
                push_audio(
                    &mut lane,
                    AudioPiece::Silence {
                        samples: sample(y) - sample(x),
                    },
                );
            } else {
                let mut piece = source_audio(p, c, x, sample(y) - sample(x))?;
                if let AudioPiece::Source { gain_db, .. } = &mut piece {
                    *gain_db += track.volume_db;
                }
                push_audio(&mut lane, piece);
            }
            t = y;
        }
        push_audio(
            &mut lane,
            AudioPiece::Silence {
                samples: sample(b) - sample(t),
            },
        );
        if lane
            .iter()
            .any(|piece| matches!(piece, AudioPiece::Source { .. }))
        {
            audio.push(lane);
        }
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

/// Timeline frame `at` of clip `clip` alone, as export samples it, whatever
/// covers it on the tracks above (the Trim Editor shows the clip it names).
pub fn clip_frame(p: &Project, clip: &str, at: i64) -> Result<VideoPiece> {
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.retain(|x| x.id == clip);
    }
    flatten(&alone, at, at + 1)?
        .video
        .into_iter()
        .next()
        .ok_or_else(|| Error::not_found(format!("no frame {at} of {clip}")))
}

/// A media item's place on a source tape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TapeEntry {
    pub media: String,
    /// First tape frame.
    pub start: i64,
    pub frames: i64,
}

/// Plans whole media items laid end to end: the Source Tape (spec §4.1), or
/// one item for the Source Clip viewer. Media without video shows black and
/// media without audio is silent; empty media is left out.
pub fn tape(p: &Project, media: &[String]) -> Result<(RenderPlan, Vec<TapeEntry>)> {
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t, s.sample_rate);
    let (mut video, mut lane, mut entries) = (Vec::new(), Vec::new(), Vec::new());
    let mut t = 0;
    for id in media {
        let m = p
            .media(id)
            .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
        let frames = p.media_frames(m);
        if frames <= 0 {
            continue;
        }
        let (path, origin) = p.media_origin(id)?;
        if m.info.video.is_some() {
            push_video(
                &mut video,
                VideoPiece::Source {
                    media: id.clone(),
                    path: path.clone(),
                    origin,
                    source_frame: 0,
                    frames,
                    image: p.media_image(id),
                },
            );
        } else {
            push_video(&mut video, VideoPiece::Black { frames });
        }
        let samples = sample(t + frames) - sample(t);
        if m.info.audio.is_empty() {
            push_audio(&mut lane, AudioPiece::Silence { samples });
        } else {
            push_audio(
                &mut lane,
                AudioPiece::Source {
                    media: id.clone(),
                    path,
                    stream: 0,
                    origin,
                    source_sample: 0,
                    samples,
                    gain_db: 0.0,
                },
            );
        }
        entries.push(TapeEntry {
            media: id.clone(),
            start: t,
            frames,
        });
        t += frames;
    }
    if t == 0 {
        return Err(Error::invalid("no media to show"));
    }
    let plan = RenderPlan {
        fps: s.fps,
        width: s.width,
        height: s.height,
        sample_rate: s.sample_rate,
        channels: s.channels,
        frames: t,
        video,
        audio: vec![lane],
        range: (0, t),
        name: "tape".into(),
    };
    Ok((plan, entries))
}

/// One media range covered by a Source Tape span (decision D17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanPiece {
    pub media: String,
    /// Source frames `[start, end)`.
    pub range: (i64, i64),
}

/// The pieces of a half-open tape span: the tail of the first clip, every
/// whole clip in between, and the head of the last (decision D17).
pub fn span_pieces(entries: &[TapeEntry], mark_in: i64, mark_out: i64) -> Vec<SpanPiece> {
    let mut out = Vec::new();
    for e in entries {
        let end = e.start + e.frames;
        let a = mark_in.max(e.start);
        let b = mark_out.min(end);
        if b > a {
            out.push(SpanPiece {
                media: e.media.clone(),
                range: (a - e.start, b - e.start),
            });
        }
    }
    out
}

/// Where a lone In or Out stays on its own clip (decision D5). Both marks
/// use the tape range as given, which may cross clips. `None` when neither
/// mark is set or a lone mark does not fall on a clip.
pub fn active_span(
    entries: &[TapeEntry],
    mark_in: Option<i64>,
    mark_out: Option<i64>,
) -> Option<(i64, i64)> {
    match (mark_in, mark_out) {
        (None, None) => None,
        (Some(i), Some(o)) => (o > i).then_some((i, o)),
        (Some(i), None) => {
            let end = entries
                .iter()
                .find(|e| e.start <= i && i < e.start + e.frames)
                .map(|e| e.start + e.frames)?;
            (end > i).then_some((i, end))
        }
        (None, Some(o)) => {
            let frame = o - 1;
            let start = entries
                .iter()
                .find(|e| e.start <= frame && frame < e.start + e.frames)
                .map(|e| e.start)?;
            (o > start).then_some((start, o))
        }
    }
}

/// Both marks set and covering more than one clip (decision D17).
pub fn crossing_span(
    entries: &[TapeEntry],
    mark_in: Option<i64>,
    mark_out: Option<i64>,
) -> Option<(i64, i64)> {
    let (i, o) = (mark_in?, mark_out?);
    (span_pieces(entries, i, o).len() > 1).then_some((i, o))
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
    fn tape_lays_whole_media_end_to_end() {
        // Spec §4.1: the Source Tape plays every pool clip end to end.
        let f = fixture();
        let ids: Vec<String> = ["m3", "m1"].iter().map(|s| s.to_string()).collect();
        let (plan, entries) = tape(f.editor.project(), &ids).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.media.as_str(), e.start, e.frames))
                .collect::<Vec<_>>(),
            vec![("m3", 0, 200), ("m1", 200, 250)]
        );
        assert_eq!(plan.frames, 450);
        assert_eq!(plan.video.len(), 2);
        // m3 has no audio: silence, then m1's audio, exactly as long as the video.
        let lane = &plan.audio[0];
        assert!(matches!(lane[0], AudioPiece::Silence { samples: 384_000 }));
        assert!(matches!(
            lane[1],
            AudioPiece::Source {
                samples: 480_000,
                ..
            }
        ));
        assert_eq!(
            lane.iter().map(AudioPiece::samples).sum::<i64>(),
            plan.samples()
        );
        assert!(tape(f.editor.project(), &[]).is_err());
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

    fn frames_of(plan: &RenderPlan) -> Vec<(Option<&str>, i64, i64)> {
        plan.video
            .iter()
            .map(|v| match v {
                VideoPiece::Source {
                    media,
                    source_frame,
                    frames,
                    ..
                } => (Some(media.as_str()), *source_frame, *frames),
                VideoPiece::Black { frames } => (None, 0, *frames),
            })
            .collect()
    }

    #[test]
    fn the_highest_visible_track_shows() {
        // Clips on higher tracks cover the ones below.
        let mut f = fixture();
        f.append("m1", 0, 100);
        req(
            &mut f,
            "edit.overwrite",
            json!({"media": "m3", "range": "0f..20f", "at": 30, "track": "V2"}),
        );
        req(
            &mut f,
            "edit.overwrite",
            json!({"media": "m2", "range": "50f..60f", "at": 35, "track": "V3"}),
        );
        let plan = flatten(f.editor.project(), 0, 100).unwrap();
        assert_eq!(
            frames_of(&plan),
            vec![
                (Some("m1"), 0, 30),
                (Some("m3"), 0, 5),
                (Some("m2"), 50, 10),
                (Some("m3"), 15, 5),
                (Some("m1"), 50, 50)
            ]
        );
        // A disabled clip, or a track with its video off, shows what is under it.
        req(&mut f, "clip.set", json!({"clips": "c3", "enabled": false}));
        req(
            &mut f,
            "track.set",
            json!({"track": "V2", "video_enabled": false}),
        );
        let plan = flatten(f.editor.project(), 0, 100).unwrap();
        assert_eq!(frames_of(&plan), vec![(Some("m1"), 0, 100)]);
    }

    #[test]
    fn tracks_mix_with_mute_solo_and_faders() {
        // Mute and solo also apply to export; the fader adds gain.
        let mut f = fixture();
        f.append("m1", 0, 100);
        let music = f.add_music("music.wav", 20.0);
        req(
            &mut f,
            "edit.overwrite",
            json!({"media": music, "range": "0f..100f", "at": 0}),
        );
        req(
            &mut f,
            "track.set",
            json!({"track": "A1", "volume_db": -6.0}),
        );
        let lanes = |f: &Fixture| -> Vec<(String, f64)> {
            flatten(f.editor.project(), 0, 100)
                .unwrap()
                .audio
                .iter()
                .map(|lane| match &lane[0] {
                    AudioPiece::Source { media, gain_db, .. } => (media.clone(), *gain_db),
                    AudioPiece::Silence { .. } => ("silence".into(), 0.0),
                })
                .collect()
        };
        assert_eq!(
            lanes(&f),
            vec![("m1".to_string(), 0.0), (music.clone(), -6.0)]
        );
        req(&mut f, "track.set", json!({"track": "A1", "solo": true}));
        assert_eq!(lanes(&f), vec![(music.clone(), -6.0)]);
        req(&mut f, "track.set", json!({"track": "A1", "muted": true}));
        assert_eq!(lanes(&f), Vec::<(String, f64)>::new(), "solo and muted");
        req(
            &mut f,
            "track.set",
            json!({"track": "A1", "solo": false, "muted": false}),
        );
        req(&mut f, "track.set", json!({"track": "V1", "muted": true}));
        assert_eq!(lanes(&f), vec![(music, -6.0)]);
    }
}
