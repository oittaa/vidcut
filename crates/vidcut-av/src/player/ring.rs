//! The mixed samples between the player's audio thread and the device's
//! callback: one writer, one reader, no locks, so the callback never waits.
//!
//! Positions are counted in sample frames (one sample per channel) since the
//! ring was made, and never wrap; the clock maps them to timeline samples.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub(crate) struct Ring {
    /// Interleaved samples, as `f32` bits.
    data: Box<[AtomicU32]>,
    channels: usize,
    /// Sample frames it holds.
    capacity: u64,
    /// Frames written and read so far.
    write: AtomicU64,
    read: AtomicU64,
    /// Frames before this are dropped unread: written for a position the
    /// player has left (a seek, a pause).
    discard: AtomicU64,
}

impl Ring {
    pub(crate) fn new(capacity: usize, channels: u16) -> Self {
        let channels = usize::from(channels.max(1));
        Self {
            data: (0..capacity * channels)
                .map(|_| AtomicU32::new(0))
                .collect(),
            channels,
            capacity: capacity as u64,
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            discard: AtomicU64::new(0),
        }
    }

    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    /// Frames written so far (the writer's position).
    pub(crate) fn written(&self) -> u64 {
        self.write.load(Ordering::Acquire)
    }

    /// Frames read so far (the reader's position).
    pub(crate) fn read_position(&self) -> u64 {
        self.read.load(Ordering::Acquire)
    }

    /// Frames written and not yet read or dropped.
    pub(crate) fn queued(&self) -> u64 {
        let w = self.written();
        let from = self
            .read_position()
            .max(self.discard.load(Ordering::Acquire));
        w.saturating_sub(from)
    }

    /// Frames the writer can write now. Dropped frames the reader hasn't
    /// passed yet still take room: it may be reading them.
    pub(crate) fn room(&self) -> u64 {
        self.capacity - (self.written() - self.read_position())
    }

    /// Writes as much of `samples` (interleaved) as there is room for;
    /// returns the frames written. The writer's side.
    pub(crate) fn push(&self, samples: &[f32]) -> usize {
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let room = self.capacity - (w - r);
        let n = ((samples.len() / self.channels) as u64).min(room);
        for i in 0..n {
            let at = ((w + i) % self.capacity) as usize * self.channels;
            let from = i as usize * self.channels;
            for c in 0..self.channels {
                self.data[at + c].store(samples[from + c].to_bits(), Ordering::Relaxed);
            }
        }
        self.write.store(w + n, Ordering::Release);
        n as usize
    }

    /// Drops everything written so far. The writer's side.
    pub(crate) fn discard_all(&self) {
        self.discard
            .store(self.write.load(Ordering::Relaxed), Ordering::Release);
    }

    /// Fills the start of `out` (interleaved) with the next frames; returns
    /// the position of the first and how many there were. The reader's
    /// side: no locks, no allocation.
    pub(crate) fn pop(&self, out: &mut [f32]) -> (u64, usize) {
        let mut r = self.read.load(Ordering::Relaxed);
        r = r.max(self.discard.load(Ordering::Acquire));
        let w = self.write.load(Ordering::Acquire);
        let n = (w - r).min((out.len() / self.channels) as u64);
        for i in 0..n {
            let at = ((r + i) % self.capacity) as usize * self.channels;
            let to = i as usize * self.channels;
            for c in 0..self.channels {
                out[to + c] = f32::from_bits(self.data[at + c].load(Ordering::Relaxed));
            }
        }
        self.read.store(r + n, Ordering::Release);
        (r, n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::Ring;

    #[test]
    fn samples_come_out_in_order_across_the_wrap() {
        let ring = Ring::new(4, 2);
        assert_eq!(ring.push(&[1.0, 1.5, 2.0, 2.5, 3.0, 3.5]), 3);
        let mut out = [0.0; 4];
        assert_eq!(ring.pop(&mut out), (0, 2));
        assert_eq!(out, [1.0, 1.5, 2.0, 2.5]);
        // Room for three more: one is still queued.
        assert_eq!(ring.push(&[4.0, 4.5, 5.0, 5.5, 6.0, 6.5, 7.0, 7.5]), 3);
        let mut out = [0.0; 8];
        assert_eq!(ring.pop(&mut out), (2, 4));
        assert_eq!(out, [3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5]);
        assert_eq!(ring.queued(), 0);
    }

    #[test]
    fn a_discard_drops_what_was_written_before_it() {
        let ring = Ring::new(8, 1);
        ring.push(&[1.0, 2.0, 3.0]);
        ring.discard_all();
        assert_eq!(ring.queued(), 0);
        ring.push(&[4.0, 5.0]);
        let mut out = [0.0; 4];
        assert_eq!(ring.pop(&mut out), (3, 2), "the first is the one after it");
        assert_eq!(&out[..2], &[4.0, 5.0]);
    }
}
