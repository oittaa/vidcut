//! Applies requests to a project with snapshot undo, revisions and dry runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, Media, MediaInfo, Project};
use crate::ops::{self, ClipProps, Edge, Source};
use crate::request::{Changes, ClipRef, Envelope, ProjectSet, Request, Response, SourceParams};
use crate::time::{Rational, TimeContext, TimeSpec, format_timecode};
use crate::validate;

/// A probed media file, as the media layer reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbedMedia {
    /// Canonical absolute path.
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i64,
    pub info: MediaInfo,
}

/// What the editor needs from the outside world.
pub trait Env {
    /// Probes a media file.
    fn probe(&self, path: &Path) -> Result<ProbedMedia>;
}

/// One undo step: the project as it was before a change.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub label: String,
    pub project: Project,
}

/// Undo and redo stacks (most recent last).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct History {
    pub undo: Vec<Entry>,
    pub redo: Vec<Entry>,
}

/// Maximum undo steps kept.
pub const HISTORY_CAP: usize = 100;

pub struct Editor {
    project: Project,
    history: History,
}

impl Editor {
    pub fn new(project: Project, history: History) -> Self {
        Self { project, history }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn into_parts(self) -> (Project, History) {
        (self.project, self.history)
    }

    /// Applies one request. `playhead` is the live playhead, if any.
    pub fn apply(
        &mut self,
        envelope: Envelope,
        env: &dyn Env,
        playhead: Option<i64>,
    ) -> Result<Response> {
        let Envelope {
            request,
            if_rev,
            dry_run,
        } = envelope;
        if !request.is_mutation() {
            let result = query(&self.project, &self.history, &request, env, playhead)?;
            return Ok(Response {
                rev: self.project.rev,
                changes: Changes::default(),
                result,
                dry_run: false,
                via: String::new(),
            });
        }
        if let Some(rev) = if_rev
            && rev != self.project.rev
        {
            return Err(Error::conflict(format!(
                "project is at revision {}, not {rev}; re-read it and retry",
                self.project.rev
            )));
        }
        match request {
            Request::HistoryUndo { steps } => return self.step_history(steps, true, dry_run),
            Request::HistoryRedo { steps } => return self.step_history(steps, false, dry_run),
            _ => {}
        }
        let label = label_of(&request);
        let mut working = self.project.clone();
        let result = mutate(&mut working, &request, env, playhead)?;
        let problems = validate::violations(&working);
        if !problems.is_empty() {
            return Err(Error::internal(format!(
                "{} would break project invariants (a vidcut bug): {}",
                request.method(),
                problems.join("; ")
            )));
        }
        let changes = diff(&self.project, &working);
        if dry_run {
            return Ok(Response {
                rev: self.project.rev,
                changes,
                result,
                dry_run: true,
                via: String::new(),
            });
        }
        working.rev = self.project.rev + 1;
        let before = std::mem::replace(&mut self.project, working);
        self.history.undo.push(Entry {
            label,
            project: before,
        });
        if self.history.undo.len() > HISTORY_CAP {
            self.history.undo.remove(0);
        }
        self.history.redo.clear();
        Ok(Response {
            rev: self.project.rev,
            changes,
            result,
            dry_run: false,
            via: String::new(),
        })
    }

    /// Records an external change (e.g. the file was edited by another
    /// process) as one undoable step.
    pub fn record_external(&mut self, project: Project) {
        let mut project = project;
        project.rev = project.rev.max(self.project.rev + 1);
        let before = std::mem::replace(&mut self.project, project);
        self.history.undo.push(Entry {
            label: "external change".into(),
            project: before,
        });
        self.history.redo.clear();
    }

    fn step_history(&mut self, steps: u32, undo: bool, dry_run: bool) -> Result<Response> {
        let available = if undo {
            self.history.undo.len()
        } else {
            self.history.redo.len()
        };
        if available < steps as usize || steps == 0 {
            return Err(Error::invalid(format!(
                "cannot {} {steps} step(s): {available} available",
                if undo { "undo" } else { "redo" }
            )));
        }
        let mut project = self.project.clone();
        let mut history = self.history.clone();
        let mut labels = Vec::new();
        for _ in 0..steps {
            let (from, to) = if undo {
                (&mut history.undo, &mut history.redo)
            } else {
                (&mut history.redo, &mut history.undo)
            };
            let entry = from.pop().expect("checked");
            labels.push(entry.label.clone());
            let current = std::mem::replace(&mut project, entry.project);
            to.push(Entry {
                label: entry.label,
                project: current,
            });
        }
        let changes = diff(&self.project, &project);
        let result = json!({ if undo { "undone" } else { "redone" }: labels });
        if dry_run {
            return Ok(Response {
                rev: self.project.rev,
                changes,
                result,
                dry_run: true,
                via: String::new(),
            });
        }
        // Revisions only grow, even when undoing back to an older state.
        project.rev = self.project.rev + 1;
        self.project = project;
        self.history = history;
        Ok(Response {
            rev: self.project.rev,
            changes,
            result,
            dry_run: false,
            via: String::new(),
        })
    }
}

/// Undo label for a request.
fn label_of(request: &Request) -> String {
    match request {
        Request::Apply { label: Some(l), .. } => l.clone(),
        Request::Apply { commands, .. } => format!("batch of {} commands", commands.len()),
        other => other.method(),
    }
}

/// Created, modified and deleted ids between two project states.
fn diff(before: &Project, after: &Project) -> Changes {
    fn index(p: &Project) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        for media in &p.media {
            m.insert(
                media.id.clone(),
                serde_json::to_value(media).unwrap_or_default(),
            );
        }
        for t in &p.tracks {
            for c in &t.clips {
                m.insert(c.id.clone(), json!([t.id, c]));
            }
        }
        for k in &p.markers {
            m.insert(k.id.clone(), serde_json::to_value(k).unwrap_or_default());
        }
        m
    }
    let (a, b) = (index(before), index(after));
    let mut c = Changes::default();
    for (id, v) in &b {
        match a.get(id) {
            None => c.created.push(id.clone()),
            Some(old) if old != v => c.modified.push(id.clone()),
            _ => {}
        }
    }
    for id in a.keys() {
        if !b.contains_key(id) {
            c.deleted.push(id.clone());
        }
    }
    let key = |s: &String| (s.chars().next(), s[1..].parse::<u64>().unwrap_or(0));
    c.created.sort_by_key(key);
    c.modified.sort_by_key(key);
    c.deleted.sort_by_key(key);
    c
}

