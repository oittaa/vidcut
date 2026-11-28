use std::collections::HashSet;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, ColorGroup, Grade, Project, Track};
use crate::request::GroupGrade;

use super::grade::{self, GradeOf};
use super::locate;

pub fn of_clip<'a>(p: &'a Project, clip: &str) -> Result<&'a ColorGroup> {
    let (ti, ci) = locate(p, clip)?;
    (p.tracks[ti].clips[ci].color_group.as_deref())
        .and_then(|g| p.color_group(g))
        .ok_or_else(|| Error::invalid(format!("{clip} is in no color group")))
}

pub fn grade_of(p: &Project, clip: &str, side: GroupGrade) -> Result<GradeOf> {
    of_clip(p, clip)?;
    Ok(GradeOf::Group(clip.to_string(), side))
}

pub fn grades_of(p: &Project, clips: &[String], side: GroupGrade) -> Result<Vec<GradeOf>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in clips {
        if seen.insert(of_clip(p, id)?.id.clone()) {
            out.push(GradeOf::Group(id.clone(), side));
        }
    }
    Ok(out)
}

pub fn around(
    pre: Option<&Grade>,
    clip: Option<&Grade>,
    post: Option<&Grade>,
    frame: [f64; 2],
) -> Option<Grade> {
    if pre.is_none() && post.is_none() {
        return clip.cloned();
    }
    let usual = Grade::usual(frame);
    let clip = clip.unwrap_or(&usual);
    let mut layers: Vec<Grade> = std::iter::once(clip.layer(1, frame))
        .chain(clip.layers.iter().cloned())
        .collect();
    let base = match pre {
        Some(pre) => pre.layer(1, frame),
        None => layers.remove(0),
    };
    layers.extend(post.map(|g| g.layer(1, frame)));
    Some(Grade {
        layers,
        kept_layers: Vec::new(),
        ..base
    })
}

pub fn acting<'a>(p: &'a Project, c: &Clip) -> Option<&'a ColorGroup> {
    p.color_group(c.color_group.as_deref()?)
        .filter(|g| g.acts())
}

pub fn graded(p: &Project, c: &Clip) -> Option<(Option<Grade>, Option<Grade>)> {
    let g = acting(p, c)?;
    let own = c.picture.as_ref().and_then(|pic| pic.grade.as_ref());
    let after = around(None, own, g.post.as_ref(), p.frame_size());
    Some((g.pre.clone(), after))
}

fn named(name: &str) -> Result<String> {
    match name.trim() {
        "" => Err(Error::usage("give the color group a name")),
        name => Ok(name.to_string()),
    }
}

fn found<'a>(p: &'a Project, id: &str) -> Result<&'a ColorGroup> {
    p.color_group(id)
        .ok_or_else(|| Error::not_found(format!("no color group {id}")))
}

pub fn proposed_name(p: &Project) -> String {
    let free = |name: &str| p.color_groups.iter().all(|g| g.name != name);
    let newest = p.color_groups.last().map(|g| g.name.as_str()).unwrap_or("");
    let lead = newest.trim_end_matches(|c: char| c.is_ascii_digit());
    let next = (newest[lead.len()..].parse::<u64>().ok()).and_then(|n| n.checked_add(1));
    let (lead, from) = match next {
        Some(n) => (lead.to_string(), n),
        None => ("Group ".to_string(), 1),
    };
    (from..)
        .map(|n| format!("{lead}{n}"))
        .find(|name| free(name))
        .expect("a free name")
}

fn joined(p: &mut Project, clips: &[String], group: &str) -> Result<()> {
    if clips.is_empty() {
        return Err(Error::usage("give the clips"));
    }
    for id in clips {
        grade::whole(p, id)?;
    }
    for t in p.tracks.iter_mut() {
        for c in t.clips.iter_mut().filter(|c| clips.contains(&c.id)) {
            c.color_group = Some(group.to_string());
        }
    }
    Ok(())
}

