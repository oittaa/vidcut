use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use vidcut_core::model::{ColorSpace, Hdr, Retime, SubtitleStyle, SuperScale, Upscaler};
use vidcut_core::place::Placing;
use vidcut_core::render::{Image, Placed, Ramp, VideoPiece, WHOLE, Wipe};
use vidcut_core::time::Rational;

use super::ahead::Ahead;
use super::make::{Fitting, Maker};
use crate::decode::{Input, Reading, VideoReader};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, format, frame};
use crate::player::{Fit, Sampling};

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

type Rgb = (Option<i64>, Look, frame::Video);

#[derive(Clone, PartialEq)]
struct Look {
    size: (u32, u32),
    kernel: u32,
    placed: Vec<Placed>,
    frame: (u32, u32),
    origin: (u32, u32),
}

impl Look {
    fn fitted(size: (u32, u32), kernel: u32) -> Self {
        Self {
            size,
            kernel,
            placed: Vec::new(),
            frame: size,
            origin: (0, 0),
        }
    }

    fn plain(&self, size: (u32, u32)) -> bool {
        self.placed.is_empty() && self.size == size
    }
}

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
    sampling: Sampling,
    looks: (Option<Look>, u64),
    scaled: Vec<(PictureId, frame::Video)>,
    gpu: Option<crate::hw::GpuDecoder>,
    ahead: Option<VideoReader>,
    run: Option<Session>,
    queued: VecDeque<frame::Video>,
    turner: Option<crate::hw::Turner>,
    drawn: Option<Single>,
    keeper: Option<crate::hw::Keeper>,
    unkept: bool,
}

type ScaledPair = (frame::Video, frame::Video, (u32, u32));

const DECODE_AHEAD: f64 = 1.0;

