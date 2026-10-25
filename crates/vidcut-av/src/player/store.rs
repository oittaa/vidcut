use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::Instant;

use vidcut_core::lru::Lru;

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

type Order = Lru<Id, (), { usize::MAX }>;

struct Inner {
    pictures: Lru<Id, Arc<Picture>, { usize::MAX }>,
    kinds: [Order; 2],
    pools: HashMap<usize, HashSet<Id>>,
    small: HashMap<u64, HashSet<Id>>,
    bytes: [usize; 2],
    budget: [usize; 2],
    shared: bool,
    held: HashMap<u64, Id>,
    owners: usize,
    coming_down: Vec<(Id, Arc<Picture>)>,
    lent: HashMap<u64, Loan>,
    loans: u64,
    cap: Option<usize>,
    lent_need: usize,
}

struct Loan {
    need: usize,
    cap: usize,
}

pub(crate) struct Lent(u64);

impl Drop for Lent {
    fn drop(&mut self) {
        let mut inner = GLOBAL.lock();
        if let Some(loan) = inner.lent.remove(&self.0) {
            inner.lent_need -= loan.need;
            inner.cap = inner.lent.values().map(|l| l.cap).min();
        }
    }
}

pub(crate) fn room(spare: Option<usize>, card: Option<usize>, cache: bool) -> Option<usize> {
    let inner = GLOBAL.lock();
    let kept = if cache {
        inner.bytes[Kind::Video.index()]
    } else {
        0
    };
    match spare {
        Some(spare) => Some(spare + kept),
        None => card.map(|c| c.saturating_sub(super::GB + inner.lent_need)),
    }
}

pub(crate) fn lend(need: usize, room: usize) -> Lent {
    let mut inner = GLOBAL.lock();
    inner.loans += 1;
    let id = inner.loans;
    let cap = room.saturating_sub(need);
    inner.lent.insert(id, Loan { need, cap });
    inner.lent_need += need;
    inner.cap = Some(inner.cap.map_or(cap, |c| c.min(cap)));
    let down = inner.evict(Kind::Video, None);
    drop(inner);
    GLOBAL.bring_down(down, None);
    Lent(id)
}

pub(crate) fn lent_cap() -> Option<usize> {
    GLOBAL.lock().cap
}

static GLOBAL: LazyLock<Store> = LazyLock::new(|| Store::new(super::GB, super::GB));

pub(crate) fn global() -> &'static Store {
    &GLOBAL
}

pub(crate) struct Share {
    owner: u64,
    part: usize,
}

impl Share {
    pub(crate) fn join(owner: u64, part: usize) -> Self {
        GLOBAL.lock().owners += 1;
        Self {
            owner,
            part: part.max(1),
        }
    }

    pub(crate) fn budget(&self, kind: Kind) -> usize {
        GLOBAL.budget(kind) / self.part
    }

    pub(crate) fn get(&self, id: &Id) -> Option<Arc<Picture>> {
        GLOBAL.get(id, self.owner)
    }

    pub(crate) fn contains(&self, id: &Id) -> bool {
        GLOBAL.contains(id, self.owner)
    }

    pub(crate) fn kept_run(&self, ids: impl IntoIterator<Item = Id>) -> usize {
        GLOBAL.kept_run(ids, self.owner)
    }

    pub(crate) fn touch(&self, ids: impl IntoIterator<Item = Id>) {
        GLOBAL.touch(ids, self.owner);
    }

    pub(crate) fn hold(&self, id: Option<Id>) {
        GLOBAL.hold(self.owner, id);
    }

    pub(crate) fn wait(&self, id: &Id, deadline: Instant) -> Option<Arc<Picture>> {
        GLOBAL.wait(id, deadline, self.owner)
    }

    pub(crate) fn forget_small(&self) {
        GLOBAL.forget_small(self.owner);
    }

    pub(crate) fn small(&self, id: &Id) -> Id {
        id.small(self.owner)
    }
}

impl std::ops::Deref for Share {
    type Target = Store;

    fn deref(&self) -> &Store {
        &GLOBAL
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        GLOBAL.leave(self.owner);
    }
}

