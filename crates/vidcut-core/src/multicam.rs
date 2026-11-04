use std::cmp::Ordering;
use std::ops::Range;

use serde::Serialize;
use serde_json::{Value, json};

use crate::editor::Env;
use crate::error::{Error, Result};
use crate::model::{AngleSync, GroupClip, GroupRow, Marks, Media, Project, SyncGroup};
use crate::ops::{self, has_bin};
use crate::time::{Rational, format_timecode, timecode_frames};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SoundPair {
    pub a: String,
    pub b: String,
    pub shift: f64,
    pub score: f32,
    pub matched: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Layout {
    pub start_timecode: String,
    pub start: i64,
    pub frames: i64,
    pub rows: Vec<GroupRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unmatched: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<SoundPair>,
}

pub const SOUND_MATCH_SCORE: f32 = 25.0;

const PICK_SECONDS: f64 = 5.0;

const LONGEST_GAP_SECONDS: f64 = 600.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncClip {
    pub media: String,
    pub frames: i64,
    pub head: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncRow {
    pub number: usize,
    pub camera: String,
    pub media: String,
    pub clips: Vec<SyncClip>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<i64>,
    pub active: bool,
}

type Placed<'a> = (&'a Media, i64, Range<i64>);

pub enum SyncOpen<'a> {
    Group(&'a SyncGroup),
    Files(Vec<&'a Media>),
}

fn origin(p: &Project, m: &Media) -> i64 {
    m.info
        .timecode
        .map_or(0, |tc| tc.frames_at(p.settings.fps).floor() as i64)
}

fn used(p: &Project, m: &Media, whole: bool) -> Range<i64> {
    let frames = p.media_frames(m);
    if whole || m.is_still() {
        return 0..frames;
    }
    m.marks.mark_in.unwrap_or(0)..m.marks.mark_out.unwrap_or(frames)
}

fn record_start(p: &Project, m: &Media, sync: AngleSync, part: &Range<i64>) -> i64 {
    let mark = match sync {
        AngleSync::Timecode | AngleSync::Sound => return origin(p, m) + part.start,
        AngleSync::In => m.marks.mark_in.unwrap_or(0),
        AngleSync::Out => m.marks.mark_out.unwrap_or(p.media_frames(m)),
    };
    part.start - mark
}

fn start_timecode(p: &Project, start: i64) -> String {
    let (fps, drop) = (p.settings.fps, p.settings.counts_drop_frame());
    format_timecode(start, fps, 0, drop)
}

pub fn visible(p: &Project, c: &GroupClip) -> i64 {
    p.media(&c.media).map_or(0, |m| p.media_frames(m)) - c.head - c.tail
}

pub fn row_file_at<'a>(p: &Project, row: &'a GroupRow, at: i64) -> Option<(&'a GroupClip, i64)> {
    row.clips
        .iter()
        .find(|c| c.at <= at && at < c.at + visible(p, c))
        .map(|c| (c, at - c.at + c.head))
}

pub fn row_camera(rows: &[GroupRow], i: usize) -> String {
    match &rows[i].camera {
        Some(name) => name.clone(),
        None => rows[..=i]
            .iter()
            .filter(|r| r.camera.is_none())
            .count()
            .to_string(),
    }
}

fn kept_clip(p: &Project, (m, _, part): &Placed<'_>, place: i64, start: i64) -> GroupClip {
    GroupClip {
        media: m.id.clone(),
        at: start,
        hidden: 0,
        head: part.start + start - place,
        tail: p.media_frames(m) - part.end,
    }
}

fn camera_row(
    p: &Project,
    placed: &[Placed<'_>],
    mut members: Vec<usize>,
    sync: AngleSync,
    dropped: &mut Vec<String>,
) -> GroupRow {
    let time = |i: usize| match sync {
        AngleSync::Sound => placed[i].1,
        _ => origin(p, placed[i].0) + placed[i].2.start,
    };
    members.sort_by_key(|i| (time(*i), *i));
    let marked = match sync {
        AngleSync::Out => members.last(),
        _ => members.first(),
    };
    let by_mark = marked.map_or(0, |i| placed[*i].1 - time(*i));
    let camera = members.first().and_then(|i| placed[*i].0.camera.clone());
    let mut reach = i64::MIN;
    let mut clips = Vec::with_capacity(members.len());
    for i in members {
        let place = time(i) + by_mark;
        let end = place + placed[i].2.end - placed[i].2.start;
        let start = place.max(reach);
        if start >= end {
            dropped.push(placed[i].0.id.clone());
            continue;
        }
        reach = end;
        clips.push(kept_clip(p, &placed[i], place, start));
    }
    GroupRow {
        camera,
        locked: false,
        clips,
    }
}

fn rows_with_no_name(
    p: &Project,
    placed: &[Placed<'_>],
    mut members: Vec<usize>,
    sync: AngleSync,
) -> Vec<GroupRow> {
    if sync == AngleSync::Timecode {
        members.sort_by_key(|i| (placed[*i].1, *i));
    }
    let alone = |clip: GroupClip| GroupRow {
        camera: None,
        locked: false,
        clips: vec![clip],
    };
    let (pictures, sounds): (Vec<usize>, Vec<usize>) = members
        .into_iter()
        .partition(|i| placed[*i].0.info.video.is_some());
    let mut rows: Vec<GroupRow> = Vec::with_capacity(pictures.len() + sounds.len());
    for i in pictures {
        let place = placed[i].1;
        let clip = kept_clip(p, &placed[i], place, place);
        let free = rows.iter_mut().find(|row| {
            sync == AngleSync::Timecode && row.clips.iter().all(|c| c.at + visible(p, c) <= place)
        });
        match free {
            Some(row) => row.clips.push(clip),
            None => rows.push(alone(clip)),
        }
    }
    for i in sounds {
        let place = placed[i].1;
        rows.push(alone(kept_clip(p, &placed[i], place, place)));
    }
    rows
}

fn ruled_rows(p: &Project, placed: &[Placed<'_>], sync: AngleSync) -> (Vec<GroupRow>, Vec<String>) {
    let mut named: Vec<(&str, Vec<usize>)> = Vec::new();
    let mut unnamed = Vec::new();
    for (i, (m, ..)) in placed.iter().enumerate() {
        match m.camera.as_deref() {
            None => unnamed.push(i),
            Some(camera) => match named.iter_mut().find(|(name, _)| *name == camera) {
                Some((_, members)) => members.push(i),
                None => named.push((camera, vec![i])),
            },
        }
    }
    named.sort_by(|a, b| camera_order(a.0, b.0));
    let mut dropped = Vec::new();
    let mut rows: Vec<GroupRow> = named
        .into_iter()
        .map(|(_, members)| camera_row(p, placed, members, sync, &mut dropped))
        .filter(|row| !row.clips.is_empty())
        .collect();
    rows.extend(rows_with_no_name(p, placed, unnamed, sync));
    (rows, dropped)
}

fn whole_files<'a>(p: &Project, files: &[&'a Media]) -> Vec<Placed<'a>> {
    files
        .iter()
        .map(|m| (*m, origin(p, m), 0..p.media_frames(m)))
        .collect()
}

pub fn layout(
    p: &Project,
    env: &dyn Env,
    ids: &[String],
    sync: AngleSync,
    full_extents: bool,
) -> Result<Layout> {
    if ids.is_empty() {
        return Err(Error::usage("name the media of the angles"));
    }
    let mut placed: Vec<Placed<'_>> = Vec::with_capacity(ids.len());
    for id in ids {
        let m = p
            .media(id)
            .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
        if placed.iter().any(|(seen, ..)| seen.id == m.id) {
            continue;
        }
        let part = used(p, m, full_extents);
        placed.push((m, record_start(p, m, sync, &part), part));
    }
    let (unmatched, pairs) = match sync {
        AngleSync::Sound => by_sound(p, env, &mut placed)?,
        _ => Default::default(),
    };
    let (mut rows, dropped) = ruled_rows(p, &placed, sync);
    let parts = || rows.iter().flat_map(|r| &r.clips);
    let first = match sync {
        AngleSync::Sound => 0,
        _ => parts().map(|c| c.at).min().unwrap_or(0),
    };
    let last = parts().map(|c| c.at + visible(p, c)).max().unwrap_or(0);
    for clip in rows.iter_mut().flat_map(|r| &mut r.clips) {
        clip.at -= first;
    }
    let start = group_start(p, sync, first);
    Ok(Layout {
        start_timecode: start_timecode(p, start),
        start,
        frames: last - first,
        rows,
        dropped,
        unmatched,
        pairs,
    })
}

fn one_hour(p: &Project) -> i64 {
    let (fps, drop) = (p.settings.fps, p.settings.counts_drop_frame());
    timecode_frames(1, 0, 0, 0, fps, drop).unwrap_or(0)
}

fn group_start(p: &Project, sync: AngleSync, first: i64) -> i64 {
    match sync {
        AngleSync::Timecode => first,
        AngleSync::In | AngleSync::Out | AngleSync::Sound => one_hour(p),
    }
}

pub fn ruler_start(p: &Project, ids: &[String]) -> i64 {
    let starts = || {
        ids.iter()
            .filter_map(move |id| p.media(id))
            .map(move |m| origin(p, m))
    };
    if starts().all(|start| start == 0) {
        return one_hour(p);
    }
    starts().min().unwrap_or_default()
}

fn close_far_gaps(fps: Rational, spans: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let longest = (LONGEST_GAP_SECONDS * fps.as_f64()).round() as i64;
    let short = fps.as_f64().round() as i64;
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|i| spans[*i].0);
    let first = order.first().map_or(0, |i| spans[*i].0);
    let (mut hidden, mut reach) = (0, first);
    let mut shown = vec![(0, 0); spans.len()];
    for i in order {
        let (real, frames) = spans[i];
        if real - reach > longest {
            hidden += real - reach - short;
        }
        shown[i] = (real - first - hidden, hidden);
        reach = reach.max(real + frames);
    }
    shown
}

