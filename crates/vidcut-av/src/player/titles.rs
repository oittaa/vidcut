use std::sync::{Arc, Mutex};

use vidcut_core::model::Title;
use vidcut_core::render::ShownTitle;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;

const KEPT: usize = 16;

#[derive(Debug, Clone, PartialEq)]
pub struct Nudge {
    pub title: Title,
    pub to: Title,
}

impl Nudge {
    pub fn moves_only(from: &Title, to: &Title) -> bool {
        steady(from) == steady(to)
    }
}

fn steady(t: &Title) -> Title {
    Title {
        position: None,
        opacity: None,
        ..t.clone()
    }
}

fn shown_as(nudge: Option<&Nudge>, s: &ShownTitle) -> (Title, f64) {
    match nudge.filter(|n| n.title == s.title) {
        Some(n) => (
            n.to.clone(),
            n.to.opaque() * s.fade.map_or(1.0, |r| r.level(0)),
        ),
        None => (s.title.clone(), s.alpha()),
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
            let (title, alpha) = shown_as(nudge.as_ref(), &shown);
            let key = steady(&title);
            match kept
                .entries
                .iter_mut()
                .find(|e| e.size == size && e.title == key)
            {
                Some(e) => {
                    e.used = now;
                    if let Some(p) = &e.picture {
                        let [x, y] = title.place();
                        let [x0, y0] = key.place();
                        let dx = ((x - x0) * w).round() as i32;
                        let dy = match shown.run {
                            Some(r) => r.top(0, h, f64::from(p.height)).round() as i32 - p.top,
                            None => ((y - y0) * h).round() as i32,
                        };
                        out.push(TitleOver {
                            above,
                            picture: p.clone(),
                            dx,
                            dy,
                            alpha: alpha as f32,
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
        let nudge = lock(&self.nudge).clone();
        let missing: Vec<Title> = {
            let kept = lock(&self.kept);
            let mut keys: Vec<Title> = Vec::new();
            for (_, shown) in plan.titles_at(t) {
                let key = steady(&shown_as(nudge.as_ref(), &shown).0);
                let have = kept
                    .entries
                    .iter()
                    .any(|e| e.size == size && e.title == key);
                if !have && !keys.contains(&key) {
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
