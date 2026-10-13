//! Applies requests to a project with snapshot undo, revisions and dry runs.

use crate::validate::GAIN;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::analysis::{self, AudioLevels, Loudness, PictureStats, SilenceSettings, Span};
use crate::error::{Error, Result};
use crate::model::{
    Clip, ClipAudio, Compound, Effects, Generator, ImageSequence, Media, MediaInfo, Project, Proxy,
    Transition, VideoInfo,
};
use crate::ops::{self, ClipProps, Edge, Source};
use crate::otio;
use crate::render::RenderPlan;
use crate::request::{
    Changes, ClipRef, Envelope, OneOrMany, ProjectSet, Request, Response, SourceParams,
};
use crate::time::{Rational, TimeContext, TimeSpec, format_timecode};
use crate::validate;

/// A probed media file, as the media layer reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbedMedia {
    /// Canonical absolute path (an image sequence's first file).
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i64,
    pub info: MediaInfo,
    /// For an image sequence, its files.
    pub sequence: Option<ImageSequence>,
    /// [`Media::fingerprint`]; none when the file couldn't be read for it.
    pub fingerprint: Option<String>,
}

/// A proxy file the editor records on a pool item.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedProxy {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub codec: String,
    /// How it was made, when it was made now: where it was decoded and
    /// with what (`decoded on the GPU (NVDEC), AV1, NVIDIA hardware
    /// (NVENC)`).
    pub made_with: Option<String>,
}

/// What the editor needs from the outside world.
pub trait Env {
    /// Probes a media file.
    fn probe(&self, path: &Path) -> Result<ProbedMedia>;

    /// Probes the numbered image sequence `path` is a file of
    /// ([`sequence::find`](crate::sequence::find)), its pictures shown at
    /// `rate`.
    fn probe_sequence(&self, path: &Path, rate: Rational) -> Result<ProbedMedia>;

    /// Files named `name` in `dirs` or their subfolders, nearest first: how
    /// an import finds media that moved.
    fn find_file(&self, dirs: &[PathBuf], name: &OsStr) -> Vec<PathBuf> {
        let names = HashSet::from([name.to_os_string()]);
        self.find_files(dirs, &names)
            .remove(name)
            .unwrap_or_default()
    }

    /// The files named any of `names` in `dirs` or their subfolders, by
    /// name, nearest first: one walk for many files.
    fn find_files(
        &self,
        dirs: &[PathBuf],
        names: &HashSet<OsString>,
    ) -> HashMap<OsString, Vec<PathBuf>> {
        let mut found: HashMap<OsString, Vec<PathBuf>> = HashMap::new();
        for dir in dirs {
            let mut queue = std::collections::VecDeque::from([(dir.clone(), 0)]);
            while let Some((d, depth)) = queue.pop_front() {
                let Ok(entries) = std::fs::read_dir(&d) else {
                    continue;
                };
                let mut subdirs = Vec::new();
                for e in entries.flatten() {
                    let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                    let name = e.file_name();
                    if !is_dir && names.contains(&name) {
                        found.entry(name).or_default().push(e.path());
                    } else if is_dir && depth < 8 && !name.to_string_lossy().starts_with('.') {
                        subdirs.push(e.path());
                    }
                }
                subdirs.sort();
                queue.extend(subdirs.into_iter().map(|p| (p, depth + 1)));
            }
        }
        found
    }

    /// Whether `m`'s file is there (an image sequence's first picture).
    fn present(&self, m: &Media) -> bool {
        m.path.is_file()
    }

    /// Peak levels of audio stream `stream` of `media`. Measuring decodes
    /// the whole stream, so the media layer keeps the result.
    fn audio_levels(&self, media: &Media, stream: u32) -> Result<Arc<AudioLevels>>;

    /// Picture measurements of `media`'s video, frame by frame (kept like
    /// [`audio_levels`](Self::audio_levels)).
    fn picture_stats(&self, media: &Media) -> Result<Arc<PictureStats>>;

    /// How loud `plan`'s audio is, mixed as an export would mix it.
    fn loudness(&self, plan: &RenderPlan) -> Result<Loudness>;

    /// Directory new proxy files are written into. The real env uses a
    /// `proxies` folder beside the project when it knows that path, and
    /// the cache otherwise.
    fn proxy_dir(&self) -> PathBuf;

    /// Writes a playback proxy of `media`'s pictures into `dir` (decision
    /// D41). `longest` is the longest side in pixels. `dry_run` reports the
    /// file that would be written and does not encode.
    fn generate_proxy(
        &self,
        media: &Media,
        longest: u32,
        dir: &Path,
        dry_run: bool,
    ) -> Result<GeneratedProxy>;

    /// The preferences' rules for automatic proxies, and the proxy size.
    fn proxy_rules(&self) -> crate::preferences::Proxies {
        crate::preferences::Proxies::default()
    }

    /// Deletes a proxy file. Missing files are fine.
    fn remove_proxy_file(&self, path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::media(format!("{}: {e}", path.display()))),
        }
    }

    /// Opens a timeline file to import: an `.otio` file, or an `.otiod`
    /// bundle folder (its `content.otio`). `.otioz` bundles (zip files) need
    /// the media layer, which unpacks their media into `unpack` (default: a
    /// folder named after the bundle, next to it) unless `write` is false.
    fn open_timeline(
        &self,
        path: &Path,
        unpack: Option<&Path>,
        write: bool,
    ) -> Result<TimelineFile> {
        let _ = (unpack, write);
        if is_otioz(path) {
            return Err(Error::usage(format!(
                "{}: .otioz bundles are read by the vidcut program",
                path.display()
            )));
        }
        read_timeline_file(path)
    }
}

/// A timeline file opened for import ([`Env::open_timeline`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineFile {
    /// The OTIO document.
    pub text: String,
    /// The folder relative media paths count from.
    pub base: PathBuf,
    /// For an `.otioz` bundle: where its media go.
    pub bundle: Option<Unpacked>,
}

/// Where an `.otioz` bundle's media are unpacked.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Unpacked {
    pub folder: PathBuf,
    /// Files and bytes of media in the bundle.
    pub files: usize,
    pub bytes: u64,
    /// All of them are there (false in a dry run that would unpack).
    pub unpacked: bool,
}

/// Whether `path` names an `.otioz` bundle.
pub fn is_otioz(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("otioz"))
}

/// Reads an `.otio` file, or the `content.otio` of an `.otiod` folder.
pub fn read_timeline_file(path: &Path) -> Result<TimelineFile> {
    let (file, base) = if path.is_dir() {
        (path.join("content.otio"), path.to_path_buf())
    } else {
        let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
        (path.to_path_buf(), base)
    };
    let text = std::fs::read_to_string(&file).map_err(|e| {
        let message = format!("{}: {e}", file.display());
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::not_found(message),
            _ => Error::usage(message),
        }
    })?;
    Ok(TimelineFile {
        text,
        base,
        bundle: None,
    })
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

/// Now, in milliseconds since 1970 UTC (`Project::edited`).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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
        envelope.check_rev(self.project.rev)?;
        let Envelope {
            request, dry_run, ..
        } = envelope;
        if !request.is_mutation() {
            let result = query(&self.project, &self.history, &request, env, playhead)?;
            return Ok(Response {
                rev: self.project.rev,
                changes: Changes::default(),
                result,
                dry_run: false,
                via: String::new(),
                warnings: Vec::new(),
            });
        }
        match request {
            Request::HistoryUndo { steps } => return self.step_history(steps, true, dry_run),
            Request::HistoryRedo { steps } => return self.step_history(steps, false, dry_run),
            _ => {}
        }
        let label = label_of(&request, &self.project);
        let mut working = self.project.clone();
        let result = mutate(&mut working, &request, env, playhead, dry_run)?;
        // Carried transitions the edit took away from their edit points.
        let mut warnings = Vec::new();
        let dropped = otio::transitions::prune(&mut working);
        if dropped > 0 {
            warnings.push(otio::transitions::dropped(dropped));
        }
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
                warnings,
            });
        }
        // An edit that changes nothing (no shot changes found, a setting set
        // to what it was) is no undo step and no new revision: an undo would
        // undo nothing, and the edit before would take two.
        if working == self.project {
            return Ok(Response {
                rev: self.project.rev,
                changes,
                result,
                dry_run: false,
                via: String::new(),
                warnings: Vec::new(),
            });
        }
        working.rev = self.project.rev + 1;
        working.edited = Some(now_ms());
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
            warnings,
        })
    }

    /// Records an external change (e.g. the file was edited by another
    /// process) as one undoable step.
    pub fn record_external(&mut self, project: Project) {
        let mut project = project;
        project.rev = project.rev.max(self.project.rev + 1);
        // The writer's time, or now for a change made by hand.
        if project.edited == self.project.edited {
            project.edited = Some(now_ms());
        }
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
                warnings: Vec::new(),
            });
        }
        // Revisions only grow, even when undoing back to an older state.
        project.rev = self.project.rev + 1;
        project.edited = Some(now_ms());
        self.project = project;
        self.history = history;
        Ok(Response {
            rev: self.project.rev,
            changes,
            result,
            dry_run: false,
            via: String::new(),
            warnings: Vec::new(),
        })
    }
}

