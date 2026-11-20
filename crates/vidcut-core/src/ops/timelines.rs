use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Project, Settings, TimelineSettings, TrackKind};
use crate::request::{TimelineFormat, TimelineNew, TimelineSet};

pub const RECENT_SHOWN: usize = 10;

pub fn find(p: &Project, timeline: &str) -> Result<String> {
    let t = (p.timelines.iter().find(|t| t.id == timeline))
        .or_else(|| p.timelines.iter().find(|t| t.name == timeline))
        .ok_or_else(|| Error::not_found(format!("no timeline {timeline:?}")))?;
    Ok(t.id.clone())
}

fn chosen(p: &Project, timeline: Option<&str>) -> Result<String> {
    match timeline {
        Some(t) => find(p, t),
        None => {
            (p.open_id().map(str::to_string)).ok_or_else(|| Error::not_found("no timeline is open"))
        }
    }
}

pub fn free_name(p: &Project, name: &str, but: Option<&str>) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::usage("a timeline needs a name"));
    }
    let taken = (p.timelines.iter()).any(|t| t.name == name && Some(t.id.as_str()) != but);
    match taken {
        true => Err(Error::invalid(format!("a timeline is named {name:?}"))),
        false => Ok(()),
    }
}

pub fn list(p: &Project) -> Value {
    let rows: Vec<Value> = (p.sheets())
        .map(|(t, s)| {
            let f = t.own.unwrap_or_else(|| TimelineSettings::of(&p.settings));
            let frames = (s.tracks.iter())
                .flat_map(|t| t.clips.iter().map(|c| c.end()))
                .max()
                .unwrap_or(0);
            let tc = crate::time::format_timecode(0, f.fps, t.start_tc, f.counts_drop_frame());
            let mut v = json!({
                "id": t.id,
                "name": t.name,
                "bin": t.bin,
                "start": tc,
                "fps": f.fps,
                "width": f.width,
                "height": f.height,
                "frames": frames,
            });
            if t.own.is_some() {
                v["own"] = json!(true);
            }
            v
        })
        .collect();
    let mut out = json!({ "timelines": rows, "open": p.open_id(), "recent": p.recent });
    if !p.compound_path.is_empty() {
        out["compound_path"] = json!(super::compound::breadcrumbs(p));
    }
    out
}

fn own_of(base: TimelineSettings, f: &TimelineFormat) -> Result<TimelineSettings> {
    let mut own = base;
    if let Some(fps) = f.fps {
        own.fps = fps;
        own.drop_frame &= fps.has_drop_frame();
    }
    if f.width.is_some() || f.height.is_some() {
        (own.width, own.height) = crate::model::timeline_size(
            f.width.unwrap_or(own.width),
            f.height.unwrap_or(own.height),
        );
    }
    if let Some(d) = f.drop_frame {
        if d && !own.fps.has_drop_frame() {
            return Err(Error::invalid(format!(
                "drop-frame timecode is for 29.97, 59.94 and 119.88 fps, not {}",
                own.fps.decimal()
            )));
        }
        own.drop_frame = d;
    }
    if let Some(colors) = f.timeline_colors {
        own.timeline_colors = colors;
    }
    Ok(own)
}

fn settings_with(p: &Project, f: TimelineSettings) -> Settings {
    Settings {
        fps: f.fps,
        width: f.width,
        height: f.height,
        drop_frame: f.drop_frame,
        timeline_colors: f.timeline_colors,
        ..p.settings.clone()
    }
}

pub fn new(p: &mut Project, n: &TimelineNew) -> Result<String> {
    let name = n.name.clone().unwrap_or_else(|| p.next_timeline_name());
    free_name(p, &name, None)?;
    let bin = n.bin.clone().unwrap_or_default();
    if !super::has_bin(p, &bin) {
        return Err(Error::not_found(format!(
            "no bin {bin:?}; add it with bins.add"
        )));
    }
    let video = n.video_tracks.unwrap_or(1);
    let audio = n.audio_tracks.unwrap_or(1);
    if !(1..=super::VIDEO_TRACKS).contains(&video) {
        return Err(Error::usage(format!(
            "video_tracks must be 1..{}",
            super::VIDEO_TRACKS
        )));
    }
    if audio > super::AUDIO_TRACKS {
        return Err(Error::usage(format!(
            "audio_tracks must be 0..{}",
            super::AUDIO_TRACKS
        )));
    }
    let own = match &n.own {
        Some(f) => Some(own_of(TimelineSettings::of(&p.settings), f)?),
        None => None,
    };
    let format = settings_with(p, own.unwrap_or_else(|| TimelineSettings::of(&p.settings)));
    let start = match &n.start_tc {
        Some(tc) => format.start_frames(tc)?,
        None => format.first_hour(),
    };
    super::compound::close_all(p)?;
    let mut t = p.new_timeline(name, bin);
    (t.own, t.start_tc) = (own, start);
    let id = t.id.clone();
    p.add_timeline(t);
    for _ in 1..video {
        super::add_track(p, TrackKind::Video);
    }
    for _ in 0..audio {
        super::add_track(p, TrackKind::Audio);
    }
    Ok(id)
}

pub fn open(p: &mut Project, timeline: &str) -> Result<String> {
    let id = find(p, timeline)?;
    super::compound::close_all(p)?;
    p.open_timeline(&id)?;
    Ok(id)
}