pub fn add(p: &mut Project, clips: &[String], name: Option<&str>) -> Result<String> {
    if clips.is_empty() {
        return Err(Error::usage("give the clips"));
    }
    let name = match name {
        Some(n) => named(n)?,
        None => proposed_name(p),
    };
    for id in clips {
        grade::whole(p, id)?;
    }
    let id = p.alloc_color_group_id();
    p.color_groups.push(ColorGroup {
        id: id.clone(),
        name,
        pre: None,
        post: None,
    });
    joined(p, clips, &id)?;
    Ok(id)
}

pub fn assign(p: &mut Project, group: &str, clips: &[String]) -> Result<()> {
    found(p, group)?;
    joined(p, clips, group)
}

pub fn unassign(p: &mut Project, clips: &[String]) -> Result<()> {
    if clips.is_empty() {
        return Err(Error::usage("give the clips"));
    }
    for id in clips {
        of_clip(p, id)?;
    }
    for t in p.tracks.iter_mut() {
        for c in t.clips.iter_mut().filter(|c| clips.contains(&c.id)) {
            c.color_group = None;
        }
    }
    Ok(())
}

pub fn rename(p: &mut Project, group: &str, name: &str) -> Result<()> {
    let name = named(name)?;
    found(p, group)?;
    if let Some(g) = p.color_group_mut(group) {
        g.name = name;
    }
    Ok(())
}

fn left(tracks: &mut [Track], group: &str) {
    let clips = tracks.iter_mut().flat_map(|t| t.clips.iter_mut());
    for c in clips.filter(|c| c.color_group.as_deref() == Some(group)) {
        c.color_group = None;
    }
}

pub fn remove(p: &mut Project, group: &str) -> Result<()> {
    found(p, group)?;
    p.color_groups.retain(|g| g.id != group);
    left(&mut p.tracks, group);
    for t in p.timelines.iter_mut() {
        left(&mut t.tracks, group);
    }
    for k in p.compounds.iter_mut() {
        left(&mut k.tracks, group);
    }
    Ok(())
}

pub fn collapse(p: &mut Project, clips: &[String]) -> Result<()> {
    if clips.is_empty() {
        return Err(Error::usage("give the clips"));
    }
    let mut seen = HashSet::new();
    let clips: Vec<&String> = clips.iter().filter(|id| seen.insert(id.as_str())).collect();
    let frame = p.frame_size();
    let usual = Grade::usual(frame);
    let mut flats = Vec::new();
    for id in &clips {
        let g = of_clip(p, id)?;
        let own = grade::stored(p, &GradeOf::Clip((*id).clone(), 1))?;
        let mut flat = g.pre.clone().unwrap_or_else(|| usual.clone());
        grade::appended(&mut flat, &own);
        grade::appended(&mut flat, g.post.as_ref().unwrap_or(&usual));
        grade::checked(&flat)?;
        flats.push(flat);
    }
    for (id, flat) in clips.iter().zip(&flats) {
        grade::store(p, &GradeOf::Clip((*id).clone(), 1), flat)?;
    }
    for t in p.tracks.iter_mut() {
        for c in t.clips.iter_mut().filter(|c| seen.contains(c.id.as_str())) {
            c.color_group = None;
        }
    }
    Ok(())
}

pub fn get(p: &Project) -> Value {
    let mut clips: Vec<&Clip> = (p.tracks.iter())
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.color_group.is_some())
        .collect();
    clips.sort_by_key(|c| c.start);
    let groups: Vec<Value> = (p.color_groups.iter())
        .map(|g| {
            let members: Vec<&str> = (clips.iter())
                .filter(|c| c.color_group.as_deref() == Some(g.id.as_str()))
                .map(|c| c.id.as_str())
                .collect();
            json!({
                "id": g.id,
                "name": g.name,
                "clips": members,
                "pre_clip": g.pre.is_some(),
                "post_clip": g.post.is_some(),
            })
        })
        .collect();
    json!(groups)
}
