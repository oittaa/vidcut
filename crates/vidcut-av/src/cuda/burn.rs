use std::ffi::c_void;
use std::sync::Mutex;

use super::CudaFrames;
use super::share::{CuContext, Cuda, check, cuda_context, in_context};
use crate::error::{Error, Result};
use crate::export::mix::Samples;
use crate::ffmpeg::{ffi, frame};

const PTX: &str = r#"
.version 7.0
.target sm_52
.address_size 64

.visible .entry vidcut_burn(
	.param .u64 p_out,
	.param .u64 p_out_pitch,
	.param .u64 p_under,
	.param .u64 p_under_pitch,
	.param .u64 p_color,
	.param .u64 p_color_pitch,
	.param .u64 p_alpha,
	.param .u64 p_alpha_pitch,
	.param .u32 p_width,
	.param .u32 p_height,
	.param .u32 p_bytes,
	.param .u32 p_shift,
	.param .u32 p_top,
	.param .u32 p_x0,
	.param .u32 p_y0,
	.param .u32 p_x1,
	.param .u32 p_y1
)
{
	.reg .pred %p<8>;
	.reg .b32 %r<16>;
	.reg .b64 %rd<16>;
	.reg .f32 %f<8>;

	mov.u32 %r1, %ctaid.x;
	mov.u32 %r2, %ntid.x;
	mov.u32 %r3, %tid.x;
	mad.lo.s32 %r4, %r1, %r2, %r3;
	mov.u32 %r1, %ctaid.y;
	mov.u32 %r2, %ntid.y;
	mov.u32 %r3, %tid.y;
	mad.lo.s32 %r5, %r1, %r2, %r3;
	ld.param.u32 %r6, [p_width];
	ld.param.u32 %r7, [p_height];
	setp.ge.u32 %p1, %r4, %r6;
	setp.ge.u32 %p2, %r5, %r7;
	or.pred %p1, %p1, %p2;
	@%p1 bra $L_done;

	ld.param.u32 %r8, [p_bytes];
	ld.param.u32 %r9, [p_shift];
	setp.eq.u32 %p3, %r8, 1;
	mul.wide.u32 %rd1, %r4, %r8;
	cvt.u64.u32 %rd2, %r5;

	ld.param.u64 %rd3, [p_under];
	ld.param.u64 %rd4, [p_under_pitch];
	mad.lo.s64 %rd5, %rd2, %rd4, %rd3;
	add.s64 %rd5, %rd5, %rd1;
	@%p3 ld.global.u8 %r10, [%rd5];
	@!%p3 ld.global.u16 %r10, [%rd5];
	mov.u32 %r14, %r10;

	ld.param.u32 %r15, [p_x0];
	setp.lt.u32 %p4, %r4, %r15;
	ld.param.u32 %r15, [p_x1];
	setp.ge.u32 %p5, %r4, %r15;
	or.pred %p4, %p4, %p5;
	ld.param.u32 %r15, [p_y0];
	setp.lt.u32 %p5, %r5, %r15;
	or.pred %p4, %p4, %p5;
	ld.param.u32 %r15, [p_y1];
	setp.ge.u32 %p5, %r5, %r15;
	or.pred %p4, %p4, %p5;
	@%p4 bra $L_store;

	ld.param.u64 %rd6, [p_color];
	ld.param.u64 %rd7, [p_color_pitch];
	mad.lo.s64 %rd8, %rd2, %rd7, %rd6;
	add.s64 %rd8, %rd8, %rd1;
	@%p3 ld.global.u8 %r11, [%rd8];
	@!%p3 ld.global.u16 %r11, [%rd8];

	ld.param.u64 %rd9, [p_alpha];
	ld.param.u64 %rd10, [p_alpha_pitch];
	mad.lo.s64 %rd11, %rd2, %rd10, %rd9;
	add.s64 %rd11, %rd11, %rd1;
	@%p3 ld.global.u8 %r12, [%rd11];
	@!%p3 ld.global.u16 %r12, [%rd11];

	shr.u32 %r10, %r10, %r9;
	shr.u32 %r11, %r11, %r9;
	shr.u32 %r12, %r12, %r9;
	ld.param.u32 %r13, [p_top];
	cvt.rn.f32.u32 %f1, %r10;
	cvt.rn.f32.u32 %f2, %r11;
	cvt.rn.f32.u32 %f3, %r12;
	cvt.rn.f32.u32 %f4, %r13;
	div.rn.f32 %f3, %f3, %f4;
	sub.f32 %f5, %f2, %f1;
	fma.rn.f32 %f6, %f5, %f3, %f1;
	cvt.rni.u32.f32 %r14, %f6;
	shl.b32 %r14, %r14, %r9;

$L_store:
	ld.param.u64 %rd12, [p_out];
	ld.param.u64 %rd13, [p_out_pitch];
	mad.lo.s64 %rd14, %rd2, %rd13, %rd12;
	add.s64 %rd14, %rd14, %rd1;
	@%p3 st.global.u8 [%rd14], %r14;
	@!%p3 st.global.u16 [%rd14], %r14;

$L_done:
	ret;
}
"#;

