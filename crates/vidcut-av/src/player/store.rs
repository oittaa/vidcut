use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use super::frame::Picture;
use super::plan::Id;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Memory,
    Video,
}

impl Kind {
    pub(crate) fn of(picture: &Picture) -> Self {
        Self::kept_by(picture.in_video_memory())
    }

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

fn charges(picture: &Picture) -> [usize; 2] {
    let mut bytes = [0; 2];
    for (n, gpu) in picture.layers() {
        bytes[Kind::kept_by(gpu).index()] += n;
    }
    bytes
}

pub(crate) struct Store {
    inner: Mutex<Inner>,
    landed: Condvar,
    down: fn(&Picture) -> Result<Picture>,
}

struct Inner {
    pictures: HashMap<Id, (Arc<Picture>, u64)>,
    clock: u64,
    bytes: [usize; 2],
    budget: [usize; 2],
    shared: bool,
    held: Option<Id>,
    coming_down: Vec<(Id, Arc<Picture>)>,
}

impl Store {
    pub(crate) fn new(memory: usize, video: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                pictures: HashMap::new(),
                clock: 0,
                bytes: [0; 2],
                budget: [memory, video],
                shared: false,
                held: None,
                coming_down: Vec::new(),
            }),
            landed: Condvar::new(),
            down: Picture::copied_down,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn budget(&self, kind: Kind) -> usize {
        let inner = self.lock();
        inner.budget[inner.pool(kind).index()]
    }

    pub(crate) fn shared(&self) -> bool {
        self.lock().shared
    }

    pub(crate) fn share(&self, shared: bool) {
        let mut inner = self.lock();
        inner.shared = shared;
        let mut down = inner.evict(Kind::Memory, None);
        down.extend(inner.evict(Kind::Video, None));
        drop(inner);
        self.bring_down(down, None);
    }

    pub(crate) fn set_budget(&self, kind: Kind, bytes: usize) {
        let mut inner = self.lock();
        inner.budget[kind.index()] = bytes;
        let down = inner.evict(kind, None);
        drop(inner);
        self.bring_down(down, None);
    }

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

    pub(crate) fn contains(&self, id: &Id) -> bool {
        self.lock().key(id).is_some()
    }

    pub(crate) fn kept_run(&self, ids: impl IntoIterator<Item = Id>) -> usize {
        let inner = self.lock();
        ids.into_iter()
            .take_while(|id| inner.key(id).is_some())
            .count()
    }

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

    pub(crate) fn hold(&self, id: Option<Id>) {
        self.lock().held = id;
    }

    pub(crate) fn put(&self, id: Id, picture: Arc<Picture>) {
        let mut inner = self.lock();
        inner.clock += 1;
        let (charged, now) = (charges(&picture), inner.clock);
        if let Some((old, _)) = inner.pictures.insert(id.clone(), (picture, now)) {
            inner.uncharge(&old);
        }
        inner.charge(charged);
        let mut down = Vec::new();
        for kind in [Kind::Memory, Kind::Video] {
            if charged[kind.index()] > 0 {
                down.extend(inner.evict(kind, Some(&id)));
            }
        }
        drop(inner);
        self.landed.notify_all();
        self.bring_down(down, Some(&id));
    }

    fn bring_down(&self, mut down: Vec<(Id, Arc<Picture>)>, keep: Option<&Id>) {
        while let Some((id, picture)) = down.pop() {
            let copied = (self.down)(&picture);
            let mut inner = self.lock();
            inner
                .coming_down
                .retain(|(i, p)| !(*i == id && Arc::ptr_eq(p, &picture)));
            if let (true, Ok(copy)) = (inner.is_kept(&id, &picture), copied) {
                let used = inner.pictures[&id].1;
                inner.uncharge(&picture);
                inner.charge(charges(&copy));
                inner.pictures.insert(id, (Arc::new(copy), used));
            }
            for kind in [Kind::Memory, Kind::Video] {
                down.extend(inner.evict(kind, keep));
            }
        }
    }

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

    pub(crate) fn clear(&self) {
        let mut inner = self.lock();
        inner.pictures.clear();
        inner.bytes = [0; 2];
    }

    pub(crate) fn bytes(&self, kind: Kind) -> usize {
        self.lock().bytes[kind.index()]
    }

    pub(crate) fn usage(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.pictures.len(), inner.bytes.iter().sum())
    }

    pub(crate) fn wake(&self) {
        self.landed.notify_all();
    }
}

impl Inner {
    fn key(&self, id: &Id) -> Option<Id> {
        [id.clone(), id.small()]
            .into_iter()
            .find(|k| self.pictures.contains_key(k))
    }

    fn uncharge(&mut self, old: &Picture) {
        let charged = charges(old);
        for (bytes, c) in self.bytes.iter_mut().zip(charged) {
            *bytes -= c;
        }
    }

    fn charge(&mut self, charged: [usize; 2]) {
        for (bytes, c) in self.bytes.iter_mut().zip(charged) {
            *bytes += c;
        }
    }

    fn pool(&self, kind: Kind) -> Kind {
        if self.shared { Kind::Memory } else { kind }
    }

    fn counted(&self, pool: Kind) -> usize {
        let leaving = self.leaving();
        [Kind::Memory, Kind::Video]
            .into_iter()
            .filter(|k| self.pool(*k) == pool)
            .map(|k| self.bytes[k.index()] - leaving[k.index()])
            .sum()
    }