/// Undo label for a request.
/// The undo history's name for an edit: the request in words.
fn label_of(request: &Request, project: &Project) -> String {
    let json = serde_json::to_value(request).unwrap_or_default();
    let method = json["method"]
        .as_str()
        .map_or_else(|| request.method(), String::from);
    crate::describe::describe(&method, &json["params"], Some(project))
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
        drop_frame: p.settings.counts_drop_frame(),
        end: p.media_frames(media),
        mark_in: media.marks.mark_in,
        mark_out: media.marks.mark_out,
        playhead,
    }
}

/// Source ranges of an edit. `tape` takes the Source Tape span (decision D17):
/// the tail of the first clip, the middle clips and the head of the last.
fn resolve_sources(p: &Project, params: &SourceParams) -> Result<Vec<Source>> {
    if params.tape {
        if !params.media.is_empty() || params.range.is_some() {
            return Err(Error::usage(
                "a tape-span edit takes the marked range; leave out media and range",
            ));
        }
        return tape_sources(p, params);
    }
    if params.media.is_empty() {
        return Err(Error::usage(
            "media is required (or pass tape: true to edit the Source Tape span)",
        ));
    }
    Ok(vec![source(p, params)?])
}

/// Both Source Tape marks, as source ranges in tape order (decision D17).
fn tape_sources(p: &Project, params: &SourceParams) -> Result<Vec<Source>> {
    let tm = &p.tape_marks;
    let (Some(mark_in), Some(mark_out)) = (tm.mark_in, tm.mark_out) else {
        return Err(Error::invalid("a Source Tape edit needs both In and Out"));
    };
    if tm.order.is_empty() {
        return Err(Error::invalid("Source Tape marks have no media order"));
    }
    let entries = crate::render::tape(p, &tm.order)?.1;
    let pieces = crate::render::span_pieces(&entries, mark_in, mark_out);
    if pieces.is_empty() {
        return Err(Error::invalid("Source Tape marks cover no media"));
    }
    Ok(pieces
        .into_iter()
        .map(|piece| Source {
            media: piece.media,
            range: piece.range,
            video_only: params.video_only,
            audio_only: params.audio_only,
            track: params.track.clone(),
        })
        .collect())
}

fn clip_ids(ids: &[String]) -> Value {
    let mut v = json!({ "clip": ids.first().cloned().unwrap_or_default() });
    if ids.len() > 1 {
        v["clips"] = json!(ids);
    }
    v
}

/// Source Tape marks count frames along a tape that held `media`: once it is
/// gone, or another length, they no longer say where they are (D17).
fn forget_tape_marks(p: &mut Project, media: &str) {
    if p.tape_marks.order.iter().any(|m| m == media) {
        p.tape_marks = Default::default();
    }
}

fn tape_marks_json(p: &Project, entries: &[crate::render::TapeEntry]) -> Value {
    let m = &p.tape_marks;
    let pieces = match crate::render::active_span(entries, m.mark_in, m.mark_out) {
        Some((a, b)) => crate::render::span_pieces(entries, a, b),
        None => Vec::new(),
    };
    json!({
        "in": m.mark_in,
        "out": m.mark_out,
        "order": m.order,
        "pieces": pieces.iter().map(|s| json!({
            "media": s.media, "in": s.range.0, "out": s.range.1,
        })).collect::<Vec<_>>(),
    })
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
/// `dry_run` only keeps it from writing files (an `.otioz` import unpacks
/// media).
fn mutate(
    p: &mut Project,
    request: &Request,
    env: &dyn Env,
    playhead: Option<i64>,
    dry_run: bool,
) -> Result<Value> {
    Ok(match request {
        Request::ProjectSet(set) => {
            project_set(p, set)?;
            Value::Null
        }
        Request::MediaAdd {
            paths,
            bin,
            sequence,
            fps,
        } => {
            if fps.is_some() && !sequence {
                return Err(Error::usage(
                    "fps is the frame rate of image sequences: add them with sequence",
                ));
            }
            let mut ids = Vec::new();
            for path in paths {
                let probed = match sequence {
                    true => env.probe_sequence(path, fps.unwrap_or(p.settings.fps))?,
                    false => env.probe(path)?,
                };
                // Several files of one sequence make one clip.
                let id = add_media(p, probed);
                if !ids.contains(&id) {
                    ids.push(id);
                }
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
            forget_tape_marks(p, media);
            Value::Null
        }
        Request::MediaRelink {
            media,
            path,
            search,
        } => relink(p, env, media.as_ref(), path.as_deref(), search, dry_run)?,
        Request::ProxyGenerate { media, width, auto } => {
            proxy_generate(p, env, media.as_ref(), *width, *auto, dry_run)?
        }
        Request::ProxyRemove { media } => proxy_remove(p, env, media.as_ref(), dry_run)?,
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
        Request::TapeMarksSet {
            mark_in,
            out,
            order,
        } => {
            if mark_in.is_none() && out.is_none() {
                return Err(Error::usage("tape.marks.set needs `in` and/or `out`"));
            }
            if order.is_empty() {
                return Err(Error::usage(
                    "tape.marks.set needs the tape's media, in order",
                ));
            }
            let entries = crate::render::tape(p, order)?.1;
            let end = entries.last().map(|e| e.start + e.frames).unwrap_or(0);
            let (cur_in, cur_out) = if p.tape_marks.order == *order {
                (p.tape_marks.mark_in, p.tape_marks.mark_out)
            } else {
                (None, None)
            };
            let ctx = TimeContext {
                fps: p.settings.fps,
                start_tc: 0,
                drop_frame: p.settings.counts_drop_frame(),
                end,
                mark_in: cur_in,
                mark_out: cur_out,
                playhead: None,
            };
            let i = mark_in.as_ref().map(|t| ctx.resolve(t)).transpose()?;
            let o = out.as_ref().map(|t| ctx.resolve(t)).transpose()?;
            ops::set_tape_marks(p, i, o, order, &entries)?;
            tape_marks_json(p, &entries)
        }
        Request::TapeMarksClear { media } => {
            ops::clear_tape_marks(p, media.as_deref())?;
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
            let sources = resolve_sources(p, src)?;
            clip_ids(&ops::append_block(p, &sources)?)
        }
        Request::EditSmartInsert {
            source: src,
            playhead: at,
        } => {
            let at = resolve(p, at, playhead)?;
            let sources = resolve_sources(p, src)?;
            let edit = ops::nearest_edit(p, at);
            let mut v = clip_ids(&ops::smart_insert_block(p, &sources, at)?);
            v["at"] = json!(edit);
            v
        }
        Request::EditInsert { source: src, at } => {
            let at = resolve(p, at, playhead)?;
            let sources = resolve_sources(p, src)?;
            clip_ids(&ops::insert_at_block(p, &sources, at)?)
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
            let sources = resolve_sources(p, src)?;
            let marked = *use_marks && (p.marks.mark_in.is_some() || p.marks.mark_out.is_some());
            let at = if marked {
                at.unwrap_or(0)
            } else {
                need(at, "at")?
            };
            let ids = without_marks(p, !use_marks, |p| ops::overwrite_block(p, &sources, at))?;
            clip_ids(&ids)
        }
        Request::EditPlaceOnTop {
            source: src,
            at,
            use_marks,
        } => {
            if src.track.is_some() {
                return Err(Error::usage(
                    "Place On Top picks its own track; leave out `track`",
                ));
            }
            let at = match at {
                Some(t) => Some(resolve(p, t, playhead)?),
                None => playhead,
            };
            let sources = resolve_sources(p, src)?;
            let marked = *use_marks && (p.marks.mark_in.is_some() || p.marks.mark_out.is_some());
            let at = if marked {
                at.unwrap_or(0)
            } else {
                need(at, "at")?
            };
            let (ids, track) =
                without_marks(p, !use_marks, |p| ops::place_on_top_block(p, &sources, at))?;
            let mut v = clip_ids(&ids);
            v["track"] = json!(track);
            v
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
            let sources = resolve_sources(p, src)?;
            let marked = *use_marks && p.marks.mark_in.is_some() && p.marks.mark_out.is_some();
            let at = if marked {
                at.unwrap_or(0)
            } else {
                need(at, "at")?
            };
            let ids = without_marks(p, !use_marks, |p| {
                ops::ripple_overwrite_block(p, &sources, at)
            })?;
            clip_ids(&ids)
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
        Request::EditSlide { clip, by } => {
            let id = resolve_clip(p, clip, playhead)?;
            let delta = length(p, by)?;
            ops::slide(p, &id, delta)?;
            Value::Null
        }
        Request::EditRippleDeleteSilence {
            clips,
            threshold_db,
            pre_head,
            post_tail,
            min,
        } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let settings = SilenceSettings {
                threshold_db: *threshold_db,
                pre_head: length(p, pre_head)?.max(0),
                post_tail: length(p, post_tail)?.max(0),
                min: length(p, min)?.max(1),
            };
            let shown = p.settings.clone();
            let tc = move |f: i64| shown.timecode(f);
            let mut parts = Vec::new();
            let mut removed = Vec::new();
            for id in &ids {
                let c = p.clip(id).expect("resolved");
                // A block plays as a gap, and a held frame is silent: no
                // sound to look at.
                if c.is_block() || c.speed() == 0.0 {
                    continue;
                }
                let found = silent_parts(p, env, &Target::Clip(c), &settings)?;
                for &(a, b) in &found {
                    removed.push(json!({
                        "clip": id, "start": a, "end": b,
                        "start_tc": tc(a), "end_tc": tc(b),
                    }));
                }
                if !found.is_empty() {
                    parts.push((id.clone(), found));
                }
            }
            let frames: i64 = parts
                .iter()
                .flat_map(|(_, spans)| spans.iter().map(|(a, b)| b - a))
                .sum();
            ops::remove_parts(p, &parts)?;
            json!({ "removed": removed, "frames": frames })
        }
        Request::EditDetectSceneCuts { clips, threshold } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let shown = p.settings.clone();
            let tc = move |f: i64| shown.timecode(f);
            let mut cuts = Vec::new();
            let mut created = Vec::new();
            for id in &ids {
                let c = p.clip(id).expect("resolved");
                // A block plays as a gap: no pictures to look at; a held
                // frame has no shots.
                if c.is_block() || c.speed() == 0.0 {
                    continue;
                }
                let found = scene_cuts(p, env, &Target::Clip(c), *threshold)?;
                cuts.extend(found.iter().map(
                    |&(f, score)| json!({ "clip": id, "frame": f, "tc": tc(f), "score": score }),
                ));
                let at: Vec<i64> = found.iter().map(|c| c.0).collect();
                created.extend(ops::split_clip(p, id, &at)?);
            }
            json!({ "cuts": cuts, "created": created })
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
                // "End": each clip runs to the end of its media.
                ops::extend_to_media_end(p, &ids, *ripple)?;
            } else {
                let frames = length(p, duration)?;
                ops::change_duration(p, &ids, frames, *ripple)?;
            }
            Value::Null
        }
        Request::EditChangeSpeed {
            clips,
            speed,
            ripple,
            at,
        } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let at = match at {
                Some(t) => Some(resolve(p, t, playhead)?),
                None => playhead,
            };
            ops::change_speed(p, &ids, *speed, *ripple, at)?;
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
            duration,
        } => {
            let at = resolve(p, at, playhead)?;
            let duration = duration.as_ref().map(|d| length(p, d)).transpose()?;
            let id = ops::add_marker(
                p,
                at,
                duration.unwrap_or(1),
                name.clone(),
                color.clone(),
                note.clone(),
            )?;
            json!({ "marker": id })
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
            duration,
        } => {
            let duration = duration.as_ref().map(|d| length(p, d)).transpose()?;
            ops::set_marker(
                p,
                marker,
                name.clone(),
                color.clone(),
                note.clone(),
                duration,
            )?;
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
            unpack,
            flatten,
        } => import_timeline(
            p,
            &Import {
                path: path.as_deref(),
                doc: otio.as_ref(),
                replace: *replace,
                search,
                unpack: unpack.as_deref(),
                write: !dry_run,
                flatten: *flatten,
            },
            env,
        )?,
        Request::EditDecompose { clips } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let (mut made, mut warnings) = (Vec::new(), Vec::new());
            for id in &ids {
                made.extend(decompose(p, id, env, (&[], false), &mut warnings)?);
            }
            fill_contents(p, &made, env, &[], &mut warnings);
            json!({ "clips": made, "warnings": warnings })
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
                    mutate(p, cmd, env, playhead, dry_run)
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
    // The same file is the same media, unless one is a still and the other
    // the sequence it starts.
    if let Some(existing) = p
        .media
        .iter()
        .find(|m| m.path == probed.path && m.sequence == probed.sequence)
    {
        return existing.id.clone();
    }
    // A still has no frame rate to give, and its size is a photo's.
    if p.settings.auto_format
        && let Some(v) = probed.info.video.as_ref().filter(|v| !v.still)
    {
        // The first video sets the project format, with no prompt (D37),
        // drop-frame counting too where its timecode counts so (D47).
        p.settings.set_fps(v.fps);
        let drop = v.fps.has_drop_frame() && probed.info.timecode.is_some_and(|t| t.drop_frame);
        p.settings
            .set_drop_frame(drop)
            .expect("the rate has drop-frame");
        adopt_size(p, v);
    }
    let id = p.alloc_media_id();
    let name = match &probed.sequence {
        Some(s) => s.display(),
        None => probed
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| id.clone()),
    };
    p.media.push(Media {
        id: id.clone(),
        path: probed.path,
        name,
        size: probed.size,
        mtime_ns: probed.mtime_ns,
        info: probed.info,
        marks: Default::default(),
        bin: String::new(),
        sequence: probed.sequence,
        proxy: None,
        fingerprint: probed.fingerprint,
    });
    id
}

