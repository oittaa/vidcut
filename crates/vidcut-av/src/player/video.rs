//! The decode thread: pictures for the frame the viewer is at, then ahead of
//! it in the direction of play.
//!
//! Forward, the export's reader decodes on from frame to frame, a piece of
//! another file opening ahead on its helper thread. Going back there is
//! nothing to decode on from, so a window renders at once, from a keyframe
//! up to the frame wanted, and is kept (as the two-up's renderer does,
//! preview.md §9). Playing backwards, two windows wait behind the one
//! shown where they fit: once one is left, the one before it decodes.
//!
//! Paused, frames stay around the playhead. A seek keeps what it decodes
//! on its way, from the keyframe before; with nothing else to decode, it
//! decodes the frame prepared of the plan played next (on a reader of its
//! own made for that picture, so the reader it plays with stays where it
//! stands), and a window behind the playhead, so the first steps back wait
//! for nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut_core::render::{RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

use super::frame::Picture;
use super::plan::Plan;
use super::store::Kind;
use super::{Busy, Decoder as Decoding, Mode, Shared, Snap};
use crate::error::{Error, Result};
use crate::export::is_cancelled;
use crate::export::make::Output;
use crate::export::renderer::{at, window};
use crate::export::video::VideoFrames;
use crate::ffmpeg::format;

/// Frames decoded ahead of a paused viewer, when decoding on is cheap.
const PAUSED_AHEAD: i64 = 3;

/// Seconds of pictures decoded ahead of the clock while playing.
const AHEAD_SECS: f64 = 0.25;

/// Frames past the reader's position it decodes on to rather than seeking
/// (the export's reader reads on within a second, too).
const DECODE_ON_SECS: f64 = 1.0;

/// Seconds a window back holds at least, where keyframes are close.
const WINDOW_SECS: f64 = 2.0;

/// Windows decoded behind the one shown, playing backwards.
const WINDOWS_BEHIND: usize = 2;

/// Pictures a window behind leaves room for in the budget: the one held
/// for the plan played next, and one to spare.
const SPARE: i64 = 2;

/// How far back a decode starts, and what of it is kept.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reach {
    /// From the frame (or on from where the reader stands): those from the
    /// viewer's on are kept.
    Forward,
    /// From a keyframe, reaching back over more to hold `min` frames, at
    /// most `max`: all kept.
    Window { min: i64, max: i64 },
    /// A seek: from its frame's keyframe, keeping what its own pictures
    /// allow ([`Keep::Seek`]).
    Seek { max: i64 },
}

/// What a decode keeps on its way to the frame it wants.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Keep {
    /// The frames from the viewer's on.
    Ahead,
    /// All of them: a window.
    Window,
    /// A seek's: as much as a window of pictures like its first holds
    /// (`chunk`). In video memory that is all of a GOP, in memory its last
    /// half second.
    Seek,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Direction {
    Paused,
    Forward(f64),
    Back,
}

struct Decoder {
    shared: Arc<Shared>,
    /// The reader, for the rate, the size (of black) and the decoding it
    /// was made for.
    frames: Option<((Rational, u32, u32, bool), VideoFrames)>,
    /// Decode on the GPU where it can, as last asked.
    hardware: bool,
    /// The GPU's decoder, once opened (`None` inside: there is none).
    gpu: Option<Option<crate::hw::GpuDecoder>>,
    /// What the latest picture took, and where: windows and the pictures
    /// ahead are counted in it, against that kind's budget.
    picture_bytes: usize,
    picture_kind: Kind,
    /// The timeline frame the reader gives next, when it is known.
    next: Option<i64>,
    plan: Arc<Plan>,
    plan_gen: Option<u64>,
    /// The paused viewer's previous frame, and whether it last moved back.
    last_focus: Option<i64>,
    back: bool,
    /// A frame that failed to decode, and the change it failed after: not
    /// tried again until something changes.
    failed: Option<(u64, i64)>,
    /// How long a seek took to its first picture lately: how far ahead a
    /// late player seeks to catch up.
    seek_cost: Duration,
    /// The frame a late player caught up at, ahead of the clock: decoding
    /// goes on from there while the clock comes.
    caught_up: Option<i64>,
    /// The prepared frame failed to decode after this change: not tried
    /// again until something changes.
    prepared_failed: Option<u64>,
    /// Playing backwards: the windows decoded (first and last frame), and
    /// the lowest frame of those kept below the playhead, last looked.
    back_windows: Vec<(i64, i64)>,
    back_low: Option<i64>,
    /// The window found to decode next playing backwards, by its last
    /// frame, while it waits for room.
    next_window: Option<(i64, i64)>,
}

