use vidcut_core::model::{Media, Project, SuperScale, Upscaler};
use vidcut_core::render::VideoPiece;

use crate::error::{Error, Result};

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

pub fn unfit(m: &Media) -> Option<&'static str> {
    let v = m.video_as_read()?;
    if v.hdr.is_some() {
        return Some("HDR pictures");
    }
    let (w, h) = if v.rotation % 180 == 90 {
        (v.height, v.width)
    } else {
        (v.width, v.height)
    };
    (w > MOST.0 || h > MOST.1).then_some("pictures over 1920x1080")
}

pub fn factors() -> Vec<u32> {
    imp::factors()
}

pub fn offers(s: SuperScale) -> bool {
    s.upscaler != Upscaler::Macos || s.factor.times().is_none_or(|t| factors().contains(&t))
}

pub fn get_ready(download: bool) -> Result<()> {
    imp::ready(download)
}

pub(crate) fn made_size(size: (u32, u32), shown: (u32, u32)) -> (u32, u32) {
    let need = (f64::from(shown.0) / f64::from(size.0)).min(f64::from(shown.1) / f64::from(size.1));
    let factors = imp::factors();
    let factor = factors
        .iter()
        .copied()
        .find(|f| f64::from(*f) >= need)
        .or_else(|| factors.last().copied())
        .unwrap_or(1);
    (size.0 * factor, size.1 * factor)
}

pub fn check<'a>(
    p: &Project,
    pieces: impl IntoIterator<Item = &'a VideoPiece>,
    download: bool,
) -> Result<()> {
    let mut media: Vec<(&str, SuperScale)> = Vec::new();
    for piece in pieces {
        for (layer, _) in piece.layers() {
            if let VideoPiece::Source {
                media: id,
                super_scale,
                ..
            } = layer
                && super_scale.ai()
                && super_scale.upscaler == Upscaler::Macos
                && !media.iter().any(|(m, _)| m == id)
            {
                media.push((id, *super_scale));
            }
        }
    }
    if media.is_empty() {
        return Ok(());
    }
    let refused: Vec<String> = media
        .iter()
        .filter_map(|(id, _)| p.media(id))
        .filter_map(|m| unfit(m).map(|why| format!("{} ({why})", m.name)))
        .collect();
    if !refused.is_empty() {
        return Err(Error::Invalid(format!(
            "the AI upscaler of macOS can't take {}: set their Upscaler to Classic, or their \
             Super Scale to Off",
            refused.join(", ")
        )));
    }
    get_ready(download)?;
    let wrong: Vec<String> = media
        .iter()
        .filter(|(_, s)| !offers(*s))
        .filter_map(|(id, s)| {
            p.media(id)
                .map(|m| format!("{} ({}x)", m.name, s.factor.times().unwrap_or(0)))
        })
        .collect();
    if !wrong.is_empty() {
        let offered: Vec<String> = factors().iter().map(|f| format!("{f}x")).collect();
        return Err(Error::Invalid(format!(
            "the AI upscaler of this Mac offers only {}: set the factor of {} to Auto or to one of \
             these",
            offered.join(", "),
            wrong.join(", ")
        )));
    }
    Ok(())
}

pub(crate) struct Run {
    scaler: imp::Scaler,
}

impl Run {
    pub(crate) fn open(size: (u32, u32), made: (u32, u32)) -> Result<Self> {
        Ok(Self {
            scaler: imp::Scaler::open(size, made)?,
        })
    }
}

