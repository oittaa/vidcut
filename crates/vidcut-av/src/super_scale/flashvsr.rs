use std::path::Path;

use vidcut_core::model::Upscaler;

use crate::compute::Gpu;
use crate::error::{Error, Result};
use crate::models::{self, Fetch, Need};
use crate::screen::held::Held;
use vidcut_core::catalog::{self, FLASHVSR_FILES as FILES};

mod gemm;
mod kernels;
mod model;
mod resize;
mod run;

const DIM: usize = 1536;
const FFN: usize = 8960;
const LAYERS: usize = 30;
const GLOBAL: [usize; 5] = [0, 6, 12, 18, 24];

pub const SCALE: u32 = catalog::FLASHVSR_SCALE;

const WHAT: &str = "the AI Slow upscaler";
const SHARED: u32 = 32 * 1024;

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

pub type Sizes = ((u32, u32), (u32, u32));

pub fn check(sizes: &[Sizes], dir: &Path, download: bool) -> Result<()> {
    get_ready(dir, download)?;
    with_engine(|gpu, _| {
        for (source, size) in sizes {
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
    gpu: Gpu,
    model: Option<model::Model>,
    lent: Option<crate::player::Lent>,
}

static ENGINE: super::Slot<Engine> = super::Slot::new();

impl super::Network for Engine {
    const UPSCALER: Upscaler = Upscaler::AiSlow;

    fn start() -> std::result::Result<Self, String> {
        let gpu = Gpu::start(WHAT, SHARED)?;
        super::runs_on(Upscaler::AiSlow, gpu.name.clone());
        Ok(Engine {
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

fn with_engine<T>(run: impl FnOnce(&Gpu, &model::Model) -> Result<T>) -> Result<T> {
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
            let loaded = crate::compute::checked(&gpu.device, WHAT, || model::load(gpu, &paths))?;
            e.lent = listed
                .zip(room)
                .and_then(|(on, room)| crate::player::lend_on(on, loaded.bytes, room));
            e.model = Some(loaded);
        }
        let model = e.model.as_ref().expect("loaded");
        crate::compute::checked(&e.gpu.device, WHAT, || run(&e.gpu, model))
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

    fn outs(&mut self, made: Vec<Held>) -> Vec<super::Out> {
        made.into_iter()
            .filter_map(|held| {
                let (index, pts) = self.fed.pop_front()?;
                Some(super::Out {
                    index,
                    pts,
                    picture: super::Picture::Held(held),
                })
            })
            .collect()
    }
}

impl super::Run for Run {
    fn push(
        &mut self,
        index: i64,
        pts: Option<i64>,
        rgb: &crate::ffmpeg::frame::Video,
    ) -> Result<Vec<super::Out>> {
        if !crate::screen::held::deep(rgb) {
            return Err(Error::Invalid(
                "the AI Slow upscaler takes 16-bit pictures".into(),
            ));
        }
        let held = crate::screen::held::held(rgb)?;
        let outs = self.stream.push(held.buffer())?;
        self.fed.push_back((index, pts));
        Ok(self.outs(outs))
    }

    fn finish(&mut self) -> Result<Vec<super::Out>> {
        let outs = self.stream.finish()?;
        Ok(self.outs(outs))
    }
}

impl Stream {
    pub fn open(source: (u32, u32), size: (u32, u32)) -> Result<Self> {
        if source.0 == 0 || source.1 == 0 || size.0 < source.0 || size.1 < source.1 {
            return Err(Error::Invalid(format!(
                "the AI Slow upscaler enlarges pictures: {}x{} to {}x{} is not larger",
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

    pub(crate) fn push(&mut self, rgb: &wgpu::Buffer) -> Result<Vec<Held>> {
        with_engine(|gpu, model| self.inner.push(gpu, model, rgb))
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<Held>> {
        with_engine(|gpu, model| self.inner.finish(gpu, model))
    }

    pub fn size(&self) -> (u32, u32) {
        (self.inner.g.w, self.inner.g.h)
    }
}