fn ctx(p: &Project, playhead: Option<i64>) -> TimeContext {
    p.time_context(playhead)
}

fn resolve(p: &Project, t: &TimeSpec, playhead: Option<i64>) -> Result<i64> {
    Ok(ctx(p, playhead).resolve(t)?)
}

/// Resolves a length given as a TimeSpec, allowing a sign (`-2s`).
fn length(p: &Project, t: &TimeSpec) -> Result<i64> {
    Ok(ctx(p, None).duration(t)?)
}

/// Resolves a clip reference (`c17` or `V1@<TimeSpec>`) to a clip id.
pub fn resolve_clip(p: &Project, r: &ClipRef, playhead: Option<i64>) -> Result<String> {
    if let Some((track, at)) = r.split_once('@') {
        let t = p
            .track(track)
            .ok_or_else(|| Error::not_found(format!("no track {track}")))?;
        let frame = resolve(p, &TimeSpec::parse(at)?, playhead)?;
        let ci = t
            .clip_at(frame)
            .ok_or_else(|| Error::not_found(format!("no clip on {track} at frame {frame}")))?;
        return Ok(t.clips[ci].id.clone());
    }
    p.clip(r)
        .map(|c| c.id.clone())
        .ok_or_else(|| Error::not_found(format!("no clip {r}")))
}

fn resolve_clips(p: &Project, refs: &[ClipRef], playhead: Option<i64>) -> Result<Vec<String>> {
    refs.iter().map(|r| resolve_clip(p, r, playhead)).collect()
}

/// Source range of `params` in source frames (default: the whole media).
fn source(p: &Project, params: &SourceParams) -> Result<Source> {
    let media = p
        .media(&params.media)
        .ok_or_else(|| Error::not_found(format!("no media {}", params.media)))?;
    let frames = p.media_frames(media);
    let range = match &params.range {
        None => (0, frames),
        Some(r) => {
            // Source times count from the media's first frame; `end` is its end.
            let c = TimeContext {
                fps: p.settings.fps,
                start_tc: 0,
                end: frames,
                mark_in: None,
                mark_out: None,
                playhead: None,
            };
            c.range(r)?
        }
    };
    Ok(Source {
        media: params.media.clone(),
        range,
        video_only: params.video_only,
    })
}

