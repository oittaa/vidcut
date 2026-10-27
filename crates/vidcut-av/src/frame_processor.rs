use std::ptr::NonNull;
use std::sync::OnceLock;

use half::f16;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFDictionary, CFRetained, CFType};
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVAttachmentMode, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferIsPlanar, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
    kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferColorPrimariesKey,
    kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferTransferFunctionKey,
    kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
    kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrixKey, kCVPixelBufferHeightKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
    kCVPixelFormatType_64RGBAHalf, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSMutableDictionary, NSNumber, NSString};
use objc2_video_toolbox::{
    VTFrameProcessorFrame, VTPixelTransferSession, VTSessionSetProperty,
    kVTPixelTransferPropertyKey_DestinationColorPrimaries,
    kVTPixelTransferPropertyKey_DestinationTransferFunction,
    kVTPixelTransferPropertyKey_DestinationYCbCrMatrix,
};

use crate::color::Matrix;
use crate::error::{Error, Result};
use crate::ffmpeg::{self, ffi, frame::Video};

const BGRA: u32 = kCVPixelFormatType_32BGRA;
const NV12: u32 = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
const RGHA: u32 = kCVPixelFormatType_64RGBAHalf;

pub(crate) fn numbers(list: &NSArray<NSNumber>) -> Vec<u32> {
    (0..list.count())
        .map(|i| list.objectAtIndex(i).unsignedIntValue())
        .collect()
}

pub(crate) fn fourcc(format: u32) -> String {
    format
        .to_be_bytes()
        .iter()
        .map(|b| char::from(*b))
        .collect()
}

pub(crate) fn failed(what: &str, e: Retained<NSError>) -> Error {
    Error::Invalid(format!("{what}: {}", e.localizedDescription()))
}

pub(crate) fn format_in(list: &NSArray<NSNumber>, what: &str) -> Result<u32> {
    let formats = numbers(list);
    [BGRA, NV12, RGHA]
        .into_iter()
        .find(|f| formats.contains(f))
        .ok_or_else(|| {
            let names: Vec<String> = formats.iter().map(|f| fourcc(*f)).collect();
            Error::Invalid(format!(
                "{what} takes none of vidcut's pixel formats, only {}",
                names.join(", ")
            ))
        })
}

pub(crate) fn pool(
    (w, h): (u32, u32),
    format: u32,
    attributes: &NSDictionary<NSString, AnyObject>,
    what: &str,
) -> Result<CFRetained<CVPixelBufferPool>> {
    let all = NSMutableDictionary::dictionaryWithDictionary(attributes);
    // SAFETY: constant keys of CoreVideo.
    let keys = unsafe {
        [
            (kCVPixelBufferWidthKey, w),
            (kCVPixelBufferHeightKey, h),
            (kCVPixelBufferPixelFormatTypeKey, format),
        ]
    };
    for (key, value) in keys {
        // SAFETY: a CFString is an NSString.
        let key = unsafe { &*std::ptr::from_ref(key).cast::<NSString>() };
        all.insert(key, NSNumber::new_u32(value).as_ref());
    }
    let attributes =
        std::ptr::from_ref::<NSDictionary<NSString, AnyObject>>(&all).cast::<CFDictionary>();
    let mut out: *mut CVPixelBufferPool = std::ptr::null_mut();
    // SAFETY: an NSDictionary is a CFDictionary, and `out` is a valid place for the pool.
    let status = unsafe {
        CVPixelBufferPool::create(None, None, Some(&*attributes), NonNull::from(&mut out))
    };
    if status != kCVReturnSuccess {
        return Err(Error::Invalid(format!(
            "picture buffers for {what}: CoreVideo error {status}"
        )));
    }
    let out = NonNull::new(out)
        .ok_or_else(|| Error::Invalid(format!("no picture buffers for {what}")))?;
    // SAFETY: CVPixelBufferPoolCreate gives the pool with one retain, which this takes.
    Ok(unsafe { CFRetained::from_raw(out) })
}

pub(crate) fn buffer(pool: &CVPixelBufferPool, what: &str) -> Result<CFRetained<CVPixelBuffer>> {
    let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
    // SAFETY: a live pool, and `out` is a valid place for the buffer.
    let status =
        unsafe { CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut out)) };
    if status != kCVReturnSuccess {
        return Err(Error::Invalid(format!(
            "a picture buffer for {what}: CoreVideo error {status}"
        )));
    }
    let out =
        NonNull::new(out).ok_or_else(|| Error::Invalid(format!("no picture buffer for {what}")))?;
    // SAFETY: CVPixelBufferPoolCreatePixelBuffer gives the buffer with one retain, which this takes.
    Ok(unsafe { CFRetained::from_raw(out) })
}

