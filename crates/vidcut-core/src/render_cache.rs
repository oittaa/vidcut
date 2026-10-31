use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::disk_cache::Kind;
use crate::lru::Lru;
use crate::model::{ClipCache, CompositeMode, Project, RenderCache};
use crate::preferences::{ProxyCodec, RenderCachePrefs};
use crate::render::{RenderPlan, ShownTitle, VideoPiece};

pub type Key = [u8; 16];

pub const KEYS: &str = "keys";

pub const FILE: &str = "mkv";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    On,
    Media,
    Speed,
    Retime,
    AiRetime,
    SuperScale,
    Transition,
    Opacity,
    Composite,
}

impl Reason {
    pub fn title(self) -> &'static str {
        match self {
            Self::On => "Render Cache On",
            Self::Media => "Slow Media",
            Self::Speed => "Speed Change",
            Self::Retime => "Retime Process",
            Self::AiRetime => "AI Retime Process",
            Self::SuperScale => "AI Super Scale",
            Self::Transition => "Transition",
            Self::Opacity => "Opacity",
            Self::Composite => "Composite Mode",
        }
    }

    fn user(self, prefs: &RenderCachePrefs) -> bool {
        match self {
            Self::On => true,
            Self::Transition => prefs.transitions,
            Self::Opacity | Self::Composite => prefs.composites,
            Self::AiRetime | Self::SuperScale => prefs.ai,
            Self::Media | Self::Speed | Self::Retime => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Look {
    pub width: u32,
    pub height: u32,
    pub codec: String,
}

impl Look {
    pub fn of(project: &Project, proxies: (u32, ProxyCodec)) -> Self {
        let s = &project.settings;
        let (width, height) = match s.use_proxies {
            true => crate::model::proxy_size(s.width, s.height, proxies.0),
            false => (s.width, s.height),
        };
        let codec = match s.use_proxies {
            true => format!("proxy {}", serde_json::json!(proxies.1)),
            false => "auto".to_string(),
        };
        Self {
            width,
            height,
            codec,
        }
    }
}

pub struct Setup<'a> {
    pub mode: RenderCache,
    pub prefs: &'a RenderCachePrefs,
    pub project: &'a Project,
    pub slow: &'a dyn Fn(&str) -> bool,
    pub look: &'a Look,
}

impl Setup<'_> {
    pub fn reasons(&self, piece: &VideoPiece) -> Vec<Reason> {
        let mut out = reasons(piece, self.slow);
        out.retain(|r| match self.mode {
            RenderCache::Off => false,
            RenderCache::Smart => true,
            RenderCache::User => r.user(self.prefs),
        });
        out
    }

    pub fn wanted(&self, piece: &VideoPiece) -> bool {
        !self.reasons(piece).is_empty()
    }

    fn name(&self, media: &str, path: &Path) -> String {
        self.project
            .media(media)
            .and_then(|m| m.fingerprint.clone())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
    }

    fn stem(&self, plan: &RenderPlan) -> String {
        format!(
            "{}x{}|{}|{:?}|{}|{}",
            self.look.width,
            self.look.height,
            plan.fps,
            plan.colors,
            self.look.codec,
            env!("CARGO_PKG_VERSION")
        )
    }
}

