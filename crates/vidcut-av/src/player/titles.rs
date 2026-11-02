use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use vidcut_core::lru::Lru;
use vidcut_core::model::{Picture, Title, TitleLine};
use vidcut_core::render::ShownTitle;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;

const KEPT: usize = 16;

type Preview = Option<(String, Title)>;

type Key = (Title, (u32, u32));

fn steady(t: &Title) -> Title {
    Title {
        position: None,
        rotation: None,
        opacity: None,
        upper: t.upper.clone().map(|u| TitleLine {
            position: None,
            rotation: None,
            ..u
        }),
        ..t.clone()
    }
}

fn wanted(preview: &Preview, s: &ShownTitle) -> Title {
    match preview {
        Some((clip, t)) if *clip == s.clip => t.sampled(s.frame()),
        _ => s.title.clone(),
    }
}

#[derive(Default)]
struct Kept {
    pictures: Lru<Key, Vec<Option<Arc<TitlePicture>>>, KEPT>,
    last: HashMap<String, Title>,
}

#[derive(Default)]
pub(crate) struct Titles {
    kept: Mutex<Kept>,
    preview: Mutex<Preview>,
}

impl Titles {
    pub(crate) fn preview(&self, p: Preview) -> bool {
        let mut held = lock(&self.preview);
        let changed = *held != p;
        *held = p;
        changed
    }

    pub(crate) fn shown(
        &self,
        plan: &Plan,
        t: i64,
        placing: &[(String, Picture)],
    ) -> (Vec<TitleOver>, bool) {
        self.shown_at(&plan.titles_at(t), plan.size(), placing)
    }

    pub(crate) fn shown_at(
        &self,
        at: &[(usize, ShownTitle)],
        size: (u32, u32),
        placing: &[(String, Picture)],
    ) -> (Vec<TitleOver>, bool) {
        let preview = lock(&self.preview).clone();
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        let mut kept = lock(&self.kept);
        let mut out = Vec::new();
        let mut all = true;
        for (above, shown) in at {
            let to = wanted(&preview, shown);
            let exact = (steady(&to), size);
            let key = if kept.pictures.contains(&exact) {
                exact
            } else {
                all = false;
                let last = kept.last.get(&shown.clip).map(|k| (k.clone(), size));
                match last.filter(|k| kept.pictures.contains(k)) {
                    Some(k) => k,
                    None => continue,
                }
            };
            let pictures = kept.pictures.get(&key).cloned().unwrap_or_default();
            let (key, _) = key;
            for ((picture, from), now) in pictures.iter().zip(key.blocks()).zip(to.blocks()) {
                let Some(p) = picture.clone() else {
                    continue;
                };
                let [x0, y0] = from.pivot;
                let [x, y] = now.pivot;
                let dy = if to.moves() {
                    shown.run.top(0, h, f64::from(p.height)).round() as i32 - p.top
                } else {
                    ((y - y0) * h).round() as i32
                };
                out.push(TitleOver {
                    above: *above,
                    picture: p,
                    dx: ((x - x0) * w).round() as i32,
                    dy,
                    turn: [
                        (x * w) as f32,
                        (y * h) as f32,
                        (now.zoom / from.zoom) as f32,
                        (now.turn - from.turn).to_radians() as f32,
                    ],
                    alpha: (to.opaque() * shown.fade.map_or(1.0, |r| r.level(0))) as f32,
                    placed: super::plan::previewed(&shown.placed, placing),
                });
            }
            kept.last.insert(shown.clip.clone(), key);
        }
        (out, all)
    }

    pub(crate) fn draw(&self, plan: &Plan, t: i64) -> bool {
        let size = plan.size();
        self.draw_at(&plan.titles_at(t), size, size.1)
    }

    pub(crate) fn draw_at(
        &self,
        at: &[(usize, ShownTitle)],
        size: (u32, u32),
        timeline: u32,
    ) -> bool {
        let preview = lock(&self.preview).clone();
        let missing: Vec<(String, Key)> = {
            let kept = lock(&self.kept);
            let mut keys: Vec<(String, Key)> = Vec::new();
            for (_, shown) in at {
                let key = (steady(&wanted(&preview, shown)), size);
                if !kept.pictures.contains(&key) && !keys.iter().any(|(_, k)| *k == key) {
                    keys.push((shown.clip.clone(), key));
                }
            }
            keys
        };
        if missing.is_empty() {
            return false;
        }
        for (clip, key) in missing {
            let picture = drawn(&key.0, size, timeline);
            let mut kept = lock(&self.kept);
            kept.last.insert(clip, key.0.clone());
            kept.pictures.insert(key, picture);
        }
        true
    }
}

fn drawn(t: &Title, (width, height): (u32, u32), timeline: u32) -> Vec<Option<Arc<TitlePicture>>> {
    crate::burn::title_blocks(t, width, height, timeline)
        .into_iter()
        .map(|sprite| {
            sprite.map(|sprite| {
                Arc::new(TitlePicture {
                    left: sprite.x,
                    top: sprite.y,
                    width: sprite.width,
                    height: sprite.height,
                    frame: (width, height),
                    rgba: sprite.premultiplied(),
                })
            })
        })
        .collect()
}
