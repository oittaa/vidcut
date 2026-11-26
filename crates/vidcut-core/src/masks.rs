use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::disk_cache::{self, Kind};
use crate::lru::ByBytes;

pub const SIDE: usize = 256;
pub const CELLS: usize = SIDE * SIDE;
pub const MAPS_BYTES: usize = 256 << 20;
pub const MOST_CLICKS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub joins: bool,
    pub clicked_stays: bool,
    pub clicks_drop_tracked: bool,
}

pub const RULES: Rules = Rules {
    joins: true,
    clicked_stays: true,
    clicks_drop_tracked: true,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub at: [f64; 2],
    pub negative: bool,
}

type Maps = ByBytes<String, Arc<[f32]>>;

static MAPS_KEPT: Mutex<Maps> = Mutex::new(ByBytes::new());

fn kept() -> MutexGuard<'static, Maps> {
    MAPS_KEPT.lock().unwrap_or_else(PoisonError::into_inner)
}

fn file(name: &str) -> PathBuf {
    Kind::Masks.dir().join(format!("{name}.map"))
}

fn held(name: &str, map: Arc<[f32]>) {
    let mut k = kept();
    k.insert(name.to_string(), map, CELLS * std::mem::size_of::<f32>());
    k.fit(MAPS_BYTES);
}

pub fn name(model: &str, picture: &str, points: &[Point]) -> String {
    let mut text = format!("{model}\n{picture}");
    for p in points {
        let [x, y] = p.at;
        text.push_str(&format!("\n{x:?} {y:?} {}", p.negative));
    }
    crate::fingerprint::short_digest(text.as_bytes())
}

pub fn put(name: &str, map: Arc<[f32]>) {
    if map.len() != CELLS {
        return;
    }
    let bytes: Vec<u8> = map.iter().flat_map(|v| v.to_le_bytes()).collect();
    let _ = disk_cache::write(&file(name), &bytes);
    held(name, map);
}

pub fn get(name: &str) -> Option<Arc<[f32]>> {
    if let Some(m) = kept().get(name) {
        return Some(m.clone());
    }
    let bytes = disk_cache::read(&file(name))?;
    if bytes.len() != CELLS * std::mem::size_of::<f32>() {
        return None;
    }
    let map: Arc<[f32]> = (bytes.as_chunks::<4>().0.iter())
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    held(name, map.clone());
    Some(map)
}

pub fn has(name: &str) -> bool {
    kept().contains(name) || file(name).is_file()
}

pub fn value(map: &[f32], [x, y]: [f64; 2]) -> f32 {
    let last = SIDE - 1;
    let cell = |v: f64| (v * SIDE as f64 - 0.5).max(0.0);
    let (u, v) = (cell(x), cell(y));
    let (x0, y0) = ((u as usize).min(last), (v as usize).min(last));
    let (x1, y1) = ((x0 + 1).min(last), (y0 + 1).min(last));
    let (tx, ty) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
    let at = |x: usize, y: usize| map[y * SIDE + x];
    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
    top + (bottom - top) * ty
}