pub fn settle(p: &Project, rows: &mut [GroupRow]) {
    let spans: Vec<(i64, i64)> = rows
        .iter()
        .flat_map(|r| &r.clips)
        .map(|c| (c.at + c.hidden, visible(p, c)))
        .collect();
    let shown = close_far_gaps(p.settings.fps, &spans);
    let clips = rows.iter_mut().flat_map(|r| &mut r.clips);
    for (clip, (at, hidden)) in clips.zip(shown) {
        clip.at = at;
        clip.hidden = hidden;
    }
    for row in rows {
        row.clips.sort_by_key(|c| c.at + c.hidden);
    }
}

pub fn shift_clip(p: &Project, rows: &mut [GroupRow], media: &str, real: i64) {
    if let Some(c) = rows
        .iter_mut()
        .flat_map(|r| &mut r.clips)
        .find(|c| c.media == media)
    {
        c.at = real.max(0);
        c.hidden = 0;
    }
    settle(p, rows);
}

pub fn drop_clip(p: &Project, rows: &mut [GroupRow], media: &str) {
    let found = rows
        .iter_mut()
        .find_map(|r| Some((r.clips.iter().find(|c| c.media == media)?.clone(), r)));
    let Some((moved, row)) = found else {
        return;
    };
    let a = moved.at + moved.hidden;
    let b = a + visible(p, &moved);
    row.clips.retain_mut(|c| {
        let x = c.at + c.hidden;
        let y = x + visible(p, c);
        if c.media == media || y <= a || b <= x {
            return true;
        }
        if a <= x && b >= y {
            return false;
        }
        if a <= x {
            c.head += b - x;
            c.at = b;
            c.hidden = 0;
        } else {
            c.tail += y - a;
        }
        true
    });
    settle(p, rows);
}

