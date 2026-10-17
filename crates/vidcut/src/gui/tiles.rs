//! The timeline's filmstrip pictures (spec §5): each tile shows the frame
//! under it.
//!
//! Painting asks for every visible tile by key: the media, the source frame
//! (project frames from the media's first frame) and the picture's height
//! in pixels. Kept pictures are textures. Above [`MEMORY`] those shown
//! least recently go first: a tile comes back from the disk cache in a
//! fraction of a millisecond. A tile not kept goes to worker threads, which take tiles in
//! the order painting asked for them and go on along a file while it has
//! more: each from the thumbnail cache on disk, where tiles are kept in
//! blocks of [`TILE_BLOCK`] frames, or decoded with the export's frame
//! picking (from a fresh proxy when there is one) and then kept on disk.
//! Until its picture is there, a tile shows the nearest kept picture of the
//! same media, else the pool's.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
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

/// Decoding threads.
const WORKERS: usize = 4;

/// Memory for kept pictures: about 12,000 tiles 56 px tall.
const MEMORY: usize = 256 << 20;

/// A tile's picture: `frame` of `media`, `height` pixels tall.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub media: String,
    pub frame: i64,
    pub height: u32,
}

/// What a worker needs to make a tile.
pub struct Job {
    key: TileKey,
    width: u32,
    fps: Rational,
    /// The frame from a fresh proxy when there is one, then from the media
    /// itself: a proxy that can't be read falls back on it.
    pieces: Vec<VideoPiece>,
    /// The tile's block file and the disk cache's limit in bytes, while the
    /// disk cache is on.
    disk: Option<(BlockFile, u64)>,
}

/// A block file of tiles, and the origin of the tiles it is for: a file
/// holding another's is made again.
#[derive(Clone, PartialEq, Eq, Hash)]
struct BlockFile {
    path: PathBuf,
    origin: Origin,
}

/// A media's file as last seen (path, size and modification time), and the
/// origin of its tiles.
struct Seen {
    path: PathBuf,
    size: u64,
    mtime_ns: i64,
    origin: Origin,
}

impl Seen {
    fn of(m: &Media) -> Self {
        Self {
            path: m.path.clone(),
            size: m.size,
            mtime_ns: m.mtime_ns,
            origin: paths::media_origin(m),
        }
    }

    /// Whether `m`'s file is the one seen.
    fn is(&self, m: &Media) -> bool {
        self.path.as_os_str() == m.path.as_os_str()
            && self.size == m.size
            && self.mtime_ns == m.mtime_ns
    }
}

struct Kept {
    texture: egui::TextureHandle,
    /// The frame (`Tiles::clock`) it was last shown in.
    used: u64,
    bytes: usize,
}

/// Jobs waiting, in the order painting asked for them, and what workers do.
#[derive(Default)]
struct Waiting {
    jobs: Vec<Job>,
    busy: HashSet<TileKey>,
    /// Tiles that couldn't be made: not asked for again.
    failed: HashSet<TileKey>,
    /// Where each worker is: the media and frame it made last.
    at: HashMap<usize, (String, i64)>,
    stop: bool,
}

struct Queue {
    waiting: Mutex<Waiting>,
    ready: Condvar,
}