/// `media.relink`: one item to `path`, or the items (all missing ones when
/// none are named) to their files found by name in `search`.
fn relink(
    p: &mut Project,
    env: &dyn Env,
    media: Option<&OneOrMany<String>>,
    path: Option<&Path>,
    search: &[PathBuf],
    dry_run: bool,
) -> Result<Value> {
    let ids = match media {
        Some(m) => m.to_vec(),
        None => p
            .media
            .iter()
            .filter(|m| !env.present(m))
            .map(|m| m.id.clone())
            .collect(),
    };
    let targets: Vec<Media> = ids
        .iter()
        .map(|id| {
            p.media(id)
                .cloned()
                .ok_or_else(|| Error::not_found(format!("no media {id}")))
        })
        .collect::<Result<_>>()?;
    // An image sequence is probed as one, at its rate.
    let probe = |m: &Media, path: &Path| match (&m.sequence, &m.info.video) {
        (Some(_), Some(v)) => env.probe_sequence(path, v.fps),
        _ => env.probe(path),
    };
    let (mut relinked, mut different, mut not_found) = (Vec::new(), Vec::new(), Vec::new());
    match (path, search.is_empty()) {
        (Some(path), true) => {
            let [m] = targets.as_slice() else {
                return Err(Error::usage(
                    "media.relink with a `path` takes one media id",
                ));
            };
            let probed = probe(m, path)?;
            let why = differs(m, &probed);
            relinked.push(json!({
                "media": m.id, "path": probed.path, "same": why.is_none(), "why": why,
            }));
            replace_file(p, env, &m.id, probed, why.is_none(), dry_run)?;
        }
        (None, false) => {
            let name = |m: &Media| m.path.file_name().map(OsStr::to_os_string);
            let names: HashSet<OsString> = targets.iter().filter_map(name).collect();
            let found = env.find_files(search, &names);
            for m in &targets {
                let mut candidates = name(m)
                    .and_then(|n| found.get(&n))
                    .cloned()
                    .unwrap_or_default();
                // Files named alike (a camera's C0001.MP4 on every card):
                // those in folders named like the old one's first.
                candidates.sort_by_key(|c| std::cmp::Reverse(common_tail(&m.path, c)));
                let mut first_difference = None;
                let mut matched = None;
                for c in candidates.iter().filter(|c| **c != m.path) {
                    let Ok(probed) = probe(m, c) else { continue };
                    match differs(m, &probed) {
                        None => {
                            matched = Some(probed);
                            break;
                        }
                        Some(why) => {
                            first_difference.get_or_insert((probed.path, why));
                        }
                    }
                }
                match (matched, first_difference) {
                    (Some(probed), _) => {
                        relinked.push(json!({ "media": m.id, "path": probed.path, "same": true }));
                        replace_file(p, env, &m.id, probed, true, dry_run)?;
                    }
                    (None, Some((path, why))) => {
                        different.push(json!({ "media": m.id, "path": path, "why": why }));
                    }
                    (None, None) => not_found.push(json!({ "media": m.id, "name": m.name })),
                }
            }
        }
        _ => {
            return Err(Error::usage(
                "media.relink takes either a `path` for one item or `search` folders",
            ));
        }
    }
    Ok(json!({ "relinked": relinked, "different": different, "not_found": not_found }))
}

