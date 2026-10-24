use std::path::Path;
use std::sync::{Mutex, PoisonError};

use vidcut_core::model::{Media, Project, Upscaler};
use vidcut_core::render::{RenderPlan, VideoPiece};

use crate::error::{Error, Result};
use crate::models::{self, Fetch, File, Need};

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

pub const SCALE: u32 = 4;

const HUB: &str = "https://huggingface.co/ruchangyao/FlashVSR_plus/resolve/46b79520946ac67dec94d1566490690953c7debc";
const CODE: &str = "https://raw.githubusercontent.com/OpenImagingLab/FlashVSR_Plus/b548a2cc2c929d3d4676f2973abe27876ec4dee7";

pub const FILES: [File; 4] = [
    File {
        name: "flashvsr-plus/turbo/dit.safetensors",
        title: "FlashVSR+ Turbo transformer",
        repo: "ruchangyao/FlashVSR_plus",
        revision: "46b79520946ac67dec94d1566490690953c7debc",
        bytes: 5_676_070_392,
        sha256: "7862b92178176fa3aebc34c6c071daca6b56ab1dddb8e7ff9a01c47f5226ec8b",
        gpu_gb: 0,
        languages: &[],
    },
    File {
        name: "flashvsr-plus/turbo/lq_proj_in.safetensors",
        title: "FlashVSR+ Turbo input projection",
        repo: "ruchangyao/FlashVSR_plus",
        revision: "46b79520946ac67dec94d1566490690953c7debc",
        bytes: 1_151_384_272,
        sha256: "e77b378ae12342eefbb58f924256f362a151b93ce73dd32808fcdaca9c051496",
        gpu_gb: 0,
        languages: &[],
    },
    File {
        name: "flashvsr-plus/TCDecoder.ckpt",
        title: "FlashVSR+ decoder",
        repo: "ruchangyao/FlashVSR_plus",
        revision: "46b79520946ac67dec94d1566490690953c7debc",
        bytes: 189_018_333,
        sha256: "e224bdcf2f52745cbf4d393ff5374c2ba09e90285d5d19062d2bf63b915b6161",
        gpu_gb: 0,
        languages: &[],
    },
    File {
        name: "flashvsr-plus/prompt_tensor/posi_prompt.pth",
        title: "FlashVSR+ prompt",
        repo: "OpenImagingLab/FlashVSR_Plus",
        revision: "b548a2cc2c929d3d4676f2973abe27876ec4dee7",
        bytes: 4_195_504,
        sha256: "4601107a11e4e11a936a6b79df579e54dbc99872132bf542151f0ffd65b4b1ef",
        gpu_gb: 0,
        languages: &[],
    },
];

fn fetch_of(f: &File) -> Fetch {
    let path = f.name.trim_start_matches("flashvsr-plus/");
    let base = if f.repo == "OpenImagingLab/FlashVSR_Plus" {
        CODE
    } else {
        HUB
    };
    Fetch {
        url: format!("{base}/{path}"),
        name: f.name.to_string(),
        title: f.title.to_string(),
        bytes: f.bytes,
        sha256: f.sha256,
    }
}

pub fn needed(dir: &Path) -> Vec<Need> {
    FILES
        .iter()
        .filter(|f| !models::present(dir, f))
        .map(|f| Need {
            title: f.title.into(),
            bytes: f.bytes,
        })
        .collect()
}

pub fn get_ready(dir: &Path, download: bool) -> Result<()> {
    let missing = needed(dir);
    if missing.is_empty() {
        return Ok(());
    }
    if !download {
        return Err(Error::Missing(format!(
            "{} isn't on this computer yet: pass download (--download) to download it into {}, \
             checked against its SHA-256",
            models::listed(&missing),
            dir.display()
        )));
    }
    models::network();
    for f in FILES.iter().filter(|f| !models::present(dir, f)) {
        models::fetch(&fetch_of(f), dir)?;
    }
    Ok(())
}

pub fn unfit(m: &Media) -> Option<&'static str> {
    m.video_as_read()?.hdr.map(|_| "HDR pictures")
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
    with_engine(|gpu, model| {
        for (source, size) in &sizes {
            run::plan(gpu, &run::Geometry::new(*source, *size), model.bytes)?;
        }
        Ok(())
    })
}

struct Engine {
    gpu: gpu::Gpu,
    model: Option<model::Model>,
}

static ENGINE: Mutex<Option<std::result::Result<Engine, String>>> = Mutex::new(None);

fn with_engine<T>(run: impl FnOnce(&gpu::Gpu, &model::Model) -> Result<T>) -> Result<T> {
    let mut engine = ENGINE.lock().unwrap_or_else(PoisonError::into_inner);
    let e = match engine
        .get_or_insert_with(|| gpu::Gpu::start().map(|gpu| Engine { gpu, model: None }))
    {
        Ok(e) => e,
        Err(why) => return Err(Error::Invalid(why.clone())),
    };
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
        let loaded = crate::compute::checked(&gpu.device, gpu::WHAT, || model::load(gpu, &paths))?;
        e.model = Some(loaded);
    }
    let model = e.model.as_ref().expect("loaded");
    crate::compute::checked(&e.gpu.device, gpu::WHAT, || run(&e.gpu, model))
}

pub fn release() {
    let mut engine = ENGINE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(Ok(e)) = engine.as_mut() {
        e.model = None;
    }
}

pub fn model_size((w, h): (u32, u32)) -> (u32, u32) {
    (w.next_multiple_of(128), h.next_multiple_of(128))
}

pub struct Stream {
    inner: run::Stream,
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
        let inner = with_engine(|gpu, model| {
            let resident = run::plan(gpu, &geometry, model.bytes)?;
            run::Stream::new(gpu, geometry, &resident, dump)
        })?;
        Ok(Self { inner })
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