fn need(at: Option<i64>, what: &str) -> Result<i64> {
    at.ok_or_else(|| Error::usage(format!("{what} is required (no live playhead)")))
}

/// Applies a mutation to `p` and returns its method-specific result.
fn mutate(
    p: &mut Project,
    request: &Request,
    env: &dyn Env,
    playhead: Option<i64>,
) -> Result<Value> {
    Ok(match request {
        Request::ProjectSet(set) => {
            project_set(p, set)?;
            Value::Null
        }
        Request::MediaAdd { paths } => {
            let mut ids = Vec::new();
            for path in paths {
                let probed = env.probe(path)?;
                if let Some(existing) = p.media.iter().find(|m| m.path == probed.path) {
                    ids.push(existing.id.clone());
                    continue;
                }
                if p.settings.auto_format
                    && let Some(v) = &probed.info.video
                {
                    // The first video sets the project format (the Cut page
                    // offers this on the first import, p. 654).
                    p.settings.fps = v.fps;
                    let sar = if v.sar[0] > 0 && v.sar[1] > 0 {
                        f64::from(v.sar[0]) / f64::from(v.sar[1])
                    } else {
                        1.0
                    };
                    p.settings.width = ((f64::from(v.width) * sar).round() as u32).max(2) & !1;
                    p.settings.height = v.height & !1;
                    p.settings.auto_format = false;
                }
                let id = p.alloc_media_id();
                let name = probed
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| id.clone());
                p.media.push(Media {
                    id: id.clone(),
                    path: probed.path,
                    name,
                    size: probed.size,
                    mtime_ns: probed.mtime_ns,
                    info: probed.info,
                });
                ids.push(id);
            }
            json!({ "media": ids })
        }
        Request::MediaRemove { media, force } => {
            p.media(media)
                .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
            let users: Vec<String> = p
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter(|c| &c.media == media)
                .map(|c| c.id.clone())
                .collect();
            if !users.is_empty() {
                if !force {
                    return Err(Error::invalid(format!(
                        "{media} is used by {}; pass force to delete those clips too",
                        users.join(", ")
                    )));
                }
                ops::delete(p, &users, false)?;
            }
            p.media.retain(|m| &m.id != media);
            Value::Null
        }
        Request::MediaRelink { media, path } => {
            let probed = env.probe(path)?;
            let m = p
                .media
                .iter_mut()
                .find(|m| &m.id == media)
                .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
            m.path = probed.path;
            m.size = probed.size;
            m.mtime_ns = probed.mtime_ns;
            m.info = probed.info;
            Value::Null
        }
        Request::MediaRename { media, name } => {
            let m = p
                .media
                .iter_mut()
                .find(|m| &m.id == media)
                .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
            m.name = name.clone();
            Value::Null
        }
        Request::EditAppend(src) => {
            let s = source(p, src)?;
            json!({ "clip": ops::append(p, &s)? })
        }
        Request::EditSmartInsert {
            source: src,
            playhead: at,
        } => {
            let at = resolve(p, at, playhead)?;
            let s = source(p, src)?;
            let edit = ops::nearest_edit(p, at);
            json!({ "clip": ops::smart_insert(p, &s, at)?, "at": edit })
        }
        Request::EditInsert { source: src, at } => {
            let at = resolve(p, at, playhead)?;
            let s = source(p, src)?;
            json!({ "clip": ops::insert_at(p, &s, at)? })
        }
        Request::EditOverwrite { source: src, at } => {
            let at = match at {
                Some(t) => Some(resolve(p, t, playhead)?),
                None => playhead,
            };
            let marked = p.marks.mark_in.is_some() || p.marks.mark_out.is_some();
            let at = if marked {
                at.unwrap_or(0)
            } else {
                need(at, "at")?
            };
            let s = source(p, src)?;
            json!({ "clip": ops::overwrite(p, &s, at)? })
        }
        Request::EditRippleOverwrite { source: src, at } => {
            let at = match at {
                Some(t) => Some(resolve(p, t, playhead)?),
                None => playhead,
            };
            let marked = p.marks.mark_in.is_some() && p.marks.mark_out.is_some();
            let at = if marked {
                at.unwrap_or(0)
            } else {
                need(at, "at")?
            };
            let s = source(p, src)?;
            json!({ "clip": ops::ripple_overwrite(p, &s, at)? })
        }
        Request::EditSplit { at, clips } => {
            let points = at
                .to_vec()
                .iter()
                .map(|t| resolve(p, t, playhead))
                .collect::<Result<Vec<_>>>()?;
            let selected = resolve_clips(p, clips, playhead)?;
            json!({ "created": ops::split(p, &points, &selected)? })
        }
        Request::EditDelete { clips, lift } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            ops::delete(p, &ids, *lift)?;
            Value::Null
        }
        Request::EditDeleteRange { range, lift } => {
            let (a, b) = ctx(p, playhead).range(range)?;
            ops::delete_range(p, a, b, *lift)?;
            Value::Null
        }
        Request::EditCloseGap { at } => {
            let at = resolve(p, at, playhead)?;
            ops::close_gap(p, at)?;
            Value::Null
        }
        Request::EditMove { clips, to, mode } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let to = resolve(p, to, playhead)?;
            ops::move_clips(p, &ids, to, *mode)?;
            Value::Null
        }
        Request::EditReplace { clip, onto } => {
            let id = resolve_clip(p, clip, playhead)?;
            let onto = resolve_clip(p, onto, playhead)?;
            ops::move_replace(p, &id, &onto)?;
            Value::Null
        }
        Request::EditTrim {
            clip,
            edge,
            by,
            to,
            ripple,
        } => {
            let id = resolve_clip(p, clip, playhead)?;
            let c = p.clip(&id).expect("resolved").clone();
            let edge_at = match edge {
                Edge::Head => c.start,
                Edge::Tail => c.end(),
            };
            let delta = match (by, to) {
                (Some(by), None) => length(p, by)?,
                (None, Some(to)) => ctx(p, playhead).resolve_from(to, edge_at)? - edge_at,
                _ => return Err(Error::usage("edit.trim needs exactly one of `by` and `to`")),
            };
            ops::trim(p, &id, *edge, delta, *ripple)?;
            Value::Null
        }
        Request::EditRoll { clip, edge, by } => {
            let id = resolve_clip(p, clip, playhead)?;
            let delta = length(p, by)?;
            ops::roll(p, &id, *edge, delta)?;
            Value::Null
        }
        Request::EditSlip { clip, by } => {
            let id = resolve_clip(p, clip, playhead)?;
            let delta = length(p, by)?;
            ops::slip(p, &id, delta)?;
            Value::Null
        }
        Request::EditAudioRoll { clip, edge, by } => {
            let id = resolve_clip(p, clip, playhead)?;
            let delta = length(p, by)?;
            ops::audio_roll(p, &id, *edge, delta)?;
            Value::Null
        }
        Request::EditTrimToPlayhead {
            edge,
            at,
            clips,
            ripple,
        } => {
            let at = resolve(p, at, playhead)?;
            let ids = resolve_clips(p, clips, playhead)?;
            ops::trim_to(p, *edge, at, &ids, *ripple)?;
            Value::Null
        }
        Request::EditChangeDuration {
            clips,
            duration,
            ripple,
        } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let frames = length(p, duration)?;
            ops::change_duration(p, &ids, frames, *ripple)?;
            Value::Null
        }
        Request::ClipSet {
            clips,
            enabled,
            muted,
            gain_db,
            name,
        } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let props = ClipProps {
                enabled: *enabled,
                muted: *muted,
                gain_db: *gain_db,
                name: name.as_ref().map(|n| (!n.is_empty()).then(|| n.clone())),
            };
            ops::set_clips(p, &ids, &props)?;
            Value::Null
        }
        Request::MarksSet { mark_in, out } => {
            let i = mark_in
                .as_ref()
                .map(|t| resolve(p, t, playhead))
                .transpose()?;
            let o = out.as_ref().map(|t| resolve(p, t, playhead)).transpose()?;
            if i.is_none() && o.is_none() {
                return Err(Error::usage("marks.set needs `in` and/or `out`"));
            }
            ops::set_marks(p, i.map(Some), o.map(Some))?;
            json!({ "in": p.marks.mark_in, "out": p.marks.mark_out })
        }
        Request::MarksClear {} => {
            ops::set_marks(p, Some(None), Some(None))?;
            Value::Null
        }
        Request::MarkersAdd {
            at,
            name,
            color,
            note,
        } => {
            let at = resolve(p, at, playhead)?;
            json!({ "marker": ops::add_marker(p, at, name.clone(), color.clone(), note.clone())? })
        }
        Request::MarkersMove { marker, to } => {
            let to = resolve(p, to, playhead)?;
            ops::move_marker(p, marker, to)?;
            Value::Null
        }
        Request::MarkersRemove { marker } => {
            ops::remove_marker(p, marker)?;
            Value::Null
        }
        Request::Apply { commands, .. } => {
            let mut results = Vec::new();
            for (i, cmd) in commands.iter().enumerate() {
                if matches!(
                    cmd,
                    Request::HistoryUndo { .. }
                        | Request::HistoryRedo { .. }
                        | Request::Apply { .. }
                ) {
                    return Err(Error::usage(format!(
                        "command {i}: {} cannot be batched",
                        cmd.method()
                    )));
                }
                let r = if cmd.is_mutation() {
                    mutate(p, cmd, env, playhead)
                } else {
                    query(p, &History::default(), cmd, env, playhead)
                };
                results.push(r.map_err(|e| {
                    Error::new(
                        e.code,
                        format!("command {i} ({}): {}", cmd.method(), e.message),
                    )
                })?);
            }
            json!({ "results": results })
        }
        other => {
            return Err(Error::internal(format!(
                "{} is not a mutation",
                other.method()
            )));
        }
    })
}

