use std::path::PathBuf;

use crate::error::Result;
use crate::ffmpeg::frame;

type Key = (PathBuf, u64, i64, i64);

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) enum Input<'a> {
    Held(&'a frame::Video),
    Frame(&'a frame::Video),
}

const NEEDS: &str = "the frame rate conversion of macOS isn't on this Mac";

pub(super) fn ready() -> Result<()> {
    imp::ready()
}

pub(super) fn formats() -> Result<Vec<String>> {
    imp::formats()
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

    pub(super) fn formats() -> Result<Vec<String>> {
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
    use crate::ffmpeg::frame::Video;
    use crate::frame_processor::{
        Frames, Transfer, buffer, failed, format_in, fourcc, frame, numbers, pixels, pool, read,
        write,
    };

    const WHAT: &str = "the frame rate conversion of macOS";

    fn available() -> bool {
        AnyClass::get(c"VTFrameRateConversionConfiguration").is_some()
            // SAFETY: a class method of a class that exists.
            && unsafe { VTFrameRateConversionConfiguration::isSupported() }
    }

    pub(super) fn formats() -> Result<Vec<String>> {
        if !available() {
            return Err(Error::Invalid(NEEDS.into()));
        }
        // SAFETY: an initializer of plain values; it gives nil for what it can't take.
        let config = unsafe {
            VTFrameRateConversionConfiguration::initWithFrameWidth_frameHeight_usePrecomputedFlow_qualityPrioritization_revision(
                VTFrameRateConversionConfiguration::alloc(),
                1920,
                1080,
                false,
                VTFrameRateConversionConfigurationQualityPrioritization::Quality,
                VTFrameRateConversionConfiguration::defaultRevision(),
            )
        }
        .ok_or_else(|| Error::Invalid(format!("{WHAT} can't take 1920x1080")))?;
        // SAFETY: a property of a live configuration.
        let list = unsafe { config.frameSupportedPixelFormats() };
        Ok(numbers(&list).into_iter().map(fourcc).collect())
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

    struct Session {
        processor: Retained<VTFrameProcessor>,
        sources: CFRetained<CVPixelBufferPool>,
        outputs: CFRetained<CVPixelBufferPool>,
        transfer: Transfer,
        lanczos: Option<crate::lanczos::Lanczos>,
        format: u32,
        size: (u32, u32),
        pair: Option<Pair>,
        frames: Frames,
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
                lanczos: None,
                format,
                size,
                pair: None,
                frames: Frames::default(),
            })
        }

        fn frame(&mut self, input: &Input, at: CMTime) -> Result<Retained<VTFrameProcessorFrame>> {
            let picture = buffer(&self.sources, WHAT)?;
            match input {
                Input::Held(f) => write(
                    &picture,
                    self.format,
                    self.size,
                    &crate::screen::held::rgb(f)?,
                )?,
                Input::Frame(f) => {
                    let scaled = crate::lanczos::sized(&mut self.lanczos, f, self.size)?;
                    let f = scaled.as_ref().unwrap_or(*f);
                    let matrix = crate::color::colors_of(f).matrix;
                    self.transfer
                        .run(pixels(f, WHAT)?, &picture, matrix, WHAT)?;
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
                Some(p) if p.key.0 == key.0 && p.key.1 == key.1 && p.key.3 == key.2 => {
                    Mode::Sequential
                }
                _ => Mode::Random,
            };
            let clock = std::time::Instant::now();
            if mode != Mode::SequentialReferencesUnchanged {
                self.pair = Some(Pair {
                    key: key.clone(),
                    source: self.frame(&a, time(key.2, 1))?,
                    next: self.frame(&b, time(key.3, 1))?,
                });
            }
            let pair = self.pair.as_ref().expect("a pair");
            let made = buffer(&self.outputs, WHAT)?;
            let at = time(
                key.2 * 1000 + ((key.3 - key.2) as f64 * t * 1000.0).round() as i64,
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
            let out = self
                .frames
                .made(&self.transfer, &made, b, self.size, WHAT)?;
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
