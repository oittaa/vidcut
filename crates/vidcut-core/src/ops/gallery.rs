use std::path::Path;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Grade, Project, Still, TrackKind};
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

pub fn grab(p: &mut Project, (clip, frame): (&str, i64), picture: String) -> Result<String> {
    let timeline = open_timeline(p)?;
    let grade = grade::whole(p, clip)?;
    let timeline_grade = p.timeline_grade().cloned();
    let id = p.alloc_still_id();
    p.stills.push(Still {
        id: id.clone(),
        clip: clip.to_string(),
        timeline,
        frame,
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

fn refused(id: &str, which: &str) -> Error {
    Error::invalid(format!(
        "the {which} of {id} has keyframes: Apply Grade takes a grade with none"
    ))
}

pub fn apply(p: &mut Project, id: &str, level: GradeLevel, clips: &[String]) -> Result<()> {
    let still = found(p, id)?.clone();
    let size = p.frame_size();
    match level {
        GradeLevel::Clip => {
            if still.grade.as_ref().is_some_and(keyed) {
                return Err(refused(id, "clip grade"));
            }
            if clips.is_empty() {
                return Err(Error::usage("give the clips that get the grade"));
            }
            let count = p.settings.node_stack_layers.len();
            for clip in clips {
                grade::whole(p, clip)?;
                let mut g = (still.grade.clone()).unwrap_or_else(|| Grade::usual(size));
                g.counted(count, size);
                grade::put_whole(p, clip, g)?;
            }
        }
        GradeLevel::Timeline => {
            if !clips.is_empty() {
                return Err(Error::usage(
                    "the timeline grade goes to the open timeline: leave out `clips`",
                ));
            }
            if still.timeline_grade.as_ref().is_some_and(keyed) {
                return Err(refused(id, "timeline grade"));
            }
            let g = (still.timeline_grade).unwrap_or_else(|| Grade::usual(size));
            grade::store(p, &GradeOf::Timeline, &g)?;
        }
    }
    Ok(())
}

pub fn get(p: &Project, dir: &Path) -> Value {
    let stills: Vec<Value> = (p.stills.iter())
        .map(|s| {
            json!({
                "id": s.id,
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
