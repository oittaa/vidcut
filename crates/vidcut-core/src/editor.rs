use crate::validate::GAIN;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::analysis::{
    self, AudioLevels, Loudness, PictureStats, SilenceSettings, Span, SpeechSettings, VoiceActivity,
};
use crate::change::Change;
use crate::error::{Error, ErrorCode, Result};
use crate::folder::Folder;
use crate::history::HistoryFile;
use crate::model::{
    Clip, ClipAudio, Compound, Effects, Format, Generator, ImageSequence, Kept, Marks, Media,
    MediaInfo, Project, Proxy, Track, TrackKind, Transition, VideoInfo,
};
use crate::ops::{self, ClipProps, Edge, Source};
use crate::otio;
use crate::render::RenderPlan;
use crate::request::{
    Changes, Client, ClientKind, ClipRef, Detect, Envelope, ImportMode, NewTrack, OneOrMany,
    ProjectSet, Request, Response, SourceParams,
};
use crate::steps::{Branch, Mark, Step, Steps, StepsCache, Tree};
use crate::time::{DurationSpec, Rational, TimeContext, TimeSpec, format_timecode};
use crate::transcript::{self, Speech};
use crate::validate;

#[derive(Debug, Clone, PartialEq)]
pub struct ProbedMedia {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i64,
    pub info: MediaInfo,
    pub sequence: Option<ImageSequence>,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedProxy {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub codec: String,
    pub made_with: Option<String>,
    pub copies: Option<Copied>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Copied {
    pub down: u64,
    pub down_bytes: u64,
    pub up: u64,
    pub up_bytes: u64,
}

pub trait Env {
    fn probe(&self, path: &Path) -> Result<ProbedMedia>;

    fn probe_sequence(&self, path: &Path, rate: Rational) -> Result<ProbedMedia>;

    fn find_file(&self, dirs: &[PathBuf], name: &OsStr) -> Vec<PathBuf> {
        let names = HashSet::from([name.to_os_string()]);
        self.find_files(dirs, &names)
            .remove(name)
            .unwrap_or_default()
    }

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

    fn read_folder(&self, dir: &Path) -> Result<Folder> {
        crate::folder::read(dir)
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::not_found(format!("{}: {e}", path.display())),
            _ => Error::usage(format!("{}: {e}", path.display())),
        })
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let name = path
            .file_name()
            .map_or_else(|| "export".into(), |n| n.to_string_lossy().into_owned());
        let part = path.with_file_name(format!(".{name}.part"));
        std::fs::write(&part, bytes)
            .and_then(|()| std::fs::rename(&part, path))
            .map_err(|e| {
                let _ = std::fs::remove_file(&part);
                Error::new(ErrorCode::Export, format!("{}: {e}", path.display()))
            })
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn present(&self, m: &Media) -> bool {
        !crate::store::from_another_system(&m.path) && m.path.is_file()
    }

    fn proxy_present(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn audio_levels(&self, media: &Media, stream: u32) -> Result<Arc<AudioLevels>>;

    fn picture_stats(&self, media: &Media) -> Result<Arc<PictureStats>>;

    fn loudness(&self, plan: &RenderPlan) -> Result<Loudness>;

    fn transcribe(&self, plan: &RenderPlan, asked: &transcript::Asked) -> Result<Arc<Speech>> {
        let _ = (plan, asked);
        Err(Error::usage("speech is transcribed by the vidcut program"))
    }

    fn voice_activity(&self, media: &Media, stream: u32) -> Result<Arc<VoiceActivity>> {
        let _ = (media, stream);
        Err(Error::usage("speech is detected by the vidcut program"))
    }

    fn proxy_dir(&self) -> PathBuf;

    fn generate_proxy(
        &self,
        media: &Media,
        longest: u32,
        codec: Option<&str>,
        dir: &Path,
        dry_run: bool,
    ) -> Result<GeneratedProxy>;

    fn proxy_rules(&self) -> crate::preferences::Proxies {
        crate::preferences::Proxies::default()
    }

    fn remove_proxy_file(&self, path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::media(format!("{}: {e}", path.display()))),
        }
    }

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

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineFile {
    pub text: String,
    pub base: PathBuf,
    pub bundle: Option<Unpacked>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Unpacked {
    pub folder: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub unpacked: bool,
}

pub fn is_otioz(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("otioz"))
}

pub fn is_timeline_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| {
        ["otio", "otioz", "otiod"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}

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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub step: Step,
    pub change: Change,
}

impl Entry {
    pub fn new(label: impl Into<String>, change: Change) -> Self {
        Self {
            step: Step::labeled(label),
            change,
        }
    }
}

impl crate::steps::Node for Entry {
    fn id(&self) -> Option<u64> {
        self.step.id
    }

