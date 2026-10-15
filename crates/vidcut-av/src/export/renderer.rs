//! Pictures that follow a moving position, such as a paused viewer being
//! stepped or a trim's two-up, each frame decoded once
//! (docs/decisions/preview.md §9).
//!
//! Forward, the file and its decoder stay open and the next frame decodes
//! on from the one before, as every player does. Every frame decoded on the
//! way to the one asked for is kept, so going back over them finds them,
//! and the frames after the position render ahead of it as the caller asks
//! ([`StillRenderer::ahead`]). Back, there is nothing to decode on
//! from: a seek, and decoding from the keyframe before, can't be avoided.
//! So a window renders at once, from a keyframe up to a frame, and is kept,
//! on another thread ([`Prefetch`]) ahead of need: wherever the position
//! is, at least two seconds behind it, and going back, a whole window more,
//! so stepping and playing back don't wait. None renders while the position
//! runs forward, and one stops once the position has moved on from it. A
//! window starts on a keyframe, so nothing in it decodes twice; it reaches
//! back over more keyframes to hold at least two seconds where they are
//! close, and a memory budget caps it. Over the budget, the pictures looked
//! at least recently go first.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

use crate::error::{Error, Result};
use crate::ffmpeg::{format, frame};
use crate::thumbs::Image;

use super::video::VideoFrames;

/// Memory for kept pictures unless the caller sets it: two windows back
/// share it.
pub const DEFAULT_BUDGET: usize = 256 << 20;

/// Seconds a window back holds at least, where keyframes are close.
const WINDOW_SECS: f64 = 2.0;

/// Renders frames as packed RGBA (the picture fit inside the size, padded
/// black), keeping what it renders within its budget.
pub struct StillRenderer {
    /// The frames reader for one rate and size.
    frames: Option<(Rational, u32, u32, VideoFrames)>,
    shared: Arc<Shared>,
    /// The source and position asked for last.
    last: Option<(String, i64)>,
    /// Whether the position moved back with the last request.
    back: bool,
    prefetch: Option<Prefetch>,
    /// Where files decode, when on a GPU ([`decoding_on`](Self::decoding_on)).
    decoder: Option<crate::hw::GpuDecoder>,
    /// The frame of a video the reader decoded last: the piece, its source
    /// and the position. The frames after it decode on from there.
    decoded: Option<(VideoPiece, String, i64)>,
}

/// Kept pictures, shared with prefetches and [`Kept`].
struct Shared {
    store: Mutex<Store>,
    /// A prefetch rendered a frame, or ended.
    landed: Condvar,
}

struct Store {
    /// Pictures by source and position, with when each was last kept or
    /// looked at (`clock`).
    pictures: HashMap<(String, i64), (Arc<Image>, u64)>,
    clock: u64,
    bytes: usize,
    budget: usize,
    /// A prefetch planned or rendering: its source and the frames it
    /// renders, first to last (the first is the last until its window is
    /// known).
    prefetching: Option<(String, i64, i64)>,
    /// The source and position asked for last (`None` once the renderer
    /// is gone), and whether the position runs forward: a window is worth
    /// rendering while the position is near it and doesn't.
    at: Option<(String, i64)>,
    forward: bool,
    /// While the prefetch renders: what stops its decoding, between frames
    /// (its cancel flag), and its longest window.
    stopper: Option<(Arc<AtomicBool>, i64)>,
}

impl Store {
    /// Whether frames `first..=last` of `key` are still worth rendering
    /// behind the position: it is on that source, doesn't run forward, and
    /// is at or above `first` and less than a window (`max`) past `last`.
    fn wants(&self, key: &str, first: i64, last: i64, max: i64) -> bool {
        !self.forward
            && self
                .at
                .as_ref()
                .is_some_and(|(k, p)| k == key && first <= *p && *p <= last + max)
    }

    /// Stops the prefetch rendering now once the position has moved on.
    fn recheck(&self) {
        if let (Some((key, first, last)), Some((stop, max))) = (&self.prefetching, &self.stopper) {
            // Its first frame isn't known yet: a window's length at most.
            let first = if first < last { *first } else { last + 1 - max };
            if !self.wants(key, first, *last, *max) {
                stop.store(true, Ordering::Relaxed);
            }
        }
    }
}

