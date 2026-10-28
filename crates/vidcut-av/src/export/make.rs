use std::path::Path;

use super::normalize::Relayout;
use crate::decode::Reading;
use crate::error::Result;
use crate::ffmpeg::{format, frame};
use crate::route::{Facts, Route, Went, plan};

pub(crate) struct Fitting {
    pub(crate) route: Option<Route>,
}

impl Fitting {
    pub(crate) fn none() -> Self {
        Self { route: None }
    }

    pub(crate) fn went(&self) -> Option<Went> {
        self.route.map(|route| match route {
            Route::Kept => Went::Stayed,
            Route::Whole => Went::Down,
            Route::Cpu => Went::Cpu,
        })
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
    keep: crate::hw::GpuPictures,
    holds: bool,
    stages: Vec<Stage>,
    reading: Reading,
}

struct Stage {
    relayout: Relayout,
    keeper: Option<crate::hw::Keeper>,
    unkept: bool,
    reading: Option<Reading>,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
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
        keep: crate::hw::GpuPictures,
        reading: Reading,
    ) -> Self {
        Self {
            out,
            keep,
            holds: false,
            stages: Vec::new(),
            reading,
        }
    }

    pub(crate) fn holding(self) -> Self {
        Self {
            holds: true,
            ..self
        }
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
        let layout = crate::player::Layout::of(crate::hw::sw_format(frame));
        crate::hw::Kind::of_frame(frame).is_some_and(|k| self.keep.contains(&(k, layout)))
    }

    pub(crate) fn make(
        &mut self,
        fitting: &mut Fitting,
        frame: &frame::Video,
        path: &Path,
    ) -> Result<frame::Video> {
        self.make_layer(0, fitting, frame, path, self.reading)
    }

    pub(crate) fn make_layer(
        &mut self,
        layer: usize,
        fitting: &mut Fitting,
        frame: &frame::Video,
        path: &Path,
        reading: Reading,
    ) -> Result<frame::Video> {
        if self.stages.len() <= layer {
            self.stages.resize_with(layer + 1, Stage::default);
        }
        let mut stage = std::mem::take(&mut self.stages[layer]);
        let made = self.make_with(&mut stage, fitting, frame, path, reading);
        self.stages[layer] = stage;
        made
    }

    fn make_with(
        &mut self,
        stage: &mut Stage,
        fitting: &mut Fitting,
        frame: &frame::Video,
        path: &Path,
        reading: Reading,
    ) -> Result<frame::Video> {
        if stage.reading.replace(reading).is_some_and(|r| r != reading) {
            stage.relayout = Relayout::new();
        }
        if crate::screen::held::of(frame).is_some() {
            if self.holds {
                fitting.took(Route::Kept, path);
                return crate::export::video::frame_ref(frame);
            }
            let down = crate::screen::held::in_memory(frame)?;
            fitting.took(Route::Whole, path);
            return crate::buffers::owned(&stage.relayout.run(&down, reading)?);
        }
        let mut facts = Facts {
            keep_failed: stage.unkept,
            ..Facts::of(frame, self.keeps(frame))
        };
        let mut route;
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
                Route::Whole => Some(crate::hw::to_read(frame)?),
                Route::Cpu => None,
            };
            let frame = came_down.as_ref().unwrap_or(frame);
            break crate::buffers::owned(&stage.relayout.run(frame, reading)?)?;
        };
        fitting.took(route, path);
        Ok(picture)
    }
}
