use std::sync::Arc;

use crate::color::{Colors, colors_of};
use crate::decode::Decoding;
use crate::error::Result;
use crate::export::normalize::Normalizer;
use crate::export::place::Canvas;
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
}

impl Layout {
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
            _ => Self::Yuv420p,
        }
    }
}

pub struct Picture {
    frame: frame::Video,
    rotation: u16,
    bytes: usize,
    pooled: Option<usize>,
    decoder: Option<super::Decoder>,
    decoding: Option<Arc<Decoding>>,
    over: Vec<Over>,
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
        }
    }

    pub(crate) fn with_over(mut self, over: Vec<Over>) -> Self {
        self.over = over;
        self
    }

    pub fn over(&self) -> &[Over] {
        &self.over
    }

    pub fn decoding(&self) -> Option<&Decoding> {
        self.decoding.as_deref()
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
            .or_else(|| self.over.iter().find_map(|o| o.picture.in_video_memory()))
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
        let sar = self.frame.aspect_ratio();
        let sar = (sar.numerator(), sar.denominator());
        let size = (self.width(), self.height());
        let (w, h) = vidcut_core::place::upright_fit(size, sar, self.rotation, (width, height));
        ((width - w) / 2, (height - h) / 2, w, h)
    }

    pub fn frame(&self) -> &frame::Video {
        &self.frame
    }

    pub fn copied_down(&self) -> Result<Picture> {
        let frame = match self.gpu() {
            None => crate::export::video::frame_ref(&self.frame)?,
            Some(_) => crate::hw::download(&self.frame)?,
        };
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
        Ok(Self::new(frame, self.rotation, self.decoder, self.decoding.clone()).with_over(over))
    }

    pub(crate) fn pools(&self) -> Vec<usize> {
        let mut pools: Vec<usize> = self
            .pooled
            .into_iter()
            .chain(self.over.iter().flat_map(|o| o.picture.pools()))
            .collect();
        pools.sort_unstable();
        pools.dedup();
        pools
    }

    pub(crate) fn holds_a_pool(&self) -> bool {
        self.pooled.is_some() || self.over.iter().any(|o| o.picture.holds_a_pool())
    }

    #[cfg(test)]
    pub(crate) fn in_pool(mut self, pool: usize) -> Self {
        self.pooled = Some(pool);
        self
    }

    pub fn bytes(&self) -> usize {
        self.bytes + self.over.iter().map(|o| o.picture.bytes()).sum::<usize>()
    }

    pub(crate) fn layers(&self) -> Vec<(usize, Option<crate::hw::Kind>)> {
        std::iter::once((self.bytes, self.gpu()))
            .chain(self.over.iter().flat_map(|o| o.picture.layers()))
            .collect()
    }

    pub fn at_wipe(
        &self,
        width: u32,
        height: u32,
        placed: &[Vec<vidcut_core::render::Placed>],
        crosses: &[bool],
        colors: Option<vidcut_core::model::Hdr>,
    ) -> Result<Option<Self>> {
        if self.over.iter().all(|o| o.wipe.is_none()) {
            return Ok(None);
        }
        let width = width.max(2) & !1;
        let height = height.max(2) & !1;
        let mut mixed = Canvas::of(&fitted(self, width, height, colors)?)?;
        let mut below: Option<Canvas> = None;
        let mut i = 0;
        while i < self.over.len() {
            let hold = self.over.get(i + 1).is_some_and(|n| n.wipe.is_some())
                && crosses.get(i + 2).copied().unwrap_or(false);
            let o = &self.over[i];
            let mine = placed.get(i + 1).map(Vec::as_slice).unwrap_or(&[][..]);
            let crossing = crosses.get(i + 1).copied().unwrap_or(false);
            if let Some(mask) = o.wipe {
                let covered = |under: &Canvas| -> Result<Canvas> {
                    if mine.is_empty() {
                        let (w, h) = under.size();
                        Canvas::of(&fitted(&o.picture, w, h, colors)?)
                    } else {
                        placed_on(under, &o.picture, mine, vidcut_core::render::WHOLE, colors)
                    }
                };
                if hold {
                    let full = covered(&mixed)?;
                    below = Some(std::mem::replace(&mut mixed, full));
                } else if crossing {
                    let full = match &below {
                        Some(bg) => covered(bg)?,
                        None => covered(&mixed)?,
                    };
                    mixed = crate::export::mix::wipe(&mixed, &full, mask)?;
                } else {
                    let top = covered(&mixed)?;
                    let next = crate::export::mix::wipe(&mixed, &top, mask)?;
                    below = Some(std::mem::replace(&mut mixed, next));
                }
                i += 1;
                continue;
            }
            let weight = o.weight;
            let laid = |under: &Canvas| -> Result<Canvas> {
                if mine.is_empty() {
                    let (w, h) = under.size();
                    let top = Canvas::of(&fitted(&o.picture, w, h, colors)?)?;
                    crate::export::mix::mix(under, &top, weight)
                } else {
                    placed_on(under, &o.picture, mine, weight, colors)
                }
            };
            if crossing && let Some(b) = &below {
                mixed = crate::export::mix::crossed(&mixed, &laid(b)?, b)?;
            } else {
                let next = laid(&mixed)?;
                below = Some(std::mem::replace(&mut mixed, next));
            }
            i += 1;
        }
        let mut mixed = mixed.frame();
        crate::color::tag_frame(&mut mixed, colors);
        Ok(Some(Self::new(mixed, 0, None, None)))
    }
}

