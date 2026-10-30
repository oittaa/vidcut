use vidcut_core::model::{Hdr, Media, Project, Upscaler};
use vidcut_core::render::VideoPiece;

use crate::error::{Error, Result};
use crate::ffmpeg::frame::Video;
use crate::route::Place;

pub const MOST: (u32, u32) = (1920, 1080);

const NEEDS: &str = "the AI upscaler of macOS needs macOS 26 or later";

const TITLE: &str = "macOS's super-resolution models";

pub fn needed() -> Vec<crate::models::Need> {
    if !imp::missing() {
        return Vec::new();
    }
    vec![crate::models::Need {
        title: TITLE.into(),
        bytes: None,
        system: true,
    }]
}

pub fn unfit(m: &Media, timeline: Option<Hdr>) -> Option<&'static str> {
    let v = m.video_as_read()?;
    if super::takes_hdr(m, timeline)? {
        return Some(super::HDR_IN_HDR);
    }
    let (w, h) = if v.rotation % 180 == 90 {
        (v.height, v.width)
    } else {
        (v.width, v.height)
    };
    (w > MOST.0 || h > MOST.1).then_some("pictures over 1920x1080")
}

pub fn get_ready(download: bool) -> Result<()> {
    imp::ready(download)
}

pub(super) fn factor(size: (u32, u32), shown: (u32, u32)) -> u32 {
    let need = super::need(size, shown);
    let factors = imp::factors();
    factors
        .iter()
        .copied()
        .find(|f| f64::from(*f) >= need)
        .or_else(|| factors.last().copied())
        .unwrap_or(1)
}

pub fn check<'a>(
    p: &Project,
    pieces: impl IntoIterator<Item = &'a VideoPiece>,
    timeline: Option<Hdr>,
    download: bool,
) -> Result<()> {
    let mut media: Vec<&str> = Vec::new();
    for piece in pieces {
        for (layer, _) in piece.layers() {
            if let VideoPiece::Source {
                media: id,
                super_scale,
                ..
            } = layer
                && super_scale.ai()
                && super_scale.upscaler == Upscaler::Macos
                && !media.contains(&id.as_str())
            {
                media.push(id);
            }
        }
    }
    if media.is_empty() {
        return Ok(());
    }
    let refused: Vec<String> = media
        .iter()
        .filter_map(|id| p.media(id))
        .filter_map(|m| unfit(m, timeline).map(|why| format!("{} ({why})", m.name)))
        .collect();
    if !refused.is_empty() {
        return Err(Error::Invalid(format!(
            "the AI upscaler of macOS can't take {}: set their Upscaler to Classic, or their \
             Super Scale to Off",
            refused.join(", ")
        )));
    }
    get_ready(download)
}

pub(crate) struct Run {
    scaler: imp::Scaler,
    turner: Option<crate::hw::Turner>,
}

impl Run {
    pub(crate) fn open(size: (u32, u32), made: (u32, u32)) -> Result<Self> {
        Ok(Self {
            scaler: imp::Scaler::open(size, made)?,
            turner: None,
        })
    }
}

impl super::Run for Run {
    fn push(&mut self, index: i64, pts: Option<i64>, rgb: &Video) -> Result<Vec<super::Out>> {
        let deep = crate::screen::held::deep(rgb);
        let rgb = self
            .scaler
            .scale(index, (&crate::screen::held::rgb(rgb)?, deep))?;
        Ok(vec![super::Out {
            index,
            pts,
            picture: super::Picture::Rgb(rgb),
        }])
    }

    fn takes(&self, f: &Video, _rotation: u16) -> bool {
        Place::of(f) == Place::VideoToolbox && crate::color::Source::of(f).hdr.is_none()
    }

    fn push_frame(&mut self, index: i64, f: &Video, rotation: u16) -> Result<Vec<super::Out>> {
        let upright = crate::hw::Turner::turn(&mut self.turner, f, rotation)?;
        let made = self.scaler.scale_frame(index, &upright)?;
        Ok(vec![super::Out {
            index,
            pts: f.pts(),
            picture: super::Picture::Frame(made),
        }])
    }

