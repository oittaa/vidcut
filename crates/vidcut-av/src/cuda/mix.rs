//! Pictures mixed on NVIDIA's GPU while a dissolve plays (decision D50),
//! for NVENC: a kernel of our own, sample by sample, with the CPU's sums
//! (`export::mix`), so both make the same pictures. Its PTX is written
//! here, and the driver compiles it for the GPU once per context.

use std::ffi::c_void;
use std::sync::Mutex;

use super::CudaFrames;
use super::share::{CuContext, Cuda, check, cuda_context, in_context};
use crate::error::{Error, Result};
use crate::export::mix::Samples;
use crate::ffmpeg::{ffi, frame};

/// One plane: sample `(x, y)` of `over` at `weight` (65536: all of it)
/// over that of `under`, into `out`. A source whose pointer is 0 is black,
/// its sample `*_black`. Samples take `bytes` bytes (1 or 2), their value
/// `shift` bits up in them.
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

/// Threads of a block across and down.
const BLOCK: (u32, u32) = (32, 8);

/// The kernel, loaded into a CUDA context.
struct Kernel {
    ctx: usize,
    function: usize,
}

/// The kernel of `ctx`, loaded the first time it is asked for there.
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
        // The module stays loaded for the context's life.
        Ok(function)
    })?;
    loaded.push(Kernel {
        ctx: ctx as usize,
        function: function as usize,
    });
    Ok(function)
}

/// `over` at `weight` over `under` (`None`: black), frames of `frames`'
/// size and layout in CUDA memory, into a frame of `frames`. The work goes
/// on `frames`' device's stream, behind what is queued there; the inputs'
/// own streams are waited for first.
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
    // Two planes, the chroma half as tall: anything else the CPU mixes.
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
    let (w, h) = (fc.width as u32, fc.height as u32);
    let deep = samples.bytes == 2;
    // 4:2:0 in two planes (NV12, P010): luma, then chroma half as tall.
    let planes = [
        (0usize, h, if deep { 64 } else { 16 }),
        (1, h.div_ceil(2), if deep { 512 } else { 128 }),
    ];
    let rows = samples.row_bytes(w);
    in_context(cuda, ctx, || {
        for (p, rows_down, black) in planes {
            let plane = |f: Option<&frame::Video>| -> (u64, u64) {
                // SAFETY: a CUDA frame's planes are device pointers.
                f.map_or((0, 0), |f| unsafe {
                    let f = &*f.as_ptr();
                    (f.data[p] as u64, f.linesize[p] as u64)
                })
            };
            let (mut o, mut o_pitch) = plane(Some(&out));
            let (mut u, mut u_pitch) = plane(under);
            let (mut v, mut v_pitch) = plane(over);
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
            // SAFETY: the context is current; `params` points to the
            // kernel's 13 parameters, in its order, alive for the call.
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
