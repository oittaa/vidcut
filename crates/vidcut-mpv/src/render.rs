//! mpv render contexts: OpenGL (the GUI) and software (headless tests).
//!
//! All `render_*` calls on a context must come from one thread (for OpenGL, the
//! thread with the GL context current). The update callback runs on mpv's
//! threads and must not call mpv; it should only wake the render thread.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Mutex;

use crate::client::{Error, Mpv, Result, check, trampoline};
use crate::ffi;

type Callback = Box<dyn Fn() + Send + Sync>;

/// State shared by both render context kinds.
struct Context {
    mpv: Mpv,
    ctx: *mut ffi::mpv_render_context,
    update_callback: Mutex<Option<Box<Callback>>>,
}

impl Context {
    fn create(mpv: &Mpv, params: &mut [ffi::mpv_render_param]) -> Result<Self> {
        let api = mpv.inner.api;
        let mut ctx = std::ptr::null_mut();
        let code =
            unsafe { (api.render_context_create)(&mut ctx, mpv.inner.handle, params.as_mut_ptr()) };
        check(api, code, || "creating render context".into())?;
        Ok(Self {
            mpv: mpv.clone(),
            ctx,
            update_callback: Mutex::new(None),
        })
    }

    fn set_update_callback(&self, callback: impl Fn() + Send + Sync + 'static) {
        let boxed: Box<Callback> = Box::new(Box::new(callback));
        let data = (&raw const *boxed).cast_mut().cast::<c_void>();
        let mut slot = self
            .update_callback
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe {
            (self.mpv.inner.api.render_context_set_update_callback)(
                self.ctx,
                Some(trampoline),
                data,
            )
        };
        *slot = Some(boxed);
    }

    fn update(&self) -> u64 {
        unsafe { (self.mpv.inner.api.render_context_update)(self.ctx) }
    }

    /// Whether the frame the next render draws is a new picture (not a redraw
    /// or a repeat of the last one).
    fn next_frame_is_new(&self) -> bool {
        let mut info = ffi::mpv_render_frame_info::default();
        let code = unsafe {
            (self.mpv.inner.api.render_context_get_info)(
                self.ctx,
                param(
                    ffi::MPV_RENDER_PARAM_NEXT_FRAME_INFO,
                    (&raw mut info).cast(),
                ),
            )
        };
        code >= 0
            && info.flags & ffi::MPV_RENDER_FRAME_INFO_PRESENT != 0
            && info.flags & (ffi::MPV_RENDER_FRAME_INFO_REDRAW | ffi::MPV_RENDER_FRAME_INFO_REPEAT)
                == 0
    }

    fn render(&self, params: &mut [ffi::mpv_render_param]) -> Result<()> {
        let api = self.mpv.inner.api;
        let code = unsafe { (api.render_context_render)(self.ctx, params.as_mut_ptr()) };
        check(api, code, || "rendering".into())
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        let api = self.mpv.inner.api;
        unsafe {
            (api.render_context_set_update_callback)(self.ctx, None, std::ptr::null_mut());
            (api.render_context_free)(self.ctx);
        }
        // `self.mpv` drops after this, so the handle outlives its render context.
    }
}

fn param(type_: c_int, data: *mut c_void) -> ffi::mpv_render_param {
    ffi::mpv_render_param { type_, data }
}

fn end() -> ffi::mpv_render_param {
    param(ffi::MPV_RENDER_PARAM_INVALID, std::ptr::null_mut())
}

/// Renders into caller-provided memory. Slow; meant for tests and tools.
pub struct SwRenderer {
    ctx: Context,
}

// SAFETY: a render context may be used from any single thread at a time; the
// `&mut self` render methods enforce exclusive use.
unsafe impl Send for SwRenderer {}

impl SwRenderer {
    pub fn new(mpv: &Mpv) -> Result<Self> {
        let mut params = [
            param(
                ffi::MPV_RENDER_PARAM_API_TYPE,
                ffi::MPV_RENDER_API_TYPE_SW.as_ptr().cast_mut().cast(),
            ),
            end(),
        ];
        Ok(Self {
            ctx: Context::create(mpv, &mut params)?,
        })
    }

    pub fn set_update_callback(&self, callback: impl Fn() + Send + Sync + 'static) {
        self.ctx.set_update_callback(callback);
    }

    /// Returns `mpv_render_update_flag` bits; [`ffi::MPV_RENDER_UPDATE_FRAME`]
    /// means a new frame is ready.
    pub fn update(&self) -> u64 {
        self.ctx.update()
    }

    /// Whether the next render draws a new picture (call after
    /// [`update`](Self::update) asked for a frame, before rendering it).
    pub fn next_frame_is_new(&self) -> bool {
        self.ctx.next_frame_is_new()
    }