impl Default for StillRenderer {
    fn default() -> Self {
        Self::with_budget(DEFAULT_BUDGET)
    }
}

impl StillRenderer {
    /// A renderer keeping up to `budget` bytes of pictures.
    pub fn with_budget(budget: usize) -> Self {
        Self {
            frames: None,
            shared: Arc::new(Shared {
                store: Mutex::new(Store {
                    pictures: HashMap::new(),
                    clock: 0,
                    bytes: 0,
                    budget,
                    prefetching: None,
                    at: None,
                    forward: false,
                    stopper: None,
                }),
                landed: Condvar::new(),
            }),
            last: None,
            back: false,
            prefetch: None,
            decoder: None,
            decoded: None,
        }
    }

    /// Files decode on `gpu` where it decodes their streams, the frames
    /// shrunk there before they come down ([`VideoFrames::decode_on`]).
    pub fn decoding_on(mut self, gpu: crate::hw::GpuDecoder) -> Self {
        self.decoder = Some(gpu);
        self
    }

    /// Renders `piece`'s first frame at `width`×`height`. `floor` is the
    /// first source position worth keeping frames behind it for: where the
    /// shown stretch of the file starts (0: the whole file). A dissolve's
    /// pictures are each rendered, and kept, as a piece of their own, then
    /// mixed (decision D50).
    pub fn rgba(
        &mut self,
        piece: &VideoPiece,
        floor: i64,
        fps: Rational,
        width: u32,
        height: u32,
    ) -> Result<Arc<Image>> {
        if let VideoPiece::Mix { .. } = piece {
            let mut mixed: Option<Arc<Image>> = None;
            for (layer, ramp) in piece.layers() {
                let img = self.rgba(&layer.part(0, 1), 0, fps, width, height)?;
                mixed = Some(match (mixed, ramp) {
                    (Some(under), Some(ramp)) => {
                        Arc::new(mixed_image(&under, &img, ramp.weight(0)))
                    }
                    _ => img,
                });
            }
            return mixed.ok_or_else(|| Error::Invalid("a mix of nothing".into()));
        }
        crate::cancel::stop()?;
        crate::init();
        // A prefetch nobody took isn't coming: nothing waits for it.
        self.prefetch = None;
        let same =
            matches!(&self.frames, Some((f, w, h, _)) if (*f, *w, *h) == (fps, width, height));
        if !same {
            let frames = frames(fps, (width, height), RGBA, self.decoder.as_ref());
            self.frames = Some((fps, width, height, frames));
            self.decoded = None;
        }
        let VideoPiece::Source {
            source_frame: pos,
            image,
            ..
        } = piece
        else {
            self.last = None;
            self.decoded = None;
            return Ok(Arc::new(self.render(piece, 1)?.remove(0)));
        };
        let (pos, video) = (*pos, image.is_none());
        let key = source_key(piece, fps, width, height);
        self.back = match &self.last {
            Some((k, p)) if *k == key && pos != *p => pos < *p,
            Some((k, _)) if *k == key => self.back,
            _ => false,
        };
        self.last = Some((key.clone(), pos));
        {
            let mut store = self.shared.lock();
            store.at = Some((key.clone(), pos));
            store.recheck();
        }
        let floor = floor.min(pos);
        let (min, max) = window_sizes(self.shared.budget(), width, height, fps);
        let img = match self.shared.get_or_wait(&key, pos, max) {
            Some(img) => img,
            None if self.back && video => {
                let (.., frames) = self.frames.as_mut().expect("set above");
                let start = window(frames, piece, pos, floor, fps, min, max, false)?;
                self.render_kept(piece, &key, start, pos)?
            }
            None => {
                // Forward a little from the frame decoded last, every frame
                // on the way decodes anyway: each is kept.
                let from = match &self.decoded {
                    Some((_, k, d)) if video && *k == key && *d < pos && pos - d <= reach(fps) => {
                        d + 1
                    }
                    _ => pos,
                };
                self.render_kept(piece, &key, from, pos)?
            }
        };
        if video {
            self.plan_prefetch(piece, &key, pos, floor, (min, max));
        }
        Ok(img)
    }

