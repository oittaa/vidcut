use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::disk_cache::Kind;
use crate::lru::Lru;
use crate::model::{ClipCache, CompositeMode, Project, RenderCache, SuperScale, Upscaler};
use crate::preferences::{ProxyCodec, RenderCachePrefs};
use crate::render::{Placed, RenderPlan, ShownTitle, VideoPiece};

pub type Key = [u8; 16];

pub const KEYS: &str = "keys";

pub const FILE: &str = "mkv";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    On,
    Effect,
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
            Self::Effect => "Render Cache Effect",
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

    fn clip(self) -> bool {
        matches!(
            self,
            Self::On | Self::Media | Self::Speed | Self::SuperScale
        )
    }

    fn user(self, prefs: &RenderCachePrefs) -> bool {
        match self {
            Self::On | Self::Effect => true,
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
    pub longest: Option<u32>,
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
            longest: s.use_proxies.then_some(proxies.0),
        }
    }
}

pub struct Setup<'a> {
    pub mode: RenderCache,
    pub prefs: &'a RenderCachePrefs,
    pub project: &'a Project,
    pub slow: &'a dyn Fn(&str) -> bool,
    pub upscaled: &'a Upscaled,
    pub look: &'a Look,
}

pub type Upscaled = dyn Fn((u32, u32), (u32, u32), &[Placed], SuperScale) -> (u32, u32);

impl Setup<'_> {
    fn kept(&self, r: &Reason) -> bool {
        match self.mode {
            RenderCache::Off => false,
            RenderCache::Smart => true,
            RenderCache::User => r.user(self.prefs),
        }
    }

    pub fn reasons(&self, piece: &VideoPiece) -> Vec<Reason> {
        let mut out = reasons(piece, self.slow, self.mode == RenderCache::User);
        out.retain(|r| self.kept(r));
        out
    }

    fn levels(&self, piece: &VideoPiece) -> (bool, Vec<bool>) {
        let user = self.mode == RenderCache::User;
        let mut whole = titled(piece) && self.kept(&Reason::Effect);
        let clips = piece
            .layers()
            .iter()
            .zip(layered(piece, self.slow, user))
            .map(|((layer, _), found)| {
                let mut clip = false;
                let own = lone(layer)
                    && !matches!(
                        layer,
                        VideoPiece::Source {
                            cache: ClipCache::Off,
                            ..
                        }
                    );
                for r in found.iter().filter(|r| self.kept(r)) {
                    match r.clip() && own {
                        true => clip = true,
                        false => whole = true,
                    }
                }
                clip
            })
            .collect();
        (whole, clips)
    }

    fn sized(&self, layer: &VideoPiece) -> Option<(u32, u32)> {
        let VideoPiece::Source {
            media,
            super_scale,
            placed,
            ..
        } = layer
        else {
            return None;
        };
        let (w, h) = self
            .project
            .media(media)?
            .info
            .video
            .as_ref()?
            .display_size();
        let size = match self.look.longest {
            Some(l) => crate::model::proxy_size(w, h, l),
            None => (w, h),
        };
        let out = (self.look.width, self.look.height);
        Some(match super_scale.ai() {
            true => (self.upscaled)(size, out, placed, *super_scale),
            false => size,
        })
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

    fn clip_stem(&self, plan: &RenderPlan, (w, h): (u32, u32)) -> String {
        format!(
            "clip|{w}x{h}|{:?}|{}|{}",
            plan.colors,
            self.look.codec,
            env!("CARGO_PKG_VERSION")
        )
    }
}

fn lone(layer: &VideoPiece) -> bool {
    matches!(layer, VideoPiece::Source { image: None, retime, .. } if retime.is_nearest())
}

fn alone(layer: &VideoPiece) -> VideoPiece {
    let mut out = layer.clone();
    if let VideoPiece::Source {
        super_scale,
        placed,
        cache,
        excluded,
        ..
    } = &mut out
    {
        match super_scale.ai() {
            true => super_scale.sharpness = Default::default(),
            false => *super_scale = Default::default(),
        }
        placed.clear();
        *cache = ClipCache::Auto;
        *excluded = false;
    }
    out
}

fn checked(placed: &[Placed]) -> bool {
    placed
        .iter()
        .any(|p| p.picture.effects.iter().any(|e| e.on && e.render_cache))
}