pub fn reasons(piece: &VideoPiece, slow: &dyn Fn(&str) -> bool) -> Vec<Reason> {
    let mut out = Vec::new();
    let layers = piece.layers();
    let motions = piece.motions();
    let smooths = piece.smooths();
    for (i, (layer, ramp)) in layers.iter().enumerate() {
        let moved = i > 0 && (ramp.is_some() || motions[i].is_some() || smooths[i].is_some());
        let VideoPiece::Source {
            media,
            image,
            pace,
            super_scale,
            retime,
            placed,
            cache,
            ..
        } = layer
        else {
            if moved {
                out.push(Reason::Transition);
            }
            continue;
        };
        let own = *cache != ClipCache::Off;
        let still = matches!(image, Some(crate::render::Image::Still));
        let found = [
            (*cache == ClipCache::On, Reason::On),
            (own && image.is_none() && slow(media), Reason::Media),
            (!still && !pace.is_normal(), Reason::Speed),
            (
                !still && !retime.is_nearest() && !retime.ai(),
                Reason::Retime,
            ),
            (!still && retime.ai(), Reason::AiRetime),
            (own && super_scale.ai(), Reason::SuperScale),
            (moved, Reason::Transition),
            (
                placed
                    .iter()
                    .any(|p| p.picture.opaque() < 1.0 || !p.picture.opacity_keys.is_empty()),
                Reason::Opacity,
            ),
            (
                placed
                    .iter()
                    .any(|p| p.picture.mode() != CompositeMode::Normal),
                Reason::Composite,
            ),
        ];
        out.extend(found.into_iter().filter(|(on, _)| *on).map(|(_, r)| r));
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn bare(piece: &VideoPiece) -> &VideoPiece {
    match piece {
        VideoPiece::Titled { under, .. } => under,
        other => other,
    }
}

fn plain(piece: &mut VideoPiece, name: &dyn Fn(&str, &Path) -> String) {
    match piece {
        VideoPiece::Source {
            media,
            path,
            placed,
            cache,
            ..
        } => {
            *media = name(media, path);
            *path = PathBuf::new();
            *cache = ClipCache::Auto;
            for p in placed {
                p.clip.clear();
                if p.ready {
                    p.start = 0;
                }
            }
        }
        VideoPiece::Black { .. } => {}
        VideoPiece::Mix { under, over } => {
            plain(under, name);
            for o in over {
                plain(&mut o.piece, name);
            }
        }
        VideoPiece::Titled { under, titles } => {
            plain(under, name);
            titles.iter_mut().for_each(plain_title);
        }
    }
}

fn plain_title(t: &mut ShownTitle) {
    t.clip.clear();
    for p in &mut t.placed {
        p.clip.clear();
        if p.ready {
            p.start = 0;
        }
    }
}

fn digest(parts: &[&[u8]]) -> Key {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    let full = h.finalize();
    let mut key = [0; 16];
    key.copy_from_slice(&full[..16]);
    key
}

pub fn hex(key: &Key) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub start: i64,
    pub frames: i64,
    pub reasons: Vec<Reason>,
    pub cached: Vec<(i64, i64)>,
}

impl Part {
    pub fn cached_frames(&self) -> i64 {
        self.cached.iter().map(|(a, b)| b - a).sum()
    }

    pub fn holds(&self, frame: i64) -> bool {
        (self.start..self.start + self.frames).contains(&frame)
    }

    pub fn has(&self, frame: i64) -> bool {
        self.cached.iter().any(|&(a, b)| (a..b).contains(&frame))
    }
}

#[derive(Default)]
pub struct Cache {
    keys: Lru<Key, Arc<[Key]>, 4096>,
    index: Index,
}

impl Cache {
    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn index_mut(&mut self) -> &mut Index {
        &mut self.index
    }

    pub fn keys(&mut self, piece: &VideoPiece, setup: &Setup, plan: &RenderPlan) -> Arc<[Key]> {
        let bare = bare(piece);
        let stem = setup.stem(plan);
        let name = |m: &str, p: &Path| setup.name(m, p);
        let mut whole = bare.clone();
        plain(&mut whole, &name);
        let whole = serde_json::to_vec(&whole).unwrap_or_default();
        let memo = digest(&[stem.as_bytes(), &whole]);
        if let Some(keys) = self.keys.get(&memo) {
            return keys.clone();
        }
        let keys: Arc<[Key]> = (0..bare.frames())
            .map(|k| {
                let mut one = bare.part(k, 1);
                plain(&mut one, &name);
                let json = serde_json::to_vec(&one).unwrap_or_default();
                digest(&[stem.as_bytes(), &json])
            })
            .collect();
        self.keys.insert(memo, keys.clone());
        keys
    }

    pub fn parts(&mut self, plan: &RenderPlan, setup: &Setup) -> Vec<Part> {
        let mut out = Vec::new();
        let mut t = plan.range.0;
        for piece in &plan.video {
            let frames = piece.frames();
            let reasons = setup.reasons(piece);
            if !reasons.is_empty() {
                let keys = self.keys(piece, setup, plan);
                let mut cached: Vec<(i64, i64)> = Vec::new();
                for (k, key) in keys.iter().enumerate() {
                    if !self.index.has(key) {
                        continue;
                    }
                    let at = t + k as i64;
                    match cached.last_mut() {
                        Some((_, end)) if *end == at => *end += 1,
                        _ => cached.push((at, at + 1)),
                    }
                }
                out.push(Part {
                    start: t,
                    frames,
                    reasons,
                    cached,
                });
            }
            t += frames;
        }
        out
    }

    pub fn next(
        &mut self,
        plan: &RenderPlan,
        setup: &Setup,
        from: i64,
        most: i64,
    ) -> Option<(i64, Vec<Key>)> {
        let mut todo: Vec<(i64, Vec<(i64, Key)>)> = Vec::new();
        let mut t = plan.range.0;
        for piece in &plan.video {
            let frames = piece.frames();
            if setup.wanted(piece) {
                let keys = self.keys(piece, setup, plan);
                let missing: Vec<(i64, Key)> = keys
                    .iter()
                    .enumerate()
                    .filter(|(_, k)| !self.index.has(k))
                    .map(|(k, key)| (t + k as i64, *key))
                    .collect();
                if !missing.is_empty() {
                    todo.push((t, missing));
                }
            }
            t += frames;
        }
        let all: Vec<(i64, Key)> = todo.into_iter().flat_map(|(_, m)| m).collect();
        let first = all
            .iter()
            .position(|(t, _)| *t >= from)
            .or((!all.is_empty()).then_some(0))?;
        let start = all[first].0;
        let run: Vec<Key> = all[first..]
            .iter()
            .enumerate()
            .take_while(|(i, (t, _))| *t == start + *i as i64 && (*i as i64) < most)
            .map(|(_, (_, key))| *key)
            .collect();
        Some((start, run))
    }

    pub fn apply(&mut self, plan: &mut RenderPlan, setup: &Setup, exists: &dyn Fn(&Path) -> bool) {
        if setup.mode == RenderCache::Off || self.index.is_empty() {
            return;
        }
        let pieces = std::mem::take(&mut plan.video);
        let mut video = Vec::with_capacity(pieces.len());
        let mut seen: HashMap<Arc<Path>, bool> = HashMap::new();
        let mut there = |f: &Arc<Path>| *seen.entry(f.clone()).or_insert_with(|| exists(f));
        for piece in pieces {
            if !setup.wanted(&piece) {
                video.push(piece);
                continue;
            }
            let keys = self.keys(&piece, setup, plan);
            let found: Vec<Option<(Arc<Path>, u32)>> = keys
                .iter()
                .map(|k| self.index.find(k).filter(|(f, _)| there(f)))
                .collect();
            if found.iter().all(Option::is_none) {
                video.push(piece);
                continue;
            }
            let mut k = 0;
            while k < found.len() {
                let mut n = 1;
                while k + n < found.len() && follows(&found[k + n - 1], &found[k + n]) {
                    n += 1;
                }
                let part = piece.part(k as i64, n as i64);
                video.push(match &found[k] {
                    Some((file, at)) => cached(&part, file, *at, n as i64),
                    None => part,
                });
                k += n;
            }
        }
        plan.video = video;
        let gone: Vec<Arc<Path>> = seen
            .into_iter()
            .filter(|(_, there)| !there)
            .map(|(f, _)| f)
            .collect();
        for f in &gone {
            self.index.remove(f);
        }
    }
}

fn follows(a: &Option<(Arc<Path>, u32)>, b: &Option<(Arc<Path>, u32)>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some((f, i)), Some((g, j))) => f == g && *j == i + 1,
        _ => false,
    }
}

