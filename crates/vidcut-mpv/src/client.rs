//! Safe wrapper around an mpv client handle.
//!
//! The mpv client API is thread-safe, so [`Mpv`] is `Send + Sync` and cheap to
//! clone (it shares one handle). The handle is destroyed when the last clone and
//! the last render context using it are dropped.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Arc, Mutex};

use crate::ffi::{self, Api};

/// An mpv error: the API return code plus what was being done.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Load(String),
    #[error("{context}: {message} ({code})")]
    Mpv {
        code: i32,
        message: String,
        context: String,
    },
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn check(api: &Api, code: c_int, context: impl FnOnce() -> String) -> Result<()> {
    if code >= 0 {
        return Ok(());
    }
    let message = unsafe { CStr::from_ptr((api.error_string)(code)) }
        .to_string_lossy()
        .into_owned();
    Err(Error::Mpv {
        code,
        message,
        context: context(),
    })
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::Invalid(format!("string contains NUL: {s:?}")))
}

type Callback = Box<dyn Fn() + Send + Sync>;

pub(crate) struct Inner {
    pub(crate) api: &'static Api,
    pub(crate) handle: *mut ffi::mpv_handle,
    /// Keeps the boxed wakeup callback alive while mpv may call it.
    wakeup: Mutex<Option<Box<Callback>>>,
}

// SAFETY: every libmpv client function may be called from any thread.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe {
            (self.api.set_wakeup_callback)(self.handle, None, std::ptr::null_mut());
            (self.api.terminate_destroy)(self.handle);
        }
    }
}

/// A property value delivered with [`Event::PropertyChange`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    String(String),
    Flag(bool),
    Int(i64),
    Double(f64),
}

/// Format to request when observing a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    None,
    String,
    Flag,
    Int,
    Double,
}

impl Format {
    fn raw(self) -> c_int {
        match self {
            Self::None => ffi::MPV_FORMAT_NONE,
            Self::String => ffi::MPV_FORMAT_STRING,
            Self::Flag => ffi::MPV_FORMAT_FLAG,
            Self::Int => ffi::MPV_FORMAT_INT64,
            Self::Double => ffi::MPV_FORMAT_DOUBLE,
        }
    }
}

/// An event from [`Mpv::wait_event`].
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Shutdown,
    LogMessage {
        prefix: String,
        level: String,
        text: String,
    },
    CommandReply {
        id: u64,
        error: i32,
    },
    SetPropertyReply {
        id: u64,
        error: i32,
    },
    StartFile,
    /// `reason`: 0 eof, 2 stop, 3 quit, 4 error, 5 redirect.
    EndFile {
        reason: i32,
        error: i32,
    },
    FileLoaded,
    Idle,
    VideoReconfig,
    AudioReconfig,
    Seek,
    PlaybackRestart,
    PropertyChange {
        id: u64,
        name: String,
        value: Value,
    },
    QueueOverflow,
    Other(i32),
}

/// A shared mpv client handle.
#[derive(Clone)]
pub struct Mpv {
    pub(crate) inner: Arc<Inner>,
}

