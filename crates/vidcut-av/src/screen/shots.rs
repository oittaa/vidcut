use vidcut_core::grade::Graded;
use vidcut_core::model::Grade;

use super::blur::{self, Spare, Texture};
use super::{Gpu, Target, Wide, begin, make_target};

const COLUMNS: u32 = 8;

pub const SMALLEST: u32 = 16;

pub const LARGEST: u32 = 512;

pub const SHOT_ASPECT: (u32, u32) = (91, 44);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShotsOf {
    Clip(String),
    Timeline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShotsWanted {
    pub of: ShotsOf,
    pub height: u32,
    pub refresh: bool,
}

#[derive(Clone)]
pub struct ShotsShown {
    pub generation: u64,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
    pub cells: Vec<(u32, [u32; 4])>,
}

pub(super) struct Source {
    pub(super) view: wgpu::TextureView,
    pub(super) size: (u32, u32),
    pub(super) turn: u16,
    pub(super) aspect: f64,
    pub(super) height: u32,
    pub(super) grade: Grade,
}

struct Room {
    size: (u32, u32),
    own: Texture,
    scratch: [Texture; 2],
    spare: Spare,
}

struct Atlas {
    size: (u32, u32),
    wide: Wide,
    shown: Target,
    generation: u64,
}

#[derive(Default)]
pub(super) struct Shots {
    pub(super) wanted: Option<ShotsWanted>,
    drawn: Option<ShotsWanted>,
    pub(super) timeline: Option<Grade>,
    drawn_timeline: Option<Grade>,
    room: Option<Room>,
    atlas: Option<Atlas>,
    cells: Vec<(u32, [u32; 4])>,
    made: u64,
}

pub(super) fn sized(height: u32) -> u32 {
    height.clamp(SMALLEST, LARGEST)
}

impl Shots {
    pub(super) fn stale(&self) -> bool {
        let timeline = self
            .wanted
            .as_ref()
            .is_some_and(|w| w.of == ShotsOf::Timeline);
        self.wanted != self.drawn || (timeline && self.timeline != self.drawn_timeline)
    }

    pub(super) fn takes(&self) -> Option<&ShotsWanted> {
        let w = self.wanted.as_ref()?;
        let same = |d: &ShotsWanted| d.of == w.of && d.height == w.height;
        let held = self.drawn.as_ref().is_some_and(same) && !self.cells.is_empty();
        (w.refresh || !held).then_some(w)
    }

    pub(super) fn draw(
        &mut self,
        gpu: &Gpu,
        source: Option<Source>,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        self.cells.clear();
        let Some(source) = source else {
            return;
        };
        let Some(graded) = Graded::tapped(&source.grade) else {
            return;
        };
        let h = source.height;
        let wide = f64::from(h) * f64::from(SHOT_ASPECT.0) / f64::from(SHOT_ASPECT.1);
        let w = (wide.round() as u32).max(1);
        let (bw, bh) = (f64::from(w), f64::from(h));
        let fit = if bw / bh > source.aspect {
            let wide = bh * source.aspect;
            [(bw - wide) / 2.0, 0.0, wide, bh]
        } else {
            let tall = bw / source.aspect;
            [0.0, (bh - tall) / 2.0, bw, tall]
        };
        if self.room.as_ref().is_none_or(|r| r.size != (w, h)) {
            self.room = Some(Room {
                size: (w, h),
                own: Texture::new(gpu, (w, h)),
                scratch: [Texture::new(gpu, (w, h)), Texture::new(gpu, (w, h))],
                spare: Spare::default(),
            });
        }
        let room = self.room.as_mut().expect("made above");
        let from = (&source.view, source.size, source.turn);
        blur::shot_source(gpu, from, fit, &room.own, encoder);
        let count = graded.taps.len().max(1) as u32;
        let size = (count.min(COLUMNS) * w, count.div_ceil(COLUMNS) * h);
        if self.atlas.as_ref().is_none_or(|a| a.size != size) {
            self.made += 1;
            self.atlas = Some(Atlas {
                size,
                wide: Wide::new(gpu, size, None),
                shown: make_target(&gpu.device, size),
                generation: self.made,
            });
        }
        let atlas = self.atlas.as_ref().expect("made above");
        let places: Vec<(u32, u32)> = (0..count)
            .map(|i| ((i % COLUMNS) * w, (i / COLUMNS) * h))
            .collect();
        let room_parts = (&room.scratch, &mut room.spare);
        let into = (&atlas.wide.texture, &places[..]);
        blur::tapped_into(gpu, &room.own, room_parts, &graded, into, encoder);
        {
            let mut pass = begin(encoder, &atlas.shown.view, Some(wgpu::Color::BLACK));
            pass.set_pipeline(&gpu.screen);
            pass.set_bind_group(0, &atlas.wide.bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.cells = (graded.taps.iter().zip(&places))
            .map(|(t, &(x, y))| (t.node, [x, y, w, h]))
            .collect();
    }

    pub(super) fn settle(&mut self) {
        self.drawn = self.wanted.clone();
        self.drawn_timeline = self.timeline.clone();
        if self.wanted.is_none() {
            self.room = None;
            self.atlas = None;
            self.cells.clear();
        }
    }

    pub(super) fn shown(&self) -> Option<ShotsShown> {
        let a = self.atlas.as_ref().filter(|_| self.wanted.is_some())?;
        Some(ShotsShown {
            generation: a.generation,
            view: a.shown.view.clone(),
            size: a.size,
            cells: self.cells.clone(),
        })
    }
}