pub(crate) fn frame(
    buffer: &CVPixelBuffer,
    time: CMTime,
    what: &str,
) -> Result<Retained<VTFrameProcessorFrame>> {
    // SAFETY: a live pixel buffer and a plain time.
    unsafe {
        VTFrameProcessorFrame::initWithBuffer_presentationTimeStamp(
            VTFrameProcessorFrame::alloc(),
            buffer,
            time,
        )
    }
    .ok_or_else(|| Error::Invalid(format!("{what} refused a picture buffer")))
}

pub(crate) struct Transfer(CFRetained<VTPixelTransferSession>);

// SAFETY: one thread at a time uses a transfer, behind the mutex of its process.
unsafe impl Send for Transfer {}

impl Transfer {
    pub(crate) fn new(what: &str) -> Result<Self> {
        let mut out: *mut VTPixelTransferSession = std::ptr::null_mut();
        // SAFETY: `out` is a valid place for the session.
        let status = unsafe { VTPixelTransferSession::create(None, NonNull::from(&mut out)) };
        let out = NonNull::new(out).filter(|_| status == 0).ok_or_else(|| {
            Error::Invalid(format!(
                "a pixel transfer for {what}: VideoToolbox error {status}"
            ))
        })?;
        // SAFETY: VTPixelTransferSessionCreate gives the session with one retain, which this takes.
        let session = unsafe { CFRetained::from_raw(out) };
        let target: &CFType = (*session).as_ref();
        // SAFETY: constant keys and values of VideoToolbox and CoreVideo, on a live session.
        unsafe {
            for (key, value) in [
                (
                    kVTPixelTransferPropertyKey_DestinationColorPrimaries,
                    kCVImageBufferColorPrimaries_ITU_R_709_2,
                ),
                (
                    kVTPixelTransferPropertyKey_DestinationTransferFunction,
                    kCVImageBufferTransferFunction_ITU_R_709_2,
                ),
            ] {
                let value: &CFType = value.as_ref();
                VTSessionSetProperty(target, key, Some(value));
            }
        }
        Ok(Self(session))
    }

    pub(crate) fn run(
        &self,
        from: &CVPixelBuffer,
        to: &CVPixelBuffer,
        matrix: Matrix,
        what: &str,
    ) -> Result<()> {
        // SAFETY: constant values of CoreVideo.
        let matrix = unsafe {
            match matrix {
                Matrix::Bt601 => kCVImageBufferYCbCrMatrix_ITU_R_601_4,
                Matrix::Bt709 => kCVImageBufferYCbCrMatrix_ITU_R_709_2,
                Matrix::Bt2020 => kCVImageBufferYCbCrMatrix_ITU_R_2020,
            }
        };
        let target: &CFType = (*self.0).as_ref();
        // SAFETY: a live buffer and session, and constant keys of CoreVideo and VideoToolbox with values of their kinds.
        unsafe {
            for (key, value) in [
                (kCVImageBufferYCbCrMatrixKey, matrix),
                (
                    kCVImageBufferColorPrimariesKey,
                    kCVImageBufferColorPrimaries_ITU_R_709_2,
                ),
                (
                    kCVImageBufferTransferFunctionKey,
                    kCVImageBufferTransferFunction_ITU_R_709_2,
                ),
            ] {
                let value: &CFType = value.as_ref();
                from.set_attachment(key, value, CVAttachmentMode::ShouldPropagate);
            }
            let value: &CFType = matrix.as_ref();
            VTSessionSetProperty(
                target,
                kVTPixelTransferPropertyKey_DestinationYCbCrMatrix,
                Some(value),
            );
        }
        // SAFETY: a live session and two live buffers.
        let status = unsafe { self.0.transfer_image(from, to) };
        if status != 0 {
            return Err(Error::Invalid(format!(
                "{what}: VideoToolbox error {status} in a pixel transfer"
            )));
        }
        Ok(())
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        // SAFETY: the session that `new` made.
        unsafe { self.0.invalidate() };
    }
}

pub(crate) fn pixels<'a>(f: &'a Video, what: &str) -> Result<&'a CVPixelBuffer> {
    // SAFETY: a VideoToolbox frame's `data[3]` is its pixel buffer, alive as long as the frame.
    unsafe { (*f.as_ptr()).data[3].cast::<CVPixelBuffer>().as_ref() }
        .ok_or_else(|| Error::Invalid(format!("{what}: a frame without a pixel buffer")))
}

