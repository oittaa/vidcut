//! Project invariants. Checked after every edit in debug builds and tests.

use std::collections::HashSet;

use crate::model::{Project, TrackKind};

/// Returns every invariant violation found (empty when the project is valid).
pub fn violations(p: &Project) -> Vec<String> {
    let mut v = Vec::new();
    let mut ids = HashSet::new();
    for m in &p.media {
        if !ids.insert(m.id.as_str()) {
            v.push(format!("duplicate id {}", m.id));
        }
    }
    let mut seen_tracks = HashSet::new();
    for (i, t) in p.tracks.iter().enumerate() {
        if !seen_tracks.insert(t.id.as_str()) {
            v.push(format!("duplicate track {}", t.id));
        }
        let expected_prefix = match t.kind {
            TrackKind::Video => 'V',
            TrackKind::Audio => 'A',
        };
        if !t.id.starts_with(expected_prefix) {
            v.push(format!("track {} has kind {:?}", t.id, t.kind));
        }
        if i > 0 && t.kind == TrackKind::Video && p.tracks[i - 1].kind == TrackKind::Audio {
            v.push(format!("video track {} after audio tracks", t.id));
        }
        let mut prev_end: Option<i64> = None;
        let mut prev_audio_end: Option<i64> = None;
        for c in &t.clips {
            if !ids.insert(c.id.as_str()) {
                v.push(format!("duplicate id {}", c.id));
            }
            if c.duration < 1 {
                v.push(format!("{}: duration {} < 1", c.id, c.duration));
            }
            if c.start < 0 {
                v.push(format!("{}: starts before 0 ({})", c.id, c.start));
            }
            if prev_end.is_some_and(|e| c.start < e) {
                v.push(format!("{} overlaps the previous clip on {}", c.id, t.id));
            }
            prev_end = Some(c.end());
            match p.media(&c.media) {
                None => v.push(format!("{}: unknown media {}", c.id, c.media)),
                Some(m) => {
                    let frames = p.media_frames(m);
                    if c.video && m.info.video.is_none() {
                        v.push(format!("{}: video clip of audio-only media {}", c.id, m.id));
                    }
                    if c.video && (c.source_in < 0 || c.source_in + c.duration > frames) {
                        v.push(format!(
                            "{}: source {}..{} outside media {} (0..{frames})",
                            c.id,
                            c.source_in,
                            c.source_in + c.duration,
                            m.id
                        ));
                    }
                    if let Some(a) = &c.audio {
                        if a.stream as usize >= m.info.audio.len() {
                            v.push(format!(
                                "{}: no audio stream {} in {}",
                                c.id, a.stream, m.id
                            ));
                        }
                        let (s, e) = (c.source_in + a.head, c.source_in + c.duration + a.tail);
                        if s < 0 || e > frames || e - s < 1 {
                            v.push(format!(
                                "{}: audio source {s}..{e} outside media {} (0..{frames})",
                                c.id, m.id
                            ));
                        }
                        let (as_, ae) = c.audio_range().expect("has audio");
                        if as_ < 0 {
                            v.push(format!("{}: audio starts before 0", c.id));
                        }
                        if prev_audio_end.is_some_and(|pe| as_ < pe) {
                            v.push(format!(
                                "{}: audio overlaps the previous clip's audio on {}",
                                c.id, t.id
                            ));
                        }
                        prev_audio_end = Some(ae);
                    }
                    if !c.video && c.audio.is_none() {
                        v.push(format!("{}: has neither video nor audio", c.id));
                    }
                }
            }
            if t.kind == TrackKind::Audio && c.video {
                v.push(format!("{}: video clip on audio track {}", c.id, t.id));
            }
        }
    }
    for k in &p.markers {
        if !ids.insert(k.id.as_str()) {
            v.push(format!("duplicate id {}", k.id));
        }
    }
    if let (Some(i), Some(o)) = (p.marks.mark_in, p.marks.mark_out)
        && i >= o
    {
        v.push(format!("timeline In {i} is not before Out {o}"));
    }
    v
}

/// Whether track V1 has no gaps between its clips (it may start after 0 only
/// if it is empty).
pub fn v1_gapless(p: &Project) -> bool {
    let Some(t) = p.track("V1") else { return true };
    let mut at = 0;
    for c in &t.clips {
        if c.start != at {
            return false;
        }
        at = c.end();
    }
    true
}
