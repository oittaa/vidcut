use std::sync::{Arc, Mutex};

use vidcut_core::lru::Lru;
use vidcut_core::model::{Cue, Picture, Place, SubtitleStyle};
use vidcut_core::render::Placed;
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

type Drawn = Vec<(Key, Vec<Placed>)>;

#[derive(Default)]
struct Kept {
    pictures: Lru<Key, Vec<Arc<TitlePicture>>, KEPT>,
    last: Option<Drawn>,
}

impl Kept {
    fn overs(&mut self, drawn: &Drawn) -> Option<Vec<TitleOver>> {
        let mut out = Vec::new();
        for (key, placed) in drawn {
            let pictures = self.pictures.get(key)?.clone();
            out.extend(pictures.into_iter().map(|p| over(p, placed.clone())));
        }
        Some(out)
    }
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
    picture: Option<Picture>,
}

#[derive(Default)]
pub(crate) struct Subtitles {
    view: Mutex<Option<Arc<SubtitleView>>>,
    timed: Mutex<Arc<Vec<Timed>>>,
    kept: Mutex<Kept>,
}

fn drawn_at(timed: &[Timed], fps: f64, size: (u32, u32), t: i64) -> Drawn {
    let mut plain = Vec::new();
    let mut alone = Vec::new();
    for c in timed.iter().filter(|c| c.start <= t && t < c.end) {
        let words = vidcut_core::subtitles::shown(
            &c.text,
            &c.times,
            &c.styles,
            &c.style,
            t - c.start,
            fps,
            c.color.as_deref(),
        );
        if words.pieces.is_empty() {
            continue;
        }
        let cue = (words, c.place, c.style.clone());
        match &c.picture {
            None => plain.push(cue),
            Some(picture) => alone.push((
                Key {
                    size,
                    cues: vec![cue],
                },
                vec![Placed {
                    clip: String::new(),
                    picture: picture.clone(),
                    run: None,
                    source: t - c.start,
                    frame: t - c.start,
                    start: c.start,
                    ready: true,
                }],
            )),
        }
    }
    if !plain.is_empty() {
        alone.insert(0, (Key { size, cues: plain }, Vec::new()));
    }
    alone
}

fn over(picture: Arc<TitlePicture>, placed: Vec<Placed>) -> TitleOver {
    TitleOver {
        above: usize::MAX,
        picture,
        dx: 0,
        dy: 0,
        turn: [0.0, 0.0, 1.0, 0.0],
        alpha: 1.0,
        placed,
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
                    picture: c.picture.clone(),
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
                picture: c.picture.clone(),
            })
            .collect();
        *lock(&self.timed) = Arc::new(timed);
    }

    pub(crate) fn shown(&self, plan: &Plan, t: i64) -> (Vec<TitleOver>, bool) {
        self.shown_at(plan.fps().as_f64(), plan.size(), t)
    }

    pub(crate) fn shown_at(&self, fps: f64, size: (u32, u32), t: i64) -> (Vec<TitleOver>, bool) {
        let timed = lock(&self.timed).clone();
        let drawn = drawn_at(&timed, fps, size, t);
        if drawn.is_empty() {
            return (Vec::new(), true);
        }
        let mut kept = lock(&self.kept);
        if let Some(out) = kept.overs(&drawn) {
            kept.last = Some(drawn);
            return (out, true);
        }
        let last = kept.last.clone();
        let stand_in = last.and_then(|l| kept.overs(&l));
        (stand_in.unwrap_or_default(), false)
    }

    pub(crate) fn draw(&self, plan: &Plan, t: i64) -> bool {
        let size = plan.size();
        self.draw_at(plan.fps().as_f64(), size, size.1, t)
    }

    pub(crate) fn draw_at(&self, fps: f64, size: (u32, u32), timeline: u32, t: i64) -> bool {
        let timed = lock(&self.timed).clone();
        let mut painted = false;
        for (key, _) in drawn_at(&timed, fps, size, t) {
            if lock(&self.kept).pictures.contains(&key) {
                continue;
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
            let pictures: Vec<Arc<TitlePicture>> = crate::burn::paint(&cues, w, h, timeline)
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
            painted = true;
        }
        painted
    }
}
