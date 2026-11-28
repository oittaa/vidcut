use std::path::Path;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Album, Grade, Memory, Project, Still, TrackKind};
use crate::request::GradeLevel;

use super::grade::{self, GradeOf};

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

pub enum Holder {
    Album(String),
    Memory(char),
}

pub fn memory_slot(slot: char) -> Result<char> {
    match slot {
        'A'..='Z' => Ok(slot),
        _ => Err(Error::usage(format!(
            "memory {slot} is not a letter from A to Z"
        ))),
    }
}

fn place(p: &Project, clip: &str) -> Result<(usize, usize)> {
    let (ti, ci) = super::locate(p, clip)?;
    let t = &p.tracks[ti];
    let c = &t.clips[ci];
    let before = (t.clips.iter())
        .filter(|o| !o.is_generator() && o.start < c.start)
        .count();
    Ok((t.number(), before + 1))
}

pub fn grab(
    p: &mut Project,
    (clip, frame): (&str, i64),
    (to, picture): (Holder, String),
) -> Result<Value> {
    let timeline = open_timeline(p)?;
    let grade = grade::whole(p, clip)?;
    let timeline_grade = p.timeline_grade().cloned();
    let layers = p.settings.node_stack_layers.len();
    match to {
        Holder::Album(album) => {
            let album = album_found(p, &album)?.id.clone();
            let (track, shot) = place(p, clip)?;
            let id = p.alloc_still_id();
            p.stills.push(Still {
                id: id.clone(),
                album,
                clip: clip.to_string(),
                timeline,
                frame,
                track,
                shot,
                layers,
                grade,
                timeline_grade,
                label: String::new(),
                picture,
            });
            Ok(json!({ "still": id }))
        }
        Holder::Memory(slot) => {
            let slot = memory_slot(slot)?;
            remember(
                p,
                Memory {
                    slot,
                    clip: clip.to_string(),
                    timeline,
                    frame,
                    layers,
                    grade,
                    timeline_grade,
                    picture,
                },
            );
            Ok(json!({ "memory": slot }))
        }
    }
}

fn remember(p: &mut Project, m: Memory) {
    p.memories.retain(|o| o.slot != m.slot);
    p.memories.push(m);
    p.memories.sort_by_key(|o| o.slot);
}

pub fn number(p: &Project, s: &Still) -> String {
    let at = |id: &str| p.stills.iter().position(|o| o.id == id);
    let key = |o: &Still| (o.frame, at(&o.id));
    let same = |o: &&Still| {
        (o.album == s.album && o.timeline == s.timeline)
            && (o.track, o.shot) == (s.track, s.shot)
            && key(o) < key(s)
    };
    let n = p.stills.iter().filter(same).count() + 1;
    format!("{}.{}.{}", s.track, s.shot, n)
}

pub fn numbered<'a>(p: &'a Project, album: &str) -> Vec<(&'a Still, String)> {
    (p.stills.iter())
        .filter(|s| s.album == album)
        .map(|s| (s, number(p, s)))
        .collect()
}

pub fn label(p: &mut Project, id: &str, text: &str) -> Result<()> {
    found(p, id)?;
    if let Some(s) = p.stills.iter_mut().find(|s| s.id == id) {
        s.label = text.to_string();
    }
    Ok(())
}

fn memory_found(p: &Project, slot: char) -> Result<&Memory> {
    let slot = memory_slot(slot)?;
    p.memory(slot)
        .ok_or_else(|| Error::not_found(format!("memory {slot} is empty")))
}

pub fn copy_to_memory(p: &mut Project, id: &str, slot: char) -> Result<()> {
    let slot = memory_slot(slot)?;
    let s = found(p, id)?.clone();
    remember(
        p,
        Memory {
            slot,
            clip: s.clip,
            timeline: s.timeline,
            frame: s.frame,
            layers: s.layers,
            grade: s.grade,
            timeline_grade: s.timeline_grade,
            picture: s.picture,
        },
    );
    Ok(())
}

