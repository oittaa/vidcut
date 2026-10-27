use std::path::Path;

use super::normalize::{Relayout, down};
use crate::decode::Reading;
use crate::error::Result;
use crate::ffmpeg::{format, frame};
use crate::route::{Facts, Route, Went, plan};

pub(crate) struct Fitting {
    pub(crate) route: Option<Route>,
    pub(crate) went: Option<Went>,
}

impl Fitting {
    pub(crate) fn none() -> Self {
        Self {
            route: None,
            went: None,
        }
    }

    pub(crate) fn took(&mut self, route: Route, path: &Path) {
        if self.route != Some(route) {
            crate::debug!("decoding", "{}: {route:?}", path.display());
        }
        self.route = Some(route);
    }
}

pub(crate) struct Maker {
    out: (u32, u32, format::Pixel),
    keep: Vec<crate::hw::Kind>,
    stages: Vec<Stage>,
    shrink: Option<(u32, u32)>,
    reading: Reading,
}

struct Stage {
    shrinker: Option<crate::hw::Shrinker>,
    relayout: Relayout,
    keeper: Option<crate::hw::Keeper>,
    unkept: bool,
    reading: Option<Reading>,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            shrinker: None,
            relayout: Relayout::new(),
            keeper: None,
            unkept: false,
            reading: None,
        }
    }
}

impl Maker {
    pub(crate) fn new(
        out: (u32, u32, format::Pixel),
        keep: Vec<crate::hw::Kind>,
        reading: Reading,
    ) -> Self {
        Self {
            out,
            keep,
            stages: Vec::new(),
            shrink: None,
            reading,
        }
    }

    pub(crate) fn shrink_to(&mut self, size: Option<(u32, u32)>) {
        self.shrink = size;
    }

    pub(crate) fn shrinks(&self) -> Option<(u32, u32)> {
        self.shrink
    }

    pub(crate) fn clear_stages(&mut self) {
        self.stages.clear();
    }

    pub(crate) fn out(&self) -> (u32, u32, format::Pixel) {
        self.out
    }

    pub(crate) fn reading(&self) -> Reading {
        self.reading
    }

    pub(crate) fn read_as(&mut self, reading: Reading) {
        self.reading = reading;
        self.stages.clear();
    }

    fn keeps(&self, frame: &frame::Video) -> bool {
        self.shrink.is_none()
            && crate::hw::Kind::of_frame(frame).is_some_and(|k| self.keep.contains(&k))
    }

    pub(crate) fn make(
        &mut self,
        fitting: &mut Fitting,
        frame: &frame::Video,
        rotation: u16,
        path: &Path,
    ) -> Result<frame::Video> {
        self.make_layer(0, fitting, frame, rotation, path, self.reading)
    }

    pub(crate) fn make_layer(
        &mut self,
        layer: usize,
        fitting: &mut Fitting,
        frame: &frame::Video,
        rotation: u16,
        path: &Path,
        reading: Reading,
    ) -> Result<frame::Video> {
        if self.stages.len() <= layer {
            self.stages.resize_with(layer + 1, Stage::default);
        }
        let mut stage = std::mem::take(&mut self.stages[layer]);
        let made = self.make_with(&mut stage, fitting, frame, rotation, path, reading);
        self.stages[layer] = stage;
        made
    }

    fn make_with(
        &mut self,
        stage: &mut Stage,
        fitting: &mut Fitting,
        frame: &frame::Video,
        rotation: u16,
        path: &Path,
        reading: Reading,
    ) -> Result<frame::Video> {
        if stage.reading.replace(reading).is_some_and(|r| r != reading) {
            stage.relayout = Relayout::new();
        }
        let mut facts = Facts {
            keep_failed: stage.unkept,
            ..Facts::of(frame, self.keeps(frame))
        };
        let mut route;
        let mut came_down_at = None;
        let picture = loop {
            route = plan(&facts);
            let came_down = match route {
                Route::Kept => match crate::hw::Keeper::keep(&mut stage.keeper, frame) {
                    Ok(kept) => break kept,
                    Err(e) => {
                        eprintln!("vidcut: GPU pictures come down to be shown: {e}");
                        stage.unkept = true;
                        stage.keeper = None;
                        facts.keep_failed = true;
                        continue;
                    }
                },
                Route::Whole => Some(match self.shrink {
                    Some(size) => down(&mut stage.shrinker, frame, rotation, size)?,
                    None => crate::hw::to_read(frame)?,
                }),
                Route::Cpu => None,
            };
            came_down_at = came_down.as_ref().map(|f| (f.width(), f.height()));
            let frame = came_down.as_ref().unwrap_or(frame);
            break crate::buffers::owned(&stage.relayout.run(frame, reading)?)?;
        };
        fitting.went = Some(match came_down_at {
            Some(size) => Went::Down {
                size,
                whole: size == (frame.width(), frame.height()),
            },
            None if route == Route::Cpu => Went::Cpu,
            None => Went::Stayed,
        });
        fitting.took(route, path);
        Ok(picture)
    }
}