pub(crate) fn run(shared: Arc<Shared>) {
    let _cancel = crate::cancel::Guard::set(shared.interrupt.clone());
    let _counting = crate::copies::Guard::set(shared.copies.clone());
    let Some(first) = shared.snap() else { return };
    let p = &first.plan.plan;
    let mut d = Decoder {
        shared: shared.clone(),
        frames: None,
        hardware: first.hardware,
        gpu: None,
        picture_bytes: (p.width as usize * p.height as usize * 3).div_ceil(2),
        picture_kind: Kind::Memory,
        next: None,
        plan: first.plan.clone(),
        plan_gen: None,
        last_focus: None,
        back: false,
        failed: None,
        seek_cost: Duration::ZERO,
        caught_up: None,
        prepared_failed: None,
        back_windows: Vec::new(),
        back_low: None,
        next_window: None,
    };
    let mut claim = crate::first::Claim::default();
    while let Some(snap) = shared.snap() {
        if d.plan_gen != Some(snap.plan_gen) {
            d.plan = snap.plan.clone();
            d.plan_gen = Some(snap.plan_gen);
            // The reader keeps its file open; where it stands in the new
            // plan is found by reading pieces of it again.
            d.next = None;
            // Frames of another plan say nothing of which way the viewer
            // goes in this one (a switch to the other thing a viewer
            // shows): no window behind until it steps back here.
            d.last_focus = None;
            d.back = false;
            d.forget_windows();
        }
        if d.hardware != snap.hardware {
            // Another decoder: its pictures, from the one shown on.
            d.hardware = snap.hardware;
            shared.store.clear();
            d.failed = None;
        }
        let dir = match snap.mode {
            Mode::Paused { .. } => Direction::Paused,
            Mode::Playing { speed, .. } if speed > 0.0 => Direction::Forward(speed),
            Mode::Playing { .. } => Direction::Back,
        };
        match dir {
            Direction::Paused => {
                if let Some(last) = d.last_focus
                    && snap.focus != last
                {
                    d.back = snap.focus < last;
                }
            }
            Direction::Forward(_) => d.back = false,
            Direction::Back => d.back = true,
        }
        if dir != Direction::Back {
            d.forget_windows();
        }
        d.last_focus = Some(snap.focus);
        // The player comes first while it plays, and while its viewer
        // waits for a picture.
        let waits = !shared.store.contains(&snap.plan.id(snap.focus))
            && d.failed != Some((snap.version, snap.focus));
        claim.set(dir != Direction::Paused || waits);
        let result = d.work(&snap, dir);
        shared.set_busy(None);
        match result {
            Ok(true) => {}
            Ok(false) => {
                // Playing, the clock moves on by itself: look again soon.
                let wait = match dir {
                    Direction::Paused => Duration::from_secs(1),
                    _ => Duration::from_secs_f64(0.5 / snap.plan.fps().as_f64())
                        .min(Duration::from_millis(10)),
                };
                shared.wait_change(snap.version, wait);
            }
            Err(e) if is_cancelled(&e) => d.lost_position(),
            Err(e) => {
                shared.fail(e.to_string());
                if !shared.store.contains(&snap.plan.id(snap.focus)) {
                    shared.failed(snap.focus);
                }
                d.failed = Some((snap.version, snap.focus));
                d.lost_position();
                shared.wait_change(snap.version, Duration::from_millis(50));
            }
        }
    }
}

impl Decoder {
    /// Not playing backwards (or another plan): its windows say nothing.
    fn forget_windows(&mut self) {
        self.back_windows.clear();
        self.back_low = None;
        self.next_window = None;
    }

    /// The reader stands anywhere: interrupted, or after an error.
    fn lost_position(&mut self) {
        if let Some((_, f)) = self.frames.as_mut() {
            f.forget_position();
        }
        self.next = None;
    }

