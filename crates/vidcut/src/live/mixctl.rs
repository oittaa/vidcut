//! Live track faders (docs/decisions/preview.md §10): the timeline preview's
//! audio graph has an `azmq` filter, and this sends it commands over
//! ZeroMQ, so a fader being dragged is heard at once. The ZeroMQ library is
//! loaded at run time, as libmpv is: without it, or without FFmpeg's
//! `azmq`, a fader changes the preview when it is released.
//!
//! Commands go out on a thread of their own, which keeps only the latest
//! level of each fader: a drag sends many, and only the last one matters.

use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Called with the filter's answer once the command was sent.
pub type Done = Box<dyn FnOnce(Result<(), String>) + Send>;

/// One fader level for the graph at `endpoint`.
struct Command {
    endpoint: String,
    /// The fader's filter ([`vidcut_core::preview::fader_filter`]).
    filter: String,
    /// Linear gain.
    gain: f64,
    done: Option<Done>,
}

/// Sends fader levels to the preview's audio graph.
pub struct Mixer {
    tx: Sender<Command>,
}

impl Mixer {
    /// The control thread, when ZeroMQ and FFmpeg's `azmq` are both there.
    pub fn start() -> Option<Self> {
        if !vidcut_av::has_filter("azmq") {
            return None;
        }
        let api = api().ok()?;
        let (tx, rx) = channel();
        crate::threads::spawn(
            "vidcut-mixer",
            crate::threads::Work::Interactive,
            move || run(api, rx),
        )
        .ok()?;
        Some(Self { tx })
    }

    /// Sets the fader `filter` of the graph at `endpoint` to `gain`.
    pub fn set(&self, endpoint: &str, filter: String, gain: f64, done: Option<Done>) {
        let _ = self.tx.send(Command {
            endpoint: endpoint.to_string(),
            filter,
            gain,
            done,
        });
    }
}

fn run(api: &'static Api, rx: Receiver<Command>) {
    while let Ok(first) = rx.recv() {
        // The latest level of each fader, and everyone waiting on it.
        let mut latest: HashMap<(String, String), (f64, Vec<Done>)> = HashMap::new();
        let mut order = Vec::new();
        for c in std::iter::once(first).chain(rx.try_iter()) {
            let key = (c.endpoint, c.filter);
            let entry = latest.entry(key.clone()).or_insert_with(|| {
                order.push(key);
                (c.gain, Vec::new())
            });
            entry.0 = c.gain;
            entry.1.extend(c.done);
        }
        for key in order {
            let (gain, done) = latest.remove(&key).expect("collected above");
            let (endpoint, filter) = key;
            let result = api.request(&endpoint, &format!("{filter} volume {gain:.6}"));
            for d in done {
                d(result.clone());
            }
        }
    }
}

const ZMQ_REQ: c_int = 3;
const ZMQ_LINGER: c_int = 17;
const ZMQ_RCVTIMEO: c_int = 27;
const ZMQ_SNDTIMEO: c_int = 28;
/// How long a command may take; the filter answers within one audio frame.
const TIMEOUT_MS: c_int = 500;

/// The parts of libzmq used here, resolved once per process.
struct Api {
    _lib: libloading::Library,
    ctx: usize,
    socket: unsafe extern "C" fn(*mut c_void, c_int) -> *mut c_void,
    setsockopt: unsafe extern "C" fn(*mut c_void, c_int, *const c_void, usize) -> c_int,
    connect: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int,
    send: unsafe extern "C" fn(*mut c_void, *const c_void, usize, c_int) -> c_int,
    recv: unsafe extern "C" fn(*mut c_void, *mut c_void, usize, c_int) -> c_int,
    close: unsafe extern "C" fn(*mut c_void) -> c_int,
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    // SAFETY: loading libzmq runs its constructors; it has no unusual
    // initialization requirements.
    let lib = unsafe { libloading::Library::new("libzmq.so.5") }.map_err(|e| e.to_string())?;
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: the declared signature matches the libzmq 4.x header.
            unsafe {
                *lib.get(concat!($name, "\0").as_bytes())
                    .map_err(|e| e.to_string())?
            }
        };
    }
    let ctx_new: unsafe extern "C" fn() -> *mut c_void = sym!("zmq_ctx_new");
    // SAFETY: a plain constructor; the context lives as long as the process.
    let ctx = unsafe { ctx_new() };
    if ctx.is_null() {
        return Err("zmq_ctx_new failed".into());
    }
    Ok(Api {
        ctx: ctx as usize,
        socket: sym!("zmq_socket"),
        setsockopt: sym!("zmq_setsockopt"),
        connect: sym!("zmq_connect"),
        send: sym!("zmq_send"),
        recv: sym!("zmq_recv"),
        close: sym!("zmq_close"),
        _lib: lib,
    })
}

impl Api {
    /// Sends `message` to the `azmq` filter at `endpoint` and reads its
    /// answer, `0 Success` when the command took. A socket per request: a
    /// request socket that missed an answer can't be used again.
    fn request(&self, endpoint: &str, message: &str) -> Result<(), String> {
        let endpoint = CString::new(endpoint).map_err(|e| e.to_string())?;
        // SAFETY: the context is valid for the process; each call below
        // gets the socket made here and buffers that outlive the call.
        unsafe {
            let socket = (self.socket)(self.ctx as *mut c_void, ZMQ_REQ);
            if socket.is_null() {
                return Err("zmq_socket failed".into());
            }
            let set = |option: c_int, value: c_int| {
                (self.setsockopt)(
                    socket,
                    option,
                    (&raw const value).cast(),
                    std::mem::size_of::<c_int>(),
                )
            };
            set(ZMQ_LINGER, 0);
            set(ZMQ_RCVTIMEO, TIMEOUT_MS);
            set(ZMQ_SNDTIMEO, TIMEOUT_MS);
            let result = if (self.connect)(socket, endpoint.as_ptr()) != 0 {
                Err("cannot reach the preview's mixer".to_string())
            } else if (self.send)(socket, message.as_ptr().cast(), message.len(), 0) < 0 {
                Err("the preview's mixer took no command".to_string())
            } else {
                let mut buf = [0u8; 256];
                let n = (self.recv)(socket, buf.as_mut_ptr().cast(), buf.len(), 0);
                if n < 0 {
                    Err("the preview's mixer did not answer".to_string())
                } else {
                    let answer = String::from_utf8_lossy(&buf[..(n as usize).min(buf.len())]);
                    if answer.starts_with("0 ") {
                        Ok(())
                    } else {
                        Err(format!("the preview's mixer: {}", answer.trim()))
                    }
                }
            };
            (self.close)(socket);
            result
        }
    }
}
