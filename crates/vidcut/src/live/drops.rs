use std::time::{Duration, Instant};

const SECONDS: u32 = 5;
const SETTLE: Duration = Duration::from_secs(1);
const SECOND: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Counts {
    pub late: u64,
    pub skipped: u64,
    pub frames: u64,
    pub making: Duration,
}

pub(super) struct Sample {
    pub counting: bool,
    pub motion: u64,
    pub position: i64,
    pub fps: f64,
    pub counts: Counts,
}

struct Second {
    began: Instant,
    from: i64,
    counts: Counts,
}

struct Stretch {
    from: i64,
    to: i64,
    seconds: u32,
    frames: u64,
    making: Duration,
    fps: f64,
}

#[derive(Default)]
pub(super) struct Drops {
    motion: Option<u64>,
    settled: Option<Instant>,
    second: Option<Second>,
    stretch: Option<Stretch>,
}

impl Drops {
    pub(super) fn watch(&mut self, now: Instant, s: &Sample) -> Option<(i64, i64)> {
        let counting = s.counting.then_some(s.motion);
        if counting.is_none() || self.motion != counting {
            let ended = self.end();
            self.motion = counting;
            self.settled = counting.map(|_| now + SETTLE);
            self.second = None;
            return ended;
        }
        if self.settled.is_some_and(|t| now < t) {
            return None;
        }
        let next = Second {
            began: now,
            from: s.position,
            counts: s.counts,
        };
        let Some(second) = &self.second else {
            self.second = Some(next);
            return None;
        };
        let spent = now.duration_since(second.began);
        if spent < SECOND {
            return None;
        }
        let (before, after) = (second.counts, s.counts);
        let missed =
            after.late.saturating_sub(before.late) + after.skipped.saturating_sub(before.skipped);
        let dropping = s.fps > 0.0 && missed as f64 * 4.0 >= spent.as_secs_f64() * s.fps;
        let frames = after.frames.saturating_sub(before.frames);
        let making = after.making.saturating_sub(before.making);
        let from = second.from;
        self.second = Some(next);
        if !dropping {
            return self.end();
        }
        let stretch = self.stretch.get_or_insert(Stretch {
            from,
            to: from,
            seconds: 0,
            frames: 0,
            making: Duration::ZERO,
            fps: s.fps,
        });
        stretch.to = s.position;
        stretch.seconds += 1;
        stretch.frames += frames;
        stretch.making += making;
        None
    }

    fn end(&mut self) -> Option<(i64, i64)> {
        let s = self.stretch.take()?;
        let slow = s.making.as_secs_f64() * s.fps > s.frames as f64;
        (s.seconds >= SECONDS && s.frames > 0 && slow && s.to > s.from).then_some((s.from, s.to))
    }
}