impl Store {
    pub(crate) fn new(memory: usize, video: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                pictures: Lru::new(),
                kinds: [Lru::new(), Lru::new()],
                pools: HashMap::new(),
                small: HashMap::new(),
                bytes: [0; 2],
                budget: [memory, video],
                shared: false,
                held: HashMap::new(),
                owners: 0,
                coming_down: Vec::new(),
                lent: HashMap::new(),
                loans: 0,
                cap: None,
                lent_need: 0,
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
        inner.limit(inner.pool(kind))
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

    pub(crate) fn get(&self, id: &Id, owner: u64) -> Option<Arc<Picture>> {
        let mut inner = self.lock();
        let key = inner.key(id, owner)?;
        inner.touch(&key)
    }

    pub(crate) fn contains(&self, id: &Id, owner: u64) -> bool {
        self.lock().key(id, owner).is_some()
    }

    pub(crate) fn kept_run(&self, ids: impl IntoIterator<Item = Id>, owner: u64) -> usize {
        let inner = self.lock();
        ids.into_iter()
            .take_while(|id| inner.key(id, owner).is_some())
            .count()
    }

    pub(crate) fn forget_small(&self, owner: u64) {
        let gone = self.lock().forget_small(owner);
        drop(gone);
    }

    pub(crate) fn touch(&self, ids: impl IntoIterator<Item = Id>, owner: u64) {
        let mut inner = self.lock();
        for id in ids {
            if let Some(key) = inner.key(&id, owner) {
                inner.touch(&key);
            }
        }
    }

    pub(crate) fn hold(&self, owner: u64, id: Option<Id>) {
        let mut inner = self.lock();
        match id {
            Some(id) => inner.held.insert(owner, id),
            None => inner.held.remove(&owner),
        };
    }

    fn leave(&self, owner: u64) {
        let mut inner = self.lock();
        inner.held.remove(&owner);
        inner.owners = inner.owners.saturating_sub(1);
        let gone = if inner.owners == 0 {
            inner.empty()
        } else {
            inner.forget_small(owner)
        };
        drop(inner);
        drop(gone);
    }

    pub(crate) fn put(&self, id: Id, picture: Arc<Picture>) {
        let mut inner = self.lock();
        let charged = charges(&picture);
        inner.insert(id.clone(), picture);
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
                inner.insert(id, Arc::new(copy));
            }
            for kind in [Kind::Memory, Kind::Video] {
                down.extend(inner.evict(kind, keep));
            }
        }
    }

    pub(crate) fn wait(&self, id: &Id, deadline: Instant, owner: u64) -> Option<Arc<Picture>> {
        let mut inner = self.lock();
        loop {
            if let Some(picture) = inner.key(id, owner).and_then(|k| inner.pictures.peek(&k)) {
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
        let gone = self.lock().empty();
        drop(gone);
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
    fn key(&self, id: &Id, owner: u64) -> Option<Id> {
        [id.clone(), id.small(owner)]
            .into_iter()
            .find(|k| self.pictures.contains(k))
    }

    fn insert(&mut self, id: Id, picture: Arc<Picture>) {
        self.remove(&id);
        let charged = charges(&picture);
        self.charge(charged);
        for kind in [Kind::Memory, Kind::Video] {
            if charged[kind.index()] > 0 {
                self.kinds[kind.index()].insert(id.clone(), ());
            }
        }
        for pool in picture.pools() {
            self.pools.entry(pool).or_default().insert(id.clone());
        }
        if let Some(owner) = id.small_owner() {
            self.small.entry(owner).or_default().insert(id.clone());
        }
        self.pictures.insert(id, picture);
    }

    fn remove(&mut self, id: &Id) -> Option<Arc<Picture>> {
        let (_, picture) = self.pictures.remove(id)?;
        self.uncharge(&picture);
        for order in &mut self.kinds {
            order.remove(id);
        }
        for pool in picture.pools() {
            if let Some(ids) = self.pools.get_mut(&pool) {
                ids.remove(id);
                if ids.is_empty() {
                    self.pools.remove(&pool);
                }
            }
        }
        if let Some(owner) = id.small_owner()
            && let Some(ids) = self.small.get_mut(&owner)
        {
            ids.remove(id);
            if ids.is_empty() {
                self.small.remove(&owner);
            }
        }
        Some(picture)
    }

    fn touch(&mut self, id: &Id) -> Option<Arc<Picture>> {
        for order in &mut self.kinds {
            order.get(id);
        }
        self.pictures.get(id).cloned()
    }

    fn empty(&mut self) -> Vec<Arc<Picture>> {
        self.kinds = [Lru::new(), Lru::new()];
        self.pools.clear();
        self.small.clear();
        self.bytes = [0; 2];
        let all = std::mem::take(&mut self.pictures);
        all.iter().map(|(_, p)| p.clone()).collect()
    }

    fn is_held(&self, id: &Id) -> bool {
        self.held.values().any(|h| h == id)
    }

    fn forget_small(&mut self, owner: u64) -> Vec<Arc<Picture>> {
        let ids = self.small.remove(&owner).unwrap_or_default();
        ids.iter().filter_map(|id| self.remove(id)).collect()
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

    fn limit(&self, pool: Kind) -> usize {
        let budget = self.budget[pool.index()];
        if pool != self.pool(Kind::Video) {
            return budget;
        }
        self.cap.map_or(budget, |c| budget.min(c))
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
            .peek(id)
            .is_some_and(|p| Arc::ptr_eq(p, picture))
    }

    fn oldest(&self, pool: Kind, keep: Option<&Id>) -> Option<Id> {
        let free = |id: &&Id| Some(*id) != keep && !self.is_held(id);
        let found = if self.shared {
            self.pictures.iter().map(|(id, _)| id).find(free)
        } else {
            self.kinds[pool.index()].iter().map(|(id, _)| id).find(free)
        };
        found.cloned()
    }

    fn evict(&mut self, kind: Kind, keep: Option<&Id>) -> Vec<(Id, Arc<Picture>)> {
        let pool = self.pool(kind);
        let room_for = keep
            .and_then(|k| self.pictures.peek(k))
            .map_or_else(Vec::new, |p| p.pools());
        let mut down = Vec::new();
        while self.counted(pool) > self.limit(pool) {
            let Some(oldest) = self.oldest(pool, keep) else {
                break;
            };
            let others: Vec<usize> = self
                .pictures
                .peek(&oldest)
                .map(|p| p.pools())
                .unwrap_or_default()
                .into_iter()
                .filter(|p| !room_for.contains(p))
                .collect();
            if others.is_empty() {
                self.remove(&oldest);
            }
            for other in others {
                down.extend(self.let_pool_go(other, keep));
            }
        }
        down
    }

    fn let_pool_go(&mut self, pool: usize, keep: Option<&Id>) -> Option<(Id, Arc<Picture>)> {
        let ids: Vec<Id> = self
            .pools
            .get(&pool)
            .into_iter()
            .flatten()
            .filter(|id| Some(*id) != keep)
            .cloned()
            .collect();
        let mut down = None;
        for id in ids {
            if !self.is_held(&id) {
                self.remove(&id);
                continue;
            }
            let Some(picture) = self.pictures.peek(&id).cloned() else {
                continue;
            };
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
            wipe: None,
        };
        Arc::new(on_gpu().with_over(vec![over]))
    }

    fn kept(store: &Store) -> [bool; 4] {
        [0, 1, 2, 3].map(|pos| store.contains(&Id::at("clip", pos), 0))
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
        store.hold(0, Some(Id::at("clip", 0)));
        for (pos, pool) in [(0, 1), (1, 1), (2, 1), (3, 2)] {
            store.put(Id::at("clip", pos), of_pool(pool));
        }
        assert_eq!(kept(&store), [true, false, false, true]);
    }

    #[test]
    fn the_picture_held_comes_down_to_memory_and_counts_there_when_its_pool_goes() {
        let store = coming_down_to_memory(18, 18);
        store.hold(0, Some(Id::at("clip", 0)));
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
        store.hold(0, Some(Id::at("clip", 1)));
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
        let kept = [0, 1, 2, 3].map(|pos| store.contains(&Id::at("clip", pos), 0));
        assert_eq!(kept, [false, true, true, true]);
    }
}