/// Why the probed file isn't `m`'s file, if it isn't: another size,
/// other streams or another fingerprint.
fn differs(m: &Media, probed: &ProbedMedia) -> Option<String> {
    let (a, b) = (&m.info, &probed.info);
    match (&m.sequence, &probed.sequence) {
        (Some(s), Some(t)) if s.count != t.count => {
            return Some(format!("{} pictures, not {}", t.count, s.count));
        }
        (Some(_), None) | (None, Some(_)) => {
            return Some("an image sequence and a single file".into());
        }
        _ => {}
    }
    if probed.size != m.size {
        return Some(format!("{} bytes, not {}", probed.size, m.size));
    }
    match (&a.video, &b.video) {
        (Some(v), Some(w)) => {
            if (v.codec.as_str(), v.width, v.height) != (w.codec.as_str(), w.width, w.height) {
                return Some(format!(
                    "{} {}×{}, not {} {}×{}",
                    w.codec, w.width, w.height, v.codec, v.width, v.height
                ));
            }
            if v.fps != w.fps {
                return Some(format!("{} fps, not {}", w.fps, v.fps));
            }
        }
        (None, None) => {}
        _ => return Some("one has pictures, the other not".into()),
    }
    let sound = |i: &MediaInfo| -> Vec<(String, u32, u16)> {
        i.audio
            .iter()
            .map(|s| (s.codec.clone(), s.sample_rate, s.channels))
            .collect()
    };
    if sound(a) != sound(b) {
        return Some("other audio streams".into());
    }
    // The same file probes alike; half a frame (or a millisecond) allows
    // for another FFmpeg rounding otherwise.
    let slack = a.video.as_ref().map_or(0.001, |v| 0.5 / v.fps.as_f64());
    if (a.duration - b.duration).abs() > slack {
        return Some(format!("{:.3} s long, not {:.3}", b.duration, a.duration));
    }
    if a.timecode != b.timecode {
        return Some("another start timecode".into());
    }
    match (&m.fingerprint, &probed.fingerprint) {
        (Some(x), Some(y)) if x != y => Some("other content (fingerprint)".into()),
        _ => None,
    }
}

/// How many folder names `a` and `b` share at the end of their parents.
fn common_tail(a: &Path, b: &Path) -> usize {
    let dirs = |p: &Path| -> Vec<OsString> {
        p.parent()
            .into_iter()
            .flat_map(Path::components)
            .map(|c| c.as_os_str().to_os_string())
            .collect()
    };
    let (a, b) = (dirs(a), dirs(b));
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

/// Points `id` at the probed file. The proxy stays when it is the same
/// file (the proxy was made from it), else it goes.
fn replace_file(
    p: &mut Project,
    env: &dyn Env,
    id: &str,
    probed: ProbedMedia,
    same: bool,
    dry_run: bool,
) -> Result<()> {
    let old = p
        .media(id)
        .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
    let frames = p.media_frames(old);
    let m = p
        .media
        .iter_mut()
        .find(|m| m.id == id)
        .expect("found above");
    match m.proxy.as_mut() {
        Some(proxy) if same => {
            (proxy.source_size, proxy.source_mtime_ns) = (probed.size, probed.mtime_ns);
        }
        Some(_) => {
            let old = m.proxy.take().expect("a proxy");
            if !dry_run {
                env.remove_proxy_file(&old.path)?;
            }
        }
        None => {}
    }
    m.path = probed.path;
    m.size = probed.size;
    m.mtime_ns = probed.mtime_ns;
    m.info = probed.info;
    m.sequence = probed.sequence;
    m.fingerprint = probed.fingerprint;
    if p.media(id).map(|m| p.media_frames(m)) != Some(frames) {
        forget_tape_marks(p, id);
    }
    Ok(())
}

/// Takes the picture size of `v` as shown (square pixels, rotated, even)
/// and fixes the format.
fn adopt_size(p: &mut Project, v: &VideoInfo) {
    (p.settings.width, p.settings.height) = v.display_size();
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

/// What `timeline.import` reads, and how.
struct Import<'a> {
    path: Option<&'a Path>,
    doc: Option<&'a Value>,
    replace: bool,
    search: &'a [PathBuf],
    unpack: Option<&'a Path>,
    /// False in a dry run: a bundle's media aren't unpacked.
    write: bool,
    /// Compound clips decomposed into their clips.
    flatten: bool,
}

/// The pool's ids of an imported timeline's media, by [`media_key`].
type MediaIds = HashMap<(String, bool), String>;

/// How an imported timeline names a media file: an image sequence and a
/// still of its first picture are named alike.
fn media_key(m: &otio::MediaRef) -> (String, bool) {
    (m.url.clone(), m.sequence.is_some())
}

/// The media of imported timeline `t`, added to the pool: each file probed
/// once where the file says (or near it), else looked for by name in
/// `dirs` (and subfolders). By [`media_key`], and their ids in order.
/// `hint`: the error suggests search folders.
fn find_media(
    p: &mut Project,
    t: &otio::Imported,
    env: &dyn Env,
    dirs: &[PathBuf],
    hint: bool,
) -> Result<(MediaIds, Vec<String>)> {
    let mut ids = MediaIds::new();
    let mut media = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for c in t.clips.iter().filter(|c| !c.is_block()) {
        let m = &c.media;
        if ids.contains_key(&media_key(m)) || missing.contains(&m.shown.as_str()) {
            continue;
        }
        // An image sequence is probed as one, at its rate.
        let probe = |path: &Path| match m.sequence {
            Some(rate) => env.probe_sequence(path, rate),
            None => env.probe(path),
        };
        let probed = m
            .paths
            .iter()
            .find_map(|path| probe(path).ok())
            .or_else(|| {
                (!m.file_name.is_empty())
                    .then(|| {
                        env.find_file(dirs, std::ffi::OsStr::new(&m.file_name))
                            .iter()
                            .find_map(|path| probe(path).ok())
                    })
                    .flatten()
            });
        match probed {
            Some(probed) => {
                let id = add_media(p, probed);
                if !media.contains(&id) {
                    media.push(id.clone());
                }
                ids.insert(media_key(m), id);
            }
            None => missing.push(&m.shown),
        }
    }
    if !missing.is_empty() {
        return Err(Error::media(format!(
            "{} media file(s) not found: {}{}",
            missing.len(),
            missing.join(", "),
            if hint {
                "; pass `search` folders to look for them by name (subfolders too)"
            } else {
                ""
            }
        )));
    }
    Ok((ids, media))
}

/// Places the clips, transitions and markers of imported timeline `t` from
/// timeline frame `offset`, on the tracks `tracks` maps its own to (the
/// same ids otherwise), its media found as `ids`. The clips' and the
/// markers' ids.
fn place_imported(
    p: &mut Project,
    t: &otio::Imported,
    ids: &MediaIds,
    offset: i64,
    tracks: &HashMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<(Vec<String>, Vec<String>)> {
    let track_of = |id: &str| tracks.get(id).cloned().unwrap_or_else(|| id.to_string());
    let from = t.fps;
    let to = p.settings.fps;
    let conv = |f: i64| rescale(f, from, to);
    // Carried clip effects count from where their picture or sound starts
    // in the file; their times and fade lengths are frames at its rate.
    let carry = |list: &[Value], first: i64| {
        (!list.is_empty()).then(|| {
            let mut e = Effects {
                from: first,
                list: list.to_vec(),
            };
            if from != to {
                e.retime(|x| (x * to.as_f64() / from.as_f64()).round());
            }
            e
        })
    };
    // Where audio may start on each track without overlapping what is
    // before `offset`.
    let mut floors: HashMap<String, i64> = p
        .tracks
        .iter()
        .map(|t| {
            let end = t
                .clips
                .iter()
                .filter(|c| c.start < offset)
                .filter_map(|c| c.audio_range().map(|r| r.1))
                .max();
            (t.id.clone(), end.unwrap_or(0))
        })
        .collect();

    let mut clips = Vec::new();
    // The clips made, by key: (id, track).
    let mut made: HashMap<usize, (String, String)> = HashMap::new();
    for c in &t.clips {
        // A generator, title or compound clip: a block of its length, no
        // media.
        if c.is_block() {
            let (start, end) = (offset + conv(c.start), offset + conv(c.end()));
            if end - start < 1 {
                continue;
            }
            let source_in = conv(c.source_in);
            // Its length covers what it shows, whatever rounding did.
            let compound = c.compound.as_ref().map(|k| Compound {
                stack: k.stack.clone(),
                frames: conv(k.frames).max(source_in + end - start),
                tracks: Vec::new(),
            });
            let id = p.alloc_clip_id();
            clips.push(id.clone());
            made.insert(c.key, (id.clone(), track_of(&c.track)));
            let track = p.track_mut(&track_of(&c.track)).expect("made above");
            track.clips.push(Clip {
                id,
                media: String::new(),
                start,
                duration: end - start,
                source_in,
                video: c.video,
                audio: None,
                enabled: c.enabled,
                name: (!c.name.is_empty()).then(|| c.name.clone()),
                effects: carry(&c.effects, source_in),
                generator: c.generator.as_ref().map(|reference| Generator {
                    reference: reference.clone(),
                }),
                compound,
                speed: c.speed.clone(),
            });
            track.sort();
            continue;
        }
        let m = p.media(&ids[&media_key(&c.media)]).expect("added").clone();
        let frames = p.media_limit(&m);
        let label = if c.name.is_empty() { &m.name } else { &c.name };
        let mut start = offset + conv(c.start);
        let end = offset + conv(c.end());
        // A source range in the media's timecode (no available range said
        // where the media start) counts from the media's first frame here.
        let tc = match (c.timecoded, m.info.timecode) {
            (true, Some(tc)) if m.sequence.is_none() => tc.frames_at(from).round() as i64,
            _ => 0,
        };
        let mut source_in = conv(c.source_in - tc);
        let effects = carry(&c.effects, source_in);
        let sound_from = conv(c.source_in - tc + c.audio.map_or(0, |a| a.0));
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
                let floor = floors.get(&track_of(&c.track)).copied().unwrap_or(0);
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
                    gain_db: c.gain_db.clamp(*GAIN.start(), *GAIN.end()),
                    muted: c.muted,
                    effects: carry(&c.audio_effects, sound_from),
                })
            });
        if audio.is_some() && !GAIN.contains(&c.gain_db) {
            warnings.push(format!(
                "{label}: gain {:.1} dB set to the nearest of -60..24 dB",
                c.gain_db
            ));
        }
        if !c.video && audio.is_none() {
            warnings.push(format!("{label}: {} has no audio; left out", m.name));
            continue;
        }
        if let Some(a) = &audio {
            floors.insert(track_of(&c.track), start + duration + a.tail);
        }
        let name = (!c.name.is_empty() && c.name != m.name && c.name != c.media.file_name)
            .then(|| c.name.clone());
        let id = p.alloc_clip_id();
        clips.push(id.clone());
        made.insert(c.key, (id.clone(), track_of(&c.track)));
        let track = p.track_mut(&track_of(&c.track)).expect("made above");
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
            effects,
            generator: None,
            compound: None,
            speed: c.speed.clone(),
        });
        track.sort();
    }
    // Transitions between clips made, on the track of both; those that don't
    // fit their edit points here (media too short, a clip moved off it) go.
    for x in &t.transitions {
        let side = |k: Option<usize>| match k {
            None => Some(None),
            Some(k) => made.get(&k).map(Some),
        };
        let (Some(from), Some(to)) = (side(x.from), side(x.to)) else {
            continue;
        };
        let track = match (from, to) {
            (Some(f), Some(g)) if f.1 != g.1 => continue,
            (_, Some(g)) => &g.1,
            (Some(f), None) => &f.1,
            (None, None) => continue,
        };
        p.track_mut(track)
            .expect("made above")
            .transitions
            .push(Transition {
                sound: x.sound,
                from: from.map(|f| f.0.clone()),
                to: to.map(|g| g.0.clone()),
                before: conv(x.before),
                after: conv(x.after),
                otio: x.otio.clone(),
            });
    }
    let dropped = otio::transitions::prune(p);
    if dropped > 0 {
        warnings.push(otio::transitions::dropped(dropped));
    }
    let mut markers = Vec::new();
    for k in &t.markers {
        markers.push(ops::add_marker(
            p,
            offset + conv(k.at),
            conv(k.duration).max(1),
            k.name.clone(),
            Some(k.color.clone()),
            k.note.clone(),
        )?);
    }
    Ok((clips, markers))
}