type Like = (ffi::AVPixelFormat, ffi::AVColorRange, (u32, u32));

fn like_of(f: &Video, size: (u32, u32)) -> Option<Like> {
    // SAFETY: the frame's own fields, and its live frames context.
    unsafe {
        let of = (*f.as_ptr()).hw_frames_ctx;
        (!of.is_null()).then(|| {
            let of = &*((*of).data as *const ffi::AVHWFramesContext);
            (of.sw_format, (*f.as_ptr()).color_range, size)
        })
    }
}

struct Pool {
    frames: *mut ffi::AVBufferRef,
    like: Like,
}

impl Pool {
    fn new(f: &Video, like: Like, what: &str) -> Result<Self> {
        let (format, range, (w, h)) = like;
        // SAFETY: the frame's live frames context, and a new frames context on its device, freed on failure.
        unsafe {
            let of = &*((*(*f.as_ptr()).hw_frames_ctx).data as *const ffi::AVHWFramesContext);
            let mut frames = ffi::av_hwframe_ctx_alloc(of.device_ref);
            if frames.is_null() {
                return Err(Error::Invalid(format!("{what}: no memory for GPU frames")));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
            fc.sw_format = format;
            fc.width = w as i32;
            fc.height = h as i32;
            (*(fc.hwctx as *mut ffi::AVVTFramesContext)).color_range = range;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                ffi::av_buffer_unref(&mut frames);
                return Err(Error::Ffmpeg {
                    context: format!("{what}: setting up VideoToolbox frames"),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Self { frames, like })
        }
    }

    fn frame(&self, what: &str) -> Result<Video> {
        let mut out = Video::empty();
        // SAFETY: a fresh frame of this live pool.
        let ret = unsafe { ffi::av_hwframe_get_buffer(self.frames, out.as_mut_ptr(), 0) };
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: format!("{what}: getting a VideoToolbox frame"),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(out)
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: the reference that `new` made.
        unsafe { ffi::av_buffer_unref(&mut self.frames) };
    }
}

#[derive(Default)]
pub(crate) struct Frames(Option<Pool>);

impl Frames {
    pub(crate) fn made(
        &mut self,
        transfer: &Transfer,
        picture: &CVPixelBuffer,
        like: &Video,
        size: (u32, u32),
        what: &str,
    ) -> Result<Video> {
        let key = like_of(like, size)
            .ok_or_else(|| Error::Invalid(format!("{what}: a frame without GPU frames")))?;
        if self.0.as_ref().is_none_or(|p| p.like != key) {
            self.0 = None;
            self.0 = Some(Pool::new(like, key, what)?);
        }
        let mut out = self.0.as_ref().expect("a pool").frame(what)?;
        let matrix = crate::color::colors_of(like).matrix;
        transfer.run(picture, pixels(&out, what)?, matrix, what)?;
        out.set_pts(like.pts());
        // SAFETY: both frames are valid; plain fields.
        unsafe {
            let (o, i) = (out.as_mut_ptr(), like.as_ptr());
            (*o).color_range = (*i).color_range;
            (*o).colorspace = (*i).colorspace;
            (*o).color_primaries = (*i).color_primaries;
            (*o).color_trc = (*i).color_trc;
            (*o).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
        }
        Ok(out)
    }
}

struct Locked<'a>(&'a CVPixelBuffer);

impl<'a> Locked<'a> {
    fn new(buffer: &'a CVPixelBuffer) -> Result<Self> {
        // SAFETY: a live buffer, unlocked when this drops.
        let status = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags(0)) };
        if status != kCVReturnSuccess {
            return Err(Error::Invalid(format!(
                "a picture buffer of macOS: CoreVideo error {status}"
            )));
        }
        Ok(Self(buffer))
    }

    fn plane(&self, i: usize) -> (*mut u8, usize) {
        if !CVPixelBufferIsPlanar(self.0) {
            return (
                CVPixelBufferGetBaseAddress(self.0).cast(),
                CVPixelBufferGetBytesPerRow(self.0),
            );
        }
        (
            CVPixelBufferGetBaseAddressOfPlane(self.0, i).cast(),
            CVPixelBufferGetBytesPerRowOfPlane(self.0, i),
        )
    }

    fn row(&mut self, plane: usize, y: usize, bytes: usize) -> &mut [u8] {
        let (base, stride) = self.plane(plane);
        // SAFETY: row y of a locked plane of `stride` bytes for each row.
        unsafe { std::slice::from_raw_parts_mut(base.add(y * stride), bytes) }
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: the lock that `new` took.
        let _ = unsafe { CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags(0)) };
    }
}

