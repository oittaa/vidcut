use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};
use crate::masks::{self, MOST_CLICKS, Point};
use crate::model::{GradeTool, MagicMask, MaskClick, Project};

use super::grade::{GradeOf, edited, node, stored, unlocked};
use super::locate;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClickChange {
    pub on: Option<bool>,
    pub place: Option<[f64; 2]>,
    pub negative: Option<bool>,
}

fn clip_only(of: &GradeOf) -> Result<()> {
    match of {
        GradeOf::Clip(..) => Ok(()),
        GradeOf::Timeline => Err(Error::invalid(
            "the Magic Mask works on the grade of a clip",
        )),
    }
}

pub fn mask_of(p: &Project, of: &GradeOf, n: u32) -> Result<MagicMask> {
    clip_only(of)?;
    let mut g = stored(p, of)?;
    let d = unlocked(&mut g, n)?;
    match &d.tool {
        GradeTool::Corrector(_) => Ok(d.magic_mask.clone().unwrap_or_default()),
        _ => Err(Error::invalid(format!(
            "node {n} is a mixer, which has no Magic Mask"
        ))),
    }
}

pub fn put(p: &mut Project, of: &GradeOf, n: u32, mask: MagicMask) -> Result<()> {
    edited(p, of, |g| {
        let d = node(g, n)?;
        d.magic_mask = (!mask.clicks.is_empty()).then_some(mask);
        Ok(())
    })
}

pub fn time_at(p: &Project, of: &GradeOf, frame: i64) -> Result<i64> {
    let GradeOf::Clip(id, _) = of else {
        return Ok(frame);
    };
    let (ti, ci) = locate(p, id)?;
    let c = &p.tracks[ti].clips[ci];
    let t = c.start + frame - c.content_at(c.start);
    match t >= c.start && t < c.end() {
        true => Ok(t),
        false => Err(Error::invalid(format!(
            "{id} has no frame {frame} in the timeline: its Magic Mask click is outside the clip"
        ))),
    }
}

pub fn placed(at: [f64; 2]) -> Result<[f64; 2]> {
    match at.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
        true => Ok(at),
        false => Err(Error::invalid(
            "a Magic Mask click is from 0 to 1 across and down the frame",
        )),
    }
}

fn is_name(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn checked(mask: &MagicMask) -> Result<()> {
    if mask.clicks.is_empty() {
        return Err(Error::invalid("a Magic Mask has one click at least"));
    }
    let mut counts: BTreeMap<(i64, u32), usize> = BTreeMap::new();
    for c in &mask.clicks {
        placed(c.at)?;
        if c.subject == 0 {
            return Err(Error::invalid("the subjects of a Magic Mask start at 1"));
        }
        let n = counts.entry((c.frame, c.subject)).or_default();
        *n += 1;
        if *n > MOST_CLICKS {
            return Err(Error::invalid(format!(
                "a subject takes {MOST_CLICKS} clicks on a frame at most"
            )));
        }
    }
    let mut seen = BTreeSet::new();
    for m in &mask.maps {
        if !is_name(&m.name) {
            return Err(Error::invalid(format!(
                "{:?} is not the name of a Magic Mask map",
                m.name
            )));
        }
        if m.from == Some(m.frame) {
            return Err(Error::invalid(format!(
                "a Magic Mask map on frame {} is not tracked from its own frame",
                m.frame
            )));
        }
        let source = m.from.unwrap_or(m.frame);
        if !counts.contains_key(&(source, m.subject)) || !seen.insert((m.frame, m.subject)) {
            return Err(Error::invalid(format!(
                "the Magic Mask has one map of subject {} on frame {} at most, from a click of it on frame {source}",
                m.subject, m.frame
            )));
        }
    }
    Ok(())
}

pub fn clicked_subjects(mask: &MagicMask, frame: i64) -> Vec<u32> {
    let mut found: Vec<u32> = (mask.clicks.iter())
        .filter(|c| c.on && c.frame == frame)
        .map(|c| c.subject)
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

pub fn points(mask: &MagicMask, frame: i64, subject: u32) -> Vec<Point> {
    (mask.clicks.iter())
        .filter(|c| c.on && c.frame == frame && c.subject == subject)
        .map(|c| Point {
            at: c.at,
            negative: c.negative,
        })
        .collect()
}

pub fn subject_at(mask: &MagicMask, frame: i64, at: [f64; 2]) -> Option<u32> {
    let mut found = None;
    for m in mask.maps.iter().filter(|m| m.frame == frame) {
        if masks::get(&m.name).is_some_and(|map| masks::value(&map, at) > 0.0) {
            found = Some(m.subject);
        }
    }
    found
}

fn last_subject(mask: &MagicMask, frame: i64) -> Option<u32> {
    (mask.clicks.iter().rev())
        .find(|c| c.frame == frame)
        .map(|c| c.subject)
}

fn next_subject(mask: &MagicMask) -> u32 {
    mask.clicks.iter().map(|c| c.subject).max().unwrap_or(0) + 1
}

pub fn clicked(
    mask: &mut MagicMask,
    frame: i64,
    (at, negative): ([f64; 2], bool),
    subject: Option<u32>,
) -> Result<(u32, usize)> {
    let at = placed(at)?;
    let fresh = next_subject(mask);
    let subject = match subject {
        Some(s) if s == 0 || s > fresh => {
            return Err(Error::invalid(format!(
                "the Magic Mask has subjects 1 to {}: a new one is {fresh}",
                fresh - 1
            )));
        }
        Some(s) => s,
        None => match (subject_at(mask, frame, at), negative) {
            (Some(s), true) => s,
            (Some(s), false) if masks::RULES.joins => s,
            (_, false) => fresh,
            (None, true) => last_subject(mask, frame).ok_or_else(|| {
                Error::invalid("a negative click needs a subject on its frame: click on one first")
            })?,
        },
    };
    let on_frame = (mask.clicks.iter())
        .filter(|c| c.frame == frame && c.subject == subject)
        .count();
    if on_frame >= MOST_CLICKS {
        return Err(Error::invalid(format!(
            "a subject takes {MOST_CLICKS} clicks on a frame at most"
        )));
    }
    mask.clicks.push(MaskClick {
        frame,
        at,
        negative,
        subject,
        on: true,
    });
    Ok((subject, mask.clicks.len()))
}

fn click_index(mask: &MagicMask, click: usize) -> Result<usize> {
    let count = mask.clicks.len();
    match click {
        1.. if click <= count => Ok(click - 1),
        _ => Err(Error::not_found(format!(
            "the Magic Mask has {count} clicks: there is no click {click}"
        ))),
    }
}

pub fn click_changed(
    mask: &mut MagicMask,
    click: usize,
    change: &ClickChange,
) -> Result<(i64, u32)> {
    let i = click_index(mask, click)?;
    let place = change.place.map(placed).transpose()?;
    let c = &mut mask.clicks[i];
    if let Some(v) = change.on {
        c.on = v;
    }
    if let Some(v) = place {
        c.at = v;
    }
    if let Some(v) = change.negative {
        c.negative = v;
    }
    Ok((c.frame, c.subject))
}

pub fn click_removed(mask: &mut MagicMask, click: usize) -> Result<(i64, u32)> {
    let i = click_index(mask, click)?;
    let c = mask.clicks.remove(i);
    Ok((c.frame, c.subject))
}
