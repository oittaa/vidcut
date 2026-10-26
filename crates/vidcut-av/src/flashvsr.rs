use std::path::Path;

use vidcut_core::model::{Media, Project, Upscaler};
use vidcut_core::render::{RenderPlan, VideoPiece};

use crate::error::{Error, Result};
use crate::models::{self, Fetch, Need};
use vidcut_core::catalog::{self, FLASHVSR_FILES as FILES};

mod dump;
mod gemm;
mod gpu;
mod kernels;
mod model;
mod resize;
mod run;
mod weights;

const DIM: usize = 1536;
const FFN: usize = 8960;
const LAYERS: usize = 30;
const GLOBAL: [usize; 5] = [0, 6, 12, 18, 24];

pub const SCALE: u32 = catalog::FLASHVSR_SCALE;

pub fn needed(dir: &Path) -> Vec<Need> {
    FILES
        .iter()
        .filter(|f| !models::present(dir, f))
        .map(|f| Need {
            title: f.title.into(),
            bytes: Some(f.bytes),
            system: false,
        })
        .collect()
}

pub fn get_ready(dir: &Path, download: bool) -> Result<()> {
    let missing = needed(dir);
    if missing.is_empty() {
        return Ok(());
    }
    if !download {
        let one = missing.len() == 1;
        return Err(Error::Missing(format!(
            "{} {} on this computer yet: pass download (--download) to download {} into {}, \
             {}checked against its SHA-256",
            models::listed(&missing),
            if one { "isn't" } else { "aren't" },
            if one { "it" } else { "them" },
            dir.display(),
            if one { "" } else { "each " }
        )));
    }
    models::network();
    for f in FILES.iter().filter(|f| !models::present(dir, f)) {
        models::fetch(&Fetch::of(f), dir)?;
    }
    Ok(())
}

pub fn unfit(m: &Media) -> Option<&'static str> {
    crate::ai::unfit(m, Upscaler::AiBest)
}

pub fn enlarged(m: &Media, (ow, oh): (u32, u32)) -> Option<((u32, u32), (u32, u32))> {
    let v = m.video_as_read()?;
    let across = if v.sar[0] > 0 && v.sar[1] > 0 {
        f64::from(v.sar[0]) / f64::from(v.sar[1])
    } else {
        1.0
    };
    let (dw, dh) = (f64::from(v.width) * across, f64::from(v.height));
    let (dw, dh) = if v.rotation % 180 == 90 {
        (dh, dw)
    } else {
        (dw, dh)
    };
    let even = |x: f64| ((x / 2.0).round() as u32 * 2).max(2);
    let shown = (even(dw), even(dh));
    let s = (f64::from(ow) / f64::from(shown.0))
        .min(f64::from(oh) / f64::from(shown.1))
        .min(f64::from(SCALE));
    (s > 1.0).then(|| {
        (
            shown,
            (
                even(f64::from(shown.0) * s).max(shown.0),
                even(f64::from(shown.1) * s).max(shown.1),
            ),
        )
    })
}

pub fn check<'a>(
    p: &Project,
    plans: impl IntoIterator<Item = &'a RenderPlan>,
    dir: &Path,
    download: bool,
) -> Result<()> {
    let mut media: Vec<&str> = Vec::new();
    let mut sizes: Vec<((u32, u32), (u32, u32))> = Vec::new();
    for plan in plans {
        for (layer, _) in plan.video.iter().flat_map(|piece| piece.layers()) {
            let VideoPiece::Source {
                media: id,
                super_scale,
                ..
            } = layer
            else {
                continue;
            };
            if !super_scale.ai() || super_scale.upscaler != Upscaler::AiBest {
                continue;
            }
            if !media.contains(&id.as_str()) {
                media.push(id);
            }
            if let Some(size) = p
                .media(id)
                .and_then(|m| enlarged(m, (plan.width, plan.height)))
                && !sizes.contains(&size)
            {
                sizes.push(size);
            }
        }
    }
    if media.is_empty() {
        return Ok(());
    }
    let refused: Vec<String> = media
        .iter()
        .filter_map(|id| p.media(id))
        .filter_map(|m| unfit(m).map(|why| format!("{} ({why})", m.name)))
        .collect();
    if !refused.is_empty() {
        return Err(Error::Invalid(format!(
            "the AI Best upscaler of Super Scale can't take {}: set their Upscaler to Classic, \
             or their Super Scale to Off",
            refused.join(", ")
        )));
    }
    get_ready(dir, download)?;
    with_engine(|gpu, _| {
        for (source, size) in &sizes {
            run::plan(gpu, &run::Geometry::new(*source, *size))?;
        }
        Ok(())
    })
}

pub fn inspect(dir: &Path, download: bool) -> Result<u64> {
    get_ready(dir, download)?;
    crate::models::verify(dir, &FILES)?;
    let bytes = with_engine(|_, model| Ok(model.bytes));
    release();
    bytes
}