    /// Decodes the next picture wanted; whether there was one.
    fn work(&mut self, snap: &Snap, dir: Direction) -> Result<bool> {
        let plan = self.plan.clone();
        let store = &self.shared.store;
        let missing = |t: i64| !store.contains(&plan.id(t));
        let focus = snap.focus;
        // Playing forward after catching up: the frames between the clock
        // and the frame caught up at were passed over; on from there.
        let anchor = match (dir, self.caught_up) {
            (Direction::Forward(_), Some(c)) if c > focus && !missing(c) => c,
            _ => {
                self.caught_up = None;
                focus
            }
        };
        // The frame the viewer is at.
        if anchor == focus && missing(focus) {
            if self.failed == Some((snap.version, focus)) {
                return Ok(false);
            }
            self.shared.set_busy(Some(Busy::Focus));
            let started = Instant::now();
            let reads_on = self.reads_on(focus);
            let target = match dir {
                // Late: catch up where the clock will be once a seek is done.
                // Until the sound of a play request is heard, the clock
                // stands where play starts (a dragged playhead makes one
                // request after another): that frame.
                Direction::Forward(speed) if !reads_on && snap.heard => {
                    let lead = self.seek_cost.as_secs_f64() * plan.fps().as_f64() * speed;
                    plan.clamp(focus + lead.ceil() as i64)
                }
                _ => focus,
            };
            let reach = match dir {
                // Playing back with nothing kept here: from this frame's
                // own keyframe, so it shows after one GOP's decoding, not
                // two; the windows before come while it is shown.
                Direction::Back => Reach::Window {
                    min: 1,
                    max: self.windows(2).1,
                },
                // A step back past what is kept: a chunk behind, the rest
                // comes when nothing else is wanted.
                _ if self.back => Reach::Window {
                    min: 1,
                    max: self.chunk(),
                },
                // A seek: the frames from its keyframe are decoded anyway,
                // and kept they are the first steps back. In video memory
                // all of them (NVDEC's, for a display that shows them from
                // there: kept, they cost nothing more); in memory the last
                // half second, as each kept there takes new memory, and a
                // 10-bit one a conversion, slowing the seek down. Its own
                // first picture says which (`Keep::Seek`), not the one
                // shown before it (black, a still, another clip). The rest
                // behind comes when nothing else is wanted. It reaches back
                // no further than the seek's keyframe.
                Direction::Paused => Reach::Seek {
                    max: self.windows(2).1,
                },
                _ => Reach::Forward,
            };
            let first = self.decode(snap, target, reach)?;
            if dir == Direction::Back {
                self.back_windows.push((first, target));
            }
            if !reads_on {
                let took = started.elapsed();
                self.seek_cost = (self.seek_cost * 3 + took) / 4;
            }
            if target > focus {
                crate::debug!("player", "caught up at {target}, the clock at {focus}");
                self.caught_up = Some(target);
            }
            return Ok(true);
        }
        self.shared.set_busy(Some(Busy::Ahead));
        let fps = plan.fps().as_f64();
        let wanted = match dir {
            Direction::Forward(speed) => {
                let ahead = ((AHEAD_SECS * fps * speed).ceil() as i64).max(2);
                let budget = store.budget(self.picture_kind);
                let ahead = ahead.min(self.pictures_in(budget / 4).max(2));
                (anchor + 1..=plan.clamp(anchor + ahead))
                    .find(|&t| missing(t))
                    .map(|t| (t, Reach::Forward))
            }
            Direction::Back => return self.window_behind(snap),
            Direction::Paused => (focus + 1..=plan.clamp(focus + PAUSED_AHEAD))
                .find(|&t| missing(t))
                .filter(|&t| self.next == Some(t))
                .map(|t| (t, Reach::Forward)),
        };
        if let Some((t, reach)) = wanted {
            self.decode(snap, t, reach)?;
            return Ok(true);
        }
        if dir != Direction::Paused {
            return Ok(false);
        }
        // Paused, whichever way it last went: at least two seconds behind
        // the playhead (half the budget at most), so the first steps back
        // wait for nothing. Nearest first: each window from the keyframe
        // of the latest frame missing up to it, a GOP at a time, so the
        // frames right behind come soonest. Stepping back, that first;
        // otherwise the frame the other plan shows (a switch) first.
        let (min, max) = self.windows(2);
        let behind = (plan.clamp(focus - min)..focus).rev().find(|&t| missing(t));
        if !self.back && self.prepare(snap)? {
            return Ok(true);
        }
        match behind {
            Some(t) => {
                self.decode_behind(snap, t, self.chunk().min(max))?;
                Ok(true)
            }
            None if self.back => self.prepare(snap),
            None => Ok(false),
        }
    }