/// The stack of compound `k` as an imported timeline of its video tracks,
/// pictures only (`video`), or of its audio tracks.
fn nested(k: &Compound, video: bool) -> Result<otio::Imported> {
    let kind = if video { "Video" } else { "Audio" };
    let tracks: Vec<Value> = k.stack["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["kind"] == kind)
        .cloned()
        .collect();
    let doc = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": k.stack["name"],
        "tracks": { "OTIO_SCHEMA": "Stack.1", "children": tracks },
    });
    let mut t = otio::import(&doc, None)?;
    if video {
        t.has_audio_tracks = true;
        for c in &mut t.clips {
            c.audio = None;
            c.audio_effects.clear();
        }
    }
    Ok(t)
}

/// What compound `k` (a picture block if `video`) plays (deviation X9): the
/// clips its stack holds placed as an import places them, on tracks of
/// their own, frames from its first, its media added to the pool, and
/// what compound clips among them play in turn (`depth` deep).
fn compound_contents(
    p: &mut Project,
    k: &Compound,
    video: bool,
    env: &dyn Env,
    dirs: &[PathBuf],
    depth: usize,
) -> Result<Vec<crate::model::Track>> {
    let t = nested(k, video)?;
    let (ids, _) = find_media(p, &t, env, dirs, false)?;
    let mut inner = Project::new(p.settings.clone());
    inner.media = p.media.clone();
    let mut needed: Vec<&str> = t.clips.iter().map(|c| c.track.as_str()).collect();
    needed.sort_by_key(|id| (!id.starts_with('V'), id[1..].parse::<usize>().unwrap_or(0)));
    needed.dedup();
    for id in needed {
        ops::ensure_track(&mut inner, id)?;
    }
    place_imported(&mut inner, &t, &ids, 0, &HashMap::new(), &mut Vec::new())?;
    if depth > 0 {
        for ti in 0..inner.tracks.len() {
            for ci in 0..inner.tracks[ti].clips.len() {
                let c = inner.tracks[ti].clips[ci].clone();
                if let Some(nk) = &c.compound {
                    let tracks =
                        compound_contents(p, nk, c.video, env, dirs, depth - 1).unwrap_or_default();
                    if let Some(nk) = inner.tracks[ti].clips[ci].compound.as_mut() {
                        nk.tracks = tracks;
                    }
                }
            }
        }
    }
    Ok(inner
        .tracks
        .into_iter()
        .filter(|t| !t.clips.is_empty())
        .collect())
}

/// Gives each compound clip among `ids` what it plays (deviation X9); one
/// whose clips aren't found plays as a gap, which `warnings` says.
fn fill_contents(
    p: &mut Project,
    ids: &[String],
    env: &dyn Env,
    dirs: &[PathBuf],
    warnings: &mut Vec<String>,
) {
    for id in ids {
        let Some((ti, ci)) = p.find_clip(id) else {
            continue;
        };
        let c = p.tracks[ti].clips[ci].clone();
        let Some(k) = &c.compound else {
            continue;
        };
        match compound_contents(p, k, c.video, env, dirs, 8) {
            Ok(tracks) => {
                if let Some(k) = p.tracks[ti].clips[ci].compound.as_mut() {
                    k.tracks = tracks;
                }
            }
            Err(e) => warnings.push(format!(
                "{}: its clips weren't found, so it plays as a gap: {}",
                c.name.as_deref().unwrap_or(id),
                e.message
            )),
        }
    }
}

/// Decompose in Place (deviation X9): compound clip `id` becomes the clips
/// its stack holds, at their times, trimmed to the part it shows, their
/// media looked for in `dirs` too. A picture block brings the stack's video
/// tracks, pictures only; a sound block its audio tracks, sound only. The
/// first goes in the block's place, each other on the next track of the
/// kind above that is free there, or a new one. The block's own effects
/// are dropped, which `warnings` says. `import`: a timeline import's own,
/// which a locked track it made doesn't stop. The clips made.
fn decompose(
    p: &mut Project,
    id: &str,
    env: &dyn Env,
    (dirs, import): (&[PathBuf], bool),
    warnings: &mut Vec<String>,
) -> Result<Vec<String>> {
    let (ti, ci) = p
        .find_clip(id)
        .ok_or_else(|| Error::not_found(format!("no clip {id}")))?;
    if p.tracks[ti].locked && !import {
        return Err(Error::invalid(format!(
            "track {} is locked",
            p.tracks[ti].id
        )));
    }
    let block = p.tracks[ti].clips[ci].clone();
    let k = block
        .compound
        .clone()
        .ok_or_else(|| Error::invalid(format!("{id} isn't a compound clip")))?;
    if block.speed.is_some() {
        return Err(Error::invalid(format!(
            "{id} has a speed change, so it can't be decomposed: its clips would have to play \
             at that speed"
        )));
    }
    let mut t = nested(&k, block.video)?;
    // What the block shows, in the stack's frames.
    let to = p.settings.fps;
    let a = rescale(block.source_in, to, t.fps);
    let b = rescale(block.source_in + block.duration, to, t.fps);
    t.clips.retain_mut(|c| {
        let (s, e) = (c.start.max(a), c.end().min(b));
        c.walk(s - c.start);
        (c.start, c.duration) = (s - a, e - s);
        e > s
    });
    t.markers.retain_mut(|m| {
        m.at -= a;
        (0..b - a).contains(&m.at)
    });
    let (ids, _) = find_media(p, &t, env, dirs, false)?;
    for w in &t.warnings {
        if !warnings.contains(w) {
            warnings.push(w.clone());
        }
    }
    // The block makes way; its tracks, the first in its place.
    p.tracks[ti].clips.remove(ci);
    let (start, end) = (block.start, block.end());
    let track_kind = p.tracks[ti].kind;
    let mut names: Vec<&str> = t.clips.iter().map(|c| c.track.as_str()).collect();
    names.sort_by_key(|n| n[1..].parse::<usize>().unwrap_or(0));
    names.dedup();
    let free = |t: &crate::model::Track| {
        !t.locked
            && t.clips.iter().all(|c| {
                let (s, e) = c.audio_range().map_or((c.start, c.end()), |(s, e)| {
                    (s.min(c.start), e.max(c.end()))
                });
                e <= start || s >= end
            })
    };
    let mut map = HashMap::new();
    let mut at = ti;
    for (n, name) in names.into_iter().enumerate() {
        let target = if n == 0 {
            p.tracks[ti].id.clone()
        } else {
            match (at + 1..p.tracks.len())
                .find(|&i| p.tracks[i].kind == track_kind && free(&p.tracks[i]))
            {
                Some(i) => {
                    at = i;
                    p.tracks[i].id.clone()
                }
                None => {
                    let made = ops::add_track(p, track_kind);
                    at = p.track_index(&made).expect("made");
                    made
                }
            }
        };
        map.insert(name.to_string(), target);
    }
    let (made, _) = place_imported(p, &t, &ids, start, &map, warnings)?;
    if let Some(e) = &block.effects {
        warnings.push(format!(
            "{id}'s own effects were dropped: {}",
            e.names().join(", ")
        ));
    }
    Ok(made)
}