pub fn lock_lined_up(rows: &mut [GroupRow], unmatched: &[String]) {
    for row in rows {
        row.locked = row.clips.iter().any(|c| !unmatched.contains(&c.media));
    }
}

fn by_sound(
    p: &Project,
    env: &dyn Env,
    placed: &mut [Placed<'_>],
) -> Result<(Vec<String>, Vec<SoundPair>)> {
    let n = placed.len();
    let mut found = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            if let Some(s) = env.sound_match(placed[i].0, placed[j].0)? {
                found.push((i, j, s));
            }
        }
    }
    found.sort_by(|a, b| b.2.score.total_cmp(&a.2.score));
    let mut party: Vec<usize> = (0..n).collect();
    let mut secs = vec![0.0f64; n];
    for (i, j, s) in found.iter().filter(|x| x.2.score >= SOUND_MATCH_SCORE) {
        let (keep, join) = (party[*i], party[*j]);
        if keep == join {
            continue;
        }
        let by = secs[*i] + s.shift - secs[*j];
        for k in 0..n {
            if party[k] == join {
                secs[k] += by;
                party[k] = keep;
            }
        }
    }
    let fps = p.settings.fps.as_f64();
    let mut base = 0;
    let mut unmatched = Vec::new();
    for k in 0..n {
        if party[k] == k && party.iter().filter(|o| **o == k).count() == 1 {
            placed[k].1 = placed[k].2.start;
            unmatched.push(placed[k].0.id.clone());
        }
        if !party[..k].contains(&party[k]) && party.iter().filter(|o| **o == party[k]).count() > 1 {
            let who = party[k];
            let least = (0..n)
                .filter(|o| party[*o] == who)
                .map(|o| secs[o])
                .fold(f64::INFINITY, f64::min);
            let mut end = base;
            for o in (0..n).filter(|o| party[*o] == who) {
                let at = base + ((secs[o] - least) * fps + 1e-6).round() as i64;
                placed[o].1 = at + placed[o].2.start;
                end = end.max(at + placed[o].2.end);
            }
            base = end + 1;
        }
    }
    let pairs = found
        .iter()
        .map(|(i, j, s)| SoundPair {
            a: placed[*i].0.id.clone(),
            b: placed[*j].0.id.clone(),
            shift: s.shift,
            score: s.score,
            matched: s.score >= SOUND_MATCH_SCORE,
        })
        .collect();
    Ok((unmatched, pairs))
}