    /// Playing backwards: decodes the next window behind those kept below
    /// the playhead, while fewer than two wait behind the one shown;
    /// whether it did. The first behind is the nearest, from its own
    /// keyframe, as the playhead may be close; the second holds at least
    /// two seconds where keyframes are close, and decodes once it fits
    /// in the budget beside the frames still to be shown. Those count as
    /// looked at first, so the frames shown go before them.
    fn window_behind(&mut self, snap: &Snap) -> Result<bool> {
        let plan = self.plan.clone();
        let shared = self.shared.clone();
        let store = &shared.store;
        let focus = snap.focus;
        let (min, max) = self.windows(2);
        // The frames kept below the playhead, from where they ended last.
        let from = self
            .back_low
            .filter(|&l| l <= focus && store.contains(&plan.id(l)))
            .unwrap_or(focus);
        let reach = plan.clamp(from - 3 * max);
        let run = store.kept_run((reach..from).rev().map(|t| plan.id(t)));
        let low = from - run as i64;
        self.back_low = Some(low);
        self.back_windows.retain(|&(first, _)| first <= focus);
        let behind = self
            .back_windows
            .iter()
            .filter(|&&(first, last)| last < focus && first >= low)
            .count();
        let next = low - 1;
        if behind >= WINDOWS_BEHIND || next < 0 || next < reach {
            return Ok(false);
        }
        let room = self.pictures_in(store.budget(self.picture_kind)) - (focus - low + 1) - SPARE;
        let first = if behind == 0 {
            if room < 1 {
                return Ok(false);
            }
            self.window_start(next, 1, max.min(room))?
        } else {
            let first = match self.next_window {
                Some((last, first)) if last == next => first,
                _ => self.window_start(next, min, max)?,
            };
            if next - first + 1 > room {
                self.next_window = Some((next, first));
                return Ok(false);
            }
            first
        };
        self.next_window = None;
        crate::debug!(
            "player",
            "window {first}..={next} behind, {behind} waiting there"
        );
        store.touch((low..=focus).map(|t| plan.id(t)));
        self.decode_from(snap, Some(first), next, Keep::Window)?;
        self.back_windows.push((first, next));
        Ok(true)
    }

    /// Decodes a window behind the playhead up to `t`: from `t`'s
    /// keyframe, at most `max` frames. The viewer stepping back into it
    /// waits for it (`Busy::Window`) rather than start it again.
    fn decode_behind(&mut self, snap: &Snap, t: i64, max: i64) -> Result<()> {
        self.reader();
        let from = if self.reads_on(t) {
            None
        } else {
            Some(self.window_start(t, 1, max)?)
        };
        self.shared.set_busy(Some(Busy::Window {
            from: from.or(self.next).unwrap_or(t),
            to: t,
            plan_gen: snap.plan_gen,
        }));
        self.decode_from(snap, from, t, Keep::Window)
    }

    /// Decodes the prepared frame of the plan played next, when it isn't
    /// kept; whether it did. A failure isn't the viewer's: it is traced,
    /// and the frame tried again once something changes.
    fn prepare(&mut self, snap: &Snap) -> Result<bool> {
        let Some((plan, t)) = &snap.prepared else {
            return Ok(false);
        };
        let id = plan.id(*t);
        if self.shared.store.contains(&id) || self.prepared_failed == Some(snap.version) {
            return Ok(false);
        }
        crate::debug!("player", "prepare {t} of the plan played next");
        let picture = (|| {
            let mut frames = self.new_reader(&plan.plan);
            frames.read_pieces(&plan.pieces_from(*t));
            let frame = frames
                .next_frame()?
                .ok_or_else(|| Error::Invalid(format!("no frame {t} to prepare")))?;
            Ok(picture(&frames, plan, *t, frame))
        })();
        match picture {
            Ok(p) => {
                self.shared.store.put(id, Arc::new(p));
                self.shared.prepared();
            }
            // Something new is wanted: the reader played with didn't move.
            Err(e) if is_cancelled(&e) => {}
            Err(e) => {
                crate::debug!("player", "preparing {t} failed: {e}");
                self.prepared_failed = Some(snap.version);
            }
        }
        Ok(true)
    }

    /// The reader gives `t` by decoding on from where it stands.
    fn reads_on(&self, t: i64) -> bool {
        let limit = (DECODE_ON_SECS * self.plan.fps().as_f64()).ceil() as i64;
        self.next.is_some_and(|r| r <= t && t - r <= limit)
    }

    /// The shortest and longest windows back, in frames, when `parts` of
    /// them share the budget: at least two seconds where keyframes are
    /// close, at most what fits.
    fn windows(&self, parts: usize) -> (i64, i64) {
        let budget = self.shared.store.budget(self.picture_kind);
        let max = self.pictures_in(budget / parts.max(1)).max(1);
        let min = ((self.plan.fps().as_f64() * WINDOW_SECS).round() as i64).clamp(1, max);
        (min, max)
    }

