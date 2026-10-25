use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use tract_onnx::prelude::*;
use vidcut_core::render::AudioPiece;

use crate::error::{Error, Result};
use crate::export::audio::Resampler;
use crate::models::{self, Fetch, Need};

pub use vidcut_core::catalog::DPDFNET_ONNX as DPDFNET;

pub const RATE: u32 = 48_000;

pub fn needed(dir: &Path) -> Vec<Need> {
    if models::present(dir, &DPDFNET) {
        return Vec::new();
    }
    vec![Need {
        title: DPDFNET.title.into(),
        bytes: DPDFNET.bytes,
    }]
}

pub fn get_ready(dir: &Path, download: bool) -> Result<()> {
    if models::present(dir, &DPDFNET) {
        return Ok(());
    }
    if !download {
        return Err(Error::Missing(format!(
            "{} ({}) isn't on this computer yet: pass download (--download) to download it into \
             {}, checked against its SHA-256",
            DPDFNET.title,
            models::size(DPDFNET.bytes),
            dir.display()
        )));
    }
    models::network();
    models::fetch(&Fetch::of(&DPDFNET), dir)?;
    Ok(())
}

pub fn inspect(dir: &Path, download: bool) -> Result<()> {
    get_ready(dir, download)?;
    models::verify(dir, &[DPDFNET])?;
    Model::load(&dir.join(DPDFNET.name)).map(drop)
}

pub fn wanted(lanes: &[Vec<AudioPiece>]) -> bool {
    lanes.iter().flatten().any(|p| {
        matches!(
            p,
            AudioPiece::Source {
                isolation: Some(_),
                ..
            }
        )
    })
}

pub fn check(lanes: &[Vec<AudioPiece>]) -> Result<()> {
    if wanted(lanes) {
        model()?;
    }
    Ok(())
}

pub(crate) struct Model {
    plan: Arc<TypedRunnableModel>,
    start: TValue,
    freq: usize,
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
}

fn bad(path: &Path, what: impl Display) -> Error {
    Error::Invalid(format!("{}: {what}", path.display()))
}

fn vorbis(n: usize) -> Vec<f32> {
    let half = n as f64 / 2.0;
    (0..n)
        .map(|i| {
            let s = (std::f64::consts::FRAC_PI_2 * (i as f64 + 0.5) / half).sin();
            (std::f64::consts::FRAC_PI_2 * s * s).sin() as f32
        })
        .collect()
}

impl Model {
    fn load(path: &Path) -> Result<Self> {
        let onnx = tract_onnx::onnx();
        let proto = onnx.proto_model_for_path(path).map_err(|e| bad(path, e))?;
        let meta: HashMap<&str, &str> = proto
            .metadata_props
            .iter()
            .map(|p| (p.key.as_str(), p.value.as_str()))
            .collect();
        let field = |k: &str| {
            meta.get(k)
                .copied()
                .ok_or_else(|| bad(path, format!("no {k} in its metadata")))
        };
        let number = |k: &str| -> Result<usize> {
            field(k)?
                .parse()
                .map_err(|_| bad(path, format!("its {k} is not a number")))
        };
        let floats = |k: &str| -> Result<Vec<f32>> {
            field(k)?
                .split(',')
                .map(|x| {
                    x.trim()
                        .parse()
                        .map_err(|_| bad(path, format!("its {k} is not numbers")))
                })
                .collect()
        };
        let (freq, window, hop) = (
            number("freq_bins")?,
            number("window_length")?,
            number("hop_length")?,
        );
        if number("sample_rate")? != RATE as usize
            || freq < 2
            || window != (freq - 1) * 2
            || hop * 2 != window
            || field("window_type")? != "vorbis"
        {
            return Err(bad(path, "its framing is not a 48 kHz DPDFNet's"));
        }
        let size = number("state_size")?;
        let (erb, spec) = (floats("erb_norm_init")?, floats("spec_norm_init")?);
        if erb.len() != number("erb_norm_state_size")?
            || spec.len() != number("spec_norm_state_size")?
            || erb.len() + spec.len() > size
        {
            return Err(bad(path, "its state does not add up"));
        }
        let mut start = vec![0f32; size];
        start[..erb.len()].copy_from_slice(&erb);
        start[erb.len()..erb.len() + spec.len()].copy_from_slice(&spec);
        let mut model = onnx
            .model_for_proto_model(&proto)
            .map_err(|e| bad(path, e))?;
        model
            .set_input_fact(0, f32::fact([1, 1, freq, 2]).into())
            .and_then(|_| model.set_input_fact(1, f32::fact([size]).into()))
            .map_err(|e| bad(path, e))?;
        let plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| bad(path, e))?;
        let mut planner = FftPlanner::new();
        Ok(Self {
            plan,
            start: tract_ndarray::Array1::from(start).into_tensor().into(),
            freq,
            window: vorbis(window),
            forward: planner.plan_fft_forward(window),
            inverse: planner.plan_fft_inverse(window),
        })
    }

    fn hop(&self) -> usize {
        self.window.len() / 2
    }

    fn delay(&self) -> usize {
        2 * self.window.len()
    }
}