    /// Renders `piece`'s frames `from..=to`, keeps them all and returns the
    /// last: the reader stands there then.
    fn render_kept(
        &mut self,
        piece: &VideoPiece,
        key: &str,
        from: i64,
        to: i64,
    ) -> Result<Arc<Image>> {
        let n = to - from + 1;
        let mut last = None;
        let rendered = self.render(&at(piece, from, n), n);
        self.decoded = None;
        for (j, img) in (from..).zip(rendered?) {
            let img = Arc::new(img);
            self.shared.put(key, j, img.clone());
            last = Some(img);
        }
        if matches!(piece, VideoPiece::Source { image: None, .. }) {
            self.decoded = Some((piece.clone(), key.to_string(), to));
        }
        last.ok_or_else(|| Error::Invalid("no frame to render".into()))
    }

    /// The window to render behind the last frame asked for: run it on
    /// another thread, while this renderer goes on. Untaken, it is dropped
    /// with the next request.
    pub fn take_prefetch(&mut self) -> Option<Prefetch> {
        self.prefetch.take()
    }

    /// Whether the position runs forward (a run, not steps): no window
    /// renders behind it then: none is planned, and those planned stop.
    pub fn running_forward(&mut self, on: bool) {
        let mut store = self.shared.lock();
        store.forward = on;
        store.recheck();
        drop(store);
        if on {
            self.prefetch = None;
        }
    }

    /// Renders and keeps the frame after the one the reader decoded last,
    /// if that is at or before `until` and the reader stands at or past the
    /// position asked for last, on its source: frames render ahead of the
    /// position, so a run or a step forward finds them kept, whatever pace
    /// the decoder keeps from frame to frame. The frame it rendered.
    pub fn ahead(&mut self, until: i64) -> Result<Option<i64>> {
        let Some((piece, key, at)) = self.decoded.clone() else {
            return Ok(None);
        };
        let here = self
            .last
            .as_ref()
            .is_some_and(|(k, pos)| *k == key && at >= *pos);
        if !here || at + 1 > until {
            return Ok(None);
        }
        crate::cancel::stop()?;
        self.render_kept(&piece, &key, at + 1, at + 1)?;
        Ok(Some(at + 1))
    }

    /// Its kept pictures, to look up from another thread.
    pub fn kept(&self) -> Kept {
        Kept(self.shared.clone())
    }

    /// Plans the window before the frames kept below `pos` while too few of
    /// them are left: two seconds wherever the position is, and going back
    /// a whole window, so the next one back renders while the position goes
    /// through this one. Never below `floor`, nor while the position runs
    /// forward; one prefetch at a time.
    fn plan_prefetch(
        &mut self,
        piece: &VideoPiece,
        key: &str,
        pos: i64,
        floor: i64,
        (min, max): (i64, i64),
    ) {
        let Some((fps, width, height, _)) = &self.frames else {
            return;
        };
        let (fps, width, height) = (*fps, *width, *height);
        let mut store = self.shared.lock();
        if store.forward || store.prefetching.is_some() || self.prefetch.is_some() {
            return;
        }
        let mut low = pos;
        while low > floor && store.pictures.contains_key(&(key.to_string(), low - 1)) {
            low -= 1;
        }
        let wanted = if self.back { max } else { min };
        if low <= floor || pos - low >= wanted {
            return;
        }
        store.prefetching = Some((key.to_string(), low - 1, low - 1));
        drop(store);
        self.prefetch = Some(Prefetch {
            piece: at(piece, low - 1, 1),
            key: key.to_string(),
            end: low - 1,
            floor,
            fps,
            width,
            height,
            min,
            max,
            shared: self.shared.clone(),
            decoder: self.decoder.clone(),
        });
    }

    /// The first `n` frames of `piece`, in order, read on from where the
    /// renderer is when they follow it closely.
    fn render(&mut self, piece: &VideoPiece, n: i64) -> Result<Vec<Image>> {
        let (_, width, height, frames) = self.frames.as_mut().expect("set by rgba");
        let (width, height) = (*width, *height);
        frames.read_piece(piece);
        (0..n)
            .map(|_| {
                let frame = frames
                    .next_frame()?
                    .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
                Ok(image(&frame, width, height))
            })
            .collect()
    }
}