fn cached(part: &VideoPiece, file: &Path, at: u32, frames: i64) -> VideoPiece {
    crate::disk_cache::used(file);
    let source = VideoPiece::Source {
        media: String::new(),
        path: file.to_path_buf(),
        origin: 0.0,
        source_frame: i64::from(at),
        frames,
        image: None,
        pace: Default::default(),
        color_space: Default::default(),
        super_scale: Default::default(),
        retime: Default::default(),
        placed: Vec::new(),
        cache: ClipCache::Auto,
    };
    match part {
        VideoPiece::Titled { titles, .. } => VideoPiece::Titled {
            under: Box::new(source),
            titles: titles.clone(),
        },
        _ => source,
    }
}

pub fn segment(plan: &RenderPlan, look: &Look, start: i64, frames: i64) -> RenderPlan {
    let mut video = Vec::new();
    let mut t = plan.range.0;
    for piece in &plan.video {
        let n = piece.frames();
        let (a, b) = (start.max(t), (start + frames).min(t + n));
        if a < b {
            video.push(bare(&piece.part(a - t, b - a)).clone());
        }
        t += n;
    }
    RenderPlan {
        fps: plan.fps,
        width: look.width,
        height: look.height,
        sample_rate: plan.sample_rate,
        channels: plan.channels,
        frames,
        video,
        audio: Vec::new(),
        ducking: Vec::new(),
        range: (start, start + frames),
        name: plan.name.clone(),
        colors: plan.colors,
    }
}