pub fn clear_memory(p: &mut Project, slot: char) -> Result<()> {
    let slot = memory_found(p, slot)?.slot;
    p.memories.retain(|m| m.slot != slot);
    Ok(())
}

pub fn load_memory(
    p: &mut Project,
    slot: char,
    (level, layer): (GradeLevel, usize),
    clips: &[String],
) -> Result<()> {
    let m = memory_found(p, slot)?.clone();
    let name = format!("memory {}", m.slot);
    let size = p.frame_size();
    match level {
        GradeLevel::Clip => {
            if clips.is_empty() {
                return Err(Error::usage("give the clips that get the grade"));
            }
            let from = if m.layers > 1 { layer } else { 1 };
            let g = (m.grade.as_ref()).map_or_else(|| Grade::usual(size), |g| g.layer(from, size));
            if keyed(&g) {
                return Err(refused(&name, "clip grade", "Load Memory"));
            }
            for id in clips {
                grade::store(p, &GradeOf::Clip(id.clone(), layer), &g)?;
            }
            Ok(())
        }
        GradeLevel::Timeline => given(
            p,
            (&name, "Load Memory"),
            (None, m.timeline_grade),
            level,
            clips,
        ),
    }
}

pub fn apply_memory(
    p: &mut Project,
    slot: char,
    level: GradeLevel,
    clips: &[String],
) -> Result<()> {
    let m = memory_found(p, slot)?.clone();
    let name = format!("memory {}", m.slot);
    given(
        p,
        (&name, "Apply Grade"),
        (m.grade, m.timeline_grade),
        level,
        clips,
    )
}

pub fn memories(p: &Project, dir: &Path) -> Value {
    let memories: Vec<Value> = (p.memories.iter())
        .map(|m| {
            json!({
                "slot": m.slot,
                "clip": m.clip,
                "timeline": m.timeline,
                "frame": m.frame,
                "picture": dir.join(&m.picture),
                "keyframes": m.grade.as_ref().is_some_and(keyed),
                "timeline_keyframes": m.timeline_grade.as_ref().is_some_and(keyed),
            })
        })
        .collect();
    json!(memories)
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

pub fn apply(p: &mut Project, id: &str, level: GradeLevel, clips: &[String]) -> Result<()> {
    let still = found(p, id)?.clone();
    given(
        p,
        (id, "Apply Grade"),
        (still.grade, still.timeline_grade),
        level,
        clips,
    )
}

fn given(
    p: &mut Project,
    (name, what): (&str, &str),
    (clip_grade, timeline_grade): (Option<Grade>, Option<Grade>),
    level: GradeLevel,
    clips: &[String],
) -> Result<()> {
    let size = p.frame_size();
    match level {
        GradeLevel::Clip => {
            if clip_grade.as_ref().is_some_and(keyed) {
                return Err(refused(name, "clip grade", what));
            }
            if clips.is_empty() {
                return Err(Error::usage("give the clips that get the grade"));
            }
            let g = clip_grade.unwrap_or_else(|| Grade::usual(size));
            grade::apply(p, &g, clips)?;
        }
        GradeLevel::Timeline => {
            if !clips.is_empty() {
                return Err(Error::usage(
                    "the timeline grade goes to the open timeline: leave out `clips`",
                ));
            }
            if timeline_grade.as_ref().is_some_and(keyed) {
                return Err(refused(name, "timeline grade", what));
            }
            let g = timeline_grade.unwrap_or_else(|| Grade::usual(size));
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
                "number": number(p, s),
                "label": s.label,
                "picture": dir.join(&s.picture),
                "keyframes": s.grade.as_ref().is_some_and(keyed),
                "timeline_keyframes": s.timeline_grade.as_ref().is_some_and(keyed),
            })
        })
        .collect();
    json!(stills)
}