impl super::Run for Run {
    fn push(&mut self, index: i64, pts: Option<i64>, rgb: Vec<u8>) -> Result<Vec<super::Out>> {
        let rgb = self.scaler.scale(index, &rgb)?;
        Ok(vec![super::Out { index, pts, rgb }])
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

        pub(super) fn scale(&mut self, _index: i64, _rgb: &[u8]) -> Result<Vec<u8>> {
            match self.0 {}
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use block2::RcBlock;
    use half::f16;
    use objc2::AllocAnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
    use objc2_core_foundation::{CFDictionary, CFRetained};
    use objc2_core_media::CMTime;
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRow, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferIsPlanar,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey,
        kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA, kCVPixelFormatType_64RGBAHalf,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
    };
    use objc2_foundation::{
        NSArray, NSDictionary, NSError, NSMutableDictionary, NSNumber, NSString,
    };
    use objc2_video_toolbox::{
        VTFrameProcessor, VTFrameProcessorFrame, VTSuperResolutionScalerConfiguration,
        VTSuperResolutionScalerConfigurationInputType,
        VTSuperResolutionScalerConfigurationModelStatus,
        VTSuperResolutionScalerConfigurationQualityPrioritization,
        VTSuperResolutionScalerParameters, VTSuperResolutionScalerParametersSubmissionMode,
    };
    use vidcut_core::model::Upscaler;

    use super::NEEDS;
    use crate::error::{Error, Result};

    const MISSING: &str = "macOS's super-resolution models aren't on this Mac yet: pass \
                           download (--download) to let macOS download them. macOS doesn't give \
                           their size";

    const BGRA: u32 = kCVPixelFormatType_32BGRA;
    const NV12: u32 = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
    const RGHA: u32 = kCVPixelFormatType_64RGBAHalf;

    fn available() -> bool {
        AnyClass::get(c"VTSuperResolutionScalerConfiguration").is_some()
            // SAFETY: a class method of a class that exists.
            && unsafe { VTSuperResolutionScalerConfiguration::isSupported() }
    }

    fn numbers(list: &NSArray<NSNumber>) -> Vec<u32> {
        (0..list.count())
            .map(|i| list.objectAtIndex(i).unsignedIntValue())
            .collect()
    }

    fn fourcc(format: u32) -> String {
        format
            .to_be_bytes()
            .iter()
            .map(|b| char::from(*b))
            .collect()
    }

    fn failed(e: Retained<NSError>) -> Error {
        Error::Invalid(format!(
            "macOS's super-resolution scaler: {}",
            e.localizedDescription()
        ))
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
        let formats = numbers(&list);
        [BGRA, NV12, RGHA]
            .into_iter()
            .find(|f| formats.contains(f))
            .ok_or_else(|| {
                let names: Vec<String> = formats.iter().map(|f| fourcc(*f)).collect();
                Error::Invalid(format!(
                    "macOS's super-resolution scaler takes none of vidcut's pixel formats, only {}",
                    names.join(", ")
                ))
            })
    }

    fn pool(
        (w, h): (u32, u32),
        format: u32,
        attributes: &NSDictionary<NSString, AnyObject>,
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
                "picture buffers for macOS's super-resolution scaler: CoreVideo error {status}"
            )));
        }
        let out = NonNull::new(out).ok_or_else(|| {
            Error::Invalid("no picture buffers for macOS's super-resolution scaler".into())
        })?;
        // SAFETY: CVPixelBufferPoolCreate gives the pool with one retain, which this takes.
        Ok(unsafe { CFRetained::from_raw(out) })
    }

    fn buffer(pool: &CVPixelBufferPool) -> Result<CFRetained<CVPixelBuffer>> {
        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: a live pool, and `out` is a valid place for the buffer.
        let status =
            unsafe { CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut out)) };
        if status != kCVReturnSuccess {
            return Err(Error::Invalid(format!(
                "a picture buffer for macOS's super-resolution scaler: CoreVideo error {status}"
            )));
        }
        let out = NonNull::new(out).ok_or_else(|| {
            Error::Invalid("no picture buffer for macOS's super-resolution scaler".into())
        })?;
        // SAFETY: CVPixelBufferPoolCreatePixelBuffer gives the buffer with one retain, which this takes.
        Ok(unsafe { CFRetained::from_raw(out) })
    }

    fn frame(buffer: &CVPixelBuffer, time: CMTime) -> Result<Retained<VTFrameProcessorFrame>> {
        // SAFETY: a live pixel buffer and a plain time.
        unsafe {
            VTFrameProcessorFrame::initWithBuffer_presentationTimeStamp(
                VTFrameProcessorFrame::alloc(),
                buffer,
                time,
            )
        }
        .ok_or_else(|| {
            Error::Invalid("macOS's super-resolution scaler refused a picture buffer".into())
        })
    }

    struct Locked<'a>(&'a CVPixelBuffer);

    impl<'a> Locked<'a> {
        fn new(buffer: &'a CVPixelBuffer) -> Result<Self> {
            // SAFETY: a live buffer, unlocked when this drops.
            let status = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags(0)) };
            if status != kCVReturnSuccess {
                return Err(Error::Invalid(format!(
                    "a picture buffer of macOS's super-resolution scaler: CoreVideo error {status}"
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

    fn write(buffer: &CVPixelBuffer, format: u32, (w, h): (u32, u32), rgb: &[u8]) -> Result<()> {
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

    fn read(buffer: &CVPixelBuffer, format: u32, (w, h): (u32, u32)) -> Result<Vec<u8>> {
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

    pub(super) struct Scaler {
        processor: Retained<VTFrameProcessor>,
        sources: CFRetained<CVPixelBufferPool>,
        outputs: CFRetained<CVPixelBufferPool>,
        format: u32,
        size: (u32, u32),
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
            let factor = made.0 / size.0;
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
            let sources = pool(size, format, &from)?;
            let outputs = pool(made, format, &to)?;
            // SAFETY: a new processor of the frame processor class.
            let processor = unsafe { VTFrameProcessor::new() };
            // SAFETY: a new processor and a live configuration.
            unsafe {
                processor.startSessionWithConfiguration_error(ProtocolObject::from_ref(&*config))
            }
            .map_err(failed)?;
            super::runs_on(Upscaler::Macos, "macOS".into());
            crate::debug!(
                "ai",
                "macOS: {}x{} to {}x{} at {factor}x, pictures as {}",
                size.0,
                size.1,
                made.0,
                made.1,
                fourcc(format)
            );
            Ok(Self {
                processor,
                sources,
                outputs,
                format,
                size,
                made,
                last: None,
            })
        }

        pub(super) fn scale(&mut self, index: i64, rgb: &[u8]) -> Result<Vec<u8>> {
            let clock = std::time::Instant::now();
            let source = buffer(&self.sources)?;
            write(&source, self.format, self.size, rgb)?;
            let enlarged = buffer(&self.outputs)?;
            let wrote = clock.elapsed();
            // SAFETY: a time of plain values.
            let time = unsafe { CMTime::new(index, 1) };
            let source = frame(&source, time)?;
            let destination = frame(&enlarged, time)?;
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
            let scaled = clock.elapsed();
            let out = read(&enlarged, self.format, self.made)?;
            crate::debug!(
                "ai",
                "macOS: frame {index}: in {:.1} ms, scaled in {:.1} ms, out in {:.1} ms",
                wrote.as_secs_f64() * 1e3,
                (scaled - wrote).as_secs_f64() * 1e3,
                (clock.elapsed() - scaled).as_secs_f64() * 1e3
            );
            self.last = Some((source, destination));
            Ok(out)
        }
    }

    impl Drop for Scaler {
        fn drop(&mut self) {
            // SAFETY: the session that `open` started.
            unsafe { self.processor.endSession() };
        }
    }
}
