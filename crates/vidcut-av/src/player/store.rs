//! Decoded pictures, kept by what they show within memory budgets, one for
//! memory and one for video memory (pictures a GPU decoded, for a display
//! that shows them from there), or one for both where they share it: those
//! ahead of the clock, those just shown, windows decoded for going back and
//! those around the playhead; and one held whatever the budget, the frame
//! of the plan played next.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use super::frame::Picture;
use super::plan::Id;

/// Where a picture's pixels are, for the budgets: memory, or video memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Memory,
    Video,
}

impl Kind {
    pub(crate) fn of(picture: &Picture) -> Self {
        Self::kept_by(picture.in_video_memory())
    }

    /// Where a picture `gpu` decoded and kept counts (`None`: in memory).
    fn kept_by(gpu: Option<crate::hw::Kind>) -> Self {
        match gpu {
            None => Self::Memory,
            Some(_) => Self::Video,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The bytes `picture` takes of each budget: a dissolve's pictures each
/// count where their own frame is.
fn charges(picture: &Picture) -> [usize; 2] {
    let mut bytes = [0; 2];
    for (n, gpu) in picture.layers() {
        bytes[Kind::kept_by(gpu).index()] += n;
    }
    bytes
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
    /// Pictures in video memory count in the memory budget ([`Store::share`]).
    shared: bool,
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
                shared: false,
                held: None,
            }),
            landed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The budget pictures of `kind` count in.
    pub(crate) fn budget(&self, kind: Kind) -> usize {
        let inner = self.lock();
        inner.budget[inner.pool(kind).index()]
    }

    /// Whether pictures in video memory count in the memory budget.
    pub(crate) fn shared(&self) -> bool {
        self.lock().shared
    }

    /// Pictures in video memory count in the memory budget from now on
    /// (`true`), where video memory is the computer's, or in their own.
    /// Over a budget, those kept or looked at least recently go at once.
    pub(crate) fn share(&self, shared: bool) {
        let mut inner = self.lock();
        inner.shared = shared;
        inner.evict(Kind::Memory, None);
        inner.evict(Kind::Video, None);
    }

    /// Another budget for pictures of `kind`: over it, those kept or
    /// looked at least recently go at once.
    pub(crate) fn set_budget(&self, kind: Kind, bytes: usize) {
        let mut inner = self.lock();
        inner.budget[kind.index()] = bytes;
        inner.evict(kind, None);
    }

    /// The picture of `id`, as decoded or else shrunk, counted as looked
    /// at.
    pub(crate) fn get(&self, id: &Id) -> Option<Arc<Picture>> {
        let mut inner = self.lock();
        inner.clock += 1;
        let now = inner.clock;
        let key = inner.key(id)?;
        inner.pictures.get_mut(&key).map(|(picture, used)| {
            *used = now;
            picture.clone()
        })
    }

    /// Whether `id`'s picture is kept, as decoded or shrunk.
    pub(crate) fn contains(&self, id: &Id) -> bool {
        self.lock().key(id).is_some()
    }

    /// How many of `ids`, in order, are kept before the first that isn't.
    pub(crate) fn kept_run(&self, ids: impl IntoIterator<Item = Id>) -> usize {
        let inner = self.lock();
        ids.into_iter()
            .take_while(|id| inner.key(id).is_some())
            .count()
    }

    /// Forgets the shrunk pictures: they are shown at another size.
    pub(crate) fn forget_small(&self) {
        let mut inner = self.lock();
        let small: Vec<Id> = inner
            .pictures
            .keys()
            .filter(|id| id.is_small())
            .cloned()
            .collect();
        for id in small {
            if let Some((old, _)) = inner.pictures.remove(&id) {
                inner.uncharge(&old);
            }
        }
    }

    /// Counts the pictures of `ids` as looked at now, in order, the last
    /// most recently: pictures still to be shown go after those shown.
    pub(crate) fn touch(&self, ids: impl IntoIterator<Item = Id>) {
        let mut inner = self.lock();
        for id in ids {
            inner.clock += 1;
            let now = inner.clock;
            if let Some(key) = inner.key(&id)
                && let Some((_, used)) = inner.pictures.get_mut(&key)
            {
                *used = now;
            }
        }
    }

    /// Holds `id`'s picture whatever the budget (once it is kept), until
    /// another is held; `None` holds none.
    pub(crate) fn hold(&self, id: Option<Id>) {
        self.lock().held = id;
    }

    /// Keeps `picture` as `id`'s. Over the budget it counts in, the
    /// pictures counted there kept or looked at least recently go first,
    /// never the one just kept nor the one held.
    pub(crate) fn put(&self, id: Id, picture: Arc<Picture>) {
        let mut inner = self.lock();
        inner.clock += 1;
        let (charged, now) = (charges(&picture), inner.clock);
        if let Some((old, _)) = inner.pictures.insert(id.clone(), (picture, now)) {
            inner.uncharge(&old);
        }
        for kind in [Kind::Memory, Kind::Video] {
            inner.bytes[kind.index()] += charged[kind.index()];
        }
        for kind in [Kind::Memory, Kind::Video] {
            if charged[kind.index()] > 0 {
                inner.evict(kind, Some(&id));
            }
        }
        drop(inner);
        self.landed.notify_all();
    }

    /// The picture of `id`, waiting for it until `deadline`.
    pub(crate) fn wait(&self, id: &Id, deadline: Instant) -> Option<Arc<Picture>> {
        let mut inner = self.lock();
        loop {
            if let Some((picture, _)) = inner.key(id).and_then(|k| inner.pictures.get(&k)) {
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

    /// The bytes kept of `kind`.
    pub(crate) fn bytes(&self, kind: Kind) -> usize {
        self.lock().bytes[kind.index()]
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
    /// What `id`'s picture is kept as: as decoded, else shrunk.
    fn key(&self, id: &Id) -> Option<Id> {
        [id.clone(), id.small()]
            .into_iter()
            .find(|k| self.pictures.contains_key(k))
    }

    /// Takes what `old` charged off the budgets.
    fn uncharge(&mut self, old: &Picture) {
        let charged = charges(old);
        for (bytes, c) in self.bytes.iter_mut().zip(charged) {
            *bytes -= c;
        }
    }

    /// The kind whose budget pictures of `kind` count in.
    fn pool(&self, kind: Kind) -> Kind {
        if self.shared { Kind::Memory } else { kind }
    }

    /// The bytes counted in `pool`'s budget.
    fn counted(&self, pool: Kind) -> usize {
        [Kind::Memory, Kind::Video]
            .into_iter()
            .filter(|k| self.pool(*k) == pool)
            .map(|k| self.bytes[k.index()])
            .sum()
    }

    /// `picture` takes some of `pool`'s budget.
    fn charges_in(&self, picture: &Picture, pool: Kind) -> bool {
        let charged = charges(picture);
        [Kind::Memory, Kind::Video]
            .into_iter()
            .any(|k| self.pool(k) == pool && charged[k.index()] > 0)
    }

    /// Lets the pictures counted in `kind`'s budget, those kept or looked
    /// at least recently, go while it is over; never `keep` nor the one
    /// held. One whose memory is a pool's other than `keep`'s
    /// (`Picture::pooled`) goes with all of that pool's pictures: letting
    /// one go would free none of it.
    fn evict(&mut self, kind: Kind, keep: Option<&Id>) {
        let pool = self.pool(kind);
        let room_for = keep
            .and_then(|k| self.pictures.get(k))
            .and_then(|(p, _)| p.pooled());
        while self.counted(pool) > self.budget[pool.index()] {
            let held = self.held.as_ref();
            let oldest = self
                .pictures
                .iter()
                .filter(|(k, (p, _))| {
                    self.charges_in(p, pool) && Some(*k) != keep && Some(*k) != held
                })
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, (p, _))| (k.clone(), p.pooled()));
            let Some((oldest, pooled)) = oldest else {
                break;
            };
            match pooled {
                Some(other) if pooled != room_for => self.let_pool_go(other, keep),
                _ => {
                    if let Some((old, _)) = self.pictures.remove(&oldest) {
                        self.uncharge(&old);
                    }
                }
            }
        }
    }

    /// Lets all the pictures of `pooled` go but `keep`: the pool's memory
    /// goes with the last of them. The one held comes down to memory
    /// instead, where it can, or stays as it is.
    fn let_pool_go(&mut self, pooled: usize, keep: Option<&Id>) {
        let ids: Vec<Id> = self
            .pictures
            .iter()
            .filter(|(k, (p, _))| p.pooled() == Some(pooled) && Some(*k) != keep)
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            let Some((old, used)) = self.pictures.remove(&id) else {
                continue;
            };
            self.uncharge(&old);
            if self.held.as_ref() != Some(&id) {
                continue;
            }
            let picture = old.copied_down().map(Arc::new).unwrap_or(old);
            for (bytes, c) in self.bytes.iter_mut().zip(charges(&picture)) {
                *bytes += c;
            }
            self.pictures.insert(id, (picture, used));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Kind, Store};
    use crate::ffmpeg::{format::Pixel, frame};
    use crate::player::frame::Picture;
    use crate::player::plan::Id;

    /// A 2×2 picture on a GPU, 6 bytes.
    fn on_gpu() -> Picture {
        let mut f = frame::Video::empty();
        f.set_format(crate::hw::Kind::VideoToolbox.pixel());
        f.set_width(2);
        f.set_height(2);
        Picture::new(f, 0, None, None)
    }

    /// A 2×2 picture, 6 bytes: in memory, or on a GPU.
    fn picture(gpu: bool) -> Arc<Picture> {
        if gpu {
            return Arc::new(on_gpu());
        }
        Arc::new(Picture::new(
            frame::Video::new(Pixel::YUV420P, 2, 2),
            0,
            None,
            None,
        ))
    }

    /// A 2×2 picture on a GPU whose memory is `pool`'s, 6 bytes.
    fn of_pool(pool: usize) -> Arc<Picture> {
        Arc::new(on_gpu().in_pool(pool))
    }

    /// Which of the clip's pictures at `0..4` are kept.
    fn kept(store: &Store) -> [bool; 4] {
        [0, 1, 2, 3].map(|pos| store.contains(&Id::at("clip", pos)))
    }

    #[test]
    fn a_picture_of_another_pool_makes_room_by_letting_a_pool_go_whole() {
        let store = Store::new(0, 18);
        for (pos, pool) in [(0, 1), (1, 1), (2, 1), (3, 2)] {
            store.put(Id::at("clip", pos), of_pool(pool));
        }
        assert_eq!(kept(&store), [false, false, false, true]);
    }

    #[test]
    fn a_picture_of_the_same_pool_lets_only_the_oldest_go() {
        let store = Store::new(0, 18);
        for pos in 0..4 {
            store.put(Id::at("clip", pos), of_pool(1));
        }
        assert_eq!(kept(&store), [false, true, true, true]);
    }

    #[test]
    fn the_picture_held_stays_when_its_pool_goes() {
        let store = Store::new(0, 18);
        store.hold(Some(Id::at("clip", 0)));
        for (pos, pool) in [(0, 1), (1, 1), (2, 1), (3, 2)] {
            store.put(Id::at("clip", pos), of_pool(pool));
        }
        assert_eq!(kept(&store), [true, false, false, true]);
    }

    #[test]
    fn a_picture_kept_on_vulkan_counts_in_video_memory() {
        assert_eq!(Kind::kept_by(Some(crate::hw::Kind::Vulkan)), Kind::Video);
    }

    #[test]
    fn a_shared_budget_lets_the_oldest_picture_of_either_kind_go() {
        let store = Store::new(18, 0);
        store.share(true);
        for (pos, gpu) in [(0, false), (1, true), (2, false), (3, true)] {
            store.put(Id::at("clip", pos), picture(gpu));
        }
        let kept = [0, 1, 2, 3].map(|pos| store.contains(&Id::at("clip", pos)));
        assert_eq!(kept, [false, true, true, true]);
    }
}