    fn finish(&mut self) -> Result<Vec<super::Out>> {
        Ok(Vec::new())
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::NEEDS;
    use crate::error::{Error, Result};

    pub(super) fn factors() -> Vec<u32> {
        Vec::new()
    }

    pub(super) fn missing() -> bool {
        false
    }

    pub(super) fn ready(_download: bool) -> Result<()> {
        Err(Error::Invalid(NEEDS.into()))
    }

    pub(super) struct Scaler(std::convert::Infallible);

    impl Scaler {
        pub(super) fn open(_size: (u32, u32), _made: (u32, u32)) -> Result<Self> {
            Err(Error::Invalid(NEEDS.into()))
        }

        pub(super) fn scale(&mut self, _index: i64, _rgb: (&[u8], bool)) -> Result<Vec<u8>> {
            match self.0 {}
        }

        pub(super) fn scale_frame(
            &mut self,
            _index: i64,
            _f: &crate::ffmpeg::frame::Video,
        ) -> Result<crate::ffmpeg::frame::Video> {
            match self.0 {}
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::OnceLock;

    use block2::RcBlock;
    use objc2::AllocAnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, ProtocolObject};
    use objc2_core_foundation::CFRetained;
    use objc2_core_media::CMTime;
    use objc2_core_video::{CVPixelBuffer, CVPixelBufferPool};
    use objc2_foundation::NSError;
    use objc2_video_toolbox::{
        VTFrameProcessor, VTFrameProcessorFrame, VTSuperResolutionScalerConfiguration,
        VTSuperResolutionScalerConfigurationInputType,
        VTSuperResolutionScalerConfigurationModelStatus,
        VTSuperResolutionScalerConfigurationQualityPrioritization,
        VTSuperResolutionScalerParameters, VTSuperResolutionScalerParametersSubmissionMode,
    };
    use vidcut_core::model::Upscaler;

    use super::NEEDS;
    use crate::color::Matrix;
    use crate::error::{Error, Result};
    use crate::ffmpeg::{ffi, format, frame::Video};
    use crate::frame_processor::{
        Frames, Transfer, buffer, format_in, fourcc, frame, numbers, pixels, pool, read, write,
    };

    const MISSING: &str = "macOS's super-resolution models aren't on this Mac yet: pass \
                           download (--download) to let macOS download them. macOS doesn't give \
                           their size";

    const WHAT: &str = "macOS's super-resolution scaler";

    fn available() -> bool {
        AnyClass::get(c"VTSuperResolutionScalerConfiguration").is_some()
            // SAFETY: a class method of a class that exists.
            && unsafe { VTSuperResolutionScalerConfiguration::isSupported() }
    }

    fn failed(e: Retained<NSError>) -> Error {
        crate::frame_processor::failed(WHAT, e)
    }

    pub(super) fn factors() -> Vec<u32> {
        static FACTORS: OnceLock<Vec<u32>> = OnceLock::new();
        FACTORS
            .get_or_init(|| {
                if !available() {
                    return Vec::new();
                }
                // SAFETY: a class method of a class that exists.
                let list = unsafe { VTSuperResolutionScalerConfiguration::supportedScaleFactors() };
                let mut out = numbers(&list);
                out.sort_unstable();
                out.dedup();
                out
            })
            .clone()
    }

    fn configuration(
        size: (u32, u32),
        factor: u32,
    ) -> Result<Retained<VTSuperResolutionScalerConfiguration>> {
        if !available() {
            return Err(Error::Invalid(NEEDS.into()));
        }
        // SAFETY: an initializer of plain values; it gives nil for what it can't take.
        let made = unsafe {
            VTSuperResolutionScalerConfiguration::initWithFrameWidth_frameHeight_scaleFactor_inputType_usePrecomputedFlow_qualityPrioritization_revision(
                VTSuperResolutionScalerConfiguration::alloc(),
                size.0 as isize,
                size.1 as isize,
                factor as isize,
                VTSuperResolutionScalerConfigurationInputType::Video,
                false,
                VTSuperResolutionScalerConfigurationQualityPrioritization::Normal,
                VTSuperResolutionScalerConfiguration::defaultRevision(),
            )
        };
        made.ok_or_else(|| {
            Error::Invalid(format!(
                "macOS's super-resolution scaler can't take {}x{} at {factor}x",
                size.0, size.1
            ))
        })
    }

    fn has_model(config: &VTSuperResolutionScalerConfiguration) -> bool {
        // SAFETY: a property of a live configuration.
        let status = unsafe { config.configurationModelStatus() };
        status == VTSuperResolutionScalerConfigurationModelStatus::Ready
    }

    fn each_factor() -> Result<Vec<Retained<VTSuperResolutionScalerConfiguration>>> {
        let factors = factors();
        if factors.is_empty() {
            return Err(Error::Invalid(NEEDS.into()));
        }
        factors
            .into_iter()
            .map(|f| configuration((1280, 720), f))
            .collect()
    }

    pub(super) fn missing() -> bool {
        each_factor().is_ok_and(|all| all.iter().any(|c| !has_model(c)))
    }

    fn download(config: &VTSuperResolutionScalerConfiguration) -> Result<()> {
        crate::debug!("ai", "macOS: downloads its super-resolution model");
        let (tx, rx) = std::sync::mpsc::channel();
        let done = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(error.is_null());
        });
        // SAFETY: a live configuration, and a block that macOS copies.
        unsafe { config.downloadConfigurationModelWithCompletionHandler(&done) };
        match rx.recv() {
            Ok(true) => Ok(()),
            _ => Err(Error::Invalid(
                "macOS could not download its super-resolution model".into(),
            )),
        }
    }

