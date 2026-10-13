//! Decoded pictures, kept by what they show within memory budgets, one for
//! memory and one for video memory (pictures NVDEC decoded for a display
//! that shows them from there): those ahead of the clock, those just shown,
//! windows decoded for going back and those around the playhead; and one
//! held whatever the budget, the frame of the plan played next.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use super::frame::{Memory, Picture};
use super::plan::Id;

/// Where a picture's pixels are, for the budgets: memory, or video memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Memory,
    Video,
}

impl Kind {
    pub(crate) fn of(picture: &Picture) -> Self {
        match picture.memory() {
            Memory::Cpu => Self::Memory,
            _ => Self::Video,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

pub(crate) struct Store {
    inner: Mutex<Inner>,
    /// A picture landed.
    landed: Condvar,
}

struct Inner {
    /// Pictures, with when each was last kept or looked at (`clock`).
    pictures: HashMap<Id, (Arc<Picture>, u64)>,
    clock: u64,
    /// Bytes kept and the budget, by [`Kind`].
    bytes: [usize; 2],
    budget: [usize; 2],
    /// Never let go for the budget.
    held: Option<Id>,
}

impl Store {
    /// `memory` and `video`: the budgets for pictures in memory and in
    /// video memory.
    pub(crate) fn new(memory: usize, video: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                pictures: HashMap::new(),
                clock: 0,
                bytes: [0; 2],
                budget: [memory, video],
                held: None,
            }),
            landed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The budget for pictures of `kind`.
    pub(crate) fn budget(&self, kind: Kind) -> usize {
        self.lock().budget[kind.index()]
    }

    /// Another budget for pictures of `kind`: over it, those kept or
    /// looked at least recently go at once.
    pub(crate) fn set_budget(&self, kind: Kind, bytes: usize) {
        let mut inner = self.lock();
        inner.budget[kind.index()] = bytes;
        inner.evict(kind, None);
    }

    /// The picture of `id`, counted as looked at.
    pub(crate) fn get(&self, id: &Id) -> Option<Arc<Picture>> {
        let mut inner = self.lock();
        inner.clock += 1;
        let now = inner.clock;
        inner.pictures.get_mut(id).map(|(picture, used)| {
            *used = now;
            picture.clone()
        })
    }

    pub(crate) fn contains(&self, id: &Id) -> bool {
        self.lock().pictures.contains_key(id)
    }

    /// How many of `ids`, in order, are kept before the first that isn't.
    pub(crate) fn kept_run(&self, ids: impl IntoIterator<Item = Id>) -> usize {
        let inner = self.lock();
        ids.into_iter()
            .take_while(|id| inner.pictures.contains_key(id))
            .count()
    }

    /// Counts the pictures of `ids` as looked at now, in order, the last
    /// most recently: pictures still to be shown go after those shown.
    pub(crate) fn touch(&self, ids: impl IntoIterator<Item = Id>) {
        let mut inner = self.lock();
        for id in ids {
            inner.clock += 1;
            let now = inner.clock;
            if let Some((_, used)) = inner.pictures.get_mut(&id) {
                *used = now;
            }
        }
    }

    /// Holds `id`'s picture whatever the budget (once it is kept), until
    /// another is held; `None` holds none.
    pub(crate) fn hold(&self, id: Option<Id>) {
        self.lock().held = id;
    }

    /// Keeps `picture` as `id`'s. Over its kind's budget, the pictures of
    /// that kind kept or looked at least recently go first, never the one
    /// just kept nor the one held.
    pub(crate) fn put(&self, id: Id, picture: Arc<Picture>) {
        let mut inner = self.lock();
        inner.clock += 1;
        let (kind, size, now) = (Kind::of(&picture), picture.bytes(), inner.clock);
        if let Some((old, _)) = inner.pictures.insert(id.clone(), (picture, now)) {
            inner.bytes[Kind::of(&old).index()] -= old.bytes();
        }
        inner.bytes[kind.index()] += size;
        inner.evict(kind, Some(&id));
        drop(inner);
        self.landed.notify_all();
    }

    /// The picture of `id`, waiting for it until `deadline`.
    pub(crate) fn wait(&self, id: &Id, deadline: Instant) -> Option<Arc<Picture>> {
        let mut inner = self.lock();
        loop {
            if let Some((picture, _)) = inner.pictures.get(id) {
                return Some(picture.clone());
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            inner = self
                .landed
                .wait_timeout(inner, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Forgets every picture (another decoder makes them from now on).
    pub(crate) fn clear(&self) {
        let mut inner = self.lock();
        inner.pictures.clear();
        inner.bytes = [0; 2];
    }

    /// How many pictures are kept, and their bytes.
    pub(crate) fn usage(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.pictures.len(), inner.bytes.iter().sum())
    }

    /// Wakes those waiting, to look again (the player closes).
    pub(crate) fn wake(&self) {
        self.landed.notify_all();
    }
}

impl Inner {
    /// Lets the pictures of `kind` kept or looked at least recently go
    /// while that kind is over its budget; never `keep` nor the one held.
    fn evict(&mut self, kind: Kind, keep: Option<&Id>) {
        while self.bytes[kind.index()] > self.budget[kind.index()] {
            let held = self.held.as_ref();
            let oldest = self
                .pictures
                .iter()
                .filter(|(k, (p, _))| Kind::of(p) == kind && Some(*k) != keep && Some(*k) != held)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            let Some(oldest) = oldest else { break };
            if let Some((old, _)) = self.pictures.remove(&oldest) {
                self.bytes[kind.index()] -= old.bytes();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::{ffi, format, frame};

    /// A 64x36 picture in memory, or one that says it is in video memory
    /// (no pixels: the store only counts them).
    fn picture(kind: Kind) -> Arc<Picture> {
        crate::init();
        let frame = match kind {
            Kind::Memory => frame::Video::new(format::Pixel::YUV420P, 64, 36),
            Kind::Video => {
                let mut f = frame::Video::empty();
                // SAFETY: plain fields of an empty frame.
                unsafe {
                    let raw = f.as_mut_ptr();
                    (*raw).format = ffi::AVPixelFormat::AV_PIX_FMT_CUDA as i32;
                    (*raw).width = 64;
                    (*raw).height = 36;
                }
                f
            }
        };
        Arc::new(Picture::new(frame, 0, None))
    }

    #[test]
    fn each_kind_of_memory_keeps_to_its_own_budget() {
        let one = picture(Kind::Memory).bytes();
        assert_eq!(picture(Kind::Video).bytes(), one);
        // Room for three pictures in memory, two in video memory.
        let store = Store::new(3 * one, 2 * one);
        let (m, v) = (|i| Id::of("m", i), |i| Id::of("v", i));
        store.put(m(0), picture(Kind::Memory));
        store.hold(Some(m(0)));
        for i in 1..6 {
            store.put(m(i), picture(Kind::Memory));
            store.put(v(i), picture(Kind::Video));
        }
        // The held one stays; the newest of each kind fill the rest, and
        // video memory's pictures pushed none of memory's out.
        assert!(store.contains(&m(0)));
        assert!(store.contains(&m(4)) && store.contains(&m(5)) && !store.contains(&m(3)));
        assert!(store.contains(&v(4)) && store.contains(&v(5)) && !store.contains(&v(3)));
        assert_eq!(store.usage(), (5, 5 * one));
        // A smaller budget lets the oldest go at once, never the held one.
        store.set_budget(Kind::Memory, one);
        assert!(store.contains(&m(0)) && !store.contains(&m(4)) && !store.contains(&m(5)));
        assert_eq!(store.usage(), (3, 3 * one));
    }

    #[test]
    fn pictures_touched_go_after_those_looked_at_before() {
        let one = picture(Kind::Memory).bytes();
        let store = Store::new(4 * one, 0);
        let m = |i| Id::of("m", i);
        // Kept 0 to 3; 3 and 2 shown, then 0 and 1 counted as looked at.
        for i in 0..4 {
            store.put(m(i), picture(Kind::Memory));
        }
        store.get(&m(3));
        store.get(&m(2));
        assert_eq!(store.kept_run([1, 0, -1].map(m)), 2);
        store.touch([0, 1].map(m));
        // Two more push out the two shown, the one shown first first.
        store.put(m(-1), picture(Kind::Memory));
        assert!(!store.contains(&m(3)) && store.contains(&m(2)));
        store.put(m(-2), picture(Kind::Memory));
        assert!(!store.contains(&m(2)));
        assert_eq!(store.kept_run((-2..=1).rev().map(m)), 4);
    }
}
