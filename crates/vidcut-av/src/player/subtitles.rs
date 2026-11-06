use std::sync::{Arc, Mutex};

use vidcut_core::model::{Cue, Picture, Place, SubtitleStyle};
use vidcut_core::render::Placed;
use vidcut_core::subtitles::Shown as Words;

use super::frame::{TitleOver, TitlePicture};
use super::lock;
use super::plan::Plan;
use super::store::{self, HeldTitles, TitleKey, TitlePictures};

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleView {
    pub style: SubtitleStyle,
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    size: (u32, u32),
    timeline: u32,
    cues: Vec<(Words, Place, SubtitleStyle)>,
}

type Drawn = Vec<(Key, Vec<Placed>)>;

#[derive(Default)]
struct Kept {
    held: HeldTitles,
    last: Option<Drawn>,
}

impl Kept {
    fn overs(&self, drawn: &Drawn) -> Option<Vec<TitleOver>> {
        let mut out = Vec::new();
        for (key, placed) in drawn {
            let pictures = self.held.get(&TitleKey::Cues(key.clone()))?;
            let overs = pictures.iter().flatten();
            out.extend(overs.map(|p| over(p.clone(), placed.clone())));
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

fn drawn_at(timed: &[Timed], fps: f64, size: (u32, u32), timeline: u32, t: i64) -> Drawn {
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
                    timeline,
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
        let key = Key {
            size,
            timeline,
            cues: plain,
        };
        alone.insert(0, (key, Vec::new()));
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
        let size = plan.size();
        self.shown_at(plan.fps().as_f64(), size, size.1, t)
    }

    pub(crate) fn shown_at(
        &self,
        fps: f64,
        size: (u32, u32),
        timeline: u32,
        t: i64,
    ) -> (Vec<TitleOver>, bool) {
        let timed = lock(&self.timed).clone();
        let drawn = drawn_at(&timed, fps, size, timeline, t);
        if drawn.is_empty() {
            return (Vec::new(), true);
        }
        let mut kept = lock(&self.kept);
        if let Some(out) = kept.overs(&drawn) {
            kept.last = Some(drawn);
            return (out, true);
        }
        let stand_in = kept.last.as_ref().and_then(|l| kept.overs(l));
        (stand_in.unwrap_or_default(), false)
    }

    pub(crate) fn draw(&self, plan: &Plan, t: i64) -> bool {
        let size = plan.size();
        self.pictures_at(plan.fps().as_f64(), size, size.1, t).0
    }

    pub(crate) fn hold(&self, plan: &Plan, t: i64) -> bool {
        let size = plan.size();
        self.hold_at(plan.fps().as_f64(), size, size.1, t)
    }

    pub(crate) fn hold_at(&self, fps: f64, size: (u32, u32), timeline: u32, t: i64) -> bool {
        let (painted, held) = self.pictures_at(fps, size, timeline, t);
        lock(&self.kept).held = HeldTitles::new(held);
        painted
    }

    fn pictures_at(
        &self,
        fps: f64,
        size: (u32, u32),
        timeline: u32,
        t: i64,
    ) -> (bool, Vec<(TitleKey, TitlePictures)>) {
        let timed = lock(&self.timed).clone();
        let mut painted = false;
        let mut out = Vec::new();
        for (key, _) in drawn_at(&timed, fps, size, timeline, t) {
            let whole = TitleKey::Cues(key.clone());
            let found = lock(&self.kept).held.get(&whole);
            let pictures = found.unwrap_or_else(|| {
                let pictures = painted_of(&key);
                store::global().keep_titles(whole.clone(), pictures.clone());
                painted = true;
                pictures
            });
            out.push((whole, pictures));
        }
        (painted, out)
    }
}

fn painted_of(key: &Key) -> TitlePictures {
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
    crate::burn::paint(&cues, w, h, key.timeline)
        .all()
        .into_iter()
        .map(|s| {
            Some(Arc::new(TitlePicture {
                left: s.x,
                top: s.y,
                width: s.width,
                height: s.height,
                frame: (w, h),
                rgba: s.premultiplied(),
            }))
        })
        .collect()
}