/// `timeline.import`: an OpenTimelineIO timeline onto V1 ([`otio`]).
fn import_timeline(p: &mut Project, import: &Import, env: &dyn Env) -> Result<Value> {
    let Import {
        path,
        doc,
        replace,
        search,
        unpack,
        write,
        flatten,
    } = *import;
    let (doc, file) = match (path, doc) {
        (Some(path), None) => {
            let file = env.open_timeline(path, unpack, write)?;
            let doc = serde_json::from_str::<Value>(&file.text).map_err(|e| {
                Error::usage(format!("{}: not an OTIO (JSON) file: {e}", path.display()))
            })?;
            (doc, Some(file))
        }
        (None, Some(doc)) => (doc.clone(), None),
        _ => {
            return Err(Error::usage(
                "timeline.import needs exactly one of `path` and `otio`",
            ));
        }
    };
    let t = otio::import(&doc, file.as_ref().map(|f| f.base.as_path()))?;
    // Some editors write no timeline name: the file's then.
    let name = match (t.name.as_str(), path) {
        ("", Some(path)) => path
            .file_stem()
            .map_or(String::new(), |s| s.to_string_lossy().into_owned()),
        (name, _) => name.to_string(),
    };
    let bundle = file.as_ref().and_then(|f| f.bundle.clone());
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
    // A dry run doesn't unpack a bundle, so its media can't be probed: say
    // what the import would read.
    if let Some(b) = bundle.as_ref().filter(|b| !b.unpacked) {
        warnings.push(format!(
            "the media weren't probed: a dry run doesn't unpack the bundle's {} file(s) into {}",
            b.files,
            b.folder.display()
        ));
        return Ok(json!({
            "name": name,
            "fps": t.fps,
            "clips": [],
            "media": [],
            "markers": [],
            "clips_in_file": t.clips.len(),
            "bundle": b,
            "warnings": warnings,
        }));
    }

    // Media: each file probed once where the file says (or near it), else
    // looked for by name in `search` (and subfolders), and in a bundle's
    // media folder.
    let mut dirs = search.to_vec();
    if let Some(b) = &bundle {
        dirs.push(b.folder.clone());
    }
    let (ids, media) = find_media(p, &t, env, &dirs, search.is_empty())?;

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
                .filter(|c| !c.is_block())
                .find_map(|c| p.media(&ids[&media_key(&c.media)])?.info.video.clone())
        {
            adopt_size(p, &v);
        }
        // A file doesn't say whether its timecode counts drop-frame; its
        // media may (decision D47).
        let drop = t.clips.iter().filter(|c| !c.is_block()).any(|c| {
            p.media(&ids[&media_key(&c.media)])
                .and_then(|m| m.info.timecode)
                .is_some_and(|tc| tc.drop_frame)
        });
        p.settings.set_fps(t.fps);
        if drop && t.fps.has_drop_frame() {
            p.settings.drop_frame = true;
        }
        p.settings.start_tc = t.start_tc;
        p.settings.auto_format = false;
    } else if t.fps != p.settings.fps {
        warnings.push(format!(
            "converted from {} to the project's {} fps: cuts may move by up to half a frame",
            t.fps, p.settings.fps
        ));
    }
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
    let to = p.settings.fps;
    let conv = |f: i64| rescale(f, from, to);
    let (mut clips, markers) = place_imported(p, &t, &ids, offset, &HashMap::new(), &mut warnings)?;
    // Flattened: each compound clip made becomes its clips, and theirs.
    let compound = |p: &Project, id: &String| p.clip(id).is_some_and(|c| c.compound.is_some());
    let mut pending: Vec<String> = clips.iter().filter(|id| compound(p, id)).cloned().collect();
    while flatten && !pending.is_empty() {
        let mut next = Vec::new();
        for id in pending {
            if p.clip(&id).is_some_and(|c| c.speed.is_some()) {
                warnings.push(format!(
                    "{id} has a speed change: kept as a compound clip, as its clips would have \
                     to play at that speed"
                ));
                continue;
            }
            let made = decompose(p, &id, env, (&dirs, true), &mut warnings)?;
            clips.retain(|c| c != &id);
            next.extend(made.iter().filter(|m| compound(p, m)).cloned());
            clips.extend(made);
        }
        pending = next;
    }
    if flatten && !clips.iter().any(|id| compound(p, id)) {
        warnings.retain(|w| !w.starts_with(otio::COMPOUNDS_KEPT));
    }
    fill_contents(p, &clips, env, &dirs, &mut warnings);
    if empty {
        let (i, o) = (t.marks.mark_in.map(conv), t.marks.mark_out.map(conv));
        if !matches!((i, o), (Some(i), Some(o)) if i >= o) {
            p.marks = crate::model::Marks {
                mark_in: i.filter(|&i| i >= 0),
                mark_out: o.filter(|&o| o > 0),
            };
        }
    }
    let mut result = json!({
        "name": name,
        "fps": p.settings.fps,
        "clips": clips,
        "media": media,
        "markers": markers,
        "warnings": warnings,
    });
    if let Some(b) = bundle {
        result["bundle"] = json!(b);
    }
    Ok(result)
}

