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

.visible .entry vidcut_mix(
	.param .u64 p_out,
	.param .u64 p_out_pitch,
	.param .u64 p_under,
	.param .u64 p_under_pitch,
	.param .u32 p_under_black,
	.param .u64 p_over,
	.param .u64 p_over_pitch,
	.param .u32 p_over_black,
	.param .u32 p_width,
	.param .u32 p_height,
	.param .u32 p_bytes,
	.param .u32 p_shift,
	.param .u32 p_weight
)
{
	.reg .pred %p<8>;
	.reg .b32 %r<16>;
	.reg .b64 %rd<12>;

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
	ld.param.u32 %r10, [p_under_black];
	setp.eq.u64 %p4, %rd3, 0;
	@%p4 bra $L_over;
	ld.param.u64 %rd4, [p_under_pitch];
	mad.lo.s64 %rd5, %rd2, %rd4, %rd3;
	add.s64 %rd5, %rd5, %rd1;
	@%p3 ld.global.u8 %r10, [%rd5];
	@!%p3 ld.global.u16 %r10, [%rd5];
	shr.u32 %r10, %r10, %r9;

$L_over:
	ld.param.u64 %rd6, [p_over];
	ld.param.u32 %r11, [p_over_black];
	setp.eq.u64 %p5, %rd6, 0;
	@%p5 bra $L_mix;
	ld.param.u64 %rd7, [p_over_pitch];
	mad.lo.s64 %rd8, %rd2, %rd7, %rd6;
	add.s64 %rd8, %rd8, %rd1;
	@%p3 ld.global.u8 %r11, [%rd8];
	@!%p3 ld.global.u16 %r11, [%rd8];
	shr.u32 %r11, %r11, %r9;

$L_mix:
	ld.param.u32 %r12, [p_weight];
	mov.u32 %r13, 65536;
	sub.s32 %r13, %r13, %r12;
	mul.lo.s32 %r14, %r10, %r13;
	mad.lo.s32 %r14, %r11, %r12, %r14;
	add.s32 %r14, %r14, 32768;
	shr.u32 %r14, %r14, 16;
	shl.b32 %r14, %r14, %r9;

	ld.param.u64 %rd9, [p_out];
	ld.param.u64 %rd10, [p_out_pitch];
	mad.lo.s64 %rd11, %rd2, %rd10, %rd9;
	add.s64 %rd11, %rd11, %rd1;
	@%p3 st.global.u8 [%rd11], %r14;
	@!%p3 st.global.u16 [%rd11], %r14;

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
        check("loading the mixing kernel", unsafe {
            (cuda.module_load_data)(&mut module, ptx.as_ptr().cast())
        })?;
        let mut function = std::ptr::null_mut();
        // SAFETY: the context is current; the module just loaded.
        check("finding the mixing kernel", unsafe {
            (cuda.module_get_function)(&mut function, module, c"vidcut_mix".as_ptr())
        })?;
        Ok(function)
    })?;
    loaded.push(Kernel {
        ctx: ctx as usize,
        function: function as usize,
    });
    Ok(function)
}

fn frame_u32(width: i32, height: i32) -> Result<(u32, u32)> {
    match (u32::try_from(width), u32::try_from(height)) {
        (Ok(width), Ok(height)) => Ok((width, height)),
        _ => Err(Error::Invalid(format!(
            "{width}×{height} doesn't fit a frame"
        ))),
    }
}

fn stride_u64(linesize: i32) -> Result<u64> {
    u64::try_from(linesize)
        .map_err(|_| Error::Invalid(format!("a picture stride of {linesize} doesn't fit")))
}

