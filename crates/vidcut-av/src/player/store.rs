//! Decoded pictures, kept by what they show within a memory budget: those
//! ahead of the clock, those just shown, and windows decoded for going back.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use super::frame::Picture;
use super::plan::Id;

pub(crate) struct Store {
    inner: Mutex<Inner>,
    /// A picture landed.
    landed: Condvar,
}

struct Inner {
    /// Pictures, with when each was last kept or looked at (`clock`).
    pictures: HashMap<Id, (Arc<Picture>, u64)>,
    clock: u64,
    bytes: usize,
    budget: usize,
}

impl Store {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                pictures: HashMap::new(),
                clock: 0,
                bytes: 0,
                budget,
            }),
            landed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn budget(&self) -> usize {
        self.lock().budget
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

    /// Keeps `picture` as `id`'s. Over the budget, the pictures kept or
    /// looked at least recently go first, never the one just kept.
    pub(crate) fn put(&self, id: Id, picture: Arc<Picture>) {
        let mut inner = self.lock();
        inner.clock += 1;
        let (size, now) = (picture.bytes(), inner.clock);
        if let Some((old, _)) = inner.pictures.insert(id, (picture, now)) {
            inner.bytes -= old.bytes();
        }
        inner.bytes += size;
        while inner.bytes > inner.budget && inner.pictures.len() > 1 {
            let oldest = inner
                .pictures
                .iter()
                .filter(|(_, (_, used))| *used != now)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            let Some(oldest) = oldest else { break };
            if let Some((old, _)) = inner.pictures.remove(&oldest) {
                inner.bytes -= old.bytes();
            }
        }
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
        inner.bytes = 0;
    }

    /// How many pictures are kept, and their bytes.
    pub(crate) fn usage(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.pictures.len(), inner.bytes)
    }

    /// Wakes those waiting, to look again (the player closes).
    pub(crate) fn wake(&self) {
        self.landed.notify_all();
    }
}
