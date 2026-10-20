use std::sync::{Arc, Mutex};

use vidcut_core::model::Title;
use vidcut_core::render::ShownTitle;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;

const KEPT: usize = 16;

#[derive(Debug, Clone, PartialEq)]
pub struct Nudge {
    pub clip: String,
    pub title: Title,
    pub to: Title,
}

impl Nudge {
    pub fn fits(from: &Title, to: &Title) -> bool {
        shape(from) == shape(to)
    }
}

fn steady(t: &Title) -> Title {
    Title {
        position: None,
        rotation: None,
        opacity: None,
        ..t.clone()
    }
}

fn shape(t: &Title) -> Title {
    Title {
        zoom: None,
        rotation: None,
        ..steady(t)
    }
}

struct Chosen {
    from: Title,
    to: Title,
    alpha: f64,
}

fn chosen(nudge: Option<&Nudge>, s: &ShownTitle, drawn: impl Fn(&Title) -> bool) -> Chosen {
    match nudge {
        Some(n) if s.clip == n.clip && (s.title == n.title || s.title == n.to) => Chosen {
            from: if drawn(&steady(&n.to)) {
                n.to.clone()
            } else {
                n.title.clone()
            },
            to: n.to.clone(),
            alpha: n.to.opaque() * s.fade.map_or(1.0, |r| r.level(0)),
        },
        _ => Chosen {
            from: s.title.clone(),
            to: s.title.clone(),
            alpha: s.alpha(),
        },
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
    nudge: Mutex<Option<Nudge>>,
}

impl Titles {
    pub(crate) fn nudge(&self, n: Option<Nudge>) -> bool {
        let mut held = lock(&self.nudge);
        let changed = *held != n;
        *held = n;
        changed
    }

    pub(crate) fn shown(&self, plan: &Plan, t: i64) -> (Vec<TitleOver>, bool) {
        let wanted = plan.titles_at(t);
        if wanted.is_empty() {
            return (Vec::new(), true);
        }
        let nudge = lock(&self.nudge).clone();
        let size = plan.size();
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        let mut kept = lock(&self.kept);
        kept.clock += 1;
        let now = kept.clock;
        let mut out = Vec::with_capacity(wanted.len());
        let mut all = true;
        for (above, shown) in wanted {
            let c = chosen(nudge.as_ref(), &shown, |k| kept.has(k, size));
            let key = steady(&c.from);
            match kept
                .entries
                .iter_mut()
                .find(|e| e.size == size && e.title == key)
            {
                Some(e) => {
                    e.used = now;
                    if let Some(p) = &e.picture {
                        let [x0, y0] = key.pivot();
                        let [x, y] = c.to.pivot();
                        let dx = ((x - x0) * w).round() as i32;
                        let dy = match shown.run {
                            Some(r) => r.top(0, h, f64::from(p.height)).round() as i32 - p.top,
                            None => ((y - y0) * h).round() as i32,
                        };
                        let turn = [
                            (x * w) as f32,
                            (y * h) as f32,
                            (c.to.scale() / key.scale()) as f32,
                            (c.to.turn() - key.turn()).to_radians() as f32,
                        ];
                        out.push(TitleOver {
                            above,
                            picture: p.clone(),
                            dx,
                            dy,
                            turn,
                            alpha: c.alpha as f32,
                        });
                    }
                }
                None => all = false,
            }
        }
        (out, all)
    }

    pub(crate) fn draw(&self, plan: &Plan, t: i64) -> bool {
        let size = plan.size();
        let missing: Vec<Title> = {
            let kept = lock(&self.kept);
            let mut keys: Vec<Title> = Vec::new();
            for (_, shown) in plan.titles_at(t) {
                let key = steady(&shown.title);
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
