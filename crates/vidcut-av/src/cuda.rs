//! The NVIDIA GPU as one pipeline: frames NVDEC decodes stay in GPU memory,
//! are scaled there (`scale_cuda`, after `bwdif_cuda` for interlaced
//! sources, then `pad_cuda`) and go to NVENC as they are. Pictures in
//! another matrix than BT.709 (BT.601 SD, VP8) or in full range are
//! converted first by libplacebo on Vulkan, on the same GPU. Proxies
//! (decision D41) and exports use it: the CPU only reads the file and
//! writes the result.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::ptr;
use std::sync::{Arc, OnceLock};

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, ffi, filter, format, frame};

/// A reference to one of FFmpeg's hardware device contexts.
struct Device(*mut ffi::AVBufferRef);

// SAFETY: a reference to a hardware device context may be used, and
// dropped, on any thread.
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Clone for Device {
    fn clone(&self) -> Self {
        Self(unsafe { ffi::av_buffer_ref(self.0) })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

/// A CUDA device: NVDEC, the CUDA filters and NVENC share it.
#[derive(Clone)]
pub struct CudaDevice {
    cuda: Device,
    /// The Vulkan device on the same GPU, for libplacebo: made the first
    /// time it is needed (that takes a quarter of a second), then shared
    /// by every clone.
    vulkan: Arc<OnceLock<std::result::Result<Device, String>>>,
}

impl CudaDevice {
    /// The first CUDA device, when there is one that works.
    pub fn open() -> Result<Self> {
        crate::init();
        let mut device = ptr::null_mut();
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut device,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
                ptr::null(),
                ptr::null_mut(),
                0,
            )
        };
        if ret < 0 || device.is_null() {
            return Err(Error::Invalid(format!(
                "no CUDA device: {}",
                ffmpeg::Error::from(ret)
            )));
        }
        Ok(Self {
            cuda: Device(device),
            vulkan: Arc::default(),
        })
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.cuda.0
    }

    fn vulkan(&self) -> Result<&Device> {
        self.vulkan
            .get_or_init(|| {
                let mut vulkan = ptr::null_mut();
                let ret = unsafe {
                    ffi::av_hwdevice_ctx_create_derived(
                        &mut vulkan,
                        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
                        self.as_ptr(),
                        0,
                    )
                };
                if ret < 0 || vulkan.is_null() {
                    Err(format!(
                        "no Vulkan device on the CUDA device's GPU: {}",
                        ffmpeg::Error::from(ret)
                    ))
                } else {
                    Ok(Device(vulkan))
                }
            })
            .as_ref()
            .map_err(|e| Error::Invalid(e.clone()))
    }
}

/// Frames in GPU memory of one size and format.
struct HwFrames(*mut ffi::AVBufferRef);

// SAFETY: as for the device.
unsafe impl Send for HwFrames {}

