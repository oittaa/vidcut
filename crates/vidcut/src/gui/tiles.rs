use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use vidcut_av::export::renderer::FrameReader;
use vidcut_av::thumbs::Image;
use vidcut_core::fingerprint::Origin;
use vidcut_core::model::{Media, Project};
use vidcut_core::preferences::Thumbnails;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::paths;
use crate::preferences::GpuDecoding;
use crate::thumbcache::{self, TILE_BLOCK};

const WORKERS: usize = 4;

const MEMORY: usize = 256 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub media: String,
    pub frame: i64,
    pub height: u32,
}

pub struct Job {
    key: TileKey,
    width: u32,
    fps: Rational,
    pieces: Vec<VideoPiece>,
    disk: Option<(BlockFile, u64)>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct BlockFile {
    path: PathBuf,
    origin: Origin,
}

struct Seen {
    path: PathBuf,
    size: u64,
    mtime_ns: i64,
    origin: Origin,
    color_space: vidcut_core::model::ColorSpace,
}

impl Seen {
    fn of(m: &Media) -> Self {
        Self {
            path: m.path.clone(),
            size: m.size,
            mtime_ns: m.mtime_ns,
            origin: paths::media_origin(m),
            color_space: m.color_space,
        }
    }

    fn is(&self, m: &Media) -> bool {
        self.path.as_os_str() == m.path.as_os_str()
            && self.size == m.size
            && self.mtime_ns == m.mtime_ns
            && self.color_space == m.color_space
    }
}

struct Kept {
    texture: egui::TextureHandle,
    used: u64,
    bytes: usize,
}

#[derive(Default)]
struct Waiting {
    jobs: Vec<Job>,
    busy: HashSet<TileKey>,
    failed: HashSet<TileKey>,
    at: HashMap<usize, (String, i64)>,
    stop: bool,
}

struct Queue {
    waiting: Mutex<Waiting>,
    ready: Condvar,
    cancel: Arc<AtomicBool>,
}

pub struct Tiles {
    kept: HashMap<(String, u32), BTreeMap<i64, Kept>>,
    versions: HashMap<String, Seen>,
    clock: u64,
    bytes: usize,
    budget: usize,
    disk: Option<u64>,
    asked: RefCell<Vec<Job>>,
    shown: RefCell<Vec<TileKey>>,
    missing: HashSet<TileKey>,
    pub proxies: Arc<Path>,
    queue: Arc<Queue>,
    results: Receiver<(TileKey, Image)>,
    sender: Sender<(TileKey, Image)>,
    started: bool,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl Tiles {
    pub fn new(prefs: &Thumbnails) -> Self {
        let (sender, results) = channel();
        let mut tiles = Self {
            kept: HashMap::new(),
            versions: HashMap::new(),
            clock: 0,
            bytes: 0,
            budget: MEMORY,
            disk: None,
            asked: RefCell::default(),
            shown: RefCell::default(),
            missing: HashSet::new(),
            proxies: Arc::from(Path::new("")),
            queue: Arc::new(Queue {
                waiting: Mutex::default(),
                ready: Condvar::new(),
                cancel: Arc::new(AtomicBool::new(false)),
            }),
            results,
            sender,
            started: false,
            workers: Vec::new(),
        };
        tiles.set_prefs(prefs);
        tiles
    }

    pub fn set_prefs(&mut self, prefs: &Thumbnails) {
        self.disk = prefs
            .disk_cache
            .then(|| prefs.cache_limit_mb.saturating_mul(1 << 20));
    }

    pub fn begin_frame(&mut self, ctx: &egui::Context, p: Option<&Project>) {
        if !self.started {
            self.started = true;
            for n in 0..WORKERS {
                let (queue, done, ctx) = (self.queue.clone(), self.sender.clone(), ctx.clone());
                let worker = crate::threads::spawn(
                    format!("vidcut-tiles-{n}"),
                    crate::threads::Work::Job,
                    move || work(n, &queue, &done, &ctx),
                );
                self.workers.extend(worker);
            }
        }
        if let Some(p) = p {
            for m in &p.media {
                let seen = self.versions.get(&m.id);
                if seen.is_some_and(|s| s.is(m)) {
                    continue;
                }
                let changed = seen.is_some();
                self.versions.insert(m.id.clone(), Seen::of(m));
                if changed {
                    self.forget(&m.id);
                }
            }
        }
        while let Ok((key, image)) = self.results.try_recv() {
            let texture = ctx.load_texture(
                format!("tile-{}-{}-{}", key.media, key.frame, key.height),
                egui::ColorImage::from_rgba_unmultiplied(
                    [image.width as usize, image.height as usize],
                    &image.rgba,
                ),
                egui::TextureOptions::LINEAR,
            );
            let bytes = image.rgba.len();
            let kept = Kept {
                texture,
                used: self.clock,
                bytes,
            };
            if let Some(old) = self
                .kept
                .entry((key.media, key.height))
                .or_default()
                .insert(key.frame, kept)
            {
                self.bytes -= old.bytes;
            }
            self.bytes += bytes;
        }
        self.evict();
    }

    pub fn want(
        &self,
        key: &TileKey,
        job: impl FnOnce() -> Option<Job>,
    ) -> Option<egui::TextureId> {
        let kept = self
            .kept
            .get(&(key.media.clone(), key.height))
            .and_then(|m| m.get(&key.frame));
        if let Some(k) = kept {
            self.shown.borrow_mut().push(key.clone());
            return Some(k.texture.id());
        }
        if let Some(j) = job() {
            self.asked.borrow_mut().push(j);
        }
        None
    }

    pub fn nearest(
        &self,
        media: &str,
        height: u32,
        frame: i64,
        within: i64,
    ) -> Option<egui::TextureId> {
        let m = self.kept.get(&(media.to_string(), height))?;
        let before = m.range(frame - within..=frame).next_back();
        let after = m.range(frame..=frame + within).next();
        let best = match (before, after) {
            (Some(b), Some(a)) => {
                if frame - b.0 <= a.0 - frame {
                    b
                } else {
                    a
                }
            }
            (b, a) => b.or(a)?,
        };
        Some(best.1.texture.id())
    }

    pub fn job(&self, p: &Project, key: &TileKey, width: u32) -> Option<Job> {
        let m = p.media(&key.media)?;
        let piece = vidcut_core::render::media_frame(p, &key.media, key.frame).ok()?;
        let mut pieces = Vec::new();
        let proxy = m.fresh_proxy().map(|x| x.path_in(&self.proxies));
        if let (Some(proxy), VideoPiece::Source { path, image, .. }) = (proxy, &piece)
            && (path != &proxy || image.is_some())
        {
            let mut from_proxy = piece.clone();
            if let VideoPiece::Source { path, image, .. } = &mut from_proxy {
                *path = proxy;
                *image = None;
            }
            pieces.push(from_proxy);
        }
        pieces.push(piece);
        Some(Job {
            key: key.clone(),
            width,
            fps: p.settings.fps,
            pieces,
            disk: self.disk.map(|limit| {
                let block = key.frame.div_euclid(TILE_BLOCK);
                let origin = match self.versions.get(&key.media) {
                    Some(seen) if seen.is(m) => seen.origin.clone(),
                    _ => paths::media_origin(m),
                };
                let path = thumbcache::tile_block(&origin, key.height, block, m.color_space);
                (BlockFile { path, origin }, limit)
            }),
        })
    }

    pub fn end_frame(&mut self) {
        let clock = self.clock;
        for key in self.shown.get_mut().drain(..) {
            if let Some(k) = self
                .kept
                .get_mut(&(key.media, key.height))
                .and_then(|m| m.get_mut(&key.frame))
            {
                k.used = clock;
            }
        }
        let asked = std::mem::take(self.asked.get_mut());
        let mut w = self.queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
        self.missing = asked
            .iter()
            .map(|j| j.key.clone())
            .filter(|k| !w.failed.contains(k))
            .collect();
        let mut seen = HashSet::new();
        let jobs = asked
            .into_iter()
            .filter(|j| {
                !w.busy.contains(&j.key) && !w.failed.contains(&j.key) && seen.insert(j.key.clone())
            })
            .collect();
        w.jobs = jobs;
        if !w.jobs.is_empty() {
            self.queue.ready.notify_all();
        }
        drop(w);
        self.clock += 1;
    }

    fn forget(&mut self, media: &str) {
        let gone: Vec<(String, u32)> = self
            .kept
            .keys()
            .filter(|(m, _)| m == media)
            .cloned()
            .collect();
        for key in gone {
            if let Some(m) = self.kept.remove(&key) {
                self.bytes -= m.values().map(|k| k.bytes).sum::<usize>();
            }
        }
        let mut w = self.queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
        w.failed.retain(|k| k.media != media);
    }

    fn evict(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut all: Vec<(u64, (String, u32), i64)> = self
            .kept
            .iter()
            .flat_map(|(k, m)| m.iter().map(move |(f, kept)| (kept.used, k.clone(), *f)))
            .filter(|(used, ..)| used + 1 < self.clock)
            .collect();
        all.sort_by_key(|(used, ..)| *used);
        let target = self.budget / 10 * 9;
        for (_, key, frame) in all {
            if self.bytes <= target {
                break;
            }
            if let Some(m) = self.kept.get_mut(&key)
                && let Some(k) = m.remove(&frame)
            {
                self.bytes -= k.bytes;
            }
        }
        self.kept.retain(|_, m| !m.is_empty());
    }

    pub fn usage(&self) -> (usize, usize) {
        (self.kept.values().map(BTreeMap::len).sum(), self.bytes)
    }

    pub fn pending(&self) -> usize {
        let w = self.queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
        self.missing.union(&w.busy).count()
    }
}

impl Drop for Tiles {
    fn drop(&mut self) {
        self.queue.cancel.store(true, Ordering::SeqCst);
        {
            let mut w = self.queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
            w.stop = true;
            w.jobs.clear();
            self.queue.ready.notify_all();
        }
        if !self.workers.is_empty() {
            crate::threads::ending("the filmstrip's workers");
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        crate::threads::ending("the window's teardown");
    }
}

pub fn tile_step(frames_per_tile: f64) -> i64 {
    if frames_per_tile.is_nan() || frames_per_tile < 2.0 {
        return 1;
    }
    let exp = frames_per_tile.log2().floor().min(40.0) as u32;
    1i64 << exp
}

pub fn tile_frame(source_in: i64, offset: f64, step: i64, first: bool, speed: f64) -> i64 {
    if first || speed == 0.0 {
        return source_in;
    }
    let left = (source_in as f64 + offset * speed) / step as f64;
    let n = if speed < 0.0 {
        left.floor()
    } else {
        left.ceil()
    };
    n as i64 * step
}

pub fn media_aspect(m: &vidcut_core::model::Media) -> Option<f32> {
    let v = m.info.video.as_ref()?;
    let sar = if v.sar[0] > 0 && v.sar[1] > 0 {
        v.sar[0] as f32 / v.sar[1] as f32
    } else {
        1.0
    };
    let (w, h) = (v.width as f32 * sar, v.height.max(1) as f32);
    Some(if v.rotation % 180 == 90 { h / w } else { w / h })
}

pub fn tile_height(px: f32) -> u32 {
    ((px / 16.0).ceil().max(1.0) as u32) * 16
}

const TRIM_EVERY: Duration = Duration::from_secs(30);

const READER_IDLE: Duration = Duration::from_secs(10);

fn reader_waits(idle: Duration) -> Option<Duration> {
    READER_IDLE.checked_sub(idle).filter(|d| !d.is_zero())
}

type Block = Vec<(i64, Image)>;

fn work(n: usize, queue: &Queue, done: &Sender<(TileKey, Image)>, ctx: &egui::Context) {
    static TRIMMED: Mutex<Option<Instant>> = Mutex::new(None);
    let _cancel = vidcut_av::cancel::Guard::set(queue.cancel.clone());
    let reader_for = |d: &GpuDecoding| match d.own_decoder() {
        Some(gpu) => FrameReader::decoding_on(gpu),
        None => FrameReader::default(),
    };
    let mut decoding = crate::preferences::gpu_decoding();
    let mut reader: Option<FrameReader> = None;
    let mut used = Instant::now();
    let mut blocks: Vec<(BlockFile, Block)> = Vec::new();
    let mut unwritten: HashMap<BlockFile, (Block, u64)> = HashMap::new();
    loop {
        let job = {
            let mut w = queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if w.stop {
                    return;
                }
                if let Some(job) = next(&mut w, n) {
                    w.busy.insert(job.key.clone());
                    w.at.insert(n, (job.key.media.clone(), job.key.frame));
                    break job;
                }
                if !unwritten.is_empty() {
                    drop(w);
                    write(&mut unwritten, &mut blocks, &TRIMMED);
                    w = queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                if reader.is_none() {
                    w = queue.ready.wait(w).unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                match reader_waits(used.elapsed()) {
                    Some(left) => {
                        w = match queue.ready.wait_timeout(w, left) {
                            Ok((w, _)) => w,
                            Err(e) => e.into_inner().0,
                        };
                    }
                    None => {
                        drop(w);
                        reader = None;
                        w = queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
                    }
                }
            }
        };
        let picture = from_disk(&job, &mut blocks, &unwritten).or_else(|| {
            let now = crate::preferences::gpu_decoding();
            if now != decoding {
                reader = None;
                decoding = now;
            }
            let reader = reader.get_or_insert_with(|| reader_for(&decoding));
            let beside = vidcut_av::first::Beside::ask();
            let made = job
                .pieces
                .iter()
                .find_map(|piece| reader.rgba(piece, job.fps, job.width, job.key.height).ok());
            drop(beside);
            used = Instant::now();
            let made = made?;
            if let Some((file, limit)) = &job.disk {
                let pending = unwritten
                    .entry(file.clone())
                    .or_insert((Vec::new(), *limit));
                pending.0.push((job.key.frame, made.clone()));
            }
            Some(made)
        });
        let mut w = queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
        w.busy.remove(&job.key);
        match picture {
            Some(p) => {
                drop(w);
                if done.send((job.key, p)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
            None => {
                w.failed.insert(job.key);
            }
        }
        if unwritten
            .values()
            .map(|(tiles, _)| tiles.len())
            .sum::<usize>()
            >= 32
        {
            write(&mut unwritten, &mut blocks, &TRIMMED);
        }
    }
}

fn next(w: &mut Waiting, n: usize) -> Option<Job> {
    if w.jobs.is_empty() {
        return None;
    }
    let near = |j: &Job, (media, frame): &(String, i64)| {
        let reach = (j.fps.as_f64() * 4.0) as i64;
        &j.key.media == media && (j.key.frame - frame).abs() <= reach
    };
    let along = w.at.get(&n).and_then(|(media, frame)| {
        w.jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| &j.key.media == media && j.key.frame >= *frame)
            .filter(|(_, j)| near(j, &(media.clone(), *frame)))
            .min_by_key(|(_, j)| j.key.frame)
            .map(|(i, _)| i)
    });
    let free = || {
        w.jobs
            .iter()
            .position(|j| !w.at.iter().any(|(other, at)| *other != n && near(j, at)))
    };
    let i = along.or_else(free).unwrap_or(0);
    Some(w.jobs.remove(i))
}

fn from_disk(
    job: &Job,
    blocks: &mut Vec<(BlockFile, Block)>,
    unwritten: &HashMap<BlockFile, (Block, u64)>,
) -> Option<Image> {
    let (file, _) = job.disk.as_ref()?;
    let pick = |tiles: &Block| {
        tiles
            .iter()
            .find(|(f, i)| *f == job.key.frame && i.width == job.width)
            .map(|(_, i)| i.clone())
    };
    if let Some(found) = unwritten.get(file).and_then(|(tiles, _)| pick(tiles)) {
        return Some(found);
    }
    let i = match blocks.iter().position(|(f, _)| f == file) {
        Some(i) => i,
        None => {
            blocks.push((
                file.clone(),
                thumbcache::load_tiles(&file.path, &file.origin).unwrap_or_default(),
            ));
            if blocks.len() > 8 {
                blocks.remove(0);
            }
            blocks.len() - 1
        }
    };
    pick(&blocks[i].1)
}

fn write(
    unwritten: &mut HashMap<BlockFile, (Block, u64)>,
    blocks: &mut Vec<(BlockFile, Block)>,
    trimmed: &Mutex<Option<Instant>>,
) {
    let mut limit = None;
    for (file, (tiles, l)) in unwritten.drain() {
        thumbcache::store_tiles(&file.path, &file.origin, &tiles);
        blocks.retain(|(f, _)| f != &file);
        limit = Some(l);
    }
    let Some(limit) = limit else { return };
    let mut last = trimmed.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_none_or(|t| t.elapsed() >= TRIM_EVERY) {
        *last = Some(Instant::now());
        drop(last);
        thumbcache::evict(&thumbcache::dir(), limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_powers_of_two_a_tile_wide_at_most() {
        assert_eq!(tile_step(0.3), 1);
        assert_eq!(tile_step(1.0), 1);
        assert_eq!(tile_step(2.0), 2);
        assert_eq!(tile_step(7.9), 4);
        assert_eq!(tile_step(8.0), 8);
        assert_eq!(tile_step(300.0), 256);
        assert_eq!(tile_step(f64::NAN), 1);
    }

    #[test]
    fn a_tile_shows_a_grid_frame_inside_it() {
        assert_eq!(tile_frame(100, 0.0, 64, true, 1.0), 100);
        for (offset, want) in [(80.0, 192), (160.0, 320), (240.0, 384)] {
            let f = tile_frame(100, offset, 64, false, 1.0);
            assert_eq!(f, want);
            assert!(f >= 100 + offset as i64 && f < 100 + offset as i64 + 80);
        }
        assert_eq!(tile_frame(100, 160.0, 128, false, 1.0), 384);
        assert_eq!(tile_frame(100, 80.0, 64, false, 2.0), 320);
        assert_eq!(tile_frame(500, 80.0, 64, false, -1.0), 384);
        assert_eq!(tile_frame(500, 80.0, 64, false, 0.0), 500);
    }

    #[test]
    fn an_idle_reader_closes_ten_seconds_after_its_last_tile() {
        assert_eq!(
            reader_waits(Duration::from_secs(4)),
            Some(Duration::from_secs(6))
        );
        assert_eq!(reader_waits(Duration::from_secs(10)), None);
    }

    #[test]
    fn heights_round_up_to_sixteen() {
        assert_eq!(tile_height(1.0), 16);
        assert_eq!(tile_height(36.0), 48);
        assert_eq!(tile_height(48.0), 48);
        assert_eq!(tile_height(49.0), 64);
    }
}
