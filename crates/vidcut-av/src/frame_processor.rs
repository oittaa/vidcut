use std::ptr::NonNull;

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFDictionary, CFRetained, CFType};
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVAttachmentMode, CVPixelBuffer, CVPixelBufferPool, kCVImageBufferColorPrimaries_ITU_R_709_2,
    kCVImageBufferColorPrimariesKey, kCVImageBufferTransferFunction_ITU_R_709_2,
    kCVImageBufferTransferFunctionKey, kCVImageBufferYCbCrMatrix_ITU_R_601_4,
    kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_2020,
    kCVImageBufferYCbCrMatrixKey, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA, kCVPixelFormatType_64RGBAHalf,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
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
pub(crate) const HALF_FIRST: [u32; 3] = [RGHA, BGRA, NV12];
pub(crate) const BYTES_FIRST: [u32; 3] = [BGRA, NV12, RGHA];

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

pub(crate) fn format_in(list: &NSArray<NSNumber>, order: [u32; 3], what: &str) -> Result<u32> {
    let formats = numbers(list);
    let names: Vec<String> = formats.iter().map(|f| fourcc(*f)).collect();
    crate::debug!("ai", "{what} takes {}", names.join(", "));
    order
        .into_iter()
        .find(|f| formats.contains(f))
        .ok_or_else(|| {
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

fn sw_format(f: &Video) -> Option<ffi::AVPixelFormat> {
    // SAFETY: the frame's own field, and its live frames context.
    unsafe {
        let of = (*f.as_ptr()).hw_frames_ctx;
        (!of.is_null()).then(|| (*((*of).data as *const ffi::AVHWFramesContext)).sw_format)
    }
}

struct Pool {
    frames: *mut ffi::AVBufferRef,
    like: Like,
}

impl Pool {
    fn new(device: *mut ffi::AVBufferRef, like: Like, what: &str) -> Result<Self> {
        let (format, range, (w, h)) = like;
        // SAFETY: a live device reference, and a new frames context on it, freed on failure.
        unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(device);
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

pub(crate) struct Surfaces {
    device: *mut ffi::AVBufferRef,
    pool: Option<Pool>,
}

// SAFETY: one thread at a time uses the surfaces, behind the mutex of their process.
unsafe impl Send for Surfaces {}

impl Surfaces {
    pub(crate) fn new(what: &str) -> Result<Self> {
        let mut device = std::ptr::null_mut();
        // SAFETY: `device` is a valid place for the new device reference.
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut device,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: format!("{what}: opening VideoToolbox"),
                source: ffmpeg::Error::from(ret),
            });
        }
        Ok(Self { device, pool: None })
    }

    pub(crate) fn fresh(
        &mut self,
        (size, pixel): ((u32, u32), ffmpeg::format::Pixel),
        what: &str,
    ) -> Result<Video> {
        let key = (pixel.into(), ffi::AVColorRange::AVCOL_RANGE_MPEG, size);
        if self.pool.as_ref().is_none_or(|p| p.like != key) {
            self.pool = None;
            self.pool = Some(Pool::new(self.device, key, what)?);
        }
        self.pool.as_ref().expect("a pool").frame(what)
    }
}

impl Drop for Surfaces {
    fn drop(&mut self) {
        self.pool = None;
        // SAFETY: the reference that `new` made.
        unsafe { ffi::av_buffer_unref(&mut self.device) };
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
        let format = sw_format(like)
            .ok_or_else(|| Error::Invalid(format!("{what}: a frame without GPU frames")))?;
        self.made_as(transfer, picture, like, (size, format), what)
    }

    pub(crate) fn made_as(
        &mut self,
        transfer: &Transfer,
        picture: &CVPixelBuffer,
        like: &Video,
        shape: ((u32, u32), ffi::AVPixelFormat),
        what: &str,
    ) -> Result<Video> {
        let out = self.fresh(like, shape, what)?;
        let matrix = crate::color::colors_of(like).matrix;
        transfer.run(picture, pixels(&out, what)?, matrix, what)?;
        Ok(out)
    }

    pub(crate) fn fresh(
        &mut self,
        like: &Video,
        shape: ((u32, u32), ffi::AVPixelFormat),
        what: &str,
    ) -> Result<Video> {
        // SAFETY: a plain field of a valid frame.
        let range = unsafe { (*like.as_ptr()).color_range };
        self.fresh_in(like, shape, range, what)
    }

    pub(crate) fn fresh_in(
        &mut self,
        like: &Video,
        (size, format): ((u32, u32), ffi::AVPixelFormat),
        range: ffi::AVColorRange,
        what: &str,
    ) -> Result<Video> {
        if sw_format(like).is_none() {
            return Err(Error::Invalid(format!(
                "{what}: a frame without GPU frames"
            )));
        }
        let key = (format, range, size);
        if self.0.as_ref().is_none_or(|p| p.like != key) {
            self.0 = None;
            // SAFETY: `like` has a live frames context, checked above.
            let device = unsafe {
                (*((*(*like.as_ptr()).hw_frames_ctx).data as *const ffi::AVHWFramesContext))
                    .device_ref
            };
            self.0 = Some(Pool::new(device, key, what)?);
        }
        let mut out = self.0.as_ref().expect("a pool").frame(what)?;
        out.set_pts(like.pts());
        // SAFETY: both frames are valid; plain fields.
        unsafe {
            let (o, i) = (out.as_mut_ptr(), like.as_ptr());
            (*o).color_range = range;
            (*o).colorspace = (*i).colorspace;
            (*o).color_primaries = (*i).color_primaries;
            (*o).color_trc = (*i).color_trc;
            (*o).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
        }
        Ok(out)
    }
}