fn checked_clip(p: &Project, ids: &[String], c: &GroupClip) -> Result<()> {
    if !ids.contains(&c.media) {
        return Err(Error::invalid(format!(
            "{} is not one of the files",
            c.media
        )));
    }
    if c.at < 0 || c.hidden < 0 {
        return Err(Error::invalid(format!(
            "{} can't start before the group",
            c.media
        )));
    }
    if c.head < 0 || c.tail < 0 {
        return Err(Error::invalid(format!(
            "{} can't lose fewer than no frames",
            c.media
        )));
    }
    if visible(p, c) <= 0 {
        return Err(Error::invalid(format!("{} has no frames left", c.media)));
    }
    Ok(())
}

fn by_hand(p: &Project, ids: &[String], rows: &[GroupRow]) -> Result<Vec<GroupRow>> {
    let mut seen: Vec<&str> = Vec::new();
    for row in rows {
        if row.clips.is_empty() {
            return Err(Error::invalid("a row of a synced group needs a file"));
        }
        for c in &row.clips {
            checked_clip(p, ids, c)?;
            if seen.contains(&c.media.as_str()) {
                return Err(Error::invalid(format!("{} is named twice", c.media)));
            }
            seen.push(&c.media);
        }
        let mut spans: Vec<(i64, i64, &str)> = row
            .clips
            .iter()
            .map(|c| (c.at + c.hidden, visible(p, c), c.media.as_str()))
            .collect();
        spans.sort_unstable();
        if let Some(pair) = spans.windows(2).find(|s| s[0].0 + s[0].1 > s[1].0) {
            return Err(Error::invalid(format!(
                "{} and {} overlap on one row",
                pair[0].2, pair[1].2
            )));
        }
    }
    if seen.is_empty() {
        return Err(Error::invalid("a synced group needs a file"));
    }
    let mut placed = rows.to_vec();
    settle(p, &mut placed);
    Ok(placed)
}

