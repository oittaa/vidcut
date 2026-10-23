use std::collections::HashSet;

use crate::model::{Project, TrackKind};

pub const GAIN: std::ops::RangeInclusive<f64> = -60.0..=24.0;

pub fn violations(p: &Project) -> Vec<String> {
    let mut v = Vec::new();
    let pool = p.pool();
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
                "track {} volume {} dB outside -60..24 dB",
                t.id, t.volume_db
            ));
        }
        if let Some(d) = &t.duck
            && let Err(e) = crate::duck::checked(p, &t.id, d)
        {
            v.push(format!("track {}: {}", t.id, e.message));
        }
        for x in &t.transitions {
            if !ids.insert(x.id.as_str()) {
                v.push(format!("duplicate id {}", x.id));
            }
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
            if let Some(e) = crate::ops::picture::violation(p, i, c) {
                v.push(e);
            }
            if c.is_block() {
                let video = c.is_generator() || t.kind == TrackKind::Video;
                if !c.media.is_empty()
                    || c.audio.is_some()
                    || c.video != video
                    || (c.is_generator() && c.compound.is_some())
                    || (c.title.is_some() && c.generator.is_some())
                {
                    v.push(format!("{}: a block with media, audio or no place", c.id));
                }
                if let Some(title) = &c.title {
                    if t.kind != TrackKind::Video {
                        v.push(format!("{}: a title on audio track {}", c.id, t.id));
                    }
                    if let Err(e) = crate::ops::titles::checked(title) {
                        v.push(format!("{}: {}", c.id, e.message));
                    }
                }
                let (lo, hi) = c.shown();
                if let Some(k) = &c.compound
                    && (lo < 0 || hi > k.frames)
                {
                    v.push(format!(
                        "{}: source {lo}..{hi} outside its compound (0..{})",
                        c.id, k.frames
                    ));
                }
                continue;
            }
            match pool.get(c.media.as_str()) {
                None => v.push(format!("{}: unknown media {}", c.id, c.media)),
                Some(m) => {
                    let frames = p.media_limit(m);
                    if c.video && m.info.video.is_none() {
                        v.push(format!("{}: video clip of audio-only media {}", c.id, m.id));
                    }
                    let (lo, hi) = c.shown();
                    let have = p.media_range(m, c);
                    if c.video && (lo < have.start || hi > have.end) {
                        v.push(format!(
                            "{}: source {lo}..{hi} outside media {} ({}..{})",
                            c.id, m.id, have.start, have.end
                        ));
                    }
                    if let Some(a) = &c.audio {
                        if !GAIN.contains(&a.gain_db) {
                            v.push(format!(
                                "{}: gain {} dB outside -60..24 dB",
                                c.id, a.gain_db
                            ));
                        }
                        if let Some(why) = crate::keyframes::problem(&a.gain_keys) {
                            v.push(format!("{}: {why}", c.id));
                        }
                        if a.stream as usize >= m.info.audio.len() {
                            v.push(format!(
                                "{}: no audio stream {} in {}",
                                c.id, a.stream, m.id
                            ));
                        }
                        let (s, e) = c.sound_span().unwrap_or((0.0, 1.0));
                        if s < -1e-6 || e > frames as f64 + 1e-6 || c.duration + a.tail - a.head < 1
                        {
                            v.push(format!(
                                "{}: audio source {}..{} outside media {} (0..{frames})",
                                c.id,
                                s.floor(),
                                e.ceil(),
                                m.id
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
    for (i, t) in p.subtitles.iter().enumerate() {
        if t.id != format!("S{}", i + 1) {
            v.push(format!("subtitle track {} in place {}", t.id, i + 1));
        }
        if p.subtitles[..i].iter().any(|o| o.shown) && t.shown {
            v.push(format!("{} is shown with another subtitle track", t.id));
        }
        if let Err(e) = crate::ops::subtitles::checked_style(&t.style) {
            v.push(format!("{}: {}", t.id, e.message));
        }
        let mut prev = None;
        for c in &t.cues {
            if !ids.insert(c.id.as_str()) {
                v.push(format!("duplicate id {}", c.id));
            }
            if c.duration < 1 {
                v.push(format!("{}: duration {} < 1", c.id, c.duration));
            }
            if c.start < 0 {
                v.push(format!("{}: starts before 0 ({})", c.id, c.start));
            }
            if let Err(e) = crate::ops::subtitles::checked_words(&c.text, c.duration, &c.words) {
                v.push(format!("{}: {}", c.id, e.message));
            }
            if let Err(e) = crate::ops::subtitles::checked_color(c.color.as_deref(), "cue") {
                v.push(format!("{}: {}", c.id, e.message));
            }
            if prev.is_some_and(|s| c.start < s) {
                v.push(format!("{}: out of order on {}", c.id, t.id));
            }
            prev = Some(c.start);
        }
    }
    for (person, color) in &p.speakers.colors {
        if let Err(e) = crate::ops::subtitles::checked_color(Some(color), "person's") {
            v.push(format!("{person}: {}", e.message));
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