    /// Frames a window behind a paused playhead keeps at once. Kept in
    /// video memory a frame costs nothing more, and the window goes from
    /// its keyframe up. Kept in memory, each takes new memory (and a 10-bit
    /// one a conversion): half a second at a time, nearest first, so a step
    /// back right after a seek waits for half a second's frames, not the
    /// whole GOP's.
    fn chunk(&self) -> i64 {
        match self.picture_kind {
            Kind::Video => self.windows(2).1,
            Kind::Memory => self.memory_chunk(),
        }
    }

    /// Half a second of frames, at least 8: what a window keeps in memory
    /// at once.
    fn memory_chunk(&self) -> i64 {
        let (_, max) = self.windows(2);
        ((self.plan.fps().as_f64() * 0.5).ceil() as i64).clamp(8, max.max(8))
    }

    /// Pictures like the latest that fit in `bytes`.
    fn pictures_in(&self, bytes: usize) -> i64 {
        (bytes / self.picture_bytes.max(1)) as i64
    }

    /// The reader, made anew when the rate, the size of black or the
    /// decoding changed.
    fn reader(&mut self) -> &mut VideoFrames {
        let plan = self.plan.clone();
        let p = &plan.plan;
        let key = (p.fps, p.width, p.height, self.hardware);
        if self.frames.as_ref().is_none_or(|(k, _)| *k != key) {
            let frames = self.new_reader(p);
            self.frames = Some((key, frames));
            self.next = None;
        }
        &mut self.frames.as_mut().expect("just made").1
    }

    /// A reader of `p`'s pictures, decoding as asked last.
    fn new_reader(&mut self, p: &RenderPlan) -> VideoFrames {
        let frames = VideoFrames::new(&[], p.fps, p.width, p.height, format::Pixel::YUV420P, None);
        let decoded = |keep| Output::Decoded { keep };
        if !self.hardware {
            crate::debug!(
                "player",
                "a new reader decodes on the CPU: GPU decoding is off"
            );
            self.shared.decodes_on_cpu(Some("GPU decoding is off"));
            return frames.output(decoded(Vec::new()));
        }
        let gpu = self.gpu.get_or_insert_with(crate::hw::GpuDecoder::best);
        let Some(gpu) = gpu else {
            crate::debug!(
                "player",
                "a new reader decodes on the CPU: no GPU decoder here"
            );
            self.shared.decodes_on_cpu(Some("no GPU decoder here"));
            return frames.output(decoded(Vec::new()));
        };
        self.shared.decodes_on_cpu(None);
        // The GPU's pictures may stay in video memory, which then has a
        // budget of its own: those of the decoders the viewer shows from
        // there (NVDEC's, say, while Vulkan's ProRes comes down).
        let shows = self.shared.gpu_pictures.clone();
        crate::debug!(
            "player",
            "a new reader decodes on {gpu} where it can; the viewer shows {shows:?} pictures \
             from GPU memory, the rest come down to memory"
        );
        if let Some(cuda) = gpu.cuda() {
            self.shared.gpu_found(cuda);
        }
        frames.output(decoded(shows)).decode_on(gpu.clone())
    }

    /// Decodes timeline frame `t`, and on the way what is worth keeping:
    /// for a window, everything from its keyframe up to `t`. The first
    /// frame decoded.
    fn decode(&mut self, snap: &Snap, t: i64, reach: Reach) -> Result<i64> {
        self.reader();
        let from = if self.reads_on(t) {
            None
        } else {
            Some(match reach {
                Reach::Window { min, max } => self.window_start(t, min, max)?,
                Reach::Seek { max } => self.window_start(t, 1, max)?,
                Reach::Forward => t,
            })
        };
        let first = from.or(self.next).unwrap_or(t);
        let keep = match reach {
            Reach::Forward => Keep::Ahead,
            Reach::Window { .. } => Keep::Window,
            Reach::Seek { .. } => Keep::Seek,
        };
        self.decode_from(snap, from, t, keep)?;
        Ok(first)
    }

