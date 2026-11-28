use std::path::Path;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{
    Album, ApplyKeys, Clip, Grade, GradeNode, GradeTool, Project, Still, StillSource, TrackKind,
};
use crate::request::GradeLevel;

use super::grade::{self, GradeOf};
use super::locate;

pub fn color_clip(p: &Project, at: i64) -> Option<String> {
    p.tracks
        .iter()
        .rev()
        .filter(|t| t.kind == TrackKind::Video && t.video_enabled)
        .flat_map(|t| t.clips.iter())
        .find(|c| c.enabled && c.start <= at && at < c.end() && super::picture::takes_effects(c))
        .map(|c| c.id.clone())
}

pub fn graded_clip(p: &Project, at: i64) -> Result<String> {
    color_clip(p, at)
        .ok_or_else(|| Error::not_found(format!("no clip at frame {at} takes a grade")))
}

pub fn keyed(g: &Grade) -> bool {
    g.is_keyed() || g.kept_layers.iter().any(Grade::is_keyed)
}

fn found<'a>(p: &'a Project, id: &str) -> Result<&'a Still> {
    p.still(id)
        .ok_or_else(|| Error::not_found(format!("no still {id}")))
}

pub fn open_timeline(p: &Project) -> Result<String> {
    p.checked_out_timeline()
        .map(str::to_string)
        .ok_or_else(|| Error::invalid("a still is of a timeline: close the compound clip first"))
}

fn origin(p: &Project, c: &Clip) -> f64 {
    let fps = p.fps();
    let frames = match p.clip_compound(c) {
        Some(k) => k.timecode_at(fps),
        None => p.media(&c.media).map_or(0.0, |m| m.timecode_at(fps)),
    };
    frames / fps.as_f64()
}

fn source_of(p: &Project, clip: &str, frame: Option<i64>) -> Result<StillSource> {
    let (ti, ci) = locate(p, clip)?;
    let c = &p.tracks[ti].clips[ci];
    let (a, b) = (c.content_at(c.start), c.content_at(c.end() - 1));
    Ok(StillSource {
        first: a.min(b),
        last: a.max(b),
        at: c.content_at(frame.unwrap_or(c.start)),
        origin: origin(p, c),
    })
}

fn album_found<'a>(p: &'a Project, id: &str) -> Result<&'a Album> {
    p.album(id)
        .ok_or_else(|| Error::not_found(format!("no album {id}")))
}

pub fn album_of(p: &Project, album: Option<&str>) -> Result<String> {
    match album {
        Some(id) => Ok(album_found(p, id)?.id.clone()),
        None => (p.albums.first())
            .map(|a| a.id.clone())
            .ok_or_else(|| Error::internal("the Gallery has no album")),
    }
}

fn named(name: &str) -> Result<String> {
    match name.trim() {
        "" => Err(Error::usage("give the album a name")),
        name => Ok(name.to_string()),
    }
}

pub fn add_album(p: &mut Project, name: Option<&str>) -> Result<String> {
    let number = p.next_id.album;
    let name = match name {
        Some(n) => named(n)?,
        None => format!("Stills {number}"),
    };
    let id = p.alloc_album_id();
    p.albums.push(Album {
        id: id.clone(),
        name,
    });
    Ok(id)
}

pub fn rename_album(p: &mut Project, id: &str, name: &str) -> Result<()> {
    let name = named(name)?;
    album_found(p, id)?;
    if let Some(a) = p.albums.iter_mut().find(|a| a.id == id) {
        a.name = name;
    }
    Ok(())
}

pub fn remove_album(p: &mut Project, id: &str) -> Result<()> {
    album_found(p, id)?;
    if p.albums.len() == 1 {
        return Err(Error::invalid("the Gallery keeps one album at least"));
    }
    p.albums.retain(|a| a.id != id);
    p.stills.retain(|s| s.album != id);
    Ok(())
}

pub fn move_stills(p: &mut Project, ids: &[String], album: &str) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::usage("give the stills to move"));
    }
    album_found(p, album)?;
    for id in ids {
        found(p, id)?;
    }
    for s in p.stills.iter_mut().filter(|s| ids.contains(&s.id)) {
        s.album = album.to_string();
    }
    Ok(())
}

pub fn albums(p: &Project) -> Value {
    let albums: Vec<Value> = (p.albums.iter())
        .map(|a| {
            let stills = p.stills.iter().filter(|s| s.album == a.id).count();
            json!({ "id": a.id, "name": a.name, "stills": stills })
        })
        .collect();
    json!(albums)
}

pub fn grab(
    p: &mut Project,
    (clip, frame): (&str, i64),
    (album, picture): (&str, String),
) -> Result<String> {
    let timeline = open_timeline(p)?;
    let album = album_found(p, album)?.id.clone();
    let grade = grade::whole(p, clip)?;
    let source = source_of(p, clip, Some(frame))?;
    let timeline_grade = p.timeline_grade().cloned();
    let id = p.alloc_still_id();
    p.stills.push(Still {
        id: id.clone(),
        album,
        clip: clip.to_string(),
        timeline,
        frame,
        source,
        grade,
        timeline_grade,
        label: String::new(),
        picture,
    });
    Ok(id)
}

pub fn remove(p: &mut Project, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::usage("give the stills to delete"));
    }
    for id in ids {
        found(p, id)?;
    }
    p.stills.retain(|s| !ids.contains(&s.id));
    Ok(())
}

fn refused(id: &str, which: &str, what: &str) -> Error {
    Error::invalid(format!(
        "the {which} of {id} has keyframes: {what} takes a grade with none"
    ))
}

