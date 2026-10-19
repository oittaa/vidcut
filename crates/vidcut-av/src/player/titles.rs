use std::sync::{Arc, Mutex};

use vidcut_core::model::Title;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;

const KEPT: usize = 16;

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
}

impl Titles {
    pub(crate) fn shown(&self, plan: &Plan, t: i64) -> (Vec<TitleOver>, bool) {
        let wanted = plan.titles_at(t);
        if wanted.is_empty() {
            return (Vec::new(), true);
        }
        let size = plan.size();
        let mut kept = lock(&self.kept);
        kept.clock += 1;
        let now = kept.clock;
        let mut out = Vec::with_capacity(wanted.len());
        let mut all = true;
        for (above, title) in wanted {
            match kept
                .entries
                .iter_mut()
                .find(|e| e.size == size && e.title == *title)
            {
                Some(e) => {
                    e.used = now;
                    if let Some(p) = &e.picture {
                        out.push(TitleOver {
                            above,
                            picture: p.clone(),
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
            plan.titles_at(t)
                .into_iter()
                .filter(|(_, title)| {
                    !kept
                        .entries
                        .iter()
                        .any(|e| e.size == size && e.title == **title)
                })
                .map(|(_, title)| title.clone())
                .collect()
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
