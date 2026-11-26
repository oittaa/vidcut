use std::path::{Path, PathBuf};

use vidcut_core::analysis::{Detections, Found, Look};

use crate::decode::Input;
use crate::error::Result;
use crate::ffmpeg::{frame, software::scaling};
use crate::models::{self, Need};
use crate::subjects::{self, Looker, Scenes};

mod unisal;

pub use vidcut_core::catalog::UNISAL_WEIGHTS as FILE;

pub fn name() -> String {
    let weights: String = FILE.sha256.chars().take(8).collect();
    format!(
        "salient-{weights}-unisal-{}-centroid-{}",
        unisal::FRAMES,
        unisal::SHARP
    )
}

pub fn needed(dir: &Path) -> Vec<Need> {
    models::needs(dir, &FILE)
}

pub fn get_ready(dir: &Path, download: bool) -> Result<()> {
    models::ready(dir, &FILE, download)
}

pub fn inspect(dir: &Path, download: bool, on_gpu: bool) -> Result<()> {
    get_ready(dir, download)?;
    models::verify(dir, &[FILE])?;
    let size = unisal::sized(16, 9);
    unisal::Model::load(&dir.join(FILE.name), FILE.title, size, on_gpu).map(drop)
}

struct Seeing {
    model: unisal::Model,
    turn: u16,
    scaler: Option<scaling::Context>,
    scenes: Scenes,
    dump: Option<PathBuf>,
}

impl Looker for Seeing {
    fn picture(&mut self, decoded: &frame::Video) -> Result<(Vec<f32>, f32)> {
        let (w, h) = self.model.size();
        let scaled = match self.turn % 180 == 90 {
            true => (h, w),
            false => (w, h),
        };
        let bgr = subjects::small(&mut self.scaler, decoded, scaled, scaling::Flags::LANCZOS)?;
        let scene = self.scenes.score(&bgr)?;
        let size = (w as usize, h as usize);
        let planes = subjects::planes(&bgr, self.turn, size, size);
        Ok((unisal::input(&planes, (w, h)), scene))
    }

    fn found(&mut self, pictures: &[Vec<f32>], first: usize) -> Result<Vec<Vec<Found>>> {
        let maps = self.model.maps(pictures)?;
        let (w, h) = self.model.size();
        if let Some(dir) = &self.dump {
            let size = (h as usize, w as usize);
            for (i, (picture, map)) in pictures.iter().zip(&maps).enumerate() {
                let at = first + i;
                let input = dir.join(format!("{at:05}.npy"));
                crate::npy::write(&input, &[size.0, size.1, 3], picture)?;
                crate::npy::write(
                    &dir.join(format!("map-{at:05}.npy")),
                    &[size.0, size.1],
                    map,
                )?;
            }
        }
        Ok(maps
            .iter()
            .map(|m| vec![unisal::centroid(m, (w, h))])
            .collect())
    }

    fn done(&self, found: &[Vec<Found>]) -> Result<()> {
        let Some(dir) = &self.dump else {
            return Ok(());
        };
        let rows: Vec<f32> = found.iter().flatten().flat_map(|f| f.center).collect();
        crate::npy::write(&dir.join("points.npy"), &[rows.len() / 2, 2], &rows)
    }

    fn on_gpu(&self) -> bool {
        self.model.on_gpu()
    }
}

pub fn find(
    source: &Input,
    look: Look,
    (gpu, on_gpu): (Option<&crate::hw::GpuDecoder>, bool),
    dir: &Path,
) -> Result<Detections> {
    crate::cancel::stop()?;
    get_ready(dir, false)?;
    let weights = dir.join(FILE.name);
    let _turn = crate::turns::cpu_analysis()?;
    let batch = unisal::FRAMES as usize;
    subjects::looked(source, look, (gpu, batch), |first, turn| {
        let (w, h) = subjects::fitted(first, turn, u32::MAX >> 8);
        Ok(Seeing {
            model: unisal::Model::load(&weights, FILE.title, unisal::sized(w, h), on_gpu)?,
            turn,
            scaler: None,
            scenes: Scenes::default(),
            dump: subjects::dumped("VIDCUT_SALIENCY_DUMP")?,
        })
    })
}
