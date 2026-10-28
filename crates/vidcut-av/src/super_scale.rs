pub mod flashvsr;
pub mod macos;
pub mod nanovsr;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use vidcut_core::model::{Media, Project, SuperScale, Upscaler};
use vidcut_core::render::{RenderPlan, VideoPiece};

use vidcut_core::catalog::{self, Measure};

use crate::error::{Error, Result};
use crate::models::Need;

pub fn needed(dir: &Path, upscaler: Upscaler) -> Vec<Need> {
    match upscaler {
        Upscaler::AiFast => nanovsr::needed(dir),
        Upscaler::AiBest => flashvsr::needed(dir),
        Upscaler::Macos => macos::needed(),
        Upscaler::Classic => Vec::new(),
    }
}

pub fn get_ready(dir: &Path, upscaler: Upscaler, download: bool) -> Result<()> {
    match upscaler {
        Upscaler::AiFast => nanovsr::get_ready(dir, download),
        Upscaler::AiBest => flashvsr::get_ready(dir, download),
        Upscaler::Macos => macos::get_ready(download),
        Upscaler::Classic => Ok(()),
    }
}

#[cfg(target_os = "macos")]
pub fn off_here(upscaler: Upscaler) -> bool {
    matches!(upscaler, Upscaler::AiFast | Upscaler::AiBest)
}

#[cfg(not(target_os = "macos"))]
pub fn off_here(_upscaler: Upscaler) -> bool {
    false
}

pub fn unfit(m: &Media, upscaler: Upscaler) -> Option<&'static str> {
    if off_here(upscaler) {
        return Some("pictures on macOS for now");
    }
    let model = match upscaler {
        Upscaler::AiFast => &catalog::NANOVSR,
        Upscaler::AiBest => &catalog::FLASHVSR,
        Upscaler::Macos => return macos::unfit(m),
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
        Upscaler::Macos | Upscaler::Classic => None,
    }
}

pub(crate) struct User(Upscaler);

impl User {
    pub(crate) fn of(s: SuperScale) -> Option<Self> {
        let i = slot(s.upscaler).filter(|_| s.ai())?;
        users()[i] += 1;
        crate::screen::held::share_again();
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
                Upscaler::AiFast => nanovsr::release(),
                Upscaler::AiBest => flashvsr::release(),
                Upscaler::Macos | Upscaler::Classic => {}
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
    engine: Mutex<Option<crate::screen::Started<N>>>,
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
        let n = match crate::screen::start_on_offscreen(&mut engine, N::start) {
            Ok(n) => n,
            Err(why) => return Err(Error::Invalid(why.clone())),
        };
        let out = run(n);
        self.tidy(n);
        out
    }

