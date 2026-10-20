use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use vidcut_core::model::Title;
use vidcut_core::render::ShownTitle;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;

const KEPT: usize = 16;

type Preview = Option<(String, Title)>;

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

struct Entry {
    title: Title,
    size: (u32, u32),
    picture: Option<Arc<TitlePicture>>,
    used: u64,
}

#[derive(Default)]
struct Kept {
    entries: Vec<Entry>,
    clock: u64,
    last: HashMap<String, Title>,
}

impl Kept {
    fn has(&self, key: &Title, size: (u32, u32)) -> bool {
        self.entries
            .iter()
            .any(|e| e.size == size && e.title == *key)
    }
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
        kept.clock += 1;
        let now = kept.clock;
        let mut out = Vec::new();
        let mut all = true;
        for (above, shown) in plan.titles_at(t) {
            let to = wanted(&preview, &shown);
            let exact = steady(to);
            let key = if kept.has(&exact, size) {
                exact
            } else {
                all = false;
                match kept.last.get(&shown.clip).filter(|k| kept.has(k, size)) {
                    Some(k) => k.clone(),
                    None => continue,
                }
            };
            let picture = kept
                .entries
                .iter_mut()
                .find(|e| e.size == size && e.title == key)
                .and_then(|e| {
                    e.used = now;
                    e.picture.clone()
                });
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
        let missing: Vec<Title> = {
            let kept = lock(&self.kept);
            let mut keys: Vec<Title> = Vec::new();
            for (_, shown) in plan.titles_at(t) {
                let key = steady(wanted(&preview, &shown));
                if !kept.has(&key, size) && !keys.contains(&key) {
                    keys.push(key);
                }
            }
            keys
        };
        if missing.is_empty() {
            return false;
        }
        for title in missing {
            let picture = drawn(&title, size);
            let mut kept = lock(&self.kept);
            kept.clock += 1;
            let used = kept.clock;
            kept.entries.push(Entry {
                title,
                size,
                picture,
                used,
            });
            while kept.entries.len() > KEPT {
                let oldest = (0..kept.entries.len())
                    .min_by_key(|&i| kept.entries[i].used)
                    .expect("more than kept");
                kept.entries.remove(oldest);
            }
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
