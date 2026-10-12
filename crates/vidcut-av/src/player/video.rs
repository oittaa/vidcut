//! The decode thread: pictures for the frame the viewer is at, then ahead of
//! it in the direction of play.
//!
//! Forward, the export's reader decodes on from frame to frame, a piece of
//! another file opening ahead on its helper thread. Going back there is
//! nothing to decode on from, so a window renders at once, from a keyframe
//! up to the frame wanted, and is kept (as the paused viewer's renderer
//! does, preview.md §13); playing backwards, the window before renders
//! while this one is shown.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut_core::render::VideoPiece;

use super::frame::Picture;
use super::plan::Plan;
use super::{Busy, Mode, Output, Shared, Snap, trace};
use crate::error::{Error, Result};
use crate::export::is_cancelled;
use crate::export::renderer::{at, window, window_sizes};
use crate::export::video::VideoFrames;

/// Frames decoded ahead of a paused viewer, when decoding on is cheap.
const PAUSED_AHEAD: i64 = 3;

/// Seconds of pictures decoded ahead of the clock while playing.
const AHEAD_SECS: f64 = 0.25;

/// Frames past the reader's position it decodes on to rather than seeking
/// (the export's reader reads on within a second, too).
const DECODE_ON_SECS: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Direction {
    Paused,
    Forward(f64),
    Back,
}

struct Decoder {
    shared: Arc<Shared>,
    /// The reader, for the output it was made for.
    frames: Option<(Output, VideoFrames)>,
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
}

pub(crate) fn run(shared: Arc<Shared>) {
    let _cancel = crate::cancel::Guard::set(shared.interrupt.clone());
    let Some(first) = shared.snap() else { return };
    let mut d = Decoder {
        shared: shared.clone(),
        frames: None,
        next: None,
        plan: first.plan.clone(),
        plan_gen: None,
        last_focus: None,
        back: false,
        failed: None,
        seek_cost: Duration::ZERO,
        caught_up: None,
    };
    while let Some(snap) = shared.snap() {
        if d.plan_gen != Some(snap.plan_gen) {
            d.plan = snap.plan.clone();
            d.plan_gen = Some(snap.plan_gen);
            // The reader keeps its file open; where it stands in the new
            // plan is found by reading pieces of it again.
            d.next = None;
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
        d.last_focus = Some(snap.focus);
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
                d.failed = Some((snap.version, snap.focus));
                d.lost_position();
                shared.wait_change(snap.version, Duration::from_millis(50));
            }
        }
    }
}

