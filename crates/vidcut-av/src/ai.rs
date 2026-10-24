use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use vidcut_core::model::{Media, Project, Upscaler};
use vidcut_core::render::RenderPlan;

use vidcut_core::catalog::{self, Measure};

use crate::error::Result;
use crate::models::Need;

pub fn needed(dir: &Path, upscaler: Upscaler) -> Vec<Need> {
    match upscaler {
        Upscaler::AiFast => crate::upscale::needed(dir),
        Upscaler::AiBest => crate::flashvsr::needed(dir),
        Upscaler::Classic => Vec::new(),
    }
}

pub fn get_ready(dir: &Path, upscaler: Upscaler, download: bool) -> Result<()> {
    match upscaler {
        Upscaler::AiFast => crate::upscale::get_ready(dir, download),
        Upscaler::AiBest => crate::flashvsr::get_ready(dir, download),
        Upscaler::Classic => Ok(()),
    }
}

pub fn unfit(m: &Media, upscaler: Upscaler) -> Option<&'static str> {
    let model = match upscaler {
        Upscaler::AiFast => &catalog::NANOVSR,
        Upscaler::AiBest => &catalog::FLASHVSR,
        Upscaler::Classic => return None,
    };
    let hdr = m.video_as_read()?.hdr.is_some();
    let verdict = catalog::check(model, &[(Measure::HdrPictures, f64::from(u8::from(hdr)))]);
    verdict.refusals.first().map(|_| "HDR pictures")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Work {
    pub upscaler: Upscaler,
    pub from: (u32, u32),
    pub made: (u32, u32),
    pub frames: (i64, i64),
    pub started: Instant,
    pub took: Option<Duration>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub making: Option<Work>,
    pub made: Option<Work>,
    pub devices: Vec<(Upscaler, String)>,
}

static REPORT: Mutex<Report> = Mutex::new(Report {
    making: None,
    made: None,
    devices: Vec::new(),
});

fn report_lock() -> MutexGuard<'static, Report> {
    REPORT.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn report() -> Report {
    report_lock().clone()
}

pub(crate) struct Making {
    done: bool,
}

impl Making {
    pub(crate) fn finished(mut self) {
        made(true);
        self.done = true;
    }

    pub(crate) fn handed(mut self) {
        self.done = true;
    }
}

impl Drop for Making {
    fn drop(&mut self) {
        if !self.done {
            made(false);
        }
    }
}

pub(crate) fn making(
    upscaler: Upscaler,
    from: (u32, u32),
    made: (u32, u32),
    frames: (i64, i64),
) -> Making {
    report_lock().making = Some(Work {
        upscaler,
        from,
        made,
        frames,
        started: Instant::now(),
        took: None,
    });
    Making { done: false }
}

pub(crate) fn made(done: bool) {
    let mut r = report_lock();
    if let Some(mut w) = r.making.take()
        && done
    {
        w.took = Some(w.started.elapsed());
        r.made = Some(w);
    }
}

pub(crate) fn runs_on(upscaler: Upscaler, device: String) {
    let mut r = report_lock();
    r.devices.retain(|(u, _)| *u != upscaler);
    r.devices.push((upscaler, device));
}

pub fn name(upscaler: Upscaler) -> &'static str {
    match upscaler {
        Upscaler::AiFast => "AI Fast (NanoVSR 5.4M)",
        Upscaler::AiBest => "AI Best (FlashVSR+ Turbo)",
        Upscaler::Classic => "Classic",
    }
}

pub fn check<'a>(
    p: &Project,
    plans: impl IntoIterator<Item = &'a RenderPlan>,
    dir: &Path,
    download: bool,
) -> Result<()> {
    let plans: Vec<&RenderPlan> = plans.into_iter().collect();
    crate::upscale::check(p, plans.iter().flat_map(|plan| &plan.video), dir, download)?;
    crate::flashvsr::check(p, plans.iter().copied(), dir, download)
}