const REVERSED_BYTES: usize = 256 << 20;
const REVERSED_MOST: i64 = 64;

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
        decoder: Option<&crate::hw::GpuDecoder>,
        reading: Reading,
        measure: bool,
        (color_space, super_scale): (ColorSpace, SuperScale),
    ) -> Result<Source> {
        let video = pictures == Pictures::Video;
        let mut reader =
            VideoReader::open_as(input, decoder.filter(|_| video), pictures.reading(reading))?;
        reader.set_color_space(color_space);
        let ahead_gpu = decoder.filter(|_| video).cloned();
        let fitting = Fitting::none();
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
            sampling: Sampling::Own,
            looks: (None, 0),
            scaled: Vec::new(),
            gpu: ahead_gpu,
            ahead: None,
            run: None,
            queued: VecDeque::new(),
            turner: None,
            drawn: None,
            keeper: None,
            unkept: false,
        };
        if pictures == Pictures::Still {
            source.advance(target)?;
        } else {
            source.seek(target)?;
            source.advance(target)?;
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
            sampling: Sampling::Own,
            looks: (None, 0),
            scaled: Vec::new(),
            gpu: ahead_gpu,
            ahead: None,
            run: None,
            queued: VecDeque::new(),
            turner: None,
            drawn: None,
            keeper: None,
            unkept: false,
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
        self.scaled.clear();
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
        self.scaled.clear();
        self.lumas.clear();
    }

    fn follow_offscreen(&mut self) {
        if self.drawn.as_ref().is_some_and(|s| !s.on_offscreen()) {
            self.drawn = None;
            self.rgb.clear();
        }
    }

    fn luma_of(&mut self, f: &frame::Video, size: (u32, u32), reading: Reading) -> Result<Vec<u8>> {
        let pts = f.pts();
        if let Some((_, l)) = self.lumas.iter().find(|(p, _)| *p == pts) {
            return Ok(l.clone());
        }
        self.follow_offscreen();
        let held = self
            .rgb
            .iter()
            .find(|(p, l, _)| *p == pts && l.plain(size))
            .map(|(_, _, rgb)| frame_ref(rgb))
            .transpose()?;
        let held = match held {
            Some(rgb) => Some(rgb),
            None if crate::hw::on_gpu(f) => Some(shown_rgb(
                &mut self.drawn,
                f,
                self.reader.rotation(),
                &Look::fitted(size, 0),
                reading,
                self.reader.path(),
            )?),
            None => None,
        };
        let l = match (held, &self.drawn) {
            (Some(rgb), Some(single)) => single.small_luma(&rgb)?,
            _ => crate::analysis::small_luma(&mut None, f)?,
        };
        if self.lumas.len() >= 3 {
            self.lumas.remove(0);
        }
        self.lumas.push((pts, l.clone()));
        Ok(l)
    }

    fn cut(&mut self, size: (u32, u32), reading: Reading) -> Result<bool> {
        let (Some((z, _)), Some((a, _)), Some((b, _))) = (&self.prev2, &self.prev, &self.current)
        else {
            return Ok(false);
        };
        let (z, a, b) = (frame_ref(z)?, frame_ref(a)?, frame_ref(b)?);
        let (lz, la, lb) = (
            self.luma_of(&z, size, reading)?,
            self.luma_of(&a, size, reading)?,
            self.luma_of(&b, size, reading)?,
        );
        let before = crate::analysis::mafd(&lz, &la);
        let score = crate::analysis::scene_score(crate::analysis::mafd(&la, &lb), before);
        Ok(f64::from(score) >= vidcut_core::analysis::SCENE_SCORE)
    }

    fn rgb_of(&mut self, prev: bool, look: &Look, reading: Reading) -> Result<frame::Video> {
        let rotation = self.reader.rotation();
        let (f, _) = match prev {
            true => self.prev.as_ref(),
            false => self.current.as_ref(),
        }
        .expect("a frame");
        let f = frame_ref(f)?;
        self.rgb_from(&f, rotation, look, reading)
    }

    fn rgb_from(
        &mut self,
        f: &frame::Video,
        rotation: u16,
        look: &Look,
        reading: Reading,
    ) -> Result<frame::Video> {
        self.follow_offscreen();
        let pts = f.pts();
        if let Some((_, _, rgb)) = self.rgb.iter().find(|(p, l, _)| *p == pts && l == look) {
            return frame_ref(rgb);
        }
        let rgb = shown_rgb(
            &mut self.drawn,
            f,
            rotation,
            look,
            reading,
            self.reader.path(),
        )?;
        self.rgb.retain(|(p, l, _)| l == look && *p != pts);
        if self.rgb.len() >= 2 {
            self.rgb.remove(0);
        }
        self.rgb.push((pts, look.clone(), frame_ref(&rgb)?));
        Ok(rgb)
    }

    fn look_id(&mut self, look: &Look) -> u64 {
        if self.looks.0.as_ref() != Some(look) {
            self.looks = (Some(look.clone()), self.looks.1 + 1);
        }
        self.looks.1
    }

    fn note_upscaled(&mut self, upscaler: Upscaler) {
        if self.told {
            return;
        }
        crate::progress::note(super::Note::Upscaled(super::UpscaleNote {
            path: self.reader.path().to_path_buf(),
            how: crate::super_scale::name(upscaler).into(),
        }));
        self.told = true;
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
    pub(crate) sampling: Sampling,
    pub(crate) decoder: Option<crate::hw::Kind>,
    pub(crate) decoding: Option<std::sync::Arc<crate::decode::Decoding>>,
    pub(crate) went: Option<crate::route::Went>,
    pub(crate) wipe: Option<Wipe>,
    pub(crate) share: f64,
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
        match made_picture(o.picture, base.made, o.pts) {
            Ok(picture) => waiting.push((key, picture)),
            Err(e) => crate::debug!("ai", "{}: {e}", crate::super_scale::name(base.upscaler)),
        }
    }
}

