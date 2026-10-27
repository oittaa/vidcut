use std::path::PathBuf;

use crate::error::Result;

type Key = (PathBuf, i64, i64);

const NEEDS: &str = "the frame rate conversion of macOS isn't on this Mac";

pub(super) fn ready() -> Result<()> {
    imp::ready()
}

pub(super) fn between(key: &Key, a: &[u8], b: &[u8], size: (u32, u32), t: f64) -> Result<Vec<u8>> {
    imp::between(key, a, b, size, t)
}

pub(super) fn release() {
    imp::release();
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Key, NEEDS};
    use crate::error::{Error, Result};

    pub(super) fn ready() -> Result<()> {
        Err(Error::Invalid(NEEDS.into()))
    }

    pub(super) fn between(
        _key: &Key,
        _a: &[u8],
        _b: &[u8],
        _size: (u32, u32),
        _t: f64,
    ) -> Result<Vec<u8>> {
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
    use objc2_core_video::CVPixelBufferPool;
    use objc2_foundation::{NSArray, NSNumber};
    use objc2_video_toolbox::{
        VTFrameProcessor, VTFrameProcessorFrame, VTFrameRateConversionConfiguration,
        VTFrameRateConversionConfigurationQualityPrioritization, VTFrameRateConversionParameters,
        VTFrameRateConversionParametersSubmissionMode as Mode,
    };

    use super::{Key, NEEDS};
    use crate::error::{Error, Result};
    use crate::frame_processor::{buffer, failed, format_in, fourcc, frame, pool, read, write};

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

    struct Session {
        processor: Retained<VTFrameProcessor>,
        sources: CFRetained<CVPixelBufferPool>,
        outputs: CFRetained<CVPixelBufferPool>,
        format: u32,
        size: (u32, u32),
        pair: Option<Pair>,
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
                format,
                size,
                pair: None,
            })
        }

        fn frame(&self, rgb: &[u8], at: CMTime) -> Result<Retained<VTFrameProcessorFrame>> {
            let picture = buffer(&self.sources, WHAT)?;
            write(&picture, self.format, self.size, rgb)?;
            frame(&picture, at, WHAT)
        }

        fn between(&mut self, key: &Key, a: &[u8], b: &[u8], t: f64) -> Result<Vec<u8>> {
            let mode = match &self.pair {
                Some(p) if p.key == *key => Mode::SequentialReferencesUnchanged,
                Some(p) if p.key.0 == key.0 && p.key.2 == key.1 => Mode::Sequential,
                _ => Mode::Random,
            };
            let clock = std::time::Instant::now();
            if mode != Mode::SequentialReferencesUnchanged {
                self.pair = Some(Pair {
                    key: key.clone(),
                    source: self.frame(a, time(key.1, 1))?,
                    next: self.frame(b, time(key.2, 1))?,
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
            let out = read(&made, self.format, self.size)?;
            crate::debug!(
                "interpolate",
                "macOS: pair in {:.1} ms, processed in {:.1} ms, read in {:.1} ms",
                wrote.as_secs_f64() * 1e3,
                (processed - wrote).as_secs_f64() * 1e3,
                (clock.elapsed() - processed).as_secs_f64() * 1e3
            );
            Ok(out)
        }
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
        a: &[u8],
        b: &[u8],
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

    pub(super) fn release() {
        *session() = None;
    }
}