fn project_set(p: &mut Project, set: &ProjectSet) -> Result<()> {
    if let Some(fps) = set.fps {
        let has_clips = p.tracks.iter().any(|t| !t.clips.is_empty());
        if has_clips && fps != p.settings.fps {
            return Err(Error::invalid(
                "the frame rate can only change while the timeline is empty",
            ));
        }
        p.settings.set_fps(fps);
        p.settings.auto_format = false;
    }
    if let Some(w) = set.width {
        crate::model::check_side("width", w)?;
        p.settings.width = w;
        p.settings.auto_format = false;
    }
    if let Some(h) = set.height {
        crate::model::check_side("height", h)?;
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
    // Before the start timecode, which then counts so.
    if let Some(d) = set.drop_frame {
        p.settings.set_drop_frame(d)?;
    }
    if let Some(tc) = &set.start_tc {
        p.settings.set_start_tc(tc)?;
    }
    if let Some(d) = &set.still_duration {
        let c = TimeContext {
            start_tc: 0,
            ..p.time_context(None)
        };
        let secs = p.settings.fps.frames_to_secs(c.resolve(d)?);
        if !(secs > 0.0 && secs <= crate::model::STILL_LIMIT_SECS) {
            return Err(Error::invalid(
                "a still lasts at least a frame and at most 17 h 40 min",
            ));
        }
        p.settings.still_secs = secs;
    }
    if let Some(on) = set.use_proxies {
        p.settings.use_proxies = on;
    }
    Ok(())
}

/// Longest side for a proxy: the default, or an even size from 16 to 7680.
fn proxy_longest(width: Option<u32>, default: u32) -> Result<u32> {
    match width {
        None => Ok(default),
        Some(n) if (16..=7680).contains(&n) && n % 2 == 0 => Ok(n),
        Some(n) => Err(Error::invalid(format!(
            "proxy width {n} must be even, from 16 to 7680"
        ))),
    }
}

fn proxy_targets(p: &Project, media: Option<&OneOrMany<String>>) -> Result<Vec<String>> {
    let mut ids = match media {
        None => p
            .media
            .iter()
            .filter(|m| m.info.video.is_some() && !m.is_still())
            .map(|m| m.id.clone())
            .collect(),
        Some(listed) => {
            let ids = listed.to_vec();
            if ids.is_empty() {
                return Err(Error::usage(
                    "name the media to proxy, or omit them to proxy every video",
                ));
            }
            for id in &ids {
                let m = p
                    .media(id)
                    .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
                if m.info.video.is_none() {
                    return Err(Error::invalid(format!(
                        "{id} has no picture to make a proxy of"
                    )));
                }
                if m.is_still() {
                    return Err(Error::invalid(format!(
                        "{id} is a still; stills play from the original picture"
                    )));
                }
            }
            ids
        }
    };
    let mut seen = Vec::new();
    ids.retain(|id| {
        if seen.contains(id) {
            false
        } else {
            seen.push(id.clone());
            true
        }
    });
    Ok(ids)
}

/// `proxy.generate`: one H.264 file per video or image sequence.
fn proxy_generate(
    p: &mut Project,
    env: &dyn Env,
    media: Option<&OneOrMany<String>>,
    width: Option<u32>,
    auto: bool,
    dry_run: bool,
) -> Result<Value> {
    let rules = env.proxy_rules();
    let longest = proxy_longest(width, rules.size)?;
    let mut ids = proxy_targets(p, media)?;
    if auto {
        // Spec D41: what the rules pick and has no fresh proxy yet.
        ids.retain(|id| {
            p.media(id)
                .is_some_and(|m| rules.wants(m) && m.fresh_proxy().is_none())
        });
    }
    let dir = env.proxy_dir();
    let mut proxies = Vec::new();
    for id in ids {
        let item = p.media(&id).expect("checked").clone();
        let made = env.generate_proxy(&item, longest, &dir, dry_run)?;
        let entry = Proxy {
            path: made.path.clone(),
            source_size: item.size,
            source_mtime_ns: item.mtime_ns,
            width: made.width,
            height: made.height,
            codec: made.codec,
        };
        let m = p.media.iter_mut().find(|m| m.id == id).expect("checked");
        if let Some(old) = m.proxy.take()
            && old.path != entry.path
            && !dry_run
        {
            env.remove_proxy_file(&old.path)?;
        }
        m.proxy = Some(entry);
        let mut done = json!({
            "media": id,
            "path": made.path,
            "width": made.width,
            "height": made.height,
        });
        if let Some(how) = made.made_with {
            done["made_with"] = json!(how);
        }
        proxies.push(done);
    }
    Ok(json!({ "proxies": proxies }))
}

/// `proxy.remove`: delete the files and forget them.
fn proxy_remove(
    p: &mut Project,
    env: &dyn Env,
    media: Option<&OneOrMany<String>>,
    dry_run: bool,
) -> Result<Value> {
    let ids = match media {
        None => p
            .media
            .iter()
            .filter(|m| m.proxy.is_some())
            .map(|m| m.id.clone())
            .collect(),
        Some(listed) => {
            let ids = listed.to_vec();
            if ids.is_empty() {
                return Err(Error::usage(
                    "name the media whose proxies to remove, or omit them to remove every proxy",
                ));
            }
            for id in &ids {
                p.media(id)
                    .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
            }
            ids
        }
    };
    let mut cleared = Vec::new();
    for id in ids {
        let m = p.media.iter_mut().find(|m| m.id == id).expect("checked");
        let Some(old) = m.proxy.take() else {
            continue;
        };
        if !dry_run {
            env.remove_proxy_file(&old.path)?;
        }
        cleared.push(id);
    }
    Ok(json!({ "cleared": cleared }))
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
    let tc = |f: i64| p.settings.timecode(f);
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
            "tape_marks": p.tape_marks,
        }),
        Request::MediaList {} => {
            json!(
                p.media
                    .iter()
                    .map(|m| media_json(p, m, env))
                    .collect::<Vec<_>>()
            )
        }
        Request::MediaGet { media } => {
            let m = p
                .media(media)
                .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
            let mut v = media_json(p, m, env);
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
        Request::TrackList {} => json!(
            p.tracks
                .iter()
                .map(|t| {
                    let mut v = json!({
                        "id": t.id, "kind": t.kind, "clips": t.clips.len(),
                        "locked": t.locked, "muted": t.muted, "solo": t.solo,
                        "volume_db": t.volume_db,
                    });
                    if t.kind == crate::model::TrackKind::Video {
                        v["video_enabled"] = json!(t.video_enabled);
                    }
                    if let Some(n) = &t.name {
                        v["name"] = json!(n);
                    }
                    v
                })
                .collect::<Vec<_>>()
        ),
        Request::BinsList {} => json!(p.bins),
        Request::MarkersList {} => json!(
            p.markers
                .iter()
                .map(|k| {
                    let mut v = serde_json::to_value(k).unwrap_or_default();
                    v["at_tc"] = json!(tc(k.at));
                    v
                })
                .collect::<Vec<_>>()
        ),
        Request::MarksGet {} => {
            let mark = |f: Option<i64>| f.map(|f| json!({ "frame": f, "tc": tc(f) }));
            json!({ "in": mark(p.marks.mark_in), "out": mark(p.marks.mark_out) })
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
        Request::TimelineBoring { boring, jump } => {
            let (long, short) = analysis::boring(p, length(p, boring)?, length(p, jump)?);
            let item = |c: &Clip| {
                json!({
                    "clip": c.id, "start": c.start, "duration": c.duration,
                    "start_tc": tc(c.start), "duration_tc": p.settings.length_timecode(c.duration),
                })
            };
            json!({
                "boring": long.into_iter().map(item).collect::<Vec<_>>(),
                "jump": short.into_iter().map(item).collect::<Vec<_>>(),
            })
        }
        Request::AnalyzeSilence {
            media,
            clips,
            threshold_db,
            min,
        } => {
            let settings = SilenceSettings {
                threshold_db: *threshold_db,
                pre_head: 0,
                post_tail: 0,
                min: length(p, min)?.max(1),
            };
            let mut out = Vec::new();
            for target in targets(p, media, clips, playhead, true)? {
                let ranges = silent_parts(p, env, &target, &settings)?;
                out.push(target.json(p, json!({ "ranges": target.ranges(p, &ranges) })));
            }
            json!(out)
        }
        Request::AnalyzeScenes {
            media,
            clips,
            threshold,
        } => {
            let mut out = Vec::new();
            for target in targets(p, media, clips, playhead, false)? {
                let cuts: Vec<Value> = scene_cuts(p, env, &target, *threshold)?
                    .into_iter()
                    .map(|(f, score)| json!({ "frame": f, "tc": target.tc(p, f), "score": score }))
                    .collect();
                out.push(target.json(p, json!({ "cuts": cuts })));
            }
            json!(out)
        }
        Request::AnalyzeBlack {
            media,
            clips,
            share,
            min,
        } => {
            let min = length(p, min)?.max(1);
            let mut out = Vec::new();
            for target in targets(p, media, clips, playhead, false)? {
                let m = target.media(p)?;
                let stats = env.picture_stats(m)?;
                let ranges = analysis::black_parts(&target.span(p)?, &stats, *share, min);
                out.push(target.json(p, json!({ "ranges": target.ranges(p, &ranges) })));
            }
            json!(out)
        }
        Request::AnalyzeLoudness { range } => {
            let (a, b) = match range {
                Some(r) => ctx(p, playhead).range(r)?,
                None => match (p.marks.mark_in, p.marks.mark_out) {
                    (Some(a), Some(b)) => (a, b),
                    _ => (0, p.end()),
                },
            };
            if b <= a {
                return Err(Error::invalid("nothing to measure: the range is empty"));
            }
            let plan = crate::render::flatten(p, a, b)?;
            let l = env.loudness(&plan)?;
            json!({
                "integrated": finite(l.integrated), "range": l.range,
                "true_peak": finite(l.true_peak),
                "from": a, "to": b, "from_tc": tc(a), "to_tc": tc(b),
            })
        }
        Request::Schema { .. }
        | Request::Doctor {}
        | Request::ExportPlan(_)
        | Request::ExportStart(_)
        | Request::FrameRender { .. }
        | Request::StoryboardRender { .. }
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

/// JSON for a level that may be `-inf` (silence): `null` then.
fn finite(db: f64) -> Value {
    if db.is_finite() {
        json!(db)
    } else {
        Value::Null
    }
}

/// What an analysis looks at: a whole media item (positions are source
/// frames) or a clip (timeline frames).
enum Target<'a> {
    Media(&'a Media),
    Clip(&'a Clip),
}

impl Target<'_> {
    fn media<'p>(&self, p: &'p Project) -> Result<&'p Media> {
        let id = match self {
            Target::Media(m) => &m.id,
            Target::Clip(c) => &c.media,
        };
        p.media(id)
            .ok_or_else(|| Error::not_found(format!("no media {id}")))
    }

    fn span(&self, p: &Project) -> Result<Span> {
        let fps = p.settings.fps;
        Ok(match self {
            Target::Media(m) => {
                let (_, origin) = p.media_origin(&m.id)?;
                Span {
                    pace: Default::default(),
                    start: 0,
                    end: p.media_frames(m),
                    source_in: 0,
                    origin,
                    fps,
                }
            }
            Target::Clip(c) => {
                let (_, origin) = p.media_origin(&c.media)?;
                Span::of_clip(c, origin, fps)
            }
        })
    }

    /// Timecode of position `f`: source timecode for media, record for clips.
    fn tc(&self, p: &Project, f: i64) -> String {
        match self {
            Target::Media(_) => p.settings.length_timecode(f),
            Target::Clip(_) => p.settings.timecode(f),
        }
    }

    fn ranges(&self, p: &Project, ranges: &[(i64, i64)]) -> Vec<Value> {
        ranges
            .iter()
            .map(|&(a, b)| {
                json!({
                    "start": a, "end": b, "start_tc": self.tc(p, a), "end_tc": self.tc(p, b),
                    "duration_tc": p.settings.length_timecode(b - a),
                })
            })
            .collect()
    }

    /// `extra` with what this is.
    fn json(&self, p: &Project, mut extra: Value) -> Value {
        match self {
            Target::Media(m) => extra["media"] = json!(m.id),
            Target::Clip(c) => {
                extra["clip"] = json!(c.id);
                let track = p
                    .tracks
                    .iter()
                    .find(|t| t.clips.iter().any(|x| x.id == c.id));
                extra["track"] = json!(track.map(|t| t.id.clone()));
            }
        }
        extra
    }
}