    /// Decodes up to timeline frame `t`: from `from` (`None`: on from where
    /// the reader stands), keeping those from the viewer's on and what
    /// `keep` says of the rest.
    fn decode_from(&mut self, snap: &Snap, from: Option<i64>, t: i64, keep: Keep) -> Result<()> {
        let plan = self.plan.clone();
        if let Some(from) = from {
            self.reader().read_pieces(&plan.pieces_from(from));
            self.next = Some(from);
        }
        let r = self.next.expect("positioned");
        crate::debug!(
            "player",
            "decode {r}..={t} (viewer at {}{})",
            snap.focus,
            if keep == Keep::Ahead {
                ""
            } else {
                ", keeping a window"
            }
        );
        let started = Instant::now();
        // A seek keeps from here on, once its first picture says where its
        // pictures go.
        let mut seek_keeps: Option<i64> = None;
        for i in r..=t {
            // Frames before the one wanted: kept going back (they are the
            // window) and when they are still ahead of the viewer; passed
            // over otherwise.
            let window = match keep {
                Keep::Ahead => false,
                Keep::Window => true,
                Keep::Seek => seek_keeps.is_none_or(|from| i >= from),
            };
            let kept =
                i == t || window || (i >= snap.focus && !self.shared.store.contains(&plan.id(i)));
            let frames = self.reader();
            if kept {
                let frame = frames
                    .next_frame()?
                    .ok_or_else(|| Error::Invalid(format!("no frame {i} to play")))?;
                let picture = picture(frames, &plan, i, frame);
                self.next = Some(i + 1);
                self.picture_bytes = picture.bytes();
                self.picture_kind = Kind::of(&picture);
                if keep == Keep::Seek && seek_keeps.is_none() {
                    seek_keeps = Some(t - self.chunk() + 1);
                }
                self.shared.store.put(plan.id(i), Arc::new(picture));
                if i == snap.focus {
                    self.shared.landed(snap.plan_gen, plan.id(i));
                }
            } else {
                frames.skip_frame()?;
                self.next = Some(i + 1);
            }
        }
        if crate::debug() {
            let took = started.elapsed().as_secs_f64() * 1000.0;
            let n = t - r + 1;
            let on = self
                .reader()
                .decoded_on()
                .map_or("the CPU", crate::hw::Kind::label);
            let store = &self.shared.store;
            let (kept, bytes) = store.usage();
            crate::debug!(
                "player",
                "decoded {r}..={t} on {on}: {n} frames in {took:.1} ms, {:.2} ms each; \
                 {kept} pictures kept in {} MB (budgets {} MB of memory, {} MB of video \
                 memory)",
                took / n as f64,
                bytes >> 20,
                store.budget(Kind::Memory) >> 20,
                store.budget(Kind::Video) >> 20,
            );
        }
        Ok(())
    }

    /// The first frame of the window to decode up to `t`: from a keyframe,
    /// so nothing in it decodes twice, holding at least `min` frames where
    /// keyframes are close and at most `max` (reaching back over another
    /// keyframe only to hold that GOP whole), never before `t`'s piece. A
    /// picture at a time for black, stills and image sequences: they stand
    /// alone.
    fn window_start(&mut self, t: i64, min: i64, max: i64) -> Result<i64> {
        let plan = self.plan.clone();
        let Some((piece, piece_start)) = plan.piece_at(t) else {
            return Ok(t);
        };
        let VideoPiece::Source {
            source_frame: pos,
            image: None,
            pace,
            ..
        } = piece
        else {
            return Ok(t);
        };
        // A piece at another speed (decision D48): two seconds, at least
        // `min`, at most `max`, read from the keyframe before its first
        // (the reader makes a backward one's from keyframes itself); a held
        // one's one picture.
        if !pace.is_normal() {
            if pace.speed == 0.0 {
                return Ok(t);
            }
            let n = ((plan.fps().as_f64() * WINDOW_SECS).round() as i64)
                .max(min)
                .min(max)
                .max(1);
            return Ok((t - n + 1).max(piece_start));
        }
        let floor = pos - (t - piece_start);
        let frames = self.reader();
        let start = window(
            frames,
            &at(&piece, pos, 1),
            pos,
            floor,
            plan.fps(),
            min,
            max,
            true,
        )?;
        // The reader was moved to find the keyframe.
        self.next = None;
        Ok(t - (pos - start))
    }
}

/// The picture of timeline frame `t` of `plan`, decoded by `frames`.
fn picture(
    frames: &VideoFrames,
    plan: &Plan,
    t: i64,
    frame: crate::ffmpeg::frame::Video,
) -> Picture {
    let file = plan.shows_a_file(t);
    let decoder = file.then(|| frames.decoded_on().map_or(Decoding::Cpu, Decoding::Gpu));
    let decoding = frames.decoding().filter(|_| file);
    Picture::new(frame, frames.rotation(), decoder, decoding)
}