pub(crate) fn model() -> Result<Arc<Model>> {
    static LOADED: Mutex<Option<(PathBuf, Arc<Model>)>> = Mutex::new(None);
    let dir = vidcut_core::dirs::models();
    let path = dir.join(DPDFNET.name);
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((p, m)) = loaded.as_ref()
        && *p == path
    {
        return Ok(m.clone());
    }
    get_ready(&dir, false)?;
    let m = Arc::new(Model::load(&path)?);
    *loaded = Some((path, m.clone()));
    Ok(m)
}

struct Voice {
    state: TValue,
    input: Vec<f32>,
    overlap: Vec<f32>,
    dry: VecDeque<f32>,
    skip: usize,
    wet: Vec<f32>,
    buf: Vec<Complex32>,
    spec: Vec<f32>,
}

impl Voice {
    fn new(model: &Model) -> Self {
        let n = model.window.len();
        Self {
            state: model.start.clone(),
            input: Vec::new(),
            overlap: vec![0.0; n],
            dry: VecDeque::new(),
            skip: model.delay(),
            wet: Vec::new(),
            buf: vec![Complex32::default(); n],
            spec: Vec::with_capacity(model.freq * 2),
        }
    }

    fn push(&mut self, model: &Model, samples: impl Iterator<Item = f32>) -> Result<()> {
        for s in samples {
            self.input.push(s);
            self.dry.push_back(s);
        }
        let (n, hop, freq) = (model.window.len(), model.hop(), model.freq);
        let mut used = 0;
        while self.input.len() - used >= n {
            let frame = &self.input[used..used + n];
            for ((b, x), w) in self.buf.iter_mut().zip(frame).zip(&model.window) {
                *b = Complex32::new(x * w, 0.0);
            }
            model.forward.process(&mut self.buf);
            self.spec.clear();
            self.spec
                .extend(self.buf[..freq].iter().flat_map(|b| [b.re, b.im]));
            let failed = |e: TractError| Error::Invalid(format!("voice isolation: {e}"));
            let spec = Tensor::from_shape(&[1, 1, freq, 2], &self.spec).map_err(failed)?;
            let out = model
                .plan
                .run(tvec!(spec.into(), self.state.clone()))
                .map_err(failed)?;
            let view = out[0].to_plain_array_view::<f32>().map_err(failed)?;
            let cleaned = view
                .as_slice()
                .filter(|c| c.len() == freq * 2)
                .ok_or_else(|| Error::Invalid("voice isolation: an unexpected output".into()))?;
            for (b, [re, im]) in self.buf.iter_mut().zip(cleaned.as_chunks::<2>().0) {
                *b = Complex32::new(*re, *im);
            }
            for k in freq..n {
                self.buf[k] = self.buf[n - k].conj();
            }
            self.state = out[1].clone();
            model.inverse.process(&mut self.buf);
            for ((o, b), w) in self.overlap.iter_mut().zip(&self.buf).zip(&model.window) {
                *o += b.re / n as f32 * w;
            }
            let skipped = self.skip.min(hop);
            self.skip -= skipped;
            self.wet.extend_from_slice(&self.overlap[skipped..hop]);
            self.overlap.copy_within(hop.., 0);
            self.overlap[n - hop..].fill(0.0);
            used += hop;
        }
        self.input.drain(..used);
        Ok(())
    }
}

