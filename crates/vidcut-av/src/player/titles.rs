use std::sync::{Arc, Mutex};

use vidcut_core::lru::Lru;
use vidcut_core::model::{Picture, Title, TitleKind, TitleLine, TitlePart};
use vidcut_core::render::ShownTitle;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;
use super::store::{self, HeldTitles, TitleKey, TitlePictures};

const LAST_KEPT: usize = 16;

type Preview = Option<(String, Title)>;

pub(crate) type Key = (Title, (u32, u32), u32, bool);

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

fn key_of(t: &Title, s: &ShownTitle, size: (u32, u32), timeline: u32) -> Key {
    if t.kind == TitleKind::LowerThird && level(t, s) < 1.0 {
        let whole = Title {
            opacity: None,
            ..t.clone()
        };
        return (whole, size, timeline, true);
    }
    (steady(t, size), size, timeline, false)
}

fn wanted(preview: &Preview, s: &ShownTitle) -> Title {
    match preview {
        Some((clip, t)) if *clip == s.clip => t.sampled(s.frame()),
        _ => s.title.clone(),
    }
}

#[derive(Default)]
struct Kept {
    held: HeldTitles,
    last: Lru<String, Key, LAST_KEPT>,
}

impl Kept {
    fn pictures(&self, key: &Key) -> Option<TitlePictures> {
        self.held.get(&TitleKey::Title(Box::new(key.clone())))
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

    pub(crate) fn shown(
        &self,
        plan: &Plan,
        t: i64,
        placing: &[(String, Picture)],
    ) -> (Vec<TitleOver>, bool) {
        let size = plan.size();
        self.shown_at(&plan.titles_at(t), size, size.1, placing)
    }

    pub(crate) fn shown_at(
        &self,
        at: &[(usize, ShownTitle)],
        size: (u32, u32),
        timeline: u32,
        placing: &[(String, Picture)],
    ) -> (Vec<TitleOver>, bool) {
        let preview = lock(&self.preview).clone();
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        let mut kept = lock(&self.kept);
        let mut out = Vec::new();
        let mut all = true;
        for (above, shown) in at {
            let to = wanted(&preview, shown);
            let exact = key_of(&to, shown, size, timeline);
            let (key, pictures) = match kept.pictures(&exact) {
                Some(pictures) => (exact, pictures),
                None => {
                    all = false;
                    let last = kept.last.get(&shown.clip).cloned();
                    let found = last
                        .filter(|k| k.1 == size)
                        .and_then(|k| kept.pictures(&k).map(|p| (k, p)));
                    match found {
                        Some(found) => found,
                        None => continue,
                    }
                }
            };
            let alpha = level(&to, shown) as f32;
            if key.3 {
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
        self.pictures_at(&plan.titles_at(t), size, size.1).0
    }

    pub(crate) fn hold(&self, plan: &Plan, t: i64) -> bool {
        let size = plan.size();
        self.hold_at(&plan.titles_at(t), size, size.1)
    }

    pub(crate) fn hold_at(
        &self,
        at: &[(usize, ShownTitle)],
        size: (u32, u32),
        timeline: u32,
    ) -> bool {
        let (drew, held) = self.pictures_at(at, size, timeline);
        lock(&self.kept).held = HeldTitles::new(held);
        drew
    }

    fn pictures_at(
        &self,
        at: &[(usize, ShownTitle)],
        size: (u32, u32),
        timeline: u32,
    ) -> (bool, Vec<(TitleKey, TitlePictures)>) {
        let preview = lock(&self.preview).clone();
        let mut drew = false;
        let mut out: Vec<(TitleKey, TitlePictures)> = Vec::new();
        for (_, shown) in at {
            let key = key_of(&wanted(&preview, shown), shown, size, timeline);
            let whole = TitleKey::Title(Box::new(key.clone()));
            if out.iter().any(|(k, _)| *k == whole) {
                continue;
            }
            let found = lock(&self.kept).pictures(&key);
            let pictures = found.unwrap_or_else(|| {
                let pictures = drawn(&key);
                lock(&self.kept).last.insert(shown.clip.clone(), key);
                store::global().keep_titles(whole.clone(), pictures.clone());
                drew = true;
                pictures
            });
            out.push((whole, pictures));
        }
        (drew, out)
    }
}

fn drawn((t, (width, height), timeline, whole): &Key) -> TitlePictures {
    let (width, height, timeline) = (*width, *height, *timeline);
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