pub struct NewGroup<'a> {
    pub sync: AngleSync,
    pub rows: Option<&'a [GroupRow]>,
    pub name: String,
    pub bin: &'a str,
}

pub struct Created {
    pub id: String,
    pub unmatched: Vec<String>,
}

pub fn create(
    p: &mut Project,
    env: &dyn Env,
    ids: &[String],
    new: NewGroup<'_>,
) -> Result<Created> {
    let NewGroup {
        sync,
        rows,
        name,
        bin,
    } = new;
    if !has_bin(p, bin) {
        return Err(Error::not_found(format!(
            "no bin {bin:?}; add it with bins.add"
        )));
    }
    let (start, rows, unmatched) = match rows {
        Some(rows) => {
            layout(p, env, ids, AngleSync::Timecode, true)?;
            (one_hour(p), by_hand(p, ids, rows)?, Vec::new())
        }
        None => {
            let lined_up = layout(p, env, ids, sync, true)?;
            let mut rows = lined_up.rows;
            lock_lined_up(&mut rows, &lined_up.unmatched);
            (lined_up.start, rows, lined_up.unmatched)
        }
    };
    let dropped = ids.iter().filter(|id| !holds(&rows, id)).cloned().collect();
    let id = p.new_group_id();
    p.groups.push(SyncGroup {
        id: id.clone(),
        name,
        bin: bin.to_string(),
        start,
        rows,
        dropped,
    });
    Ok(Created { id, unmatched })
}

fn holds(rows: &[GroupRow], media: &str) -> bool {
    rows.iter().flat_map(|r| &r.clips).any(|c| c.media == media)
}

fn store_rows(g: &mut SyncGroup, rows: Vec<GroupRow>) {
    let gone: Vec<String> = g
        .clips()
        .filter(|c| !holds(&rows, &c.media))
        .map(|c| c.media.clone())
        .collect();
    g.dropped.extend(gone);
    g.rows = rows;
}

pub fn set_rows(p: &mut Project, id: &str, rows: &[GroupRow]) -> Result<()> {
    let g = group_mut(p, id)?;
    let ids: Vec<String> = g.clips().map(|c| c.media.clone()).collect();
    let placed = by_hand(p, &ids, rows)?;
    store_rows(group_mut(p, id)?, placed);
    Ok(())
}

pub fn place(p: &mut Project, id: &str, media: &str, at: i64) -> Result<()> {
    let mut rows = group_mut(p, id)?.rows.clone();
    let row = rows
        .iter()
        .find(|r| r.clips.iter().any(|c| c.media == media))
        .ok_or_else(|| Error::not_found(format!("{media} is not in {id}")))?;
    if row.locked {
        return Err(Error::invalid(format!(
            "The row of {media} has the sync lock"
        )));
    }
    shift_clip(p, &mut rows, media, at);
    drop_clip(p, &mut rows, media);
    store_rows(group_mut(p, id)?, rows);
    Ok(())
}

pub fn group_mut<'a>(p: &'a mut Project, id: &str) -> Result<&'a mut SyncGroup> {
    p.groups
        .iter_mut()
        .find(|g| g.id == id)
        .ok_or_else(|| Error::not_found(format!("no synced group {id}")))
}

