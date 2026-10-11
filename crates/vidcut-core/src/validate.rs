//! Project invariants. Checked after every edit in debug builds and tests.

use std::collections::HashSet;

use crate::model::{Project, TrackKind};

/// Clip gain and track volume in dB (decision D12).
pub const GAIN: std::ops::RangeInclusive<f64> = -60.0..=24.0;

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
        if !GAIN.contains(&t.volume_db) {
            v.push(format!(
                "track {} volume {} dB outside -60..24 dB (decision D12)",
                t.id, t.volume_db
            ));
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
                    let frames = p.media_limit(m);
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
                        if !GAIN.contains(&a.gain_db) {
                            v.push(format!(
                                "{}: gain {} dB outside -60..24 dB (decision D12)",
                                c.id, a.gain_db
                            ));
                        }
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
    for (kind, prefix) in [(TrackKind::Video, 'V'), (TrackKind::Audio, 'A')] {
        for (n, i) in p.tracks_of(kind).into_iter().enumerate() {
            let want = format!("{prefix}{}", n + 1);
            if p.tracks[i].id != want {
                v.push(format!("track {} should be {want}", p.tracks[i].id));
            }
        }
    }
    if p.track("V1").is_none() {
        v.push("no track V1".into());
    }
    for m in &p.media {
        if !m.bin.is_empty() && !p.bins.contains(&m.bin) {
            v.push(format!("{} is in unknown bin {:?}", m.id, m.bin));
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
    if let (Some(i), Some(o)) = (p.tape_marks.mark_in, p.tape_marks.mark_out)
        && i >= o
    {
        v.push(format!("tape In {i} is not before Out {o}"));
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

#[cfg(test)]
mod tests {
    use super::violations;
    use crate::test_support::Fixture;

    #[test]
    fn gains_out_of_range_make_a_project_inconsistent() {
        // A hand-edited file can hold any number; loading checks it (D12).
        let mut f = Fixture::new(&[(10.0, true)]);
        f.append("m1", 0, 25);
        let mut p = f.editor.project().clone();
        assert!(violations(&p).is_empty());
        p.tracks[0].clips[0].audio.as_mut().unwrap().gain_db = 1000.0;
        p.tracks[0].volume_db = f64::NAN;
        let v = violations(&p).join("; ");
        assert!(v.contains("gain 1000 dB outside -60..24 dB"), "{v}");
        assert!(v.contains("track V1 volume NaN dB"), "{v}");
    }
}
