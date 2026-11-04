use std::sync::{Arc, Mutex};

use vidcut_core::lru::Lru;
use vidcut_core::model::{Picture, Title, TitleKind, TitleLine, TitlePart};
use vidcut_core::render::ShownTitle;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;

const KEPT: usize = 16;

type Preview = Option<(String, Title)>;

type Key = (Title, (u32, u32), bool);

fn steady(t: &Title, size: (u32, u32)) -> Title {
    let bare = Title {
        position: None,
        rotation: None,
        opacity: None,
        upper: t.upper.clone().map(|u| TitleLine {
            position: None,
            rotation: None,
            ..u
        }),
        ..t.clone()
    };
    let cut = t.shape_past_edge(size) || bare.shape_past_edge(size);
    Title {
        position: t.position.filter(|_| cut),
        ..bare
    }
}

fn level(t: &Title, s: &ShownTitle) -> f64 {
    t.opaque() * s.fade.map_or(1.0, |r| r.level(0))
}

fn key_of(t: &Title, s: &ShownTitle, size: (u32, u32)) -> Key {
    if t.kind == TitleKind::LowerThird && level(t, s) < 1.0 {
        let whole = Title {
            opacity: None,
            ..t.clone()
        };
        return (whole, size, true);
    }
    (steady(t, size), size, false)
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
    last: Lru<String, Key, KEPT>,
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
            let exact = key_of(&to, shown, size);
            let key = if kept.pictures.contains(&exact) {
                exact
            } else {
                all = false;
                let last = kept.last.get(&shown.clip).cloned();
                match last.filter(|k| k.1 == size && kept.pictures.contains(k)) {
                    Some(k) => k,
                    None => continue,
                }
            };
            let pictures = kept.pictures.get(&key).cloned().unwrap_or_default();
            let alpha = level(&to, shown) as f32;
            if key.2 {
                out.extend(pictures.iter().flatten().map(|p| TitleOver {
                    above: *above,
                    picture: p.clone(),
                    dx: 0,
                    dy: 0,
                    turn: [0.0, 0.0, 1.0, 0.0],
                    alpha,
                    placed: super::plan::previewed(&shown.placed, placing),
                }));
                kept.last.insert(shown.clip.clone(), key);
                continue;
            }
            for ((picture, from), now) in pictures.iter().zip(key.0.blocks()).zip(to.blocks()) {
                let Some(p) = picture.clone() else {
                    continue;
                };
                let [x0, y0] = from.pivot;
                let [x, y] = now.pivot;
                let boxes = (now.part == TitlePart::Shape)
                    .then(|| key.0.shape_pixels(from.pivot, (w, h)))
                    .flatten()
                    .zip(to.shape_pixels(now.pivot, (w, h)));
                let (dx, dy) = match boxes {
                    Some((a, b)) => ((b[0] - a[0]) as i32, (b[1] - a[1]) as i32),
                    None if to.moves() => {
                        let top = shown.run.top(0, h, f64::from(p.height)) + f64::from(p.lift);
                        (((x - x0) * w).round() as i32, top.round() as i32 - p.top)
                    }
                    None => (
                        ((x * w).round() - (x0 * w).round()) as i32,
                        ((y * h).round() - (y0 * h).round()) as i32,
                    ),
                };
                out.push(TitleOver {
                    above: *above,
                    picture: p,
                    dx,
                    dy,
                    turn: [
                        (x * w) as f32,
                        (y * h) as f32,
                        (now.zoom / from.zoom) as f32,
                        (now.turn - from.turn).to_radians() as f32,
                    ],
                    alpha,
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
                let key = key_of(&wanted(&preview, shown), shown, size);
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
            let picture = drawn(&key, timeline);
            let mut kept = lock(&self.kept);
            kept.last.insert(clip, key.clone());
            kept.pictures.insert(key, picture);
        }
        true
    }
}

fn drawn((t, (width, height), whole): &Key, timeline: u32) -> Vec<Option<Arc<TitlePicture>>> {
    let (width, height) = (*width, *height);
    let sprites = if *whole {
        crate::burn::title(t, width, height, timeline)
            .into_iter()
            .map(Some)
            .collect()
    } else {
        crate::burn::title_blocks(t, width, height, timeline)
    };
    sprites
        .into_iter()
        .map(|sprite| {
            sprite.map(|sprite| {
                Arc::new(TitlePicture {
                    left: sprite.x,
                    top: sprite.y,
                    width: sprite.width,
                    height: sprite.height,
                    lift: sprite.lift,
                    frame: (width, height),
                    rgba: sprite.premultiplied(),
                })
            })
        })
        .collect()
}
