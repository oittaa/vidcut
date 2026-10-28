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

struct Timed {
    start: i64,
    end: i64,
    text: String,
    times: Vec<i64>,
    styles: Vec<Option<SubtitleStyle>>,
    color: Option<String>,
    style: SubtitleStyle,
    place: Place,
}

#[derive(Default)]
pub(crate) struct Subtitles {
    view: Mutex<Option<Arc<SubtitleView>>>,
    timed: Mutex<Arc<Vec<Timed>>>,
    kept: Mutex<Kept>,
}

fn key_at(timed: &[Timed], fps: f64, size: (u32, u32), t: i64) -> Option<Key> {
    let cues: Vec<(Words, Place, SubtitleStyle)> = timed
        .iter()
        .filter(|c| c.start <= t && t < c.end)
        .map(|c| {
            let words = vidcut_core::subtitles::shown(
                &c.text,
                &c.times,
                &c.styles,
                &c.style,
                t - c.start,
                fps,
                c.color.as_deref(),
            );
            (words, c.place, c.style.clone())
        })
        .filter(|(w, _, _)| !w.pieces.is_empty())
        .collect();
    (!cues.is_empty()).then_some(Key { size, cues })
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
        let timed = view.as_ref().map_or_else(Vec::new, |v| {
            v.cues
                .iter()
                .map(|c| Timed {
                    start: c.start,
                    end: c.end(),
                    text: c.text.clone(),
                    times: c.times(),
                    styles: c.words.iter().map(|w| w.style.clone()).collect(),
                    color: c.color.clone(),
                    style: v.style.for_cue(c.style.as_ref()),
                    place: c.place,
                })
                .collect()
        });
        *lock(&self.timed) = Arc::new(timed);
        *held = view.map(Arc::new);
        true
    }

    pub(crate) fn burned(&self, cues: &[crate::export::subtitles::Cue], style: &SubtitleStyle) {
        let timed = cues
            .iter()
            .map(|c| Timed {
                start: c.start,
                end: c.end,
                text: c.text.clone(),
                times: c.words.clone(),
                styles: c.word_styles.clone(),
                color: c.color.clone(),
                style: style.for_cue(c.style.as_ref()),
                place: c.place,
            })
            .collect();
        *lock(&self.timed) = Arc::new(timed);
    }

    pub(crate) fn shown(&self, plan: &Plan, t: i64) -> (Vec<TitleOver>, bool) {
        self.shown_at(plan.fps().as_f64(), plan.size(), t)
    }

    pub(crate) fn shown_at(&self, fps: f64, size: (u32, u32), t: i64) -> (Vec<TitleOver>, bool) {
        let timed = lock(&self.timed).clone();
        let Some(key) = key_at(&timed, fps, size, t) else {
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
        self.draw_at(plan.fps().as_f64(), plan.size(), t)
    }

    pub(crate) fn draw_at(&self, fps: f64, size: (u32, u32), t: i64) -> bool {
        let timed = lock(&self.timed).clone();
        let Some(key) = key_at(&timed, fps, size, t) else {
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