    fn leaving(&self) -> [usize; 2] {
        let mut bytes = [0; 2];
        for (id, picture) in &self.coming_down {
            if self.is_kept(id, picture) {
                for (b, c) in bytes.iter_mut().zip(charges(picture)) {
                    *b += c;
                }
            }
        }
        bytes
    }

    fn is_kept(&self, id: &Id, picture: &Arc<Picture>) -> bool {
        self.pictures
            .get(id)
            .is_some_and(|(p, _)| Arc::ptr_eq(p, picture))
    }

    fn charges_in(&self, picture: &Picture, pool: Kind) -> bool {
        let charged = charges(picture);
        [Kind::Memory, Kind::Video]
            .into_iter()
            .any(|k| self.pool(k) == pool && charged[k.index()] > 0)
    }

    fn evict(&mut self, kind: Kind, keep: Option<&Id>) -> Vec<(Id, Arc<Picture>)> {
        let pool = self.pool(kind);
        let room_for = keep
            .and_then(|k| self.pictures.get(k))
            .map_or_else(Vec::new, |(p, _)| p.pools());
        let mut down = Vec::new();
        while self.counted(pool) > self.budget[pool.index()] {
            let held = self.held.as_ref();
            let oldest = self
                .pictures
                .iter()
                .filter(|(k, (p, _))| {
                    self.charges_in(p, pool) && Some(*k) != keep && Some(*k) != held
                })
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, (p, _))| (k.clone(), p.pools()));
            let Some((oldest, pools)) = oldest else {
                break;
            };
            let others: Vec<usize> = pools
                .into_iter()
                .filter(|p| !room_for.contains(p))
                .collect();
            if others.is_empty()
                && let Some((old, _)) = self.pictures.remove(&oldest)
            {
                self.uncharge(&old);
            }
            for other in others {
                down.extend(self.let_pool_go(other, keep));
            }
        }
        down
    }

    fn let_pool_go(&mut self, pool: usize, keep: Option<&Id>) -> Option<(Id, Arc<Picture>)> {
        let ids: Vec<Id> = self
            .pictures
            .iter()
            .filter(|(k, (p, _))| p.holds(pool) && Some(*k) != keep)
            .map(|(k, _)| k.clone())
            .collect();
        let mut down = None;
        for id in ids {
            if self.held.as_ref() != Some(&id) {
                if let Some((old, _)) = self.pictures.remove(&id) {
                    self.uncharge(&old);
                }
                continue;
            }
            let picture = self.pictures[&id].0.clone();
            let on_its_way = self
                .coming_down
                .iter()
                .any(|(i, p)| *i == id && Arc::ptr_eq(p, &picture));
            if !on_its_way {
                self.coming_down.push((id.clone(), picture.clone()));
                down = Some((id, picture));
            }
        }
        down
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Kind, Store};
    use crate::ffmpeg::{format::Pixel, frame};
    use crate::player::frame::{Over, Picture};
    use crate::player::plan::Id;

    fn on_gpu() -> Picture {
        let mut f = frame::Video::empty();
        f.set_format(crate::hw::Kind::VideoToolbox.pixel());
        f.set_width(2);
        f.set_height(2);
        Picture::new(f, 0, None, None)
    }

    fn in_memory() -> Picture {
        Picture::new(frame::Video::new(Pixel::YUV420P, 2, 2), 0, None, None)
    }

    fn picture(gpu: bool) -> Arc<Picture> {
        Arc::new(if gpu { on_gpu() } else { in_memory() })
    }

    fn coming_down_to_memory(memory: usize, video: usize) -> Store {
        let mut store = Store::new(memory, video);
        store.down = |_| Ok(in_memory());
        store
    }

    fn of_pool(pool: usize) -> Arc<Picture> {
        Arc::new(on_gpu().in_pool(pool))
    }

    fn under_one_of(pool: usize) -> Arc<Picture> {
        let over = Over {
            picture: of_pool(pool),
            weight: 1,
        };
        Arc::new(on_gpu().with_over(vec![over]))
    }

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
    fn the_picture_held_comes_down_to_memory_and_counts_there_when_its_pool_goes() {
        let store = coming_down_to_memory(18, 18);
        store.hold(Some(Id::at("clip", 0)));
        for (pos, pool) in [(0, 1), (1, 1), (2, 1), (3, 2)] {
            store.put(Id::at("clip", pos), of_pool(pool));
        }
        let bytes = [Kind::Memory, Kind::Video].map(|k| store.bytes(k));
        assert_eq!(bytes, [6, 6]);
    }

    #[test]
    fn memory_makes_room_for_the_picture_held_come_down() {
        let store = coming_down_to_memory(6, 12);
        store.put(Id::at("clip", 0), picture(false));
        store.hold(Some(Id::at("clip", 1)));
        for (pos, pool) in [(1, 1), (2, 1), (3, 2)] {
            store.put(Id::at("clip", pos), of_pool(pool));
        }
        assert_eq!(kept(&store), [false, true, false, true]);
    }

    #[test]
    fn a_dissolve_goes_with_the_pool_of_the_picture_over_it() {
        let store = Store::new(0, 18);
        store.put(Id::at("clip", 0), of_pool(1));
        store.put(Id::at("clip", 1), under_one_of(1));
        store.put(Id::at("clip", 2), of_pool(2));
        assert_eq!(kept(&store), [false, false, true, false]);
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
