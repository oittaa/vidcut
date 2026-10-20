use std::sync::{Arc, Mutex};

use vidcut_core::model::{Cue, Place, SubtitleStyle};
use vidcut_core::subtitles::Shown as Words;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;

const KEPT: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleView {
    pub style: SubtitleStyle,
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, PartialEq)]
struct Key {
    style: SubtitleStyle,
    size: (u32, u32),
    cues: Vec<(Words, Place)>,
}

struct Entry {
    key: Key,
    pictures: Vec<Arc<TitlePicture>>,
    used: u64,
}

#[derive(Default)]
struct Kept {
    entries: Vec<Entry>,
    clock: u64,
    last: Option<Key>,
}

impl Kept {
    fn find(&mut self, key: &Key) -> Option<Vec<Arc<TitlePicture>>> {
        self.clock += 1;
        let now = self.clock;
        let e = self.entries.iter_mut().find(|e| e.key == *key)?;
        e.used = now;
        Some(e.pictures.clone())
    }
}

#[derive(Default)]
pub(crate) struct Subtitles {
    view: Mutex<Option<Arc<SubtitleView>>>,
    kept: Mutex<Kept>,
}

fn key_at(view: &SubtitleView, plan: &Plan, t: i64) -> Option<Key> {
    let fps = plan.fps().as_f64();
    let cues: Vec<(Words, Place)> = view
        .cues
        .iter()
        .filter(|c| c.start <= t && t < c.end())
        .map(|c| {
            let words =
                vidcut_core::subtitles::shown(&c.text, &c.words, &view.style, t - c.start, fps);
            (words, c.place)
        })
        .filter(|(w, _)| !w.pieces.is_empty())
        .collect();
    (!cues.is_empty()).then(|| Key {
        style: view.style.clone(),
        size: plan.size(),
        cues,
    })
}

fn over(picture: Arc<TitlePicture>) -> TitleOver {
    TitleOver {
        above: usize::MAX,
        picture,
        dx: 0,
        dy: 0,
        turn: [0.0, 0.0, 1.0, 0.0],
        alpha: 1.0,
    }
}

impl Subtitles {
    pub(crate) fn set(&self, view: Option<SubtitleView>) -> bool {
        let mut held = lock(&self.view);
        if held.as_deref() == view.as_ref() {
            return false;
        }
        *held = view.map(Arc::new);
        true
    }

    pub(crate) fn shown(&self, plan: &Plan, t: i64) -> (Vec<TitleOver>, bool) {
        let Some(view) = lock(&self.view).clone() else {
            return (Vec::new(), true);
        };
        let Some(key) = key_at(&view, plan, t) else {
            return (Vec::new(), true);
        };
        let mut kept = lock(&self.kept);
        if let Some(p) = kept.find(&key) {
            kept.last = Some(key);
            return (p.into_iter().map(over).collect(), true);
        }
        let stand_in = kept.last.clone().and_then(|k| kept.find(&k));
        (stand_in.into_iter().flatten().map(over).collect(), false)
    }

    pub(crate) fn draw(&self, plan: &Plan, t: i64) -> bool {
        let Some(view) = lock(&self.view).clone() else {
            return false;
        };
        let Some(key) = key_at(&view, plan, t) else {
            return false;
        };
        if lock(&self.kept).entries.iter().any(|e| e.key == key) {
            return false;
        }
        let (w, h) = key.size;
        let cues: Vec<(&Words, Place)> = key.cues.iter().map(|(s, p)| (s, *p)).collect();
        let pictures: Vec<Arc<TitlePicture>> = crate::burn::draw(&cues, w, h, &key.style)
            .into_iter()
            .map(|s| {
                Arc::new(TitlePicture {
                    left: s.x,
                    top: s.y,
                    width: s.width,
                    height: s.height,
                    frame: (w, h),
                    rgba: s.premultiplied(),
                })
            })
            .collect();
        let mut kept = lock(&self.kept);
        kept.clock += 1;
        let used = kept.clock;
        kept.entries.push(Entry {
            key,
            pictures,
            used,
        });
        while kept.entries.len() > KEPT {
            let oldest = (0..kept.entries.len())
                .min_by_key(|&i| kept.entries[i].used)
                .expect("more than kept");
            kept.entries.remove(oldest);
        }
        true
    }
}