const BLOCK: (u32, u32) = (32, 8);

struct Kernel {
    ctx: usize,
    function: usize,
}

fn kernel(cuda: &Cuda, ctx: CuContext) -> Result<*mut c_void> {
    static LOADED: Mutex<Vec<Kernel>> = Mutex::new(Vec::new());
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = loaded.iter().find(|k| k.ctx == ctx as usize) {
        return Ok(k.function as *mut c_void);
    }
    let function = in_context(cuda, ctx, || {
        let ptx = std::ffi::CString::new(PTX).expect("no NUL");
        let mut module = std::ptr::null_mut();
        // SAFETY: the context is current; `ptx` is NUL-terminated.
        check("loading the subtitle kernel", unsafe {
            (cuda.module_load_data)(&mut module, ptx.as_ptr().cast())
        })?;
        let mut function = std::ptr::null_mut();
        // SAFETY: the context is current; the module just loaded.
        check("finding the subtitle kernel", unsafe {
            (cuda.module_get_function)(&mut function, module, c"vidcut_burn".as_ptr())
        })?;
        Ok(function)
    })?;
    loaded.push(Kernel {
        ctx: ctx as usize,
        function: function as usize,
    });
    Ok(function)
}

pub(crate) struct Overlay {
    color: frame::Video,
    alpha: frame::Video,
    area: (u32, u32, u32, u32),
}

fn pool_frame(frames: &CudaFrames) -> Result<frame::Video> {
    let mut f = frame::Video::empty();
    // SAFETY: a frame of the pool, its planes in CUDA memory.
    let ret = unsafe { ffi::av_hwframe_get_buffer(frames.as_ptr(), f.as_mut_ptr(), 0) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "getting a GPU frame".into(),
            source: crate::ffmpeg::Error::from(ret),
        });
    }
    Ok(f)
}

fn two_planes(sw: crate::ffmpeg::format::Pixel) -> Result<()> {
    match sw {
        crate::ffmpeg::format::Pixel::NV12 | crate::ffmpeg::format::Pixel::P010LE => Ok(()),
        _ => Err(Error::Invalid(format!(
            "the GPU draws subtitles into NV12 and P010 pictures, not {sw:?}"
        ))),
    }
}

impl Overlay {
    pub(crate) fn new(
        frames: &CudaFrames,
        like: &frame::Video,
        sprites: &[crate::burn::Sprite],
        shift: crate::burn::Placing,
    ) -> Result<Self> {
        // SAFETY: the pool is a live CUDA frames context.
        let sw = unsafe {
            let fc = &*((*frames.as_ptr()).data as *const ffi::AVHWFramesContext);
            crate::ffmpeg::format::Pixel::from(fc.sw_format)
        };
        two_planes(sw)?;
        let (color, alpha) = (pool_frame(frames)?, pool_frame(frames)?);
        let size = (like.width(), like.height());
        let Some(area) = crate::burn::bounds(sprites, shift, size) else {
            return Ok(Self {
                color,
                alpha,
                area: (0, 0, 0, 0),
            });
        };
        let (x0, y0, w, h) = area;
        let (up_color, up_alpha) = crate::burn::overlay(sw, like, sprites, shift, area)?;
        Self::up(
            frames,
            (color, alpha),
            (&up_color, &up_alpha),
            (x0, y0, w, h),
        )
    }

    pub(crate) fn of(
        frames: &CudaFrames,
        layer: &crate::export::place::Layer,
        size: (u32, u32),
    ) -> Result<Self> {
        // SAFETY: the pool is a live CUDA frames context.
        let sw = unsafe {
            let fc = &*((*frames.as_ptr()).data as *const ffi::AVHWFramesContext);
            crate::ffmpeg::format::Pixel::from(fc.sw_format)
        };
        two_planes(sw)?;
        let (color, alpha) = (pool_frame(frames)?, pool_frame(frames)?);
        let Some(area) = layer.area(size) else {
            return Ok(Self {
                color,
                alpha,
                area: (0, 0, 0, 0),
            });
        };
        let (up_color, up_alpha) = crate::export::place::overlay(sw, layer, size, area)?;
        let [x0, y0, x1, y1] = area;
        Self::up(
            frames,
            (color, alpha),
            (&up_color, &up_alpha),
            (x0 as i32, y0 as i32, x1 - x0, y1 - y0),
        )
    }