impl Mpv {
    /// Loads libmpv if needed, creates a handle, applies `options` (set before
    /// initialization, so they behave like command-line options) and initializes it.
    pub fn new(options: &[(&str, &str)]) -> Result<Self> {
        let api = ffi::api().map_err(Error::Load)?;
        let major = unsafe { (api.client_api_version)() } >> 16;
        if major != 2 {
            return Err(Error::Load(format!(
                "unsupported libmpv client API major version {major}, need 2"
            )));
        }
        // mpv refuses to start unless numbers are formatted the C way. This
        // sets the whole process's locale, not this thread's: from here on,
        // C code that formats numbers by the locale uses C's, not the user's.
        unsafe { libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr()) };
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err(Error::Invalid("mpv_create failed".into()));
        }
        let mpv = Self {
            inner: Arc::new(Inner {
                api,
                handle,
                wakeup: Mutex::new(None),
            }),
        };
        for (name, value) in options {
            let (n, v) = (cstring(name)?, cstring(value)?);
            let code = unsafe { (api.set_option_string)(handle, n.as_ptr(), v.as_ptr()) };
            check(api, code, || format!("setting option {name}={value}"))?;
        }
        check(api, unsafe { (api.initialize)(handle) }, || {
            "initializing mpv".into()
        })?;
        Ok(mpv)
    }

    fn api(&self) -> &'static Api {
        self.inner.api
    }

    fn handle(&self) -> *mut ffi::mpv_handle {
        self.inner.handle
    }

    /// Runs a command synchronously, e.g. `["seek", "1.5", "absolute+exact"]`.
    pub fn command(&self, args: &[&str]) -> Result<()> {
        let (owned, mut ptrs) = argv(args)?;
        let code = unsafe { (self.api().command)(self.handle(), ptrs.as_mut_ptr()) };
        drop(owned);
        check(self.api(), code, || format!("command {args:?}"))
    }

    /// Queues a command; completion arrives as [`Event::CommandReply`] with `id`.
    pub fn command_async(&self, id: u64, args: &[&str]) -> Result<()> {
        let (owned, mut ptrs) = argv(args)?;
        let code = unsafe { (self.api().command_async)(self.handle(), id, ptrs.as_mut_ptr()) };
        drop(owned);
        check(self.api(), code, || format!("async command {args:?}"))
    }

    pub fn set_property_string(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (cstring(name)?, cstring(value)?);
        let mut ptr = v.as_ptr();
        let code = unsafe {
            (self.api().set_property)(
                self.handle(),
                n.as_ptr(),
                ffi::MPV_FORMAT_STRING,
                (&raw mut ptr).cast::<c_void>(),
            )
        };
        check(self.api(), code, || format!("setting {name}={value}"))
    }

    pub fn set_property_flag(&self, name: &str, value: bool) -> Result<()> {
        let n = cstring(name)?;
        let mut v: c_int = value.into();
        let code = unsafe {
            (self.api().set_property)(
                self.handle(),
                n.as_ptr(),
                ffi::MPV_FORMAT_FLAG,
                (&raw mut v).cast(),
            )
        };
        check(self.api(), code, || format!("setting {name}={value}"))
    }

    pub fn set_property_f64(&self, name: &str, value: f64) -> Result<()> {
        let n = cstring(name)?;
        let mut v = value;
        let code = unsafe {
            (self.api().set_property)(
                self.handle(),
                n.as_ptr(),
                ffi::MPV_FORMAT_DOUBLE,
                (&raw mut v).cast(),
            )
        };
        check(self.api(), code, || format!("setting {name}={value}"))
    }

    pub fn get_property_string(&self, name: &str) -> Result<String> {
        let n = cstring(name)?;
        let mut out: *mut c_char = std::ptr::null_mut();
        let code = unsafe {
            (self.api().get_property)(
                self.handle(),
                n.as_ptr(),
                ffi::MPV_FORMAT_STRING,
                (&raw mut out).cast(),
            )
        };
        check(self.api(), code, || format!("getting {name}"))?;
        let s = unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.api().free)(out.cast()) };
        Ok(s)
    }

    pub fn get_property_f64(&self, name: &str) -> Result<f64> {
        let n = cstring(name)?;
        let mut out = 0f64;
        let code = unsafe {
            (self.api().get_property)(
                self.handle(),
                n.as_ptr(),
                ffi::MPV_FORMAT_DOUBLE,
                (&raw mut out).cast(),
            )
        };
        check(self.api(), code, || format!("getting {name}"))?;
        Ok(out)
    }

    pub fn get_property_i64(&self, name: &str) -> Result<i64> {
        let n = cstring(name)?;
        let mut out = 0i64;
        let code = unsafe {
            (self.api().get_property)(
                self.handle(),
                n.as_ptr(),
                ffi::MPV_FORMAT_INT64,
                (&raw mut out).cast(),
            )
        };
        check(self.api(), code, || format!("getting {name}"))?;
        Ok(out)
    }

    pub fn get_property_flag(&self, name: &str) -> Result<bool> {
        let n = cstring(name)?;
        let mut out: c_int = 0;
        let code = unsafe {
            (self.api().get_property)(
                self.handle(),
                n.as_ptr(),
                ffi::MPV_FORMAT_FLAG,
                (&raw mut out).cast(),
            )
        };
        check(self.api(), code, || format!("getting {name}"))?;
        Ok(out != 0)
    }

    /// Delivers [`Event::PropertyChange`] with `id` whenever `name` changes.
    pub fn observe_property(&self, id: u64, name: &str, format: Format) -> Result<()> {
        let n = cstring(name)?;
        let code =
            unsafe { (self.api().observe_property)(self.handle(), id, n.as_ptr(), format.raw()) };
        check(self.api(), code, || format!("observing {name}"))
    }

    /// Enables [`Event::LogMessage`] at `level` and above (e.g. `"warn"`).
    pub fn request_log_messages(&self, level: &str) -> Result<()> {
        let l = cstring(level)?;
        let code = unsafe { (self.api().request_log_messages)(self.handle(), l.as_ptr()) };
        check(self.api(), code, || format!("requesting log level {level}"))
    }

    /// Waits up to `timeout` seconds (0 = poll) for the next event.
    /// Returns `None` on timeout.
    ///
    /// mpv allows only one thread to wait for events on a handle at a time.
    pub fn wait_event(&self, timeout: f64) -> Option<Event> {
        let ev = unsafe { &*(self.api().wait_event)(self.handle(), timeout) };
        let cstr = |p: *const c_char| -> String {
            if p.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
            }
        };
        Some(match ev.event_id {
            ffi::MPV_EVENT_NONE => return None,
            ffi::MPV_EVENT_SHUTDOWN => Event::Shutdown,
            ffi::MPV_EVENT_LOG_MESSAGE => {
                let m = unsafe { &*(ev.data as *const ffi::mpv_event_log_message) };
                Event::LogMessage {
                    prefix: cstr(m.prefix),
                    level: cstr(m.level),
                    text: cstr(m.text),
                }
            }
            ffi::MPV_EVENT_COMMAND_REPLY => Event::CommandReply {
                id: ev.reply_userdata,
                error: ev.error,
            },
            ffi::MPV_EVENT_SET_PROPERTY_REPLY => Event::SetPropertyReply {
                id: ev.reply_userdata,
                error: ev.error,
            },
            ffi::MPV_EVENT_START_FILE => Event::StartFile,
            ffi::MPV_EVENT_END_FILE => {
                let e = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
                Event::EndFile {
                    reason: e.reason,
                    error: e.error,
                }
            }
            ffi::MPV_EVENT_FILE_LOADED => Event::FileLoaded,
            ffi::MPV_EVENT_IDLE => Event::Idle,
            ffi::MPV_EVENT_VIDEO_RECONFIG => Event::VideoReconfig,
            ffi::MPV_EVENT_AUDIO_RECONFIG => Event::AudioReconfig,
            ffi::MPV_EVENT_SEEK => Event::Seek,
            ffi::MPV_EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
            ffi::MPV_EVENT_PROPERTY_CHANGE => {
                let p = unsafe { &*(ev.data as *const ffi::mpv_event_property) };
                let value = unsafe {
                    match p.format {
                        ffi::MPV_FORMAT_STRING => {
                            Value::String(cstr(*(p.data as *const *const c_char)))
                        }
                        ffi::MPV_FORMAT_FLAG => Value::Flag(*(p.data as *const c_int) != 0),
                        ffi::MPV_FORMAT_INT64 => Value::Int(*(p.data as *const i64)),
                        ffi::MPV_FORMAT_DOUBLE => Value::Double(*(p.data as *const f64)),
                        _ => Value::None,
                    }
                };
                Event::PropertyChange {
                    id: ev.reply_userdata,
                    name: cstr(p.name),
                    value,
                }
            }
            ffi::MPV_EVENT_QUEUE_OVERFLOW => Event::QueueOverflow,
            other => Event::Other(other),
        })
    }

    /// Interrupts a blocking [`wait_event`](Self::wait_event).
    pub fn wakeup(&self) {
        unsafe { (self.api().wakeup)(self.handle()) }
    }

    /// Sets the callback mpv invokes (from its own threads) when events are
    /// pending. It must not call back into mpv; signal another thread instead.
    pub fn set_wakeup_callback(&self, callback: impl Fn() + Send + Sync + 'static) {
        let boxed: Box<Callback> = Box::new(Box::new(callback));
        let ctx = (&raw const *boxed).cast_mut().cast::<c_void>();
        let mut slot = self.inner.wakeup.lock().unwrap_or_else(|e| e.into_inner());
        // mpv serializes this call with callback invocations, so the old box is
        // unused once it returns.
        unsafe { (self.api().set_wakeup_callback)(self.handle(), Some(trampoline), ctx) };
        *slot = Some(boxed);
    }
}

pub(crate) unsafe extern "C" fn trampoline(ctx: *mut c_void) {
    // SAFETY: `ctx` points at a `Callback` kept alive by its owner until the
    // callback is unregistered.
    let callback = unsafe { &*(ctx as *const Callback) };
    callback();
}

/// NUL-terminated argv for mpv commands; the CStrings must outlive the pointers.
fn argv(args: &[&str]) -> Result<(Vec<CString>, Vec<*const c_char>)> {
    let owned = args
        .iter()
        .map(|a| cstring(a))
        .collect::<Result<Vec<_>>>()?;
    let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    Ok((owned, ptrs))
}