    pub(super) fn ready(allowed: bool) -> Result<()> {
        for config in each_factor()? {
            if has_model(&config) {
                continue;
            }
            if !allowed {
                return Err(Error::Missing(MISSING.into()));
            }
            download(&config)?;
        }
        Ok(())
    }

    fn format_of(config: &VTSuperResolutionScalerConfiguration) -> Result<u32> {
        // SAFETY: a property of a live configuration.
        let list = unsafe { config.frameSupportedPixelFormats() };
        format_in(&list, WHAT)
    }

    pub(super) struct Scaler {
        processor: Retained<VTFrameProcessor>,
        sources: CFRetained<CVPixelBufferPool>,
        outputs: CFRetained<CVPixelBufferPool>,
        shrunk: Option<CFRetained<CVPixelBufferPool>>,
        transfer: Transfer,
        lanczos: Option<crate::lanczos::Lanczos>,
        shrink: Option<crate::lanczos::Lanczos>,
        frames: Frames,
        format: u32,
        size: (u32, u32),
        enlarged: (u32, u32),
        made: (u32, u32),
        last: Option<(
            Retained<VTFrameProcessorFrame>,
            Retained<VTFrameProcessorFrame>,
        )>,
    }

    // SAFETY: one thread at a time uses a scaler; an AI run moves between threads but is never shared.
    unsafe impl Send for Scaler {}

    impl Scaler {
        pub(super) fn open(size: (u32, u32), made: (u32, u32)) -> Result<Self> {
            let factor = super::factor(size, made);
            let enlarged = (size.0 * factor, size.1 * factor);
            let config = configuration(size, factor)?;
            if !has_model(&config) {
                return Err(Error::Missing(MISSING.into()));
            }
            let format = format_of(&config)?;
            // SAFETY: properties of a live configuration.
            let (from, to) = unsafe {
                (
                    config.sourcePixelBufferAttributes(),
                    config.destinationPixelBufferAttributes(),
                )
            };
            let sources = pool(size, format, &from, WHAT)?;
            let outputs = pool(enlarged, format, &to, WHAT)?;
            let shrunk = (made != enlarged)
                .then(|| pool(made, format, &to, WHAT))
                .transpose()?;
            // SAFETY: a new processor of the frame processor class.
            let processor = unsafe { VTFrameProcessor::new() };
            // SAFETY: a new processor and a live configuration.
            unsafe {
                processor.startSessionWithConfiguration_error(ProtocolObject::from_ref(&*config))
            }
            .map_err(failed)?;
            crate::super_scale::runs_on(Upscaler::Macos, "macOS".into());
            crate::debug!(
                "ai",
                "macOS: {}x{} to {}x{} at {factor}x, then to {}x{}, pictures as {}",
                size.0,
                size.1,
                enlarged.0,
                enlarged.1,
                made.0,
                made.1,
                fourcc(format)
            );
            Ok(Self {
                processor,
                sources,
                outputs,
                shrunk,
                transfer: Transfer::new(WHAT)?,
                lanczos: None,
                shrink: None,
                frames: Frames::default(),
                format,
                size,
                enlarged,
                made,
                last: None,
            })
        }