    /// Renders the current frame as `rgb0` (4 bytes per pixel) into `buffer`.
    /// `stride` must be a multiple of 4 and at least `4 * width`.
    pub fn render_rgb0(
        &mut self,
        width: usize,
        height: usize,
        stride: usize,
        buffer: &mut [u8],
    ) -> Result<()> {
        if stride < width * 4 || !stride.is_multiple_of(4) || buffer.len() < stride * height {
            return Err(Error::Invalid("invalid software render target".into()));
        }
        let mut size = [width as c_int, height as c_int];
        let mut stride = stride;
        let mut params = [
            param(ffi::MPV_RENDER_PARAM_SW_SIZE, size.as_mut_ptr().cast()),
            param(
                ffi::MPV_RENDER_PARAM_SW_FORMAT,
                c"rgb0".as_ptr().cast_mut().cast(),
            ),
            param(ffi::MPV_RENDER_PARAM_SW_STRIDE, (&raw mut stride).cast()),
            param(ffi::MPV_RENDER_PARAM_SW_POINTER, buffer.as_mut_ptr().cast()),
            end(),
        ];
        self.ctx.render(&mut params)
    }
}

/// Looks up OpenGL function pointers by name (e.g. eframe's `get_proc_address`).
pub type ProcAddress = Box<dyn Fn(&CStr) -> *const c_void + Send + Sync>;

/// Renders into an OpenGL framebuffer. Create it, and call every method, on the
/// thread whose GL context mpv should use, with that context current.
pub struct GlRenderer {
    ctx: Context,
    /// Referenced by mpv while the context lives.
    _proc_address: Box<ProcAddress>,
}

unsafe extern "C" fn get_proc_address(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: `ctx` is the boxed ProcAddress owned by the GlRenderer.
    let lookup = unsafe { &*(ctx as *const ProcAddress) };
    lookup(unsafe { CStr::from_ptr(name) }).cast_mut()
}

/// Platform display connection mpv can use for hardware-decoding interop.
#[derive(Debug, Clone, Copy)]
pub enum Display {
    None,
    Wayland(*mut c_void),
    X11(*mut c_void),
}

impl GlRenderer {
    pub fn new(mpv: &Mpv, proc_address: ProcAddress, display: Display) -> Result<Self> {
        let proc_address: Box<ProcAddress> = Box::new(proc_address);
        let mut init = ffi::mpv_opengl_init_params {
            get_proc_address: Some(get_proc_address),
            get_proc_address_ctx: (&raw const *proc_address).cast_mut().cast(),
        };
        let mut params = vec![
            param(
                ffi::MPV_RENDER_PARAM_API_TYPE,
                ffi::MPV_RENDER_API_TYPE_OPENGL.as_ptr().cast_mut().cast(),
            ),
            param(
                ffi::MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                (&raw mut init).cast(),
            ),
        ];
        match display {
            Display::Wayland(d) if !d.is_null() => {
                params.push(param(ffi::MPV_RENDER_PARAM_WL_DISPLAY, d))
            }
            Display::X11(d) if !d.is_null() => {
                params.push(param(ffi::MPV_RENDER_PARAM_X11_DISPLAY, d))
            }
            _ => {}
        }
        params.push(end());
        let ctx = Context::create(mpv, &mut params)?;
        Ok(Self {
            ctx,
            _proc_address: proc_address,
        })
    }

    pub fn set_update_callback(&self, callback: impl Fn() + Send + Sync + 'static) {
        self.ctx.set_update_callback(callback);
    }

    /// Returns `mpv_render_update_flag` bits; [`ffi::MPV_RENDER_UPDATE_FRAME`]
    /// means a new frame should be rendered.
    pub fn update(&self) -> u64 {
        self.ctx.update()
    }

    /// Whether the next render draws a new picture (call after
    /// [`update`](Self::update) asked for a frame, before rendering it).
    pub fn next_frame_is_new(&self) -> bool {
        self.ctx.next_frame_is_new()
    }

    /// Renders the current frame into framebuffer object `fbo` (0 = default)
    /// of `width`×`height` pixels. mpv changes GL state; restore what you need.
    pub fn render(&self, fbo: u32, width: i32, height: i32, flip_y: bool) -> Result<()> {
        let mut target = ffi::mpv_opengl_fbo {
            fbo: fbo as c_int,
            w: width,
            h: height,
            internal_format: 0,
        };
        let mut flip: c_int = flip_y.into();
        let mut params = [
            param(ffi::MPV_RENDER_PARAM_OPENGL_FBO, (&raw mut target).cast()),
            param(ffi::MPV_RENDER_PARAM_FLIP_Y, (&raw mut flip).cast()),
            end(),
        ];
        self.ctx.render(&mut params)
    }

    /// Tells mpv a frame was presented (improves its timing estimates).
    pub fn report_swap(&self) {
        unsafe { (self.ctx.mpv.inner.api.render_context_report_swap)(self.ctx.ctx) }
    }
}
