use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use vidcut_core::lru::Lru;
use vidcut_core::model::Title;
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
        ..t.clone()
    }
}

fn wanted<'a>(preview: &'a Preview, s: &'a ShownTitle) -> &'a Title {
    match preview {
        Some((clip, t)) if *clip == s.clip => t,
        _ => &s.title,
    }
}

#[derive(Default)]
struct Kept {
    pictures: Lru<Key, Option<Arc<TitlePicture>>, KEPT>,
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

    pub(crate) fn shown(&self, plan: &Plan, t: i64) -> (Vec<TitleOver>, bool) {
        let preview = lock(&self.preview).clone();
        let size = plan.size();
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        let mut kept = lock(&self.kept);
        let mut out = Vec::new();
        let mut all = true;
        for (above, shown) in plan.titles_at(t) {
            let to = wanted(&preview, &shown);
            let exact = (steady(to), size);
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
            let picture = kept.pictures.get(&key).cloned().flatten();
            let (key, _) = key;
            if let Some(p) = picture {
                let [x0, y0] = key.pivot();
                let [x, y] = to.pivot();
                let dy = if to.moves() {
                    shown.run.top(0, h, f64::from(p.height)).round() as i32 - p.top
                } else {
                    ((y - y0) * h).round() as i32
                };
                out.push(TitleOver {
                    above,
                    picture: p,
                    dx: ((x - x0) * w).round() as i32,
                    dy,
                    turn: [
                        (x * w) as f32,
                        (y * h) as f32,
                        (to.scale() / key.scale()) as f32,
                        (to.turn() - key.turn()).to_radians() as f32,
                    ],
                    alpha: (to.opaque() * shown.fade.map_or(1.0, |r| r.level(0))) as f32,
                });
            }
            kept.last.insert(shown.clip.clone(), key);
        }
        (out, all)
    }

    pub(crate) fn draw(&self, plan: &Plan, t: i64) -> bool {
        let size = plan.size();
        let preview = lock(&self.preview).clone();
        let missing: Vec<(String, Key)> = {
            let kept = lock(&self.kept);
            let mut keys: Vec<(String, Key)> = Vec::new();
            for (_, shown) in plan.titles_at(t) {
                let key = (steady(wanted(&preview, &shown)), size);
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
            let picture = drawn(&key.0, size);
            let mut kept = lock(&self.kept);
            kept.last.insert(clip, key.0.clone());
            kept.pictures.insert(key, picture);
        }
        true
    }
}

fn drawn(t: &Title, (width, height): (u32, u32)) -> Option<Arc<TitlePicture>> {
    let sprite = crate::burn::title(t, width, height).into_iter().next()?;
    Some(Arc::new(TitlePicture {
        left: sprite.x,
        top: sprite.y,
        width: sprite.width,
        height: sprite.height,
        frame: (width, height),
        rgba: sprite.premultiplied(),
    }))
}