fn placed_on(
    under: &Canvas,
    picture: &Picture,
    placed: &[vidcut_core::render::Placed],
    weight: u32,
    colors: Option<vidcut_core::model::Hdr>,
) -> Result<Canvas> {
    let shown = source_frame(picture, colors)?;
    let (placing, at) = crate::export::video::placing_of(
        picture.frame(),
        picture.rotation(),
        (shown.width(), shown.height()),
        under.size(),
        placed,
    );
    crate::export::place::over(
        under,
        &crate::export::place::Layer {
            picture: &shown,
            placing: &placing,
            fitted: at,
            weight,
        },
    )
}

fn source_frame(
    picture: &Picture,
    colors: Option<vidcut_core::model::Hdr>,
) -> Result<frame::Video> {
    let frame = match picture.gpu() {
        None => crate::export::video::frame_ref(picture.frame())?,
        Some(_) => crate::hw::to_read(picture.frame())?,
    };
    let rotation = picture.rotation();
    let sar = frame.aspect_ratio();
    let (n, d) = (sar.numerator(), sar.denominator());
    let square = n <= 0 || d <= 0 || n == d;
    if colors.is_none()
        && frame.format() == format::Pixel::YUV420P
        && rotation.is_multiple_of(360)
        && square
    {
        return Ok(frame);
    }
    let (w, h) = if rotation % 180 == 90 {
        (frame.height(), frame.width())
    } else {
        (frame.width(), frame.height())
    };
    let mut normalizer = Normalizer::new(
        &frame,
        rotation,
        w,
        h,
        wipe_pixel(colors),
        colors,
        crate::decode::Reading::Stream,
    )?;
    normalizer.run(&frame)
}

fn wipe_pixel(colors: Option<vidcut_core::model::Hdr>) -> format::Pixel {
    match colors {
        Some(_) => format::Pixel::YUV420P10LE,
        None => format::Pixel::YUV420P,
    }
}

fn fitted(
    picture: &Picture,
    width: u32,
    height: u32,
    colors: Option<vidcut_core::model::Hdr>,
) -> Result<frame::Video> {
    let frame = match picture.gpu() {
        None => crate::export::video::frame_ref(picture.frame())?,
        Some(_) => crate::hw::to_read(picture.frame())?,
    };
    if colors.is_none()
        && frame.format() == format::Pixel::YUV420P
        && frame.width() == width
        && frame.height() == height
        && picture.rotation().is_multiple_of(360)
    {
        return Ok(frame);
    }
    let mut normalizer = Normalizer::new(
        &frame,
        picture.rotation(),
        width,
        height,
        wipe_pixel(colors),
        colors,
        crate::decode::Reading::Stream,
    )?;
    normalizer.run(&frame)
}

fn bytes_of(frame: &frame::Video) -> usize {
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
    use std::sync::Arc;

    use super::{Layout, Over, Picture};
    use crate::ffmpeg::{ffi, format::Pixel, frame};
    use vidcut_core::model::WipeDirection;
    use vidcut_core::render::{Pace, Placed, WHOLE, Wipe};

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

    fn filled(y: u8) -> frame::Video {
        let mut f = frame::Video::new(Pixel::YUV420P, 16, 8);
        crate::export::video::fill_black(&mut f).unwrap();
        let stride = f.stride(0);
        let data = f.data_mut(0);
        for row in 0..8 {
            data[row * stride..row * stride + 16].fill(y);
        }
        f
    }

    fn luma_rect(frame: &frame::Video, y: u8) -> [u32; 4] {
        let (w, h) = (frame.width(), frame.height());
        let stride = frame.stride(0);
        let data = frame.data(0);
        let mut rect = [w, h, 0, 0];
        for row in 0..h {
            for col in 0..w {
                if data[row as usize * stride + col as usize] != y {
                    continue;
                }
                rect[0] = rect[0].min(col);
                rect[1] = rect[1].min(row);
                rect[2] = rect[2].max(col + 1);
                rect[3] = rect[3].max(row + 1);
            }
        }
        rect
    }

    #[test]
    fn a_zoomed_side_keeps_the_export_rect_at_the_middle_of_a_left_wipe() {
        let mask = |falling| Wipe {
            direction: WipeDirection::Left,
            along: (1, 2),
            falling,
        };
        let over = |y, falling| Over {
            picture: Arc::new(Picture::new(filled(y), 0, None, None)),
            weight: WHOLE,
            wipe: Some(mask(falling)),
        };
        let picture = Picture::new(filled(16), 0, None, None)
            .with_over(vec![over(80, true), over(200, false)]);
        let placed = vec![
            Vec::new(),
            Vec::new(),
            vec![Placed {
                clip: "in".into(),
                picture: vidcut_core::model::Picture {
                    zoom: Some([0.5, 0.5]),
                    position: Some([0.25, 0.5]),
                    ..vidcut_core::model::Picture::default()
                },
                run: None,
                source: 0,
                pace: Pace::default(),
                start: 0,
                ready: true,
            }],
        ];
        let out = picture
            .at_wipe(16, 8, &placed, &[false, false, true], None)
            .unwrap()
            .unwrap();
        assert_eq!(luma_rect(out.frame(), 200), [0, 2, 8, 6]);
    }

    #[test]
    fn a_left_wipe_without_placement_still_splits_the_frame() {
        let picture = Picture::new(filled(16), 0, None, None).with_over(vec![Over {
            picture: Arc::new(Picture::new(filled(200), 0, None, None)),
            weight: WHOLE,
            wipe: Some(Wipe {
                direction: WipeDirection::Left,
                along: (1, 2),
                falling: false,
            }),
        }]);
        let out = picture.at_wipe(16, 8, &[], &[], None).unwrap().unwrap();
        assert_eq!(luma_rect(out.frame(), 200), [0, 0, 8, 8]);
        assert_eq!(luma_rect(out.frame(), 16), [8, 0, 16, 8]);
    }
}