impl HwFrames {
    /// `width`×`height` frames of `sw` laid out as `hw` (CUDA or Vulkan)
    /// on `device`.
    fn new(
        device: *mut ffi::AVBufferRef,
        hw: ffi::AVPixelFormat,
        sw: ffi::AVPixelFormat,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(device);
            if frames.is_null() {
                return Err(Error::Invalid("no memory for GPU frames".into()));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = hw;
            fc.sw_format = sw;
            fc.width = width as i32;
            fc.height = height as i32;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                ffi::av_buffer_unref(&mut frames);
                return Err(Error::Ffmpeg {
                    context: "setting up GPU frames".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Self(frames))
        }
    }

    /// A frame of this pool holding the picture of `src` (in memory, or in
    /// GPU memory of another kind), no other properties.
    fn copy(&self, src: &frame::Video, what: &str) -> Result<frame::Video> {
        let mut hw = frame::Video::empty();
        unsafe {
            let ret = ffi::av_hwframe_get_buffer(self.0, hw.as_mut_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "getting a GPU frame".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let ret = ffi::av_hwframe_transfer_data(hw.as_mut_ptr(), src.as_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: what.into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
        }
        Ok(hw)
    }
}

impl Drop for HwFrames {
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

/// GPU frames of one size and format: what NVENC is opened with, and where
/// pictures made on the CPU go up to the GPU.
pub struct CudaFrames(HwFrames);

impl CudaFrames {
    /// `width`×`height` frames of `sw` (NV12 or P010) on `device`.
    pub fn new(device: &CudaDevice, sw: format::Pixel, width: u32, height: u32) -> Result<Self> {
        HwFrames::new(
            device.as_ptr(),
            ffi::AVPixelFormat::AV_PIX_FMT_CUDA,
            sw.into(),
            width,
            height,
        )
        .map(Self)
    }

    pub fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.0.0
    }

    /// `src` (in memory, of this pool's format and size) copied into a GPU
    /// frame, keeping its timestamp.
    pub fn upload(&self, src: &frame::Video) -> Result<frame::Video> {
        let mut hw = self.0.copy(src, "uploading to the GPU")?;
        hw.set_pts(src.pts());
        Ok(hw)
    }
}

/// Where libplacebo keeps the shaders it compiles (`keep_shaders_in`).
static SHADERS: OnceLock<PathBuf> = OnceLock::new();

/// Keeps libplacebo's compiled shaders in `dir`: compiling them takes a
/// tenth of a second for each piece of an export it converts, reading them
/// back a fifth of that. Without it they are compiled every time.
pub fn keep_shaders_in(dir: PathBuf) {
    let _ = SHADERS.set(dir);
}

/// The prefix libplacebo names its shader files with, once the folder is
/// there.
fn shader_cache() -> Option<String> {
    let dir = SHADERS.get()?;
    std::fs::create_dir_all(dir).ok()?;
    Some(format!("{}/", dir.to_str()?))
}

/// Pictures for NVENC from frames NVDEC decoded: deinterlaced when the
/// source is (one frame per frame, as the CPU reader makes them), scaled to
/// fit `width`×`height` keeping the display aspect and padded with black to
/// it, as the CPU's normalizer does, as `format` (8-bit NV12 or 10-bit
/// P010) in BT.709, square pixels, all in GPU memory. Timestamps stay the
/// source's.
pub struct CudaPictures {
    graph: filter::Graph,
    time_base: ffmpeg::Rational,
    placebo: Option<Placebo>,
}

/// BT.709 from another matrix or full range, at the source's size, ahead
/// of the CUDA filters (they change neither): each frame is copied to a
/// Vulkan frame, converted by libplacebo and copied back to a CUDA frame,
/// never leaving the GPU. libplacebo can't pad: it clears the border with
/// a copy that FFmpeg's Vulkan frames don't allow.
struct Placebo {
    graph: filter::Graph,
    input: HwFrames,
    output: HwFrames,
    /// Pushed after each frame when its picture is wanted at once.
    /// libplacebo gives out a frame's picture only once it has the frame
    /// after it; the filler's own pictures are dropped. Without it, a
    /// picture comes a frame late and the GPU renders half as many.
    filler: Option<frame::Video>,
    /// How the CPU reads the source: its matrix and range.
    matrix: ffi::AVColorSpace,
    range: ffi::AVColorRange,
    /// Frames are timed by count, two apart with the filler between, as
    /// libplacebo wants rising timestamps; their own wait here.
    count: i64,
    pts: VecDeque<(i64, Option<i64>)>,
}

/// The time base of the counted timestamps libplacebo sees: a second
/// apart, as libplacebo takes a frame within a millisecond of the time it
/// renders for as that time's frame, and may blend neighbors when pictures
/// come under a tenth of a second apart.
const COUNTED: ffi::AVRational = ffi::AVRational { num: 1, den: 1 };

/// The filters from `w`×`h` frames to `width`×`height` NV12 pictures.
/// `scale_cuda` samples a fixed few pixels around each output pixel, which
/// aliases when it shrinks a lot (4K to 960 wide): halvings first, each a
/// bilinear sample between four pixels, so their average, then one bicubic
/// step to the size.
fn filters(
    (w, h): (u32, u32),
    (fit_w, fit_h): (u32, u32),
    (width, height, out): (u32, u32, &str),
    deinterlace: bool,
) -> String {
    let mut steps = Vec::new();
    if deinterlace {
        steps.push("bwdif_cuda=mode=send_frame:parity=auto:deint=interlaced".to_string());
    }
    let (mut w, mut h) = (w, h);
    while w >= 2 * fit_w && h >= 2 * fit_h && w >= 4 && h >= 4 {
        (w, h) = ((w / 2) & !1, (h / 2) & !1);
        steps.push(format!("scale_cuda=w={w}:h={h}:interp_algo=bilinear"));
    }
    steps.push(format!(
        "scale_cuda=w={fit_w}:h={fit_h}:format={out}:interp_algo=bicubic"
    ));
    // Centered on black, as the CPU's normalizer pads.
    if (fit_w, fit_h) != (width, height) {
        steps.push(format!(
            "pad_cuda=w={width}:h={height}:x={}:y={}:color=black",
            (width - fit_w) / 2,
            (height - fit_h) / 2
        ));
    }
    steps.join(",")
}

/// libplacebo's options for `width`×`height` pictures of `out`, the size
/// they came in. Only the matrix and range change, as swscale converts SDR
/// on the CPU: the input is tagged with BT.709's primaries and transfer
/// too. Film grain and Dolby Vision metadata are left alone (the decoder
/// applied the grain).
fn placebo_options((width, height, out): (u32, u32, &str)) -> Vec<(&'static str, String)> {
    let mut options: Vec<(&str, String)> = vec![
        ("w", width.to_string()),
        ("h", height.to_string()),
        ("format", out.into()),
        ("colorspace", "bt709".into()),
        ("color_primaries", "bt709".into()),
        ("color_trc", "bt709".into()),
        ("range", "tv".into()),
        ("apply_filmgrain", "0".into()),
        ("apply_dolbyvision", "0".into()),
    ];
    if let Some(prefix) = shader_cache() {
        options.push(("shader_cache", prefix));
    }
    options
}

/// The buffer source `in` of `graph`, for `width`×`height` frames of
/// `pixel` in GPU memory from `frames`. It needs to know where they are
/// before it starts, so it is made by hand.
fn add_source(
    graph: &mut filter::Graph,
    pixel: ffi::AVPixelFormat,
    (width, height): (u32, u32),
    time_base: ffi::AVRational,
    sar: ffmpeg::Rational,
    frames: *mut ffi::AVBufferRef,
    color: Option<(ffi::AVColorSpace, ffi::AVColorRange)>,
) -> Result<()> {
    unsafe {
        let ctx = ffi::avfilter_graph_alloc_filter(
            graph.as_mut_ptr(),
            ffi::avfilter_get_by_name(c"buffer".as_ptr()),
            c"in".as_ptr(),
        );
        if ctx.is_null() {
            return Err(Error::Invalid("no memory for the GPU source".into()));
        }
        let par = ffi::av_buffersrc_parameters_alloc();
        if par.is_null() {
            return Err(Error::Invalid("no memory for the GPU source".into()));
        }
        (*par).format = pixel as i32;
        (*par).width = width as i32;
        (*par).height = height as i32;
        (*par).time_base = time_base;
        (*par).sample_aspect_ratio = ffi::AVRational {
            num: sar.numerator().max(1),
            den: sar.denominator().max(1),
        };
        // The source takes a reference of its own.
        (*par).hw_frames_ctx = frames;
        if let Some((matrix, range)) = color {
            (*par).color_space = matrix;
            (*par).color_range = range;
        }
        let set = ffi::av_buffersrc_parameters_set(ctx, par);
        ffi::av_free(par.cast());
        let ret = if set < 0 {
            set
        } else {
            ffi::avfilter_init_str(ctx, ptr::null())
        };
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "creating the GPU source".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
    }
    Ok(())
}

/// The buffer sink `out` of `graph`.
fn add_sink(graph: &mut filter::Graph) -> Result<()> {
    let sink = filter::find("buffersink").ok_or_else(|| Error::Invalid("no buffersink".into()))?;
    graph
        .add(&sink, "out", "")
        .ctx(|| "creating the GPU sink".into())?;
    Ok(())
}

/// The next frame out of `graph`'s sink, if one is ready.
fn take(graph: &mut filter::Graph, what: &str) -> Result<Option<(frame::Video, ffmpeg::Rational)>> {
    let mut sink = graph.get("out").expect("sink");
    let mut out = frame::Video::empty();
    match sink.sink().frame(&mut out) {
        Ok(()) => Ok(Some((out, sink.sink().time_base()))),
        Err(ffmpeg::Error::Eof)
        | Err(ffmpeg::Error::Other {
            errno: ffmpeg::util::error::EAGAIN,
        }) => Ok(None),
        Err(e) => Err(Error::Ffmpeg {
            context: what.into(),
            source: e,
        }),
    }
}

impl CudaPictures {
    /// The pictures for frames like `first` (decoded on `device`), timed in
    /// `time_base`. `at_once`: each frame's picture is ready as soon as the
    /// frame is in, as an export picks them one by one; otherwise one may
    /// come with the next frame, leaving the GPU less to do.
    pub fn new(
        device: &CudaDevice,
        first: &frame::Video,
        time_base: ffmpeg::Rational,
        (width, height): (u32, u32),
        format: format::Pixel,
        deinterlace: bool,
        at_once: bool,
    ) -> Result<Self> {
        if first.format() != format::Pixel::CUDA {
            return Err(Error::Invalid("not frames in GPU memory".into()));
        }
        let (matrix, range) = crate::color::sdr_yuv(first)
            .ok_or_else(|| Error::Invalid("HDR pictures need tone mapping".into()))?;
        let out = match format {
            format::Pixel::P010LE => "p010le",
            _ => "nv12",
        };
        let sar = first.aspect_ratio();
        let fitted = crate::export::video::fit(
            first.width(),
            first.height(),
            (sar.numerator(), sar.denominator()),
            width,
            height,
        );
        if fitted != (width, height) && format == format::Pixel::P010LE {
            return Err(Error::Invalid(
                "FFmpeg's pad_cuda takes no 10-bit frames".into(),
            ));
        }
        let size = (first.width(), first.height());
        let decoded = unsafe { (*first.as_ptr()).hw_frames_ctx };
        if decoded.is_null() {
            return Err(Error::Invalid(
                "decoded frames without a frames context".into(),
            ));
        }
        let bt709 = matrix == ffi::AVColorSpace::AVCOL_SPC_BT709
            && range == ffi::AVColorRange::AVCOL_RANGE_MPEG;
        let placebo = if bt709 {
            None
        } else if deinterlace {
            return Err(Error::Invalid(
                "no deinterlacer before libplacebo on the GPU".into(),
            ));
        } else {
            let color = (matrix, range);
            Some(Placebo::new(device, decoded, size, sar, color, at_once)?)
        };
        let frames = placebo.as_ref().map_or(decoded, |p| p.output.0);
        let mut graph = filter::Graph::new();
        add_source(
            &mut graph,
            ffi::AVPixelFormat::AV_PIX_FMT_CUDA,
            size,
            time_base.into(),
            sar,
            frames,
            None,
        )?;
        add_sink(&mut graph)?;
        let spec = filters(size, fitted, (width, height, out), deinterlace);
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse(&spec))
            .ctx(|| format!("building {spec}"))?;
        graph.validate().ctx(|| format!("configuring {spec}"))?;
        Ok(Self {
            graph,
            time_base,
            placebo,
        })
    }

    /// The frames context of the pictures: what NVENC is opened with.
    pub fn frames_ctx(&mut self) -> *mut ffi::AVBufferRef {
        unsafe {
            ffi::av_buffersink_get_hw_frames_ctx(self.graph.get("out").expect("sink").as_mut_ptr())
        }
    }

    /// Feeds the next decoded frame, or the end of the stream (`None`).
    pub fn push(&mut self, f: Option<&frame::Video>) -> Result<()> {
        let mut source = self.graph.get("in").expect("source");
        let Some(p) = &mut self.placebo else {
            return match f {
                Some(f) => source.source().add(f).ctx(|| "scaling on the GPU".into()),
                None => source
                    .source()
                    .flush()
                    .ctx(|| "flushing the GPU scaler".into()),
            };
        };
        p.push(f)?;
        while let Some(converted) = p.pull()? {
            source
                .source()
                .add(&converted)
                .ctx(|| "scaling on the GPU".into())?;
        }
        if f.is_none() {
            source
                .source()
                .flush()
                .ctx(|| "flushing the GPU scaler".into())?;
        }
        Ok(())
    }

    /// The next picture, if one is ready: timed in the source's time base.
    pub fn pull(&mut self) -> Result<Option<frame::Video>> {
        let Some((mut out, tb)) = take(&mut self.graph, "scaling on the GPU")? else {
            return Ok(None);
        };
        // bwdif halves the time base (for field rates): back to the
        // stream's.
        let pts = out
            .pts()
            .map(|p| unsafe { ffi::av_rescale_q(p, tb.into(), self.time_base.into()) });
        out.set_pts(pts);
        unsafe {
            (*out.as_mut_ptr()).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
        }
        Ok(Some(out))
    }
}

impl Placebo {
    /// The converter for frames of `decoded` (a CUDA frames context) of
    /// `size`, read with `matrix` and `range`.
    fn new(
        device: &CudaDevice,
        decoded: *mut ffi::AVBufferRef,
        (width, height): (u32, u32),
        sar: ffmpeg::Rational,
        (matrix, range): (ffi::AVColorSpace, ffi::AVColorRange),
        at_once: bool,
    ) -> Result<Self> {
        let vulkan = device.vulkan()?;
        let sw = unsafe { (*((*decoded).data as *mut ffi::AVHWFramesContext)).sw_format };
        let deep = format::Pixel::from(sw)
            .descriptor()
            .is_some_and(|d| unsafe { (*d.as_ptr()).comp[0].depth } > 8);
        let (out, out_sw) = if deep {
            ("p010le", ffi::AVPixelFormat::AV_PIX_FMT_P010LE)
        } else {
            ("nv12", ffi::AVPixelFormat::AV_PIX_FMT_NV12)
        };
        let input = HwFrames::new(
            vulkan.0,
            ffi::AVPixelFormat::AV_PIX_FMT_VULKAN,
            sw,
            width,
            height,
        )?;
        let output = HwFrames::new(
            device.as_ptr(),
            ffi::AVPixelFormat::AV_PIX_FMT_CUDA,
            out_sw,
            width,
            height,
        )?;
        let mut graph = filter::Graph::new();
        add_source(
            &mut graph,
            ffi::AVPixelFormat::AV_PIX_FMT_VULKAN,
            (width, height),
            COUNTED,
            sar,
            input.0,
            Some((matrix, range)),
        )?;
        add_sink(&mut graph)?;
        let options = placebo_options((width, height, out));
        // libplacebo needs the Vulkan device before it starts; given none,
        // FFmpeg would convert on the CPU in front of it.
        unsafe {
            let ctx = ffi::avfilter_graph_alloc_filter(
                graph.as_mut_ptr(),
                ffi::avfilter_get_by_name(c"libplacebo".as_ptr()),
                c"placebo".as_ptr(),
            );
            if ctx.is_null() {
                return Err(Error::Invalid("no libplacebo filter".into()));
            }
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(vulkan.0);
            let mut dict = ffmpeg::Dictionary::new();
            for (k, v) in &options {
                dict.set(k, v);
            }
            let mut raw = dict.disown();
            let ret = ffi::avfilter_init_dict(ctx, &mut raw);
            ffi::av_dict_free(&mut raw);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "starting libplacebo".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let src = graph.get("in").expect("source").as_mut_ptr();
            let sink = graph.get("out").expect("sink").as_mut_ptr();
            let ret = ffi::avfilter_link(src, 0, ctx, 0);
            let ret = if ret < 0 {
                ret
            } else {
                ffi::avfilter_link(ctx, 0, sink, 0)
            };
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "linking libplacebo".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
        }
        graph.validate().ctx(|| "configuring libplacebo".into())?;
        let filler = if at_once {
            // Any picture will do: a black one.
            let mut black = frame::Video::new(sw.into(), width, height);
            crate::export::video::fill_black(&mut black)?;
            let mut filler = input.copy(&black, "copying to Vulkan")?;
            tag(&mut filler, (matrix, range));
            Some(filler)
        } else {
            None
        };
        Ok(Self {
            graph,
            input,
            output,
            filler,
            matrix,
            range,
            count: 0,
            pts: VecDeque::new(),
        })
    }

    /// Feeds `f` (a CUDA frame), or the end of the stream (`None`).
    fn push(&mut self, f: Option<&frame::Video>) -> Result<()> {
        let mut source = self.graph.get("in").expect("source");
        let Some(f) = f else {
            return source.source().flush().ctx(|| "flushing libplacebo".into());
        };
        let mut v = self.input.copy(f, "copying to Vulkan")?;
        unsafe { ffi::av_frame_copy_props(v.as_mut_ptr(), f.as_ptr()) };
        tag(&mut v, (self.matrix, self.range));
        let n = 2 * self.count;
        self.count += 1;
        v.set_pts(Some(n));
        self.pts.push_back((n, f.pts()));
        source
            .source()
            .add(&v)
            .ctx(|| "converting on the GPU".into())?;
        if let Some(filler) = &self.filler {
            let mut next = frame::Video::empty();
            unsafe { ffi::av_frame_ref(next.as_mut_ptr(), filler.as_ptr()) };
            next.set_pts(Some(n + 1));
            source
                .source()
                .add(&next)
                .ctx(|| "converting on the GPU".into())?;
        }
        Ok(())
    }

    /// The next converted frame, if one is ready, with its properties and
    /// timestamp, in a CUDA frame.
    fn pull(&mut self) -> Result<Option<frame::Video>> {
        while let Some((out, tb)) = take(&mut self.graph, "converting on the GPU")? {
            let k = out
                .pts()
                .map_or(-1, |k| unsafe { ffi::av_rescale_q(k, tb.into(), COUNTED) });
            if k % 2 != 0 {
                // The filler's.
                continue;
            }
            let mut pts = None;
            while let Some(&(j, t)) = self.pts.front()
                && j <= k
            {
                self.pts.pop_front();
                pts = t;
            }
            let mut c = self.output.copy(&out, "copying back from Vulkan")?;
            unsafe { ffi::av_frame_copy_props(c.as_mut_ptr(), out.as_ptr()) };
            c.set_pts(pts);
            return Ok(Some(c));
        }
        Ok(None)
    }
}

/// Tags a frame for libplacebo: read with the source's matrix and range,
/// in BT.709's primaries and transfer, one count long.
fn tag(f: &mut frame::Video, (matrix, range): (ffi::AVColorSpace, ffi::AVColorRange)) {
    unsafe {
        let f = &mut *f.as_mut_ptr();
        f.colorspace = matrix;
        f.color_range = range;
        f.color_primaries = ffi::AVColorPrimaries::AVCOL_PRI_BT709;
        f.color_trc = ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
        f.duration = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{filters, placebo_options};

    #[test]
    fn big_shrinks_halve_first() {
        assert_eq!(
            filters((3840, 2160), (960, 540), (960, 540, "nv12"), false),
            "scale_cuda=w=1920:h=1080:interp_algo=bilinear,\
             scale_cuda=w=960:h=540:interp_algo=bilinear,\
             scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic"
        );
        assert_eq!(
            filters((1280, 720), (960, 540), (960, 540, "nv12"), true),
            "bwdif_cuda=mode=send_frame:parity=auto:deint=interlaced,\
             scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic"
        );
        // A 2.4:1 film in 16:9: fitted, then centered on black.
        assert_eq!(
            filters((1920, 800), (1920, 800), (1920, 1080, "nv12"), false),
            "scale_cuda=w=1920:h=800:format=nv12:interp_algo=bicubic,\
             pad_cuda=w=1920:h=1080:x=0:y=140:color=black"
        );
    }

    #[test]
    fn libplacebo_changes_only_the_colors() {
        let options = placebo_options((720, 480, "nv12"));
        let get = |k: &str| {
            options
                .iter()
                .find(|(o, _)| *o == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!((get("w"), get("h")), (Some("720"), Some("480")));
        assert_eq!(get("format"), Some("nv12"));
        assert_eq!(get("colorspace"), Some("bt709"));
        assert_eq!(get("color_primaries"), Some("bt709"));
        assert_eq!(get("range"), Some("tv"));
    }
}
