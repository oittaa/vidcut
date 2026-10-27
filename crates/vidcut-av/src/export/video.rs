use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use vidcut_core::model::{
    ColorSpace, CompositeMode, Hdr, Retime, ScaleFactor, Sharpness, SubtitleStyle, SuperScale,
    Title, Upscaler,
};
use vidcut_core::place::Placing;
use vidcut_core::render::{Image, Placed, Ramp, ShownTitle, VideoPiece, WHOLE, Wipe};
use vidcut_core::time::Rational;

use super::ahead::Ahead;
use super::make::{Fitting, Maker, Output};
use super::place::Canvas;
use crate::decode::{Input, Reading, VideoReader};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, format, frame};
use crate::route::{Place, Route};

pub(crate) fn target(fps: Rational, origin: f64, pos: i64) -> f64 {
    origin + fps.frames_to_secs(pos) - 0.5 * fps.frames_to_secs(1)
}

pub(crate) fn input_of(path: &Path, image: Option<&Image>) -> Input {
    match image {
        Some(Image::Sequence {
            pattern,
            first,
            rate,
            ..
        }) => Input::Sequence {
            pattern: pattern.clone(),
            first: *first,
            rate: (rate.num() as i32, rate.den() as i32),
        },
        Some(Image::Still) | None => Input::File(path.to_path_buf()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pictures {
    Video,
    Still,
    Sequence { count: u64 },
}

impl Pictures {
    fn of(image: Option<&Image>) -> Self {
        match image {
            Some(Image::Still) => Self::Still,
            Some(Image::Sequence { count, .. }) => Self::Sequence { count: *count },
            None => Self::Video,
        }
    }

    fn reading(self, reading: Reading) -> Reading {
        match self {
            Self::Still => Reading::Frames,
            Self::Video | Self::Sequence { .. } => reading,
        }
    }
}

pub(crate) fn frame_ref(src: &frame::Video) -> Result<frame::Video> {
    let mut dst = frame::Video::empty();
    let ret = unsafe { ffmpeg::ffi::av_frame_ref(dst.as_mut_ptr(), src.as_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "referencing a frame".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    Ok(dst)
}

type Rgb = (Option<i64>, (u32, u32), Vec<u8>);

struct Source {
    reader: VideoReader,
    tick: f64,
    origin: f64,
    source_frame: i64,
    target: f64,
    current: Option<(frame::Video, f64)>,
    prev: Option<(frame::Video, f64)>,
    prev2: Option<(frame::Video, f64)>,
    lumas: Vec<(Option<i64>, Vec<u8>)>,
    retime: Retime,
    retimer: Option<crate::interpolate::User>,
    rgb: Vec<Rgb>,
    dynamic: frame::Video,
    meter: Option<crate::light::Meter>,
    light: Option<crate::light::FrameLight>,
    normalized: Option<frame::Video>,
    pictures: Pictures,
    fitting: Fitting,
    decoder: Option<crate::hw::Kind>,
    reversed: Vec<Reversed>,
    left: Option<Instant>,
    scale: SuperScale,
    ai: Option<crate::super_scale::User>,
    told: bool,
    upright: bool,
    gpu: Option<crate::hw::GpuDecoder>,
    ahead: Option<VideoReader>,
    run: Option<Session>,
    queued: VecDeque<frame::Video>,
}

#[derive(Clone)]
pub(crate) struct GpuSide {
    device: crate::cuda::CudaDevice,
    out: (u32, u32),
    format: format::Pixel,
    target: Option<Hdr>,
}

const DECODE_AHEAD: f64 = 1.0;

const REVERSED_BYTES: usize = 256 << 20;
const REVERSED_MOST: i64 = 64;
const REVERSED_ON_GPU: i64 = 4;

const LEFT_FOR: Duration = Duration::from_secs(10);

const LEFT_MOST: usize = 4;
const LEFT_ON_GPU: usize = 2;

fn staying(left: &[(Duration, bool)]) -> Vec<bool> {
    let mut newest: Vec<usize> = (0..left.len()).collect();
    newest.sort_by_key(|&i| left[i].0);
    let mut stays = vec![false; left.len()];
    let (mut kept, mut on_gpu) = (0, 0);
    for i in newest {
        let (age, gpu) = left[i];
        if age >= LEFT_FOR || kept == LEFT_MOST || (gpu && on_gpu == LEFT_ON_GPU) {
            continue;
        }
        stays[i] = true;
        kept += 1;
        on_gpu += usize::from(gpu);
    }
    stays
}

struct Reversed {
    input: Input,
    pos: i64,
    frame: frame::Video,
    dolby_vision: frame::Video,
}

impl Source {
    #[allow(clippy::too_many_arguments)]
    fn open(
        input: &Input,
        origin: f64,
        source_frame: i64,
        target: f64,
        pictures: Pictures,
        gpu: Option<&GpuSide>,
        decoder: Option<&crate::hw::GpuDecoder>,
        reading: Reading,
        measure: bool,
        (color_space, super_scale): (ColorSpace, SuperScale),
    ) -> Result<Source> {
        let video = pictures == Pictures::Video;
        let sharp = sharpness(super_scale);
        let nvdec = gpu.filter(|_| video);
        let nvdec = nvdec.and_then(|g| g.device.with_own_stream().ok());
        let nvdec = nvdec.map(crate::hw::GpuDecoder::nvdec).transpose()?;
        let mut reader = VideoReader::open_as(
            input,
            nvdec.as_ref().or(decoder).filter(|_| video),
            pictures.reading(reading),
        )?;
        reader.set_color_space(color_space);
        let ahead_gpu = decoder.filter(|_| video).cloned();
        let fitting = match gpu {
            Some(g) => {
                Fitting::on_gpu(&reader, g.out, g.format, g.target, true, Place::Cuda, sharp)
            }
            None => Fitting::none(),
        };
        let decoder = reader.gpu();
        let tb = reader.time_base();
        let mut source = Source {
            reader,
            tick: f64::from(tb.numerator()) / f64::from(tb.denominator()),
            origin,
            source_frame,
            target,
            current: None,
            prev: None,
            prev2: None,
            lumas: Vec::new(),
            retime: Retime::Nearest,
            retimer: None,
            rgb: Vec::new(),
            dynamic: frame::Video::empty(),
            meter: measure.then(crate::light::Meter::new),
            light: None,
            normalized: None,
            pictures,
            fitting,
            decoder,
            reversed: Vec::new(),
            left: None,
            scale: super_scale,
            ai: crate::super_scale::User::of(super_scale),
            told: false,
            upright: false,
            gpu: ahead_gpu,
            ahead: None,
            run: None,
            queued: VecDeque::new(),
        };
        if pictures == Pictures::Still {
            source.advance(target)?;
        } else {
            source.seek(target)?;
            source.advance(target)?;
        }
        if source.fitting.scaler.is_some()
            && !super_scale.ai()
            && let Some((frame, _)) = source.current.as_ref()
            && crate::hw::on_gpu(frame)
        {
            source.normalized = Some(source.fitting.scale(frame)?.ok_or_else(no_picture)?);
            source.fitting.took(Route::Gpu, source.reader.path());
        }
        Ok(source)
    }

    fn bare(
        input: &Input,
        origin: f64,
        pictures: Pictures,
        decoder: Option<&crate::hw::GpuDecoder>,
        reading: Reading,
        measure: bool,
        color_space: ColorSpace,
    ) -> Result<Source> {
        let mut reader = VideoReader::open_as(input, decoder, pictures.reading(reading))?;
        reader.set_color_space(color_space);
        let ahead_gpu = decoder.cloned();
        let decoder = reader.gpu();
        let tb = reader.time_base();
        Ok(Source {
            reader,
            tick: f64::from(tb.numerator()) / f64::from(tb.denominator()),
            origin,
            source_frame: 0,
            target: 0.0,
            current: None,
            prev: None,
            prev2: None,
            lumas: Vec::new(),
            retime: Retime::Nearest,
            retimer: None,
            rgb: Vec::new(),
            dynamic: frame::Video::empty(),
            meter: measure.then(crate::light::Meter::new),
            light: None,
            normalized: None,
            pictures,
            fitting: Fitting::none(),
            decoder,
            reversed: Vec::new(),
            left: None,
            scale: SuperScale::default(),
            ai: None,
            told: false,
            upright: false,
            gpu: ahead_gpu,
            ahead: None,
            run: None,
            queued: VecDeque::new(),
        })
    }

    fn seek(&mut self, target: f64) -> Result<()> {
        self.reader.seek((target / self.tick).floor() as i64)?;
        self.queued.clear();
        self.target = target;
        self.current = None;
        self.prev = None;
        self.prev2 = None;
        self.light = None;
        self.normalized = None;
        Ok(())
    }

    fn advance(&mut self, target: f64) -> Result<bool> {
        let behind = match &self.current {
            Some(_) => target < self.target - 1e-6,
            None => self
                .reader
                .passes_before()
                .is_some_and(|p| p as f64 * self.tick > target + 1e-6),
        };
        if self.pictures != Pictures::Still && behind {
            self.seek(target)?;
        }
        self.target = target;
        let mut changed = false;
        while self
            .current
            .as_ref()
            .is_none_or(|(_, t)| *t < target - 1e-6)
        {
            let next = match self.queued.pop_front() {
                Some(f) => Some(f),
                None => self.reader.next_frame()?,
            };
            match next {
                Some(f) => {
                    self.miss(&f)?;
                    let t = f.pts().unwrap_or(0) as f64 * self.tick;
                    self.dynamic = frame::Video::empty();
                    let all = crate::hdr::Dynamic {
                        hdr10plus: true,
                        dolby_vision: true,
                    };
                    crate::hdr::copy_dynamic(&f, &mut self.dynamic, all)?;
                    if !self.retime.is_nearest() {
                        self.prev2 = self.prev.take();
                        self.prev = self.current.take();
                    }
                    self.current = Some((f, t));
                    self.light = None;
                    changed = true;
                }
                None => {
                    let shown = self.current.as_ref().map(|(f, _)| f.pts().unwrap_or(0));
                    let missing = match (shown, self.pictures) {
                        (None, _) => String::new(),
                        (Some(k), Pictures::Sequence { count }) if k + 1 < count as i64 => {
                            let first = match self.reader.input() {
                                Input::Sequence { first, .. } => *first,
                                Input::File(_) => 0,
                            };
                            format!(
                                ": picture {} of {count} (file number {}) is missing",
                                k + 1,
                                first + k as u64 + 1
                            )
                        }
                        (Some(_), _) => break,
                    };
                    return Err(Error::Invalid(format!(
                        "{} ends before {target:.3} s{missing}",
                        self.reader.path().display()
                    )));
                }
            }
        }
        if changed
            && let Some(meter) = self.meter.as_mut()
            && !crate::hdr::has_hdr10plus(&self.dynamic)
        {
            let (f, _) = self.current.as_ref().expect("have a frame");
            self.light = meter.measure(f).ok();
        }
        Ok(changed)
    }

    fn reposition(&mut self, origin: f64, source_frame: i64, target: f64) -> Result<()> {
        self.origin = origin;
        self.source_frame = source_frame;
        if !self.reads_on(target) {
            self.seek(target)?;
        }
        Ok(())
    }

    fn reads_on(&self, target: f64) -> bool {
        if self.pictures == Pictures::Still {
            return true;
        }
        match &self.current {
            Some((_, t)) => target >= self.target && target - t <= DECODE_AHEAD,
            None => self
                .reader
                .pending_pts()
                .map(|p| p as f64 * self.tick)
                .is_some_and(|t| {
                    t <= target + 1e-6
                        && (target <= self.target + 1e-6 || target - t <= DECODE_AHEAD)
                }),
        }
    }

    fn leave(&mut self, now: Instant) {
        if self.left.is_none() {
            self.left = Some(now);
            self.reversed.clear();
        }
        self.run = None;
        self.ai = None;
        self.retimer = None;
        self.rgb.clear();
    }

    fn back(&mut self) {
        self.left = None;
        if self.ai.is_none() {
            self.ai = crate::super_scale::User::of(self.scale);
        }
        if self.retimer.is_none() {
            self.retimer = crate::interpolate::User::of(self.retime);
        }
    }

    fn set_retime(&mut self, retime: Retime) {
        if self.retime == retime {
            return;
        }
        self.retime = retime;
        self.retimer = crate::interpolate::User::of(retime);
        self.prev = None;
        self.prev2 = None;
        self.rgb.clear();
        self.lumas.clear();
    }

    fn luma_of(&mut self, f: &frame::Video) -> Result<Vec<u8>> {
        let pts = f.pts();
        if let Some((_, l)) = self.lumas.iter().find(|(p, _)| *p == pts) {
            return Ok(l.clone());
        }
        let l = crate::analysis::small_luma(&mut None, f)?;
        if self.lumas.len() >= 3 {
            self.lumas.remove(0);
        }
        self.lumas.push((pts, l.clone()));
        Ok(l)
    }

    fn cut(&mut self) -> Result<bool> {
        let (Some((z, _)), Some((a, _)), Some((b, _))) = (&self.prev2, &self.prev, &self.current)
        else {
            return Ok(false);
        };
        let (z, a, b) = (frame_ref(z)?, frame_ref(a)?, frame_ref(b)?);
        let (lz, la, lb) = (self.luma_of(&z)?, self.luma_of(&a)?, self.luma_of(&b)?);
        let before = crate::analysis::mafd(&lz, &la);
        let score = crate::analysis::scene_score(crate::analysis::mafd(&la, &lb), before);
        Ok(f64::from(score) >= vidcut_core::analysis::SCENE_SCORE)
    }

    fn rgb_of(
        &mut self,
        prev: bool,
        size: (u32, u32),
        reading: Reading,
    ) -> Result<(Option<i64>, Vec<u8>)> {
        let rotation = self.reader.rotation();
        let (f, _) = match prev {
            true => self.prev.as_ref(),
            false => self.current.as_ref(),
        }
        .expect("a frame");
        let pts = f.pts();
        if let Some((_, _, rgb)) = self.rgb.iter().find(|(p, s, _)| *p == pts && *s == size) {
            return Ok((pts, rgb.clone()));
        }
        let rgb = shown_rgb(f, rotation, size, reading)?;
        self.rgb.retain(|(p, s, _)| *s == size && *p != pts);
        if self.rgb.len() >= 2 {
            self.rgb.remove(0);
        }
        self.rgb.push((pts, size, rgb.clone()));
        Ok((pts, rgb))
    }

    fn resume(&mut self, origin: f64, source_frame: i64, target: f64) -> Result<()> {
        self.back();
        self.reposition(origin, source_frame, target)?;
        if self.advance(target)? {
            self.normalized = None;
        }
        Ok(())
    }
}

pub(crate) struct Layer {
    pub(crate) frame: Option<frame::Video>,
    pub(crate) weight: Option<u32>,
    pub(crate) rotation: u16,
    pub(crate) decoder: Option<crate::hw::Kind>,
    pub(crate) decoding: Option<std::sync::Arc<crate::decode::Decoding>>,
    made_on_gpu: bool,
    placing: Option<(Placing, [f64; 4])>,
    crossing: bool,
    pub(crate) wipe: Option<Wipe>,
}

impl Layer {
    fn whole(frame: frame::Video) -> Self {
        Self {
            frame: Some(frame),
            weight: None,
            rotation: 0,
            decoder: None,
            decoding: None,
            made_on_gpu: false,
            placing: None,
            crossing: false,
            wipe: None,
        }
    }
}

fn mask_of(ramp: Option<Ramp>, k: i64) -> Option<Wipe> {
    ramp.and_then(|r| {
        r.wipe.map(|direction| Wipe {
            direction,
            along: r.rising(k),
            falling: r.falling,
        })
    })
}

fn upright_fit(f: &frame::Video, rotation: u16, out: (u32, u32)) -> (u32, u32) {
    let sar = f.aspect_ratio();
    let sar = (sar.numerator(), sar.denominator());
    vidcut_core::place::upright_fit((f.width(), f.height()), sar, rotation, out)
}

pub(crate) fn placing_of(
    source: &frame::Video,
    rotation: u16,
    layer: (u32, u32),
    out: (u32, u32),
    placed: &[Placed],
) -> (Placing, [f64; 4]) {
    let at_out = vidcut_core::place::fitted(out, upright_fit(source, rotation, out));
    let at_layer = vidcut_core::place::fitted(layer, upright_fit(source, rotation, layer));
    (
        Placing::new(placed, (f64::from(out.0), f64::from(out.1)), at_out),
        at_layer,
    )
}

const PLACED_MOST: f64 = 16384.0;

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Frame(i64),
    Still(Option<i64>),
}

#[derive(Clone, PartialEq)]
struct PictureId {
    input: Input,
    space: ColorSpace,
    reading: Reading,
    rotation: u16,
    upscaler: Upscaler,
    made: (u32, u32),
    key: Key,
}

type Waiting = std::sync::Arc<std::sync::Mutex<Vec<(Key, frame::Video)>>>;

const RUN_JUMP: i64 = 48;

struct Session {
    run: Box<dyn crate::super_scale::Run>,
    id: PictureId,
    size: (u32, u32),
    first: i64,
    last: Option<i64>,
    next: i64,
    out_next: i64,
    own: bool,
    missed: Vec<frame::Video>,
    lost: bool,
    ended: bool,
    waiting: Waiting,
    given: Option<(Key, frame::Video)>,
}

impl Session {
    fn same(&self, id: &PictureId) -> bool {
        let same = PictureId {
            key: id.key,
            ..self.id.clone()
        };
        same == *id
    }

    fn covers(&self, id: &PictureId, size: (u32, u32)) -> bool {
        if !self.same(id) || self.size != size || self.lost {
            return false;
        }
        match (id.key, self.id.key) {
            (Key::Still(a), Key::Still(b)) => a == b,
            (Key::Frame(i), Key::Frame(_)) => {
                i >= self.first && i <= self.next + RUN_JUMP && self.last.is_none_or(|l| i <= l)
            }
            _ => false,
        }
    }

    fn serves(&self, id: &PictureId, size: (u32, u32)) -> bool {
        let ahead = match id.key {
            Key::Frame(i) => i >= self.out_next,
            Key::Still(_) => true,
        };
        ahead && !self.ended && self.covers(id, size)
    }

    fn keep(&mut self, outs: Vec<crate::super_scale::Out>) {
        if let Some(o) = outs.iter().map(|o| o.index).max() {
            self.out_next = self.out_next.max(o + 1);
        }
        hold(&self.waiting, &self.id, outs);
    }

    fn take(&mut self, id: &PictureId) -> Result<Option<frame::Video>> {
        if !self.same(id) {
            return Ok(None);
        }
        if let Some((key, p)) = &self.given
            && *key == id.key
        {
            return frame_ref(p).map(Some);
        }
        let found = {
            let mut waiting = self
                .waiting
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Key::Frame(k) = id.key {
                waiting.retain(|(key, _)| !matches!(key, Key::Frame(i) if *i < k));
            }
            let at = waiting.iter().position(|(key, _)| *key == id.key);
            at.map(|i| waiting.remove(i).1)
        };
        let Some(p) = found else {
            return Ok(None);
        };
        let out = frame_ref(&p)?;
        self.given = Some((id.key, p));
        Ok(Some(out))
    }
}

fn hold(
    waiting: &std::sync::Mutex<Vec<(Key, frame::Video)>>,
    base: &PictureId,
    outs: Vec<crate::super_scale::Out>,
) {
    let mut waiting = waiting
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for o in outs {
        let key = match base.key {
            Key::Still(pts) => Key::Still(pts),
            Key::Frame(_) => Key::Frame(o.index),
        };
        waiting.retain(|(k, _)| *k != key);
        waiting.push((key, big_picture(&o.rgb, base.made, o.pts)));
    }
}

fn in_memory(frame: &frame::Video) -> Result<frame::Video> {
    if crate::hw::on_gpu(frame) {
        crate::hw::to_read(frame)
    } else {
        frame_ref(frame)
    }
}

fn shown_rgb(
    frame: &frame::Video,
    rotation: u16,
    (dw, dh): (u32, u32),
    reading: Reading,
) -> Result<Vec<u8>> {
    let mem = in_memory(frame)?;
    let mut to_rgb = super::normalize::Normalizer::new(
        &mem,
        rotation,
        dw,
        dh,
        format::Pixel::RGB24,
        None,
        reading,
    )?;
    let rgb = to_rgb.run(&mem)?;
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let stride = rgb.stride(0);
    let mut packed = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        packed.extend_from_slice(&rgb.data(0)[y * stride..y * stride + w * 3]);
    }
    Ok(packed)
}

fn big_picture(out: &[u8], (w, h): (u32, u32), pts: Option<i64>) -> frame::Video {
    let mut big = frame::Video::new(format::Pixel::RGB24, w, h);
    let (row, into) = (w as usize * 3, big.stride(0));
    for (y, line) in out.chunks_exact(row).enumerate() {
        big.data_mut(0)[y * into..y * into + row].copy_from_slice(line);
    }
    big.set_color_range(ffmpeg::util::color::Range::JPEG);
    big.set_color_space(ffmpeg::util::color::Space::RGB);
    big.set_color_primaries(ffmpeg::util::color::Primaries::BT709);
    big.set_color_transfer_characteristic(ffmpeg::util::color::TransferCharacteristic::BT709);
    // SAFETY: a plain field of its own frame.
    unsafe { (*big.as_mut_ptr()).sample_aspect_ratio = ffmpeg::ffi::AVRational { num: 1, den: 1 } };
    big.set_pts(pts);
    big
}

impl Source {
    fn frame_index(&self, pts: Option<i64>) -> Option<i64> {
        if self.pictures == Pictures::Still {
            return None;
        }
        let rate = self.reader.frame_rate()?;
        let first = self.reader.first_pts()?;
        let fps = f64::from(rate.numerator()) / f64::from(rate.denominator());
        Some((((pts? - first) as f64 * self.tick) * fps).round() as i64)
    }

    fn seek_ahead(&mut self, first: i64, reading: Reading) -> Result<()> {
        let rate = self.reader.frame_rate().expect("a frame rate");
        let fps = f64::from(rate.numerator()) / f64::from(rate.denominator());
        let start =
            self.reader.first_pts().unwrap_or(0) as f64 * self.tick + (first as f64 - 0.5) / fps;
        let input = self.reader.input().clone();
        let space = self.reader.color_space();
        if self.ahead.is_none() {
            let mut r = VideoReader::open_as(&input, self.gpu.as_ref(), reading)?;
            r.set_color_space(space);
            self.ahead = Some(r);
        }
        let tick = self.tick;
        self.ahead
            .as_mut()
            .expect("opened")
            .seek((start.max(0.0) / tick).floor() as i64)
    }

    fn ai_picture(
        &mut self,
        target: (u32, u32),
        reading: Reading,
        upscaler: Upscaler,
    ) -> Result<Option<frame::Video>> {
        let rotation = self.reader.rotation();
        let (frame, _) = self.current.as_ref().expect("have a frame");
        let sar = frame.aspect_ratio();
        let sar = (sar.numerator(), sar.denominator());
        let shown = super::normalize::shown_size(frame, rotation, target);
        if !super::normalize::enlarges((frame.width(), frame.height()), sar, 0, shown) {
            return Ok(None);
        }
        if crate::color::Source::of(frame).hdr.is_some() {
            return Err(Error::Invalid(format!(
                "{}: {} takes SDR pictures, and this one is HDR",
                self.reader.path().display(),
                crate::super_scale::name(upscaler)
            )));
        }
        let across = if sar.0 > 0 && sar.1 > 0 {
            f64::from(sar.0) / f64::from(sar.1)
        } else {
            1.0
        };
        let (dw, dh) = (f64::from(frame.width()) * across, f64::from(frame.height()));
        let (dw, dh) = if rotation % 180 == 90 {
            (dh, dw)
        } else {
            (dw, dh)
        };
        let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
        let size = (even(dw), even(dh));
        let up = if rotation % 180 == 90 {
            (shown.1, shown.0)
        } else {
            shown
        };
        let pts = frame.pts();
        let id = PictureId {
            input: self.reader.input().clone(),
            space: self.reader.color_space(),
            reading,
            rotation,
            upscaler,
            made: crate::super_scale::made_size(upscaler, size, up),
            key: match self.frame_index(pts) {
                Some(i) => Key::Frame(i),
                None => Key::Still(pts),
            },
        };
        if let Some(p) = self.taken(&id)? {
            return Ok(Some(p));
        }
        if self.run.as_ref().is_some_and(|r| !r.covers(&id, size)) {
            self.run = None;
        }
        if self.run.is_some() {
            crate::super_scale::settle(upscaler);
            if let Some(p) = self.taken(&id)? {
                return Ok(Some(p));
            }
        }
        let made = self.ai_run(&id, size, reading);
        if made.is_err() {
            self.run = None;
        }
        made.map(Some)
    }

    fn taken(&mut self, id: &PictureId) -> Result<Option<frame::Video>> {
        match self.run.as_mut() {
            Some(run) => run.take(id),
            None => Ok(None),
        }
    }

    fn ai_run(
        &mut self,
        id: &PictureId,
        size: (u32, u32),
        reading: Reading,
    ) -> Result<frame::Video> {
        let want = match id.key {
            Key::Frame(i) => i,
            Key::Still(_) => 0,
        };
        if !self.run.as_ref().is_some_and(|r| r.serves(id, size)) {
            self.run = None;
            self.open_run(id, size, want, reading)?;
        }
        let clock = Instant::now();
        let work = crate::super_scale::making(id.upscaler, size, id.made, (want, want));
        loop {
            crate::cancel::stop()?;
            if let Some(p) = self.taken(id)? {
                let last = self
                    .run
                    .as_ref()
                    .map_or(want, |r| (r.out_next - 1).max(want));
                work.finished((want, last));
                crate::debug!(
                    "ai",
                    "{}: source frame {want} in {:.0} ms",
                    crate::super_scale::name(id.upscaler),
                    clock.elapsed().as_secs_f64() * 1e3
                );
                return Ok(p);
            }
            let run = self.run.as_mut().expect("opened");
            if run.ended {
                return Err(Error::Invalid(format!(
                    "{}: {} has no source frame {want}",
                    self.reader.path().display(),
                    crate::super_scale::name(id.upscaler)
                )));
            }
            if !run.missed.is_empty() {
                let missed = std::mem::take(&mut run.missed);
                for f in &missed {
                    self.feed(f, reading)?;
                }
                continue;
            }
            let next = if run.own {
                match self.reader.next_frame()? {
                    Some(f) => {
                        let kept = in_memory(&f)?;
                        self.queued.push_back(frame_ref(&kept)?);
                        Some(kept)
                    }
                    None => None,
                }
            } else {
                self.ahead.as_mut().expect("opened").next_frame()?
            };
            match next {
                Some(f) => self.feed(&f, reading)?,
                None => self.end_run()?,
            }
        }
    }

    fn open_run(
        &mut self,
        id: &PictureId,
        size: (u32, u32),
        want: i64,
        reading: Reading,
    ) -> Result<()> {
        let first = crate::super_scale::first_frame(id.upscaler, want);
        let base = id.clone();
        let waiting = Waiting::default();
        let later = std::sync::Arc::downgrade(&waiting);
        let sink: crate::super_scale::Sink = Box::new(move |outs| match outs {
            Ok(outs) => {
                if let Some(waiting) = later.upgrade() {
                    hold(&waiting, &base, outs);
                }
            }
            Err(e) => crate::debug!("ai", "{}: {e}", crate::super_scale::name(base.upscaler)),
        });
        let run = crate::super_scale::open(id.upscaler, size, id.made, want, sink)?;
        let current = self.current.as_ref().map(|(f, _)| f.pts());
        let at = current.and_then(|pts| self.frame_index(pts));
        let still = matches!(id.key, Key::Still(_));
        let own = !still && at == Some(first) && self.reader.reading() == reading;
        self.run = Some(Session {
            run,
            id: id.clone(),
            size,
            first,
            last: crate::super_scale::last_frame(id.upscaler, first),
            next: first,
            out_next: first,
            own,
            missed: Vec::new(),
            lost: false,
            ended: false,
            waiting,
            given: None,
        });
        if still {
            let (frame, _) = self.current.as_ref().expect("have a frame");
            let f = frame_ref(frame)?;
            self.feed(&f, reading)?;
            return self.end_run();
        }
        if own {
            let mut known = vec![frame_ref(&self.current.as_ref().expect("have a frame").0)?];
            for f in &self.queued {
                known.push(frame_ref(f)?);
            }
            for f in &known {
                self.feed(f, reading)?;
            }
            Ok(())
        } else {
            self.seek_ahead(first, reading)
        }
    }

    fn feed(&mut self, f: &frame::Video, reading: Reading) -> Result<()> {
        let rotation = self.reader.rotation();
        let still = self
            .run
            .as_ref()
            .is_some_and(|r| matches!(r.id.key, Key::Still(_)));
        let at = if still {
            Some(0)
        } else {
            self.frame_index(f.pts())
        };
        let run = self.run.as_mut().expect("opened");
        let Some(i) = at.filter(|i| *i >= run.next) else {
            return Ok(());
        };
        if run.ended {
            return Ok(());
        }
        if run.last.is_some_and(|l| i > l) {
            return self.end_run();
        }
        let clock = Instant::now();
        let rgb = shown_rgb(f, rotation, run.size, reading)?;
        let converted = clock.elapsed();
        let outs = run.run.push(i, f.pts(), rgb)?;
        let pushed = clock.elapsed();
        run.next = i + 1;
        run.keep(outs);
        crate::debug!(
            "ai",
            "{}: frame {i}: to RGB in {:.1} ms, upscaler in {:.1} ms, kept in {:.1} ms",
            crate::super_scale::name(run.id.upscaler),
            converted.as_secs_f64() * 1e3,
            (pushed - converted).as_secs_f64() * 1e3,
            (clock.elapsed() - pushed).as_secs_f64() * 1e3
        );
        if run.last == Some(i) {
            return self.end_run();
        }
        Ok(())
    }

    fn end_run(&mut self) -> Result<()> {
        let run = self.run.as_mut().expect("opened");
        if run.ended {
            return Ok(());
        }
        run.ended = true;
        let outs = run.run.finish()?;
        run.keep(outs);
        Ok(())
    }

    fn miss(&mut self, f: &frame::Video) -> Result<()> {
        let at = self.frame_index(f.pts());
        let Some(run) = self.run.as_mut().filter(|r| r.own && !r.lost && !r.ended) else {
            return Ok(());
        };
        if at.is_none_or(|i| i < run.next) {
            return Ok(());
        }
        if run.missed.len() as i64 >= RUN_JUMP {
            run.missed.clear();
            run.lost = true;
            return Ok(());
        }
        run.missed.push(in_memory(f)?);
        Ok(())
    }
}

fn between_size(f: &frame::Video, rotation: u16, aim: (u32, u32)) -> (u32, u32) {
    let shown = super::normalize::shown_size(f, rotation, aim);
    let sar = f.aspect_ratio();
    let across = if sar.numerator() > 0 && sar.denominator() > 0 {
        f64::from(sar.numerator()) / f64::from(sar.denominator())
    } else {
        1.0
    };
    let (dw, dh) = (f64::from(f.width()) * across, f64::from(f.height()));
    let (dw, dh) = if rotation % 180 == 90 {
        (dh, dw)
    } else {
        (dw, dh)
    };
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if f64::from(shown.0) > dw + 0.5 || f64::from(shown.1) > dh + 0.5 {
        (even(dw), even(dh))
    } else {
        (even(f64::from(shown.0)), even(f64::from(shown.1)))
    }
}

fn sharpness(super_scale: SuperScale) -> Option<Sharpness> {
    (!super_scale.is_off()).then_some(super_scale.sharpness)
}

fn reach(factor: ScaleFactor) -> f64 {
    match factor {
        ScaleFactor::Auto => 4.0,
        f => f64::from(f.times().unwrap_or(1)),
    }
}

fn layer_size(
    out: (u32, u32),
    f: &frame::Video,
    rotation: u16,
    placed: &[Placed],
    factor: ScaleFactor,
) -> (u32, u32) {
    let (fw, fh) = upright_fit(f, rotation, out);
    let sar = f.aspect_ratio();
    let across = if sar.numerator() > 0 && sar.denominator() > 0 {
        f64::from(sar.numerator()) / f64::from(sar.denominator())
    } else {
        1.0
    };
    let (w, h) = (f64::from(f.width()) * across, f64::from(f.height()));
    let (dw, dh) = if rotation % 180 == 90 { (h, w) } else { (w, h) };
    let native = (dw / f64::from(fw.max(1))).max(dh / f64::from(fh.max(1)));
    let (ow, oh) = (f64::from(out.0), f64::from(out.1));
    let most = PLACED_MOST / ow.max(oh);
    let least = 2.0 / ow.min(oh);
    let scale = vidcut_core::place::detail(placed)
        .min((native * reach(factor)).max(1.0))
        .clamp(least, most.max(least));
    let even = |v: f64| ((v * scale / 2.0).round() as u32 * 2).max(2);
    let mut size = (even(ow), even(oh));
    for _ in 0..3 {
        let (fw, fh) = upright_fit(f, rotation, size);
        let (x, y) = ((size.0 - fw.min(size.0)) / 2, (size.1 - fh.min(size.1)) / 2);
        if x % 4 == 0 && y % 4 == 0 {
            break;
        }
        size.0 += if x % 4 == 0 { 0 } else { 2 };
        size.1 += if y % 4 == 0 { 0 } else { 2 };
    }
    size
}

const LAYERS: usize = 16;

fn slot(piece: usize, layer: usize) -> usize {
    piece * LAYERS + layer.min(LAYERS - 1)
}

fn heaviest(layers: &[Layer], (fw, fh): (u32, u32)) -> usize {
    let mut left = u64::from(WHOLE);
    let mut best = (0, 0);
    for (i, l) in layers.iter().enumerate().rev() {
        let share = l
            .placing
            .as_ref()
            .map_or(1.0, |(p, _)| p.share((f64::from(fw), f64::from(fh))));
        let w = (f64::from(l.weight.unwrap_or(WHOLE)) * share) as u64;
        let shown = left * w;
        if shown > best.1 {
            best = (i, shown);
        }
        left = left * (u64::from(WHOLE) - w) / u64::from(WHOLE);
    }
    best.0
}

pub struct VideoFrames {
    pieces: Vec<VideoPiece>,
    fps: Rational,
    piece: usize,
    k: i64,
    open: Vec<Source>,
    reading: Vec<Option<usize>>,
    ahead: Ahead<Source, (Input, u64, i64)>,
    black: Option<frame::Video>,
    decoder: Option<crate::hw::GpuDecoder>,
    maker: Maker,
    gpu_made: i64,
    rotation: u16,
    last_decoder: Option<crate::hw::Kind>,
    last_decoding: Option<std::sync::Arc<crate::decode::Decoding>>,
    gpu_decoded: i64,
    dynamic: crate::hdr::Dynamic,
    light:
        std::collections::HashMap<std::path::PathBuf, std::sync::Arc<crate::light::LightAnalysis>>,
    last_dolby_vision: frame::Video,
    cuda_mix_failed: Option<String>,
    sources: Vec<super::SourceNote>,
    burn: Option<Burner>,
    titles: Titles,
    finish: Option<Finish>,
}

const MIXING: format::Pixel = format::Pixel::YUV444P12LE;

struct Finish {
    target: Option<Hdr>,
    pixel: format::Pixel,
    output: Output,
    normalizer: Option<super::normalize::Normalizer>,
    gpu: Option<crate::cuda::CudaPictures>,
    gpu_failed: bool,
}

impl Finish {
    fn run(&mut self, f: &frame::Video, reading: Reading) -> Result<frame::Video> {
        let on_cuda = Place::of(f) == Place::Cuda;
        if let (true, Output::Cuda { device, frames }) = (on_cuda, &self.output) {
            if !self.gpu_failed {
                match converted_on_gpu(&mut self.gpu, device, f, self.pixel, self.target) {
                    Ok(out) => return Ok(out),
                    Err(e) => {
                        crate::debug!(
                            "decoding",
                            "the GPU won't convert the timeline's colors, the CPU converts them: {e}"
                        );
                        self.gpu_failed = true;
                    }
                }
            }
            let m = crate::hw::download(f)?;
            let out =
                converted_in_memory(&mut self.normalizer, &m, self.pixel, self.target, reading)?;
            return frames.upload(&out);
        }
        let m = match Place::of(f) {
            Place::Memory => frame_ref(f)?,
            _ => crate::hw::to_read(f)?,
        };
        converted_in_memory(&mut self.normalizer, &m, self.pixel, self.target, reading)
    }
}

fn converted_on_gpu(
    gpu: &mut Option<crate::cuda::CudaPictures>,
    device: &crate::cuda::CudaDevice,
    f: &frame::Video,
    pixel: format::Pixel,
    target: Option<Hdr>,
) -> Result<frame::Video> {
    if gpu.is_none() {
        *gpu = Some(crate::cuda::CudaPictures::converting(
            device, f, pixel, target,
        )?);
    }
    let g = gpu.as_mut().expect("made above");
    g.push(Some(&frame_ref(f)?))?;
    g.pull()?
        .ok_or_else(|| Error::Invalid("the GPU held back a converted frame".into()))
}

fn converted_in_memory(
    normalizer: &mut Option<super::normalize::Normalizer>,
    f: &frame::Video,
    pixel: format::Pixel,
    target: Option<Hdr>,
    reading: Reading,
) -> Result<frame::Video> {
    if normalizer.as_ref().is_none_or(|n| !n.matches(f, 0)) {
        *normalizer = Some(super::normalize::Normalizer::new(
            f,
            0,
            f.width(),
            f.height(),
            pixel,
            target,
            reading,
        )?);
    }
    normalizer.as_mut().expect("made above").run(f)
}

struct Burner {
    cues: Vec<super::subtitles::Cue>,
    style: SubtitleStyle,
    fps: f64,
    frame: i64,
    showing: Vec<(usize, Option<usize>, Option<i64>)>,
    size: (u32, u32),
    drawn: Drawn,
    gpu_failed: bool,
}

type OnGpu = Option<(crate::cuda::burn::Overlay, crate::burn::Placing)>;

#[derive(Default)]
struct Drawn {
    sprites: Vec<crate::burn::Sprite>,
    on_gpu: OnGpu,
    pops: Vec<crate::burn::Sprite>,
    pops_on_gpu: OnGpu,
}

fn overlaid(
    kept: &mut OnGpu,
    frames: &crate::cuda::CudaFrames,
    f: &frame::Video,
    sprites: &[crate::burn::Sprite],
    shift: crate::burn::Placing,
) -> Result<frame::Video> {
    if kept.as_ref().is_none_or(|(_, at)| *at != shift) {
        let overlay = crate::cuda::burn::Overlay::new(frames, f, sprites, shift)?;
        *kept = Some((overlay, shift));
    }
    crate::cuda::burn::burn(frames, f, &kept.as_ref().expect("made").0)
}

impl Drawn {
    fn of(sprites: Vec<crate::burn::Sprite>) -> Self {
        Self {
            sprites,
            ..Self::default()
        }
    }

    fn drawing(d: crate::burn::Drawing) -> Self {
        Self {
            sprites: d.lines,
            pops: d.pops,
            ..Self::default()
        }
    }

    fn pops(&mut self, pops: Vec<crate::burn::Sprite>) {
        self.pops = pops;
        self.pops_on_gpu = None;
    }

    fn over(
        &mut self,
        f: frame::Video,
        output: &Output,
        gpu_failed: &mut bool,
        what: &str,
        shift: crate::burn::Placing,
    ) -> Result<frame::Video> {
        if (self.sprites.is_empty() && self.pops.is_empty()) || shift.1 <= 0.0 {
            return Ok(f);
        }
        match (Place::of(&f), output) {
            (Place::Memory, _) => {
                let mut f = f;
                crate::burn::blend_at(&mut f, &self.sprites, shift)?;
                crate::burn::blend_at(&mut f, &self.pops, shift)?;
                Ok(f)
            }
            (Place::Cuda, Output::Cuda { frames, .. }) if !*gpu_failed => {
                match self.on_cuda(frames, &f, shift) {
                    Ok(out) => Ok(out),
                    Err(e) => {
                        crate::debug!(
                            "decoding",
                            "the GPU won't draw {what}, the CPU draws them: {e}"
                        );
                        *gpu_failed = true;
                        self.in_memory(&f, Some(frames), shift)
                    }
                }
            }
            (_, Output::Cuda { frames, .. }) => self.in_memory(&f, Some(frames), shift),
            _ => self.in_memory(&f, None, shift),
        }
    }

    fn on_cuda(
        &mut self,
        frames: &crate::cuda::CudaFrames,
        f: &frame::Video,
        shift: crate::burn::Placing,
    ) -> Result<frame::Video> {
        let lined = overlaid(&mut self.on_gpu, frames, f, &self.sprites, shift)?;
        match self.pops.is_empty() {
            true => Ok(lined),
            false => overlaid(&mut self.pops_on_gpu, frames, &lined, &self.pops, shift),
        }
    }

    fn in_memory(
        &self,
        f: &frame::Video,
        up: Option<&crate::cuda::CudaFrames>,
        shift: crate::burn::Placing,
    ) -> Result<frame::Video> {
        let mut m = crate::hw::download(f)?;
        crate::burn::blend_at(&mut m, &self.sprites, shift)?;
        crate::burn::blend_at(&mut m, &self.pops, shift)?;
        match up {
            Some(frames) => frames.upload(&m),
            None => Ok(m),
        }
    }
}

impl Burner {
    fn burn(&mut self, f: frame::Video, output: &Output) -> Result<frame::Video> {
        let j = self.frame;
        self.frame += 1;
        let shown: Vec<(usize, vidcut_core::subtitles::Shown, SubtitleStyle)> = self
            .cues
            .iter()
            .enumerate()
            .filter(|(_, c)| c.start <= j && j < c.end)
            .map(|(i, c)| {
                let style = self.style.for_cue(c.style.as_ref());
                let s = vidcut_core::subtitles::shown(
                    &c.text,
                    &c.words,
                    &c.word_styles,
                    &style,
                    j - c.start,
                    self.fps,
                    c.color.as_deref(),
                );
                (i, s, style)
            })
            .collect();
        if shown.is_empty() {
            return Ok(f);
        }
        let showing: Vec<(usize, Option<usize>, Option<i64>)> = shown
            .iter()
            .map(|(i, s, _)| (*i, s.spoken, s.step))
            .collect();
        let size = (f.width(), f.height());
        if showing != self.showing || size != self.size {
            let cues: Vec<crate::burn::Painted<'_>> = shown
                .iter()
                .map(|(i, s, style)| crate::burn::Painted {
                    shown: s,
                    place: self.cues[*i].place,
                    style,
                })
                .collect();
            let drawing = crate::burn::paint(&cues, size.0, size.1);
            let steady = |s: &[(usize, Option<usize>, Option<i64>)]| -> Vec<_> {
                s.iter()
                    .map(|&(i, w, step)| (i, w, step.is_some()))
                    .collect()
            };
            match size == self.size && steady(&showing) == steady(&self.showing) {
                true => self.drawn.pops(drawing.pops),
                false => self.drawn = Drawn::drawing(drawing),
            }
            self.showing = showing;
            self.size = size;
        }
        self.drawn
            .over(f, output, &mut self.gpu_failed, "subtitles", ((0, 0), 1.0))
    }
}

struct KeptTitle {
    title: Title,
    size: (u32, u32),
    upright: bool,
    drawn: Drawn,
    laid: Option<(Vec<Placed>, i32, Drawn)>,
    used: bool,
}

#[derive(Default)]
struct Titles {
    kept: Vec<KeptTitle>,
    gpu_failed: bool,
}

impl Titles {
    fn draw(
        &mut self,
        mut f: frame::Video,
        titles: &[ShownTitle],
        output: &Output,
    ) -> Result<frame::Video> {
        let size = (f.width(), f.height());
        for shown in titles {
            let upright = !shown.placed.is_empty();
            let at = self
                .kept
                .iter()
                .position(|k| k.size == size && k.upright == upright && k.title == shown.title);
            let at = at.unwrap_or_else(|| {
                let rotation = shown.title.rotation.filter(|_| !upright);
                let drawn = Title {
                    rotation,
                    ..shown.title.clone()
                };
                self.kept.push(KeptTitle {
                    title: shown.title.clone(),
                    size,
                    upright,
                    drawn: Drawn::of(crate::burn::title(&drawn, size.0, size.1)),
                    laid: None,
                    used: false,
                });
                self.kept.len() - 1
            });
            let k = &mut self.kept[at];
            k.used = true;
            let dy = match k.drawn.sprites.first() {
                Some(s) if shown.title.moves() => {
                    let top = shown.run.top(0, f64::from(size.1), f64::from(s.height));
                    top.round() as i32 - s.y
                }
                _ => 0,
            };
            let alpha = shown.alpha() as f32;
            if !upright {
                f = k
                    .drawn
                    .over(f, output, &mut self.gpu_failed, "titles", ((0, dy), alpha))?;
                continue;
            }
            if k.laid
                .as_ref()
                .is_none_or(|(placed, at, _)| *placed != shown.placed || *at != dy)
            {
                let sprite =
                    k.drawn.sprites.first().and_then(|s| {
                        super::place::title(s, &shown.title, &shown.placed, dy, size)
                    });
                let drawn = Drawn::of(sprite.into_iter().collect());
                k.laid = Some((shown.placed.clone(), dy, drawn));
            }
            if let Some((_, _, drawn)) = k.laid.as_mut() {
                f = drawn.over(f, output, &mut self.gpu_failed, "titles", ((0, 0), alpha))?;
            }
        }
        Ok(f)
    }

    fn shown(&mut self) {
        self.kept.retain(|k| k.used);
        for k in &mut self.kept {
            k.used = false;
        }
    }
}

impl VideoFrames {
    pub fn new(
        pieces: &[VideoPiece],
        fps: Rational,
        width: u32,
        height: u32,
        pixel: format::Pixel,
        target: Option<Hdr>,
    ) -> Self {
        Self {
            pieces: pieces.to_vec(),
            fps,
            piece: 0,
            k: 0,
            open: Vec::new(),
            reading: Vec::new(),
            ahead: Ahead::new(),
            black: None,
            decoder: None,
            maker: Maker::new(
                (width, height, pixel),
                target,
                Output::Fitted,
                Reading::Stream,
            ),
            gpu_made: 0,
            rotation: 0,
            last_decoder: None,
            last_decoding: None,
            gpu_decoded: 0,
            dynamic: Default::default(),
            light: Default::default(),
            last_dolby_vision: frame::Video::empty(),
            cuda_mix_failed: None,
            sources: Vec::new(),
            burn: None,
            titles: Titles::default(),
            finish: None,
        }
    }

    pub fn with_timeline(mut self, timeline: Option<Hdr>) -> Result<Self> {
        let target = self.maker.target();
        if timeline == target
            || matches!(self.maker.output(), Output::Decoded { .. } | Output::Vulkan)
        {
            return Ok(self);
        }
        let (w, h, pixel) = self.maker.out();
        let reading = self.maker.reading();
        let (mixing, output) = match self.maker.output() {
            Output::Cuda { device, .. } => {
                let p010 = format::Pixel::P010LE;
                let frames = crate::cuda::CudaFrames::new(device, p010, w, h)?;
                let device = device.clone();
                (p010, Output::Cuda { device, frames })
            }
            _ => (MIXING, Output::Fitted),
        };
        let maker = Maker::new((w, h, mixing), timeline, output, reading);
        let encoder = std::mem::replace(&mut self.maker, maker).into_output();
        self.finish = Some(Finish {
            target,
            pixel,
            output: encoder,
            normalizer: None,
            gpu: None,
            gpu_failed: false,
        });
        self.black = None;
        Ok(self)
    }

    fn finished(&mut self, mut f: frame::Video) -> Result<frame::Video> {
        let timeline = self.maker.target();
        let reading = self.maker.reading();
        let Some(finish) = self.finish.as_mut() else {
            return Ok(f);
        };
        crate::color::tag_frame(&mut f, timeline);
        let mut out = finish.run(&f, reading)?;
        crate::hdr::strip(&mut out);
        crate::hdr::copy_dynamic(&f, &mut out, self.dynamic)?;
        out.set_pts(f.pts());
        Ok(out)
    }

    pub fn with_burn(mut self, cues: Vec<super::subtitles::Cue>, style: SubtitleStyle) -> Self {
        self.burn = (!cues.is_empty()).then(|| Burner {
            cues,
            style,
            fps: self.fps.as_f64(),
            frame: 0,
            showing: Vec::new(),
            size: (0, 0),
            drawn: Drawn::default(),
            gpu_failed: false,
        });
        self
    }

    pub fn sources(&self) -> &[super::SourceNote] {
        &self.sources
    }

    fn read_from(&mut self, path: &std::path::Path, decoding: &crate::decode::Decoding) {
        let source = super::SourceNote {
            path: path.to_path_buf(),
            decoding: decoding.clone(),
        };
        match self.sources.iter_mut().find(|s| s.path == source.path) {
            Some(s) if *s == source => return,
            Some(s) => *s = source.clone(),
            None => self.sources.push(source.clone()),
        }
        crate::progress::note(super::Note::Source(source));
    }

    pub fn mix_failed(&self) -> Option<&str> {
        self.cuda_mix_failed.as_deref()
    }

    fn gpu_mixes(&self) -> bool {
        matches!(self.maker.output(), Output::Cuda { .. })
            && crate::hw::can_mix(Place::Cuda)
            && self.cuda_mix_failed.is_none()
    }

    pub(crate) fn output(mut self, output: Output) -> Self {
        let mut out = self.maker.out();
        if matches!(output, Output::Decoded { .. }) {
            out.2 = format::Pixel::YUV420P;
        }
        self.maker = Maker::new(out, self.maker.target(), output, self.maker.reading());
        self
    }

    pub fn single_frames(mut self) -> Self {
        self.maker.read_as(Reading::Frames);
        self
    }

    pub(crate) fn shrink_to(&mut self, size: Option<(u32, u32)>) {
        if self.maker.shrinks() != size {
            self.maker.shrink_to(size);
            for s in &mut self.open {
                s.normalized = None;
            }
        }
    }

    pub(crate) fn clear_stages(&mut self) {
        self.maker.clear_stages();
    }

    pub fn decode_on(mut self, gpu: crate::hw::GpuDecoder) -> Self {
        self.decoder = Some(gpu);
        self
    }

    pub fn decoded_on(&self) -> Option<crate::hw::Kind> {
        self.last_decoder
    }

    pub fn decoding(&self) -> Option<std::sync::Arc<crate::decode::Decoding>> {
        self.last_decoding.clone()
    }

    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    pub fn with_dynamic(mut self, dynamic: crate::hdr::Dynamic) -> Self {
        self.dynamic = dynamic;
        self
    }

    pub fn with_light(
        mut self,
        light: std::collections::HashMap<
            std::path::PathBuf,
            std::sync::Arc<crate::light::LightAnalysis>,
        >,
    ) -> Self {
        self.light = light;
        self
    }

    pub fn gpu_frames(&self) -> i64 {
        self.gpu_made
    }

    fn gpu_side(&self) -> Option<GpuSide> {
        let Output::Cuda { device, .. } = self.maker.output() else {
            return None;
        };
        let (w, h, format) = self.maker.out();
        Some(GpuSide {
            device: device.clone(),
            out: (w, h),
            format,
            target: self.maker.target(),
        })
    }

    pub fn gpu_decoded(&self) -> i64 {
        self.gpu_decoded
    }

    pub fn fps(&self) -> Rational {
        self.fps
    }

    pub fn next_frame(&mut self) -> Result<Option<frame::Video>> {
        let Some(layers) = self.next_layers()? else {
            return Ok(None);
        };
        let k = (self.k - 1).max(0);
        let titles: Vec<(usize, ShownTitle)> = self
            .pieces
            .get(self.piece)
            .map(|p| {
                p.titles()
                    .into_iter()
                    .map(|(n, t)| (n, t.after(k)))
                    .collect()
            })
            .unwrap_or_default();
        let f = self.compose(layers, &titles)?;
        self.titles.shown();
        let f = match self.burn.as_mut() {
            Some(b) => b.burn(f, self.maker.output())?,
            None => f,
        };
        self.finished(f).map(Some)
    }

    fn compose(
        &mut self,
        layers: Vec<Layer>,
        titles: &[(usize, ShownTitle)],
    ) -> Result<frame::Video> {
        let mut rest = layers.into_iter();
        let mut below: Option<Layer> = None;
        let mut taken = 0;
        let mut i = 0;
        while i < titles.len() {
            let n = titles[i].0;
            let mut group = Vec::new();
            while i < titles.len() && titles[i].0 == n {
                group.push(titles[i].1.clone());
                i += 1;
            }
            let parts: Vec<Layer> = below
                .take()
                .into_iter()
                .chain(rest.by_ref().take(n.saturating_sub(taken)))
                .collect();
            taken = n.max(taken);
            let f = self.flat(parts)?;
            let f = self.titles.draw(f, &group, self.maker.output())?;
            below = Some(Layer::whole(f));
        }
        let parts: Vec<Layer> = below.into_iter().chain(rest).collect();
        self.flat(parts)
    }

    fn flat(&mut self, layers: Vec<Layer>) -> Result<frame::Video> {
        if layers.len() != 1 {
            return self.mix(layers);
        }
        match layers.into_iter().next().and_then(|l| l.frame) {
            Some(f) => Ok(f),
            None => self.black_frame(),
        }
    }

    pub(crate) fn next_layers(&mut self) -> Result<Option<Vec<Layer>>> {
        let Some(layers) = self.next(true)? else {
            return Ok(None);
        };
        let layers = layers.expect("a picture was asked for");
        let (w, h, _) = self.maker.out();
        let heaviest = heaviest(&layers, (w, h));
        let shown = &layers[heaviest];
        let read = shown
            .decoding
            .as_ref()
            .filter(|d| {
                self.last_decoding
                    .as_ref()
                    .is_none_or(|last| !std::sync::Arc::ptr_eq(last, d))
            })
            .and_then(
                |d| match self.pieces.get(self.piece)?.layers().get(heaviest) {
                    Some((VideoPiece::Source { path, .. }, _)) => Some((path.clone(), d.clone())),
                    _ => None,
                },
            );
        if let Some((path, d)) = read {
            self.read_from(&path, &d);
        }
        self.rotation = shown.rotation;
        self.last_decoder = shown.decoder;
        self.last_decoding = shown.decoding.clone();
        self.gpu_decoded += i64::from(shown.decoder.is_some());
        let made_on_gpu = layers
            .iter()
            .all(|l| l.made_on_gpu || (l.frame.is_none() && layers.len() > 1));
        self.gpu_made += i64::from(made_on_gpu);
        Ok(Some(layers))
    }

    pub fn skip_frame(&mut self) -> Result<bool> {
        let skipped = self.next(false)?.is_some();
        if skipped && let Some(b) = self.burn.as_mut() {
            b.frame += 1;
        }
        Ok(skipped)
    }

    fn next(&mut self, picture: bool) -> Result<Option<Option<Vec<Layer>>>> {
        loop {
            let Some(piece) = self.pieces.get(self.piece) else {
                return Ok(None);
            };
            if self.k >= piece.frames() {
                self.piece += 1;
                self.k = 0;
                continue;
            }
            let k = self.k;
            self.k += 1;
            let piece = piece.clone();
            let layers = piece.layers();
            let crosses = piece.crosses();
            let files = layers
                .iter()
                .any(|(l, _)| matches!(l, VideoPiece::Source { .. }));
            if k == 0 && files {
                self.start_layers(&piece)?;
            }
            if k == 1 && files {
                self.open_next();
            }
            let mixed = layers.len() > 1;
            self.maker.set_mixing(mixed);
            let mut made = Vec::with_capacity(layers.len());
            for (i, (layer, ramp)) in layers.into_iter().enumerate() {
                let weight = ramp.map(|r| r.weight(k));
                let wipe = mask_of(ramp, k);
                let VideoPiece::Source {
                    image,
                    pace,
                    placed,
                    retime,
                    ..
                } = layer
                else {
                    if picture {
                        let decoded = matches!(self.maker.output(), Output::Decoded { .. });
                        let frame = if mixed && !decoded {
                            None
                        } else {
                            Some(self.black_frame()?)
                        };
                        made.push(Layer {
                            frame,
                            weight,
                            rotation: 0,
                            decoder: None,
                            decoding: None,
                            made_on_gpu: false,
                            placing: None,
                            crossing: crosses.get(i).copied().unwrap_or(false),
                            wipe,
                        });
                    }
                    continue;
                };
                let placed: Vec<Placed> = placed.iter().map(|p| p.after(k)).collect();
                let still = Pictures::of(image.as_ref()) == Pictures::Still;
                let backward = pace.speed < 0.0 && !still;
                let retimed = !retime.is_nearest() && !still && pace.speed > 0.0;
                let k = if still { 0 } else { k };
                let s = self.reading[i].expect("a file's picture is read");
                if !picture {
                    if retimed {
                        let at = self.exact_at(s, layer, k);
                        self.pass_to(s, at)?;
                    } else if !backward {
                        self.pass_source_frame(s, layer, k)?;
                    }
                    continue;
                }
                let (rotation, decoder, decoding) = {
                    let src = &self.open[s];
                    (
                        src.reader.rotation(),
                        src.decoder,
                        src.reader.decoding().clone(),
                    )
                };
                let mut frame = if backward {
                    self.reversed_frame(i, s, layer, k)?
                } else if retimed {
                    self.retimed_frame(i, s, layer, k)?
                } else {
                    self.source_frame(i, s, layer, k)?
                };
                if self.finish.is_some() && Place::of(&frame) == Place::Memory {
                    crate::color::tag_frame(&mut frame, self.maker.target());
                }
                let decoded = matches!(self.maker.output(), Output::Decoded { .. });
                let placing = match (&self.open[s].current, placed.is_empty() || decoded) {
                    (Some((f, _)), false) => {
                        let (w, h, _) = self.maker.out();
                        Some(placing_of(
                            f,
                            rotation,
                            (frame.width(), frame.height()),
                            (w, h),
                            &placed,
                        ))
                    }
                    _ => None,
                };
                made.push(Layer {
                    frame: Some(frame),
                    weight,
                    rotation: if self.open[s].upright { 0 } else { rotation },
                    decoder,
                    decoding: Some(decoding),
                    made_on_gpu: self.open[s].fitting.route == Some(Route::Gpu),
                    placing,
                    crossing: crosses.get(i).copied().unwrap_or(false),
                    wipe,
                });
            }
            return Ok(Some(picture.then_some(made)));
        }
    }

    fn black_frame(&mut self) -> Result<frame::Video> {
        let black = frame_ref(self.black()?)?;
        let mut out = match self.maker.output() {
            Output::Cuda { frames, .. } => frames.upload(&black)?,
            _ => black,
        };
        self.black_metadata(&mut out)?;
        Ok(out)
    }

    fn black_metadata(&self, out: &mut frame::Video) -> Result<()> {
        if self.dynamic.hdr10plus {
            crate::light::FrameLight::default()
                .hdr10plus()
                .attach(out)?;
        }
        if self.dynamic.dolby_vision {
            let dv = crate::hdr::Dynamic {
                hdr10plus: false,
                dolby_vision: true,
            };
            crate::hdr::copy_dynamic(&self.last_dolby_vision, out, dv)?;
        }
        Ok(())
    }

    fn mix(&mut self, layers: Vec<Layer>) -> Result<frame::Video> {
        if let Output::Decoded { .. } = self.maker.output() {
            return Err(Error::Invalid(
                "pictures as decoded come apart, not mixed".into(),
            ));
        }
        let (w, h, _) = self.maker.out();
        let shown = heaviest(&layers, (w, h));
        let placed = layers.iter().any(|l| l.placing.is_some() || l.crossing);
        let black = match self.maker.output() {
            Output::Cuda { .. } if placed && self.gpu_mixes() && layers[0].frame.is_none() => {
                Some(self.black_frame()?)
            }
            _ => None,
        };
        let (w, h, _) = self.maker.out();
        let wiped = layers.iter().any(|l| l.wipe.is_some());
        let blended = layers.iter().any(|l| {
            l.placing
                .as_ref()
                .is_some_and(|(p, _)| p.mode != CompositeMode::Normal)
        });
        let on_gpu = match self.maker.output() {
            Output::Cuda { frames, .. } if self.gpu_mixes() && !wiped && !blended => {
                match cuda_mix(frames, &layers, black.as_ref(), (w, h)) {
                    Ok(mixed) => Some(mixed),
                    Err(e) => {
                        crate::debug!(
                            "decoding",
                            "the GPU won't mix a dissolve's pictures, the CPU mixes them: {e}"
                        );
                        self.cuda_mix_failed = Some(e.to_string());
                        None
                    }
                }
            }
            _ => None,
        };
        let mut out = match on_gpu {
            Some(mixed) => mixed,
            None => {
                let black = frame_ref(self.black()?)?;
                let in_memory = |f: Option<&frame::Video>| -> Result<frame::Video> {
                    match f {
                        None => frame_ref(&black),
                        Some(f) if Place::of(f) == Place::Memory => frame_ref(f),
                        Some(f) => crate::hw::to_read(f),
                    }
                };
                let mut mixed = Canvas::of(&in_memory(layers[0].frame.as_ref())?)?;
                let mut below: Option<Canvas> = None;
                let mut i = 1;
                while i < layers.len() {
                    let l = &layers[i];
                    let over = in_memory(l.frame.as_ref())?;
                    if let Some(mask) = l.wipe {
                        let covered =
                            |under: &Canvas, over: &frame::Video, l: &Layer| match &l.placing {
                                Some((placing, fitted)) => super::place::over(
                                    under,
                                    &super::place::Layer {
                                        picture: over,
                                        placing,
                                        fitted: *fitted,
                                        weight: WHOLE,
                                    },
                                ),
                                None => Canvas::of(over),
                            };
                        let hold = layers
                            .get(i + 1)
                            .is_some_and(|n| n.crossing && n.wipe.is_some());
                        if hold {
                            let full = covered(&mixed, &over, l)?;
                            below = Some(std::mem::replace(&mut mixed, full));
                        } else if l.crossing {
                            let full = match &below {
                                Some(bg) => covered(bg, &over, l)?,
                                None => covered(&mixed, &over, l)?,
                            };
                            mixed = super::mix::wipe(&mixed, &full, mask)?;
                        } else {
                            let top = covered(&mixed, &over, l)?;
                            let next = super::mix::wipe(&mixed, &top, mask)?;
                            below = Some(std::mem::replace(&mut mixed, next));
                        }
                        i += 1;
                        continue;
                    }
                    let weight = l.weight.unwrap_or(WHOLE);
                    let laid = |under: &Canvas| -> Result<Canvas> {
                        match &l.placing {
                            Some((placing, fitted)) => super::place::over(
                                under,
                                &super::place::Layer {
                                    picture: &over,
                                    placing,
                                    fitted: *fitted,
                                    weight,
                                },
                            ),
                            None => super::mix::mix(under, &Canvas::of(&over)?, weight),
                        }
                    };
                    if l.crossing
                        && let Some(b) = &below
                    {
                        mixed = super::mix::crossed(&mixed, &laid(b)?, b)?;
                    } else {
                        let next = laid(&mixed)?;
                        below = Some(std::mem::replace(&mut mixed, next));
                    }
                    i += 1;
                }
                let mixed = mixed.frame();
                match self.maker.output() {
                    Output::Cuda { frames, .. } => frames.upload(&mixed)?,
                    _ => mixed,
                }
            }
        };
        let like = layers[shown]
            .frame
            .as_ref()
            .or_else(|| layers.iter().find_map(|l| l.frame.as_ref()));
        if let Some(like) = like {
            // SAFETY: both are valid frames.
            unsafe { ffmpeg::ffi::av_frame_copy_props(out.as_mut_ptr(), like.as_ptr()) };
        }
        crate::hdr::strip(&mut out);
        if self.dynamic.dolby_vision {
            let dv = crate::hdr::Dynamic {
                hdr10plus: false,
                dolby_vision: true,
            };
            let from = layers[shown]
                .frame
                .as_ref()
                .unwrap_or(&self.last_dolby_vision);
            crate::hdr::copy_dynamic(from, &mut out, dv)?;
        }
        if self.dynamic.hdr10plus {
            let light = layers
                .iter()
                .filter_map(|l| l.frame.as_ref())
                .filter_map(crate::hdr::Hdr10Plus::of_frame)
                .reduce(|a, b| a.brightest(&b));
            let black = layers.iter().all(|l| l.frame.is_none());
            match light {
                Some(light) => light.attach(&mut out)?,
                None if black => crate::light::FrameLight::default()
                    .hdr10plus()
                    .attach(&mut out)?,
                None => {}
            }
        }
        Ok(out)
    }

    pub fn forget_position(&mut self) {
        for s in self
            .open
            .iter_mut()
            .filter(|s| s.pictures != Pictures::Still)
        {
            s.current = None;
            s.normalized = None;
        }
    }

    pub fn keyframe_time(&mut self, piece: &VideoPiece) -> Result<Option<f64>> {
        let VideoPiece::Source {
            path,
            origin,
            source_frame,
            image: None,
            color_space,
            ..
        } = piece
        else {
            return Ok(None);
        };
        let input = input_of(path, None);
        let target = self.target(*origin, *source_frame);
        let s = match self.open.iter().position(|s| s.reader.input() == &input) {
            Some(s) => s,
            None => {
                let bare = Source::bare(
                    &input,
                    *origin,
                    Pictures::Video,
                    self.decoder.as_ref(),
                    self.maker.reading(),
                    self.measures(&input),
                    *color_space,
                )?;
                self.open.push(bare);
                self.open.len() - 1
            }
        };
        let source = &mut self.open[s];
        let sought = source.seek(target);
        source.reader.from_keyframe();
        let at = source.reader.pending_pts().map(|p| p as f64 * source.tick);
        sought?;
        Ok(at)
    }

    pub fn read_piece(&mut self, piece: &VideoPiece) {
        self.read_pieces(std::slice::from_ref(piece));
    }

    pub fn read_pieces(&mut self, pieces: &[VideoPiece]) {
        self.pieces = pieces.to_vec();
        self.piece = 0;
        self.k = 0;
        self.ahead.forget();
    }

    fn black(&mut self) -> Result<&frame::Video> {
        if self.black.is_none() {
            let (w, h, pix) = self.maker.out();
            let mut f = frame::Video::new(pix, w, h);
            fill_black(&mut f)?;
            crate::color::tag_frame(&mut f, self.maker.target());
            self.black = Some(f);
        }
        Ok(self.black.as_ref().expect("just set"))
    }

    fn target(&self, origin: f64, pos: i64) -> f64 {
        target(self.fps, origin, pos)
    }

    fn measures(&self, input: &Input) -> bool {
        self.dynamic.hdr10plus && !self.light.contains_key(input.path())
    }

    fn start_layers(&mut self, piece: &VideoPiece) -> Result<()> {
        let mut taken = vec![false; self.open.len()];
        let mut reading = Vec::new();
        for (i, (layer, _)) in piece.layers().into_iter().enumerate() {
            let VideoPiece::Source {
                path,
                origin,
                source_frame,
                image,
                pace,
                color_space,
                super_scale,
                retime,
                ..
            } = layer
            else {
                reading.push(None);
                continue;
            };
            let pictures = Pictures::of(image.as_ref());
            let still = pictures == Pictures::Still;
            let from = if still { 0 } else { *source_frame };
            let input = input_of(path, image.as_ref());
            let target = self.target(*origin, from);
            let free: Vec<usize> = (0..self.open.len())
                .filter(|&s| {
                    !taken[s]
                        && self.open[s].reader.input() == &input
                        && self.open[s].scale == *super_scale
                })
                .collect();
            let reads_on = free
                .iter()
                .copied()
                .find(|&s| self.open[s].reads_on(target));
            let s = match reads_on.or(free.first().copied()) {
                Some(s) => {
                    self.open[s].reader.set_color_space(*color_space);
                    if let Err(e) = self.open[s].reposition(*origin, from, target) {
                        self.open[s].current = None;
                        self.open[s].normalized = None;
                        return Err(e);
                    }
                    s
                }
                None => {
                    let reclaimed = self
                        .ahead
                        .reclaim(|k| k.0 == input)
                        .transpose()?
                        .filter(|s| s.scale == *super_scale);
                    let opened = match reclaimed {
                        Some(mut source) => {
                            source.back();
                            source.reader.set_color_space(*color_space);
                            source.reposition(*origin, from, target)?;
                            source
                        }
                        None => match self.ahead.take(slot(self.piece, i)) {
                            Some(opened) => opened?,
                            None => Source::open(
                                &input,
                                *origin,
                                from,
                                target,
                                pictures,
                                self.gpu_side().as_ref(),
                                self.decoder.as_ref(),
                                self.maker.reading(),
                                self.measures(&input),
                                (*color_space, *super_scale),
                            )?,
                        },
                    };
                    self.open.push(opened);
                    taken.push(false);
                    self.open.len() - 1
                }
            };
            taken[s] = true;
            self.open[s].set_retime(*retime);
            if !(pace.speed < 0.0 && !still) {
                self.open[s].reversed.clear();
            }
            reading.push(Some(s));
        }
        let now = Instant::now();
        for (source, taken) in self.open.iter_mut().zip(taken) {
            if taken {
                source.back();
            } else {
                source.leave(now);
            }
        }
        self.reading = reading;
        self.close_left();
        Ok(())
    }

    pub(crate) fn close_left(&mut self) {
        let now = Instant::now();
        let left: Vec<usize> = (0..self.open.len())
            .filter(|&s| self.open[s].left.is_some())
            .collect();
        let ages: Vec<(Duration, bool)> = left
            .iter()
            .map(|&s| {
                let source = &self.open[s];
                let since = source.left.expect("left");
                (now.duration_since(since), source.decoder.is_some())
            })
            .collect();
        let stays = staying(&ages);
        let mut closed = false;
        for (&s, stays) in left.iter().zip(stays).rev() {
            if !stays {
                drop(self.take_out(s));
                closed = true;
            }
        }
        if closed {
            crate::buffers::give_back();
        }
    }

    fn take_out(&mut self, s: usize) -> Source {
        for r in self.reading.iter_mut().flatten() {
            if *r > s {
                *r -= 1;
            }
        }
        self.open.remove(s)
    }

    fn open_next(&mut self) {
        let next = self
            .pieces
            .iter()
            .enumerate()
            .skip(self.piece + 1)
            .find(|(_, p)| {
                p.layers()
                    .iter()
                    .any(|(l, _)| matches!(l, VideoPiece::Source { frames, .. } if *frames > 0))
            })
            .map(|(j, p)| (j, p.clone()));
        let Some((j, piece)) = next else {
            return;
        };
        let mut used = vec![false; self.open.len()];
        for (i, (layer, _)) in piece.layers().into_iter().enumerate() {
            let VideoPiece::Source {
                path,
                origin,
                source_frame,
                image,
                color_space,
                super_scale,
                ..
            } = layer
            else {
                continue;
            };
            let (color_space, super_scale) = (*color_space, *super_scale);
            let input = input_of(path, image.as_ref());
            let free = |s: &usize| {
                !used[*s]
                    && self.open[*s].reader.input() == &input
                    && self.open[*s].scale == super_scale
            };
            let read = (0..self.open.len()).find(|s| free(s) && self.open[*s].left.is_none());
            let left = (0..self.open.len()).find(free);
            if let Some(s) = read {
                used[s] = true;
                continue;
            }
            let pictures = Pictures::of(image.as_ref());
            let from = if pictures == Pictures::Still {
                0
            } else {
                *source_frame
            };
            let origin = *origin;
            let target = self.target(origin, from);
            let gpu = self.gpu_side();
            let decoder = self.decoder.clone();
            let measure = self.measures(&input);
            let reading = self.maker.reading();
            let key = (input.clone(), origin.to_bits(), from);
            if let Some(s) = left {
                let mut source = self.take_out(s);
                self.ahead.start(slot(j, i), key, move || {
                    source.reader.set_color_space(color_space);
                    source.resume(origin, from, target)?;
                    Ok(source)
                });
                return;
            }
            self.ahead.start(slot(j, i), key, move || {
                Source::open(
                    &input,
                    origin,
                    from,
                    target,
                    pictures,
                    gpu.as_ref(),
                    decoder.as_ref(),
                    reading,
                    measure,
                    (color_space, super_scale),
                )
            });
            return;
        }
    }

    fn step(layer: &VideoPiece, k: i64) -> i64 {
        layer.position(k) - layer.position(0)
    }

    fn pass_source_frame(&mut self, s: usize, layer: &VideoPiece, k: i64) -> Result<()> {
        let first = self.open[s].source_frame;
        self.pass_at(s, first + Self::step(layer, k))
    }

    fn pass_at(&mut self, s: usize, pos: i64) -> Result<()> {
        let at = target(self.fps, self.open[s].origin, pos);
        self.pass_to(s, at)
    }

    fn pass_to(&mut self, s: usize, at: f64) -> Result<()> {
        let src = &mut self.open[s];
        if src.advance(at)? {
            src.normalized = None;
        }
        Ok(())
    }

    fn exact_at(&self, s: usize, layer: &VideoPiece, k: i64) -> f64 {
        let src = &self.open[s];
        let exact = src.source_frame as f64 + (layer.exact(k) - layer.position(0) as f64);
        src.origin + exact * self.fps.frames_to_secs(1)
    }

    fn source_frame(
        &mut self,
        i: usize,
        s: usize,
        layer: &VideoPiece,
        k: i64,
    ) -> Result<frame::Video> {
        let first = self.open[s].source_frame;
        self.frame_at(i, s, first + Self::step(layer, k), layer)
    }

    fn reversed_frame(
        &mut self,
        i: usize,
        s: usize,
        layer: &VideoPiece,
        k: i64,
    ) -> Result<frame::Video> {
        let pos = layer.position(k);
        let input = self.open[s].reader.input().clone();
        let made = |r: &Reversed| r.pos == pos && r.input == input;
        if !self.open[s].reversed.iter().any(made) {
            self.open[s].reversed.clear();
            let most = self.reversed_frames(s);
            let mut shown: Vec<i64> = Vec::new();
            for j in k..layer.frames() {
                let p = layer.position(j);
                if shown.last() != Some(&p) {
                    if shown.len() as i64 == most {
                        break;
                    }
                    shown.push(p);
                }
            }
            let low = *shown.last().expect("frame k");
            for p in low..=pos {
                if shown.contains(&p) {
                    let frame = self.frame_at(i, s, p, layer)?;
                    let dolby_vision = dolby_vision(&self.last_dolby_vision)?;
                    self.open[s].reversed.push(Reversed {
                        input: input.clone(),
                        pos: p,
                        frame,
                        dolby_vision,
                    });
                } else {
                    self.pass_at(s, p)?;
                }
            }
        }
        let r = self.open[s]
            .reversed
            .iter()
            .find(|r| made(r))
            .expect("just made");
        self.last_dolby_vision = dolby_vision(&r.dolby_vision)?;
        frame_ref(&r.frame)
    }

    fn reversed_frames(&self, s: usize) -> i64 {
        let current = self.open[s].current.as_ref();
        let (w, h) = match (self.maker.output(), current) {
            (Output::Decoded { .. }, Some((f, _))) => (f.width(), f.height()),
            _ => (self.maker.out().0, self.maker.out().1),
        };
        let bytes = (w as usize * h as usize * 3).max(1);
        let most = if matches!(
            self.maker.output(),
            Output::Cuda { .. } | Output::FittedOnGpu { .. }
        ) {
            REVERSED_ON_GPU
        } else {
            REVERSED_MOST
        };
        ((REVERSED_BYTES / bytes) as i64).clamp(1, most)
    }

    fn frame_at(
        &mut self,
        i: usize,
        s: usize,
        pos: i64,
        layer: &VideoPiece,
    ) -> Result<frame::Video> {
        let fps = self.fps;
        let src = &mut self.open[s];
        let changed = src.advance(target(fps, src.origin, pos))?;
        self.shown(i, s, layer, changed)
    }

    fn retimed_frame(
        &mut self,
        i: usize,
        s: usize,
        layer: &VideoPiece,
        k: i64,
    ) -> Result<frame::Video> {
        let VideoPiece::Source {
            placed,
            super_scale,
            retime,
            ..
        } = layer
        else {
            return self.source_frame(i, s, layer, k);
        };
        let at = self.exact_at(s, layer, k);
        let src = &mut self.open[s];
        let mut changed = src.advance(at)?;
        let later = src.current.as_ref().is_some_and(|(_, t)| *t > at + 1e-6);
        if later && src.prev.is_none() {
            src.seek((at - 1.0).max(0.0))?;
            src.advance(at)?;
            changed = true;
        }
        let weight = match (&src.prev, &src.current) {
            (Some((_, tp)), Some((_, tc))) if *tc > at + 1e-6 && *tp <= at && tc > tp => {
                Some((at - tp) / (tc - tp))
            }
            _ => None,
        };
        let hdr = src
            .current
            .as_ref()
            .is_some_and(|(f, _)| crate::color::Source::of(f).hdr.is_some());
        let Some(weight) = weight.filter(|w| *w > 1e-6 && !hdr) else {
            return self.shown(i, s, layer, changed);
        };
        let decoded = matches!(self.maker.output(), Output::Decoded { .. });
        let up = (!self.maker.mixing() || self.gpu_mixes()) && placed.is_empty();
        let (w, h, _) = self.maker.out();
        let reading = self.open[s].pictures.reading(self.maker.reading());
        let src = &mut self.open[s];
        let rotation = src.reader.rotation();
        let (cur, _) = src.current.as_ref().expect("a frame");
        let wanted = (!(placed.is_empty() || decoded))
            .then(|| layer_size((w, h), cur, rotation, placed, super_scale.factor));
        let size = between_size(cur, rotation, wanted.unwrap_or((w, h)));
        let start = Instant::now();
        let pair = match (&src.prev, &src.current) {
            (Some((a, _)), Some((b, _)))
                if crate::interpolate::takes_frames(*retime, a, b, rotation) =>
            {
                Some((frame_ref(a)?, frame_ref(b)?))
            }
            _ => None,
        };
        let pictures = match pair {
            Some(_) => None,
            None => Some((
                src.rgb_of(true, size, reading)?.1,
                src.rgb_of(false, size, reading)?.1,
            )),
        };
        let pts = |f: &Option<(frame::Video, f64)>| f.as_ref().and_then(|(f, _)| f.pts());
        let (pa, pb) = (pts(&src.prev), pts(&src.current));
        let key = (
            src.reader.path().to_path_buf(),
            pa.unwrap_or(0),
            pb.unwrap_or(0),
        );
        let clock = Instant::now();
        let converted = clock - start;
        let big = if src.cut()? {
            crate::debug!(
                "interpolate",
                "{}: a cut between the frames at {weight:.3}",
                crate::interpolate::name(*retime)
            );
            crate::interpolate::kept_at_cut(*retime, size, weight);
            let nearer = weight < 0.5;
            let rgb = match pictures {
                Some((a, b)) => {
                    if nearer {
                        a
                    } else {
                        b
                    }
                }
                None => src.rgb_of(nearer, size, reading)?.1,
            };
            big_picture(&rgb, size, pb)
        } else {
            let made = match &pair {
                Some((a, b)) => crate::interpolate::between_frames(&key, a, b, size, weight)?,
                None => {
                    let (a, b) = pictures.as_ref().expect("the pictures of the pair");
                    let rgb = crate::interpolate::between(*retime, &key, a, b, size, weight)?;
                    big_picture(&rgb, size, pb)
                }
            };
            crate::debug!(
                "interpolate",
                "{}: {}x{} at {weight:.3} in {:.1} ms",
                crate::interpolate::name(*retime),
                size.0,
                size.1,
                clock.elapsed().as_secs_f64() * 1e3
            );
            made
        };
        let between = clock.elapsed();
        self.maker.size_layer(i, wanted);
        self.maker.sharpen_layer(i, sharpness(*super_scale));
        let src = &mut self.open[s];
        let made = self
            .maker
            .make_layer(i, &mut src.fitting, &big, 0, src.reader.path(), reading)?
            .ok_or_else(no_picture)?;
        src.normalized = None;
        src.upright = true;
        let out = self.finished_layer(s, up, made)?;
        crate::debug!(
            "interpolate",
            "steps: to RGB in {:.1} ms, made in {:.1} ms, handed on in {:.1} ms",
            converted.as_secs_f64() * 1e3,
            between.as_secs_f64() * 1e3,
            (clock.elapsed() - between).as_secs_f64() * 1e3
        );
        Ok(out)
    }

    fn shown(
        &mut self,
        i: usize,
        s: usize,
        layer: &VideoPiece,
        changed: bool,
    ) -> Result<frame::Video> {
        let (placed, super_scale) = match layer {
            VideoPiece::Source {
                placed,
                super_scale,
                ..
            } => (placed.as_slice(), *super_scale),
            _ => (&[][..], SuperScale::default()),
        };
        let decoded = matches!(self.maker.output(), Output::Decoded { .. });
        let up = (!self.maker.mixing() || self.gpu_mixes()) && placed.is_empty();
        let src = &mut self.open[s];
        let (w, h, _) = self.maker.out();
        let wanted = match (&src.current, placed.is_empty() || decoded) {
            (Some((f, _)), false) => Some(layer_size(
                (w, h),
                f,
                src.reader.rotation(),
                placed,
                super_scale.factor,
            )),
            _ => None,
        };
        self.maker.size_layer(i, wanted);
        self.maker.sharpen_layer(i, sharpness(super_scale));
        let stale = !decoded
            && src
                .normalized
                .as_ref()
                .is_some_and(|n| (n.width(), n.height()) != wanted.unwrap_or((w, h)));
        if changed || stale || src.normalized.is_none() {
            let rotation = src.reader.rotation();
            let reading = src.pictures.reading(self.maker.reading());
            let big = if super_scale.ai() {
                let aim = match (placed.is_empty(), &src.current) {
                    (false, Some((frame, _))) => {
                        layer_size((w, h), frame, rotation, placed, super_scale.factor)
                    }
                    _ => (w, h),
                };
                src.ai_picture(aim, reading, super_scale.upscaler)?
            } else {
                None
            };
            let (frame, _) = src.current.as_ref().expect("have a frame");
            let clock = Instant::now();
            let made = match &big {
                Some(big) => self.maker.make_layer(
                    i,
                    &mut src.fitting,
                    big,
                    0,
                    src.reader.path(),
                    reading,
                )?,
                None => self.maker.make_layer(
                    i,
                    &mut src.fitting,
                    frame,
                    rotation,
                    src.reader.path(),
                    reading,
                )?,
            };
            if big.is_some() {
                crate::debug!(
                    "ai",
                    "{}: fitted in {:.1} ms",
                    crate::super_scale::name(super_scale.upscaler),
                    clock.elapsed().as_secs_f64() * 1e3
                );
            }
            src.normalized = Some(made.ok_or_else(no_picture)?);
            src.upright = big.is_some();
            if big.is_some() && !src.told {
                crate::progress::note(super::Note::Upscaled(super::UpscaleNote {
                    path: src.reader.path().to_path_buf(),
                    how: crate::super_scale::name(super_scale.upscaler).into(),
                }));
                src.told = true;
            }
        }
        let picture = frame_ref(src.normalized.as_ref().expect("normalized"))?;
        self.finished_layer(s, up, picture)
    }

    fn finished_layer(
        &mut self,
        s: usize,
        up: bool,
        picture: frame::Video,
    ) -> Result<frame::Video> {
        let src = &mut self.open[s];
        let mut out = match self.maker.output() {
            Output::Cuda { frames, .. } if up && Place::of(&picture) == Place::Memory => {
                frames.upload(&picture)?
            }
            _ => picture,
        };
        crate::hdr::strip(&mut out);
        if self.dynamic != crate::hdr::Dynamic::default() {
            crate::hdr::copy_dynamic(&src.dynamic, &mut out, self.dynamic)?;
        }
        if self.dynamic.hdr10plus
            && !crate::hdr::has_hdr10plus(&out)
            && let Some(l) = src.light.or_else(|| {
                self.light
                    .get(src.reader.path())
                    .and_then(|a| a.at(src.current.as_ref().expect("have a frame").1))
                    .copied()
            })
        {
            l.hdr10plus().attach(&mut out)?;
        }
        if self.dynamic.dolby_vision {
            self.last_dolby_vision = frame::Video::empty();
            let dv = crate::hdr::Dynamic {
                hdr10plus: false,
                dolby_vision: true,
            };
            crate::hdr::copy_dynamic(&src.dynamic, &mut self.last_dolby_vision, dv)?;
        }
        Ok(out)
    }
}

fn cuda_mix(
    frames: &crate::cuda::CudaFrames,
    layers: &[Layer],
    black: Option<&frame::Video>,
    size: (u32, u32),
) -> Result<frame::Video> {
    let solid = |f: Option<&frame::Video>| match (f, black) {
        (Some(f), _) | (None, Some(f)) => frame_ref(f),
        (None, None) => Err(Error::Invalid("nothing under a placed picture".into())),
    };
    let laid = |under: Option<&frame::Video>, l: &Layer| -> Result<Option<frame::Video>> {
        if let (Some((placing, fitted)), Some(f)) = (&l.placing, &l.frame) {
            let picture = match Place::of(f) {
                Place::Memory => frame_ref(f)?,
                _ => crate::hw::download(f)?,
            };
            let layer = super::place::Layer {
                picture: &picture,
                placing,
                fitted: *fitted,
                weight: l.weight.unwrap_or(WHOLE),
            };
            let overlay = crate::cuda::burn::Overlay::of(frames, &layer, size)?;
            let under = solid(under)?;
            return Ok(Some(crate::cuda::burn::burn(frames, &under, &overlay)?));
        }
        match (under, l.weight) {
            (None, None) => l.frame.as_ref().map(frame_ref).transpose(),
            (under, w) => Ok(Some(crate::cuda::mix::mix(
                frames,
                under,
                l.frame.as_ref(),
                w.unwrap_or(WHOLE),
            )?)),
        }
    };
    let mut mixed: Option<frame::Video> = None;
    let mut below: Option<Option<frame::Video>> = None;
    for l in layers {
        if l.crossing
            && let Some(b) = &below
        {
            let then = solid(laid(b.as_ref(), l)?.as_ref())?;
            let (now, b) = (solid(mixed.as_ref())?, solid(b.as_ref())?);
            mixed = Some(crate::cuda::mix::sum(frames, &now, &then, &b)?);
        } else {
            let next = laid(mixed.as_ref(), l)?;
            below = Some(std::mem::replace(&mut mixed, next));
        }
    }
    mixed.ok_or_else(|| Error::Invalid("a mix of black alone".into()))
}

fn no_picture() -> Error {
    Error::Invalid("the GPU scaler gave no picture".into())
}

fn dolby_vision(f: &frame::Video) -> Result<frame::Video> {
    let mut dv = frame::Video::empty();
    let only = crate::hdr::Dynamic {
        hdr10plus: false,
        dolby_vision: true,
    };
    crate::hdr::copy_dynamic(f, &mut dv, only)?;
    Ok(dv)
}

pub(crate) fn fill_black(f: &mut frame::Video) -> Result<()> {
    let desc = f
        .format()
        .descriptor()
        .ok_or_else(|| Error::Invalid("unknown pixel format".into()))?;
    let (depth, shift, rgb, alpha) = unsafe {
        let d = &*desc.as_ptr();
        let flags = d.flags;
        (
            d.comp[0].depth,
            d.comp[0].shift,
            flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_RGB as u64) != 0,
            flags & (ffmpeg::ffi::AV_PIX_FMT_FLAG_ALPHA as u64) != 0,
        )
    };
    let planes = f.planes();
    let packed_alpha = alpha && planes == 1;
    for plane in 0..planes {
        let is_alpha = alpha && plane == planes - 1 && planes > 1;
        let v8: u16 = if is_alpha {
            255
        } else if rgb {
            0
        } else if plane == 0 {
            16
        } else {
            128
        };
        let data = f.data_mut(plane);
        if depth > 8 {
            let v = if is_alpha {
                (1 << depth) - 1
            } else {
                v8 << (depth - 8)
            };
            for px in data.as_chunks_mut::<2>().0 {
                *px = (v << shift).to_le_bytes();
            }
        } else {
            data.fill(v8 as u8);
        }
    }
    if packed_alpha {
        // SAFETY: FFmpeg's static descriptor of the format.
        let (step, offset, a_depth) = unsafe {
            let d = &*desc.as_ptr();
            let a = d.comp[usize::from(d.nb_components) - 1];
            (a.step as usize, a.offset as usize, a.depth)
        };
        let (w, h, stride) = (f.width() as usize, f.height() as usize, f.stride(0));
        let data = f.data_mut(0);
        for y in 0..h {
            for x in 0..w {
                let at = y * stride + x * step + offset;
                if a_depth > 8 {
                    data[at..at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
                } else {
                    data[at] = 255;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{fill_black, staying};
    use crate::ffmpeg::{format::Pixel, frame};

    const fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn a_file_left_closes_ten_seconds_after() {
        assert_eq!(
            staying(&[(secs(9), false), (secs(10), false)]),
            [true, false]
        );
    }

    #[test]
    fn past_four_files_left_the_oldest_closes() {
        let left = [
            (secs(5), false),
            (secs(1), false),
            (secs(2), false),
            (secs(4), false),
            (secs(3), false),
        ];
        assert_eq!(staying(&left), [false, true, true, true, true]);
    }

    #[test]
    fn two_files_left_a_gpu_decodes_stay() {
        assert_eq!(
            staying(&[
                (secs(3), true),
                (secs(1), true),
                (secs(2), true),
                (secs(4), false)
            ]),
            [false, true, true, true]
        );
    }

    #[test]
    fn packed_rgba_black_is_opaque() {
        let mut f = frame::Video::new(Pixel::RGBA, 4, 4);
        fill_black(&mut f).unwrap();
        assert_eq!(&f.data(0)[..4], [0, 0, 0, 255]);
    }

    #[test]
    fn black_is_video_range_black_in_every_layout() {
        let sample = |pixel, plane| {
            let mut f = frame::Video::new(pixel, 16, 16);
            fill_black(&mut f).unwrap();
            let d = f.data(plane);
            u16::from_le_bytes([d[0], d[1]])
        };
        assert_eq!(sample(Pixel::YUV420P10LE, 0), 64);
        assert_eq!(sample(Pixel::YUV420P10LE, 1), 512);
        assert_eq!(sample(Pixel::P010LE, 0), 64 << 6);
        assert_eq!(sample(Pixel::P010LE, 1), 512 << 6);
        let mut f = frame::Video::new(Pixel::NV12, 16, 16);
        fill_black(&mut f).unwrap();
        assert_eq!((f.data(0)[0], f.data(1)[0]), (16, 128));
    }
}