/// Single frames as small pictures, in the order they are asked for: a
/// timeline's filmstrip tiles. Nothing is kept. The file stays open, so a
/// frame shortly after the one before decodes on from it instead of seeking;
/// ask for a file's frames in order.
#[derive(Default)]
pub struct FrameReader {
    frames: Option<(Rational, u32, u32, VideoFrames)>,
    /// Where files decode, when on a GPU ([`decoding_on`](Self::decoding_on)).
    decoder: Option<crate::hw::GpuDecoder>,
}

impl FrameReader {
    /// Files decode on `gpu` where it decodes their streams, the frames
    /// shrunk there before they come down ([`VideoFrames::decode_on`]).
    pub fn decoding_on(gpu: crate::hw::GpuDecoder) -> Self {
        Self {
            frames: None,
            decoder: Some(gpu),
        }
    }

    /// The GPU that decoded the last frame it rendered (`None`: the CPU,
    /// or black).
    pub fn decoded_on(&self) -> Option<crate::hw::Kind> {
        self.frames.as_ref().and_then(|(.., f)| f.decoded_on())
    }

    /// `piece`'s first frame as export samples it, at `width`×`height`
    /// (packed RGBA, the picture fit inside, padded black).
    pub fn rgba(
        &mut self,
        piece: &VideoPiece,
        fps: Rational,
        width: u32,
        height: u32,
    ) -> Result<Image> {
        crate::cancel::stop()?;
        crate::init();
        let same =
            matches!(&self.frames, Some((f, w, h, _)) if (*f, *w, *h) == (fps, width, height));
        if !same {
            let frames = frames(fps, (width, height), RGBA, self.decoder.as_ref());
            self.frames = Some((fps, width, height, frames));
        }
        let (.., frames) = self.frames.as_mut().expect("set above");
        frames.read_piece(&piece.part(0, 1));
        let frame = frames
            .next_frame()?
            .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
        Ok(image(&frame, width, height))
    }
}

/// Pictures packed for these renderers' images.
const RGBA: format::Pixel = format::Pixel::RGBA;

/// Frames as `pixel` pictures of `width`×`height` at `fps`, decoding on
/// `decoder` where it decodes a stream.
pub(crate) fn frames(
    fps: Rational,
    (width, height): (u32, u32),
    pixel: format::Pixel,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> VideoFrames {
    let frames = VideoFrames::new(&[], fps, width, height, pixel, None);
    match decoder {
        Some(d) => frames.decode_on(d.clone()),
        None => frames,
    }
}

/// `over` at `weight` over `under`, packed RGBA pictures of one size
/// ([`super::mix::mixed`]).
pub(crate) fn mixed_image(under: &Image, over: &Image, weight: u32) -> Image {
    let rgba = under
        .rgba
        .iter()
        .zip(&over.rgba)
        .map(|(&a, &b)| super::mix::mixed(u32::from(a), u32::from(b), weight) as u8)
        .collect();
    Image {
        width: under.width,
        height: under.height,
        rgba,
    }
}

/// A window of frames to render behind a renderer's position, on a thread
/// of its own; what it renders lands in that renderer's pictures.
pub struct Prefetch {
    /// The window's last frame.
    piece: VideoPiece,
    key: String,
    end: i64,
    /// The window starts no earlier.
    floor: i64,
    fps: Rational,
    width: u32,
    height: u32,
    min: i64,
    max: i64,
    shared: Arc<Shared>,
    decoder: Option<crate::hw::GpuDecoder>,
}

/// What a [`Prefetch`] rendered: its window's first and last frame, and
/// how many of them from the first on, all unless the position moved on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ran {
    pub first: i64,
    pub last: i64,
    pub rendered: i64,
}

impl Ran {
    /// Whether it rendered its whole window.
    pub fn whole(&self) -> bool {
        self.rendered == self.last - self.first + 1
    }
}

impl Prefetch {
    /// Renders the window. Once it renders, the renderer waits for a frame
    /// of it it is asked for, rather than rendering it again. Its decoding
    /// stops, between frames, once the position has moved on: run forward,
    /// or gone below the window or a whole window past it, or the renderer
    /// is gone.
    pub fn run(self) -> Result<Ran> {
        crate::init();
        let mut ran = Ran {
            first: self.end,
            last: self.end,
            rendered: 0,
        };
        let stop = Arc::new(AtomicBool::new(false));
        {
            // Its first frame isn't known yet: a window's length at most. A
            // renderer asked for its last frame meanwhile rendered it.
            let mut store = self.shared.lock();
            if !store.wants(&self.key, self.end + 1 - self.max, self.end, self.max)
                || store.pictures.contains_key(&(self.key.clone(), self.end))
            {
                return Ok(ran);
            }
            store.stopper = Some((stop.clone(), self.max));
        }
        let _stops = crate::cancel::Guard::set(stop.clone());
        match self.render(&mut ran) {
            Err(_) if stop.load(Ordering::Relaxed) => Ok(ran),
            r => r.map(|()| ran),
        }
    }