    fn up(
        frames: &CudaFrames,
        (color, alpha): (frame::Video, frame::Video),
        (up_color, up_alpha): (&frame::Video, &frame::Video),
        (x0, y0, w, h): (i32, i32, u32, u32),
    ) -> Result<Self> {
        let cuda = Cuda::get()?;
        // SAFETY: the pool is a live CUDA frames context.
        let (sw, (ctx, stream)) = unsafe {
            let fc = &*((*frames.as_ptr()).data as *const ffi::AVHWFramesContext);
            (
                crate::ffmpeg::format::Pixel::from(fc.sw_format),
                cuda_context(fc.device_ref),
            )
        };
        let samples = Samples::of(sw)?;
        let from = samples.row_bytes(x0 as u32);
        let rows = samples.row_bytes(w);
        in_context(cuda, ctx, || {
            for (host, device) in [(up_color, &color), (up_alpha, &alpha)] {
                for p in 0..2 {
                    let (y, n) = match p {
                        0 => (y0 as usize, h as usize),
                        _ => (y0 as usize / 2, (h as usize).div_ceil(2)),
                    };
                    // SAFETY: a CUDA frame's planes are device pointers.
                    let (to, to_pitch) = unsafe {
                        let d = &*device.as_ptr();
                        (d.data[p] as u64, d.linesize[p] as usize)
                    };
                    // SAFETY: both planes, of one format, hold the area's rows at their strides.
                    check("copying subtitles to the GPU", unsafe {
                        cuda.copy_up(
                            (host.data(p).as_ptr(), host.stride(p)),
                            (to, to_pitch),
                            (from[p], y),
                            (rows[p], n),
                            stream,
                        )
                    })?;
                }
            }
            Ok(())
        })?;
        crate::copies::up(up_color);
        crate::copies::up(up_alpha);
        Ok(Self {
            color,
            alpha,
            area: (x0 as u32, y0 as u32, w, h),
        })
    }
}

pub(crate) fn burn(
    frames: &CudaFrames,
    under: &frame::Video,
    over: &Overlay,
) -> Result<frame::Video> {
    let cuda = Cuda::get()?;
    super::share::wait_for(under)?;
    // SAFETY: the pool is a live CUDA frames context.
    let (fc, (ctx, stream)) = unsafe {
        let fc = &*((*frames.as_ptr()).data as *const ffi::AVHWFramesContext);
        (fc, cuda_context(fc.device_ref))
    };
    let sw: crate::ffmpeg::format::Pixel = fc.sw_format.into();
    two_planes(sw)?;
    let samples = Samples::of(sw)?;
    let mut out = pool_frame(frames)?;
    let function = kernel(cuda, ctx)?;
    let (w, h) = match (u32::try_from(fc.width), u32::try_from(fc.height)) {
        (Ok(w), Ok(h)) => (w, h),
        _ => {
            return Err(Error::Invalid(format!(
                "{}×{} doesn't fit a frame",
                fc.width, fc.height
            )));
        }
    };
    let deep = samples.bytes == 2;
    let rows = samples.row_bytes(w);
    let (ax, ay, aw, ah) = over.area;
    let (left, right) = (samples.row_bytes(ax), samples.row_bytes(ax + aw));
    in_context(cuda, ctx, || {
        for (p, rows_down) in [(0usize, h), (1, h.div_ceil(2))] {
            let plane = |f: &frame::Video| -> Result<(u64, u64)> {
                // SAFETY: a CUDA frame's planes are device pointers.
                let f = unsafe { &*f.as_ptr() };
                let pitch = u64::try_from(f.linesize[p]).map_err(|_| {
                    Error::Invalid(format!("a picture stride of {} doesn't fit", f.linesize[p]))
                })?;
                Ok((f.data[p] as u64, pitch))
            };
            let (mut o, mut o_pitch) = plane(&out)?;
            let (mut u, mut u_pitch) = plane(under)?;
            let (mut c, mut c_pitch) = plane(&over.color)?;
            let (mut a, mut a_pitch) = plane(&over.alpha)?;
            let mut width = (rows[p] / samples.bytes) as u32;
            let mut height = rows_down;
            let mut bytes = samples.bytes as u32;
            let mut shift = samples.shift;
            let mut top: u32 = if deep { 1023 } else { 255 };
            let mut x0 = (left[p] / samples.bytes) as u32;
            let mut x1 = (right[p] / samples.bytes) as u32;
            let (mut y0, mut y1) = match p {
                0 => (ay, ay + ah),
                _ => (ay / 2, ay / 2 + ah.div_ceil(2)),
            };
            let mut params: [*mut c_void; 17] = [
                (&raw mut o).cast(),
                (&raw mut o_pitch).cast(),
                (&raw mut u).cast(),
                (&raw mut u_pitch).cast(),
                (&raw mut c).cast(),
                (&raw mut c_pitch).cast(),
                (&raw mut a).cast(),
                (&raw mut a_pitch).cast(),
                (&raw mut width).cast(),
                (&raw mut height).cast(),
                (&raw mut bytes).cast(),
                (&raw mut shift).cast(),
                (&raw mut top).cast(),
                (&raw mut x0).cast(),
                (&raw mut y0).cast(),
                (&raw mut x1).cast(),
                (&raw mut y1).cast(),
            ];
            // SAFETY: the context is current; `params` holds the kernel's 17 parameters, in order.
            check("drawing subtitles", unsafe {
                (cuda.launch_kernel)(
                    function,
                    width.div_ceil(BLOCK.0),
                    height.div_ceil(BLOCK.1),
                    1,
                    BLOCK.0,
                    BLOCK.1,
                    1,
                    0,
                    stream,
                    params.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            })?;
        }
        Ok(())
    })?;
    // SAFETY: both are valid frames.
    unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), under.as_ptr()) };
    Ok(out)
}