        pub(super) fn scale(&mut self, index: i64, (rgb, deep): (&[u8], bool)) -> Result<Vec<u8>> {
            let clock = std::time::Instant::now();
            let source = buffer(&self.sources, WHAT)?;
            write(&source, self.format, self.size, (rgb, deep))?;
            let wrote = clock.elapsed();
            let enlarged = self.enlarge(index, &source)?;
            let scaled = clock.elapsed();
            let out = match &self.shrunk {
                Some(shrunk) => {
                    let small = buffer(shrunk, WHAT)?;
                    self.transfer.run(&enlarged, &small, Matrix::Bt709, WHAT)?;
                    enlarged.remove_all_attachments();
                    read(&small, self.format, self.made, deep)?
                }
                None => read(&enlarged, self.format, self.made, deep)?,
            };
            told(index, wrote, scaled, clock);
            Ok(out)
        }

        pub(super) fn scale_frame(&mut self, index: i64, f: &Video) -> Result<Video> {
            let clock = std::time::Instant::now();
            let source = buffer(&self.sources, WHAT)?;
            let scaled = crate::lanczos::sized(&mut self.lanczos, f, self.size)?;
            let f = scaled.as_ref().unwrap_or(f);
            let matrix = crate::color::colors_of(f).matrix;
            self.transfer.run(pixels(f, WHAT)?, &source, matrix, WHAT)?;
            source.remove_all_attachments();
            let wrote = clock.elapsed();
            let enlarged = self.enlarge(index, &source)?;
            let scaled = clock.elapsed();
            let layout = crate::hw::sw_format(f);
            let layout = if crate::lanczos::takes(layout) {
                layout
            } else {
                format::Pixel::P416LE
            };
            let shape = (self.enlarged, ffi::AVPixelFormat::from(layout));
            let big = self
                .frames
                .made_as(&self.transfer, &enlarged, f, shape, WHAT)?;
            enlarged.remove_all_attachments();
            let out = match crate::lanczos::sized(&mut self.shrink, &big, self.made)? {
                Some(small) => small,
                None => big,
            };
            told(index, wrote, scaled, clock);
            Ok(out)
        }

        fn enlarge(
            &mut self,
            index: i64,
            source: &CVPixelBuffer,
        ) -> Result<CFRetained<CVPixelBuffer>> {
            let enlarged = buffer(&self.outputs, WHAT)?;
            // SAFETY: a time of plain values.
            let time = unsafe { CMTime::new(index, 1) };
            let source = frame(source, time, WHAT)?;
            let destination = frame(&enlarged, time, WHAT)?;
            let (previous, previous_out) = match &self.last {
                Some((s, d)) => (Some(&**s), Some(&**d)),
                None => (None, None),
            };
            // SAFETY: live frames of buffers made with the configuration's attributes.
            let parameters = unsafe {
                VTSuperResolutionScalerParameters::initWithSourceFrame_previousFrame_previousOutputFrame_opticalFlow_submissionMode_destinationFrame(
                    VTSuperResolutionScalerParameters::alloc(),
                    &source,
                    previous,
                    previous_out,
                    None,
                    VTSuperResolutionScalerParametersSubmissionMode::Sequential,
                    &destination,
                )
            }
            .ok_or_else(|| {
                Error::Invalid("macOS's super-resolution scaler refused a frame".into())
            })?;
            // SAFETY: a started session and live parameters.
            unsafe {
                self.processor
                    .processWithParameters_error(ProtocolObject::from_ref(&*parameters))
            }
            .map_err(failed)?;
            self.last = Some((source, destination));
            Ok(enlarged)
        }
    }

    fn told(
        index: i64,
        wrote: std::time::Duration,
        scaled: std::time::Duration,
        clock: std::time::Instant,
    ) {
        crate::debug!(
            "ai",
            "macOS: frame {index}: in {:.1} ms, scaled in {:.1} ms, out in {:.1} ms",
            wrote.as_secs_f64() * 1e3,
            (scaled - wrote).as_secs_f64() * 1e3,
            (clock.elapsed() - scaled).as_secs_f64() * 1e3
        );
    }

    impl Drop for Scaler {
        fn drop(&mut self) {
            // SAFETY: the session that `open` started.
            unsafe { self.processor.endSession() };
        }
    }
}
