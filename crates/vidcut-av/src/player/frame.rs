use std::sync::Arc;

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
}

impl Layout {
    pub const ALL: [Self; 12] = [
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

    pub fn placement(&self, (width, height): (u32, u32)) -> (u32, u32, u32, u32) {
        let (w, h) =
            vidcut_core::place::upright_fit(self.size, self.sar, self.rotation, (width, height));
        ((width - w) / 2, (height - h) / 2, w, h)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Sampling {
    #[default]
    Own,
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
    pub moving: Option<vidcut_core::render::Moving>,
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
        }
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
        self.frame.format() == format::Pixel::YUVA444P12LE
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

    pub fn placement(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        Fit::of(&self.frame, self.rotation).placement((width, height))
    }

    pub fn placed_in(&self, size: (u32, u32)) -> ((f64, f64), [f64; 4]) {
        let own = Fit::of(&self.frame, self.rotation);
        let (frame, fit) = match &self.sampling {
            Sampling::Own => (size, own),
            Sampling::Colors { frame, .. } => (*frame, own),
            Sampling::Placed { fit, .. } => (size, *fit),
        };
        let (x, y, w, h) = fit.placement(frame);
        (
            (f64::from(frame.0), f64::from(frame.1)),
            [x, y, w, h].map(f64::from),
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
                    moving: o.moving,
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

    pub(crate) fn alone_copied_down(&self) -> Result<Picture> {
        let frame = match (self.gpu(), crate::screen::held::of(&self.frame)) {
            (Some(_), _) => crate::hw::download(&self.frame)?,
            (None, Some(_)) => crate::screen::held::in_memory(&self.frame)?,
            (None, None) => crate::export::video::frame_ref(&self.frame)?,
        };
        Ok(
            Self::new(frame, self.rotation, self.decoder, self.decoding.clone())
                .with_sampling(self.sampling.clone()),
        )
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
        self.bytes + self.over.iter().map(|o| o.picture.bytes()).sum::<usize>()
    }

    pub(crate) fn layers(&self) -> Vec<(usize, Option<crate::hw::Kind>, bool)> {
        std::iter::once((self.bytes, self.gpu(), self.held()))
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
    pub crosses: Vec<bool>,
    pub scales: Vec<Option<vidcut_core::model::Sharpness>>,
    pub colors: Option<vidcut_core::model::Hdr>,
}

pub struct TitlePicture {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub frame: (u32, u32),
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
    pub placed: Vec<vidcut_core::render::Placed>,
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
}