    pub(crate) fn started(&self, run: impl FnOnce(&mut N)) {
        let mut engine = self.engine.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, Ok(n))) = engine.as_mut() {
            run(n);
            self.tidy(n);
        }
    }

    pub(crate) fn release(&self) {
        self.later.store(true, Ordering::Relaxed);
        if let Ok(mut engine) = self.engine.try_lock()
            && let Some((_, Ok(n))) = engine.as_mut()
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

pub(crate) enum Picture {
    Held(crate::screen::held::Held),
    Rgb(Vec<u8>),
    Frame(crate::ffmpeg::frame::Video),
}

pub(crate) struct Out {
    pub(crate) index: i64,
    pub(crate) pts: Option<i64>,
    pub(crate) picture: Picture,
}

pub(crate) type Sink = Box<dyn FnOnce(Result<Vec<Out>>) + Send>;

pub(crate) trait Run: Send {
    fn push(
        &mut self,
        index: i64,
        pts: Option<i64>,
        rgb: &crate::ffmpeg::frame::Video,
    ) -> Result<Vec<Out>>;

    fn takes(&self, _f: &crate::ffmpeg::frame::Video, _rotation: u16) -> bool {
        false
    }

    fn push_frame(
        &mut self,
        index: i64,
        f: &crate::ffmpeg::frame::Video,
        rotation: u16,
    ) -> Result<Vec<Out>> {
        let _ = (index, f, rotation);
        Err(Error::Invalid("this upscaler takes RGB pictures".into()))
    }

    fn finish(&mut self) -> Result<Vec<Out>>;
}

pub(crate) fn first_frame(upscaler: Upscaler, want: i64) -> i64 {
    match upscaler {
        Upscaler::AiFast | Upscaler::Classic => want - want.rem_euclid(nanovsr::FRAMES as i64),
        Upscaler::AiBest | Upscaler::Macos => want,
    }
}

pub(crate) fn last_frame(upscaler: Upscaler, first: i64) -> Option<i64> {
    match upscaler {
        Upscaler::AiFast | Upscaler::Classic => Some(first + nanovsr::FRAMES as i64 - 1),
        Upscaler::AiBest | Upscaler::Macos => None,
    }
}

fn need(size: (u32, u32), shown: (u32, u32)) -> f64 {
    (f64::from(shown.0) / f64::from(size.0)).min(f64::from(shown.1) / f64::from(size.1))
}

fn scaled(size: (u32, u32), s: f64) -> (u32, u32) {
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    (
        even(f64::from(size.0) * s).max(size.0),
        even(f64::from(size.1) * s).max(size.1),
    )
}

pub(crate) fn made_size(upscaler: Upscaler, size: (u32, u32), shown: (u32, u32)) -> (u32, u32) {
    match upscaler {
        Upscaler::AiFast | Upscaler::Classic => {
            let scale = nanovsr::SCALE;
            (size.0 * scale, size.1 * scale)
        }
        Upscaler::AiBest => scaled(size, need(size, shown).min(f64::from(flashvsr::SCALE))),
        Upscaler::Macos => scaled(
            size,
            need(size, shown).min(f64::from(macos::factor(size, shown))),
        ),
    }
}

pub(crate) fn open(
    upscaler: Upscaler,
    size: (u32, u32),
    made: (u32, u32),
    want: i64,
    sink: Sink,
) -> Result<Box<dyn Run>> {
    if off_here(upscaler) {
        return Err(Error::Invalid(format!(
            "{} is off on macOS for now: set the Upscaler to AI or Classic",
            name(upscaler)
        )));
    }
    match upscaler {
        Upscaler::AiFast | Upscaler::Classic => Ok(Box::new(nanovsr::Run::new(size, want, sink))),
        Upscaler::AiBest => Ok(Box::new(flashvsr::Run::open(size, made)?)),
        Upscaler::Macos => Ok(Box::new(macos::Run::open(size, made)?)),
    }
}

pub(crate) fn settle(upscaler: Upscaler) {
    if upscaler == Upscaler::AiFast {
        nanovsr::settle();
    }
}

pub fn name(upscaler: Upscaler) -> &'static str {
    match upscaler {
        Upscaler::AiFast => "AI Fast (NanoVSR 5.4M)",
        Upscaler::AiBest => "AI Best (FlashVSR+ Turbo)",
        Upscaler::Macos => "AI (macOS super resolution)",
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
    let mut off: Vec<String> = plans
        .iter()
        .flat_map(|plan| &plan.video)
        .flat_map(|piece| piece.layers())
        .filter_map(|(layer, _)| match layer {
            VideoPiece::Source {
                media, super_scale, ..
            } if super_scale.ai() && off_here(super_scale.upscaler) => {
                p.media(media).map(|m| m.name.clone())
            }
            _ => None,
        })
        .collect();
    off.sort();
    off.dedup();
    if !off.is_empty() {
        return Err(Error::Invalid(format!(
            "AI Fast and AI Best are off on macOS for now: set the Upscaler of {} to AI or \
             Classic",
            off.join(", ")
        )));
    }
    nanovsr::check(p, plans.iter().flat_map(|plan| &plan.video), dir, download)?;
    flashvsr::check(p, plans.iter().copied(), dir, download)?;
    macos::check(p, plans.iter().flat_map(|plan| &plan.video), download)
}