pub fn remove(p: &mut Project, id: &str) -> Result<()> {
    group_mut(p, id)?;
    p.groups.retain(|g| g.id != id);
    Ok(())
}

pub fn move_to_bin(p: &mut Project, id: &str, bin: &str) -> Result<()> {
    if !has_bin(p, bin) {
        return Err(Error::not_found(format!(
            "no bin {bin:?}; add it with bins.add"
        )));
    }
    group_mut(p, id)?.bin = bin.to_string();
    Ok(())
}

pub fn frames(p: &Project, g: &SyncGroup) -> i64 {
    g.clips().map(|c| c.at + visible(p, c)).max().unwrap_or(0)
}

pub fn camera_order(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

fn groups_of<'a>(p: &'a Project, files: &[&Media]) -> Vec<&'a SyncGroup> {
    let mut groups: Vec<&SyncGroup> = Vec::new();
    for g in files.iter().filter_map(|m| p.group_of(&m.id)) {
        if !groups.iter().any(|seen| seen.id == g.id) {
            groups.push(g);
        }
    }
    groups
}

pub fn sync_clips_files<'a>(
    p: &'a Project,
    bin: &str,
    selected: &[String],
) -> Result<SyncOpen<'a>> {
    let shown = || {
        p.media
            .iter()
            .filter(move |m| bin.is_empty() || ops::within(&m.bin, bin))
    };
    let picked: Vec<&Media> = shown().filter(|m| selected.contains(&m.id)).collect();
    let files: Vec<&Media> = match picked.len() {
        0 => shown().collect(),
        1 => return Err(Error::invalid("Select two files or more, or none")),
        _ => picked,
    };
    let files: Vec<&Media> = files.into_iter().filter(|m| !m.is_still()).collect();
    let two_or_more = || Error::invalid("A sync needs two files or more");
    if files.len() < 2 {
        return Err(two_or_more());
    }
    if let [g] = groups_of(p, &files)[..]
        && files
            .iter()
            .all(|m| p.group_of(&m.id).is_some() || g.dropped.contains(&m.id))
    {
        return Ok(SyncOpen::Group(g));
    }
    let (rows, _) = ruled_rows(p, &whole_files(p, &files), AngleSync::Timecode);
    if rows.iter().flat_map(|r| &r.clips).count() < 2 {
        return Err(two_or_more());
    }
    Ok(SyncOpen::Files(files))
}

fn sync_row(
    p: &Project,
    rows: &[GroupRow],
    i: usize,
    at: i64,
    shown: Option<&str>,
    group: Option<&SyncGroup>,
) -> SyncRow {
    let row = &rows[i];
    let mut clips = Vec::with_capacity(row.clips.len());
    let mut under = None;
    for c in &row.clips {
        let Some(m) = p.media(&c.media) else {
            continue;
        };
        let start = ops::file_start_at(p, m, at, group);
        let frames = visible(p, c);
        if under.is_none()
            && let Some(frame) = start
                .map(|s| at - s)
                .filter(|f| (c.head..c.head + frames).contains(f))
        {
            under = Some((c.media.clone(), frame));
        }
        clips.push(SyncClip {
            media: c.media.clone(),
            frames,
            head: c.head,
            start: start.map(|s| s + c.head),
        });
    }
    let (media, frame) = match under {
        Some((media, frame)) => (media, Some(frame)),
        None => (
            clips.first().map(|c| c.media.clone()).unwrap_or_default(),
            None,
        ),
    };
    SyncRow {
        number: i + 1,
        camera: row_camera(rows, i),
        media,
        active: clips.iter().any(|c| shown == Some(c.media.as_str())),
        clips,
        frame,
    }
}

fn group_at_playhead(p: &Project, at: i64) -> Option<&SyncGroup> {
    p.shown_clip(at).and_then(|c| p.group_of(&c.media))
}