#[derive(Default)]
pub struct Index {
    frames: HashMap<Key, (Arc<Path>, u32)>,
    files: HashMap<Arc<Path>, Arc<[Key]>>,
}

impl Index {
    pub fn dir() -> PathBuf {
        Kind::Render.dir()
    }

    pub fn scan(dir: &Path) -> Self {
        let mut index = Self::default();
        let Ok(listing) = std::fs::read_dir(dir) else {
            return index;
        };
        for e in listing.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != KEYS) {
                continue;
            }
            let file = path.with_extension(FILE);
            match (file.is_file(), read_keys(&path)) {
                (true, Some(keys)) => index.add(&file, &keys),
                _ => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        index
    }

    pub fn add(&mut self, file: &Path, keys: &[Key]) {
        let file: Arc<Path> = file.into();
        for (i, k) in keys.iter().enumerate() {
            self.frames.insert(*k, (file.clone(), i as u32));
        }
        self.files.insert(file, keys.into());
    }

    pub fn has(&self, key: &Key) -> bool {
        self.frames.contains_key(key)
    }

    pub fn find(&self, key: &Key) -> Option<(Arc<Path>, u32)> {
        self.frames.get(key).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn files(&self) -> impl Iterator<Item = (&Arc<Path>, &Arc<[Key]>)> {
        self.files.iter()
    }

    pub fn frames(&self) -> usize {
        self.frames.len()
    }

    pub fn remove(&mut self, file: &Path) {
        let Some(keys) = self.files.remove(file) else {
            return;
        };
        for k in keys.iter() {
            if self.frames.get(k).is_some_and(|(f, _)| &**f == file) {
                self.frames.remove(k);
            }
        }
        let _ = std::fs::remove_file(file);
        let _ = std::fs::remove_file(file.with_extension(KEYS));
    }

    pub fn clear(&mut self) {
        let files: Vec<Arc<Path>> = self.files.keys().cloned().collect();
        for f in files {
            self.remove(&f);
        }
    }

    pub fn keep_only(&mut self, keep: &std::collections::HashSet<Key>) -> usize {
        let unused: Vec<Arc<Path>> = self
            .files
            .iter()
            .filter(|(_, keys)| !keys.iter().any(|k| keep.contains(k)))
            .map(|(f, _)| f.clone())
            .collect();
        for f in &unused {
            self.remove(f);
        }
        unused.len()
    }

    pub fn drop_with(&mut self, gone: &std::collections::HashSet<Key>) -> usize {
        let hit: Vec<Arc<Path>> = self
            .files
            .iter()
            .filter(|(_, keys)| keys.iter().any(|k| gone.contains(k)))
            .map(|(f, _)| f.clone())
            .collect();
        for f in &hit {
            self.remove(f);
        }
        hit.len()
    }

    pub fn bytes(&self) -> u64 {
        self.files
            .keys()
            .filter_map(|f| std::fs::metadata(f).ok())
            .map(|m| m.len())
            .sum()
    }
}

pub fn name_of(keys: &[Key]) -> String {
    let all: Vec<u8> = keys.iter().flatten().copied().collect();
    hex(&digest(&[&all]))
}

pub fn write_keys(file: &Path, keys: &[Key]) -> std::io::Result<()> {
    let all: Vec<u8> = keys.iter().flatten().copied().collect();
    crate::temp::write(&file.with_extension(KEYS), &all)
}

fn read_keys(path: &Path) -> Option<Vec<Key>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() || !bytes.len().is_multiple_of(16) {
        return None;
    }
    Some(bytes.as_chunks::<16>().0.to_vec())
}