    fn render(&self, ran: &mut Ran) -> Result<()> {
        let mut frames = frames(
            self.fps,
            (self.width, self.height),
            RGBA,
            self.decoder.as_ref(),
        );
        let start = window(
            &mut frames,
            &self.piece,
            self.end,
            self.floor,
            self.fps,
            self.min,
            self.max,
            false,
        )?;
        ran.first = start;
        {
            let mut store = self.shared.lock();
            store.prefetching = Some((self.key.clone(), start, self.end));
            store.recheck();
        }
        let n = self.end - start + 1;
        frames.read_piece(&at(&self.piece, start, n));
        for j in start..=self.end {
            crate::cancel::stop()?;
            let frame = frames
                .next_frame()?
                .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
            self.shared.put(
                &self.key,
                j,
                Arc::new(image(&frame, self.width, self.height)),
            );
            ran.rendered += 1;
            self.shared.landed.notify_all();
        }
        Ok(())
    }
}

/// The pictures a [`StillRenderer`] keeps, looked up from another thread.
#[derive(Clone)]
pub struct Kept(Arc<Shared>);

impl Kept {
    /// The kept picture of `piece`'s first frame at `fps` and
    /// `width`×`height`, as [`StillRenderer::rgba`] renders it: a source's
    /// frames are kept, a mix's aren't.
    pub fn get(
        &self,
        piece: &VideoPiece,
        fps: Rational,
        width: u32,
        height: u32,
    ) -> Option<Arc<Image>> {
        let VideoPiece::Source { source_frame, .. } = piece else {
            return None;
        };
        let key = source_key(piece, fps, width, height);
        let mut store = self.0.lock();
        store.clock += 1;
        let now = store.clock;
        let (img, used) = store.pictures.get_mut(&(key, *source_frame))?;
        *used = now;
        Some(img.clone())
    }
}

impl Drop for StillRenderer {
    /// Its windows stop, and its pictures go, whoever still looks them up.
    fn drop(&mut self) {
        let mut store = self.shared.lock();
        store.at = None;
        store.recheck();
        store.pictures.clear();
        store.bytes = 0;
        drop(store);
        self.shared.landed.notify_all();
    }
}

