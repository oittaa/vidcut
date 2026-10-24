use std::path::Path;

use vidcut_core::model::{Media, Project, Upscaler};
use vidcut_core::render::RenderPlan;

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
    match upscaler {
        Upscaler::AiFast => crate::upscale::unfit(m),
        Upscaler::AiBest => crate::flashvsr::unfit(m),
        Upscaler::Classic => None,
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