fn ycbcr(p: &[u8]) -> (f32, f32, f32) {
    let [r, g, b] = [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0);
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    (y, (b - y) / 1.8556, (r - y) / 1.5748)
}

fn code(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

pub(crate) fn write(
    buffer: &CVPixelBuffer,
    format: u32,
    (w, h): (u32, u32),
    rgb: &[u8],
) -> Result<()> {
    let (w, h) = (w as usize, h as usize);
    let mut locked = Locked::new(buffer)?;
    let pixel = |x: usize, y: usize| &rgb[(y * w + x) * 3..(y * w + x) * 3 + 3];
    if format == BGRA {
        for y in 0..h {
            let row = locked.row(0, y, w * 4);
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let s = pixel(x, y);
                *px = [s[2], s[1], s[0], 255];
            }
        }
        return Ok(());
    }
    if format == RGHA {
        let one = f16::from_f32(1.0).to_bits().to_le_bytes();
        for y in 0..h {
            let row = locked.row(0, y, w * 8);
            for (x, px) in row.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                let s = pixel(x, y);
                let [r, g, b] = [s[0], s[1], s[2]]
                    .map(|v| f16::from_f32(f32::from(v) / 255.0).to_bits().to_le_bytes());
                *px = [r[0], r[1], g[0], g[1], b[0], b[1], one[0], one[1]];
            }
        }
        return Ok(());
    }
    for y in 0..h {
        let row = locked.row(0, y, w);
        for (x, v) in row.iter_mut().enumerate() {
            *v = code(16.0 + 219.0 * ycbcr(pixel(x, y)).0);
        }
    }
    for cy in 0..h / 2 {
        let row = locked.row(1, cy, w);
        for cx in 0..w / 2 {
            let (mut cb, mut cr) = (0.0, 0.0);
            for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (_, b, r) = ycbcr(pixel(cx * 2 + x, cy * 2 + y));
                cb += b / 4.0;
                cr += r / 4.0;
            }
            row[cx * 2] = code(128.0 + 224.0 * cb);
            row[cx * 2 + 1] = code(128.0 + 224.0 * cr);
        }
    }
    Ok(())
}

fn halves() -> &'static [u8] {
    static CODES: OnceLock<Vec<u8>> = OnceLock::new();
    CODES.get_or_init(|| {
        (0..=u16::MAX)
            .map(|bits| code(255.0 * f16::from_bits(bits).to_f32()))
            .collect()
    })
}

pub(crate) fn read(buffer: &CVPixelBuffer, format: u32, (w, h): (u32, u32)) -> Result<Vec<u8>> {
    let (w, h) = (w as usize, h as usize);
    let mut locked = Locked::new(buffer)?;
    if format == RGHA {
        let codes = halves();
        let mut out = vec![0; w * h * 3];
        for (y, line) in out.chunks_exact_mut(w * 3).enumerate() {
            let row = locked.row(0, y, w * 8);
            for (to, px) in line
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(row.as_chunks::<8>().0)
            {
                let c = |i: usize| codes[usize::from(u16::from_le_bytes([px[i], px[i + 1]]))];
                *to = [c(0), c(2), c(4)];
            }
        }
        return Ok(out);
    }
    let mut out = Vec::with_capacity(w * h * 3);
    if format == BGRA {
        for y in 0..h {
            for px in locked.row(0, y, w * 4).as_chunks::<4>().0 {
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        return Ok(out);
    }
    for y in 0..h {
        let luma = locked.row(0, y, w).to_vec();
        let chroma = locked.row(1, y / 2, w).to_vec();
        for (x, l) in luma.iter().enumerate() {
            let y = (f32::from(*l) - 16.0) / 219.0;
            let pb = (f32::from(chroma[x / 2 * 2]) - 128.0) / 224.0;
            let pr = (f32::from(chroma[x / 2 * 2 + 1]) - 128.0) / 224.0;
            out.extend_from_slice(&[
                code(255.0 * (y + 1.5748 * pr)),
                code(255.0 * (y - 0.1873 * pb - 0.4681 * pr)),
                code(255.0 * (y + 1.8556 * pb)),
            ]);
        }
    }
    Ok(out)
}