impl Drop for Prefetch {
    /// Done, failed or never run: the renderer no longer waits for it.
    fn drop(&mut self) {
        let mut store = self.shared.lock();
        if store
            .prefetching
            .as_ref()
            .is_some_and(|(k, _, e)| *k == self.key && *e == self.end)
        {
            store.prefetching = None;
            store.stopper = None;
        }
        drop(store);
        self.shared.landed.notify_all();
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn budget(&self) -> usize {
        self.lock().budget
    }

    /// The kept picture of `pos`, waiting for it while a prefetch renders
    /// it.
    fn get_or_wait(&self, key: &str, pos: i64, max: i64) -> Option<Arc<Image>> {
        let mut store = self.lock();
        loop {
            store.clock += 1;
            let now = store.clock;
            if let Some((img, used)) = store.pictures.get_mut(&(key.to_string(), pos)) {
                *used = now;
                return Some(img.clone());
            }
            // One planned and not rendering yet may be waiting for others:
            // the renderer renders the frame itself.
            match &store.prefetching {
                Some((k, start, end)) if store.stopper.is_some() && k == key && pos <= *end => {
                    let first = if start < end { *start } else { end - max };
                    if pos < first {
                        return None;
                    }
                }
                _ => return None,
            }
            store = self.landed.wait(store).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Keeps `img` as `pos` of `key`. Over the budget, the pictures kept or
    /// looked at least recently go first: whatever the position moves
    /// between, its pictures stay whole.
    fn put(&self, key: &str, pos: i64, img: Arc<Image>) {
        let mut store = self.lock();
        store.clock += 1;
        let (size, now) = (img.rgba.len(), store.clock);
        if let Some((old, _)) = store.pictures.insert((key.to_string(), pos), (img, now)) {
            store.bytes -= old.rgba.len();
        }
        store.bytes += size;
        while store.bytes > store.budget && store.pictures.len() > 1 {
            let oldest = store
                .pictures
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone())
                .expect("not empty");
            if let Some((old, _)) = store.pictures.remove(&oldest) {
                store.bytes -= old.rgba.len();
            }
        }
    }
}

/// Frames after the one decoded last that decode on from it, rather than
/// seek: the frames reader's own rule.
fn reach(fps: Rational) -> i64 {
    ((fps.as_f64() * super::video::DECODE_AHEAD).round() as i64).max(1)
}

/// The shortest and longest windows back: two seconds, and half the budget
/// (the other half holds the window before it), in frames.
pub(crate) fn window_sizes(budget: usize, width: u32, height: u32, fps: Rational) -> (i64, i64) {
    let frame = (width as usize * height as usize * 4).max(1);
    let max = ((budget / frame / 2) as i64).max(1);
    let min = ((fps.as_f64() * WINDOW_SECS).round() as i64).clamp(1, max);
    (min, max)
}

/// The first frame of the window to render together for `pos`, going back:
/// from a keyframe, so nothing in it decodes twice, reaching back over more
/// keyframes to hold `min` frames, holding at most `max`, and starting no
/// earlier than `floor`. With `whole_gops` it reaches back over another
/// keyframe only to hold that GOP whole: started between two keyframes, the
/// frames before it decode for nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn window(
    frames: &mut VideoFrames,
    piece: &VideoPiece,
    pos: i64,
    floor: i64,
    fps: Rational,
    min: i64,
    max: i64,
    whole_gops: bool,
) -> Result<i64> {
    let VideoPiece::Source { origin, .. } = piece else {
        return Ok(pos);
    };
    let rate = fps.as_f64();
    let (mut probe, mut start) = (pos, pos);
    for i in 0..64 {
        let Some(key) = frames.keyframe_time(&at(piece, probe, 1))? else {
            start = pos - min + 1;
            break;
        };
        // The first frame sampled at or after the keyframe: its target time
        // is past the keyframe's (a microsecond more against float noise),
        // so seeking for it starts from that keyframe, not the one before.
        let first = ((key - origin) * rate + 0.5 + 1e-6).ceil() as i64;
        let first = first.clamp(0, probe);
        if whole_gops && i > 0 && pos - first.max(floor) + 1 > max {
            break;
        }
        start = first;
        if pos - start + 1 >= min || start <= floor {
            break;
        }
        probe = start - 1;
    }
    Ok(start.max(pos - max + 1).max(floor).max(0))
}

/// `piece` from source position `pos`, `frames` long, walking its source
/// frame by frame; a mix's first `frames`.
pub(crate) fn at(piece: &VideoPiece, pos: i64, frames: i64) -> VideoPiece {
    let mut p = piece.clone();
    match &mut p {
        VideoPiece::Mix { .. } => return piece.part(0, frames),
        VideoPiece::Source {
            source_frame,
            frames: n,
            pace,
            ..
        } => {
            *source_frame = pos;
            *n = frames;
            *pace = Default::default();
        }
        VideoPiece::Black { frames: n } => *n = frames,
    }
    p
}

/// What a piece shows, apart from where in it, at a rate and size: its
/// positions can stand in for each other.
fn source_key(piece: &VideoPiece, fps: Rational, width: u32, height: u32) -> String {
    match piece {
        VideoPiece::Source {
            path,
            origin,
            image,
            ..
        } => format!(
            "{}|{:x}|{image:?}|{fps}|{width}x{height}",
            path.display(),
            origin.to_bits()
        ),
        VideoPiece::Black { .. } => format!("black|{fps}|{width}x{height}"),
        VideoPiece::Mix { .. } => format!("mix|{piece:?}|{fps}|{width}x{height}"),
    }
}

/// A packed RGBA frame's rows as a picture.
fn image(frame: &frame::Video, width: u32, height: u32) -> Image {
    let stride = frame.stride(0);
    let row = width as usize * 4;
    let data = frame.data(0);
    let mut rgba = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        rgba.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    Image {
        width,
        height,
        rgba,
    }
}
