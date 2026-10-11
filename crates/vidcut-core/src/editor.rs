//! Applies requests to a project with snapshot undo, revisions and dry runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, ClipAudio, Media, MediaInfo, Project, VideoInfo};
use crate::ops::{self, ClipProps, Edge, Source};
use crate::otio;
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

    /// Files named `name` in `dirs` or their subfolders, nearest first: how
    /// an import finds media that moved (Resolve searches subfolders too,
    /// p. 530).
    fn find_file(&self, dirs: &[PathBuf], name: &std::ffi::OsStr) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for dir in dirs {
            let mut queue = std::collections::VecDeque::from([(dir.clone(), 0)]);
            while let Some((d, depth)) = queue.pop_front() {
                let Ok(entries) = std::fs::read_dir(&d) else {
                    continue;
                };
                let mut subdirs = Vec::new();
                for e in entries.flatten() {
                    let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                    if !is_dir && e.file_name() == name {
                        found.push(e.path());
                    } else if is_dir
                        && depth < 8
                        && !e.file_name().to_string_lossy().starts_with('.')
                    {
                        subdirs.push(e.path());
                    }
                }
                subdirs.sort();
                queue.extend(subdirs.into_iter().map(|p| (p, depth + 1)));
            }
        }
        found
    }

    /// Reads a text file (an `.otio` file to import).
    fn read_text(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path).map_err(|e| {
            let message = format!("{}: {e}", path.display());
            match e.kind() {
                std::io::ErrorKind::NotFound => Error::not_found(message),
                _ => Error::usage(message),
            }
        })
    }
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

/// Context for source times of `media`: they count from its first frame,
/// `end` is its end and `in`/`out` are its source marks.
pub fn media_context(p: &Project, media: &Media, playhead: Option<i64>) -> TimeContext {
    TimeContext {
        fps: p.settings.fps,
        start_tc: 0,
        end: p.media_frames(media),
        mark_in: media.marks.mark_in,
        mark_out: media.marks.mark_out,
        playhead,
    }
}

/// Source range of `params` in source frames. Default: the media's source
/// marks, with a missing In meaning its first frame and a missing Out its end
/// (decision D5).
fn source(p: &Project, params: &SourceParams) -> Result<Source> {
    let media = p
        .media(&params.media)
        .ok_or_else(|| Error::not_found(format!("no media {}", params.media)))?;
    let frames = p.media_frames(media);
    let range = match &params.range {
        None => (
            media.marks.mark_in.unwrap_or(0),
            media.marks.mark_out.unwrap_or(frames),
        ),
        Some(r) => media_context(p, media, None).range(r)?,
    };
    Ok(Source {
        media: params.media.clone(),
        range,
        video_only: params.video_only,
        audio_only: params.audio_only,
        track: params.track.clone(),
    })
}

