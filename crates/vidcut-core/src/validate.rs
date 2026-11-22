use std::collections::{HashMap, HashSet};

use crate::model::{Generator, Media, Project, TrackKind};

pub const GAIN: std::ops::RangeInclusive<f64> = -100.0..=30.0;
pub const FADER: std::ops::RangeInclusive<f64> = -100.0..=10.0;
pub const PAN: std::ops::RangeInclusive<f64> = -100.0..=100.0;

pub fn fader_silent(volume_db: f64) -> bool {
    volume_db <= *FADER.start()
}

fn compounds(p: &Project, pool: &HashMap<&str, &Media>, v: &mut Vec<String>) {
    let mut ids = HashSet::new();
    for k in &p.compounds {
        if !ids.insert(k.id.as_str()) {
            v.push(format!("duplicate compound {}", k.id));
        }
        if k.sequence_id.trim().is_empty() {
            v.push(format!("compound {} has no sequence_id", k.id));
        }
        if k.removed && !k.bin.is_empty() {
            v.push(format!("removed compound {} is in bin {:?}", k.id, k.bin));
        }
        if !k.bin.is_empty() && !p.bins.contains(&k.bin) {
            v.push(format!("{} is in unknown bin {:?}", k.id, k.bin));
        }
        if k.drop_frame && !k.fps.has_drop_frame() {
            v.push(format!("compound {} counts drop-frame at {}", k.id, k.fps));
        }
        if k.start_tc < 0 {
            v.push("compound clip starts before 00:00:00:00".into());
        }
        if k.frames < 0 {
            v.push(format!(
                "compound {} has negative length {}",
                k.id, k.frames
            ));
        }
        for c in k.tracks.iter().flat_map(|t| &t.clips) {
            if let Some(cid) = &c.compound {
                match p.compound(cid) {
                    None => v.push(format!("{}: unknown compound {cid}", c.id)),
                    Some(child) => {
                        let (lo, hi) = c.shown();
                        let frames = crate::time::rescale(child.frames, child.fps, k.fps);
                        if lo < 0 || hi > frames {
                            v.push(format!(
                                "{}: source {lo}..{hi} outside its compound (0..{frames})",
                                c.id
                            ));
                        }
                    }
                }
            } else if !c.is_block() && !pool.contains_key(c.media.as_str()) {
                v.push(format!("{}: unknown media {}", c.id, c.media));
            }
        }
    }
    let mut state: HashMap<&str, u8> = HashMap::new();
    for k in &p.compounds {
        compound_cycle(p, &k.id, &mut state, v);
    }
}

