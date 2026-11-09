use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use vidcut_core::model::{AlphaMode, Retime, Upscaler};

use crate::color::{Colors, colors_of};
use crate::decode::Decoding;
use crate::error::Result;
use crate::ffmpeg::{self, format, frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Yuv420p,
    Nv12,
    P010,
    Yuv420p10,
    Yuv422p,
    Yuv422p10,
    P210,
    Yuv444p12,
    P416,
    Ayuv64,
    Rgb,
    Rgb48,
    Gbrp16,
    Gbrpf16,
    Gbrpf32,
    Rgbaf32,
}

impl Layout {
    pub const ALL: [Self; 16] = [
        Self::Yuv420p,
        Self::Nv12,
        Self::P010,
        Self::Yuv420p10,
        Self::Yuv422p,
        Self::Yuv422p10,
        Self::P210,
        Self::Yuv444p12,
        Self::P416,
        Self::Ayuv64,
        Self::Rgb,
        Self::Rgb48,
        Self::Gbrp16,
        Self::Gbrpf16,
        Self::Gbrpf32,
        Self::Rgbaf32,
    ];

    pub(crate) fn of(p: format::Pixel) -> Self {
        use format::Pixel as P;
        match p {
            P::NV12 => Self::Nv12,
            P::P010LE => Self::P010,
            P::YUV420P10LE => Self::Yuv420p10,
            P::YUV422P | P::YUVJ422P => Self::Yuv422p,
            P::YUV422P10LE => Self::Yuv422p10,
            P::P210LE => Self::P210,
            P::YUV444P12LE | P::YUVA444P12LE => Self::Yuv444p12,
            P::P416LE => Self::P416,
            P::AYUV64LE => Self::Ayuv64,
            P::RGB24 => Self::Rgb,
            P::RGB48LE => Self::Rgb48,
            P::GBRP16LE | P::GBRAP16LE => Self::Gbrp16,
            P::GBRPF16LE | P::GBRAPF16LE => Self::Gbrpf16,
            P::GBRPF32LE | P::GBRAPF32LE => Self::Gbrpf32,
            P::RGBAF32LE => Self::Rgbaf32,
            _ => Self::Yuv420p,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub size: (u32, u32),
    pub sar: (i32, i32),
    pub rotation: u16,
}

impl Fit {
    pub fn of(f: &frame::Video, rotation: u16) -> Self {
        let sar = f.aspect_ratio();
        Self {
            size: (f.width(), f.height()),
            sar: (sar.numerator(), sar.denominator()),
            rotation,
        }
    }

    pub fn placement(&self, frame: (u32, u32)) -> [f64; 4] {
        vidcut_core::place::fitted(self.size, self.sar, self.rotation, frame)
    }

    pub fn pixels(&self) -> (f64, f64) {
        vidcut_core::place::upright_pixels(self.size, self.rotation)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Sampling {
    #[default]
    Own,
    Rounded,
    Colors {
        frame: (u32, u32),
        origin: (u32, u32),
    },
    Placed {
        fit: Fit,
        part: [f64; 4],
        placed: Arc<[vidcut_core::render::Placed]>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AlphaUse {
    #[default]
    Shown,
    Multiplied,
    Matte,
    TakenOut,
}

pub struct Picture {
    frame: frame::Video,
    rotation: u16,
    bytes: usize,
    pooled: Option<usize>,
    decoder: Option<super::Decoder>,
    decoding: Option<Arc<Decoding>>,
    over: Vec<Over>,
    sampling: Sampling,
    woven: Option<Woven>,
    origin: Option<Origin>,
    alpha_mode: AlphaMode,
    alpha_use: AlphaUse,
    matte: Option<Arc<Picture>>,
    lut: Option<crate::screen::Lut>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Made {
    Solid([u8; 3]),
    Generated,
    Still,
    Source(Option<i64>),
    Between {
        retime: Retime,
        frames: [Option<i64>; 2],
        at: f64,
        took: Duration,
    },
    Nearer {
        retime: Retime,
        frame: Option<i64>,
        at: f64,
    },
    Smooth {
        retime: Retime,
        at: f64,
        took: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Upscaled {
    pub upscaler: Upscaler,
    pub sizes: Option<[(u32, u32); 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    pub made: Made,
    pub upscaled: Option<Upscaled>,
    pub when: Instant,
    pub took: Duration,
    pub cached: Option<Arc<Cached>>,
}

pub type Making = (Made, Option<Upscaled>);

#[derive(Debug, Clone, PartialEq)]
pub struct Cached {
    pub written: u64,
    pub layers: Vec<Option<Making>>,
}

#[derive(Clone)]
pub struct Woven {
    pub top_first: bool,
    pub before: Option<Arc<Picture>>,
    pub after: Option<Arc<Picture>>,
}

impl Woven {
    fn pictures(&self) -> impl Iterator<Item = &Arc<Picture>> {
        self.before.iter().chain(&self.after)
    }
}

#[derive(Clone)]
pub struct Over {
    pub picture: Arc<Picture>,
    pub weight: u32,
    pub wipe: Option<vidcut_core::render::Wipe>,
}

impl Over {
    pub fn level(&self) -> f32 {
        (f64::from(self.weight) / f64::from(vidcut_core::render::WHOLE)) as f32
    }
}

impl Picture {
    pub(crate) fn new(
        frame: frame::Video,
        rotation: u16,
        decoder: Option<super::Decoder>,
        decoding: Option<Arc<Decoding>>,
    ) -> Self {
        let bytes = bytes_of(&frame);
        let pooled = crate::hw::pooled(&frame);
        Self {
            frame,
            rotation,
            bytes,
            pooled,
            decoder,
            decoding,
            over: Vec::new(),
            sampling: Sampling::Own,
            woven: None,
            origin: None,
            alpha_mode: AlphaMode::Straight,
            alpha_use: AlphaUse::Shown,
            matte: None,
            lut: None,
        }
    }

    pub(crate) fn with_alpha_mode(mut self, mode: AlphaMode) -> Self {
        self.alpha_mode = mode;
        self
    }

    pub fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    pub(crate) fn with_alpha_use(mut self, alpha_use: AlphaUse) -> Self {
        self.alpha_use = alpha_use;
        self
    }

    pub(crate) fn alpha_use(&self) -> AlphaUse {
        self.alpha_use
    }

    pub(crate) fn with_matte(mut self, matte: Option<frame::Video>) -> Self {
        self.matte = matte.map(|m| Arc::new(Self::new(m, 0, None, None)));
        self
    }

    pub(crate) fn matte(&self) -> Option<&Arc<Picture>> {
        self.matte.as_ref()
    }

    pub(crate) fn with_lut(mut self, lut: Option<crate::screen::Lut>) -> Self {
        self.lut = lut;
        self
    }

    pub(crate) fn lut(&self) -> Option<&crate::screen::Lut> {
        self.lut.as_ref()
    }

    pub(crate) fn with_origin(mut self, origin: Option<Origin>) -> Self {
        self.origin = origin;
        self
    }

    pub fn origin(&self) -> Option<&Origin> {
        self.origin.as_ref()
    }

    pub fn took(&self) -> Duration {
        std::iter::once(self.origin())
            .chain(self.over.iter().map(|o| o.picture.origin()))
            .flatten()
            .map(|o| o.took)
            .sum()
    }

    pub(crate) fn with_woven(mut self, woven: Woven) -> Self {
        self.woven = Some(woven);
        self
    }

    pub fn woven(&self) -> Option<&Woven> {
        self.woven.as_ref()
    }

    fn uses(&self) -> impl Iterator<Item = &Arc<Picture>> {
        self.over
            .iter()
            .map(|o| &o.picture)
            .chain(self.woven.iter().flat_map(Woven::pictures))
            .chain(self.matte.iter())
    }

    pub(crate) fn with_over(mut self, over: Vec<Over>) -> Self {
        self.over = over;
        self
    }

    pub(crate) fn with_sampling(mut self, sampling: Sampling) -> Self {
        self.sampling = sampling;
        self
    }

    pub fn sampling(&self) -> &Sampling {
        &self.sampling
    }

    pub fn over(&self) -> &[Over] {
        &self.over
    }

    pub fn decoding(&self) -> Option<&Arc<Decoding>> {
        self.decoding.as_ref()
    }

    pub fn decoder(&self) -> Option<super::Decoder> {
        self.decoder
    }

    pub fn width(&self) -> u32 {
        self.frame.width()
    }

    pub fn height(&self) -> u32 {
        self.frame.height()
    }

    pub fn gpu(&self) -> Option<crate::hw::Kind> {
        crate::hw::Kind::of_frame(&self.frame)
    }

    pub fn alpha_plane(&self) -> bool {
        matches!(
            crate::hw::sw_format(&self.frame),
            format::Pixel::YUVA444P12LE
                | format::Pixel::GBRAP16LE
                | format::Pixel::GBRAPF16LE
                | format::Pixel::GBRAPF32LE
        )
    }

    pub(crate) fn in_video_memory(&self) -> Option<crate::hw::Kind> {
        self.gpu()
            .or_else(|| self.uses().find_map(|p| p.in_video_memory()))
    }

    pub fn held(&self) -> bool {
        crate::screen::held::of(&self.frame).is_some()
    }

    pub(crate) fn held_on_gpu(&self) -> bool {
        self.held() || self.uses().any(|p| p.held_on_gpu())
    }

    pub fn layout(&self) -> Layout {
        Layout::of(crate::hw::sw_format(&self.frame))
    }

    pub fn colors(&self) -> Colors {
        colors_of(&self.frame)
    }

    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    pub fn placement(&self, width: u32, height: u32) -> [f64; 4] {
        let fit = Fit::of(&self.frame, self.rotation);
        match self.sampling {
            Sampling::Rounded => {
                vidcut_core::place::rounded_place(fit.size, fit.sar, fit.rotation, (width, height))
            }
            _ => fit.placement((width, height)),
        }
    }

    pub fn placed_in(&self, size: (u32, u32)) -> ((f64, f64), [f64; 4], (f64, f64)) {
        let own = Fit::of(&self.frame, self.rotation);
        let (frame, fit) = match &self.sampling {
            Sampling::Own | Sampling::Rounded => (size, own),
            Sampling::Colors { frame, .. } => (*frame, own),
            Sampling::Placed { fit, .. } => (size, *fit),
        };
        (
            (f64::from(frame.0), f64::from(frame.1)),
            fit.placement(frame),
            fit.pixels(),
        )
    }

    pub fn frame(&self) -> &frame::Video {
        &self.frame
    }

    pub fn copied_down(&self) -> Result<Picture> {
        let alone = self.alone_copied_down()?;
        let over = self
            .over
            .iter()
            .map(|o| {
                Ok(Over {
                    picture: Arc::new(o.picture.copied_down()?),
                    weight: o.weight,
                    wipe: o.wipe,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let alone = alone.with_over(over);
        let Some(w) = &self.woven else {
            return Ok(alone);
        };
        let down = |p: &Option<Arc<Picture>>| {
            p.as_ref()
                .map(|p| p.alone_copied_down().map(Arc::new))
                .transpose()
        };
        Ok(alone.with_woven(Woven {
            top_first: w.top_first,
            before: down(&w.before)?,
            after: down(&w.after)?,
        }))
    }

    pub(crate) fn alone(&self) -> Result<Picture> {
        let frame = crate::export::video::frame_ref(&self.frame)?;
        let alone = Self::new(frame, self.rotation, self.decoder, self.decoding.clone())
            .with_sampling(self.sampling.clone())
            .with_origin(self.origin.clone())
            .with_alpha_mode(self.alpha_mode)
            .with_alpha_use(self.alpha_use)
            .with_lut(self.lut.clone());
        let alone = Self {
            matte: self.matte.clone(),
            ..alone
        };
        Ok(match &self.woven {
            Some(w) => alone.with_woven(w.clone()),
            None => alone,
        })
    }

    pub(crate) fn alone_copied_down(&self) -> Result<Picture> {
        let frame = match (self.gpu(), crate::screen::held::of(&self.frame)) {
            (Some(_), _) => crate::hw::download(&self.frame)?,
            (None, Some(_)) => crate::screen::held::in_memory(&self.frame)?,
            (None, None) => crate::export::video::frame_ref(&self.frame)?,
        };
        let matte = self
            .matte
            .as_ref()
            .map(|m| m.alone_copied_down().map(Arc::new))
            .transpose()?;
        let alone = Self::new(frame, self.rotation, self.decoder, self.decoding.clone())
            .with_sampling(self.sampling.clone())
            .with_origin(self.origin.clone())
            .with_alpha_mode(self.alpha_mode)
            .with_alpha_use(self.alpha_use)
            .with_lut(self.lut.clone());
        Ok(Self { matte, ..alone })
    }

    pub(crate) fn pools(&self) -> Vec<usize> {
        let mut pools: Vec<usize> = self
            .pooled
            .into_iter()
            .chain(self.uses().flat_map(|p| p.pools()))
            .collect();
        pools.sort_unstable();
        pools.dedup();
        pools
    }

    pub(crate) fn holds_a_pool(&self) -> bool {
        self.pooled.is_some() || self.uses().any(|p| p.holds_a_pool())
    }

    #[cfg(test)]
    pub(crate) fn in_pool(mut self, pool: usize) -> Self {
        self.pooled = Some(pool);
        self
    }

    pub fn bytes(&self) -> usize {
        self.bytes
            + self.matte.as_ref().map_or(0, |m| m.bytes())
            + self.over.iter().map(|o| o.picture.bytes()).sum::<usize>()
    }

    pub(crate) fn layers(&self) -> Vec<(usize, Option<crate::hw::Kind>, bool)> {
        std::iter::once((self.bytes, self.gpu(), self.held()))
            .chain(self.matte.iter().flat_map(|m| m.layers()))
            .chain(self.over.iter().flat_map(|o| o.picture.layers()))
            .collect()
    }
}

fn bytes_of(frame: &frame::Video) -> usize {
    if let Some(held) = crate::screen::held::of(frame) {
        return held.buffer().size() as usize;
    }
    // SAFETY: reads the frame's own buffer reference.
    let held = unsafe {
        let buf = (*frame.as_ptr()).buf[0];
        if buf.is_null() { 0 } else { (*buf).size }
    };
    if frame.format() == format::Pixel::CUDA && held > 0 {
        return held;
    }
    if let Some(n) = crate::hw::surface_bytes(frame) {
        return n;
    }
    let (w, h) = (frame.width() as i32, frame.height() as i32);
    // SAFETY: a size computation on plain numbers.
    let n = unsafe {
        ffmpeg::ffi::av_image_get_buffer_size(
            ffmpeg::ffi::AVPixelFormat::from(crate::hw::sw_format(frame)),
            w,
            h,
            1,
        )
    };
    if n > 0 {
        n as usize
    } else {
        (w.max(1) as usize * h.max(1) as usize * 3).div_ceil(2)
    }
}

#[derive(Clone)]
pub struct Shown {
    pub frame: i64,
    pub picture: Arc<Picture>,
    pub exact: bool,
    pub titles: Vec<TitleOver>,
    pub placed: Vec<Vec<vidcut_core::render::Placed>>,
    pub scales: Vec<Option<vidcut_core::model::Sharpness>>,
    pub colors: Option<vidcut_core::model::Hdr>,
    pub output_lut: Option<crate::screen::Lut>,
    pub size: (u32, u32),
}

pub struct TitlePicture {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub lift: f32,
    pub frame: (u32, u32),
    pub float: bool,
    pub rgba: Vec<u8>,
}

#[derive(Clone)]
pub struct TitleOver {
    pub above: usize,
    pub picture: Arc<TitlePicture>,
    pub dx: i32,
    pub dy: i32,
    pub turn: [f32; 4],
    pub alpha: f32,
    pub wipe: Option<vidcut_core::render::Wipe>,
    pub placed: Vec<vidcut_core::render::Placed>,
    pub nth: usize,
}

#[cfg(test)]
mod tests {

    use super::{Layout, Picture};
    use crate::ffmpeg::{ffi, format::Pixel, frame};

    #[test]
    fn a_picture_in_cuda_memory_takes_what_its_buffer_holds() {
        let mut f = frame::Video::empty();
        f.set_format(Pixel::CUDA);
        f.set_width(960);
        f.set_height(540);
        // SAFETY: a fresh frame given a buffer of its own.
        unsafe { (*f.as_mut_ptr()).buf[0] = ffi::av_buffer_alloc(1_658_880) };
        assert_eq!(Picture::new(f, 0, None, None).bytes(), 1_658_880);
    }

    #[test]
    fn rgb48le_pictures_are_16_bit_rgb() {
        assert_eq!(Layout::of(Pixel::RGB48LE), Layout::Rgb48);
    }

    #[test]
    fn yuv420p10le_pictures_are_10_bit_planar_4_2_0() {
        assert_eq!(Layout::of(Pixel::YUV420P10LE), Layout::Yuv420p10);
    }

    #[test]
    fn yuv422p_pictures_are_8_bit_planar_4_2_2() {
        assert_eq!(Layout::of(Pixel::YUV422P), Layout::Yuv422p);
    }

    #[test]
    fn full_range_yuvj422p_pictures_are_8_bit_planar_4_2_2() {
        assert_eq!(Layout::of(Pixel::YUVJ422P), Layout::Yuv422p);
    }

    #[test]
    fn yuv422p10le_pictures_are_10_bit_planar_4_2_2() {
        assert_eq!(Layout::of(Pixel::YUV422P10LE), Layout::Yuv422p10);
    }

    #[test]
    fn p210le_pictures_are_10_bit_interleaved_4_2_2() {
        assert_eq!(Layout::of(Pixel::P210LE), Layout::P210);
    }

    #[test]
    fn yuv444p12le_pictures_are_12_bit_planar_4_4_4() {
        assert_eq!(Layout::of(Pixel::YUV444P12LE), Layout::Yuv444p12);
    }

    #[test]
    fn yuva444p12le_pictures_are_12_bit_planar_4_4_4_without_their_alpha() {
        assert_eq!(Layout::of(Pixel::YUVA444P12LE), Layout::Yuv444p12);
    }

    #[test]
    fn p416le_pictures_are_16_bit_interleaved_4_4_4() {
        assert_eq!(Layout::of(Pixel::P416LE), Layout::P416);
    }

    #[test]
    fn gbrpf16le_pictures_are_planar_half_floats() {
        assert_eq!(Layout::of(Pixel::GBRPF16LE), Layout::Gbrpf16);
    }

    #[test]
    fn gbrapf32le_pictures_are_planar_floats() {
        assert_eq!(Layout::of(Pixel::GBRAPF32LE), Layout::Gbrpf32);
    }

    #[test]
    fn a_gbrapf32le_picture_has_an_alpha_plane() {
        let picture = Picture::new(frame::Video::new(Pixel::GBRAPF32LE, 2, 2), 0, None, None);
        assert!(picture.alpha_plane());
    }
}
