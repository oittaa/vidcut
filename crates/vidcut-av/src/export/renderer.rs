//! Pictures that follow a moving position, such as a paused viewer being
//! stepped or a trim's two-up, each frame decoded once
//! (docs/decisions/preview.md §9).
//!
//! Forward, the file and its decoder stay open and the next frame decodes
//! on from the one before, as every player does. Back, there is nothing to
//! decode on from: a seek, and decoding from the keyframe before, can't be
//! avoided. So a window renders at once, from a keyframe up to a frame, and
//! is kept, on another thread ([`Prefetch`]) ahead of need: wherever the
//! position is, at least two seconds behind it, and going back, a whole
//! window more, so stepping and playing back don't wait. A window starts on
//! a keyframe, so nothing in it decodes twice; it reaches back over more
//! keyframes to hold at least two seconds where they are close, and a
//! memory budget caps it. Over the budget, the pictures looked at least
//! recently go first.

use std::collections::HashMap;
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
}

/// Kept pictures, shared with prefetches.
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
    /// A prefetch on its way: its source and the frames it renders, first
    /// to last (the first is the last until its window is known).
    prefetching: Option<(String, i64, i64)>,
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
                }),
                landed: Condvar::new(),
            }),
            last: None,
            back: false,
            prefetch: None,
            decoder: None,
        }
    }

    /// Files decode on `gpu` where it decodes their streams, but H.264
    /// (`frames`), the frames shrunk there before they come down
    /// ([`VideoFrames::decode_on`]).
    pub fn decoding_on(mut self, gpu: crate::hw::GpuDecoder) -> Self {
        self.decoder = Some(gpu);
        self
    }

    /// Renders `piece`'s first frame at `width`×`height`. `floor` is the
    /// first source position worth keeping frames behind it for: where the
    /// shown stretch of the file starts (0: the whole file).
    pub fn rgba(
        &mut self,
        piece: &VideoPiece,
        floor: i64,
        fps: Rational,
        width: u32,
        height: u32,
    ) -> Result<Arc<Image>> {
        crate::cancel::stop()?;
        crate::init();
        // A prefetch nobody took isn't coming: nothing waits for it.
        self.prefetch = None;
        let same =
            matches!(&self.frames, Some((f, w, h, _)) if (*f, *w, *h) == (fps, width, height));
        if !same {
            let frames = frames(fps, (width, height), self.decoder.as_ref());
            self.frames = Some((fps, width, height, frames));
        }
        let VideoPiece::Source {
            source_frame: pos,
            image,
            ..
        } = piece
        else {
            self.last = None;
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
        let floor = floor.min(pos);
        let (min, max) = window_sizes(self.shared.budget(), width, height, fps);
        let img = match self.shared.get_or_wait(&key, pos, max) {
            Some(img) => img,
            None if self.back && video => {
                let (.., frames) = self.frames.as_mut().expect("set above");
                let start = window(frames, piece, pos, floor, fps, min, max, false)?;
                let n = pos - start + 1;
                let mut shown = None;
                for (j, img) in (start..).zip(self.render(&at(piece, start, n), n)?) {
                    let img = Arc::new(img);
                    if j == pos {
                        shown = Some(img.clone());
                    }
                    self.shared.put(&key, j, img);
                }
                shown.expect("rendered up to the frame asked for")
            }
            None => {
                let img = Arc::new(self.render(&at(piece, pos, 1), 1)?.remove(0));
                self.shared.put(&key, pos, img.clone());
                img
            }
        };
        if video {
            self.plan_prefetch(piece, &key, pos, floor, (min, max));
        }
        Ok(img)
    }

    /// The window to render behind the last frame asked for: run it on
    /// another thread, while this renderer goes on. Untaken, it is dropped
    /// with the next request.
    pub fn take_prefetch(&mut self) -> Option<Prefetch> {
        self.prefetch.take()
    }

    /// Plans the window before the frames kept below `pos` while too few of
    /// them are left: two seconds wherever the position is, and going back
    /// a whole window, so the next one back renders while the position goes
    /// through this one. Never below `floor`; one prefetch at a time.
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
        if store.prefetching.is_some() || self.prefetch.is_some() {
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
    /// Files decode on `gpu` where it decodes their streams, but H.264
    /// (`frames`), the frames shrunk there before they come down
    /// ([`VideoFrames::decode_on`]).
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
            let frames = frames(fps, (width, height), self.decoder.as_ref());
            self.frames = Some((fps, width, height, frames));
        }
        let (.., frames) = self.frames.as_mut().expect("set above");
        frames.read_piece(&at(piece, first_frame(piece), 1));
        let frame = frames
            .next_frame()?
            .ok_or_else(|| Error::Invalid("no frame to render".into()))?;
        Ok(image(&frame, width, height))
    }
}

/// Frames as RGBA pictures of `width`×`height` at `fps`, decoding on
/// `decoder` where it decodes a stream: HEVC, AV1 and VP9. H.264, which the
/// CPU decodes cheaply, stays on the CPU: frames read in jumps seek at every
/// one, and FFmpeg 8 makes the GPU's H.264 decoder anew at each (gotchas.md).
fn frames(
    fps: Rational,
    (width, height): (u32, u32),
    decoder: Option<&crate::hw::GpuDecoder>,
) -> VideoFrames {
    let frames = VideoFrames::new(&[], fps, width, height, format::Pixel::RGBA, None);
    match decoder {
        Some(d) => frames.decode_on(d.clone().without(
            crate::ffmpeg::codec::Id::H264,
            "frames read in jumps seek at every one, and the CPU decodes H.264 sooner",
        )),
        None => frames,
    }
}

/// The source position a piece starts at (0 for black).
fn first_frame(piece: &VideoPiece) -> i64 {
    match piece {
        VideoPiece::Source { source_frame, .. } => *source_frame,
        VideoPiece::Black { .. } => 0,
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

impl Prefetch {
    /// Renders the window. The renderer waits for a frame of it it is asked
    /// for meanwhile, rather than rendering it again.
    pub fn run(self) -> Result<()> {
        crate::init();
        let mut frames = frames(self.fps, (self.width, self.height), self.decoder.as_ref());
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
        self.shared.lock().prefetching = Some((self.key.clone(), start, self.end));
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
            self.shared.landed.notify_all();
        }
        Ok(())
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
            match &store.prefetching {
                Some((k, start, end)) if k == key && pos <= *end => {
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
/// frame by frame.
pub(crate) fn at(piece: &VideoPiece, pos: i64, frames: i64) -> VideoPiece {
    let mut p = piece.clone();
    match &mut p {
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