fn group_of_bin<'a>(p: &'a Project, files: &[&Media], at: i64) -> Option<&'a SyncGroup> {
    let groups = groups_of(p, files);
    let listed = |g: &SyncGroup| groups.iter().any(|seen| seen.id == g.id);
    group_at_playhead(p, at)
        .filter(|g| listed(g))
        .or_else(|| p.groups.iter().rev().find(|g| listed(g)))
}

pub fn sync_rows(p: &Project, bin: &str, at: i64) -> Result<Vec<SyncRow>> {
    if !bin.is_empty() && !has_bin(p, bin) {
        return Err(Error::not_found(format!("no bin {bin:?}")));
    }
    let shown = p.shown_clip(at).map(|c| c.media.as_str());
    let files: Vec<&Media> = p
        .media
        .iter()
        .filter(|m| !m.is_still() && (bin.is_empty() || ops::within(&m.bin, bin)))
        .collect();
    let group = group_of_bin(p, &files, at);
    let rows: Vec<GroupRow> = match group {
        Some(g) => g
            .rows
            .iter()
            .map(|r| GroupRow {
                clips: r
                    .clips
                    .iter()
                    .filter(|c| files.iter().any(|m| m.id == c.media))
                    .cloned()
                    .collect(),
                ..r.clone()
            })
            .filter(|r| !r.clips.is_empty())
            .collect(),
        None => ruled_rows(p, &whole_files(p, &files), AngleSync::Timecode).0,
    };
    Ok((0..rows.len())
        .map(|i| sync_row(p, &rows, i, at, shown, group))
        .collect())
}