fn made_picture(
    picture: crate::super_scale::Picture,
    made: (u32, u32),
    pts: Option<i64>,
) -> Result<frame::Video> {
    match picture {
        crate::super_scale::Picture::Held(held) => crate::screen::held::picture(held, made, pts),
        crate::super_scale::Picture::Rgb(rgb) => Ok(big_picture(&rgb, made, pts)),
        crate::super_scale::Picture::Frame(f) => Ok(f),
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
    drawn: &mut Option<Single>,
    frame: &frame::Video,
    rotation: u16,
    look: &Look,
    reading: Reading,
    path: &Path,
) -> Result<frame::Video> {
    let single = match drawn {
        Some(single) => single,
        None => drawn.insert(Single::new(reading)?),
    };
    match look.placed.is_empty() {
        true => single.held(frame, rotation, look.size, look.kernel, path),
        false => single.placed(frame, rotation, look, path),
    }
}

fn big_picture(out: &[u8], (w, h): (u32, u32), pts: Option<i64>) -> frame::Video {
    let mut big = crate::buffers::unpacked(format::Pixel::RGB24, (w, h), 3, out);
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
        prev: bool,
        target: (u32, u32),
        reading: Reading,
        upscaler: Upscaler,
    ) -> Result<Option<frame::Video>> {
        let rotation = self.reader.rotation();
        let (frame, _) = match prev {
            true => self.prev.as_ref(),
            false => self.current.as_ref(),
        }
        .expect("have a frame");
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
        if let Some((_, p)) = self.scaled.iter().find(|(k, _)| *k == id) {
            return frame_ref(p).map(Some);
        }
        let p = self.ai_made(&id, size, reading)?;
        if !self.retime.is_nearest() {
            if self.scaled.len() >= 2 {
                self.scaled.remove(0);
            }
            self.scaled.push((id, frame_ref(&p)?));
        }
        Ok(Some(p))
    }

    fn ai_made(
        &mut self,
        id: &PictureId,
        size: (u32, u32),
        reading: Reading,
    ) -> Result<frame::Video> {
        if let Some(p) = self.taken(id)? {
            return Ok(p);
        }
        if self.run.as_ref().is_some_and(|r| !r.covers(id, size)) {
            self.run = None;
        }
        if self.run.is_some() {
            crate::super_scale::settle(id.upscaler);
            if let Some(p) = self.taken(id)? {
                return Ok(p);
            }
        }
        let made = self.ai_run(id, size, reading);
        if made.is_err() {
            self.run = None;
        }
        made
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
        let clock = Instant::now();
        let work = crate::super_scale::making(id.upscaler, size, id.made, (want, want));
        if !self.run.as_ref().is_some_and(|r| r.serves(id, size)) {
            self.run = None;
            self.open_run(id, size, want, reading)?;
        }
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
                let rotation = self.reader.rotation();
                let takes = |f: &frame::Video| run.run.takes(f, rotation);
                match self.reader.next_frame()? {
                    Some(f) if takes(&f) => {
                        self.queued.push_back(frame_ref(&f)?);
                        Some(f)
                    }
                    Some(f) => {
                        let kept = self.kept(&f)?;
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
        self.follow_offscreen();
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
        let rgb = match run.run.takes(f, rotation) {
            true => None,
            false => Some(shown_rgb(
                &mut self.drawn,
                f,
                rotation,
                &Look::fitted(run.size, 0),
                reading,
                self.reader.path(),
            )?),
        };
        let converted = clock.elapsed();
        let outs = match rgb {
            Some(rgb) => run.run.push(i, f.pts(), &rgb)?,
            None => run.run.push_frame(i, f, rotation)?,
        };
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

    fn kept(&mut self, f: &frame::Video) -> Result<frame::Video> {
        if !crate::hw::on_gpu(f) {
            return frame_ref(f);
        }
        if !self.unkept {
            match crate::hw::Keeper::keep(&mut self.keeper, f) {
                Ok(kept) => return Ok(kept),
                Err(e) => {
                    eprintln!("vidcut: GPU pictures come down for the upscaler: {e}");
                    self.unkept = true;
                    self.keeper = None;
                }
            }
        }
        in_memory(f)
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
        let rotation = self.reader.rotation();
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
        let kept = match run.run.takes(f, rotation) {
            true => frame_ref(f)?,
            false => self.kept(f)?,
        };
        self.run.as_mut().expect("opened").missed.push(kept);
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

fn fitted_size(fit: Fit, aim: (u32, u32)) -> (u32, u32) {
    let (_, _, w, h) = fit.placement(aim);
    let even = |v: u32| (v.div_ceil(2) * 2).max(2);
    (even(w), even(h))
}

fn placed_look(placed: &[Placed], fit: Fit, frame: (u32, u32), kernel: u32) -> Option<Look> {
    let (x, y, w, h) = fit.placement(frame);
    let whole = (f64::from(frame.0), f64::from(frame.1));
    let placing = Placing::new(placed, whole, [x, y, w, h].map(f64::from));
    let [x0, y0, x1, y1] = placing.bounds(whole)?;
    let (x0, y0) = (x0 / 2 * 2, y0 / 2 * 2);
    let (x1, y1) = (
        (x1.div_ceil(2) * 2).min(frame.0),
        (y1.div_ceil(2) * 2).min(frame.1),
    );
    (x1 > x0 && y1 > y0).then(|| Look {
        size: (x1 - x0, y1 - y0),
        kernel,
        placed: placed.to_vec(),
        frame,
        origin: (x0, y0),
    })
}

fn reach(super_scale: SuperScale) -> f64 {
    if super_scale.on { 4.0 } else { 1.0 }
}

fn layer_size(
    out: (u32, u32),
    f: &frame::Video,
    rotation: u16,
    placed: &[Placed],
    super_scale: SuperScale,
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
        .min((native * reach(super_scale)).max(1.0))
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

fn heaviest(layers: &[Layer]) -> usize {
    let mut left = u64::from(WHOLE);
    let mut best = (0, 0);
    for (i, l) in layers.iter().enumerate().rev() {
        let w = (f64::from(l.weight.unwrap_or(WHOLE)) * l.share) as u64;
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
    last_went: Option<crate::route::Went>,
    gpu_decoded: i64,
    dynamic: crate::hdr::Dynamic,
    light:
        std::collections::HashMap<std::path::PathBuf, std::sync::Arc<crate::light::LightAnalysis>>,
    last_dolby_vision: frame::Video,
    sources: Vec<super::SourceNote>,
    out: (format::Pixel, Option<Hdr>),
    timeline: Option<Hdr>,
    titles: crate::player::Titles,
    subtitles: crate::player::Subtitles,
    drawer: Option<Drawer>,
    drawn: i64,
    encoder: Option<Handoff>,
}

pub(crate) enum Handoff {
    Cuda(crate::cuda::CudaDevice, crate::cuda::CudaFrames),
    Surfaces(crate::hw::Frames),
}

impl Handoff {
    pub(crate) fn of(
        kind: vidcut_core::export::EncoderChoice,
        cuda: Option<(crate::cuda::CudaDevice, crate::cuda::CudaFrames)>,
        upload: Option<&super::encode::Upload>,
    ) -> Result<Option<Self>> {
        Ok(match (cuda, upload) {
            (Some((device, frames)), _) => Some(Self::Cuda(device, frames)),
            (None, Some(up)) if kind == vidcut_core::export::EncoderChoice::VideoToolbox => {
                // SAFETY: `up.frames` is the encoder's live frames reference.
                Some(Self::Surfaces(unsafe { crate::hw::Frames::of(up.frames)? }))
            }
            _ => None,
        })
    }
}

pub(crate) struct Drawer {
    gpu: &'static crate::screen::Gpu,
    screen: crate::screen::Screen,
    encoder: Option<Handoff>,
    through_memory: Option<String>,
}

impl Drawer {
    pub(crate) fn new(encoder: Option<Handoff>) -> Result<Self> {
        Ok(Self {
            gpu: &crate::screen::offscreen().map_err(Error::Invalid)?.gpu,
            screen: crate::screen::Screen::for_export(),
            encoder,
            through_memory: None,
        })
    }

    pub(crate) fn keeps(&self) -> Vec<crate::hw::Kind> {
        self.gpu.gpu_pictures()
    }

    pub(crate) fn through_memory(&self) -> Option<&str> {
        self.through_memory.as_deref()
    }

    pub(crate) fn frame(
        &mut self,
        picture: &std::sync::Arc<crate::player::Picture>,
        titles: &[crate::player::TitleOver],
        layers: crate::screen::Layers<'_>,
        (w, h): (u32, u32),
        (pixel, colors): (format::Pixel, (Option<Hdr>, Option<Hdr>)),
    ) -> Result<frame::Video> {
        let (gpu, screen) = (self.gpu, &mut self.screen);
        screen.draw(gpu, Some(picture), titles, layers, w, h);
        let handed =
            match (&self.encoder, &self.through_memory) {
                (Some(Handoff::Cuda(device, frames)), None) => Some(screen.encoded(
                    gpu,
                    pixel,
                    colors,
                    crate::screen::Sink::Cuda(device, frames),
                )),
                (Some(Handoff::Surfaces(frames)), None) => Some(frames.fresh().and_then(|f| {
                    screen.encoded(gpu, pixel, colors, crate::screen::Sink::Surface(f))
                })),
                _ => None,
            };
        match handed {
            Some(Ok(out)) => return Ok(out),
            Some(Err(e)) => self.through_memory = Some(e.to_string()),
            None => {}
        }
        let out = screen.encoded(gpu, pixel, colors, crate::screen::Sink::Memory)?;
        match &self.encoder {
            Some(Handoff::Cuda(_, frames)) => {
                let mut up = frames.upload(&out)?;
                crate::color::tag_frame(&mut up, colors.1);
                Ok(up)
            }
            _ => Ok(out),
        }
    }
}

pub(crate) struct Single {
    drawer: Drawer,
    maker: Maker,
    fitting: Fitting,
}

impl Single {
    pub(crate) fn new(reading: Reading) -> Result<Self> {
        let drawer = Drawer::new(None)?;
        let maker = Maker::new((2, 2, format::Pixel::YUV420P), drawer.keeps(), reading).holding();
        Ok(Self {
            drawer,
            maker,
            fitting: Fitting::none(),
        })
    }

    pub(crate) fn draw(
        &mut self,
        frame: &frame::Video,
        rotation: u16,
        size: (u32, u32),
        pixel: format::Pixel,
        path: &Path,
    ) -> Result<frame::Video> {
        let made = self.maker.make(&mut self.fitting, frame, path)?;
        let picture = std::sync::Arc::new(crate::player::Picture::new(made, rotation, None, None));
        let layers = (&[][..], &[][..], &[][..], None);
        self.drawer
            .frame(&picture, &[], layers, size, (pixel, (None, None)))
    }

    fn on_offscreen(&self) -> bool {
        crate::screen::offscreen().is_ok_and(|o| std::ptr::eq(self.drawer.gpu, &o.gpu))
    }

    pub(crate) fn small_luma(&self, picture: &frame::Video) -> Result<Vec<u8>> {
        crate::screen::held::small_luma(self.drawer.gpu, picture, crate::analysis::MEASURE)
    }

    pub(crate) fn held(
        &mut self,
        frame: &frame::Video,
        rotation: u16,
        (w, h): (u32, u32),
        kernel: u32,
        path: &Path,
    ) -> Result<frame::Video> {
        let made = self.maker.make(&mut self.fitting, frame, path)?;
        let picture = std::sync::Arc::new(crate::player::Picture::new(made, rotation, None, None));
        let (gpu, screen) = (self.drawer.gpu, &mut self.drawer.screen);
        screen.draw(gpu, Some(&picture), &[], (&[], &[], &[kernel], None), w, h);
        screen.encoded(
            gpu,
            format::Pixel::RGB24,
            (None, None),
            crate::screen::Sink::Held,
        )
    }

    fn placed(
        &mut self,
        frame: &frame::Video,
        rotation: u16,
        look: &Look,
        path: &Path,
    ) -> Result<frame::Video> {
        let made = self.maker.make(&mut self.fitting, frame, path)?;
        let sampling = Sampling::Colors {
            frame: look.frame,
            origin: look.origin,
        };
        let picture =
            crate::player::Picture::new(made, rotation, None, None).with_sampling(sampling);
        let picture = std::sync::Arc::new(picture);
        let placed = [look.placed.clone()];
        let (gpu, screen) = (self.drawer.gpu, &mut self.drawer.screen);
        let (w, h) = look.size;
        screen.draw(
            gpu,
            Some(&picture),
            &[],
            (&placed, &[], &[look.kernel], None),
            w,
            h,
        );
        screen.encoded(
            gpu,
            format::Pixel::RGB24,
            (None, None),
            crate::screen::Sink::Held,
        )
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
                (width, height, format::Pixel::YUV420P),
                Vec::new(),
                Reading::Stream,
            ),
            gpu_made: 0,
            rotation: 0,
            last_decoder: None,
            last_decoding: None,
            last_went: None,
            gpu_decoded: 0,
            dynamic: Default::default(),
            light: Default::default(),
            last_dolby_vision: frame::Video::empty(),
            sources: Vec::new(),
            out: (pixel, target),
            timeline: None,
            titles: Default::default(),
            subtitles: Default::default(),
            drawer: None,
            drawn: 0,
            encoder: None,
        }
    }

    pub(crate) fn encode_on(mut self, encoder: Option<Handoff>) -> Self {
        self.encoder = encoder;
        self
    }

    pub fn through_memory(&self) -> Option<&str> {
        self.drawer.as_ref().and_then(Drawer::through_memory)
    }

    pub fn with_timeline(mut self, timeline: Option<Hdr>) -> Result<Self> {
        self.timeline = timeline;
        Ok(self)
    }

    pub fn with_burn(self, cues: Vec<super::subtitles::Cue>, style: SubtitleStyle) -> Self {
        self.subtitles.burned(&cues, &style);
        self
    }

    pub fn sources(&self) -> &[super::SourceNote] {
        &self.sources
    }

    fn read_from(
        &mut self,
        path: &std::path::Path,
        decoding: &crate::decode::Decoding,
        went: Option<crate::route::Went>,
    ) {
        let source = super::SourceNote {
            path: path.to_path_buf(),
            decoding: decoding.clone(),
            went: went.and_then(crate::route::Went::label),
        };
        match self.sources.iter_mut().find(|s| s.path == source.path) {
            Some(s) if *s == source => return,
            Some(s) => *s = source.clone(),
            None => self.sources.push(source.clone()),
        }
        crate::progress::note(super::Note::Source(source));
    }

    pub(crate) fn keeping(mut self, keep: Vec<crate::hw::Kind>) -> Self {
        self.maker = Maker::new(self.maker.out(), keep, self.maker.reading());
        self
    }

    pub fn single_frames(mut self) -> Self {
        self.maker.read_as(Reading::Frames);
        self
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

    pub fn gpu_decoded(&self) -> i64 {
        self.gpu_decoded
    }

    pub fn fps(&self) -> Rational {
        self.fps
    }

    pub fn next_frame(&mut self) -> Result<Option<frame::Video>> {
        if self.drawer.is_none() {
            let drawer = Drawer::new(self.encoder.take())?;
            let (out, reading) = (self.maker.out(), self.maker.reading());
            self.maker = Maker::new(out, drawer.keeps(), reading).holding();
            self.drawer = Some(drawer);
        }
        let Some(layers) = self.next_layers()? else {
            return Ok(None);
        };
        let k = (self.k - 1).max(0);
        let piece = self.pieces.get(self.piece).cloned();
        let (w, h, _) = self.maker.out();
        let j = self.drawn;
        self.drawn += 1;
        let at = piece
            .as_ref()
            .map(|p| crate::player::titles_of(p, k))
            .unwrap_or_default();
        self.titles.draw_at(&at, (w, h));
        let (mut titles, _) = self.titles.shown_at(&at, (w, h), &[]);
        let fps = self.fps.as_f64();
        self.subtitles.draw_at(fps, (w, h), j);
        titles.extend(self.subtitles.shown_at(fps, (w, h), j).0);
        let placed = piece
            .as_ref()
            .map(|p| crate::player::placed_of(p, k, &[]))
            .unwrap_or_default();
        let crosses = piece.as_ref().map(VideoPiece::crosses).unwrap_or_default();
        let kernels: Vec<u32> = piece
            .as_ref()
            .map(crate::player::scales_of)
            .unwrap_or_default()
            .into_iter()
            .map(crate::screen::kernel)
            .collect();
        let shown = heaviest(&layers);
        let like = layers[shown].frame.as_ref().map(frame_ref).transpose()?;
        let light = layers
            .iter()
            .filter_map(|l| l.frame.as_ref())
            .filter_map(crate::hdr::Hdr10Plus::of_frame)
            .reduce(|a, b| a.brightest(&b));
        let picture = std::sync::Arc::new(crate::player::picture(layers));
        let drawer = self.drawer.as_mut().expect("made above");
        let mut out = drawer.frame(
            &picture,
            &titles,
            (&placed, &crosses, &kernels, self.timeline),
            (w, h),
            (self.out.0, (self.timeline, self.out.1)),
        )?;
        self.gpu_made += 1;
        if self.dynamic.dolby_vision {
            let dv = crate::hdr::Dynamic {
                hdr10plus: false,
                dolby_vision: true,
            };
            let from = like.as_ref().unwrap_or(&self.last_dolby_vision);
            crate::hdr::copy_dynamic(from, &mut out, dv)?;
        }
        if self.dynamic.hdr10plus {
            match light {
                Some(light) => light.attach(&mut out)?,
                None => crate::light::FrameLight::default()
                    .hdr10plus()
                    .attach(&mut out)?,
            }
        }
        Ok(Some(out))
    }

    pub(crate) fn next_layers(&mut self) -> Result<Option<Vec<Layer>>> {
        let Some(layers) = self.next(true)? else {
            return Ok(None);
        };
        let layers = layers.expect("a picture was asked for");
        let heaviest = heaviest(&layers);
        let shown = &layers[heaviest];
        let read = shown
            .decoding
            .as_ref()
            .filter(|d| {
                shown.went != self.last_went
                    || self
                        .last_decoding
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
            self.read_from(&path, &d, shown.went);
        }
        self.last_went = shown.went;
        self.rotation = shown.rotation;
        self.last_decoder = shown.decoder;
        self.last_decoding = shown.decoding.clone();
        self.gpu_decoded += i64::from(shown.decoder.is_some());
        Ok(Some(layers))
    }

    pub fn skip_frame(&mut self) -> Result<bool> {
        let skipped = self.next(false)?.is_some();
        self.drawn += i64::from(skipped);
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
            let files = layers
                .iter()
                .any(|(l, _)| matches!(l, VideoPiece::Source { .. }));
            if k == 0 && files {
                self.start_layers(&piece)?;
            }
            if k == 1 && files {
                self.open_next();
            }
            let placed = crate::player::placed_of(&piece, k, &[]);
            let mut made = Vec::with_capacity(layers.len());
            for (i, (layer, ramp)) in layers.into_iter().enumerate() {
                let weight = ramp.map(|r| r.weight(k));
                let wipe = mask_of(ramp, k);
                let VideoPiece::Source {
                    image,
                    pace,
                    retime,
                    ..
                } = layer
                else {
                    if picture {
                        made.push(Layer {
                            frame: Some(self.black_frame()?),
                            weight,
                            rotation: 0,
                            sampling: Sampling::Own,
                            decoder: None,
                            decoding: None,
                            went: None,
                            wipe,
                            share: 1.0,
                        });
                    }
                    continue;
                };
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
                let frame = if backward {
                    self.reversed_frame(i, s, layer, k)?
                } else if retimed {
                    let placed = placed.get(i).map(Vec::as_slice).unwrap_or_default();
                    self.retimed_frame(i, s, layer, k, placed)?
                } else {
                    self.source_frame(i, s, layer, k)?
                };
                let sampling = match backward {
                    true => Sampling::Own,
                    false => self.open[s].sampling.clone(),
                };
                made.push(Layer {
                    frame: Some(frame),
                    weight,
                    rotation: if self.open[s].upright { 0 } else { rotation },
                    sampling,
                    decoder,
                    decoding: Some(decoding),
                    went: self.open[s].fitting.went(),
                    wipe,
                    share: self.share_of(s, rotation, placed.get(i)),
                });
            }
            return Ok(Some(picture.then_some(made)));
        }
    }

    fn share_of(&self, s: usize, rotation: u16, placed: Option<&Vec<Placed>>) -> f64 {
        match (&self.open[s].current, placed.filter(|p| !p.is_empty())) {
            (Some((f, _)), Some(placed)) => {
                let (w, h, _) = self.maker.out();
                let size = (f64::from(w), f64::from(h));
                let at = vidcut_core::place::fitted((w, h), upright_fit(f, rotation, (w, h)));
                Placing::new(placed, size, at).share(size)
            }
            _ => 1.0,
        }
    }

    fn black_frame(&mut self) -> Result<frame::Video> {
        let mut out = frame_ref(self.black()?)?;
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
            crate::color::tag_frame(&mut f, None);
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
        let (w, h) = match self.open[s].current.as_ref() {
            Some((f, _)) => (f.width(), f.height()),
            None => (self.maker.out().0, self.maker.out().1),
        };
        let bytes = (w as usize * h as usize * 3).max(1);
        ((REVERSED_BYTES / bytes) as i64).clamp(1, REVERSED_MOST)
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
        placed: &[Placed],
    ) -> Result<frame::Video> {
        let VideoPiece::Source {
            retime,
            super_scale,
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
        let (w, h, _) = self.maker.out();
        let reading = self.open[s].pictures.reading(self.maker.reading());
        let scaled = self.scaled_pair(s, layer, reading)?;
        let kernel =
            crate::screen::kernel((!super_scale.is_off()).then_some(super_scale.sharpness));
        let start = Instant::now();
        let src = &mut self.open[s];
        let rotation = src.reader.rotation();
        let (cur, _) = src.current.as_ref().expect("a frame");
        let (fit, measured, aim) = match &scaled {
            Some((_, b, aim)) => (Fit::of(b, 0), between_size(b, 0, *aim), *aim),
            None => (
                Fit::of(cur, rotation),
                between_size(cur, rotation, (w, h)),
                (w, h),
            ),
        };
        let look = match placed.is_empty() {
            true => Some(Look::fitted(fitted_size(fit, aim), kernel)),
            false => placed_look(placed, fit, (w, h), kernel),
        };
        let Some(look) = look else {
            return self.shown(i, s, layer, changed);
        };
        let size = look.size;
        let pair = match (&scaled, &src.prev, &src.current) {
            _ if !look.placed.is_empty() => None,
            (Some((a, b, _)), ..) if crate::interpolate::takes_frames(*retime, a, b) => {
                Some((frame_ref(a)?, frame_ref(b)?))
            }
            (None, Some((a, _)), Some((b, _)))
                if crate::interpolate::takes_frames(*retime, a, b) =>
            {
                let turner = &mut src.turner;
                Some((
                    crate::hw::Turner::turn(turner, a, rotation)?,
                    crate::hw::Turner::turn(turner, b, rotation)?,
                ))
            }
            _ => None,
        };
        let frames = pair.is_some();
        let (a, b) = match (pair, &scaled) {
            (Some(pair), _) => pair,
            (None, Some((a, b, _))) => (
                src.rgb_from(a, 0, &look, reading)?,
                src.rgb_from(b, 0, &look, reading)?,
            ),
            (None, None) => (
                src.rgb_of(true, &look, reading)?,
                src.rgb_of(false, &look, reading)?,
            ),
        };
        let pts = |f: &Option<(frame::Video, f64)>| f.as_ref().and_then(|(f, _)| f.pts());
        let (pa, pb) = (pts(&src.prev), pts(&src.current));
        let key = (
            src.reader.path().to_path_buf(),
            src.look_id(&look),
            pa.unwrap_or(0),
            pb.unwrap_or(0),
        );
        let clock = Instant::now();
        let converted = clock - start;
        let big = if src.cut(measured, reading)? {
            crate::debug!(
                "interpolate",
                "{}: a cut between the frames at {weight:.3}",
                crate::interpolate::name(*retime)
            );
            crate::interpolate::kept_at_cut(*retime, size, weight);
            let mut picture = frame_ref(if weight < 0.5 { &a } else { &b })?;
            picture.set_pts(pb);
            picture
        } else {
            let made = match frames {
                true => crate::interpolate::between_frames(&key, &a, &b, size, weight)?,
                false => {
                    let made = crate::interpolate::between(*retime, &key, &a, &b, size, weight)?;
                    made_picture(made, size, pb)?
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
        let src = &mut self.open[s];
        let made = self
            .maker
            .make_layer(i, &mut src.fitting, &big, src.reader.path(), reading)?;
        src.normalized = None;
        src.upright = true;
        src.sampling = match look.placed.is_empty() {
            true => Sampling::Own,
            false => {
                let (fw, fh) = (f64::from(w), f64::from(h));
                let (x, y) = look.origin;
                Sampling::Placed {
                    fit,
                    part: [
                        f64::from(x) / fw,
                        f64::from(y) / fh,
                        f64::from(size.0) / fw,
                        f64::from(size.1) / fh,
                    ],
                    placed: look.placed.into(),
                }
            }
        };
        let out = self.finished_layer(s, made)?;
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
        let aim = self.aim(s, placed, super_scale);
        let src = &mut self.open[s];
        if changed || src.normalized.is_none() {
            let reading = src.pictures.reading(self.maker.reading());
            let big = if super_scale.ai() {
                src.ai_picture(false, aim, reading, super_scale.upscaler)?
            } else {
                None
            };
            let (frame, _) = src.current.as_ref().expect("have a frame");
            let clock = Instant::now();
            let made = match &big {
                Some(big) => {
                    self.maker
                        .make_layer(i, &mut src.fitting, big, src.reader.path(), reading)?
                }
                None => {
                    self.maker
                        .make_layer(i, &mut src.fitting, frame, src.reader.path(), reading)?
                }
            };
            if big.is_some() {
                crate::debug!(
                    "ai",
                    "{}: fitted in {:.1} ms",
                    crate::super_scale::name(super_scale.upscaler),
                    clock.elapsed().as_secs_f64() * 1e3
                );
            }
            src.normalized = Some(made);
            src.upright = big.is_some();
            src.sampling = Sampling::Own;
            if big.is_some() {
                src.note_upscaled(super_scale.upscaler);
            }
        }
        let picture = frame_ref(src.normalized.as_ref().expect("normalized"))?;
        self.finished_layer(s, picture)
    }

    fn aim(&self, s: usize, placed: &[Placed], super_scale: SuperScale) -> (u32, u32) {
        let (w, h, _) = self.maker.out();
        let src = &self.open[s];
        match (placed.is_empty(), &src.current) {
            (false, Some((frame, _))) => {
                layer_size((w, h), frame, src.reader.rotation(), placed, super_scale)
            }
            _ => (w, h),
        }
    }

    fn scaled_pair(
        &mut self,
        s: usize,
        layer: &VideoPiece,
        reading: Reading,
    ) -> Result<Option<ScaledPair>> {
        let VideoPiece::Source {
            placed,
            super_scale,
            ..
        } = layer
        else {
            return Ok(None);
        };
        if !super_scale.ai() {
            return Ok(None);
        }
        let aim = self.aim(s, placed, *super_scale);
        let src = &mut self.open[s];
        let Some(a) = src.ai_picture(true, aim, reading, super_scale.upscaler)? else {
            return Ok(None);
        };
        let Some(b) = src.ai_picture(false, aim, reading, super_scale.upscaler)? else {
            return Ok(None);
        };
        src.note_upscaled(super_scale.upscaler);
        Ok(Some((a, b, aim)))
    }

    fn finished_layer(&mut self, s: usize, mut out: frame::Video) -> Result<frame::Video> {
        let src = &mut self.open[s];
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
