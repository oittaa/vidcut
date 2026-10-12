//! The NVIDIA GPU as one pipeline: frames NVDEC decodes stay in GPU memory,
//! are scaled there (`scale_cuda`, after `bwdif_cuda` for interlaced
//! sources) and go to NVENC as they are. Proxies use it (decision D41):
//! the CPU only reads the file and writes the result.

use std::ptr;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, ffi, filter, format, frame};

/// A CUDA device: NVDEC, the CUDA filters and NVENC share it.
pub struct CudaDevice(*mut ffi::AVBufferRef);

// SAFETY: a reference to a hardware device context may be used, and
// dropped, on any thread.
unsafe impl Send for CudaDevice {}

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
        Ok(Self(device))
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.0
    }
}

impl Clone for CudaDevice {
    fn clone(&self) -> Self {
        Self(unsafe { ffi::av_buffer_ref(self.0) })
    }
}

impl Drop for CudaDevice {
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

/// GPU frames of one size and format: what NVENC is opened with, and where
/// pictures made on the CPU go up to the GPU.
pub struct CudaFrames(*mut ffi::AVBufferRef);

// SAFETY: as for the device.
unsafe impl Send for CudaFrames {}

impl CudaFrames {
    /// `width`×`height` frames of `sw` (NV12 or P010) on `device`.
    pub fn new(device: &CudaDevice, sw: format::Pixel, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(device.as_ptr());
            if frames.is_null() {
                return Err(Error::Invalid("no memory for GPU frames".into()));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = ffi::AVPixelFormat::AV_PIX_FMT_CUDA;
            fc.sw_format = sw.into();
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

    pub fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.0
    }

    /// `src` (in memory, of this pool's format and size) copied into a GPU
    /// frame, keeping its timestamp.
    pub fn upload(&self, src: &frame::Video) -> Result<frame::Video> {
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
                    context: "uploading to the GPU".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
        }
        hw.set_pts(src.pts());
        Ok(hw)
    }
}

impl Drop for CudaFrames {
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

/// Pictures for NVENC from frames NVDEC decoded: deinterlaced when the
/// source is (one frame per frame, as the CPU reader makes them), scaled to
/// `width`×`height` as `format` (8-bit NV12 or 10-bit P010), square
/// pixels, all in GPU memory. Timestamps stay the source's.
pub struct CudaPictures {
    graph: filter::Graph,
    time_base: ffmpeg::Rational,
}

/// The filters from `w`×`h` frames to `width`×`height` NV12 pictures.
/// `scale_cuda` samples a fixed few pixels around each output pixel, which
/// aliases when it shrinks a lot (4K to 960 wide): halvings first, each a
/// bilinear sample between four pixels, so their average, then one bicubic
/// step to the size.
fn filters(w: u32, h: u32, (width, height, out): (u32, u32, &str), deinterlace: bool) -> String {
    let mut steps = Vec::new();
    if deinterlace {
        steps.push("bwdif_cuda=mode=send_frame:parity=auto:deint=interlaced".to_string());
    }
    let (mut w, mut h) = (w, h);
    while w >= 2 * width && h >= 2 * height && w >= 4 && h >= 4 {
        (w, h) = ((w / 2) & !1, (h / 2) & !1);
        steps.push(format!("scale_cuda=w={w}:h={h}:interp_algo=bilinear"));
    }
    steps.push(format!(
        "scale_cuda=w={width}:h={height}:format={out}:interp_algo=bicubic"
    ));
    steps.join(",")
}

impl CudaPictures {
    /// The graph for frames like `first`, timed in `time_base`.
    pub fn new(
        first: &frame::Video,
        time_base: ffmpeg::Rational,
        (width, height): (u32, u32),
        format: format::Pixel,
        deinterlace: bool,
    ) -> Result<Self> {
        if first.format() != format::Pixel::CUDA {
            return Err(Error::Invalid("not frames in GPU memory".into()));
        }
        let find = |name: &str| {
            filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")))
        };
        let mut graph = filter::Graph::new();
        // The source needs to know where the frames are (the decoder's
        // frames context) before it starts, so it is made by hand.
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
            let sar = first.aspect_ratio();
            (*par).format = ffi::AVPixelFormat::AV_PIX_FMT_CUDA as i32;
            (*par).width = first.width() as i32;
            (*par).height = first.height() as i32;
            (*par).time_base = time_base.into();
            (*par).sample_aspect_ratio = ffi::AVRational {
                num: sar.numerator().max(1),
                den: sar.denominator().max(1),
            };
            // The source takes a reference of its own.
            (*par).hw_frames_ctx = (*first.as_ptr()).hw_frames_ctx;
            let set = ffi::av_buffersrc_parameters_set(ctx, par);
            ffi::av_free(par.cast());
            let ret = if set < 0 {
                set
            } else {
                ffi::avfilter_init_str(ctx, std::ptr::null())
            };
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "creating the GPU source".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
        }
        graph
            .add(&find("buffersink")?, "out", "")
            .ctx(|| "creating the GPU sink".into())?;
        let out = match format {
            format::Pixel::P010LE => "p010le",
            _ => "nv12",
        };
        let spec = filters(
            first.width(),
            first.height(),
            (width, height, out),
            deinterlace,
        );
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse(&spec))
            .ctx(|| format!("building {spec}"))?;
        graph.validate().ctx(|| format!("configuring {spec}"))?;
        Ok(Self { graph, time_base })
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
        match f {
            Some(f) => source.source().add(f).ctx(|| "scaling on the GPU".into()),
            None => source
                .source()
                .flush()
                .ctx(|| "flushing the GPU scaler".into()),
        }
    }

    /// The next picture, if one is ready: timed in the source's time base.
    pub fn pull(&mut self) -> Result<Option<frame::Video>> {
        let mut sink = self.graph.get("out").expect("sink");
        let mut out = frame::Video::empty();
        match sink.sink().frame(&mut out) {
            Ok(()) => {
                // bwdif halves the time base (for field rates): back to the
                // stream's.
                let tb = sink.sink().time_base();
                let pts = out
                    .pts()
                    .map(|p| unsafe { ffi::av_rescale_q(p, tb.into(), self.time_base.into()) });
                out.set_pts(pts);
                unsafe {
                    (*out.as_mut_ptr()).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
                }
                Ok(Some(out))
            }
            Err(ffmpeg::Error::Eof)
            | Err(ffmpeg::Error::Other {
                errno: ffmpeg::util::error::EAGAIN,
            }) => Ok(None),
            Err(e) => Err(Error::Ffmpeg {
                context: "scaling on the GPU".into(),
                source: e,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::filters;

    #[test]
    fn big_shrinks_halve_first() {
        assert_eq!(
            filters(3840, 2160, (960, 540, "nv12"), false),
            "scale_cuda=w=1920:h=1080:interp_algo=bilinear,\
             scale_cuda=w=960:h=540:interp_algo=bilinear,\
             scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic"
        );
        assert_eq!(
            filters(1280, 720, (960, 540, "nv12"), true),
            "bwdif_cuda=mode=send_frame:parity=auto:deint=interlaced,\
             scale_cuda=w=960:h=540:format=nv12:interp_algo=bicubic"
        );
    }
}