/// Runs `f` with the timeline marks hidden when `hide` (they come back
/// afterwards): edits placed by position rather than by marks.
fn without_marks<T>(
    p: &mut Project,
    hide: bool,
    f: impl FnOnce(&mut Project) -> Result<T>,
) -> Result<T> {
    if !hide {
        return f(p);
    }
    let marks = std::mem::take(&mut p.marks);
    let result = f(p);
    p.marks = marks;
    result
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
        Request::MediaAdd { paths, bin } => {
            let mut ids = Vec::new();
            for path in paths {
                ids.push(add_media(p, env.probe(path)?));
            }
            if let Some(bin) = bin {
                ops::move_media(p, &ids, bin)?;
            }
            json!({ "media": ids })
        }
        Request::MediaMove { media, bin } => {
            ops::move_media(p, &media.to_vec(), bin)?;
            Value::Null
        }
        Request::BinsAdd { path } => {
            ops::add_bin(p, path)?;
            Value::Null
        }
        Request::BinsRemove { path } => {
            ops::remove_bin(p, path)?;
            Value::Null
        }
        Request::BinsRename { path, to } => {
            ops::rename_bin(p, path, to)?;
            Value::Null
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
        Request::MediaMarksSet {
            media,
            mark_in,
            out,
        } => {
            let m = p
                .media(media)
                .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
            let c = media_context(p, m, None);
            let i = mark_in.as_ref().map(|t| c.resolve(t)).transpose()?;
            let o = out.as_ref().map(|t| c.resolve(t)).transpose()?;
            if i.is_none() && o.is_none() {
                return Err(Error::usage("media.marks.set needs `in` and/or `out`"));
            }
            ops::set_media_marks(p, media, i.map(Some), o.map(Some))?;
            let m = p.media(media).expect("exists");
            json!({ "in": m.marks.mark_in, "out": m.marks.mark_out })
        }
        Request::MediaMarksClear { media } => {
            ops::set_media_marks(p, media, Some(None), Some(None))?;
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
        Request::EditOverwrite {
            source: src,
            at,
            use_marks,
        } => {
            let at = match at {
                Some(t) => Some(resolve(p, t, playhead)?),
                None => playhead,
            };
            let marked = *use_marks && (p.marks.mark_in.is_some() || p.marks.mark_out.is_some());
            let at = if marked {
                at.unwrap_or(0)
            } else {
                need(at, "at")?
            };
            let s = source(p, src)?;
            let clip = without_marks(p, !use_marks, |p| ops::overwrite(p, &s, at))?;
            json!({ "clip": clip })
        }
        Request::EditPlaceOnTop {
            source: src,
            at,
            use_marks,
        } => {
            if src.track.is_some() {
                return Err(Error::usage(
                    "Place On Top picks its own track (p. 699); leave out `track`",
                ));
            }
            let at = match at {
                Some(t) => Some(resolve(p, t, playhead)?),
                None => playhead,
            };
            let marked = *use_marks && (p.marks.mark_in.is_some() || p.marks.mark_out.is_some());
            let at = if marked {
                at.unwrap_or(0)
            } else {
                need(at, "at")?
            };
            let s = source(p, src)?;
            let (clip, track) = without_marks(p, !use_marks, |p| ops::place_on_top(p, &s, at))?;
            json!({ "clip": clip, "track": track })
        }
        Request::EditRippleOverwrite {
            source: src,
            at,
            use_marks,
        } => {
            let at = match at {
                Some(t) => Some(resolve(p, t, playhead)?),
                None => playhead,
            };
            let marked = *use_marks && p.marks.mark_in.is_some() && p.marks.mark_out.is_some();
            let at = if marked {
                at.unwrap_or(0)
            } else {
                need(at, "at")?
            };
            let s = source(p, src)?;
            let clip = without_marks(p, !use_marks, |p| ops::ripple_overwrite(p, &s, at))?;
            json!({ "clip": clip })
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
        Request::EditMove {
            clips,
            to,
            mode,
            track,
        } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let to = resolve(p, to, playhead)?;
            ops::move_clips(p, &ids, to, *mode, track.as_deref())?;
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
            if matches!(duration, TimeSpec::Keyword(crate::time::Keyword::End)) {
                // "End": each clip runs to the end of its media (p. 693).
                ops::extend_to_media_end(p, &ids, *ripple)?;
            } else {
                let frames = length(p, duration)?;
                ops::change_duration(p, &ids, frames, *ripple)?;
            }
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
        Request::EditCut { clips } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let copied = ops::copy_clips(p, &ids)?;
            ops::delete(p, &ids, false)?;
            json!({ "clips": copied })
        }
        Request::EditPaste { clips, at } => {
            let clips = clips.as_ref().ok_or_else(|| {
                Error::usage("edit.paste needs `clips` (the GUI pastes its clipboard)")
            })?;
            let at = match at {
                Some(t) => resolve(p, t, playhead)?,
                None => need(playhead, "at")?,
            };
            json!({ "clips": ops::paste(p, clips, at)? })
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
        Request::MarkersSet {
            marker,
            name,
            color,
            note,
        } => {
            ops::set_marker(p, marker, name.clone(), color.clone(), note.clone())?;
            Value::Null
        }
        Request::TrackSet {
            track,
            locked,
            muted,
            solo,
            volume_db,
            video_enabled,
            name,
        } => {
            let props = ops::TrackProps {
                locked: *locked,
                muted: *muted,
                solo: *solo,
                volume_db: *volume_db,
                video_enabled: *video_enabled,
                name: name.as_ref().map(|n| (!n.is_empty()).then(|| n.clone())),
            };
            ops::set_track(p, track, &props)?;
            Value::Null
        }
        Request::TrackAdd { kind, count } => {
            if !(1..=16).contains(count) {
                return Err(Error::usage("count must be 1..16"));
            }
            let ids: Vec<String> = (0..*count).map(|_| ops::add_track(p, *kind)).collect();
            json!({ "tracks": ids })
        }
        Request::TrackRemove { track, force } => {
            ops::remove_track(p, track, *force)?;
            Value::Null
        }
        Request::MarkersRemove { marker } => {
            ops::remove_marker(p, marker)?;
            Value::Null
        }
        Request::TimelineImport {
            path,
            otio,
            replace,
            search,
        } => import_timeline(p, path.as_deref(), otio.as_ref(), *replace, search, env)?,
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

/// Adds probed media to the pool (or finds it there); returns its id.
fn add_media(p: &mut Project, probed: ProbedMedia) -> String {
    if let Some(existing) = p.media.iter().find(|m| m.path == probed.path) {
        return existing.id.clone();
    }
    if p.settings.auto_format
        && let Some(v) = &probed.info.video
    {
        // The first video sets the project format (the Cut page offers this
        // on the first import, p. 654).
        p.settings.fps = v.fps;
        adopt_size(p, v);
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
        marks: Default::default(),
        bin: String::new(),
    });
    id
}

/// Takes the frame size of `v` (square pixels, even) and fixes the format.
fn adopt_size(p: &mut Project, v: &VideoInfo) {
    let sar = if v.sar[0] > 0 && v.sar[1] > 0 {
        f64::from(v.sar[0]) / f64::from(v.sar[1])
    } else {
        1.0
    };
    p.settings.width = ((f64::from(v.width) * sar).round() as u32).max(2) & !1;
    p.settings.height = v.height & !1;
    p.settings.auto_format = false;
}

/// Frame `f` at rate `from` as the nearest frame at rate `to`.
fn rescale(f: i64, from: Rational, to: Rational) -> i64 {
    if from == to {
        return f;
    }
    let n = i128::from(f) * i128::from(from.den()) * i128::from(to.num());
    let d = i128::from(from.num()) * i128::from(to.den());
    (2 * n + d).div_euclid(2 * d) as i64
}

/// `timeline.import`: an OpenTimelineIO timeline onto V1 ([`otio`]).
fn import_timeline(
    p: &mut Project,
    path: Option<&Path>,
    doc: Option<&Value>,
    replace: bool,
    search: &[PathBuf],
    env: &dyn Env,
) -> Result<Value> {
    let read;
    let doc = match (path, doc) {
        (Some(path), None) => {
            read = serde_json::from_str::<Value>(&env.read_text(path)?).map_err(|e| {
                Error::usage(format!("{}: not an OTIO (JSON) file: {e}", path.display()))
            })?;
            &read
        }
        (None, Some(doc)) => doc,
        _ => {
            return Err(Error::usage(
                "timeline.import needs exactly one of `path` and `otio`",
            ));
        }
    };
    let t = otio::import(doc)?;
    if t.clips.is_empty() {
        return Err(Error::invalid(format!(
            "nothing to import: the timeline has no clips with media files{}",
            if t.warnings.is_empty() {
                String::new()
            } else {
                format!(" ({})", t.warnings.join("; "))
            }
        )));
    }
    let mut warnings = t.warnings.clone();

    // Media: each file probed once, looked for by name in `search` (and
    // subfolders) if it isn't where the file says.
    let mut ids: HashMap<PathBuf, String> = HashMap::new();
    let mut media = Vec::new();
    let mut missing: Vec<&Path> = Vec::new();
    for c in &t.clips {
        if ids.contains_key(&c.path) || missing.contains(&c.path.as_path()) {
            continue;
        }
        let probed = env.probe(&c.path).or_else(|e| {
            c.path
                .file_name()
                .and_then(|name| {
                    env.find_file(search, name)
                        .iter()
                        .find_map(|p| env.probe(p).ok())
                })
                .ok_or(e)
        });
        match probed {
            Ok(m) => {
                let id = add_media(p, m);
                if !media.contains(&id) {
                    media.push(id.clone());
                }
                ids.insert(c.path.clone(), id);
            }
            Err(_) => missing.push(&c.path),
        }
    }
    if !missing.is_empty() {
        return Err(Error::media(format!(
            "{} media file(s) not found: {}{}",
            missing.len(),
            missing
                .iter()
                .map(|m| m.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            if search.is_empty() {
                "; pass `search` folders to look for them by name (subfolders too)"
            } else {
                ""
            }
        )));
    }

    if replace {
        for track in &mut p.tracks {
            track.clips.clear();
        }
        p.markers.clear();
        p.marks = Default::default();
    }
    // An empty timeline takes the file's rate and start timecode; otherwise
    // positions are converted to the project's rate.
    let empty = p.tracks.iter().all(|t| t.clips.is_empty());
    let from = t.fps;
    if empty {
        if p.settings.auto_format
            && let Some(v) = t
                .clips
                .iter()
                .find_map(|c| p.media(&ids[&c.path])?.info.video.clone())
        {
            adopt_size(p, &v);
        }
        p.settings.fps = t.fps;
        p.settings.start_tc = t.start_tc;
        p.settings.auto_format = false;
    } else if t.fps != p.settings.fps {
        warnings.push(format!(
            "converted from {} to the project's {} fps: cuts may move by up to half a frame",
            t.fps, p.settings.fps
        ));
    }
    let to = p.settings.fps;
    let conv = |f: i64| rescale(f, from, to);
    let offset = if empty { 0 } else { p.end() };
    // The tracks first (making video tracks moves the audio tracks' indexes).
    let mut needed: Vec<&str> = t.clips.iter().map(|c| c.track.as_str()).collect();
    needed.extend(t.tracks.iter().map(|t| t.id.as_str()));
    needed.sort_by_key(|id| (!id.starts_with('V'), id[1..].parse::<usize>().unwrap_or(0)));
    needed.dedup();
    for id in needed {
        ops::ensure_track(p, id)?;
    }
    if empty {
        for s in &t.tracks {
            if let Some(track) = p.track_mut(&s.id) {
                track.locked = s.locked;
                track.muted = s.muted;
                track.solo = s.solo;
                track.volume_db = s.volume_db.clamp(-60.0, 24.0);
                track.video_enabled = s.video_enabled;
                track.name = s.name.clone();
            }
        }
    }
    // Where audio may start on each track without overlapping.
    let mut floors: HashMap<String, i64> = p
        .tracks
        .iter()
        .map(|t| {
            let end = t
                .clips
                .iter()
                .filter_map(|c| c.audio_range().map(|r| r.1))
                .max();
            (t.id.clone(), end.unwrap_or(0))
        })
        .collect();

    let mut clips = Vec::new();
    for c in &t.clips {
        let m = p.media(&ids[&c.path]).expect("added").clone();
        let frames = p.media_frames(&m);
        let label = if c.name.is_empty() { &m.name } else { &c.name };
        let mut start = offset + conv(c.start);
        let end = offset + conv(c.end());
        let mut source_in = conv(c.source_in);
        if c.video && m.info.video.is_none() {
            warnings.push(format!("{label}: {} has no video; left out", m.name));
            continue;
        }
        // Keep to the media (rates and rounding may differ by a frame).
        if source_in < 0 {
            warnings.push(format!(
                "{label}: {} frame(s) before the start of {} were left out",
                -source_in, m.name
            ));
            start -= source_in;
            source_in = 0;
        }
        let duration = (end - start).min(frames - source_in);
        if duration < end - start {
            warnings.push(format!(
                "{label}: {} frame(s) past the end of {} were left out",
                end - start - duration.max(0),
                m.name
            ));
        }
        if duration < 1 {
            continue;
        }
        // The audio's timeline range, then as offsets from the video edges.
        let audio = match (c.audio, t.has_audio_tracks) {
            (Some((h, tl)), _) => Some((
                offset + conv(c.start + h) - start,
                offset + conv(c.end() + tl) - (start + duration),
            )),
            // No audio tracks in the file: clips bring their media's audio.
            (None, false) => Some((0, 0)),
            (None, true) => None,
        };
        let audio = audio
            .filter(|_| !m.info.audio.is_empty())
            .and_then(|(h, tl)| {
                let floor = floors.get(&c.track).copied().unwrap_or(0);
                let h = h.max(-source_in).max(floor - start);
                let tl = tl.min(frames - source_in - duration);
                (duration + tl - h >= 1).then_some(ClipAudio {
                    stream: if (c.stream as usize) < m.info.audio.len() {
                        c.stream
                    } else {
                        0
                    },
                    head: h,
                    tail: tl,
                    gain_db: c.gain_db,
                    muted: c.muted,
                })
            });
        if !c.video && audio.is_none() {
            warnings.push(format!("{label}: {} has no audio; left out", m.name));
            continue;
        }
        if let Some(a) = &audio {
            floors.insert(c.track.clone(), start + duration + a.tail);
        }
        let name = (!c.name.is_empty()
            && c.name != m.name
            && Some(c.name.as_str()) != c.path.file_name().and_then(|n| n.to_str()))
        .then(|| c.name.clone());
        let id = p.alloc_clip_id();
        clips.push(id.clone());
        let track = p.track_mut(&c.track).expect("made above");
        track.clips.push(Clip {
            id,
            media: m.id.clone(),
            start,
            duration,
            source_in,
            video: c.video,
            audio,
            enabled: c.enabled,
            name,
        });
        track.sort();
    }
    let mut markers = Vec::new();
    for k in &t.markers {
        markers.push(ops::add_marker(
            p,
            offset + conv(k.at),
            k.name.clone(),
            Some(k.color.clone()),
            k.note.clone(),
        )?);
    }
    if empty {
        let (i, o) = (t.marks.mark_in.map(conv), t.marks.mark_out.map(conv));
        if !matches!((i, o), (Some(i), Some(o)) if i >= o) {
            p.marks = crate::model::Marks {
                mark_in: i.filter(|&i| i >= 0),
                mark_out: o.filter(|&o| o > 0),
            };
        }
    }
    Ok(json!({
        "name": t.name,
        "fps": p.settings.fps,
        "clips": clips,
        "media": media,
        "markers": markers,
        "warnings": warnings,
    }))
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
            "bins": p.bins,
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
        Request::EditCopy { clips } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            json!({ "clips": ops::copy_clips(p, &ids)? })
        }
        live if live.is_live() => {
            return Err(Error::no_instance(format!(
                "{} controls a running vidcut GUI; open the project with `vidcut -p FILE` \
                 (the CLI forwards to it automatically)",
                live.method()
            )));
        }
        Request::Schema { .. }
        | Request::Doctor {}
        | Request::ExportPlan(_)
        | Request::ExportStart(_)
        | Request::FrameRender { .. }
        | Request::PresetsList {}
        | Request::PresetsGet { .. }
        | Request::PresetsSave { .. }
        | Request::PresetsDelete { .. }
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
    let mut v = json!({
        "id": m.id,
        "name": m.name,
        "path": m.path,
        "frames": frames,
        "duration_tc": format_timecode(frames, p.settings.fps, 0),
        "video": m.info.video.as_ref().map(|v| json!({ "width": v.width, "height": v.height, "fps": v.fps.to_string(), "codec": v.codec })),
        "audio_streams": m.info.audio.len(),
    });
    if !m.marks.is_empty() {
        v["marks"] = json!(m.marks);
    }
    if !m.bin.is_empty() {
        v["bin"] = json!(m.bin);
    }
    v
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
            let mut v = json!({ "id": t.id, "kind": t.kind, "locked": t.locked, "muted": t.muted, "items": items });
            if t.solo {
                v["solo"] = json!(true);
            }
            if t.volume_db != 0.0 {
                v["volume_db"] = json!(t.volume_db);
            }
            if t.kind == crate::model::TrackKind::Video && !t.video_enabled {
                v["video_enabled"] = json!(false);
            }
            if let Some(n) = &t.name {
                v["name"] = json!(n);
            }
            v
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
