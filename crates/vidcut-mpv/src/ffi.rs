//! Raw libmpv ABI (client API 2.x, mpv 0.41 headers), loaded at runtime.
//!
//! Only what vidcut uses is declared. Values come from `client.h`, `render.h`
//! and `render_gl.h` of mpv v0.41.0.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::sync::OnceLock;

#[repr(C)]
pub struct mpv_handle {
    _private: [u8; 0],
}

#[repr(C)]
pub struct mpv_render_context {
    _private: [u8; 0],
}

#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_property {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

#[repr(C)]
pub struct mpv_event_log_message {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

#[repr(C)]
pub struct mpv_render_param {
    pub type_: c_int,
    pub data: *mut c_void,
}

pub type GetProcAddressFn =
    unsafe extern "C" fn(ctx: *mut c_void, name: *const c_char) -> *mut c_void;

#[repr(C)]
pub struct mpv_opengl_init_params {
    pub get_proc_address: Option<GetProcAddressFn>,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_fbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

// mpv_format
pub const MPV_FORMAT_NONE: c_int = 0;
pub const MPV_FORMAT_STRING: c_int = 1;
pub const MPV_FORMAT_FLAG: c_int = 3;
pub const MPV_FORMAT_INT64: c_int = 4;
pub const MPV_FORMAT_DOUBLE: c_int = 5;

// mpv_event_id
pub const MPV_EVENT_NONE: c_int = 0;
pub const MPV_EVENT_SHUTDOWN: c_int = 1;
pub const MPV_EVENT_LOG_MESSAGE: c_int = 2;
pub const MPV_EVENT_GET_PROPERTY_REPLY: c_int = 3;
pub const MPV_EVENT_SET_PROPERTY_REPLY: c_int = 4;
pub const MPV_EVENT_COMMAND_REPLY: c_int = 5;
pub const MPV_EVENT_START_FILE: c_int = 6;
pub const MPV_EVENT_END_FILE: c_int = 7;
pub const MPV_EVENT_FILE_LOADED: c_int = 8;
pub const MPV_EVENT_IDLE: c_int = 11;
pub const MPV_EVENT_VIDEO_RECONFIG: c_int = 17;
pub const MPV_EVENT_AUDIO_RECONFIG: c_int = 18;
pub const MPV_EVENT_SEEK: c_int = 20;
pub const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;
pub const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;
pub const MPV_EVENT_QUEUE_OVERFLOW: c_int = 24;

// mpv_render_param_type
pub const MPV_RENDER_PARAM_INVALID: c_int = 0;
pub const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
pub const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;
pub const MPV_RENDER_PARAM_X11_DISPLAY: c_int = 8;
pub const MPV_RENDER_PARAM_WL_DISPLAY: c_int = 9;
pub const MPV_RENDER_PARAM_SW_SIZE: c_int = 17;
pub const MPV_RENDER_PARAM_SW_FORMAT: c_int = 18;
pub const MPV_RENDER_PARAM_SW_STRIDE: c_int = 19;
pub const MPV_RENDER_PARAM_SW_POINTER: c_int = 20;

// mpv_render_update_flag
pub const MPV_RENDER_UPDATE_FRAME: u64 = 1;

pub const MPV_RENDER_API_TYPE_OPENGL: &std::ffi::CStr = c"opengl";
pub const MPV_RENDER_API_TYPE_SW: &std::ffi::CStr = c"sw";

pub type WakeupFn = unsafe extern "C" fn(*mut c_void);

/// libmpv entry points, resolved once per process.
pub struct Api {
    _lib: libloading::Library,
    pub client_api_version: unsafe extern "C" fn() -> c_ulong,
    pub error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    pub free: unsafe extern "C" fn(*mut c_void),
    pub create: unsafe extern "C" fn() -> *mut mpv_handle,
    pub initialize: unsafe extern "C" fn(*mut mpv_handle) -> c_int,
    pub terminate_destroy: unsafe extern "C" fn(*mut mpv_handle),
    pub set_option_string:
        unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
    pub command: unsafe extern "C" fn(*mut mpv_handle, *mut *const c_char) -> c_int,
    pub command_async: unsafe extern "C" fn(*mut mpv_handle, u64, *mut *const c_char) -> c_int,
    pub set_property:
        unsafe extern "C" fn(*mut mpv_handle, *const c_char, c_int, *mut c_void) -> c_int,
    pub set_property_async:
        unsafe extern "C" fn(*mut mpv_handle, u64, *const c_char, c_int, *mut c_void) -> c_int,
    pub get_property:
        unsafe extern "C" fn(*mut mpv_handle, *const c_char, c_int, *mut c_void) -> c_int,
    pub observe_property: unsafe extern "C" fn(*mut mpv_handle, u64, *const c_char, c_int) -> c_int,
    pub request_log_messages: unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> c_int,
    pub wait_event: unsafe extern "C" fn(*mut mpv_handle, f64) -> *mut mpv_event,
    pub wakeup: unsafe extern "C" fn(*mut mpv_handle),
    pub set_wakeup_callback: unsafe extern "C" fn(*mut mpv_handle, Option<WakeupFn>, *mut c_void),
    pub render_context_create: unsafe extern "C" fn(
        *mut *mut mpv_render_context,
        *mut mpv_handle,
        *mut mpv_render_param,
    ) -> c_int,
    pub render_context_set_update_callback:
        unsafe extern "C" fn(*mut mpv_render_context, Option<WakeupFn>, *mut c_void),
    pub render_context_update: unsafe extern "C" fn(*mut mpv_render_context) -> u64,
    pub render_context_render:
        unsafe extern "C" fn(*mut mpv_render_context, *mut mpv_render_param) -> c_int,
    pub render_context_report_swap: unsafe extern "C" fn(*mut mpv_render_context),
    pub render_context_free: unsafe extern "C" fn(*mut mpv_render_context),
}

/// Library names tried in order; `$VIDCUT_LIBMPV` takes precedence.
const LIBRARY_NAMES: &[&str] = &["libmpv.so.2", "libmpv.so"];

/// Loads libmpv (once) and returns its entry points.
pub fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    let override_name = std::env::var("VIDCUT_LIBMPV").ok();
    let names: Vec<&str> = override_name
        .as_deref()
        .into_iter()
        .chain(LIBRARY_NAMES.iter().copied())
        .collect();
    let mut errors = Vec::new();
    for name in names {
        // SAFETY: loading libmpv runs its constructors; it has no unusual
        // initialization requirements.
        match unsafe { libloading::Library::new(name) } {
            Ok(lib) => return resolve(lib).map_err(|e| format!("{name}: {e}")),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Err(format!(
        "libmpv not found ({}); install it with `sudo apt install libmpv2`",
        errors.join("; ")
    ))
}

fn resolve(lib: libloading::Library) -> Result<Api, libloading::Error> {
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: the declared signature matches the libmpv 2.x header.
            unsafe { *lib.get(concat!($name, "\0").as_bytes())? }
        };
    }
    let api = Api {
        client_api_version: sym!("mpv_client_api_version"),
        error_string: sym!("mpv_error_string"),
        free: sym!("mpv_free"),
        create: sym!("mpv_create"),
        initialize: sym!("mpv_initialize"),
        terminate_destroy: sym!("mpv_terminate_destroy"),
        set_option_string: sym!("mpv_set_option_string"),
        command: sym!("mpv_command"),
        command_async: sym!("mpv_command_async"),
        set_property: sym!("mpv_set_property"),
        set_property_async: sym!("mpv_set_property_async"),
        get_property: sym!("mpv_get_property"),
        observe_property: sym!("mpv_observe_property"),
        request_log_messages: sym!("mpv_request_log_messages"),
        wait_event: sym!("mpv_wait_event"),
        wakeup: sym!("mpv_wakeup"),
        set_wakeup_callback: sym!("mpv_set_wakeup_callback"),
        render_context_create: sym!("mpv_render_context_create"),
        render_context_set_update_callback: sym!("mpv_render_context_set_update_callback"),
        render_context_update: sym!("mpv_render_context_update"),
        render_context_render: sym!("mpv_render_context_render"),
        render_context_report_swap: sym!("mpv_render_context_report_swap"),
        render_context_free: sym!("mpv_render_context_free"),
        _lib: lib,
    };
    Ok(api)
}