pub(crate) fn mix(
    frames: &CudaFrames,
    under: Option<&frame::Video>,
    over: Option<&frame::Video>,
    weight: u32,
) -> Result<frame::Video> {
    let cuda = Cuda::get()?;
    for f in under.iter().chain(over.iter()) {
        super::share::wait_for(f)?;
    }
    // SAFETY: the pool is a live CUDA frames context.
    let (fc, (ctx, stream)) = unsafe {
        let fc = &*((*frames.as_ptr()).data as *const ffi::AVHWFramesContext);
        (fc, cuda_context(fc.device_ref))
    };
    let sw: crate::ffmpeg::format::Pixel = fc.sw_format.into();
    if !matches!(
        sw,
        crate::ffmpeg::format::Pixel::NV12 | crate::ffmpeg::format::Pixel::P010LE
    ) {
        return Err(Error::Invalid(format!(
            "the GPU mixes NV12 and P010 pictures, not {sw:?}"
        )));
    }
    let samples = Samples::of(sw)?;
    let mut out = frame::Video::empty();
    // SAFETY: a frame of the pool, its planes in CUDA memory.
    let ret = unsafe { ffi::av_hwframe_get_buffer(frames.as_ptr(), out.as_mut_ptr(), 0) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "getting a GPU frame".into(),
            source: crate::ffmpeg::Error::from(ret),
        });
    }
    let function = kernel(cuda, ctx)?;
    let (w, h) = frame_u32(fc.width, fc.height)?;
    let deep = samples.bytes == 2;
    let planes = [
        (0usize, h, if deep { 64 } else { 16 }),
        (1, h.div_ceil(2), if deep { 512 } else { 128 }),
    ];
    let rows = samples.row_bytes(w);
    in_context(cuda, ctx, || {
        for (p, rows_down, black) in planes {
            let plane = |f: Option<&frame::Video>| -> Result<(u64, u64)> {
                // SAFETY: a CUDA frame's planes are device pointers.
                f.map_or(Ok((0, 0)), |f| unsafe {
                    let f = &*f.as_ptr();
                    Ok((f.data[p] as u64, stride_u64(f.linesize[p])?))
                })
            };
            let (mut o, mut o_pitch) = plane(Some(&out))?;
            let (mut u, mut u_pitch) = plane(under)?;
            let (mut v, mut v_pitch) = plane(over)?;
            let (mut u_black, mut v_black) = (black, black);
            let mut width = (rows[p] / samples.bytes) as u32;
            let mut height = rows_down;
            let mut bytes = samples.bytes as u32;
            let mut shift = samples.shift;
            let mut weight = weight;
            let mut params: [*mut c_void; 13] = [
                (&raw mut o).cast(),
                (&raw mut o_pitch).cast(),
                (&raw mut u).cast(),
                (&raw mut u_pitch).cast(),
                (&raw mut u_black).cast(),
                (&raw mut v).cast(),
                (&raw mut v_pitch).cast(),
                (&raw mut v_black).cast(),
                (&raw mut width).cast(),
                (&raw mut height).cast(),
                (&raw mut bytes).cast(),
                (&raw mut shift).cast(),
                (&raw mut weight).cast(),
            ];
            // SAFETY: the context is current; `params` holds the kernel's 13 parameters, in order.
            check("mixing pictures", unsafe {
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
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{frame_u32, stride_u64};

    #[test]
    fn a_negative_frame_width_doesnt_fit_the_mix() {
        let err = frame_u32(-1, 1080).unwrap_err();
        assert_eq!(err.to_string(), "-1×1080 doesn't fit a frame");
    }

    #[test]
    fn a_negative_frame_height_doesnt_fit_the_mix() {
        let err = frame_u32(1920, -8).unwrap_err();
        assert_eq!(err.to_string(), "1920×-8 doesn't fit a frame");
    }

    #[test]
    fn the_largest_frame_side_fits_the_mix() {
        assert_eq!(frame_u32(i32::MAX, 0).unwrap(), (2_147_483_647, 0));
    }

    #[test]
    fn a_negative_stride_doesnt_fit_the_mix() {
        let err = stride_u64(-1).unwrap_err();
        assert_eq!(err.to_string(), "a picture stride of -1 doesn't fit");
    }

    #[test]
    fn the_largest_stride_fits_the_mix() {
        assert_eq!(stride_u64(i32::MAX).unwrap(), 2_147_483_647);
    }
}
