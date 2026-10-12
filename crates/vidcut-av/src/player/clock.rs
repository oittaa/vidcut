//! The player's clock: the timeline sample the listener hears now.
//!
//! The audio device's callback is the master clock. Each callback records
//! which ring samples it took and when the first of them reaches the
//! listener (its time plus the output's latency); markers written with the
//! samples say which timeline sample a ring position is. A picture is shown
//! when the clock reaches its frame, so pictures follow the sound as heard,
//! output latency included.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

/// Device buffers remembered: enough for the newest ones still ahead of
/// the listener (output latency) at the shortest callbacks.
const SLOTS: usize = 64;

/// What the device has played: written by its callback, read by anyone.
pub(crate) struct Heard {
    base: Instant,
    rate: f64,
    slots: Box<[Slot]>,
    /// The newest slot's number (from 1; 0: none yet).
    latest: AtomicU64,
    /// How long before it is heard the newest buffer was taken, ns.
    latency: AtomicU64,
}

/// One device buffer, behind a sequence lock: `seq` is odd while the
/// callback writes it.
#[derive(Default)]
struct Slot {
    seq: AtomicU64,
    number: AtomicU64,
    /// Ring position of the buffer's first sample frame, and how many ring
    /// frames it held (fewer than the buffer when the ring ran dry).
    counter: AtomicU64,
    len: AtomicU64,
    /// Nanoseconds after `base` the first one is heard.
    at: AtomicU64,
}

impl Heard {
    pub(crate) fn new(base: Instant, rate: u32) -> Self {
        Self {
            base,
            rate: f64::from(rate),
            slots: (0..SLOTS).map(|_| Slot::default()).collect(),
            latest: AtomicU64::new(0),
            latency: AtomicU64::new(0),
        }
    }

    /// How long before it is heard the device took its newest buffer: its
    /// output latency. `None` before it took one.
    pub(crate) fn latency(&self) -> Option<std::time::Duration> {
        (self.latest.load(Ordering::Acquire) > 0)
            .then(|| std::time::Duration::from_nanos(self.latency.load(Ordering::Relaxed)))
    }

    /// Records a device buffer: `len` ring frames from `counter` on, the
    /// first heard at `at`. The callback's side, and only it writes: no
    /// locks, no allocation.
    pub(crate) fn record(&self, counter: u64, len: u64, at: Instant) {
        let number = self.latest.load(Ordering::Relaxed) + 1;
        let slot = &self.slots[number as usize % SLOTS];
        let seq = slot.seq.load(Ordering::Relaxed);
        slot.seq.store(seq + 1, Ordering::Relaxed);
        fence(Ordering::Release);
        slot.number.store(number, Ordering::Relaxed);
        slot.counter.store(counter, Ordering::Relaxed);
        slot.len.store(len, Ordering::Relaxed);
        let nanos = at.saturating_duration_since(self.base).as_nanos() as u64;
        slot.at.store(nanos, Ordering::Relaxed);
        slot.seq.store(seq + 2, Ordering::Release);
        let ahead = at.saturating_duration_since(Instant::now()).as_nanos() as u64;
        self.latency.store(ahead, Ordering::Relaxed);
        self.latest.store(number, Ordering::Release);
    }

    /// The ring position heard at `now`: in the newest buffer that has
    /// begun to be heard, as far as the time since its start reaches, and
    /// no further than the ring frames it held. `None` before any has.
    pub(crate) fn counter_at(&self, now: Instant) -> Option<u64> {
        let now = now.saturating_duration_since(self.base).as_nanos() as u64;
        let latest = self.latest.load(Ordering::Acquire);
        let oldest = latest.saturating_sub(SLOTS as u64 - 2).max(1);
        for number in (oldest..=latest).rev() {
            let Some((counter, len, at)) = self.read(number) else {
                continue;
            };
            if at <= now {
                let since = ((now - at) as f64 * self.rate / 1e9) as u64;
                return Some(counter + since.min(len));
            }
        }
        None
    }

    /// Slot `number`'s buffer, if it still holds it.
    fn read(&self, number: u64) -> Option<(u64, u64, u64)> {
        let slot = &self.slots[number as usize % SLOTS];
        for _ in 0..8 {
            let s1 = slot.seq.load(Ordering::Acquire);
            if s1 % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let n = slot.number.load(Ordering::Relaxed);
            let fields = (
                slot.counter.load(Ordering::Relaxed),
                slot.len.load(Ordering::Relaxed),
                slot.at.load(Ordering::Relaxed),
            );
            fence(Ordering::Acquire);
            if slot.seq.load(Ordering::Relaxed) == s1 {
                return (n == number).then_some(fields);
            }
        }
        None
    }
}

/// Where a run of ring samples sits on the timeline: from ring position
/// `counter` on, sample `sample` (counted from the plan's start) moving
/// `speed` timeline samples per ring frame (negative backwards, 0 holds:
/// the end).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Marker {
    pub counter: u64,
    pub sample: i64,
    pub speed: f64,
    /// The play request the samples answer.
    pub epoch: u64,
}

#[derive(Default)]
pub(crate) struct Markers(Mutex<VecDeque<Marker>>);

impl Markers {
    fn lock(&self) -> MutexGuard<'_, VecDeque<Marker>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn push(&self, marker: Marker) {
        let mut markers = self.lock();
        markers.push_back(marker);
        // Far more than any position still unheard needs.
        while markers.len() > 256 {
            markers.pop_front();
        }
    }

    /// The timeline sample at ring position `counter` in play request
    /// `epoch`; `None` while that request's samples aren't there yet.
    pub(crate) fn sample_at(&self, counter: u64, epoch: u64) -> Option<f64> {
        let markers = self.lock();
        let m = markers
            .iter()
            .rev()
            .find(|m| m.epoch == epoch && m.counter <= counter)?;
        Some(m.sample as f64 + (counter - m.counter) as f64 * m.speed)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_clock_follows_what_was_heard_and_stops_where_the_sound_did() {
        let base = Instant::now();
        let heard = Heard::new(base, 1000);
        let ms = |n| base + Duration::from_millis(n);
        assert_eq!(heard.counter_at(ms(5)), None, "nothing played yet");
        // Ten frames heard from 100 ms, then a buffer from 110 ms with only
        // four ring frames (the ring ran dry), both written well before.
        heard.record(0, 10, ms(100));
        heard.record(10, 4, ms(110));
        assert_eq!(heard.counter_at(ms(50)), None, "not heard yet");
        assert_eq!(heard.counter_at(ms(100)), Some(0));
        assert_eq!(heard.counter_at(ms(105)), Some(5));
        assert_eq!(heard.counter_at(ms(112)), Some(12));
        assert_eq!(heard.counter_at(ms(130)), Some(14), "the sound stopped");
    }

    #[test]
    fn markers_map_ring_positions_to_the_timeline() {
        let markers = Markers::default();
        markers.push(Marker {
            counter: 100,
            sample: 4800,
            speed: 1.0,
            epoch: 1,
        });
        markers.push(Marker {
            counter: 300,
            sample: 9600,
            speed: -2.0,
            epoch: 2,
        });
        markers.push(Marker {
            counter: 400,
            sample: 0,
            speed: 0.0,
            epoch: 2,
        });
        assert_eq!(markers.sample_at(150, 1), Some(4850.0));
        assert_eq!(markers.sample_at(250, 2), None, "not yet heard");
        assert_eq!(markers.sample_at(310, 2), Some(9580.0));
        assert_eq!(markers.sample_at(450, 2), Some(0.0), "the end holds");
    }
}