fn compound_cycle<'a>(
    p: &'a Project,
    cid: &'a str,
    state: &mut HashMap<&'a str, u8>,
    v: &mut Vec<String>,
) {
    match state.get(cid).copied() {
        Some(1) => {
            v.push(format!("compound {cid} holds itself"));
            return;
        }
        Some(_) => return,
        None => {}
    }
    state.insert(cid, 1);
    if let Some(k) = p.compound(cid) {
        for c in p.compound_tracks(k).iter().flat_map(|t| &t.clips) {
            if let Some(inner) = &c.compound {
                compound_cycle(p, inner, state, v);
            }
        }
    }
    state.insert(cid, 2);
}

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
        if !FADER.contains(&t.volume_db) {
            v.push(format!(
                "track {} volume {} dB outside -100..10 dB",
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
            if let Some(e) = crate::ops::motion::violation(c) {
                v.push(e);
            }
            if c.is_block() {
                let video = (c.is_generator() && !c.is_offline()) || t.kind == TrackKind::Video;
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
                if let Some(Generator::FourColorGradient(g)) = &c.generator
                    && let Some(why) = g.problem()
                {
                    v.push(format!("{}: {why}", c.id));
                }
                let (lo, hi) = c.shown();
                if let Some(cid) = &c.compound {
                    match p.compound(cid) {
                        None => v.push(format!("{}: unknown compound {cid}", c.id)),
                        Some(k) => {
                            let frames = p.compound_frames(k);
                            if lo < 0 || hi > frames {
                                v.push(format!(
                                    "{}: source {lo}..{hi} outside its compound (0..{frames})",
                                    c.id
                                ));
                            }
                        }
                    }
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
                                "{}: gain {} dB outside -100..30 dB",
                                c.id, a.gain_db
                            ));
                        }
                        if !PAN.contains(&a.pan) {
                            v.push(format!("{}: pan {} outside -100..100", c.id, a.pan));
                        }
                        if let Some(why) = a.equalizer.as_ref().and_then(crate::equalizer::problem)
                        {
                            v.push(format!("{}: {why}", c.id));
                        }
                        if let Some(why) = a
                            .dialogue_leveler
                            .as_ref()
                            .and_then(crate::leveler::problem)
                        {
                            v.push(format!("{}: {why}", c.id));
                        }
                        if let Some(why) =
                            a.compressor.as_ref().and_then(crate::compressor::problem)
                        {
                            v.push(format!("{}: {why}", c.id));
                        }
                        if let Some(why) = a
                            .noise_reduction
                            .as_ref()
                            .and_then(crate::noise_reduction::problem)
                        {
                            v.push(format!("{}: {why}", c.id));
                        }
                        if let Some(why) = crate::keyframes::problem(&a.gain_keys) {
                            v.push(format!("{}: {why}", c.id));
                        }
                        if let Some(why) = crate::keyframes::pan_problem(&a.pan_keys) {
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
    for g in &p.groups {
        if !ids.insert(g.id.as_str()) {
            v.push(format!("duplicate id {}", g.id));
        }
        if !g.bin.is_empty() && !p.bins.contains(&g.bin) {
            v.push(format!("group {} is in unknown bin {:?}", g.id, g.bin));
        }
        let mut held = HashSet::new();
        for c in g.clips() {
            if !pool.contains_key(c.media.as_str()) {
                v.push(format!("group {} uses unknown media {}", g.id, c.media));
            }
            if !held.insert(c.media.as_str()) {
                v.push(format!("group {} holds {} twice", g.id, c.media));
            }
            if c.head < 0 || c.tail < 0 {
                v.push(format!(
                    "group {} gives {} a negative head or tail",
                    g.id, c.media
                ));
            }
        }
        for media in &g.dropped {
            if !pool.contains_key(media.as_str()) {
                v.push(format!("group {} drops unknown media {media}", g.id));
            }
            if held.contains(media.as_str()) {
                v.push(format!("group {} holds and drops {media}", g.id));
            }
        }
    }
    for m in &p.media {
        if !m.bin.is_empty() && !p.bins.contains(&m.bin) {
            v.push(format!("{} is in unknown bin {:?}", m.id, m.bin));
        }
    }
    compounds(p, &pool, &mut v);
    for m in &p.media {
        if let Some(s) = m.subclip
            && (s.start < 0 || s.end <= s.start)
        {
            v.push(format!("{} is an empty subclip", m.id));
        }
    }
    for k in p
        .markers
        .iter()
        .chain(p.media.iter().flat_map(|m| &m.markers))
    {
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
            if let Err(e) = crate::ops::subtitles::checked_words(&c.text, c.duration, &c.times()) {
                v.push(format!("{}: {}", c.id, e.message));
            }
            for w in &c.words {
                if let Some(s) = &w.style
                    && let Err(e) = crate::ops::subtitles::checked_style(s)
                {
                    v.push(format!("{}: {}", c.id, e.message));
                }
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
    timelines(p, &mut v);
    v
}

fn timelines(p: &Project, v: &mut Vec<String>) {
    let mut ids = HashSet::new();
    for t in &p.timelines {
        if !ids.insert(t.id.as_str()) {
            v.push(format!("duplicate timeline {}", t.id));
        }
        let n = t.id.strip_prefix('t').and_then(|n| n.parse::<u64>().ok());
        if n.is_none_or(|n| n == 0 || n >= p.next_id.timeline) {
            v.push(format!(
                "timeline id {} is not t1 to t{}",
                t.id,
                p.next_id.timeline - 1
            ));
        }
        if !t.bin.is_empty() && !p.bins.contains(&t.bin) {
            v.push(format!("timeline {} is in unknown bin {:?}", t.id, t.bin));
        }
        if t.name.trim().is_empty() {
            v.push(format!("timeline {} has no name", t.id));
        }
        if t.start_tc < 0 {
            v.push(format!("timeline {} starts before 00:00:00:00", t.id));
        }
        if let Some(Err(e)) = t.grade.as_ref().map(crate::ops::grade::checked) {
            v.push(format!("timeline {}: {}", t.id, e.message));
        }
        if let Some(o) = t.own {
            if o.drop_frame && !o.fps.has_drop_frame() {
                v.push(format!("timeline {} counts drop-frame at {}", t.id, o.fps));
            }
            if crate::model::timeline_size(o.width, o.height) != (o.width, o.height) {
                v.push(format!("timeline {} is {}x{}", t.id, o.width, o.height));
            }
        }
    }
    let mut recent: Vec<&str> = p.recent.iter().map(String::as_str).collect();
    recent.sort_unstable();
    let mut all: Vec<&str> = ids.into_iter().collect();
    all.sort_unstable();
    if recent != all {
        v.push("the recent timelines are not the timelines".into());
    }
    if !p.compound_path.is_empty() && !p.subtitles.is_empty() {
        v.push("an open compound clip holds subtitles".into());
    }
    if p.compound_path.is_empty() {
        if let Some(t) = p.timeline()
            && !(t.tracks.is_empty()
                && t.marks.is_empty()
                && t.markers.is_empty()
                && t.subtitles.is_empty())
        {
            v.push(format!("open timeline {} holds its own tracks", t.id));
        }
        return;
    }
    for t in &p.timelines {
        if !t.tracks.iter().any(|x| x.id == crate::ops::MAIN) {
            v.push(format!("timeline {} has no V1 track", t.id));
        }
    }
    if let Some(oc) = p.compound_path.last()
        && let Some(k) = p.compound(&oc.compound)
        && !(k.tracks.is_empty() && k.marks.is_empty() && k.markers.is_empty())
    {
        v.push(format!("open compound {} holds its own tracks", k.name));
    }
    let mut open_seen = HashSet::new();
    for (i, oc) in p.compound_path.iter().enumerate() {
        if !open_seen.insert(oc.compound.as_str()) {
            v.push(format!("open compound {} is in path twice", oc.compound));
        }
        let Some(k) = p.compound(&oc.compound) else {
            v.push(format!("open compound {i} has unknown id {}", oc.compound));
            continue;
        };
        if k.name.trim().is_empty() {
            v.push(format!("open compound {i} has no name"));
        }
        match &oc.target {
            crate::model::CompoundTarget::Pooled { compound } => {
                if i != 0 || compound != &oc.compound || p.pooled_compound(compound).is_none() {
                    v.push(format!("open compound targets missing pooled {compound}"));
                }
            }
            crate::model::CompoundTarget::Clip { clip } => {
                let tracks: Option<&[crate::model::Track]> = if i == 0 {
                    p.timeline().map(|t| t.tracks.as_slice())
                } else {
                    p.compound(&p.compound_path[i - 1].compound)
                        .map(|pk| pk.tracks.as_slice())
                };
                let ok = tracks.is_some_and(|ts| {
                    ts.iter().flat_map(|t| &t.clips).any(|x| {
                        &x.id == clip && x.compound.as_deref() == Some(oc.compound.as_str())
                    })
                });
                if !ok {
                    v.push(format!("open compound targets missing clip {clip}"));
                }
            }
        }
    }
}

pub fn everywhere(p: &Project) -> Vec<String> {
    let mut v = violations(p);
    let mut base = p.clone();
    if !base.compound_path.is_empty() {
        match crate::ops::compound::close_all(&mut base) {
            Ok(()) => v.extend(
                violations(&base)
                    .into_iter()
                    .map(|e| format!("closed: {e}")),
            ),
            Err(e) => {
                v.push(format!("close: {}", e.message));
                return v;
            }
        }
    }
    let open = base.open_id();
    for t in base
        .timelines
        .iter()
        .filter(|t| Some(t.id.as_str()) != open)
    {
        let mut q = base.clone();
        if q.open_timeline(&t.id).is_ok() {
            v.extend(violations(&q).into_iter().map(|e| format!("{}: {e}", t.id)));
        }
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
    use serde_json::json;

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
        assert!(v.contains("gain 1000 dB outside -100..30 dB"), "{v}");
        assert!(v.contains("track V1 volume NaN dB"), "{v}");
    }

    #[test]
    fn a_compound_clip_of_unknown_media_makes_a_project_inconsistent() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 50);
        f.ok(json!({"method": "edit.compound", "params": {"clips": [a]}}));
        let mut p = f.editor.project().clone();
        let cid = p.tracks[0].clips[0].compound.clone().unwrap();
        let inner = &mut p.compound_mut(&cid).unwrap().tracks[0].clips[0];
        inner.media = "gone".into();
        let want = format!("{}: unknown media gone", inner.id);
        assert_eq!(violations(&p), vec![want]);
    }

    #[test]
    fn a_pooled_compound_clip_in_an_unknown_bin_makes_a_project_inconsistent() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 60);
        f.ok(json!({"method": "media.compound", "params": {"range": "10f..40f"}}));
        let mut p = f.editor.project().clone();
        p.compounds[0].bin = "gone".into();
        let want = format!("{} is in unknown bin \"gone\"", p.compounds[0].id);
        assert_eq!(violations(&p), vec![want]);
    }

    #[test]
    fn negative_compound_start_tc_is_reported() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 60);
        f.ok(json!({"method": "media.compound", "params": {"range": "10f..40f"}}));
        let mut p = f.editor.project().clone();
        p.compounds[0].start_tc = -1;
        assert_eq!(
            violations(&p),
            vec!["compound clip starts before 00:00:00:00".to_string()]
        );
    }

    #[test]
    fn pooled_compound_drop_frame_at_non_drop_fps_is_reported() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 60);
        f.ok(json!({"method": "media.compound", "params": {"range": "10f..40f"}}));
        let mut p = f.editor.project().clone();
        p.compounds[0].drop_frame = true;
        assert!(
            violations(&p)
                .iter()
                .any(|e| e.contains("counts drop-frame"))
        );
    }

    #[test]
    fn unknown_compound_id_on_clip_is_reported() {
        let mut f = Fixture::new(&[(4.0, true)]);
        f.append("m1", 0, 50);
        let mut p = f.editor.project().clone();
        p.tracks[0].clips[0].media.clear();
        p.tracks[0].clips[0].audio = None;
        p.tracks[0].clips[0].compound = Some("k99".into());
        assert!(
            violations(&p)
                .iter()
                .any(|e| e.contains("unknown compound k99"))
        );
    }

    #[test]
    fn cyclic_compounds_are_reported() {
        let mut f = Fixture::new(&[(4.0, true)]);
        let a = f.append("m1", 0, 30);
        let inner = f.ok(json!({"method": "edit.compound", "params": {"clips": [a]}}));
        let inner_id = inner.result["clips"][0].as_str().unwrap().to_string();
        f.ok(json!({"method": "edit.compound", "params": {"clips": [inner_id]}}));
        let mut p = f.editor.project().clone();
        let outer_kid = p.compounds[1].id.clone();
        p.compounds[0].tracks[0].clips[0].media.clear();
        p.compounds[0].tracks[0].clips[0].audio = None;
        p.compounds[0].tracks[0].clips[0].compound = Some(outer_kid);
        assert!(violations(&p).iter().any(|e| e.contains("holds itself")));
    }
}