/// The timeline's tile pictures and the threads making them.
pub struct Tiles {
    kept: HashMap<(String, u32), BTreeMap<i64, Kept>>,
    /// Each media's file as last seen: another file drops its pictures.
    versions: HashMap<String, Seen>,
    clock: u64,
    bytes: usize,
    budget: usize,
    /// The disk cache's limit in bytes, while it is on.
    disk: Option<u64>,
    /// Asked for while painting this frame, and kept ones shown.
    asked: RefCell<Vec<Job>>,
    shown: RefCell<Vec<TileKey>>,
    /// Asked for in the last frame, except those that couldn't be made.
    missing: HashSet<TileKey>,
    queue: Arc<Queue>,
    results: Receiver<(TileKey, Image)>,
    sender: Sender<(TileKey, Image)>,
    started: bool,
    /// The workers, joined when the tiles drop: their readers close with
    /// them, before the process ends (gotchas.md).
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
            queue: Arc::new(Queue {
                waiting: Mutex::default(),
                ready: Condvar::new(),
            }),
            results,
            sender,
            started: false,
            workers: Vec::new(),
        };
        tiles.set_prefs(prefs);
        tiles
    }

    /// The disk cache, as the preferences say.
    pub fn set_prefs(&mut self, prefs: &Thumbnails) {
        self.disk = prefs
            .disk_cache
            .then(|| prefs.cache_limit_mb.saturating_mul(1 << 20));
    }

    /// Takes in the pictures made since the last frame and keeps memory in
    /// the budget. Before painting.
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

    /// The kept picture of `key`, marking it as shown. Without one, asks for
    /// it (`job` makes the request) and returns `None`.
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

    /// The kept picture of `media` at `height` nearest `frame`, no further
    /// than `within` frames away.
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

    /// The request for `key`'s picture, `width` pixels wide: its frame from
    /// a fresh proxy if there is one, and from the media.
    pub fn job(&self, p: &Project, key: &TileKey, width: u32) -> Option<Job> {
        let m = p.media(&key.media)?;
        let piece = vidcut_core::render::media_frame(p, &key.media, key.frame).ok()?;
        let mut pieces = Vec::new();
        if let (Some(proxy), VideoPiece::Source { path, image, .. }) = (m.fresh_proxy(), &piece)
            && (path != &proxy.path || image.is_some())
        {
            let mut from_proxy = piece.clone();
            if let VideoPiece::Source { path, image, .. } = &mut from_proxy {
                *path = proxy.path.clone();
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
                // Its origin as kept when the frame began: painting digests
                // nothing.
                let origin = match self.versions.get(&key.media) {
                    Some(seen) if seen.is(m) => seen.origin.clone(),
                    _ => paths::media_origin(m),
                };
                let path = thumbcache::tile_block(&origin, key.height, block);
                (BlockFile { path, origin }, limit)
            }),
        })
    }

    /// Marks the kept pictures shown in this frame as used and hands what
    /// was asked for to the workers: the newest asks replace the old ones,
    /// so tiles scrolled away aren't made. After painting.
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

    /// Drops `media`'s pictures (its file changed).
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

    /// Over the budget, drops the pictures shown least recently, down to
    /// nine tenths of it; never those shown in the last frame.
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

    /// How many pictures are kept and their bytes.
    pub fn usage(&self) -> (usize, usize) {
        (self.kept.values().map(BTreeMap::len).sum(), self.bytes)
    }

    /// How many tiles are on their way: asked for in the last frame and not
    /// made yet, or being made.
    pub fn pending(&self) -> usize {
        let w = self.queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
        self.missing.union(&w.busy).count()
    }
}