/// The media or clips an analysis request names, or every clip with sound
/// (`audio`) or pictures.
fn targets<'a>(
    p: &'a Project,
    media: &Option<String>,
    clips: &Option<OneOrMany<ClipRef>>,
    playhead: Option<i64>,
    audio: bool,
) -> Result<Vec<Target<'a>>> {
    match (media, clips) {
        (Some(_), Some(_)) => Err(Error::usage("give `media` or `clips`, not both")),
        (Some(id), None) => {
            let m = p
                .media(id)
                .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
            Ok(vec![Target::Media(m)])
        }
        (None, Some(refs)) => resolve_clips(p, &refs.to_vec(), playhead)?
            .iter()
            .map(|id| {
                let c = p.clip(id).expect("resolved");
                if c.is_block() {
                    return Err(Error::invalid(format!(
                        "{id} is a block from another editor: it has no media"
                    )));
                }
                Ok(Target::Clip(c))
            })
            .collect(),
        (None, None) => Ok(p
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| !c.is_block())
            .filter(|c| if audio { c.audio.is_some() } else { c.video })
            .map(Target::Clip)
            .collect()),
    }
}

/// Ripple Delete Silence's parts of `target`.
fn silent_parts(
    p: &Project,
    env: &dyn Env,
    target: &Target<'_>,
    settings: &SilenceSettings,
) -> Result<Vec<(i64, i64)>> {
    let m = target.media(p)?;
    let (stream, gain) = match target {
        Target::Media(_) => (0, 0.0),
        Target::Clip(c) => match &c.audio {
            Some(a) => (a.stream, a.gain_db),
            None => {
                return Err(Error::invalid(format!(
                    "{} has no sound to find silence in",
                    c.id
                )));
            }
        },
    };
    if m.info.audio.is_empty() {
        return Err(Error::invalid(format!("{} has no sound", m.id)));
    }
    let levels = env.audio_levels(m, stream)?;
    Ok(analysis::silent_parts(
        &target.span(p)?,
        &levels,
        gain,
        settings,
    ))
}

/// Detect Scene Cuts' frames in `target`, with their scores.
fn scene_cuts(
    p: &Project,
    env: &dyn Env,
    target: &Target<'_>,
    threshold: f64,
) -> Result<Vec<(i64, f32)>> {
    let m = target.media(p)?;
    if m.info.video.is_none() || matches!(target, Target::Clip(c) if !c.video) {
        return Err(Error::invalid(format!(
            "{} has no picture to find shots in",
            match target {
                Target::Media(m) => &m.id,
                Target::Clip(c) => &c.id,
            }
        )));
    }
    let stats = env.picture_stats(m)?;
    Ok(analysis::scene_cuts(&target.span(p)?, &stats, threshold))
}

fn media_json(p: &Project, m: &Media, env: &dyn Env) -> Value {
    let frames = p.media_frames(m);
    let mut v = json!({
        "id": m.id,
        "name": m.name,
        "path": m.path,
        "frames": frames,
        "duration_tc": p.settings.length_timecode(frames),
        "video": m.info.video.as_ref().map(|v| {
            let (width, height) = v.display_size();
            json!({ "width": width, "height": height, "fps": v.fps.to_string(), "codec": v.codec,
                    "rotation": v.rotation, "fields": v.fields, "hdr": v.hdr })
        }),
        "audio_streams": m.info.audio.len(),
    });
    if !env.present(m) {
        v["missing"] = json!(true);
    }
    if !m.marks.is_empty() {
        v["marks"] = json!(m.marks);
    }
    if !m.bin.is_empty() {
        v["bin"] = json!(m.bin);
    }
    // A still lasts as long as clips make it; `frames` is the still duration.
    if m.is_still() {
        v["still"] = json!(true);
        v["video"]["fps"] = Value::Null;
    }
    if let Some(s) = &m.sequence {
        v["sequence"] = json!({
            "pattern": s.pattern, "first": s.first, "count": s.count,
        });
    }
    if let Some(px) = &m.proxy {
        v["proxy"] = json!({
            "path": px.path,
            "width": px.width,
            "height": px.height,
            "codec": px.codec,
            "stale": m.fresh_proxy().is_none(),
        });
    }
    // The file's own start timecode, as `HH:MM:SS:FF` in its frames (or,
    // for sound with a time of day, at the project rate).
    if let Some(tc) = m.info.timecode {
        let fps = match &m.info.video {
            Some(v) if v.fps == tc.rate => v.fps,
            _ => p.settings.fps,
        };
        let frames = tc.frames_at(fps).round() as i64;
        v["timecode"] = json!(format_timecode(frames, fps, 0, tc.drop_frame));
    }
    v
}

/// A clip as reported by timeline queries.
pub fn clip_json(p: &Project, c: &Clip, track: &str) -> Value {
    let mut v = json!({
        "type": "clip",
        "id": c.id,
        "track": track,
        "media": c.media,
        "name": c.name.clone().or_else(|| p.media(&c.media).map(|m| m.name.clone())),
        "start": c.start,
        "end": c.end(),
        "duration": c.duration,
        "start_tc": p.settings.timecode(c.start),
        "end_tc": p.settings.timecode(c.end()),
        "source_in": c.source_in,
        // Where it would show next (decision D48).
        "source_out": c.source_at(c.end()),
        "source_in_tc": p.settings.length_timecode(c.source_in),
        "enabled": c.enabled,
    });
    // A block from another editor (deviation X9), by kind.
    if let Some(g) = &c.generator {
        v["generator"] = json!(otio::generator_kind(g));
    }
    if let Some(k) = &c.compound {
        v["compound"] = json!(otio::compound_kind(k));
    }
    // A speed change: how fast (2.0, -1.0 reversed), else what it is.
    if let Some(speed) = &c.speed {
        v["speed"] = speed
            .scalar()
            .map_or_else(|| json!(speed.label()), |x| json!(x));
    }
    // Carried clip effects by name (deviation X9); `project.get` has them
    // whole.
    if let Some(e) = &c.effects {
        v["effects"] = json!(e.names());
    }
    if let Some(a) = &c.audio {
        v["audio"] = json!({ "stream": a.stream, "head": a.head, "tail": a.tail, "gain_db": a.gain_db, "muted": a.muted });
        if let Some(e) = &a.effects {
            v["audio"]["effects"] = json!(e.names());
        }
    }
    v
}

fn timeline_json(p: &Project, a: i64, b: i64) -> Value {
    let fps = p.settings.fps;
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
                        "start_tc": p.settings.timecode(gs), "end_tc": p.settings.timecode(ge) }));
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
            // Carried transitions (deviation X9), at their cuts.
            let transitions: Vec<Value> = t
                .transitions
                .iter()
                .filter_map(|x| {
                    let at = otio::transitions::place(p, t, x)?;
                    (at >= a && at <= b).then(|| {
                        json!({ "name": otio::transitions::name(x), "at": at, "before": x.before,
                            "after": x.after, "from": x.from, "to": x.to, "sound": x.sound })
                    })
                })
                .collect();
            if !transitions.is_empty() {
                v["transitions"] = json!(transitions);
            }
            v
        })
        .collect();
    json!({
        "fps": fps.to_string(),
        "start": a,
        "end": b,
        "duration": p.end(),
        "duration_tc": p.settings.timecode(p.end()),
        "tracks": tracks,
        "marks": p.marks,
        "markers": p.markers,
    })
}

/// A fixed rate for tests and tools that need one.
pub fn rate(num: i64, den: i64) -> Rational {
    Rational::new(num, den).expect("valid rate")
}
