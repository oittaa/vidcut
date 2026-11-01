use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut_core::model::{Project, RenderCache as Mode};
use vidcut_core::preferences::{Gpu, PlaybackFiles, Preferences, Proxies, RenderCachePrefs};
use vidcut_core::render::RenderPlan;
use vidcut_core::render_cache::{Cache, Index, Key, Look, Part, Setup};

use super::Msg;
use crate::threads::Work;

const SECONDS_EACH: i64 = 2;

pub(crate) enum Event {
    Scanned(Index),
    Slow {
        job: Arc<AtomicBool>,
        decodes: HashMap<PathBuf, bool>,
    },
    Written,
    Ended {
        job: Arc<AtomicBool>,
        done: bool,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Mark {
    pub(super) rev: Option<u64>,
    pub(super) at: i64,
    pub(super) playing: bool,
    pub(super) busy: bool,
}

pub(super) struct Background {
    tx: Sender<Msg>,
    cache: Arc<Mutex<Cache>>,
    prefs: RenderCachePrefs,
    proxies: Proxies,
    files: PlaybackFiles,
    project_dir: Option<PathBuf>,
    gpu: Gpu,
    index: Scan,
    decodes: Arc<HashMap<PathBuf, bool>>,
    base: Option<Arc<RenderPlan>>,
    mark: Option<Mark>,
    touched: u64,
    seen: u64,
    quiet: Instant,
    running: Option<Arc<AtomicBool>>,
    done: bool,
    error: Option<String>,
    line: Arc<Vec<Part>>,
}

#[derive(PartialEq)]
enum Scan {
    None,
    Running,
    Done,
}

fn slow_media(
    project: &Project,
    proxies: &Proxies,
    decodes: &HashMap<PathBuf, bool>,
    id: &str,
) -> bool {
    project.media(id).is_some_and(|m| {
        decodes.get(&m.path).copied().unwrap_or(false)
            || (proxies.wants(m) && !(project.settings.use_proxies && m.fresh_proxy().is_some()))
    })
}

impl Background {
    pub(super) fn new(tx: Sender<Msg>, prefs: &Preferences) -> Self {
        Self {
            tx,
            cache: Arc::default(),
            prefs: prefs.render_cache,
            proxies: prefs.proxies.clone(),
            files: prefs.playback_files.clone(),
            project_dir: None,
            gpu: prefs.gpu.clone(),
            index: Scan::None,
            decodes: Arc::default(),
            base: None,
            mark: None,
            touched: 0,
            seen: 0,
            quiet: Instant::now(),
            running: None,
            done: false,
            error: None,
            line: Arc::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn look(&self, project: &Project) -> Look {
        Look::of(
            project,
            (self.proxies.size, self.proxies.codec),
            self.prefs.codec,
        )
    }

    fn dir(&self) -> PathBuf {
        let base = self
            .project_dir
            .clone()
            .unwrap_or_else(crate::paths::cache_dir);
        self.files.render_cache_in(&base)
    }

    pub(super) fn open(&mut self, project_file: &Path) {
        let dir = project_file.parent().map(Path::to_path_buf);
        if self.project_dir != dir {
            self.stop();
            self.project_dir = dir;
            self.moved();
        }
    }

    fn moved(&mut self) {
        self.index = Scan::None;
        self.done = false;
        self.error = None;
        *self.lock().index_mut() = Index::default();
    }

    pub(super) fn follow(&mut self, prefs: &Preferences) -> bool {
        let gpu = self.gpu != prefs.gpu;
        let folder = self.files != prefs.playback_files;
        let changed =
            gpu || folder || self.prefs != prefs.render_cache || self.proxies != prefs.proxies;
        if changed {
            self.stop();
            self.prefs = prefs.render_cache;
            self.proxies = prefs.proxies.clone();
            self.done = false;
        }
        if folder {
            let before = self.dir();
            self.files = prefs.playback_files.clone();
            if self.dir() != before {
                self.moved();
            }
        }
        if gpu {
            self.gpu = prefs.gpu.clone();
            self.decodes = Arc::default();
        }
        changed
    }

    pub(super) fn touch(&mut self) {
        self.touched += 1;
    }

    pub(super) fn rendering(&self) -> bool {
        self.running.is_some()
    }

    pub(super) fn mark(&mut self, project: &Project, stretch: (i64, i64)) -> bool {
        let Some(base) = self.base.clone() else {
            return false;
        };
        self.indexed();
        let look = self.look(project);
        let slow = |id: &str| slow_media(project, &self.proxies, &self.decodes, id);
        let setup = Setup {
            mode: project.settings.render_cache,
            prefs: &self.prefs,
            project,
            slow: &slow,
            upscaled: &vidcut_av::export::video::ai_size,
            look: &look,
        };
        let marked = self.lock().mark(&base, &setup, stretch);
        if marked {
            self.done = false;
        }
        marked
    }

    fn stop(&mut self) {
        if let Some(cancel) = self.running.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn line(&self) -> Arc<Vec<Part>> {
        self.line.clone()
    }

    fn show(&mut self, parts: Vec<Part>) -> bool {
        let changed = *self.line != parts;
        if changed {
            self.line = Arc::new(parts);
        }
        changed
    }

    pub(super) fn apply(&mut self, plan: &mut RenderPlan, project: &Project) -> bool {
        if self.base.as_deref() != Some(plan) {
            self.base = Some(Arc::new(plan.clone()));
            self.done = false;
        }
        if project.settings.render_cache == Mode::Off {
            self.stop();
            return self.show(Vec::new());
        }
        if self.index != Scan::Done {
            if self.index == Scan::None {
                self.scan();
            }
            return self.show(Vec::new());
        }
        let look = self.look(project);
        let decodes = self.decodes.clone();
        let slow = |id: &str| slow_media(project, &self.proxies, &decodes, id);
        let setup = Setup {
            mode: project.settings.render_cache,
            prefs: &self.prefs,
            project,
            slow: &slow,
            upscaled: &vidcut_av::export::video::ai_size,
            look: &look,
        };
        let mut cache = self.lock();
        cache.apply(plan, &setup, &Path::is_file);
        let parts = match &self.base {
            Some(base) => cache.parts(base, &setup),
            None => Vec::new(),
        };
        drop(cache);
        self.show(parts)
    }

    fn scan(&mut self) {
        self.index = Scan::Running;
        let tx = self.tx.clone();
        let dir = self.dir();
        let _ = crate::threads::spawn("vidcut-render-index", Work::Job, move || {
            let index = Index::scan(&dir);
            let _ = tx.send(Msg::RenderCache(Event::Scanned(index)));
        });
    }

    pub(super) fn event(&mut self, e: Event) -> bool {
        match e {
            Event::Scanned(index) => {
                if self.index == Scan::Done || index.folder() != Some(&self.dir()) {
                    return false;
                }
                *self.lock().index_mut() = index;
                self.index = Scan::Done;
                true
            }
            Event::Slow { job, decodes } => {
                if !self.running.as_ref().is_some_and(|r| Arc::ptr_eq(r, &job)) {
                    return false;
                }
                let changed = *self.decodes != decodes;
                self.decodes = Arc::new(decodes);
                changed
            }
            Event::Written => {
                self.error = None;
                true
            }
            Event::Ended { job, done, error } => {
                if self.running.as_ref().is_some_and(|r| Arc::ptr_eq(r, &job)) {
                    self.running = None;
                    self.done |= done || error.is_some();
                    self.error = error;
                }
                false
            }
        }
    }

    pub(super) fn tick(
        &mut self,
        now: Instant,
        mark: Mark,
        project: Option<&Project>,
    ) -> Option<Instant> {
        let active = mark.playing || mark.busy;
        if self.mark.as_ref() != Some(&mark) || self.seen != self.touched || active {
            self.mark = Some(mark.clone());
            self.seen = self.touched;
            self.quiet = now;
            self.stop();
        }
        let project = project?;
        let off = project.settings.render_cache == Mode::Off || !self.prefs.background;
        if off || active || self.done || self.running.is_some() || self.index != Scan::Done {
            return None;
        }
        let base = self.base.clone()?;
        let due = self.quiet + Duration::from_secs(u64::from(self.prefs.after_secs));
        if now < due {
            return Some(due);
        }
        self.start(project, base, mark.at);
        None
    }

    fn work(&self, project: &Project, base: Arc<RenderPlan>, from: i64) -> Render {
        Render {
            cache: self.cache.clone(),
            base,
            project: Arc::new(project.clone()),
            prefs: self.prefs,
            proxies: self.proxies.clone(),
            decodes: self.decodes.clone(),
            look: self.look(project),
            dir: self.dir(),
            from,
            cancel: Arc::new(AtomicBool::new(false)),
            tx: Some(self.tx.clone()),
        }
    }

    fn start(&mut self, project: &Project, base: Arc<RenderPlan>, from: i64) {
        let work = self.work(project, base, from);
        let cancel = work.cancel.clone();
        if crate::threads::spawn("vidcut-render-cache", Work::Job, move || work.run()).is_ok() {
            self.running = Some(cancel);
        }
    }

    pub(super) fn render_now(&mut self, project: &Project, from: i64) -> Value {
        self.stopped();
        if let Some(base) = self.base.clone()
            && project.settings.render_cache != Mode::Off
        {
            self.start(project, base, from);
        }
        self.status(project)
    }

    pub(super) fn status(&mut self, project: &Project) -> Value {
        self.indexed();
        let mode = project.settings.render_cache;
        let look = self.look(project);
        let slow = |id: &str| slow_media(project, &self.proxies, &self.decodes, id);
        let setup = Setup {
            mode,
            prefs: &self.prefs,
            project,
            slow: &slow,
            upscaled: &vidcut_av::export::video::ai_size,
            look: &look,
        };
        let mut cache = self.lock();
        let parts: Vec<Value> = match &self.base {
            Some(base) => cache
                .parts(base, &setup)
                .into_iter()
                .map(|p| {
                    json!({
                        "start": p.start,
                        "frames": p.frames,
                        "cached": p.cached_frames(),
                        "cached_ranges": p.cached,
                        "reasons": super::stats::reason_names(&p.reasons),
                    })
                })
                .collect(),
            None => Vec::new(),
        };
        let index = cache.index();
        json!({
            "mode": mode,
            "background": self.prefs.background,
            "after_secs": self.prefs.after_secs,
            "rendering": self.running.is_some(),
            "size": [look.width, look.height],
            "folder": self.dir(),
            "parts": parts,
            "files": index.files().count(),
            "frames": index.frames(),
            "bytes": index.bytes(),
            "error": self.error,
        })
    }

    fn indexed(&mut self) {
        if self.index != Scan::Done {
            *self.lock().index_mut() = Index::scan(&self.dir());
            self.index = Scan::Done;
        }
    }

    fn stopped(&mut self) {
        self.stop();
        self.done = false;
        self.indexed();
    }

    pub(super) fn delete_all(&mut self) -> usize {
        self.stopped();
        self.lock().index_mut().clear();
        let mut n = 0;
        if let Ok(listing) = std::fs::read_dir(self.dir()) {
            for e in listing.flatten() {
                let path = e.path();
                if path.is_file() && std::fs::remove_file(&path).is_ok() {
                    n += usize::from(
                        path.extension()
                            .is_some_and(|x| x == vidcut_core::render_cache::FILE),
                    );
                }
            }
        }
        n
    }

    pub(super) fn delete_frames(&mut self, project: &Project, ranges: &[Span]) -> usize {
        self.stopped();
        let Some(base) = self.base.clone() else {
            return 0;
        };
        let look = self.look(project);
        let slow = |id: &str| slow_media(project, &self.proxies, &self.decodes, id);
        let setup = Setup {
            mode: Mode::Smart,
            prefs: &self.prefs,
            project,
            slow: &slow,
            upscaled: &vidcut_av::export::video::ai_size,
            look: &look,
        };
        let mut cache = self.lock();
        let keys = keys_in(&mut cache, &base, &setup, ranges);
        cache.index_mut().drop_with(&keys)
    }

    pub(super) fn delete_unused(&mut self, project: &Project) -> usize {
        self.stopped();
        let Some(base) = self.base.clone() else {
            return 0;
        };
        let look = self.look(project);
        let slow = |id: &str| slow_media(project, &self.proxies, &self.decodes, id);
        let setup = Setup {
            mode: Mode::Smart,
            prefs: &self.prefs,
            project,
            slow: &slow,
            upscaled: &vidcut_av::export::video::ai_size,
            look: &look,
        };
        let mut cache = self.lock();
        let all = [(base.range.0, base.range.1, None)];
        let keys = keys_in(&mut cache, &base, &setup, &all);
        cache.index_mut().keep_only(&keys)
    }
}

pub(super) type Span = (i64, i64, Option<Vec<String>>);

pub(crate) fn render_all(
    (project, file): (&Project, &Path),
    show_super_scale: bool,
    cancel: Arc<AtomicBool>,
) -> vidcut_core::Result<Value> {
    let prefs = crate::preferences::load().unwrap_or_default();
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut background = Background::new(tx, &prefs);
    background.open(file);
    background.indexed();
    let end = project.end();
    if end > 0 && project.settings.render_cache != Mode::Off {
        let (plan, _) = vidcut_core::render::flatten_lanes(project, 0, end)?;
        let base = Arc::new(super::shown(plan, show_super_scale));
        background.base = Some(base.clone());
        let mut work = background.work(project, base.clone(), base.range.0);
        work.cancel = cancel;
        work.tx = None;
        match work.render() {
            Ok(true) => {}
            Ok(false) => return Err(super::cancelled()),
            Err(e) => return Err(vidcut_core::Error::new(vidcut_core::ErrorCode::Media, e)),
        }
    }
    Ok(background.status(project))
}

fn keys_in(cache: &mut Cache, base: &RenderPlan, setup: &Setup, spans: &[Span]) -> HashSet<Key> {
    let mut out = HashSet::new();
    let mut t = base.range.0;
    for piece in &base.video {
        let n = piece.frames();
        for (a, b, media) in spans.iter().filter(|(a, b, _)| *a < t + n && *b > t) {
            for keys in cache.names(piece, setup, base, media.as_deref()) {
                for (k, key) in keys.iter().enumerate() {
                    if (*a..*b).contains(&(t + k as i64)) {
                        out.insert(*key);
                    }
                }
            }
        }
        t += n;
    }
    out
}

struct Render {
    cache: Arc<Mutex<Cache>>,
    base: Arc<RenderPlan>,
    project: Arc<Project>,
    prefs: RenderCachePrefs,
    proxies: Proxies,
    decodes: Arc<HashMap<PathBuf, bool>>,
    look: Look,
    dir: PathBuf,
    from: i64,
    cancel: Arc<AtomicBool>,
    tx: Option<Sender<Msg>>,
}

impl Render {
    fn send(&self, e: Event) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::RenderCache(e));
        }
    }

    fn lock(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(self) {
        let ended = self.render();
        let (done, error) = match ended {
            Ok(done) => (done, None),
            Err(e) => (false, Some(e)),
        };
        self.send(Event::Ended {
            job: self.cancel.clone(),
            done,
            error,
        });
    }

    fn render(&self) -> Result<bool, String> {
        let gpu = crate::preferences::gpu_decoding().own_decoder();
        let mut decodes = (*self.decodes).clone();
        let mut found = false;
        for m in &self.project.media {
            let Some(v) = m.info.video.as_ref().filter(|v| !v.still) else {
                continue;
            };
            if !decodes.contains_key(&m.path) {
                let slow = vidcut_av::render_cache::slow(gpu.as_ref(), &m.path, &v.codec);
                decodes.insert(m.path.clone(), slow);
                found = true;
            }
        }
        if found {
            self.send(Event::Slow {
                job: self.cancel.clone(),
                decodes: decodes.clone(),
            });
        }
        let prefs = crate::preferences::load().unwrap_or_default();
        let proxies = self
            .project
            .settings
            .use_proxies
            .then_some(self.proxies.codec);
        let encoders = crate::env::cache_encoders(
            &[(self.prefs.codec, proxies)],
            prefs.gpu.acceleration,
            prefs.gpu.acceleration && prefs.gpu.decoding,
        )
        .concat();
        let project = &*self.project;
        let slow = |id: &str| slow_media(project, &self.proxies, &decodes, id);
        let setup = Setup {
            mode: project.settings.render_cache,
            prefs: &self.prefs,
            project,
            slow: &slow,
            upscaled: &vidcut_av::export::video::ai_size,
            look: &self.look,
        };
        let most = (self.base.fps.as_f64() * SECONDS_EACH as f64)
            .round()
            .max(1.0) as i64;
        let mut from = self.from;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let next = self.lock().next(&self.base, &setup, from, most);
            let Some(job) = next else {
                return Ok(true);
            };
            full(&self.dir)?;
            let written = vidcut_av::render_cache::write(
                (&job.plan, job.clip),
                &job.keys,
                &encoders,
                gpu.as_ref(),
                &self.dir,
                &self.cancel,
            );
            match written {
                Ok(w) => {
                    self.lock().index_mut().add(&w.path, &job.keys);
                    self.send(Event::Written);
                    from = match job.clip {
                        true => job.start,
                        false => job.start + job.keys.len() as i64,
                    };
                }
                Err(_) if self.cancel.load(Ordering::Relaxed) => return Ok(false),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

fn full(dir: &Path) -> Result<(), String> {
    let keep = vidcut_core::disk_cache::KEEP_FREE_PERCENT;
    match crate::system::disk(dir) {
        Some(d) if d.free < d.size / 100 * keep => Err(format!(
            "{}: the disk has less than {keep}% free; free some space, or set \
             playback_files.render_cache to another disk",
            dir.display()
        )),
        _ => Ok(()),
    }
}