fn nodes_mut(g: &mut Grade) -> impl Iterator<Item = &mut GradeNode> {
    let Grade {
        nodes,
        layers,
        kept_layers,
        ..
    } = g;
    let above = layers.iter_mut().chain(kept_layers.iter_mut());
    nodes
        .iter_mut()
        .chain(above.flat_map(|l| l.nodes.iter_mut()))
}

fn held(d: &mut GradeNode, frame: i64) {
    if let Some(i) = d.changed_key(frame) {
        let k = d.keys[i].clone();
        d.key = k.key;
        match &mut d.tool {
            GradeTool::Corrector(c) => {
                **c = k.values;
                d.key_input = k.key_input;
                d.qualifier = k.qualifier;
                d.windows = k.windows;
            }
            GradeTool::KeyMixer { mix } => *mix = k.mix,
            _ => {}
        }
    }
    d.keys.clear();
    d.animated = false;
}

fn shift(keys: ApplyKeys, from: &StillSource, to: &StillSource, fps: f64) -> Option<i64> {
    let starts = to.first - from.first;
    match keys {
        ApplyKeys::None => None,
        ApplyKeys::StartFrames => Some(starts),
        ApplyKeys::CurrentFrame => Some(to.at - from.at),
        ApplyKeys::SourceTimecode => {
            let (a, b) = (from.origin * fps, to.origin * fps);
            let span = |o: f64, s: &StillSource| (o + s.first as f64, o + s.last as f64);
            let ((a0, a1), (b0, b1)) = (span(a, from), span(b, to));
            Some(match a0 <= b1 && b0 <= a1 {
                true => (a - b).round() as i64,
                false => starts,
            })
        }
    }
}

fn placed(g: &mut Grade, from: &StillSource, to: &StillSource, by: Option<i64>) {
    for d in nodes_mut(g).filter(|d| !d.keys.is_empty()) {
        let Some(by) = by else {
            held(d, from.at);
            continue;
        };
        for k in &mut d.keys {
            k.frame += by;
        }
        if !(d.keys.iter()).any(|k| (to.first..=to.last).contains(&k.frame)) {
            held(d, to.first);
        }
    }
}

fn target(p: &Project, clip: &str, keys: ApplyKeys, at: Option<i64>) -> Result<StillSource> {
    if keys != ApplyKeys::CurrentFrame {
        return source_of(p, clip, None);
    }
    let at = at.ok_or_else(|| Error::usage("give `at`: the frame for the grabbed frame"))?;
    let (ti, ci) = locate(p, clip)?;
    let c = &p.tracks[ti].clips[ci];
    if !(c.start <= at && at < c.end()) {
        return Err(Error::invalid(format!(
            "clip {clip} is not at frame {at}: the keyframes align on a frame of the clip"
        )));
    }
    source_of(p, clip, Some(at))
}

pub fn apply(
    p: &mut Project,
    id: &str,
    (level, clips): (GradeLevel, &[String]),
    (keys, at): (ApplyKeys, Option<i64>),
) -> Result<()> {
    let still = found(p, id)?.clone();
    let size = p.frame_size();
    match level {
        GradeLevel::Clip => {
            if clips.is_empty() {
                return Err(Error::usage("give the clips that get the grade"));
            }
            let g = (still.grade).unwrap_or_else(|| Grade::usual(size));
            let fps = p.fps().as_f64();
            for clip in clips {
                let to = target(p, clip, keys, at)?;
                let mut placed_grade = g.clone();
                let by = shift(keys, &still.source, &to, fps);
                placed(&mut placed_grade, &still.source, &to, by);
                grade::apply(p, &placed_grade, std::slice::from_ref(clip))?;
            }
        }
        GradeLevel::Timeline => {
            if !clips.is_empty() {
                return Err(Error::usage(
                    "the timeline grade goes to the open timeline: leave out `clips`",
                ));
            }
            if still.timeline_grade.as_ref().is_some_and(keyed) {
                return Err(refused(id, "timeline grade", "Apply Grade"));
            }
            let g = (still.timeline_grade).unwrap_or_else(|| Grade::usual(size));
            grade::store(p, &GradeOf::Timeline, &g)?;
        }
    }
    Ok(())
}

pub fn append(p: &mut Project, id: &str, to: &[GradeOf]) -> Result<()> {
    let still = found(p, id)?.clone();
    let size = p.frame_size();
    if to.is_empty() {
        return Err(Error::usage("give the clips that get the nodes"));
    }
    for of in to {
        let graph = match of {
            GradeOf::Clip(_, layer) => {
                let g = still.grade.as_ref();
                g.map_or_else(|| Grade::usual(size), |g| g.layer(*layer, size))
            }
            GradeOf::Timeline => {
                let g = (still.timeline_grade.clone()).unwrap_or_else(|| Grade::usual(size));
                if keyed(&g) {
                    return Err(refused(id, "timeline grade", "Append Node Graph"));
                }
                g
            }
        };
        grade::append_graph(p, of, &graph)?;
    }
    Ok(())
}

pub fn get(p: &Project, dir: &Path) -> Value {
    let stills: Vec<Value> = (p.stills.iter())
        .map(|s| {
            json!({
                "id": s.id,
                "album": s.album,
                "clip": s.clip,
                "timeline": s.timeline,
                "frame": s.frame,
                "label": s.label,
                "picture": dir.join(&s.picture),
                "keyframes": s.grade.as_ref().is_some_and(keyed),
                "timeline_keyframes": s.timeline_grade.as_ref().is_some_and(keyed),
            })
        })
        .collect();
    json!(stills)
}