pub fn pick(p: &mut Project, media: &str, at: i64) -> Result<Marks> {
    let m = p
        .media(media)
        .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
    let frames = p.media_frames(m);
    let group = group_at_playhead(p, at)
        .filter(|g| g.at_of(media).is_some())
        .or_else(|| p.group_of(media));
    let start = ops::file_start_at(p, m, at, group)
        .filter(|s| (0..frames).contains(&(at - s)))
        .ok_or_else(|| Error::invalid(format!("{} has no frame at the playhead", m.name)))?;
    let end = p.end();
    let (a, b) = if at > end {
        (end - start, at - start)
    } else {
        let length = (PICK_SECONDS * p.settings.fps.as_f64()).round() as i64;
        (at - start, at - start + length)
    };
    let (a, b) = (a.max(0), b.min(frames));
    if a >= b {
        return Err(Error::invalid(format!(
            "{} has no frames before the playhead",
            m.name
        )));
    }
    ops::set_media_marks(p, media, Some(Some(a)), Some(Some(b)))?;
    Ok(Marks {
        mark_in: Some(a),
        mark_out: Some(b),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GridFile {
    pub media: String,
    pub at: i64,
    pub frames: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GridCamera {
    pub label: String,
    pub files: Vec<GridFile>,
}

impl GridCamera {
    pub fn file_at(&self, at: i64) -> Option<&GridFile> {
        self.files
            .iter()
            .find(|f| (f.at..f.at + f.frames).contains(&at))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridPart {
    pub media: String,
    pub range: (i64, i64),
    pub lead: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SourceGrid {
    pub start: i64,
    pub frames: i64,
    pub count: usize,
    pub cameras: Vec<GridCamera>,
}

impl SourceGrid {
    pub fn camera_of(&self, media: &str) -> Option<&GridCamera> {
        self.cameras
            .iter()
            .find(|c| c.files.iter().any(|f| f.media == media))
    }

    pub fn shown(&self, at: i64) -> impl Iterator<Item = (usize, &GridCamera, &GridFile)> {
        self.cameras
            .iter()
            .enumerate()
            .filter_map(move |(i, c)| Some((i, c, c.file_at(at)?)))
    }

    pub fn part(&self, media: &str, marks: &Marks) -> Option<GridPart> {
        let file = self
            .cameras
            .iter()
            .flat_map(|c| &c.files)
            .find(|f| f.media == media)?;
        if marks.is_empty() {
            return Some(GridPart {
                media: media.to_string(),
                range: (0, file.frames),
                lead: 0,
            });
        }
        let a = marks.mark_in.unwrap_or(0) - file.at;
        let b = marks.mark_out.map_or(file.frames, |o| o - file.at);
        let range = (a.max(0), b.min(file.frames));
        (range.0 < range.1).then(|| GridPart {
            media: media.to_string(),
            range,
            lead: (-a).max(0),
        })
    }

    pub fn file_marks(&self, media: &str, marks: &Marks) -> Marks {
        let Some(file) = self
            .cameras
            .iter()
            .flat_map(|c| &c.files)
            .find(|f| f.media == media)
        else {
            return Marks::default();
        };
        Marks {
            mark_in: marks
                .mark_in
                .map(|i| (i - file.at).max(0))
                .filter(|i| *i < file.frames),
            mark_out: marks
                .mark_out
                .map(|o| o - file.at)
                .filter(|o| *o > 0)
                .map(|o| o.min(file.frames)),
        }
    }
}

pub fn source_grid(p: &Project, media: &[String]) -> SourceGrid {
    let files: Vec<(&Media, i64, i64)> = media
        .iter()
        .filter_map(|id| p.media(id))
        .filter(|m| m.info.video.is_some())
        .map(|m| (m, origin(p, m), p.media_frames(m)))
        .filter(|(.., frames)| *frames > 0)
        .collect();
    let mut spans: Vec<(i64, i64)> = files.iter().map(|(_, s, n)| (*s, s + n)).collect();
    spans.sort_unstable();
    let mut closed: Vec<(i64, i64, i64)> = Vec::new();
    let mut frames = 0;
    for (a, b) in spans {
        match closed.last_mut() {
            Some((_, end, _)) if a <= *end => {
                let grown = b.max(*end);
                frames += grown - *end;
                *end = grown;
            }
            _ => {
                closed.push((a, b, frames));
                frames += b - a;
            }
        }
    }
    let place = |start: i64| {
        closed
            .iter()
            .find(|(a, b, _)| (*a..*b).contains(&start))
            .map_or(0, |(a, _, at)| at + start - a)
    };
    let mut named: Vec<(&str, Vec<GridFile>)> = Vec::new();
    let mut unnamed: Vec<GridFile> = Vec::new();
    for (m, start, n) in &files {
        let file = GridFile {
            media: m.id.clone(),
            at: place(*start),
            frames: *n,
        };
        match m.camera.as_deref() {
            Some(name) => match named.iter_mut().find(|(c, _)| *c == name) {
                Some((_, list)) => list.push(file),
                None => named.push((name, vec![file])),
            },
            None => unnamed.push(file),
        }
    }
    named.sort_by(|a, b| camera_order(a.0, b.0));
    let cameras = named
        .into_iter()
        .map(|(name, files)| GridCamera {
            label: format!("Camera {name}"),
            files,
        })
        .chain(unnamed.into_iter().enumerate().map(|(i, file)| GridCamera {
            label: format!("Camera {}", i + 1),
            files: vec![file],
        }))
        .collect();
    SourceGrid {
        start: closed.first().map_or(0, |(a, ..)| *a),
        frames,
        count: files.len(),
        cameras,
    }
}

pub fn group_json(p: &Project, g: &SyncGroup) -> Value {
    let mut v = json!({
        "id": g.id,
        "name": g.name,
        "start_timecode": start_timecode(p, g.start),
        "frames": frames(p, g),
        "rows": g.rows,
    });
    if !g.bin.is_empty() {
        v["bin"] = json!(g.bin);
    }
    if !g.dropped.is_empty() {
        v["dropped"] = json!(g.dropped);
    }
    v
}
