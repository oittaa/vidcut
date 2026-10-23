use std::sync::{Arc, Mutex};

use vidcut_core::lru::Lru;
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    size: (u32, u32),
    cues: Vec<(Words, Place, SubtitleStyle)>,
}

#[derive(Default)]
struct Kept {
    pictures: Lru<Key, Vec<Arc<TitlePicture>>, KEPT>,
    last: Option<Key>,
}

#[derive(Default)]
pub(crate) struct Subtitles {
    view: Mutex<Option<Arc<SubtitleView>>>,
    kept: Mutex<Kept>,
}

fn key_at(view: &SubtitleView, plan: &Plan, t: i64) -> Option<Key> {
    let fps = plan.fps().as_f64();
    let cues: Vec<(Words, Place, SubtitleStyle)> = view
        .cues
        .iter()
        .filter(|c| c.start <= t && t < c.end())
        .map(|c| {
            let style = view.style.for_cue(c.style.as_ref());
            let words = vidcut_core::subtitles::shown(
                &c.text,
                &c.words,
                &style,
                t - c.start,
                fps,
                c.color.as_deref(),
            );
            (words, c.place, style)
        })
        .filter(|(w, _, _)| !w.pieces.is_empty())
        .collect();
    (!cues.is_empty()).then(|| Key {
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
        placed: Vec::new(),
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
        if let Some(p) = kept.pictures.get(&key).cloned() {
            kept.last = Some(key);
            return (p.into_iter().map(over).collect(), true);
        }
        let last = kept.last.clone();
        let stand_in = last.and_then(|k| kept.pictures.get(&k).cloned());
        (stand_in.into_iter().flatten().map(over).collect(), false)
    }

    pub(crate) fn draw(&self, plan: &Plan, t: i64) -> bool {
        let Some(view) = lock(&self.view).clone() else {
            return false;
        };
        let Some(key) = key_at(&view, plan, t) else {
            return false;
        };
        if lock(&self.kept).pictures.contains(&key) {
            return false;
        }
        let (w, h) = key.size;
        let cues: Vec<crate::burn::Painted<'_>> = key
            .cues
            .iter()
            .map(|(shown, place, style)| crate::burn::Painted {
                shown,
                place: *place,
                style,
            })
            .collect();
        let pictures: Vec<Arc<TitlePicture>> = crate::burn::paint(&cues, w, h)
            .all()
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
        lock(&self.kept).pictures.insert(key, pictures);
        true
    }
}
