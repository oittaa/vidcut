use std::path::PathBuf;

use crate::error::Result;
use crate::ffmpeg::frame;

type Key = (PathBuf, i64, i64);

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) enum Input<'a> {
    Rgb(&'a [u8]),
    Frame(&'a frame::Video),
}

const NEEDS: &str = "the frame rate conversion of macOS isn't on this Mac";

pub(super) fn ready() -> Result<()> {
    imp::ready()
}

pub(super) fn between(key: &Key, a: Input, b: Input, size: (u32, u32), t: f64) -> Result<Vec<u8>> {
    imp::between(key, a, b, size, t)
}

pub(super) fn between_frames(
    key: &Key,
    a: &frame::Video,
    b: &frame::Video,
    size: (u32, u32),
    t: f64,
) -> Result<frame::Video> {
    imp::between_frames(key, a, b, size, t)
}

pub(super) fn release() {
    imp::release();
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Input, Key, NEEDS};
    use crate::error::{Error, Result};

    pub(super) fn ready() -> Result<()> {
        Err(Error::Invalid(NEEDS.into()))
    }

    pub(super) fn between(
        _key: &Key,
        _a: Input,
        _b: Input,
        _size: (u32, u32),
        _t: f64,
    ) -> Result<Vec<u8>> {
        Err(Error::Invalid(NEEDS.into()))
    }

    pub(super) fn between_frames(
        _key: &Key,
        _a: &crate::ffmpeg::frame::Video,
        _b: &crate::ffmpeg::frame::Video,
        _size: (u32, u32),
        _t: f64,
    ) -> Result<crate::ffmpeg::frame::Video> {
        Err(Error::Invalid(NEEDS.into()))
    }

    pub(super) fn release() {}
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::{Mutex, PoisonError};

    use objc2::AllocAnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, ProtocolObject};
    use objc2_core_foundation::CFRetained;
    use objc2_core_media::CMTime;
    use objc2_core_video::{CVPixelBuffer, CVPixelBufferPool};
    use objc2_foundation::{NSArray, NSNumber};
    use objc2_video_toolbox::{
        VTFrameProcessor, VTFrameProcessorFrame, VTFrameRateConversionConfiguration,
        VTFrameRateConversionConfigurationQualityPrioritization, VTFrameRateConversionParameters,
        VTFrameRateConversionParametersSubmissionMode as Mode,
    };

    use super::{Input, Key, NEEDS};
    use crate::error::{Error, Result};
    use crate::ffmpeg::{self, ffi, frame::Video};
    use crate::frame_processor::{
        Transfer, buffer, failed, format_in, fourcc, frame, pool, read, write,
    };

    const WHAT: &str = "the frame rate conversion of macOS";

    fn available() -> bool {
        AnyClass::get(c"VTFrameRateConversionConfiguration").is_some()
            // SAFETY: a class method of a class that exists.
            && unsafe { VTFrameRateConversionConfiguration::isSupported() }
    }

    pub(super) fn ready() -> Result<()> {
        if available() {
            Ok(())
        } else {
            Err(Error::Invalid(NEEDS.into()))
        }
    }

    struct Pair {
        key: Key,
        source: Retained<VTFrameProcessorFrame>,
        next: Retained<VTFrameProcessorFrame>,
    }

    struct Pool {
        frames: *mut ffi::AVBufferRef,
        like: (ffi::AVPixelFormat, ffi::AVColorRange),
    }

    fn like(f: &Video) -> Option<(ffi::AVPixelFormat, ffi::AVColorRange)> {
        // SAFETY: the frame's own fields, and its live frames context.
        unsafe {
            let of = (*f.as_ptr()).hw_frames_ctx;
            (!of.is_null()).then(|| {
                let of = &*((*of).data as *const ffi::AVHWFramesContext);
                (of.sw_format, (*f.as_ptr()).color_range)
            })
        }
    }

    impl Pool {
        fn like(f: &Video, (w, h): (u32, u32)) -> Result<Self> {
            // SAFETY: the frame's live frames context, and a new frames context on its device, freed on failure.
            unsafe {
                let of = (*f.as_ptr()).hw_frames_ctx;
                if of.is_null() {
                    return Err(Error::Invalid(format!(
                        "{WHAT}: a frame without GPU frames"
                    )));
                }
                let of = &*((*of).data as *const ffi::AVHWFramesContext);
                let mut frames = ffi::av_hwframe_ctx_alloc(of.device_ref);
                if frames.is_null() {
                    return Err(Error::Invalid(format!("{WHAT}: no memory for GPU frames")));
                }
                let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
                fc.format = ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
                fc.sw_format = of.sw_format;
                fc.width = w as i32;
                fc.height = h as i32;
                let range = (*f.as_ptr()).color_range;
                (*(fc.hwctx as *mut ffi::AVVTFramesContext)).color_range = range;
                let ret = ffi::av_hwframe_ctx_init(frames);
                if ret < 0 {
                    ffi::av_buffer_unref(&mut frames);
                    return Err(Error::Ffmpeg {
                        context: format!("{WHAT}: setting up VideoToolbox frames"),
                        source: ffmpeg::Error::from(ret),
                    });
                }
                Ok(Self {
                    frames,
                    like: (of.sw_format, range),
                })
            }
        }

        fn frame(&self) -> Result<Video> {
            let mut out = Video::empty();
            // SAFETY: a fresh frame of this live pool.
            let ret = unsafe { ffi::av_hwframe_get_buffer(self.frames, out.as_mut_ptr(), 0) };
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: format!("{WHAT}: getting a VideoToolbox frame"),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(out)
        }
    }

    impl Drop for Pool {
        fn drop(&mut self) {
            // SAFETY: the reference that `like` made.
            unsafe { ffi::av_buffer_unref(&mut self.frames) };
        }
    }

    fn pixels(f: &Video) -> Result<&CVPixelBuffer> {
        // SAFETY: a VideoToolbox frame's `data[3]` is its pixel buffer, alive as long as the frame.
        unsafe { (*f.as_ptr()).data[3].cast::<CVPixelBuffer>().as_ref() }
            .ok_or_else(|| Error::Invalid(format!("{WHAT}: a frame without a pixel buffer")))
    }

    struct Session {
        processor: Retained<VTFrameProcessor>,
        sources: CFRetained<CVPixelBufferPool>,
        outputs: CFRetained<CVPixelBufferPool>,
        transfer: Transfer,
        format: u32,
        size: (u32, u32),
        pair: Option<Pair>,
        pool: Option<Pool>,
    }

    // SAFETY: the session sits behind a mutex, so one thread at a time uses it.
    unsafe impl Send for Session {}

    static SESSION: Mutex<Option<Session>> = Mutex::new(None);

    fn time(value: i64, scale: i32) -> CMTime {
        // SAFETY: a time of plain values.
        unsafe { CMTime::new(value, scale) }
    }

    impl Session {
        fn open(size: (u32, u32)) -> Result<Self> {
            if !available() {
                return Err(Error::Invalid(NEEDS.into()));
            }
            // SAFETY: an initializer of plain values; it gives nil for what it can't take.
            let config = unsafe {
                VTFrameRateConversionConfiguration::initWithFrameWidth_frameHeight_usePrecomputedFlow_qualityPrioritization_revision(
                    VTFrameRateConversionConfiguration::alloc(),
                    size.0 as isize,
                    size.1 as isize,
                    false,
                    VTFrameRateConversionConfigurationQualityPrioritization::Quality,
                    VTFrameRateConversionConfiguration::defaultRevision(),
                )
            }
            .ok_or_else(|| {
                Error::Invalid(format!("{WHAT} can't take {}x{}", size.0, size.1))
            })?;
            // SAFETY: properties of a live configuration.
            let (formats, from, to) = unsafe {
                (
                    config.frameSupportedPixelFormats(),
                    config.sourcePixelBufferAttributes(),
                    config.destinationPixelBufferAttributes(),
                )
            };
            let format = format_in(&formats, WHAT)?;
            let sources = pool(size, format, &from, WHAT)?;
            let outputs = pool(size, format, &to, WHAT)?;
            // SAFETY: a new processor of the frame processor class.
            let processor = unsafe { VTFrameProcessor::new() };
            // SAFETY: a new processor and a live configuration.
            unsafe {
                processor.startSessionWithConfiguration_error(ProtocolObject::from_ref(&*config))
            }
            .map_err(|e| failed(WHAT, e))?;
            crate::interpolate::runs_on("macOS".into());
            crate::debug!(
                "interpolate",
                "macOS: {}x{}, pictures as {}",
                size.0,
                size.1,
                fourcc(format)
            );
            Ok(Self {
                processor,
                sources,
                outputs,
                transfer: Transfer::new(WHAT)?,
                format,
                size,
                pair: None,
                pool: None,
            })
        }

        fn frame(&self, input: &Input, at: CMTime) -> Result<Retained<VTFrameProcessorFrame>> {
            let picture = buffer(&self.sources, WHAT)?;
            match input {
                Input::Rgb(rgb) => write(&picture, self.format, self.size, rgb)?,
                Input::Frame(f) => {
                    let matrix = crate::color::colors_of(f).matrix;
                    self.transfer.run(pixels(f)?, &picture, matrix, WHAT)?;
                }
            }
            frame(&picture, at, WHAT)
        }

        fn make(
            &mut self,
            key: &Key,
            a: Input,
            b: Input,
            t: f64,
        ) -> Result<(CFRetained<CVPixelBuffer>, [f64; 2])> {
            let mode = match &self.pair {
                Some(p) if p.key == *key => Mode::SequentialReferencesUnchanged,
                Some(p) if p.key.0 == key.0 && p.key.2 == key.1 => Mode::Sequential,
                _ => Mode::Random,
            };
            let clock = std::time::Instant::now();
            if mode != Mode::SequentialReferencesUnchanged {
                self.pair = Some(Pair {
                    key: key.clone(),
                    source: self.frame(&a, time(key.1, 1))?,
                    next: self.frame(&b, time(key.2, 1))?,
                });
            }
            let pair = self.pair.as_ref().expect("a pair");
            let made = buffer(&self.outputs, WHAT)?;
            let at = time(
                key.1 * 1000 + ((key.2 - key.1) as f64 * t * 1000.0).round() as i64,
                1000,
            );
            let destinations = NSArray::from_retained_slice(&[frame(&made, at, WHAT)?]);
            let phases = NSArray::from_retained_slice(&[NSNumber::new_f32(t as f32)]);
            // SAFETY: live frames of buffers made with the configuration's attributes.
            let parameters = unsafe {
                VTFrameRateConversionParameters::initWithSourceFrame_nextFrame_opticalFlow_interpolationPhase_submissionMode_destinationFrames(
                    VTFrameRateConversionParameters::alloc(),
                    &pair.source,
                    &pair.next,
                    None,
                    &phases,
                    mode,
                    &destinations,
                )
            }
            .ok_or_else(|| Error::Invalid(format!("{WHAT} refused a pair of frames")))?;
            let wrote = clock.elapsed();
            // SAFETY: a started session and live parameters.
            unsafe {
                self.processor
                    .processWithParameters_error(ProtocolObject::from_ref(&*parameters))
            }
            .map_err(|e| failed(WHAT, e))?;
            let processed = clock.elapsed();
            Ok((
                made,
                [
                    wrote.as_secs_f64() * 1e3,
                    (processed - wrote).as_secs_f64() * 1e3,
                ],
            ))
        }

        fn between(&mut self, key: &Key, a: Input, b: Input, t: f64) -> Result<Vec<u8>> {
            let (made, [wrote, processed]) = self.make(key, a, b, t)?;
            let clock = std::time::Instant::now();
            let out = read(&made, self.format, self.size)?;
            told(wrote, processed, clock);
            Ok(out)
        }

        fn between_frames(&mut self, key: &Key, a: &Video, b: &Video, t: f64) -> Result<Video> {
            let (made, [wrote, processed]) = self.make(key, Input::Frame(a), Input::Frame(b), t)?;
            let clock = std::time::Instant::now();
            if self.pool.as_ref().is_none_or(|p| Some(p.like) != like(b)) {
                self.pool = None;
                self.pool = Some(Pool::like(b, self.size)?);
            }
            let mut out = self.pool.as_ref().expect("a pool").frame()?;
            let matrix = crate::color::colors_of(b).matrix;
            self.transfer.run(&made, pixels(&out)?, matrix, WHAT)?;
            out.set_pts(b.pts());
            // SAFETY: both frames are valid; plain fields.
            unsafe {
                let (o, i) = (out.as_mut_ptr(), b.as_ptr());
                (*o).color_range = (*i).color_range;
                (*o).colorspace = (*i).colorspace;
                (*o).color_primaries = (*i).color_primaries;
                (*o).color_trc = (*i).color_trc;
                (*o).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
            }
            told(wrote, processed, clock);
            Ok(out)
        }
    }

    fn told(wrote: f64, processed: f64, clock: std::time::Instant) {
        crate::debug!(
            "interpolate",
            "macOS: pair in {wrote:.1} ms, processed in {processed:.1} ms, read in {:.1} ms",
            clock.elapsed().as_secs_f64() * 1e3
        );
    }

    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: the session that `open` started.
            unsafe { self.processor.endSession() };
        }
    }

    fn session() -> std::sync::MutexGuard<'static, Option<Session>> {
        SESSION.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn between(
        key: &Key,
        a: Input,
        b: Input,
        size: (u32, u32),
        t: f64,
    ) -> Result<Vec<u8>> {
        let mut slot = session();
        if slot.as_ref().is_none_or(|s| s.size != size) {
            *slot = None;
            *slot = Some(Session::open(size)?);
        }
        slot.as_mut().expect("a session").between(key, a, b, t)
    }

    pub(super) fn between_frames(
        key: &Key,
        a: &Video,
        b: &Video,
        size: (u32, u32),
        t: f64,
    ) -> Result<Video> {
        let mut slot = session();
        if slot.as_ref().is_none_or(|s| s.size != size) {
            *slot = None;
            *slot = Some(Session::open(size)?);
        }
        slot.as_mut()
            .expect("a session")
            .between_frames(key, a, b, t)
    }

    pub(super) fn release() {
        *session() = None;
    }
}