impl Decoder {
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
                Direction::Forward(speed) if !reads_on => {
                    let lead = self.seek_cost.as_secs_f64() * plan.fps().as_f64() * speed;
                    plan.clamp(focus + lead.ceil() as i64)
                }
                _ => focus,
            };
            self.decode(snap, target, self.back)?;
            if !reads_on {
                let took = started.elapsed();
                self.seek_cost = (self.seek_cost * 3 + took) / 4;
            }
            if target > focus {
                trace!("caught up at {target}, the clock at {focus}");
                self.caught_up = Some(target);
            }
            return Ok(true);
        }
        self.shared.set_busy(Some(Busy::Ahead));
        let fps = plan.fps().as_f64();
        let wanted = match dir {
            Direction::Forward(speed) => {
                let ahead = ((AHEAD_SECS * fps * speed).ceil() as i64).max(2);
                let ahead = ahead.min(self.frames_in(store.budget() / 4, snap));
                (anchor + 1..=plan.clamp(anchor + ahead))
                    .find(|&t| missing(t))
                    .map(|t| (t, false))
            }
            Direction::Back => {
                // A window behind the one being shown, so going back
                // doesn't wait at keyframes: a third of the budget each.
                let (_, max) = self.windows(snap, 3);
                (plan.clamp(focus - max)..focus)
                    .rev()
                    .find(|&t| missing(t))
                    .map(|t| (t, true))
            }
            Direction::Paused => {
                let next = (focus + 1..=plan.clamp(focus + PAUSED_AHEAD))
                    .find(|&t| missing(t))
                    .filter(|&t| self.next == Some(t));
                let behind = || {
                    let (min, _) = self.windows(snap, 2);
                    (plan.clamp(focus - min)..focus).rev().find(|&t| missing(t))
                };
                match next {
                    Some(t) => Some((t, false)),
                    None if self.back => behind().map(|t| (t, true)),
                    None => None,
                }
            }
        };
        match wanted {
            Some((t, back)) => {
                self.decode(snap, t, back)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The reader gives `t` by decoding on from where it stands.
    fn reads_on(&self, t: i64) -> bool {
        let limit = (DECODE_ON_SECS * self.plan.fps().as_f64()).ceil() as i64;
        self.next.is_some_and(|r| r <= t && t - r <= limit)
    }

    /// The shortest and longest windows back, in frames, for a budget
    /// shared by `parts` of them.
    fn windows(&self, snap: &Snap, parts: usize) -> (i64, i64) {
        let o = snap.output;
        window_sizes(
            self.shared.store.budget() * 2 / parts.max(1),
            o.width,
            o.height,
            self.plan.fps(),
        )
    }

    /// Pictures of the current output that fit in `bytes`.
    fn frames_in(&self, bytes: usize, snap: &Snap) -> i64 {
        let o = snap.output;
        ((bytes / (o.width as usize * o.height as usize * 4).max(1)) as i64).max(2)
    }

    /// The reader for `out`, made anew when the output changed.
    fn reader(&mut self, out: Output) -> &mut VideoFrames {
        if self.frames.as_ref().is_none_or(|(o, _)| *o != out) {
            let fps = self.plan.fps();
            let frames = VideoFrames::new(&[], fps, out.width, out.height, out.pixel, None);
            self.frames = Some((out, frames));
            self.next = None;
        }
        &mut self.frames.as_mut().expect("just made").1
    }

    /// Decodes timeline frame `t`, and on the way what is worth keeping:
    /// going `back`, a window from a keyframe up to `t`.
    fn decode(&mut self, snap: &Snap, t: i64, back: bool) -> Result<()> {
        let plan = self.plan.clone();
        let out = snap.output;
        if self
            .frames
            .as_ref()
            .is_some_and(|(_, f)| f.fps() != plan.fps())
        {
            self.frames = None;
        }
        self.reader(out);
        if !self.reads_on(t) {
            let from = if back { self.window_start(t)? } else { t };
            self.reader(out).read_pieces(&plan.pieces_from(from));
            self.next = Some(from);
        }
        let r = self.next.expect("positioned");
        trace!(
            "decode {r}..={t} (viewer at {}{})",
            snap.focus,
            if back { ", going back" } else { "" }
        );
        for i in r..=t {
            // Frames before the one wanted: kept going back (they are the
            // window) and when they are still ahead of the viewer; passed
            // over otherwise.
            let keep =
                i == t || back || (i >= snap.focus && !self.shared.store.contains(&plan.id(i)));
            let frames = self.reader(out);
            if keep {
                let frame = frames
                    .next_frame()?
                    .ok_or_else(|| Error::Invalid(format!("no frame {i} to play")))?;
                self.next = Some(i + 1);
                self.shared
                    .store
                    .put(plan.id(i), Arc::new(Picture::new(frame)));
                if i == snap.focus {
                    self.shared.landed();
                }
            } else {
                frames.skip_frame()?;
                self.next = Some(i + 1);
            }
        }
        Ok(())
    }

    /// The first frame of the window to decode going back to `t`: from a
    /// keyframe, so nothing in it decodes twice, holding at least the
    /// shortest window where keyframes are close and at most the longest,
    /// never before `t`'s piece. A picture at a time for black, stills
    /// and image sequences: they stand alone.
    fn window_start(&mut self, t: i64) -> Result<i64> {
        let plan = self.plan.clone();
        let Some((piece, piece_start)) = plan.piece_at(t) else {
            return Ok(t);
        };
        let VideoPiece::Source {
            source_frame: pos,
            image: None,
            ..
        } = piece
        else {
            return Ok(t);
        };
        let floor = pos - (t - piece_start);
        let out = self.frames.as_ref().expect("made").0;
        let budget = self.shared.store.budget();
        let (min, max) = window_sizes(budget * 2 / 3, out.width, out.height, plan.fps());
        let frames = self.reader(out);
        let start = window(
            frames,
            &at(&piece, pos, 1),
            pos,
            floor,
            plan.fps(),
            min,
            max,
        )?;
        // The reader was moved to find the keyframe.
        self.next = None;
        Ok(t - (pos - start))
    }
}