fn project_set(p: &mut Project, set: &ProjectSet) -> Result<()> {
    if let Some(fps) = set.fps {
        let has_clips = p.tracks.iter().any(|t| !t.clips.is_empty());
        if has_clips && fps != p.settings.fps {
            return Err(Error::invalid(
                "the frame rate can only change while the timeline is empty",
            ));
        }
        p.settings.fps = fps;
        p.settings.auto_format = false;
    }
    if let Some(w) = set.width {
        if w < 16 || w % 2 != 0 {
            return Err(Error::invalid("width must be even and at least 16"));
        }
        p.settings.width = w;
        p.settings.auto_format = false;
    }
    if let Some(h) = set.height {
        if h < 16 || h % 2 != 0 {
            return Err(Error::invalid("height must be even and at least 16"));
        }
        p.settings.height = h;
        p.settings.auto_format = false;
    }
    if let Some(sr) = set.sample_rate {
        if !(8_000..=192_000).contains(&sr) {
            return Err(Error::invalid("sample rate must be 8000..192000"));
        }
        p.settings.sample_rate = sr;
    }
    if let Some(ch) = set.channels {
        if !(1..=8).contains(&ch) {
            return Err(Error::invalid("channels must be 1..8"));
        }
        p.settings.channels = ch;
    }
    if let Some(r) = set.ripple_on {
        p.settings.ripple_on = r;
    }
    if let Some(tc) = &set.start_tc {
        let c = TimeContext {
            start_tc: 0,
            ..p.time_context(None)
        };
        p.settings.start_tc = c.resolve(tc)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- queries

/// Answers a query request.
pub fn query(
    p: &Project,
    history: &History,
    request: &Request,
    env: &dyn Env,
    playhead: Option<i64>,
) -> Result<Value> {
    let fps = p.settings.fps;
    let tc = |f: i64| format_timecode(f, fps, p.settings.start_tc);
    Ok(match request {
        Request::ProjectGet {} => {
            serde_json::to_value(p).map_err(|e| Error::internal(e.to_string()))?
        }
        Request::ProjectSummary {} => json!({
            "rev": p.rev,
            "settings": p.settings,
            "media": p.media.len(),
            "clips": p.tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
            "tracks": p.tracks.iter().map(|t| &t.id).collect::<Vec<_>>(),
            "duration": p.end(),
            "duration_tc": tc(p.end()),
            "marks": p.marks,
            "markers": p.markers.len(),
        }),
        Request::MediaList {} => {
            json!(p.media.iter().map(|m| media_json(p, m)).collect::<Vec<_>>())
        }
        Request::MediaGet { media } => {
            let m = p
                .media(media)
                .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
            let mut v = media_json(p, m);
            v["info"] = serde_json::to_value(&m.info).unwrap_or_default();
            v["used_by"] = json!(
                p.tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .filter(|c| &c.media == media)
                    .map(|c| &c.id)
                    .collect::<Vec<_>>()
            );
            v
        }
        Request::Probe { path } => {
            let probed = env.probe(path)?;
            json!({ "path": probed.path, "size": probed.size, "info": probed.info })
        }
        Request::TimelineGet { range } => {
            let (a, b) = match range {
                Some(r) => ctx(p, playhead).range(r)?,
                None => (0, p.end()),
            };
            timeline_json(p, a, b)
        }
        Request::TimelineAt { at } => {
            let at = resolve(p, at, playhead)?;
            let items: Vec<Value> = p
                .tracks
                .iter()
                .filter_map(|t| t.clip_at(at).map(|ci| (t, &t.clips[ci])))
                .map(|(t, c)| {
                    let mut v = clip_json(p, c, &t.id);
                    v["source_frame"] = json!(c.source_at(at));
                    v
                })
                .collect();
            json!({ "at": at, "at_tc": tc(at), "clips": items })
        }
        Request::TimelineEditPoints {} => {
            let points = p
                .track(ops::MAIN)
                .map(|t| t.edit_points())
                .unwrap_or_default();
            json!(
                points
                    .iter()
                    .map(|&f| json!({ "frame": f, "tc": tc(f) }))
                    .collect::<Vec<_>>()
            )
        }
        Request::HistoryGet {} => json!({
            "undo": history.undo.iter().rev().map(|e| &e.label).collect::<Vec<_>>(),
            "redo": history.redo.iter().rev().map(|e| &e.label).collect::<Vec<_>>(),
        }),
        Request::Schema { .. }
        | Request::Doctor {}
        | Request::ExportPlan(_)
        | Request::ExportStart(_)
        | Request::FrameRender { .. }
        | Request::PresetsList {}
        | Request::PresetsGet { .. }
        | Request::EncodersList { .. } => {
            return Err(Error::internal(format!(
                "{} is answered by the session, not the editor",
                request.method()
            )));
        }
        other => {
            return Err(Error::internal(format!(
                "{} is not a query",
                other.method()
            )));
        }
    })
}

fn media_json(p: &Project, m: &Media) -> Value {
    let frames = p.media_frames(m);
    json!({
        "id": m.id,
        "name": m.name,
        "path": m.path,
        "frames": frames,
        "duration_tc": format_timecode(frames, p.settings.fps, 0),
        "video": m.info.video.as_ref().map(|v| json!({ "width": v.width, "height": v.height, "fps": v.fps.to_string(), "codec": v.codec })),
        "audio_streams": m.info.audio.len(),
    })
}

/// A clip as reported by timeline queries.
pub fn clip_json(p: &Project, c: &Clip, track: &str) -> Value {
    let (fps, stc) = (p.settings.fps, p.settings.start_tc);
    let mut v = json!({
        "type": "clip",
        "id": c.id,
        "track": track,
        "media": c.media,
        "name": c.name.clone().or_else(|| p.media(&c.media).map(|m| m.name.clone())),
        "start": c.start,
        "end": c.end(),
        "duration": c.duration,
        "start_tc": format_timecode(c.start, fps, stc),
        "end_tc": format_timecode(c.end(), fps, stc),
        "source_in": c.source_in,
        "source_out": c.source_in + c.duration,
        "source_in_tc": format_timecode(c.source_in, fps, 0),
        "enabled": c.enabled,
    });
    if let Some(a) = &c.audio {
        v["audio"] = json!({ "stream": a.stream, "head": a.head, "tail": a.tail, "gain_db": a.gain_db, "muted": a.muted });
    }
    v
}

fn timeline_json(p: &Project, a: i64, b: i64) -> Value {
    let (fps, stc) = (p.settings.fps, p.settings.start_tc);
    let tracks: Vec<Value> = p
        .tracks
        .iter()
        .map(|t| {
            let mut items = Vec::new();
            let mut at = 0;
            for c in &t.clips {
                if c.start > at && c.start > a && at < b {
                    let (gs, ge) = (at.max(a), c.start.min(b));
                    items.push(json!({ "type": "gap", "start": gs, "end": ge, "duration": ge - gs,
                        "start_tc": format_timecode(gs, fps, stc), "end_tc": format_timecode(ge, fps, stc) }));
                }
                if c.end() > a && c.start < b {
                    items.push(clip_json(p, c, &t.id));
                }
                at = c.end();
            }
            json!({ "id": t.id, "kind": t.kind, "locked": t.locked, "muted": t.muted, "items": items })
        })
        .collect();
    json!({
        "fps": fps.to_string(),
        "start": a,
        "end": b,
        "duration": p.end(),
        "duration_tc": format_timecode(p.end(), fps, stc),
        "tracks": tracks,
        "marks": p.marks,
        "markers": p.markers,
    })
}

/// A fixed rate for tests and tools that need one.
pub fn rate(num: i64, den: i64) -> Rational {
    Rational::new(num, den).expect("valid rate")
}