struct Engine {
    gpu: gpu::Gpu,
    model: Option<model::Model>,
    lent: Option<crate::player::Lent>,
}

static ENGINE: crate::ai::Slot<Engine> = crate::ai::Slot::new();

impl crate::ai::Network for Engine {
    const UPSCALER: Upscaler = Upscaler::AiBest;

    fn start() -> std::result::Result<Self, String> {
        gpu::Gpu::start().map(|gpu| Engine {
            gpu,
            model: None,
            lent: None,
        })
    }

    fn free(&mut self) {
        self.model = None;
        self.lent = None;
    }
}

fn with_engine<T>(run: impl FnOnce(&gpu::Gpu, &model::Model) -> Result<T>) -> Result<T> {
    ENGINE.with(|e| {
        if e.model.is_none() {
            let dir = vidcut_core::dirs::models();
            get_ready(&dir, false)?;
            let path = |i: usize| dir.join(FILES[i].name);
            let (dit, lq, decoder, prompt) = (path(0), path(1), path(2), path(3));
            let paths = model::Paths {
                dit: &dit,
                lq: &lq,
                decoder: &decoder,
                prompt: &prompt,
            };
            let gpu = &e.gpu;
            let listed = gpu.listed.as_ref();
            let room = listed.and_then(crate::player::room_on);
            let loaded =
                crate::compute::checked(&gpu.device, gpu::WHAT, || model::load(gpu, &paths))?;
            e.lent = listed
                .zip(room)
                .and_then(|(on, room)| crate::player::lend_on(on, loaded.bytes, room));
            e.model = Some(loaded);
        }
        let model = e.model.as_ref().expect("loaded");
        crate::compute::checked(&e.gpu.device, gpu::WHAT, || run(&e.gpu, model))
    })
}

pub fn release() {
    ENGINE.release();
}

pub fn model_size((w, h): (u32, u32)) -> (u32, u32) {
    (w.next_multiple_of(128), h.next_multiple_of(128))
}

pub struct Stream {
    inner: run::Stream,
    _lent: Option<crate::player::Lent>,
}

pub(crate) struct Run {
    stream: Stream,
    fed: std::collections::VecDeque<(i64, Option<i64>)>,
}

impl Run {
    pub(crate) fn open(size: (u32, u32), made: (u32, u32)) -> Result<Self> {
        Ok(Self {
            stream: Stream::open(size, made)?,
            fed: std::collections::VecDeque::new(),
        })
    }

    fn outs(&mut self, rgbs: Vec<Vec<u8>>) -> Vec<crate::ai::Out> {
        rgbs.into_iter()
            .filter_map(|rgb| {
                let (index, pts) = self.fed.pop_front()?;
                Some(crate::ai::Out { index, pts, rgb })
            })
            .collect()
    }
}

impl crate::ai::Run for Run {
    fn push(&mut self, index: i64, pts: Option<i64>, rgb: Vec<u8>) -> Result<Vec<crate::ai::Out>> {
        let outs = self.stream.push(&rgb)?;
        self.fed.push_back((index, pts));
        Ok(self.outs(outs))
    }

    fn finish(&mut self) -> Result<Vec<crate::ai::Out>> {
        let outs = self.stream.finish()?;
        Ok(self.outs(outs))
    }
}

impl Stream {
    pub fn open(source: (u32, u32), size: (u32, u32)) -> Result<Self> {
        if source.0 == 0 || source.1 == 0 || size.0 < source.0 || size.1 < source.1 {
            return Err(Error::Invalid(format!(
                "the AI Best upscaler enlarges pictures: {}x{} to {}x{} is not larger",
                source.0, source.1, size.0, size.1
            )));
        }
        let geometry = run::Geometry::new(source, size);
        let dump = std::env::var_os("VIDCUT_FLASHVSR_DUMP").map(std::path::PathBuf::from);
        if let Some(dir) = &dump {
            std::fs::create_dir_all(dir)?;
        }
        let (inner, lent) = with_engine(|gpu, _| {
            let plan = run::plan(gpu, &geometry)?;
            let lent = gpu
                .listed
                .as_ref()
                .zip(plan.room)
                .and_then(|(on, room)| crate::player::lend_on(on, plan.need, room));
            Ok((run::Stream::new(gpu, geometry, &plan.resident, dump)?, lent))
        })?;
        Ok(Self { inner, _lent: lent })
    }

    pub fn push(&mut self, rgb: &[u8]) -> Result<Vec<Vec<u8>>> {
        with_engine(|gpu, model| self.inner.push(gpu, model, rgb))
    }

    pub fn finish(&mut self) -> Result<Vec<Vec<u8>>> {
        with_engine(|gpu, model| self.inner.finish(gpu, model))
    }

    pub fn size(&self) -> (u32, u32) {
        (self.inner.g.w, self.inner.g.h)
    }
}