impl Drop for Tiles {
    fn drop(&mut self) {
        {
            let mut w = self.queue.waiting.lock().unwrap_or_else(|e| e.into_inner());
            w.stop = true;
            w.jobs.clear();
            self.queue.ready.notify_all();
        }
        // Each closes its reader as it ends: before the process does.
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// The frame step between tiles `frames_per_tile` frames wide: the largest
/// power of two not above it. Each tile then holds a frame of the grid, and
/// zooming in or out by two keeps half of the frames.
pub fn tile_step(frames_per_tile: f64) -> i64 {
    // Below two frames a tile, and for nonsense (NaN), every frame.
    if frames_per_tile.is_nan() || frames_per_tile < 2.0 {
        return 1;
    }
    let exp = frames_per_tile.log2().floor().min(40.0) as u32;
    1i64 << exp
}

/// The source frame a tile shows: the clip's first one for its first tile,
/// else the first multiple of `step` at or after (backward: at or before)
/// the one at the tile's left edge, `offset` frames into the clip from
/// `source_in` at `speed` (decision D48).
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

/// A tile picture's height for a band `px` pixels tall: a multiple of 16,
/// so small changes of row height keep the pictures.
pub fn tile_height(px: f32) -> u32 {
    ((px / 16.0).ceil().max(1.0) as u32) * 16
}

/// Disk cache trims: at most one in this long, by any worker.
const TRIM_EVERY: Duration = Duration::from_secs(30);

/// A worker's reader closes after this long without decoding a tile: it
/// holds an open file, a decoder (on a GPU, a session in video memory) and
/// the frame it decoded last. The next tile opens one anew.
const READER_IDLE: Duration = Duration::from_secs(10);

/// How much longer a worker whose reader last decoded `idle` ago waits for
/// a job before the reader closes; `None` once it is time.
fn reader_waits(idle: Duration) -> Option<Duration> {
    READER_IDLE.checked_sub(idle).filter(|d| !d.is_zero())
}

/// Tiles kept for a block, as a worker last read or wrote them.
type Block = Vec<(i64, Image)>;

/// One worker: takes jobs until told to stop.
fn work(n: usize, queue: &Queue, done: &Sender<(TileKey, Image)>, ctx: &egui::Context) {
    static TRIMMED: Mutex<Option<Instant>> = Mutex::new(None);
    // Decoded on the GPU the viewer decodes on while the preferences say
    // so, read as a tile is decoded: a change reaches the tiles at once.
    let reader_for = |d: &GpuDecoding| match d.own_decoder() {
        Some(gpu) => FrameReader::decoding_on(gpu),
        None => FrameReader::default(),
    };
    let mut decoding = crate::preferences::gpu_decoding();
    // Made for a tile to decode, and when it last decoded one.
    let mut reader: Option<FrameReader> = None;
    let mut used = Instant::now();
    // Blocks read or written lately (a missing file is an empty block),
    // and tiles made but not yet on disk.
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
                    // Where it is heading: others leave what is near it.
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
                        // Idle: its file and decoder close, outside the lock.
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
            let made = job
                .pieces
                .iter()
                .find_map(|piece| reader.rgba(piece, job.fps, job.width, job.key.height).ok());
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
        // Tiles go to disk in batches: when the queue empties, or once
        // enough have gathered (it may not empty while the view moves).
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

/// The next job for worker `n`: the next frame along the file it is on,
/// when that is asked for (decoding on beats a seek); else the first asked
/// for that no other worker is near (it gets there soon), else the first.
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

/// `job`'s tile from its block on disk, or from tiles made but not yet
/// written.
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
            // A few blocks per worker: a clip's tiles share one at a time.
            if blocks.len() > 8 {
                blocks.remove(0);
            }
            blocks.len() - 1
        }
    };
    pick(&blocks[i].1)
}

/// Writes the tiles made to their block files, and now and then trims the
/// disk cache to its limit.
fn write(
    unwritten: &mut HashMap<BlockFile, (Block, u64)>,
    blocks: &mut Vec<(BlockFile, Block)>,
    trimmed: &Mutex<Option<Instant>>,
) {
    let mut limit = None;
    for (file, (tiles, l)) in unwritten.drain() {
        thumbcache::store_tiles(&file.path, &file.origin, &tiles);
        // What the worker remembers of the block is what is there now.
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
        // The first tile: the clip's first frame, whatever the grid.
        assert_eq!(tile_frame(100, 0.0, 64, true, 1.0), 100);
        // Tiles 80 frames wide (step 64) from frame 100: lefts at 180,
        // 260 and 340 show 192, 320 and 384.
        for (offset, want) in [(80.0, 192), (160.0, 320), (240.0, 384)] {
            let f = tile_frame(100, offset, 64, false, 1.0);
            assert_eq!(f, want);
            assert!(f >= 100 + offset as i64 && f < 100 + offset as i64 + 80);
        }
        // Zoomed out twice as far (step 128), the frame there is one the
        // closer view showed too: kept already.
        assert_eq!(tile_frame(100, 160.0, 128, false, 1.0), 384);
        // At twice the speed 80 frames in show 260 on; backward from 500,
        // 420 back.
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