pub fn rename(p: &mut Project, timeline: &str, name: &str) -> Result<()> {
    let id = find(p, timeline)?;
    free_name(p, name, Some(&id))?;
    let t = p.timelines.iter_mut().find(|t| t.id == id).expect("found");
    t.name = name.to_string();
    Ok(())
}

pub fn copy_name(p: &Project, name: &str) -> String {
    let used = |n: &str| p.timelines.iter().any(|t| t.name == n);
    let first = format!("{name} copy");
    if !used(&first) {
        return first;
    }
    (1..)
        .map(|k| format!("{name} copy {k}"))
        .find(|n| !used(n))
        .expect("a free name")
}

pub fn duplicate(p: &mut Project, timeline: Option<&str>) -> Result<String> {
    let id = chosen(p, timeline)?;
    super::compound::close_all(p)?;
    let source = p.whole_timeline(&id).expect("found");
    let fresh = p.new_timeline(copy_name(p, &source.name), source.bin.clone());
    let new_id = fresh.id.clone();
    let mut copy = crate::model::Timeline {
        id: fresh.id,
        name: fresh.name,
        marks: Default::default(),
        ..source
    };
    fresh_ids(p, &mut copy);
    p.add_timeline(copy);
    Ok(new_id)
}

fn fresh_ids(p: &mut Project, t: &mut crate::model::Timeline) {
    let mut clips = std::collections::HashMap::new();
    for c in t.tracks.iter_mut().flat_map(|t| &mut t.clips) {
        let id = p.alloc_clip_id();
        clips.insert(std::mem::replace(&mut c.id, id.clone()), id);
    }
    let renamed = |id: &mut String| {
        if let Some(new) = clips.get(id) {
            id.clone_from(new);
        }
    };
    for track in &mut t.tracks {
        for x in &mut track.transitions {
            x.id = p.alloc_transition_id();
            x.from.iter_mut().chain(x.to.iter_mut()).for_each(renamed);
        }
        for c in &mut track.clips {
            if let Some(f) = c.title.as_mut().and_then(|t| t.follows.as_mut()) {
                renamed(&mut f.clip);
            }
        }
    }
    for m in &mut t.markers {
        m.id = p.alloc_marker_id();
    }
    for q in t.subtitles.iter_mut().flat_map(|s| &mut s.cues) {
        q.id = p.alloc_cue_id();
    }
}

pub fn delete(p: &mut Project, timelines: &[String]) -> Result<Vec<String>> {
    let ids: Vec<String> = timelines
        .iter()
        .map(|t| find(p, t))
        .collect::<Result<_>>()?;
    if !ids.is_empty() {
        super::compound::close_all(p)?;
    }
    let open = p.open_id().map(str::to_string);
    let next = open.as_ref().filter(|o| ids.contains(o)).and_then(|o| {
        let at = p.timelines.iter().position(|t| &t.id == o)?;
        let left = |t: &&crate::model::Timeline| !ids.contains(&t.id);
        (p.timelines[..at].iter().rev().find(left))
            .or_else(|| p.timelines[at + 1..].iter().find(left))
            .map(|t| t.id.clone())
    });
    let mut all = p.take_timelines();
    all.retain(|t| !ids.contains(&t.id));
    p.recent.retain(|r| !ids.contains(r));
    if let Some(next) = next {
        p.recent.retain(|r| r != &next);
        p.recent.insert(0, next);
    }
    p.put_timelines(all);
    Ok(ids)
}

pub fn set(p: &mut Project, s: &TimelineSet) -> Result<()> {
    let id = chosen(p, s.timeline.as_deref())?;
    let t = p.whole_timeline(&id).expect("found");
    let before = t.own.unwrap_or_else(|| TimelineSettings::of(&p.settings));
    let mut own = match s.use_project {
        Some(true) => None,
        Some(false) => Some(t.own.unwrap_or(before)),
        None => t.own,
    };
    if let Some(f) = &s.own {
        own = Some(own_of(own.unwrap_or(before), f)?);
    }
    let after = own.unwrap_or_else(|| TimelineSettings::of(&p.settings));
    let rate_changes =
        after.fps != before.fps || after.counts_drop_frame() != before.counts_drop_frame();
    if rate_changes && t.has_clips() {
        return Err(Error::invalid(
            "the frame rate and drop-frame of a timeline change only while it has no clips",
        ));
    }
    let start = match &s.start_tc {
        Some(tc) => settings_with(p, after).start_frames(tc)?,
        None if rate_changes => {
            settings_with(p, after).relabel(t.start_tc, &settings_with(p, before))
        }
        None => t.start_tc,
    };
    let mut all = p.take_timelines();
    let t = all.iter_mut().find(|t| t.id == id).expect("found");
    (t.own, t.start_tc) = (own, start);
    if let Some(r) = super::conform::Rates::new(before.fps, after.fps) {
        t.markers
            .iter_mut()
            .for_each(|k| super::conform::marker(k, r));
        let cues = t.subtitles.iter_mut().flat_map(|s| &mut s.cues);
        cues.for_each(|c| super::conform::cue(c, r));
        super::conform::marks(&mut t.marks, r);
    }
    p.put_timelines(all);
    Ok(())
}