pub(crate) struct Isolator {
    model: Arc<Model>,
    amount: f32,
    channels: usize,
    voices: Vec<Voice>,
    into: Option<Resampler>,
    out_of: Option<Resampler>,
    pull: usize,
    pulled: Vec<f32>,
    at_rate: Vec<f32>,
    mixed: Vec<f32>,
    ready: Vec<f32>,
    cursor: usize,
}

impl Isolator {
    pub(crate) fn new(
        model: Arc<Model>,
        amount: f64,
        rate: u32,
        channels: u16,
        shared: bool,
    ) -> Result<Self> {
        let (into, out_of) = match rate {
            RATE => (None, None),
            r => (
                Some(Resampler::between(r, RATE, channels)?),
                Some(Resampler::between(RATE, r, channels)?),
            ),
        };
        let voices = (0..if shared { 1 } else { channels.max(1) })
            .map(|_| Voice::new(&model))
            .collect();
        let pull = (model.hop() as u64 * u64::from(rate) / u64::from(RATE)).max(1) as usize;
        Ok(Self {
            model,
            amount: (amount / 100.0).clamp(0.0, 1.0) as f32,
            channels: usize::from(channels.max(1)),
            voices,
            into,
            out_of,
            pull,
            pulled: Vec::new(),
            at_rate: Vec::new(),
            mixed: Vec::new(),
            ready: Vec::new(),
            cursor: 0,
        })
    }

    pub(crate) fn read(
        &mut self,
        n: usize,
        out: &mut Vec<f32>,
        mut pull: impl FnMut(usize, &mut Vec<f32>) -> Result<()>,
    ) -> Result<()> {
        let ch = self.channels;
        while (self.ready.len() - self.cursor) / ch < n {
            self.pulled.clear();
            pull(self.pull, &mut self.pulled)?;
            let input = match self.into.as_mut() {
                Some(r) => {
                    self.at_rate.clear();
                    r.convert_samples(&self.pulled, &mut self.at_rate)?;
                    &self.at_rate
                }
                None => &self.pulled,
            };
            for (c, voice) in self.voices.iter_mut().enumerate() {
                voice.push(&self.model, input.iter().skip(c).step_by(ch).copied())?;
            }
            let made = self.voices.iter().map(|v| v.wet.len()).min().unwrap_or(0);
            self.mixed.clear();
            let copies = ch / self.voices.len();
            for i in 0..made {
                for voice in &mut self.voices {
                    let dry = voice.dry.pop_front().unwrap_or(0.0);
                    let value = self.amount * voice.wet[i] + (1.0 - self.amount) * dry;
                    self.mixed.extend(std::iter::repeat_n(value, copies));
                }
            }
            for voice in &mut self.voices {
                voice.wet.drain(..made);
            }
            match self.out_of.as_mut() {
                Some(r) => r.convert_samples(&self.mixed, &mut self.ready)?,
                None => self.ready.extend_from_slice(&self.mixed),
            }
        }
        out.extend_from_slice(&self.ready[self.cursor..self.cursor + n * ch]);
        self.cursor += n * ch;
        if self.cursor > 1 << 16 {
            self.ready.drain(..self.cursor);
            self.cursor = 0;
        }
        Ok(())
    }
}