    fn placed(&self) -> Option<u64> {
        self.step.placed()
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct History {
    pub undo: Vec<Entry>,
    pub redo: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<Branch<Entry>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
}

impl History {
    pub(crate) fn of(mut tree: Tree<Entry>) -> Self {
        let mut redo = tree.line.split_off(tree.done.min(tree.line.len()));
        redo.reverse();
        Self {
            undo: tree.line,
            redo,
            branches: tree.branches,
            marks: tree.marks,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Labels {
    pub undo: Vec<String>,
    pub redo: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Moved {
    Step(Box<Entry>),
    Undo(u32),
    Redo(u32),
    Mark(Mark),
    Unmark(u64),
}

pub const HISTORY_CAP: usize = 1 << 17;

pub const EXTERNAL: &str = "Changed outside vidcut";

fn over(len: usize) -> usize {
    if len > HISTORY_CAP {
        len - (HISTORY_CAP - HISTORY_CAP / 10)
    } else {
        0
    }
}

pub struct Editor {
    project: Project,
    history: History,
    moved: Vec<Moved>,
    pub(crate) file: Option<(HistoryFile, String)>,
    opened: Option<u64>,
    blocked: Option<(String, String)>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Editor {
    pub fn new(project: Project, mut history: History) -> Self {
        history.undo.drain(..over(history.undo.len()));
        Self {
            project,
            history,
            moved: Vec::new(),
            file: None,
            opened: None,
            blocked: None,
        }
    }

    pub(crate) fn unreadable(project: Project, hash: String, why: String) -> Self {
        Self {
            blocked: Some((hash, why)),
            ..Self::new(project, History::default())
        }
    }

    pub fn history_trouble(&self) -> Option<&str> {
        self.blocked.as_ref().map(|(_, why)| why.as_str())
    }

    pub(crate) fn blocked_on(&self) -> Option<&str> {
        self.blocked.as_ref().map(|(hash, _)| hash.as_str())
    }

    pub(crate) fn stands_on(&self) -> Option<&str> {
        match (&self.file, &self.blocked) {
            (Some((_, hash)), _) | (None, Some((hash, _))) => Some(hash),
            (None, None) => None,
        }
    }

    pub(crate) fn block(&mut self, why: String) {
        if let Some((_, hash)) = self.file.take() {
            for m in &self.moved {
                if let Moved::Step(e) = m {
                    self.history.undo.push((**e).clone());
                }
            }
            self.blocked = Some((hash, why));
        } else if let Some((_, was)) = &mut self.blocked {
            *was = why;
        }
    }

    pub fn opened_at(&mut self, ms: u64) {
        self.opened = Some(ms);
    }

    pub(crate) fn on_file(project: Project, file: HistoryFile, hash: String) -> Self {
        Self {
            file: Some((file, hash)),
            ..Self::new(project, History::default())
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub(crate) fn project_mut(&mut self) -> &mut Project {
        &mut self.project
    }

    pub(crate) fn held(&self) -> &History {
        &self.history
    }

    pub(crate) fn take_moved(&mut self) -> Vec<Moved> {
        std::mem::take(&mut self.moved)
    }

    pub(crate) fn saved_to(&mut self, file: HistoryFile, hash: String) {
        self.history = History::default();
        self.file = Some((file, hash));
        self.blocked = None;
    }

    pub fn history_counts(&self) -> (usize, usize) {
        let Some((file, _)) = &self.file else {
            return (self.history.undo.len(), self.history.redo.len());
        };
        let (mut u, mut r) = file.tail().map_or((0, 0), |(t, _)| (t.undo, t.redo));
        for m in &self.moved {
            match m {
                Moved::Step(_) => (u, r) = (u + 1, 0),
                Moved::Undo(n) => {
                    let n = (*n as usize).min(u);
                    (u, r) = (u - n, r + n);
                }
                Moved::Redo(n) => {
                    let n = (*n as usize).min(r);
                    (u, r) = (u + n, r - n);
                }
                Moved::Mark(_) | Moved::Unmark(_) => {}
            }
        }
        (u, r)
    }

    pub fn pending(&self) -> bool {
        !self.moved.is_empty()
    }

    pub fn unsaved(&self) -> (u64, usize) {
        (self.project.rev, self.moved.len())
    }

    pub fn on_its_file(&self) -> bool {
        self.file.is_some() && self.moved.is_empty()
    }

    pub fn history_view(&self, n: usize) -> Result<History> {
        let (undo, redo) = self.view(n, HistoryFile::top, Entry::clone)?;
        Ok(History {
            undo,
            redo,
            ..History::default()
        })
    }

    pub fn history_labels(&self, n: usize) -> Result<Labels> {
        let (undo, redo) = self.view(n, HistoryFile::top_labels, |e| e.step.label.clone())?;
        Ok(Labels { undo, redo })
    }

    pub fn history_steps(&self, cache: &mut StepsCache) -> Result<Steps> {
        let Some((file, _)) = &self.file else {
            let mut steps = Steps::default();
            for e in &self.history.undo {
                steps.place(e.step.clone());
            }
            let done = steps.done;
            for e in self.history.redo.iter().rev() {
                steps.place(e.step.clone());
            }
            steps.done = done;
            for m in &self.history.marks {
                steps.mark(m.clone());
            }
            return Ok(steps);
        };
        let unreadable = |e: std::io::Error| {
            Error::internal(format!(
                "the undo history {} can't be read: {e}",
                file.path().display()
            ))
        };
        let mut steps = match file.steps_now(cache).map_err(unreadable)? {
            true => cache.steps.clone(),
            false => Steps::default(),
        };
        for m in &self.moved {
            match m {
                Moved::Step(e) => steps.place(e.step.clone()),
                Moved::Undo(n) => steps.back(*n as usize),
                Moved::Redo(n) => steps.forward(*n as usize),
                Moved::Mark(m) => steps.mark(m.clone()),
                Moved::Unmark(id) => steps.unmark(*id),
            }
        }
        Ok(steps)
    }

    pub fn project_at(&self, step: usize) -> Result<Project> {
        let (u, _) = self.history_counts();
        self.project_between(self.project.clone(), u, step)
    }

    pub fn project_between(&self, start: Project, from: usize, to: usize) -> Result<Project> {
        let (u, r) = self.history_counts();
        if from.max(to) > u + r {
            return Err(Error::invalid(format!(
                "no step {}: the undo history has {}",
                from.max(to),
                u + r
            )));
        }
        let (lo, hi) = (from.min(to), from.max(to));
        if lo < u && hi > u {
            return Err(Error::internal(
                "a step on the other side of the current one",
            ));
        }
        let entries = self.entries_between(lo, hi, u)?;
        let fits = |e: String| Error::internal(format!("the undo history doesn't fit: {e}"));
        let mut project = start;
        if to < from {
            for e in entries.iter().rev() {
                undo_step(&mut project, &e.change, false).map_err(fits)?;
            }
        } else {
            for e in &entries {
                undo_step(&mut project, &e.change, true).map_err(fits)?;
            }
        }
        Ok(project)
    }

    fn entries_between(&self, lo: usize, hi: usize, u: usize) -> Result<Vec<Entry>> {
        if lo == hi {
            return Ok(Vec::new());
        }
        if let Some((file, _)) = &self.file
            && self.moved.is_empty()
        {
            let unreadable = |e: std::io::Error| {
                Error::internal(format!(
                    "the undo history {} can't be read: {e}",
                    file.path().display()
                ))
            };
            let Some((tail, end)) = file.read_tail().map_err(unreadable)? else {
                return Err(Error::internal("the undo history is gone"));
            };
            return file.between(&tail, end, lo, hi).map_err(unreadable);
        }
        if hi <= u {
            let h = self.history_view(u - lo)?;
            return Ok(h.undo.into_iter().take(hi - lo).collect());
        }
        let mut redo = self.history_view(hi - u)?.redo;
        redo.reverse();
        Ok(redo.into_iter().skip(lo - u).collect())
    }

    fn view<T: Clone>(
        &self,
        n: usize,
        top: impl Fn(
            &HistoryFile,
            &crate::history::Tail,
            u64,
            usize,
            usize,
        ) -> std::io::Result<(Vec<T>, Vec<T>)>,
        of: impl Fn(&Entry) -> T,
    ) -> Result<(Vec<T>, Vec<T>)> {
        let newest = |v: &[T]| v[v.len().saturating_sub(n)..].to_vec();
        let Some((file, _)) = &self.file else {
            let all = |v: &[Entry]| v.iter().map(&of).collect::<Vec<_>>();
            return Ok((
                newest(&all(&self.history.undo)),
                newest(&all(&self.history.redo)),
            ));
        };
        let unreadable = |e: std::io::Error| {
            Error::internal(format!(
                "the undo history {} can't be read: {e}",
                file.path().display()
            ))
        };
        let k = n + self.moved.len();
        let (mut undo, mut redo) = match file.read_tail().map_err(unreadable)? {
            Some((tail, end)) => {
                let (mut undo, mut redo) = top(file, &tail, end, k, k).map_err(unreadable)?;
                undo.reverse();
                redo.reverse();
                (undo, redo)
            }
            None => (Vec::new(), Vec::new()),
        };
        for m in &self.moved {
            match m {
                Moved::Step(e) => {
                    undo.push(of(e));
                    redo.clear();
                }
                Moved::Undo(n) => shift(*n, &mut undo, &mut redo),
                Moved::Redo(n) => shift(*n, &mut redo, &mut undo),
                Moved::Mark(_) | Moved::Unmark(_) => {}
            }
        }
        Ok((newest(&undo), newest(&redo)))
    }

    fn push(&mut self, mut entry: Entry) {
        if self.file.is_none() && entry.step.id.is_none() {
            let next = self
                .history
                .undo
                .iter()
                .chain(&self.history.redo)
                .filter_map(|e| e.step.id)
                .max()
                .unwrap_or(0)
                + 1;
            entry.step.id = Some(next);
        }
        self.moved.push(Moved::Step(Box::new(entry.clone())));
        if self.file.is_none() {
            self.history.undo.push(entry);
            self.history.redo.clear();
            let over = over(self.history.undo.len());
            self.history.undo.drain(..over);
        }
    }

    pub fn apply(
        &mut self,
        envelope: Envelope,
        env: &dyn Env,
        playhead: Option<i64>,
    ) -> Result<Response> {
        envelope.check_rev(self.project.rev)?;
        let Envelope {
            request,
            dry_run,
            client,
            ..
        } = envelope;
        if let Request::HistoryGet {
            from,
            limit,
            since,
            branch,
        } = request
        {
            let steps = self.history_steps(&mut StepsCache::default())?;
            let mut result = crate::steps::listing(&steps, from, limit, &self.project);
            match (since, branch) {
                (Some(step), Some(id)) => {
                    let k = step
                        .checked_sub(1)
                        .ok_or_else(|| Error::invalid("a version's steps count from 1"))?;
                    let then = crate::past::Past::default().version_at(self, id, k)?;
                    result["since"] =
                        crate::steps::since_version(&then, &self.project, id).json(step);
                }
                (Some(step), None) => {
                    let then = self.project_at(step)?;
                    result["since"] =
                        crate::steps::since_step(&then, &self.project, step, steps.done).json(step);
                }
                (None, Some(_)) => {
                    return Err(Error::usage("`branch` goes with `since`"));
                }
                (None, None) => {}
            }
            return Ok(Response {
                rev: self.project.rev,
                changes: Changes::default(),
                result,
                dry_run: false,
                via: String::new(),
                warnings: self
                    .history_trouble()
                    .map(String::from)
                    .into_iter()
                    .collect(),
            });
        }
        if !request.is_mutation() {
            let result = query(
                &self.project,
                &Labels::default(),
                &request,
                env,
                playhead,
                dry_run,
            )?;
            return Ok(Response {
                rev: self.project.rev,
                changes: Changes::default(),
                result,
                dry_run: dry_run && matches!(request, Request::SubtitlesExport { .. }),
                via: String::new(),
                warnings: Vec::new(),
            });
        }
        match request {
            Request::HistoryUndo { steps } => return self.step_history(steps, true, dry_run),
            Request::HistoryRedo { steps } => return self.step_history(steps, false, dry_run),
            Request::HistoryGo { step } => return self.go_to(step, dry_run),
            Request::HistoryMark { step, name } => return self.mark(step, Some(name), dry_run),
            Request::HistoryUnmark { step } => return self.mark(step, None, dry_run),
            Request::HistoryRestore { branch } => return self.restore(branch, dry_run),
            Request::HistoryRevert { step } => return self.revert(step, client, dry_run),
            _ => {}
        }
        let step = Step {
            label: label_of(&request, &self.project, playhead),
            kind: Some(request.method()),
            by: client,
            parts: match &request {
                Request::Apply { commands, .. } => commands
                    .iter()
                    .filter(|c| c.is_mutation())
                    .map(|c| label_of(c, &self.project, playhead))
                    .collect(),
                _ => Vec::new(),
            },
            ..Step::default()
        };
        let mut working = self.project.clone();
        let result = mutate(&mut working, &request, env, playhead, dry_run)?;
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
        self.commit(working, step, result, warnings, dry_run)
    }

    fn commit(
        &mut self,
        mut working: Project,
        step: Step,
        result: Value,
        warnings: Vec<String>,
        dry_run: bool,
    ) -> Result<Response> {
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
        let (clips, spans) = crate::steps::places(&self.project, &working);
        let Some(change) = changed(&mut self.project, &mut working) else {
            return Ok(Response {
                rev: self.project.rev,
                changes,
                result,
                dry_run: false,
                via: String::new(),
                warnings: Vec::new(),
            });
        };
        let now = now_ms();
        working.rev = self.project.rev + 1;
        working.edited = Some(now);
        self.project = working;
        self.push(Entry {
            step: Step {
                time: Some(now),
                opened: self.opened,
                clips,
                spans,
                ..step
            },
            change,
        });
        Ok(Response {
            rev: self.project.rev,
            changes,
            result,
            dry_run: false,
            via: String::new(),
            warnings,
        })
    }

    fn step_id(steps: &Steps, at: usize) -> Result<u64> {
        if at == 0 {
            return Ok(0);
        }
        let s = steps.line.get(at - 1).ok_or_else(|| {
            Error::invalid(format!(
                "no step {at}: the undo history has {}",
                steps.line.len()
            ))
        })?;
        s.id.ok_or_else(|| {
            Error::invalid(format!(
                "step {at} was made before steps could be named; name a later one"
            ))
        })
    }

    fn mark(
        &mut self,
        step: Option<usize>,
        name: Option<String>,
        dry_run: bool,
    ) -> Result<Response> {
        let steps = self.history_steps(&mut StepsCache::default())?;
        let at = step.unwrap_or(steps.done);
        let id = Self::step_id(&steps, at)?;
        let name = name.map(|n| n.trim().to_string());
        if name.as_deref() == Some("") {
            return Err(Error::usage("a version needs a name"));
        }
        let was = steps.named(id).map(String::from);
        if name.is_none() && was.is_none() {
            return Err(Error::invalid(format!("step {at} has no name")));
        }
        if !dry_run {
            let moved = match &name {
                Some(n) => Moved::Mark(Mark {
                    step: id,
                    name: n.clone(),
                    time: Some(now_ms()),
                }),
                None => Moved::Unmark(id),
            };
            if self.file.is_none() {
                self.history.marks.retain(|m| m.step != id);
                if let Moved::Mark(m) = &moved {
                    self.history.marks.push(m.clone());
                }
            }
            self.moved.push(moved);
        }
        Ok(Response {
            rev: self.project.rev,
            changes: Changes::default(),
            result: json!({ "step": at, "name": name, "was": was }),
            dry_run,
            via: String::new(),
            warnings: Vec::new(),
        })
    }

    fn restore(&mut self, id: u64, dry_run: bool) -> Result<Response> {
        if self.file.is_none() {
            return Err(Error::invalid(
                "this project's undo history isn't saved yet, so it keeps no versions",
            ));
        }
        let chain = self.chain(id).map_err(|e| match e.code {
            ErrorCode::NotFound => Error::not_found(format!(
                "no version {id} to restore: history.get lists them in branches"
            )),
            _ => e,
        })?;
        let after = chain[0].0;
        let (u, _) = self.history_counts();
        let (mut project, back) = match after.cmp(&u) {
            std::cmp::Ordering::Less => (
                self.stepped((u - after) as u32, true)?.0,
                Some(Moved::Undo((u - after) as u32)),
            ),
            std::cmp::Ordering::Greater => (
                self.stepped((after - u) as u32, false)?.0,
                Some(Moved::Redo((after - u) as u32)),
            ),
            std::cmp::Ordering::Equal => (self.project.clone(), None),
        };
        let mut moved: Vec<Moved> = back.into_iter().collect();
        let at = climb(&chain, &mut project, usize::MAX, &mut moved)?;
        let labels: Vec<&str> = chain
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|e| e.step.label.as_str()))
            .collect();
        let changes = diff(&self.project, &project);
        let result = json!({ "restored": labels, "at": at });
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
        let now = now_ms();
        project.rev = self.project.rev + 1;
        project.edited = Some(now);
        self.project = project;
        self.moved.extend(moved.into_iter().map(|m| match m {
            Moved::Step(mut e) => {
                e.step.restored = Some(now);
                Moved::Step(e)
            }
            m => m,
        }));
        Ok(Response {
            rev: self.project.rev,
            changes,
            result,
            dry_run: false,
            via: String::new(),
            warnings: Vec::new(),
        })
    }

    pub fn chain(&self, id: u64) -> Result<Chain> {
        let missing = || {
            Error::not_found(format!(
                "no version {id}: history.get lists them in branches"
            ))
        };
        let Some((file, _)) = &self.file else {
            return Err(missing());
        };
        match file.tail() {
            Some((_, end)) => file.chain(end, id).map_err(|e| {
                Error::internal(format!(
                    "the undo history {} can't be read: {e}",
                    file.path().display()
                ))
            })?,
            None => None,
        }
        .ok_or_else(missing)
    }

    fn revert(&mut self, step: usize, client: Option<Client>, dry_run: bool) -> Result<Response> {
        let (u, _) = self.history_counts();
        if u == 0 {
            return Err(Error::invalid("there are no steps to revert"));
        }
        if step == 0 || step > u {
            return Err(Error::invalid(format!(
                "step {step} isn't done: only steps 1 to {u} can be reverted"
            )));
        }
        let view = self.history_view(u - step + 1)?;
        let (target, later) = view
            .undo
            .split_first()
            .ok_or_else(|| Error::internal("the undo history is shorter than it says"))?;
        let ours = |c: &Change| -> Vec<Vec<String>> {
            c.touched()
                .into_iter()
                .filter(|p| {
                    !matches!(
                        p.first().map(String::as_str),
                        Some("next_id" | "edited" | "rev")
                    )
                })
                .collect()
        };
        let mine = ours(&target.change);
        for (j, e) in later.iter().enumerate() {
            if let Some(path) = crate::change::overlap(&mine, &ours(&e.change)) {
                return Err(Error::conflict(format!(
                    "can't revert step {step}: step {} (\u{201c}{}\u{201d}) changed {} since",
                    step + 1 + j,
                    e.step.label,
                    what(&self.project, path)
                )));
            }
        }
        let mut working = self.project.clone();
        if let Err(e) = undo_step(&mut working, &target.change, false) {
            let why = match self.in_the_way(&target.change, later) {
                Some(j) => format!(
                    "step {} (\u{201c}{}\u{201d}) changed what is around it since",
                    step + 1 + j,
                    later[j].step.label
                ),
                None => e,
            };
            return Err(Error::conflict(format!("can't revert step {step}: {why}")));
        }
        working.next_id = self.project.next_id.clone();
        let mut warnings = Vec::new();
        let dropped = otio::transitions::prune(&mut working);
        if dropped > 0 {
            warnings.push(otio::transitions::dropped(dropped));
        }
        let problems = validate::violations(&working);
        if !problems.is_empty() {
            return Err(Error::conflict(format!(
                "can't revert step {step}: the project would be broken: {}",
                problems.join("; ")
            )));
        }
        let label = format!("Revert \u{201c}{}\u{201d}", target.step.label);
        let result = json!({ "reverted": step, "label": target.step.label });
        self.commit(
            working,
            Step {
                label,
                kind: Some("history.revert".into()),
                by: client,
                ..Step::default()
            },
            result,
            warnings,
            dry_run,
        )
    }

    fn in_the_way(&self, change: &Change, later: &[Entry]) -> Option<usize> {
        let mut then = self.project.clone();
        for (j, e) in later.iter().enumerate().rev() {
            undo_step(&mut then, &e.change, false).ok()?;
            if undo_step(&mut then.clone(), change, false).is_ok() {
                return Some(j);
            }
        }
        None
    }

    pub fn record_external(&mut self, project: Project) {
        let mut project = project;
        let (clips, spans) = crate::steps::places(&self.project, &project);
        let kind = Some("file".to_string());
        let change = changed(&mut self.project, &mut project)
            .unwrap_or_else(|| Change::Fields(Default::default()));
        let now = now_ms();
        project.rev = project.rev.max(self.project.rev + 1);
        if project.edited == self.project.edited {
            project.edited = Some(now);
        }
        self.project = project;
        self.push(Entry {
            step: Step {
                label: EXTERNAL.into(),
                kind,
                by: Some(Client::new(ClientKind::File, None)),
                time: Some(now),
                opened: self.opened,
                clips,
                spans,
                ..Step::default()
            },
            change,
        });
    }

    fn go_to(&mut self, step: usize, dry_run: bool) -> Result<Response> {
        let (u, r) = self.history_counts();
        match step {
            s if s > u + r => Err(Error::invalid(format!(
                "no step {step}: the undo history has {}",
                u + r
            ))),
            s if s < u => self.step_history((u - s) as u32, true, dry_run),
            s if s > u => self.step_history((s - u) as u32, false, dry_run),
            _ => Ok(Response {
                rev: self.project.rev,
                changes: Changes::default(),
                result: json!({ "at": u }),
                dry_run,
                via: String::new(),
                warnings: Vec::new(),
            }),
        }
    }

    fn stepped(&self, steps: u32, undo: bool) -> Result<(Project, History, Vec<Step>)> {
        let (u, r) = self.history_counts();
        let available = if undo { u } else { r };
        if available < steps as usize || steps == 0 {
            return Err(Error::invalid(format!(
                "cannot {} {steps} step(s): {available} available",
                if undo { "undo" } else { "redo" }
            )));
        }
        let mut project = self.project.clone();
        let mut json = cfg!(debug_assertions).then(|| to_json(&self.project));
        let fits = |e: String| Error::internal(format!("the undo history doesn't fit: {e}"));
        let mut history = match self.file {
            None => self.history.clone(),
            Some(_) => self.history_view(steps as usize)?,
        };
        let mut moved = Vec::new();
        for _ in 0..steps {
            let (from, to) = if undo {
                (&mut history.undo, &mut history.redo)
            } else {
                (&mut history.redo, &mut history.undo)
            };
            let entry = from.pop().expect("checked");
            undo_step(&mut project, &entry.change, !undo).map_err(fits)?;
            if let Some(json) = json.as_mut() {
                entry.change.apply(json, !undo).map_err(fits)?;
            }
            moved.push(entry.step.clone());
            to.push(entry);
        }
        debug_assert!(
            json.is_none_or(|j| serde_json::from_value::<Project>(j).is_ok_and(|q| q == project)),
            "the undo step makes another project as JSON"
        );
        Ok((project, history, moved))
    }

    fn step_history(&mut self, steps: u32, undo: bool, dry_run: bool) -> Result<Response> {
        let (mut project, history, moved) = self.stepped(steps, undo)?;
        let changes = diff(&self.project, &project);
        let (u, _) = self.history_counts();
        let at = if undo {
            u - steps as usize
        } else {
            u + steps as usize
        };
        let labels: Vec<&str> = moved.iter().map(|s| s.label.as_str()).collect();
        let (clips, spans) = crate::steps::union(&moved);
        let result = json!({
            if undo { "undone" } else { "redone" }: labels,
            "at": at,
            "clips": clips,
            "spans": spans,
        });
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
        project.rev = self.project.rev + 1;
        project.edited = Some(now_ms());
        self.project = project;
        if self.file.is_none() {
            self.history = history;
        }
        self.moved.push(if undo {
            Moved::Undo(steps)
        } else {
            Moved::Redo(steps)
        });
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

fn what(p: &Project, path: &[String]) -> String {
    for part in path.iter().rev() {
        if let Some(c) = p.clip(part) {
            let name = c.name.clone().filter(|n| !n.is_empty());
            return name
                .or_else(|| p.media(&c.media).map(|m| m.name.clone()))
                .unwrap_or_else(|| part.clone());
        }
        if let Some(m) = p.media(part) {
            return m.name.clone();
        }
        if p.markers.iter().any(|k| &k.id == part) || p.track(part).is_some() {
            return part.clone();
        }
        if let Some(c) = p.cue(part) {
            return format!(
                "\"{}\"",
                crate::subtitles::plain(&c.text).replace('\n', " ")
            );
        }
        if let Some(t) = p.subtitle_track(part) {
            return t.display_name();
        }
    }
    path.last().cloned().unwrap_or_default()
}

fn shift<T>(n: u32, from: &mut Vec<T>, to: &mut Vec<T>) {
    for _ in 0..n {
        let Some(e) = from.pop() else { return };
        to.push(e);
    }
}

fn to_json(p: &Project) -> Value {
    serde_json::to_value(p).expect("a project is JSON")
}

fn changed(before: &mut Project, after: &mut Project) -> Option<Change> {
    let change = project_change(before, after);
    debug_assert_eq!(change, Change::between(&to_json(before), &to_json(after)));
    change
}

fn project_change(a: &mut Project, b: &mut Project) -> Option<Change> {
    use std::mem::take;
    let (mut media, mut tracks, mut markers) = (
        (take(&mut a.media), take(&mut b.media)),
        (take(&mut a.tracks), take(&mut b.tracks)),
        (take(&mut a.markers), take(&mut b.markers)),
    );
    let rest = Change::between(&to_json(a), &to_json(b));
    let lists = [
        (
            "media",
            Change::of_items(&mut media.0, &mut media.1, |m| &m.id, leaf),
        ),
        (
            "tracks",
            Change::of_items(&mut tracks.0, &mut tracks.1, |t| &t.id, track_change),
        ),
        (
            "markers",
            Change::of_items(
                &mut markers.0,
                &mut markers.1,
                |k| &k.id,
                |a, b| moved_only(a, b, |k| &mut k.at, "at"),
            ),
        ),
    ];
    (a.media, b.media) = media;
    (a.tracks, b.tracks) = tracks;
    (a.markers, b.markers) = markers;
    joined(rest, lists)
}

fn track_change(a: &mut Track, b: &mut Track) -> Option<Change> {
    use std::mem::take;
    if a == b {
        return None;
    }
    let (mut clips, mut transitions) = (
        (take(&mut a.clips), take(&mut b.clips)),
        (take(&mut a.transitions), take(&mut b.transitions)),
    );
    let rest = Change::between(&to_value(&*a), &to_value(&*b));
    let transitions_change = match (transitions.0.is_empty(), transitions.1.is_empty()) {
        (true, true) => None,
        (false, true) => Some(Change::Set {
            before: Some(to_value(&transitions.0)),
            after: None,
        }),
        (true, false) => Some(Change::Set {
            before: None,
            after: Some(to_value(&transitions.1)),
        }),
        (false, false) => Change::of_items(&mut transitions.0, &mut transitions.1, |x| &x.id, leaf),
    };
    let lists = [
        (
            "clips",
            Change::of_items(
                &mut clips.0,
                &mut clips.1,
                |c| &c.id,
                |a, b| moved_only(a, b, |c| &mut c.start, "start"),
            ),
        ),
        ("transitions", transitions_change),
    ];
    (a.clips, b.clips) = clips;
    (a.transitions, b.transitions) = transitions;
    joined(rest, lists)
}

pub type Chain = Vec<(usize, Vec<Entry>)>;

pub(crate) fn climb(
    chain: &[(usize, Vec<Entry>)],
    project: &mut Project,
    last: usize,
    moved: &mut Vec<Moved>,
) -> Result<usize> {
    let fits = |e: String| Error::internal(format!("the undo history doesn't fit: {e}"));
    let mut at = chain.first().map_or(0, |c| c.0);
    let mut placed: &[Entry] = &[];
    for (k, (inner, entries)) in chain.iter().enumerate() {
        if k > 0 {
            for e in placed[*inner..].iter().rev() {
                undo_step(project, &e.change, false).map_err(fits)?;
            }
            if placed.len() > *inner {
                moved.push(Moved::Undo((placed.len() - inner) as u32));
            }
            at += inner;
        }
        let take = match k + 1 == chain.len() {
            true => last.min(entries.len()),
            false => entries.len(),
        };
        for e in &entries[..take] {
            undo_step(project, &e.change, true).map_err(fits)?;
        }
        moved.extend(
            entries[..take]
                .iter()
                .map(|e| Moved::Step(Box::new(e.clone()))),
        );
        placed = &entries[..take];
    }
    Ok(at + placed.len())
}

fn undo_step(p: &mut Project, change: &Change, forward: bool) -> std::result::Result<(), String> {
    use crate::change::{apply_json, shift_json};
    let Change::Fields(fields) = change else {
        return apply_json(p, change, forward);
    };
    let (lists, rest) = split_fields(fields, &["media", "tracks", "markers"]);
    if !rest.is_empty() {
        use std::mem::take;
        let kept = (
            take(&mut p.media),
            take(&mut p.tracks),
            take(&mut p.markers),
        );
        apply_json(p, &Change::Fields(rest), forward)?;
        (p.media, p.tracks, p.markers) = kept;
    }
    for (name, c) in lists {
        match name {
            "media" => {
                c.apply_to_items(&mut p.media, forward, |m| &m.id, apply_json, shift_json)?
            }
            "tracks" => {
                c.apply_to_items(&mut p.tracks, forward, |t| &t.id, track_step, shift_json)?
            }
            _ => c.apply_to_items(
                &mut p.markers,
                forward,
                |k| &k.id,
                apply_json,
                |k, field, by| {
                    match field {
                        "at" => k.at += by,
                        _ => shift_json(k, field, by)?,
                    }
                    Ok(())
                },
            )?,
        }
    }
    Ok(())
}

fn track_step(t: &mut Track, change: &Change, forward: bool) -> std::result::Result<(), String> {
    use crate::change::{apply_json, shift_json};
    let Change::Fields(fields) = change else {
        return apply_json(t, change, forward);
    };
    let (lists, rest) = split_fields(fields, &["clips", "transitions"]);
    if !rest.is_empty() {
        use std::mem::take;
        let kept = (take(&mut t.clips), take(&mut t.transitions));
        apply_json(t, &Change::Fields(rest), forward)?;
        (t.clips, t.transitions) = kept;
    }
    for (name, c) in lists {
        match name {
            "clips" => c.apply_to_items(
                &mut t.clips,
                forward,
                |c| &c.id,
                apply_json,
                |c, field, by| {
                    match field {
                        "start" => c.start += by,
                        _ => shift_json(c, field, by)?,
                    }
                    Ok(())
                },
            )?,
            _ => c.apply_to_items(
                &mut t.transitions,
                forward,
                |x| &x.id,
                apply_json,
                shift_json,
            )?,
        }
    }
    Ok(())
}

fn split_fields<'a>(
    fields: &'a std::collections::BTreeMap<String, Change>,
    lists: &[&'a str],
) -> (
    Vec<(&'a str, &'a Change)>,
    std::collections::BTreeMap<String, Change>,
) {
    let mut named = Vec::new();
    let mut rest = std::collections::BTreeMap::new();
    for (k, c) in fields {
        match lists.iter().find(|l| **l == k.as_str()) {
            Some(l) => named.push((*l, c)),
            None => {
                rest.insert(k.clone(), c.clone());
            }
        }
    }
    (named, rest)
}

fn moved_only<T: serde::Serialize + PartialEq>(
    a: &mut T,
    b: &mut T,
    at: impl Fn(&mut T) -> &mut i64,
    field: &str,
) -> Option<Change> {
    let (was, is) = (*at(a), *at(b));
    if was != is {
        *at(a) = is;
        let only = a == b;
        *at(a) = was;
        if only {
            let set = Change::Set {
                before: Some(was.into()),
                after: Some(is.into()),
            };
            return Some(Change::Fields([(field.to_string(), set)].into()));
        }
    }
    leaf(a, b)
}

fn leaf<T: serde::Serialize + PartialEq>(a: &mut T, b: &mut T) -> Option<Change> {
    if a == b {
        return None;
    }
    Change::between(&to_value(&*a), &to_value(&*b))
}

fn joined<const N: usize>(
    rest: Option<Change>,
    lists: [(&str, Option<Change>); N],
) -> Option<Change> {
    let mut fields = match rest {
        Some(Change::Fields(fields)) => fields,
        None => Default::default(),
        Some(_) => unreachable!("an object's change is its fields'"),
    };
    for (name, change) in lists {
        if let Some(c) = change {
            fields.insert(name.to_string(), c);
        }
    }
    (!fields.is_empty()).then_some(Change::Fields(fields))
}

fn to_value<T: serde::Serialize + ?Sized>(x: &T) -> Value {
    serde_json::to_value(x).expect("a project is JSON")
}

fn label_of(request: &Request, project: &Project, playhead: Option<i64>) -> String {
    if let Request::Apply {
        commands,
        label: None,
    } = request
    {
        let edits: Vec<&Request> = commands.iter().filter(|c| c.is_mutation()).collect();
        match edits.as_slice() {
            [one] => return label_of(one, project, playhead),
            [first, rest @ ..] => {
                return format!(
                    "{} and {} more",
                    label_of(first, project, playhead),
                    rest.len()
                );
            }
            [] => {}
        }
    }
    let json = serde_json::to_value(request).unwrap_or_default();
    let method = json["method"]
        .as_str()
        .map_or_else(|| request.method(), String::from);
    crate::describe::describe_at(&method, &json["params"], Some(project), playhead)
}

fn diff(before: &Project, after: &Project) -> Changes {
    #[derive(PartialEq)]
    enum Named<'a> {
        Media(&'a Media),
        Clip(&'a str, &'a Clip),
        Marker(&'a crate::model::Marker),
        Cue(&'a str, &'a crate::model::Cue),
    }
    fn index(p: &Project) -> HashMap<&str, Named<'_>> {
        let mut m = HashMap::new();
        for media in &p.media {
            m.insert(media.id.as_str(), Named::Media(media));
        }
        for t in &p.tracks {
            for c in &t.clips {
                m.insert(c.id.as_str(), Named::Clip(&t.id, c));
            }
        }
        for k in &p.markers {
            m.insert(k.id.as_str(), Named::Marker(k));
        }
        for t in &p.subtitles {
            for c in &t.cues {
                m.insert(c.id.as_str(), Named::Cue(&t.id, c));
            }
        }
        m
    }
    let (a, b) = (index(before), index(after));
    let mut c = Changes::default();
    for (id, v) in &b {
        match a.get(id) {
            None => c.created.push(id.to_string()),
            Some(old) if old != v => c.modified.push(id.to_string()),
            _ => {}
        }
    }
    for id in a.keys() {
        if !b.contains_key(id) {
            c.deleted.push(id.to_string());
        }
    }
    let key = |s: &String| (s.chars().next(), s[1..].parse::<u64>().unwrap_or(0));
    c.created.sort_by_key(key);
    c.modified.sort_by_key(key);
    c.deleted.sort_by_key(key);
    c
}

fn transcribed_range(
    p: &Project,
    range: Option<&crate::time::RangeSpec>,
    playhead: Option<i64>,
) -> Result<(i64, i64)> {
    let (a, b) = match range {
        Some(r) => ctx(p, playhead).range(r)?,
        None => match (p.marks.mark_in, p.marks.mark_out) {
            (Some(a), Some(b)) => (a, b),
            _ => (0, p.end()),
        },
    };
    if b <= a {
        return Err(Error::invalid("nothing to transcribe: the range is empty"));
    }
    Ok((a, b))
}

fn sound_plan(p: &Project, a: i64, b: i64) -> Result<RenderPlan> {
    let mut q = p.clone();
    q.settings.sample_rate = transcript::RATE;
    q.settings.channels = 1;
    crate::render::flatten(&q, a, b)
}

fn ctx(p: &Project, playhead: Option<i64>) -> TimeContext {
    p.time_context(playhead)
}

fn mark_duration(
    d: &DurationSpec,
    c: &TimeContext,
    mark_in: Option<i64>,
    out: Option<i64>,
    marks: &Marks,
) -> Result<(Option<i64>, Option<i64>)> {
    if out.is_some() {
        return Err(Error::usage("give `out` or `duration`, not both"));
    }
    let shown = marks.mark_out.unwrap_or(c.end) - marks.mark_in.unwrap_or(0);
    let length = d.frames(c, shown)?;
    if length < 1 {
        return Err(Error::invalid(format!(
            "a duration of {length} frames: it must be at least one frame"
        )));
    }
    let start = mark_in.unwrap_or(0);
    Ok((Some(start), Some(start + length)))
}

fn resolve(p: &Project, t: &TimeSpec, playhead: Option<i64>) -> Result<i64> {
    Ok(ctx(p, playhead).resolve(t)?)
}

fn length(p: &Project, t: &TimeSpec) -> Result<i64> {
    Ok(ctx(p, None).duration(t)?)
}

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

pub fn resolve_cue(p: &Project, r: &str, playhead: Option<i64>) -> Result<String> {
    if let Some((track, at)) = r.split_once('@') {
        let t = p
            .subtitle_track(track)
            .ok_or_else(|| Error::not_found(format!("no subtitle track {track}")))?;
        let frame = resolve(p, &TimeSpec::parse(at)?, playhead)?;
        let ci = ops::subtitles::cue_at(t, frame)
            .ok_or_else(|| Error::not_found(format!("no cue on {track} at frame {frame}")))?;
        return Ok(t.cues[ci].id.clone());
    }
    p.cue(r)
        .map(|c| c.id.clone())
        .ok_or_else(|| Error::not_found(format!("no cue {r}")))
}

fn resolve_cues(p: &Project, refs: &[String], playhead: Option<i64>) -> Result<Vec<String>> {
    refs.iter().map(|r| resolve_cue(p, r, playhead)).collect()
}

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
            let mut files = PoolFiles::of(p);
            for path in paths {
                let probed = match sequence {
                    true => env.probe_sequence(path, fps.unwrap_or(p.settings.fps))?,
                    false => env.probe(path)?,
                };
                let id = add_media(p, &mut files, probed);
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            if let Some(bin) = bin {
                ops::move_media(p, &ids, bin)?;
            }
            json!({ "media": ids })
        }
        Request::MediaAddFolder {
            path,
            bin,
            sequence,
            fps,
        } => {
            if fps.is_some() && !sequence {
                return Err(Error::usage(
                    "fps is the frame rate of image sequences: add them with sequence",
                ));
            }
            let parent = bin.clone().unwrap_or_default();
            if !ops::has_bin(p, &parent) {
                return Err(Error::not_found(format!(
                    "no bin {parent:?}; add it with bins.add"
                )));
            }
            let folder = env.read_folder(path)?;
            let name = folder.name().trim().to_string();
            if name.is_empty() {
                return Err(Error::usage(format!(
                    "{} has no name to give its bin",
                    path.display()
                )));
            }
            let top = match parent.is_empty() {
                true => name,
                false => format!("{parent}/{name}"),
            };
            let mut out = FolderImport {
                sequence: *sequence,
                fps: *fps,
                ..FolderImport::default()
            };
            let mut files = PoolFiles::of(p);
            add_folder(p, env, &mut files, &folder, &top, &mut out)?;
            if out.media.is_empty() {
                return Err(Error::media(no_media(path, &out.skipped)));
            }
            json!({ "bin": top, "media": out.media, "skipped": out.skipped })
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
            duration,
        } => {
            let m = p
                .media(media)
                .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
            let c = media_context(p, m, None);
            let mut i = mark_in.as_ref().map(|t| c.resolve(t)).transpose()?;
            let mut o = out.as_ref().map(|t| c.resolve(t)).transpose()?;
            if let Some(d) = duration {
                (i, o) = mark_duration(d, &c, i.or(m.marks.mark_in), o, &m.marks)?;
            }
            if i.is_none() && o.is_none() {
                return Err(Error::usage(
                    "media.marks.set needs `in`, `out` or `duration`",
                ));
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
        Request::MediaSet { media, color } => {
            let color = (!color.is_empty()).then(|| color.clone());
            ops::set_media_color(p, &media.to_vec(), color)?;
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
        Request::EditJoin { at, clips } => {
            let at = match at {
                Some(t) => Some(resolve(p, t, playhead)?),
                None => playhead,
            };
            let selected = resolve_clips(p, clips, playhead)?;
            json!({ "joined": ops::join(p, at, &selected)? })
        }
        Request::EditDissolve {
            at,
            track,
            clips,
            length,
            align,
            before,
            after,
            picture,
            sound,
            all,
        } => {
            let at = match at {
                Some(t) => resolve(p, t, playhead)?,
                None => playhead.unwrap_or(0),
            };
            let frames = |t: &TimeSpec| ctx(p, playhead).duration(t);
            let shape = match (before, after) {
                (None, None) => ops::Shape::Length(
                    match length {
                        Some(l) => frames(l)?,
                        None => p.settings.fps.as_f64().round() as i64,
                    },
                    *align,
                ),
                (Some(b), Some(a)) if length.is_none() && *align == ops::Align::default() => {
                    let (b, a) = (frames(b)?, frames(a)?);
                    if b < 0 || a < 0 {
                        return Err(Error::invalid("a dissolve's sides can't be negative"));
                    }
                    ops::Shape::Sides(b, a)
                }
                _ => {
                    return Err(Error::usage(
                        "give a dissolve `before` and `after` together, without `length` and `align`",
                    ));
                }
            };
            let total = match shape {
                ops::Shape::Length(f, _) => f,
                ops::Shape::Sides(b, a) => b + a,
            };
            if total < 1 {
                return Err(Error::invalid("a dissolve lasts a frame at least"));
            }
            let made = if clips.is_empty() {
                let track = track.as_deref().unwrap_or(ops::MAIN);
                ops::dissolve(p, track, at, shape, *picture, *sound, *all)?
            } else if track.is_some() || *all {
                return Err(Error::usage(
                    "dissolves on `clips` go on their own edits; leave out `track` and `all`",
                ));
            } else {
                let ids = resolve_clips(p, clips, playhead)?;
                ops::dissolve_clips(p, &ids, shape, *picture, *sound)?
            };
            json!({ "transitions": made })
        }
        Request::EditRemoveTransition {
            at,
            track,
            picture,
            sound,
            all,
        } => {
            let at = match at {
                Some(t) => resolve(p, t, playhead)?,
                None => playhead.unwrap_or(0),
            };
            let track = track.as_deref().unwrap_or(ops::MAIN);
            json!({ "removed": ops::to_cut(p, track, at, *picture, *sound, *all)? })
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
            detect,
            threshold_db,
            speech_threshold,
            min_speech,
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
            let speech = speech_settings(p, *detect, *speech_threshold, min_speech)?;
            let shown = p.settings.clone();
            let tc = move |f: i64| shown.timecode(f);
            let mut parts = Vec::new();
            let mut removed = Vec::new();
            for id in &ids {
                let c = p.clip(id).expect("resolved");
                if c.is_block() || c.speed() == 0.0 {
                    continue;
                }
                let found = silent_parts(p, env, &Target::Clip(c), &settings, speech.as_ref())?;
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
            color,
        } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let or_none =
                |v: &Option<String>| v.as_ref().map(|v| (!v.is_empty()).then(|| v.clone()));
            let props = ClipProps {
                enabled: *enabled,
                muted: *muted,
                gain_db: *gain_db,
                name: or_none(name),
                color: or_none(color),
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
        Request::MarksSet {
            mark_in,
            out,
            duration,
        } => {
            let mut i = mark_in
                .as_ref()
                .map(|t| resolve(p, t, playhead))
                .transpose()?;
            let mut o = out.as_ref().map(|t| resolve(p, t, playhead)).transpose()?;
            if let Some(d) = duration {
                let c = ctx(p, playhead);
                (i, o) = mark_duration(d, &c, i.or(p.marks.mark_in), o, &p.marks)?;
            }
            if i.is_none() && o.is_none() {
                return Err(Error::usage("marks.set needs `in`, `out` or `duration`"));
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
            name,
            shown,
            language,
            muted: None,
            solo: None,
            volume_db: None,
            video_enabled: None,
        } if track.starts_with('S') => {
            let props = ops::subtitles::TrackProps {
                locked: *locked,
                name: name.as_ref().map(|n| (!n.is_empty()).then(|| n.clone())),
                shown: *shown,
                language: language
                    .as_ref()
                    .map(|l| (!l.trim().is_empty()).then(|| l.trim().to_string())),
            };
            ops::subtitles::set_track(p, track, &props)?;
            Value::Null
        }
        Request::TrackSet { track, .. } if track.starts_with('S') => {
            return Err(Error::usage(
                "a subtitle track takes locked, name, shown and language",
            ));
        }
        Request::TrackSet { shown: Some(_), .. }
        | Request::TrackSet {
            language: Some(_), ..
        } => {
            return Err(Error::usage("shown and language are a subtitle track's"));
        }
        Request::TrackSet {
            track,
            locked,
            muted,
            solo,
            volume_db,
            video_enabled,
            name,
            ..
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
            let ids: Vec<String> = (0..*count)
                .map(|_| match kind {
                    NewTrack::Video => ops::add_track(p, TrackKind::Video),
                    NewTrack::Audio => ops::add_track(p, TrackKind::Audio),
                    NewTrack::Subtitle => ops::subtitles::add_track(p, None),
                })
                .collect();
            json!({ "tracks": ids })
        }
        Request::TrackRemove { track, force } if track.starts_with('S') => {
            ops::subtitles::remove_track(p, track, *force)?;
            Value::Null
        }
        Request::TrackRemove { track, force } => {
            ops::remove_track(p, track, *force)?;
            Value::Null
        }
        Request::MarkersRemove { marker } => {
            ops::remove_marker(p, marker)?;
            Value::Null
        }
        Request::SubtitlesAdd {
            text,
            at,
            end,
            duration,
            track,
            place,
        } => {
            let start = match at {
                Some(t) => resolve(p, t, playhead)?,
                None => need(playhead, "at")?,
            };
            let end = end.as_ref().map(|t| resolve(p, t, playhead)).transpose()?;
            let duration = duration.as_ref().map(|d| length(p, d)).transpose()?;
            let ti = ops::subtitles::target(p, track.as_deref())?;
            let id = ops::subtitles::add_cue(p, ti, start, end, duration, text, *place)?;
            json!({ "cue": id, "track": p.subtitles[ti].id })
        }
        Request::SubtitlesSet {
            cue,
            text,
            start,
            end,
            place,
        } => {
            let id = resolve_cue(p, cue, playhead)?;
            let start = start
                .as_ref()
                .map(|t| resolve(p, t, playhead))
                .transpose()?;
            let end = end.as_ref().map(|t| resolve(p, t, playhead)).transpose()?;
            ops::subtitles::set_cue(p, &id, text.as_deref(), start, end, *place)?;
            Value::Null
        }
        Request::SubtitlesMove {
            cues,
            by,
            to,
            track,
        } => {
            let ids = resolve_cues(p, &cues.to_vec(), playhead)?;
            let by = by.as_ref().map(|d| length(p, d)).transpose()?;
            let to = to.as_ref().map(|t| resolve(p, t, playhead)).transpose()?;
            ops::subtitles::move_cues(p, &ids, by, to, track.as_deref())?;
            Value::Null
        }
        Request::SubtitlesRemove { cues } => {
            let ids = resolve_cues(p, &cues.to_vec(), playhead)?;
            ops::subtitles::remove_cues(p, &ids)?;
            Value::Null
        }
        Request::SubtitlesSplit { cue, at } => {
            let id = resolve_cue(p, cue, playhead)?;
            let at = match at {
                Some(t) => resolve(p, t, playhead)?,
                None => need(playhead, "at")?,
            };
            json!({ "cue": ops::subtitles::split_cue(p, &id, at)? })
        }
        Request::SubtitlesJoin { cues } => {
            let ids = resolve_cues(p, &cues.to_vec(), playhead)?;
            json!({ "cue": ops::subtitles::join_cues(p, &ids)? })
        }
        Request::SubtitlesImport {
            path,
            track,
            at,
            timecode,
            time_base,
        } => {
            let bytes = env.read_file(path)?;
            let (read, format, encoding) = crate::subtitles::parse(&bytes, path)?;
            let at = at.as_ref().map(|t| resolve(p, t, playhead)).transpose()?;
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty());
            let mut warnings = read.warnings.clone();
            let placed =
                ops::subtitles::import(p, read, track.as_deref(), name, at, *timecode, *time_base)?;
            if placed.before_zero > 0 {
                warnings.push(format!(
                    "{} cue(s) end before the timeline starts, left out",
                    placed.before_zero
                ));
            }
            let fps = p.settings.fps.as_f64();
            json!({
                "track": placed.track,
                "cues": placed.cues,
                "format": format,
                "encoding": encoding.name(),
                "time_base": placed.time_base,
                "time_base_why": placed.why,
                "moved_by": format!("{:+.3}s", 0.0 - placed.offset as f64 / fps),
                "moved_by_frames": -placed.offset,
                "warnings": warnings,
            })
        }
        Request::SubtitlesTranscribe {
            range,
            model,
            model_file,
            vad_file,
            language,
            track,
            chars,
            lines,
            gap,
            download,
        } => {
            if !(1..=60).contains(chars) {
                return Err(Error::usage("chars: 1 to 60 characters per line"));
            }
            if !(1..=2).contains(lines) {
                return Err(Error::usage("lines: 1 or 2"));
            }
            if *gap > 10 {
                return Err(Error::usage("gap: 0 to 10 frames"));
            }
            let (a, b) = transcribed_range(p, range.as_ref(), playhead)?;
            let speech = env.transcribe(
                &sound_plan(p, a, b)?,
                &transcript::Asked {
                    model: model.unwrap_or_default(),
                    model_file: model_file.clone(),
                    vad_file: vad_file.clone(),
                    language: language.clone(),
                    download: *download,
                },
            )?;
            let made = transcript::cues(
                &speech.said,
                p.settings.fps,
                transcript::Layout {
                    chars: *chars as usize,
                    lines: *lines as usize,
                    gap: i64::from(*gap),
                },
            );
            let mut warnings = speech.warnings.clone();
            let made: Vec<_> = made.into_iter().filter(|m| a + m.start < b).collect();
            let mut ids = Vec::new();
            let mut on = None;
            if made.is_empty() {
                warnings.push("no speech found".into());
            } else {
                let ti = match track {
                    Some(t) => ops::subtitles::target(p, Some(t))?,
                    None if p.subtitles.is_empty() => ops::subtitles::target(p, None)?,
                    None => p.subtitles.len() - 1,
                };
                if p.subtitles[ti].language.is_none() {
                    p.subtitles[ti].language = speech.language.clone();
                }
                for m in &made {
                    let (start, end) = (a + m.start, (a + m.end).min(b));
                    ids.push(ops::subtitles::add_cue(
                        p,
                        ti,
                        start,
                        Some(end.max(start + 1)),
                        None,
                        &m.text,
                        None,
                    )?);
                }
                on = Some(p.subtitles[ti].id.clone());
            }
            json!({
                "track": on,
                "cues": ids,
                "from": a, "to": b,
                "model": speech.model,
                "device": speech.device,
                "language": speech.language,
                "timing": speech.timing,
                "pieces": speech.pieces,
                "longest_piece": speech.longest_piece,
                "warnings": warnings,
            })
        }
        Request::TimelineImport {
            path,
            otio,
            mode,
            search,
            unpack,
            flatten,
        } => import_timeline(
            p,
            &Import {
                path: path.as_deref(),
                doc: otio.as_ref(),
                mode: *mode,
                search,
                unpack: unpack.as_deref(),
                write: !dry_run,
                flatten: *flatten,
                adopt_rate: true,
            },
            env,
        )?,
        Request::EditDecompose { clips } => {
            let ids = resolve_clips(p, &clips.to_vec(), playhead)?;
            let (mut made, mut warnings, mut played) = (Vec::new(), Vec::new(), Vec::new());
            for id in &ids {
                if let Some(k) = p.clip(id).and_then(|c| c.compound.as_ref()) {
                    played.extend(otio::played_by(k));
                }
                made.extend(decompose(
                    p,
                    id,
                    env,
                    (Lookup::default(), false),
                    &mut warnings,
                )?);
            }
            let at = Lookup {
                played: &played,
                ..Lookup::default()
            };
            fill_contents(p, &made, env, at, &mut warnings);
            json!({ "clips": made, "warnings": warnings })
        }
        Request::Apply { commands, .. } => {
            let mut results = Vec::new();
            for (i, cmd) in commands.iter().enumerate() {
                if matches!(
                    cmd,
                    Request::HistoryUndo { .. }
                        | Request::HistoryRedo { .. }
                        | Request::HistoryGo { .. }
                        | Request::HistoryMark { .. }
                        | Request::HistoryUnmark { .. }
                        | Request::HistoryRestore { .. }
                        | Request::HistoryRevert { .. }
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
                    query(p, &Labels::default(), cmd, env, playhead, dry_run)
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

fn first_video_sets_format(p: &mut Project, info: &MediaInfo) {
    if p.settings.auto_format
        && let Some(v) = info.video.as_ref().filter(|v| !v.still)
    {
        p.settings.set_fps(v.fps);
        let drop = v.fps.has_drop_frame() && info.timecode.is_some_and(|t| t.drop_frame);
        p.settings
            .set_drop_frame(drop)
            .expect("the rate has drop-frame");
        adopt_size(p, v);
    }
}

struct PoolFiles(HashMap<PathBuf, Vec<usize>>);

impl PoolFiles {
    fn of(p: &Project) -> Self {
        let mut files = Self(HashMap::new());
        for i in 0..p.media.len() {
            files.note(p, i);
        }
        files
    }

    fn find<'p>(
        &self,
        p: &'p Project,
        path: &Path,
        sequence: &Option<ImageSequence>,
    ) -> Option<&'p Media> {
        self.0
            .get(path)?
            .iter()
            .map(|&i| &p.media[i])
            .find(|m| &m.sequence == sequence)
    }

    fn note(&mut self, p: &Project, i: usize) {
        self.0.entry(p.media[i].path.clone()).or_default().push(i);
    }
}

fn add_media(p: &mut Project, files: &mut PoolFiles, probed: ProbedMedia) -> String {
    if let Some(existing) = files.find(p, &probed.path, &probed.sequence) {
        return existing.id.clone();
    }
    first_video_sets_format(p, &probed.info);
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
        color: None,
        fingerprint: probed.fingerprint,
    });
    files.note(p, p.media.len() - 1);
    id
}

#[derive(Default)]
struct FolderImport {
    sequence: bool,
    fps: Option<Rational>,
    media: Vec<String>,
    skipped: Vec<Value>,
}

impl FolderImport {
    fn skip(&mut self, path: &Path, why: &str) {
        self.skipped.push(json!({ "path": path, "why": why }));
    }
}

fn add_folder(
    p: &mut Project,
    env: &dyn Env,
    files: &mut PoolFiles,
    folder: &Folder,
    bin: &str,
    out: &mut FolderImport,
) -> Result<()> {
    if let Some(e) = &folder.error {
        out.skip(&folder.path, e);
        return Ok(());
    }
    ops::add_bin(p, bin)?;
    let runs: HashMap<PathBuf, ImageSequence> = match out.sequence {
        true => folder.runs_here().into_iter().collect(),
        false => HashMap::new(),
    };
    let taken: HashSet<PathBuf> = runs
        .iter()
        .flat_map(|(file, s)| {
            (0..s.count)
                .map(|k| s.file(&folder.path, k))
                .filter(move |f| f != file)
        })
        .collect();
    for file in &folder.files {
        if taken.contains(file) {
            continue;
        }
        if !crate::folder::is_media(file) {
            out.skip(file, crate::folder::NOT_MEDIA);
            continue;
        }
        let probed = match runs.contains_key(file) {
            true => env.probe_sequence(file, out.fps.unwrap_or(p.settings.fps)),
            false => env.probe(file),
        };
        let probed = match probed {
            Ok(probed) => probed,
            Err(e) => {
                match runs.get(file) {
                    Some(s) => out.skip(&folder.path.join(s.display()), &e.message),
                    None => out.skip(file, &e.message),
                }
                continue;
            }
        };
        let id = add_media(p, files, probed);
        ops::move_media(p, std::slice::from_ref(&id), bin)?;
        if !out.media.contains(&id) {
            out.media.push(id);
        }
    }
    for sub in &folder.folders {
        let name = sub.name();
        if name.trim().is_empty() {
            out.skip(&sub.path, "a folder with a blank name can't be a bin");
            continue;
        }
        let inner = format!("{bin}/{}", name.trim());
        add_folder(p, env, files, sub, &inner, out)?;
    }
    Ok(())
}

fn no_media(path: &Path, skipped: &[Value]) -> String {
    let mut text = format!("no media in {} or the folders in it", path.display());
    let shown: Vec<String> = skipped
        .iter()
        .take(3)
        .map(|s| {
            let file = Path::new(s["path"].as_str().unwrap_or_default());
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            format!("{name}: {}", s["why"].as_str().unwrap_or_default())
        })
        .collect();
    if !shown.is_empty() {
        text.push_str(&format!(" (skipped {}", shown.join("; ")));
        if skipped.len() > shown.len() {
            text.push_str(&format!(" and {} more", skipped.len() - shown.len()));
        }
        text.push(')');
    }
    text
}

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
                env.remove_proxy_file(&old.path_in(&env.proxy_dir()))?;
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

fn adopt_size(p: &mut Project, v: &VideoInfo) {
    (p.settings.width, p.settings.height) = v.display_size();
    p.settings.auto_format = false;
}

fn rescale(f: i64, from: Rational, to: Rational) -> i64 {
    if from == to {
        return f;
    }
    let n = i128::from(f) * i128::from(from.den()) * i128::from(to.num());
    let d = i128::from(from.num()) * i128::from(to.den());
    (2 * n + d).div_euclid(2 * d) as i64
}

struct Import<'a> {
    path: Option<&'a Path>,
    doc: Option<&'a Value>,
    mode: ImportMode,
    search: &'a [PathBuf],
    unpack: Option<&'a Path>,
    write: bool,
    flatten: bool,
    adopt_rate: bool,
}

type MediaIds = HashMap<(String, bool), String>;

fn media_key(m: &otio::MediaRef) -> (String, bool) {
    (m.url.clone(), m.sequence.is_some())
}

fn find_media<'a>(
    p: &mut Project,
    clips: impl IntoIterator<Item = &'a otio::ImportedClip>,
    env: &dyn Env,
    dirs: &[PathBuf],
    hint: bool,
) -> Result<(MediaIds, Vec<String>)> {
    let mut ids = MediaIds::new();
    let mut media = Vec::new();
    let mut found: HashSet<String> = HashSet::new();
    let mut files = PoolFiles::of(p);
    let mut missing: Vec<&str> = Vec::new();
    for c in clips.into_iter().filter(|c| !c.is_block()) {
        let m = &c.media;
        if ids.contains_key(&media_key(m)) || missing.contains(&m.shown.as_str()) {
            continue;
        }
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
                let id = add_media(p, &mut files, probed);
                if found.insert(id.clone()) {
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

fn at_project_rate(p: &Project, t: &mut otio::Imported, ids: &MediaIds) {
    let pool: HashMap<&str, &Media> = p.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let (from, to) = (t.fps, p.settings.fps);
    let conv = |f: i64| rescale(f, from, to);
    let retime = |list: &mut Vec<Value>| {
        if from != to && !list.is_empty() {
            let mut e = Effects {
                from: 0,
                list: std::mem::take(list).into_iter().map(Kept::from).collect(),
            };
            e.retime(|x| (x * to.as_f64() / from.as_f64()).round());
            *list = e.list.into_iter().map(Kept::into_value).collect();
        }
    };
    for c in &mut t.clips {
        let tc = match (c.is_block(), c.timecoded) {
            (false, true) => pool
                .get(ids[&media_key(&c.media)].as_str())
                .filter(|m| m.sequence.is_none())
                .and_then(|m| m.info.timecode)
                .map_or(0, |tc| tc.frames_at(from).round() as i64),
            _ => 0,
        };
        c.timecoded = false;
        let (start, end) = (conv(c.start), conv(c.end()));
        if let Some((h, tl)) = c.audio {
            c.audio = Some((conv(c.start + h) - start, conv(c.end() + tl) - end));
        }
        c.source_in = conv(c.source_in - tc);
        if let Some(k) = c.compound.as_mut() {
            k.frames = conv(k.frames).max(c.source_in + end - start);
        }
        retime(&mut c.effects);
        retime(&mut c.audio_effects);
        (c.start, c.duration) = (start, end - start);
    }
    for x in &mut t.transitions {
        (x.before, x.after) = (conv(x.before), conv(x.after));
    }
    for k in &mut t.markers {
        (k.at, k.duration) = (conv(k.at), conv(k.duration).max(1));
    }
    t.marks = crate::model::Marks {
        mark_in: t.marks.mark_in.map(conv),
        mark_out: t.marks.mark_out.map(conv),
    };
    t.fps = to;
}

fn place_imported(
    p: &mut Project,
    t: &otio::Imported,
    ids: &MediaIds,
    offset: i64,
    tracks: &HashMap<String, String>,
    warnings: &mut Vec<String>,
    keep_ids: bool,
) -> Result<(Vec<String>, Vec<String>)> {
    let color = |c: &otio::ImportedClip| match (&c.own_color, keep_ids) {
        (Some(own), true) => own.clone(),
        _ => c.color.clone(),
    };
    let mut taken: HashSet<String> = HashSet::new();
    if keep_ids {
        for t in &p.tracks {
            taken.extend(t.clips.iter().map(|c| c.id.clone()));
            taken.extend(t.transitions.iter().map(|x| x.id.clone()));
        }
    }
    let mut clip_id = |p: &mut Project, kept: &Option<String>| match kept
        .as_ref()
        .filter(|id| keep_ids && taken.insert(id.to_string()))
    {
        Some(id) => id.clone(),
        None => p.alloc_clip_id(),
    };
    let mut placed: HashSet<String> = HashSet::new();
    let pool: HashMap<String, usize> = p
        .media
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.clone(), i))
        .collect();
    debug_assert_eq!(t.fps, p.settings.fps, "converted first (at_project_rate)");
    let track_of = |id: &str| tracks.get(id).cloned().unwrap_or_else(|| id.to_string());
    let carry = |list: &[Value], first: i64| {
        (!list.is_empty()).then(|| Effects {
            from: first,
            list: list.iter().cloned().map(Kept::from).collect(),
        })
    };
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
    let mut made: HashMap<usize, (String, String)> = HashMap::new();
    for c in &t.clips {
        if c.is_block() {
            let (start, end) = (offset + c.start, offset + c.end());
            if end - start < 1 {
                continue;
            }
            let source_in = c.source_in;
            let compound = c.compound.as_ref().map(|k| Compound {
                stack: k.stack.clone(),
                frames: k.frames,
                tracks: Vec::new(),
            });
            let id = clip_id(p, &c.id);
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
                    reference: reference.clone().into(),
                }),
                compound,
                speed: c.speed.clone(),
                color: color(c),
            });
            placed.insert(track_of(&c.track));
            continue;
        }
        let m = p.media[pool[&ids[&media_key(&c.media)]]].clone();
        let frames = p.media_limit(&m);
        let label = if c.name.is_empty() { &m.name } else { &c.name };
        let mut start = offset + c.start;
        let end = offset + c.end();
        let mut source_in = c.source_in;
        let effects = carry(&c.effects, source_in);
        let sound_from = c.source_in + c.audio.map_or(0, |a| a.0);
        if c.video && m.info.video.is_none() {
            warnings.push(format!("{label}: {} has no video; left out", m.name));
            continue;
        }
        let still = m.is_still();
        if source_in < 0 && !still {
            warnings.push(format!(
                "{label}: {} frame(s) before the start of {} were left out",
                -source_in, m.name
            ));
            start -= source_in;
            source_in = 0;
        }
        let duration = (end - start).min(if still { frames } else { frames - source_in });
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
        let audio = match (c.audio, t.has_audio_tracks) {
            (Some((h, tl)), _) => Some((
                offset + c.start + h - start,
                offset + c.end() + tl - (start + duration),
            )),
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
        let id = clip_id(p, &c.id);
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
            color: color(c),
        });
        placed.insert(track_of(&c.track));
    }
    for id in &placed {
        if let Some(track) = p.track_mut(id) {
            track.sort();
        }
    }
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
        let id = match x
            .id
            .clone()
            .filter(|id| keep_ids && taken.insert(id.clone()))
        {
            Some(id) => id,
            None => p.alloc_transition_id(),
        };
        p.track_mut(track)
            .expect("made above")
            .transitions
            .push(Transition {
                id,
                sound: x.sound,
                from: from.map(|f| f.0.clone()),
                to: to.map(|g| g.0.clone()),
                before: x.before,
                after: x.after,
                otio: x.otio.clone().into(),
            });
    }
    let dropped = otio::transitions::prune(p);
    if dropped > 0 {
        warnings.push(otio::transitions::dropped(dropped));
    }
    let markers = t
        .markers
        .iter()
        .map(|k| ops::NewMarker {
            kept: k.id.clone().filter(|_| keep_ids),
            at: offset + k.at,
            duration: k.duration,
            name: k.name.clone(),
            color: Some(k.color.clone()),
            note: k.note.clone(),
        })
        .collect();
    let markers = ops::add_markers(p, markers)?;
    Ok((clips, markers))
}

#[derive(Debug, Clone, Copy, Default)]
struct Lookup<'a> {
    base: Option<&'a Path>,
    dirs: &'a [PathBuf],
    played: &'a [String],
}

fn nested_media(
    p: &mut Project,
    clips: &[otio::ImportedClip],
    env: &dyn Env,
    at: Lookup,
) -> Result<MediaIds> {
    let among: Vec<&Media> = p
        .media
        .iter()
        .filter(|m| at.played.contains(&m.id))
        .collect();
    let mut ids: MediaIds = clips
        .iter()
        .filter(|c| !c.is_block())
        .filter_map(|c| {
            let m = otio::same_media(&c.media, &among)?;
            Some((media_key(&c.media), m.id.clone()))
        })
        .collect();
    let others = clips
        .iter()
        .filter(|c| !ids.contains_key(&media_key(&c.media)));
    let (found, _) = find_media(p, others, env, at.dirs, false)?;
    ids.extend(found);
    Ok(ids)
}

fn nested(k: &Compound, video: bool, base: Option<&Path>) -> Result<otio::Imported> {
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
    let mut t = otio::import(&doc, base)?;
    if video {
        t.has_audio_tracks = true;
        for c in &mut t.clips {
            c.audio = None;
            c.audio_effects.clear();
        }
    }
    Ok(t)
}

fn compound_contents(
    p: &mut Project,
    k: &Compound,
    video: bool,
    env: &dyn Env,
    at: Lookup,
    depth: usize,
) -> Result<Vec<crate::model::Track>> {
    let mut t = nested(k, video, at.base)?;
    let ids = nested_media(p, &t.clips, env, at)?;
    at_project_rate(p, &mut t, &ids);
    let mut inner = Project::new(p.settings.clone());
    let mut needed: Vec<&str> = t.clips.iter().map(|c| c.track.as_str()).collect();
    needed.sort_by_key(|id| (!id.starts_with('V'), id[1..].parse::<usize>().unwrap_or(0)));
    needed.dedup();
    for id in needed {
        ops::ensure_track(&mut inner, id)?;
    }
    inner.media = std::mem::take(&mut p.media);
    let placed = place_imported(
        &mut inner,
        &t,
        &ids,
        0,
        &HashMap::new(),
        &mut Vec::new(),
        false,
    );
    p.media = std::mem::take(&mut inner.media);
    placed?;
    if depth > 0 {
        for ti in 0..inner.tracks.len() {
            for ci in 0..inner.tracks[ti].clips.len() {
                let c = inner.tracks[ti].clips[ci].clone();
                if let Some(nk) = &c.compound {
                    let tracks =
                        compound_contents(p, nk, c.video, env, at, depth - 1).unwrap_or_default();
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

fn fill_contents(
    p: &mut Project,
    ids: &[String],
    env: &dyn Env,
    at: Lookup,
    warnings: &mut Vec<String>,
) {
    let ids: HashSet<&String> = ids.iter().collect();
    let blocks: Vec<(usize, usize)> = p
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(ti, t)| {
            t.clips
                .iter()
                .enumerate()
                .filter(|(_, c)| c.compound.is_some() && ids.contains(&c.id))
                .map(move |(ci, _)| (ti, ci))
        })
        .collect();
    for (ti, ci) in blocks {
        let c = p.tracks[ti].clips[ci].clone();
        let id = &c.id;
        let Some(k) = &c.compound else {
            continue;
        };
        match compound_contents(p, k, c.video, env, at, 8) {
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

fn decompose(
    p: &mut Project,
    id: &str,
    env: &dyn Env,
    (at, import): (Lookup, bool),
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
    let mut t = nested(&k, block.video, at.base)?;
    let to = p.settings.fps;
    let conv = |f: i64| rescale(f, t.fps, to);
    let (lo, hi) = block.shown();
    t.clips.retain(|c| conv(c.start) < hi && conv(c.end()) > lo);
    t.markers.retain(|m| (lo..hi).contains(&conv(m.at)));
    let played = otio::played_by(&k);
    let ids = nested_media(
        p,
        &t.clips,
        env,
        Lookup {
            played: &played,
            ..at
        },
    )?;
    at_project_rate(p, &mut t, &ids);
    if block.speed() != 1.0 {
        t.clips.retain_mut(|c| {
            let Some((k0, k1)) = crate::render::shown_between(&block, c.start, c.end()) else {
                return false;
            };
            c.walk(block.source_at(block.start + k0) - c.start);
            (c.start, c.duration) = (k0, k1 - k0);
            c.speed = crate::render::paced(&block, c.speed.as_ref(), k0);
            if let Some(a) = c.audio.as_mut() {
                *a = (0, 0);
            }
            true
        });
        t.markers.retain_mut(
            |m| match crate::render::shown_between(&block, m.at, m.at + 1) {
                Some((k, _)) => {
                    m.at = k;
                    true
                }
                None => false,
            },
        );
    } else {
        t.clips.retain_mut(|c| {
            let (s, e) = (c.start.max(lo), c.end().min(hi));
            c.walk(s - c.start);
            (c.start, c.duration) = (s - lo, e - s);
            e > s
        });
        for m in &mut t.markers {
            m.at -= lo;
        }
    }
    for w in &t.warnings {
        if !warnings.contains(w) {
            warnings.push(w.clone());
        }
    }
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
    let (made, _) = place_imported(p, &t, &ids, start, &map, warnings, false)?;
    if let Some(e) = &block.effects {
        warnings.push(format!(
            "{id}'s own effects were dropped: {}",
            e.names().join(", ")
        ));
    }
    Ok(made)
}

fn import_timeline(p: &mut Project, import: &Import, env: &dyn Env) -> Result<Value> {
    let Import {
        path,
        doc,
        mode,
        search,
        unpack,
        write,
        flatten,
        adopt_rate,
    } = *import;
    if flatten && mode == ImportMode::Compound {
        return Err(Error::usage(
            "flatten decomposes compound clips: it can't import as one",
        ));
    }
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
    let base = file.as_ref().map(|f| f.base.as_path());
    let mut t = otio::import(&doc, base)?;
    let stem = path
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().into_owned());
    let name = match (t.name.as_str(), &stem) {
        ("", Some(stem)) => stem.clone(),
        (name, _) => name.to_string(),
    };
    let meta = t.project.take();
    let wanted = meta
        .as_ref()
        .map(|meta| wanted_media(meta, &t, mode == ImportMode::Media));
    if mode == ImportMode::Compound {
        let warnings = std::mem::take(&mut t.warnings);
        t = otio::import(&otio::as_compound(&doc, &name)?, base)?;
        t.warnings = warnings;
    }
    let bundle = file.as_ref().and_then(|f| f.bundle.clone());
    let pool = meta
        .as_ref()
        .is_some_and(|m| mode == ImportMode::Media && !m.media.is_empty());
    if t.clips.is_empty() && !pool {
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

    let mut dirs = search.to_vec();
    if let Some(b) = &bundle {
        dirs.push(b.folder.clone());
    }
    let at = Lookup {
        base,
        dirs: &dirs,
        ..Lookup::default()
    };
    let (ids, media) = match (&meta, &wanted) {
        (Some(meta), Some(wanted)) => {
            let bin = stem.as_deref().unwrap_or(&name).trim().replace('/', "-");
            let bin = if bin.is_empty() {
                "Imported".into()
            } else {
                bin
            };
            let kept = join_pool(p, meta, wanted, &bin)?;
            let mut ids = MediaIds::new();
            for c in t.clips.iter().filter(|c| !c.is_block()) {
                if let Some(id) = c.media_id.as_ref().and_then(|id| kept.get(id)) {
                    ids.insert(media_key(&c.media), id.clone());
                }
            }
            let others = t
                .clips
                .iter()
                .filter(|c| !ids.contains_key(&media_key(&c.media)));
            let (found, mut media) = find_media(p, others, env, &dirs, search.is_empty())?;
            ids.extend(found);
            for id in kept.into_values() {
                if !media.contains(&id) {
                    media.push(id);
                }
            }
            (ids, media)
        }
        _ => find_media(p, &t.clips, env, &dirs, search.is_empty())?,
    };
    if mode == ImportMode::Media {
        let mut media = media;
        compound_media(p, &t, env, at, &mut media)?;
        return Ok(json!({
            "name": name,
            "fps": p.settings.fps,
            "clips": [],
            "media": media,
            "markers": [],
            "warnings": warnings,
        }));
    }

    if mode == ImportMode::Replace {
        for track in &mut p.tracks {
            track.clips.clear();
        }
        p.markers.clear();
        p.marks = Default::default();
    }
    let empty = p.tracks.iter().all(|t| t.clips.is_empty());
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
        let drop = t.clips.iter().filter(|c| !c.is_block()).any(|c| {
            p.media(&ids[&media_key(&c.media)])
                .and_then(|m| m.info.timecode)
                .is_some_and(|tc| tc.drop_frame)
        });
        let rate = p.settings.fps;
        p.settings.set_fps(t.fps);
        if drop && t.fps.has_drop_frame() {
            p.settings.drop_frame = true;
        }
        p.settings.start_tc = t.start_tc;
        if !adopt_rate {
            p.settings.set_fps(rate);
        }
        p.settings.auto_format = false;
    }
    if t.fps != p.settings.fps {
        warnings.push(format!(
            "converted from {} to the project's {} fps: cuts may move by up to half a frame",
            t.fps, p.settings.fps
        ));
    }
    at_project_rate(p, &mut t, &ids);
    let offset = if empty { 0 } else { p.end() };
    add_tracks(p, &t, empty)?;
    let (mut clips, markers) =
        place_imported(p, &t, &ids, offset, &HashMap::new(), &mut warnings, false)?;
    let mut pending = compounds_among(p, &clips);
    while flatten && !pending.is_empty() {
        let mut next = Vec::new();
        for id in pending {
            let made = decompose(p, &id, env, (at, true), &mut warnings)?;
            clips.retain(|c| c != &id);
            next.extend(compounds_among(p, &made));
            clips.extend(made);
        }
        pending = next;
    }
    if mode == ImportMode::Compound || flatten && compounds_among(p, &clips).is_empty() {
        warnings.retain(|w| !w.starts_with(otio::COMPOUNDS_KEPT));
    }
    fill_contents(p, &clips, env, at, &mut warnings);
    if empty {
        let (i, o) = (t.marks.mark_in, t.marks.mark_out);
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

fn compounds_among(p: &Project, ids: &[String]) -> Vec<String> {
    let all: HashSet<&str> = p
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| c.compound.is_some())
        .map(|c| c.id.as_str())
        .collect();
    ids.iter()
        .filter(|id| all.contains(id.as_str()))
        .cloned()
        .collect()
}

fn same_file(r: &otio::MediaRef, m: &Media) -> bool {
    r.paths.contains(&m.path) && r.sequence.is_some() == m.sequence.is_some()
}

fn wanted_media(meta: &otio::ProjectMeta, t: &otio::Imported, all: bool) -> HashSet<String> {
    let by_id: HashMap<&str, &Media> = meta.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut agree: HashMap<&str, bool> = HashMap::new();
    for c in t.clips.iter().filter(|c| !c.is_block()) {
        if let Some(m) = c.media_id.as_deref().and_then(|id| by_id.get(id)) {
            let same = same_file(&c.media, m);
            agree
                .entry(&m.id)
                .and_modify(|a| *a &= same)
                .or_insert(same);
        }
    }
    meta.media
        .iter()
        .filter(|m| agree.get(m.id.as_str()).copied().unwrap_or(all))
        .map(|m| m.id.clone())
        .collect()
}

fn join_pool(
    p: &mut Project,
    meta: &otio::ProjectMeta,
    wanted: &HashSet<String>,
    bin: &str,
) -> Result<HashMap<String, String>> {
    let mut ids = HashMap::new();
    let mut files = PoolFiles::of(p);
    for m in meta.media.iter().filter(|m| wanted.contains(&m.id)) {
        let id = match files.find(p, &m.path, &m.sequence) {
            Some(x) => x.id.clone(),
            None => {
                let bin = match m.bin.as_str() {
                    "" => bin.to_string(),
                    own => format!("{bin}/{own}"),
                };
                ops::add_bin(p, &bin)?;
                first_video_sets_format(p, &m.info);
                let id = p.alloc_media_id();
                p.media.push(Media {
                    id: id.clone(),
                    bin,
                    proxy: None,
                    ..m.clone()
                });
                files.note(p, p.media.len() - 1);
                id
            }
        };
        ids.insert(m.id.clone(), id);
    }
    Ok(ids)
}

fn compound_media(
    p: &mut Project,
    t: &otio::Imported,
    env: &dyn Env,
    at: Lookup,
    media: &mut Vec<String>,
) -> Result<()> {
    for c in &t.clips {
        let Some(k) = &c.compound else { continue };
        let inner = nested(k, c.video, at.base)?;
        let (_, found) = find_media(p, &inner.clips, env, at.dirs, false)?;
        for id in found {
            if !media.contains(&id) {
                media.push(id);
            }
        }
        compound_media(p, &inner, env, at, media)?;
    }
    Ok(())
}

fn add_tracks(p: &mut Project, t: &otio::Imported, settings: bool) -> Result<()> {
    let mut needed: Vec<&str> = t.clips.iter().map(|c| c.track.as_str()).collect();
    needed.extend(t.tracks.iter().map(|t| t.id.as_str()));
    needed.sort_by_key(|id| (!id.starts_with('V'), id[1..].parse::<usize>().unwrap_or(0)));
    needed.dedup();
    for id in needed {
        ops::ensure_track(p, id)?;
    }
    if settings {
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
    Ok(())
}

pub fn open_otio(
    doc: &Value,
    base: Option<&Path>,
    env: &dyn Env,
) -> Result<Option<(Project, Vec<String>)>> {
    let mut t = otio::import(doc, base)?;
    let Some(meta) = t.project.take() else {
        return Ok(None);
    };
    let mut p = Project::new(meta.settings);
    p.next_id = meta.next_id;
    p.media = meta.media;
    p.bins = meta.bins;
    p.tape_marks = meta.tape_marks;
    p.subtitles = meta.subtitles;
    p.settings.start_tc = t.start_tc;
    let mut warnings = t.warnings.clone();
    let mut ids = MediaIds::new();
    for c in t.clips.iter().filter(|c| !c.is_block()) {
        let named = c.media_id.as_deref().and_then(|id| p.media(id));
        if let Some(m) = named.filter(|m| same_file(&c.media, m)) {
            ids.insert(media_key(&c.media), m.id.clone());
        }
    }
    let others = t
        .clips
        .iter()
        .filter(|c| !ids.contains_key(&media_key(&c.media)));
    let (found, _) = find_media(&mut p, others, env, &[], false)?;
    ids.extend(found);
    if t.fps != p.settings.fps {
        warnings.push(format!(
            "converted from {} to the project's {} fps: cuts may move by up to half a frame",
            t.fps, p.settings.fps
        ));
    }
    at_project_rate(&p, &mut t, &ids);
    add_tracks(&mut p, &t, true)?;
    let (clips, _) = place_imported(&mut p, &t, &ids, 0, &HashMap::new(), &mut warnings, true)?;
    fill_contents(
        &mut p,
        &clips,
        env,
        Lookup {
            base,
            ..Lookup::default()
        },
        &mut warnings,
    );
    p.marks = t.marks.clone();
    let problems = validate::violations(&p);
    if !problems.is_empty() {
        return Err(Error::invalid(format!(
            "the file's project is inconsistent: {}",
            problems.join("; ")
        )));
    }
    Ok(Some((p, warnings)))
}

pub fn project_from_otio(
    path: &Path,
    format: &Format,
    write: bool,
    env: &dyn Env,
) -> Result<(Project, Vec<String>)> {
    let file = env.open_timeline(path, None, write)?;
    let doc = serde_json::from_str::<Value>(&file.text)
        .map_err(|e| Error::usage(format!("{}: not an OTIO (JSON) file: {e}", path.display())))?;
    if otio::holds_project(&doc) {
        if !format.is_empty() {
            return Err(Error::usage(format!(
                "{} holds a vidcut project, with its own format: leave out the format",
                path.display()
            )));
        }
        if let Some(opened) = open_otio(&doc, Some(&file.base), env)? {
            return Ok(opened);
        }
    }
    let first = Format {
        drop_frame: None,
        start_tc: None,
        ..format.clone()
    };
    let mut p = Project::new(first.settings()?);
    let import = Import {
        path: Some(path),
        doc: None,
        mode: ImportMode::Append,
        search: &[],
        unpack: None,
        write,
        flatten: false,
        adopt_rate: format.fps.is_none(),
    };
    let result = import_timeline(&mut p, &import, env)?;
    if let Some(d) = format.drop_frame {
        p.settings.set_drop_frame(d)?;
    }
    if let Some(tc) = &format.start_tc {
        p.settings.set_start_tc(tc)?;
    }
    let mut warnings: Vec<String> =
        serde_json::from_value(result["warnings"].clone()).unwrap_or_default();
    let dropped = otio::transitions::prune(&mut p);
    if dropped > 0 {
        warnings.push(otio::transitions::dropped(dropped));
    }
    let problems = validate::violations(&p);
    if !problems.is_empty() {
        return Err(Error::internal(format!(
            "the imported project breaks invariants (a vidcut bug): {}",
            problems.join("; ")
        )));
    }
    Ok((p, warnings))
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
    let dir = env.proxy_dir();
    let lost = |m: &Media| {
        m.fresh_proxy()
            .filter(|x| !env.proxy_present(&x.path_in(&dir)))
            .cloned()
    };
    if auto {
        ids.retain(|id| {
            p.media(id)
                .is_some_and(|m| rules.wants(m) && (m.fresh_proxy().is_none() || lost(m).is_some()))
        });
    }
    let mut proxies = Vec::new();
    for id in ids {
        let item = p.media(&id).expect("checked").clone();
        let like = auto.then(|| lost(&item)).flatten();
        let (size, codec) = match &like {
            Some(x) => (x.width.max(x.height), Some(x.codec.as_str())),
            None => (longest, None),
        };
        let made = env.generate_proxy(&item, size, codec, &dir, dry_run)?;
        let file = made
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entry = Proxy {
            file,
            source_size: item.size,
            source_mtime_ns: item.mtime_ns,
            width: made.width,
            height: made.height,
            codec: made.codec,
        };
        let m = p.media.iter_mut().find(|m| m.id == id).expect("checked");
        if let Some(old) = m.proxy.take()
            && old.file != entry.file
            && !dry_run
        {
            env.remove_proxy_file(&old.path_in(&dir))?;
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
        if let Some(copies) = made.copies {
            done["copies"] = json!(copies);
        }
        proxies.push(done);
    }
    Ok(json!({ "proxies": proxies }))
}

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
    let dir = env.proxy_dir();
    for id in ids {
        let m = p.media.iter_mut().find(|m| m.id == id).expect("checked");
        let Some(old) = m.proxy.take() else {
            continue;
        };
        if !dry_run {
            env.remove_proxy_file(&old.path_in(&dir))?;
        }
        cleared.push(id);
    }
    Ok(json!({ "cleared": cleared }))
}

pub fn query(
    p: &Project,
    history: &Labels,
    request: &Request,
    env: &dyn Env,
    playhead: Option<i64>,
    dry_run: bool,
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
            "subtitles": p.subtitles.iter().map(|t| json!({
                "track": t.id, "name": t.display_name(), "cues": t.cues.len(), "shown": t.shown,
            })).collect::<Vec<_>>(),
            "bins": p.bins,
            "tape_marks": p.tape_marks,
        }),
        Request::MediaList {} => {
            json!(
                p.media
                    .iter()
                    .map(|m| media_json(p, m, env, &env.proxy_dir()))
                    .collect::<Vec<_>>()
            )
        }
        Request::MediaGet { media } => {
            let m = p
                .media(media)
                .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
            let mut v = media_json(p, m, env, &env.proxy_dir());
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
                    let mut v = clip_json(p, c, p.media(&c.media), &t.id);
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
                .chain(p.subtitles.iter().map(|t| {
                    json!({
                        "id": t.id, "kind": "subtitle", "cues": t.cues.len(),
                        "locked": t.locked, "shown": t.shown, "name": t.display_name(),
                        "language": t.language,
                    })
                }))
                .collect::<Vec<_>>()
        ),
        Request::BinsList {} => json!(p.bins),
        Request::SubtitlesList { track, range } => {
            let (a, b) = match range {
                Some(r) => ctx(p, playhead).range(r)?,
                None => (0, i64::MAX),
            };
            let tracks: Vec<&crate::model::SubtitleTrack> = match track {
                Some(id) => vec![
                    p.subtitle_track(id)
                        .ok_or_else(|| Error::not_found(format!("no subtitle track {id}")))?,
                ],
                None => p.subtitles.iter().collect(),
            };
            json!(
                tracks
                    .iter()
                    .map(|t| json!({
                        "track": t.id,
                        "name": t.display_name(),
                        "language": t.language,
                        "shown": t.shown,
                        "locked": t.locked,
                        "cues": t.cues
                            .iter()
                            .filter(|c| c.start < b && c.end() > a)
                            .map(|c| {
                                let mut v = serde_json::to_value(c).unwrap_or_default();
                                v["end"] = json!(c.end());
                                v["start_tc"] = json!(tc(c.start));
                                v["end_tc"] = json!(tc(c.end()));
                                v
                            })
                            .collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>()
            )
        }
        Request::SubtitlesExport {
            out,
            track,
            format,
            range,
            timecode,
            overwrite,
        } => {
            let ti = match track {
                Some(id) => p
                    .subtitle_index(id)
                    .ok_or_else(|| Error::not_found(format!("no subtitle track {id}")))?,
                None => ops::subtitles::shown(p)?,
            };
            let format = format
                .or_else(|| crate::subtitles::format_of(out))
                .ok_or_else(|| {
                    Error::usage(format!(
                        "{}: name a format (srt, vtt, ttml) or use one of their extensions",
                        out.display()
                    ))
                })?;
            if !overwrite && env.exists(out) {
                return Err(Error::invalid(format!(
                    "{} exists; pass overwrite to replace it",
                    out.display()
                )));
            }
            let span = match range {
                Some(r) => ctx(p, playhead).range(r)?,
                None => (0, i64::MAX),
            };
            let cues = ops::subtitles::collect(p, ti, span, timecode.unwrap_or(true));
            let t = &p.subtitles[ti];
            let text = crate::subtitles::write(
                &cues,
                format,
                &crate::subtitles::Writing {
                    language: t.language.as_deref(),
                    frame_rate: Some(p.settings.fps.as_f64()),
                    positions: true,
                },
            );
            if !dry_run {
                env.write_file(out, text.as_bytes())?;
            }
            json!({ "path": out, "track": t.id, "format": format, "cues": cues.len() })
        }
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
        Request::HistoryGet { .. } => json!({
            "undo": history.undo.iter().rev().collect::<Vec<_>>(),
            "redo": history.redo.iter().rev().collect::<Vec<_>>(),
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
            detect,
            threshold_db,
            speech_threshold,
            min_speech,
            min,
        } => {
            let settings = SilenceSettings {
                threshold_db: *threshold_db,
                pre_head: 0,
                post_tail: 0,
                min: length(p, min)?.max(1),
            };
            let speech = speech_settings(p, *detect, *speech_threshold, min_speech)?;
            let mut out = Vec::new();
            for target in targets(p, media, clips, playhead, true)? {
                let ranges = silent_parts(p, env, &target, &settings, speech.as_ref())?;
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
        | Request::EncodersList { .. }
        | Request::DecodersList {} => {
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

fn finite(db: f64) -> Value {
    if db.is_finite() {
        json!(db)
    } else {
        Value::Null
    }
}

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

fn speech_settings(
    p: &Project,
    detect: Detect,
    threshold: f64,
    min_speech: &TimeSpec,
) -> Result<Option<SpeechSettings>> {
    if detect == Detect::Level {
        return Ok(None);
    }
    if !(0.0..=1.0).contains(&threshold) {
        return Err(Error::usage("speech_threshold: a probability, 0 to 1"));
    }
    Ok(Some(SpeechSettings {
        threshold,
        min_speech: length(p, min_speech)?.max(0),
    }))
}

fn silent_parts(
    p: &Project,
    env: &dyn Env,
    target: &Target<'_>,
    settings: &SilenceSettings,
    speech: Option<&SpeechSettings>,
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
    let span = target.span(p)?;
    Ok(match speech {
        Some(speech) => {
            let va = env.voice_activity(m, stream)?;
            analysis::non_speech_parts(&span, &va, settings, speech)
        }
        None => {
            let levels = env.audio_levels(m, stream)?;
            analysis::silent_parts(&span, &levels, gain, settings)
        }
    })
}

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

fn media_json(p: &Project, m: &Media, env: &dyn Env, proxies: &Path) -> Value {
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
    if let Some(c) = &m.color {
        v["color"] = json!(c);
    }
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
        let path = px.path_in(proxies);
        v["proxy"] = json!({
            "file": px.file,
            "path": path,
            "width": px.width,
            "height": px.height,
            "codec": px.codec,
            "stale": m.fresh_proxy().is_none(),
        });
        if !env.proxy_present(&path) {
            v["proxy"]["missing"] = json!(true);
        }
    }
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

pub fn clip_json(p: &Project, c: &Clip, media: Option<&Media>, track: &str) -> Value {
    let mut v = json!({
        "type": "clip",
        "id": c.id,
        "track": track,
        "media": c.media,
        "name": c.name.clone().or_else(|| media.map(|m| m.name.clone())),
        "start": c.start,
        "end": c.end(),
        "duration": c.duration,
        "start_tc": p.settings.timecode(c.start),
        "end_tc": p.settings.timecode(c.end()),
        "source_in": c.source_in,
        "source_out": c.source_at(c.end()),
        "source_in_tc": p.settings.length_timecode(c.source_in),
        "enabled": c.enabled,
    });
    if let Some(color) = c.shown_color(media) {
        v["color"] = json!(color);
    }
    if let Some(g) = &c.generator {
        v["generator"] = json!(otio::generator_kind(g));
    }
    if let Some(k) = &c.compound {
        v["compound"] = json!(otio::compound_kind(k));
    }
    if let Some(speed) = &c.speed {
        v["speed"] = speed
            .scalar()
            .map_or_else(|| json!(speed.label()), |x| json!(x));
    }
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
    let pool = p.pool();
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
                    items.push(clip_json(p, c, pool.get(c.media.as_str()).copied(), &t.id));
                }
                at = c.end();
            }
            let mut v = json!({ "id": t.id, "kind": t.kind, "locked": t.locked, "muted": t.muted });
            v["items"] = Value::Array(items);
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
            let cuts = otio::transitions::Cuts::of(p, t);
            let transitions: Vec<Value> = t
                .transitions
                .iter()
                .filter_map(|x| {
                    let at = cuts.place(p, x)?;
                    (at >= a && at <= b).then(|| {
                        json!({ "id": x.id, "name": otio::transitions::name(x), "at": at, "before": x.before,
                            "after": x.after, "from": x.from, "to": x.to, "sound": x.sound })
                    })
                })
                .collect();
            if !transitions.is_empty() {
                v["transitions"] = Value::Array(transitions);
            }
            v
        })
        .collect();
    let mut v = json!({
        "fps": fps.to_string(),
        "start": a,
        "end": b,
        "duration": p.end(),
        "duration_tc": p.settings.timecode(p.end()),
        "marks": p.marks,
        "markers": p.markers,
    });
    v["tracks"] = Value::Array(tracks);
    v
}

pub fn rate(num: i64, den: i64) -> Rational {
    Rational::new(num, den).expect("valid rate")
}

#[cfg(test)]
mod tests {
    use super::{moved_only, over};
    use crate::model::Marker;

    #[test]
    fn past_the_limit_all_but_90_percent_of_it_go() {
        assert_eq!(over((1 << 17) + 1), 13108);
    }

    #[test]
    fn a_marker_that_only_moved_changes_only_its_time() {
        let marker = |at| Marker {
            id: "k1".into(),
            at,
            duration: 1,
            name: "a".into(),
            color: "blue".into(),
            note: String::new(),
        };
        let change = moved_only(&mut marker(10), &mut marker(25), |k| &mut k.at, "at");
        assert_eq!(
            serde_json::to_value(change).unwrap(),
            serde_json::json!({"fields": {"at": {"set": {"was": 10, "is": 25}}}})
        );
    }
}
