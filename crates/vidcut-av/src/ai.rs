use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use vidcut_core::model::{Media, Project, SuperScale, Upscaler};
use vidcut_core::render::RenderPlan;

use vidcut_core::catalog::{self, Measure};

use crate::error::{Error, Result};
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
    pub(crate) fn finished(mut self, frames: (i64, i64)) {
        if let Some(w) = report_lock().making.as_mut() {
            w.frames = frames;
        }
        made(true);
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

static USERS: Mutex<[usize; 2]> = Mutex::new([0, 0]);

fn users() -> MutexGuard<'static, [usize; 2]> {
    USERS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn slot(upscaler: Upscaler) -> Option<usize> {
    match upscaler {
        Upscaler::AiFast => Some(0),
        Upscaler::AiBest => Some(1),
        Upscaler::Classic => None,
    }
}

pub(crate) struct User(Upscaler);

impl User {
    pub(crate) fn of(s: SuperScale) -> Option<Self> {
        let i = slot(s.upscaler).filter(|_| s.ai())?;
        users()[i] += 1;
        Some(Self(s.upscaler))
    }
}

impl Drop for User {
    fn drop(&mut self) {
        let Some(i) = slot(self.0) else {
            return;
        };
        let last = {
            let mut u = users();
            u[i] = u[i].saturating_sub(1);
            u[i] == 0
        };
        if last {
            match self.0 {
                Upscaler::AiFast => crate::upscale::release(),
                Upscaler::AiBest => crate::flashvsr::release(),
                Upscaler::Classic => {}
            }
        }
    }
}

pub(crate) fn in_use(upscaler: Upscaler) -> bool {
    slot(upscaler).is_some_and(|i| users()[i] > 0)
}

pub(crate) fn runs_on(upscaler: Upscaler, device: String) {
    let mut r = report_lock();
    r.devices.retain(|(u, _)| *u != upscaler);
    r.devices.push((upscaler, device));
}

pub(crate) trait Network: Send {
    const UPSCALER: Upscaler;

    fn start() -> std::result::Result<Self, String>
    where
        Self: Sized;

    fn free(&mut self);

    fn busy(&self) -> bool {
        false
    }
}

pub(crate) struct Slot<N> {
    engine: Mutex<Option<std::result::Result<N, String>>>,
    later: AtomicBool,
}

impl<N: Network> Slot<N> {
    pub(crate) const fn new() -> Self {
        Self {
            engine: Mutex::new(None),
            later: AtomicBool::new(false),
        }
    }

    pub(crate) fn with<T>(&self, run: impl FnOnce(&mut N) -> Result<T>) -> Result<T> {
        let mut engine = self.engine.lock().unwrap_or_else(PoisonError::into_inner);
        let n = match engine.get_or_insert_with(N::start) {
            Ok(n) => n,
            Err(why) => return Err(Error::Invalid(why.clone())),
        };
        let out = run(n);
        self.tidy(n);
        out
    }

    pub(crate) fn started(&self, run: impl FnOnce(&mut N)) {
        let mut engine = self.engine.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(Ok(n)) = engine.as_mut() {
            run(n);
            self.tidy(n);
        }
    }

    pub(crate) fn release(&self) {
        self.later.store(true, Ordering::Relaxed);
        if let Ok(mut engine) = self.engine.try_lock()
            && let Some(Ok(n)) = engine.as_mut()
        {
            self.tidy(n);
        }
    }

    fn tidy(&self, n: &mut N) {
        if n.busy() || !self.later.swap(false, Ordering::Relaxed) {
            return;
        }
        if !in_use(N::UPSCALER) {
            n.free();
        }
    }
}

pub(crate) struct Out {
    pub(crate) index: i64,
    pub(crate) pts: Option<i64>,
    pub(crate) rgb: Vec<u8>,
}

pub(crate) type Sink = Box<dyn FnOnce(Result<Vec<Out>>) + Send>;

pub(crate) trait Run: Send {
    fn push(&mut self, index: i64, pts: Option<i64>, rgb: Vec<u8>) -> Result<Vec<Out>>;

    fn finish(&mut self) -> Result<Vec<Out>>;
}

pub(crate) fn first_frame(upscaler: Upscaler, want: i64) -> i64 {
    match upscaler {
        Upscaler::AiFast | Upscaler::Classic => {
            want - want.rem_euclid(crate::upscale::FRAMES as i64)
        }
        Upscaler::AiBest => want,
    }
}

pub(crate) fn last_frame(upscaler: Upscaler, first: i64) -> Option<i64> {
    match upscaler {
        Upscaler::AiFast | Upscaler::Classic => Some(first + crate::upscale::FRAMES as i64 - 1),
        Upscaler::AiBest => None,
    }
}

pub(crate) fn made_size(upscaler: Upscaler, size: (u32, u32), shown: (u32, u32)) -> (u32, u32) {
    match upscaler {
        Upscaler::AiFast | Upscaler::Classic => {
            let scale = crate::upscale::SCALE;
            (size.0 * scale, size.1 * scale)
        }
        Upscaler::AiBest => {
            let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
            let s = (f64::from(shown.0) / f64::from(size.0))
                .min(f64::from(shown.1) / f64::from(size.1))
                .min(f64::from(crate::flashvsr::SCALE));
            (
                even(f64::from(size.0) * s).max(size.0),
                even(f64::from(size.1) * s).max(size.1),
            )
        }
    }
}

pub(crate) fn open(
    upscaler: Upscaler,
    size: (u32, u32),
    made: (u32, u32),
    want: i64,
    sink: Sink,
) -> Result<Box<dyn Run>> {
    match upscaler {
        Upscaler::AiFast | Upscaler::Classic => {
            Ok(Box::new(crate::upscale::Run::new(size, want, sink)))
        }
        Upscaler::AiBest => Ok(Box::new(crate::flashvsr::Run::open(size, made)?)),
    }
}

pub(crate) fn settle(upscaler: Upscaler) {
    if upscaler == Upscaler::AiFast {
        crate::upscale::settle();
    }
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