fn titled(piece: &VideoPiece) -> bool {
    piece.titles().iter().any(|(_, t)| checked(&t.placed))
}

pub fn reasons(piece: &VideoPiece, slow: &dyn Fn(&str) -> bool, user: bool) -> Vec<Reason> {
    let mut out: Vec<Reason> = layered(piece, slow, user).into_iter().flatten().collect();
    if titled(piece) {
        out.push(Reason::Effect);
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn layered(piece: &VideoPiece, slow: &dyn Fn(&str) -> bool, user: bool) -> Vec<Vec<Reason>> {
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
            excluded,
            ..
        } = layer
        else {
            let excluded = matches!(layer, VideoPiece::Black { excluded: true, .. });
            out.push(match moved && !(user && excluded) {
                true => vec![Reason::Transition],
                false => Vec::new(),
            });
            continue;
        };
        let own = *cache != ClipCache::Off;
        let mixed = !(user && *excluded);
        let still = matches!(image, Some(crate::render::Image::Still));
        let found = [
            (*cache == ClipCache::On, Reason::On),
            (checked(placed), Reason::Effect),
            (own && image.is_none() && slow(media), Reason::Media),
            (mixed && !still && !pace.is_normal(), Reason::Speed),
            (
                mixed && !still && !retime.is_nearest() && !retime.ai(),
                Reason::Retime,
            ),
            (mixed && !still && retime.ai(), Reason::AiRetime),
            (own && super_scale.ai(), Reason::SuperScale),
            (mixed && moved, Reason::Transition),
            (
                mixed
                    && placed
                        .iter()
                        .any(|p| p.picture.opaque() < 1.0 || !p.picture.opacity_keys.is_empty()),
                Reason::Opacity,
            ),
            (
                mixed
                    && placed
                        .iter()
                        .any(|p| p.picture.mode() != CompositeMode::Normal),
                Reason::Composite,
            ),
        ];
        out.push(
            found
                .into_iter()
                .filter(|(on, _)| *on)
                .map(|(_, r)| r)
                .collect(),
        );
    }
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
            excluded,
            ..
        } => {
            *media = name(media, path);
            *path = PathBuf::new();
            *cache = ClipCache::Auto;
            *excluded = false;
            placed.iter_mut().for_each(plain_placed);
        }
        VideoPiece::Black { excluded, .. } => *excluded = false,
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
    t.placed.iter_mut().for_each(plain_placed);
}

fn plain_placed(p: &mut Placed) {
    p.clip.clear();
    if p.ready {
        p.start = 0;
    }
    p.picture
        .effects
        .iter_mut()
        .for_each(|e| e.render_cache = false);
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
    pub super_scales: Vec<SuperScale>,
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

    pub fn clip_keys(
        &mut self,
        layer: &VideoPiece,
        setup: &Setup,
        plan: &RenderPlan,
    ) -> Option<Arc<[Key]>> {
        let size = setup.sized(layer)?;
        let stem = setup.clip_stem(plan, size);
        let name = |m: &str, p: &Path| setup.name(m, p);
        let alone = alone(layer);
        let mut whole = alone.clone();
        plain(&mut whole, &name);
        let whole = serde_json::to_vec(&whole).unwrap_or_default();
        let memo = digest(&[stem.as_bytes(), &whole]);
        if let Some(keys) = self.keys.get(&memo) {
            return Some(keys.clone());
        }
        let keys: Arc<[Key]> = (0..alone.frames())
            .map(|k| {
                let mut one = alone.part(k, 1);
                if let VideoPiece::Source { pace, .. } = &mut one {
                    *pace = Default::default();
                }
                plain(&mut one, &name);
                let json = serde_json::to_vec(&one).unwrap_or_default();
                digest(&[stem.as_bytes(), &json])
            })
            .collect();
        self.keys.insert(memo, keys.clone());
        Some(keys)
    }

    fn levels(&mut self, piece: &VideoPiece, setup: &Setup, plan: &RenderPlan) -> Levels {
        let (mut whole, clips) = setup.levels(piece);
        let clips: Vec<Option<Arc<[Key]>>> = piece
            .layers()
            .iter()
            .zip(clips)
            .map(|((layer, _), clip)| {
                let keys = clip.then(|| self.clip_keys(layer, setup, plan)).flatten();
                whole |= clip && keys.is_none();
                keys
            })
            .collect();
        let whole = whole.then(|| self.keys(piece, setup, plan));
        (whole, clips)
    }

    pub fn names(
        &mut self,
        piece: &VideoPiece,
        setup: &Setup,
        plan: &RenderPlan,
        media: Option<&[String]>,
    ) -> Vec<Arc<[Key]>> {
        let mut out = vec![self.keys(piece, setup, plan)];
        for (layer, _) in piece.layers() {
            let mine = match (layer, media) {
                (_, None) => true,
                (VideoPiece::Source { media, .. }, Some(m)) => m.contains(media),
                _ => false,
            };
            if mine
                && lone(layer)
                && let Some(keys) = self.clip_keys(layer, setup, plan)
            {
                out.push(keys);
            }
        }
        out
    }

    pub fn parts(&mut self, plan: &RenderPlan, setup: &Setup) -> Vec<Part> {
        let mut out = Vec::new();
        let mut t = plan.range.0;
        for piece in &plan.video {
            let frames = piece.frames();
            let reasons = setup.reasons(piece);
            if !reasons.is_empty() {
                let (whole, clips) = self.levels(piece, setup, plan);
                let mut cached: Vec<(i64, i64)> = Vec::new();
                for k in 0..frames as usize {
                    let played = match &whole {
                        Some(keys) => self.index.has(&keys[k]),
                        None => clips.iter().flatten().all(|keys| self.index.has(&keys[k])),
                    };
                    if !played {
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
                    super_scales: piece.super_scales(),
                });
            }
            t += frames;
        }
        out
    }

    pub fn next(&mut self, plan: &RenderPlan, setup: &Setup, from: i64, most: i64) -> Option<Job> {
        let mut whole: Vec<(i64, Key)> = Vec::new();
        let mut ahead: Option<(i64, usize, usize, usize)> = None;
        let mut first: Option<(i64, usize, usize, usize)> = None;
        let mut t = plan.range.0;
        for (p, piece) in plan.video.iter().enumerate() {
            let (keys, clips) = self.levels(piece, setup, plan);
            if let Some(keys) = keys {
                whole.extend(
                    keys.iter()
                        .enumerate()
                        .filter(|(_, key)| !self.index.has(key))
                        .map(|(k, key)| (t + k as i64, *key)),
                );
            }
            for (i, keys) in clips.iter().enumerate() {
                let Some(keys) = keys else {
                    continue;
                };
                let missing = |k: &usize| !self.index.has(&keys[*k]);
                let at = |k: usize| (t + k as i64, p, i, k);
                if let Some(k) = (0..keys.len()).find(missing).map(at) {
                    first = first.filter(|f| f.0 <= k.0).or(Some(k));
                }
                let soon = |k: &usize| t + *k as i64 >= from && missing(k);
                if let Some(k) = (0..keys.len()).find(soon).map(at) {
                    ahead = ahead.filter(|a| a.0 <= k.0).or(Some(k));
                }
            }
            t += piece.frames();
        }
        let later = whole.iter().position(|(t, _)| *t >= from);
        let clip = match (ahead, later) {
            (Some(a), Some(w)) => (a.0 <= whole[w].0).then_some(a),
            (Some(a), None) => Some(a),
            (None, Some(_)) => None,
            (None, None) => first.filter(|f| whole.first().is_none_or(|w| f.0 <= w.0)),
        };
        if let Some((start, p, i, k)) = clip {
            let layer = plan.video[p].layers()[i].0.clone();
            let keys = self.clip_keys(&layer, setup, plan)?;
            let size = setup.sized(&layer)?;
            let run: Vec<Key> = keys[k..]
                .iter()
                .take_while(|key| !self.index.has(key))
                .take(most.max(1) as usize)
                .copied()
                .collect();
            let alone = alone(&layer.part(k as i64, run.len() as i64));
            return Some(Job {
                start,
                plan: clip_plan(plan, alone, size),
                keys: run,
                clip: true,
            });
        }
        let first = later.or((!whole.is_empty()).then_some(0))?;
        let start = whole[first].0;
        let run: Vec<Key> = whole[first..]
            .iter()
            .enumerate()
            .take_while(|(i, (t, _))| *t == start + *i as i64 && (*i as i64) < most)
            .map(|(_, (_, key))| *key)
            .collect();
        Some(Job {
            start,
            plan: segment(plan, setup.look, start, run.len() as i64),
            keys: run,
            clip: false,
        })
    }

    pub fn apply(&mut self, plan: &mut RenderPlan, setup: &Setup, exists: &dyn Fn(&Path) -> bool) {
        if setup.mode == RenderCache::Off || self.index.is_empty() {
            return;
        }
        let pieces = std::mem::take(&mut plan.video);
        let mut video = Vec::with_capacity(pieces.len());
        let mut seen: HashMap<Arc<Path>, bool> = HashMap::new();
        for piece in pieces {
            let (whole, clips) = self.levels(&piece, setup, plan);
            if whole.is_none() && clips.iter().all(Option::is_none) {
                video.push(piece);
                continue;
            }
            let index = &self.index;
            let mut found = |key: &Key| {
                let (f, i) = index.find(key)?;
                let there = *seen.entry(f.clone()).or_insert_with(|| exists(&f));
                there.then_some((f, i))
            };
            let n = piece.frames() as usize;
            let mut k = 0;
            while k < n {
                if let Some(keys) = &whole
                    && let Some((f, i)) = found(&keys[k])
                {
                    let m = index.run(&f, i, &keys[k..]);
                    video.push(cached(&piece.part(k as i64, m as i64), &f, i, m as i64));
                    k += m;
                    continue;
                }
                let at: Vec<Option<(Arc<Path>, u32)>> = clips
                    .iter()
                    .map(|keys| keys.as_ref().and_then(|keys| found(&keys[k])))
                    .collect();
                let mut m = 1;
                while k + m < n {
                    let j = k + m;
                    if whole.as_ref().is_some_and(|keys| found(&keys[j]).is_some()) {
                        break;
                    }
                    let goes_on = clips.iter().zip(&at).all(|(keys, a)| match (keys, a) {
                        (Some(keys), Some((f, i))) => {
                            index.run(f, *i + m as u32, &keys[j..=j]) == 1
                        }
                        (Some(keys), None) => found(&keys[j]).is_none(),
                        (None, _) => true,
                    });
                    if !goes_on {
                        break;
                    }
                    m += 1;
                }
                let mut part = piece.part(k as i64, m as i64);
                for (layer, a) in part.layers_mut().into_iter().zip(&at) {
                    if let Some((f, i)) = a {
                        from_file(layer, f, *i);
                    }
                }
                video.push(part);
                k += m;
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

type Levels = (Option<Arc<[Key]>>, Vec<Option<Arc<[Key]>>>);

pub struct Job {
    pub start: i64,
    pub keys: Vec<Key>,
    pub plan: RenderPlan,
    pub clip: bool,
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
        excluded: false,
    };
    match part {
        VideoPiece::Titled { titles, .. } => VideoPiece::Titled {
            under: Box::new(source),
            titles: titles.clone(),
        },
        _ => source,
    }
}

fn from_file(layer: &mut VideoPiece, file: &Path, at: u32) {
    crate::disk_cache::used(file);
    if let VideoPiece::Source {
        media,
        path,
        origin,
        source_frame,
        image,
        pace,
        color_space,
        super_scale,
        ..
    } = layer
    {
        media.clear();
        *path = file.to_path_buf();
        *origin = 0.0;
        *source_frame = i64::from(at);
        *image = None;
        *pace = Default::default();
        *color_space = Default::default();
        if super_scale.ai() {
            super_scale.upscaler = Upscaler::Classic;
        }
    }
}

fn clip_plan(plan: &RenderPlan, layer: VideoPiece, (width, height): (u32, u32)) -> RenderPlan {
    let frames = layer.frames();
    RenderPlan {
        fps: plan.fps,
        width,
        height,
        sample_rate: plan.sample_rate,
        channels: plan.channels,
        frames,
        video: vec![layer],
        audio: Vec::new(),
        ducking: Vec::new(),
        range: (0, frames),
        name: plan.name.clone(),
        colors: plan.colors,
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
        for (i, k) in keys.iter().enumerate().rev() {
            self.frames.insert(*k, (file.clone(), i as u32));
        }
        self.files.insert(file, keys.into());
    }

    pub fn has(&self, key: &Key) -> bool {
        self.frames.contains_key(key)
    }

    pub fn run(&self, file: &Path, at: u32, keys: &[Key]) -> usize {
        self.files.get(file).map_or(0, |held| {
            held.iter()
                .skip(at as usize)
                .zip(keys)
                .take_while(|(a, b)| a == b)
                .count()
        })
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
