//! Where a player's mix goes.
//!
//! The session pulls samples and writes them here. The default opens the
//! system playback device (PulseAudio, which PipeWire serves). Tests pass a
//! recorder instead: that boundary is the device, and the samples are the
//! ones the player pulled. `player.mute` writes silence.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::snapshot::PlayerId;

/// Interleaved f32 at the plan's rate and channel count.
pub trait Playback: Send {
    /// The plan about to be played. The device opens here.
    fn configure(&mut self, _rate: u32, _channels: u16) {}
    fn write(&mut self, samples: &[f32]);
}

/// Drops every sample. Pictures still play.
pub struct Discard;

impl Playback for Discard {
    fn write(&mut self, _: &[f32]) {}
}

/// Keeps what was written, in order. A stand-in for the device.
pub struct Recorder {
    pub samples: Arc<Mutex<Vec<f32>>>,
}

impl Playback for Recorder {
    fn write(&mut self, samples: &[f32]) {
        self.samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(samples);
    }
}

/// One player’s output. `None` is not used: callers pass this or [`Discard`].
pub type PlaybackFactory = Arc<dyn Fn(PlayerId) -> Box<dyn Playback> + Send + Sync>;

/// Plays on the default PulseAudio device.
pub fn device_playback() -> PlaybackFactory {
    Arc::new(|id| {
        let name = match id {
            PlayerId::Timeline => "timeline",
            PlayerId::Source => "source",
        };
        Box::new(Device::new(name))
    })
}

/// Drops samples. Sessions under test use this unless they record.
pub fn discard_playback() -> PlaybackFactory {
    Arc::new(|_| Box::new(Discard))
}

/// Opens PulseAudio on first [`Playback::configure`] and writes from its own
/// thread, so a full device buffer cannot stall the session.
pub struct Device {
    name: &'static str,
    rate: u32,
    channels: u16,
    tx: Option<SyncSender<Vec<f32>>>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Mutex<Status>>,
    writes: Arc<AtomicU64>,
}

enum Status {
    Starting,
    Open,
    Failed(String),
}

impl Device {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            rate: 0,
            channels: 0,
            tx: None,
            thread: None,
            status: Arc::new(Mutex::new(Status::Starting)),
            writes: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Waits until the device has accepted its format, or reports why not.
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let started = std::time::Instant::now();
        loop {
            match &*self.status.lock().unwrap_or_else(|e| e.into_inner()) {
                Status::Open => return Ok(()),
                Status::Failed(e) => return Err(e.clone()),
                Status::Starting => {}
            }
            if started.elapsed() >= timeout {
                return Err("the playback device did not open in time".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// How many buffers the device accepted.
    pub fn writes(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }

    fn shutdown(&mut self) {
        // Dropping the sender lets the thread finish its current write and exit.
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Playback for Device {
    fn configure(&mut self, rate: u32, channels: u16) {
        if rate == 0 || channels == 0 {
            return;
        }
        if self.tx.is_some() && self.rate == rate && self.channels == channels {
            return;
        }
        self.shutdown();
        self.rate = rate;
        self.channels = channels;
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = Status::Starting;
        self.writes.store(0, Ordering::Relaxed);
        let (tx, rx) = sync_channel(64);
        self.tx = Some(tx);
        let name = self.name;
        let status = self.status.clone();
        let writes = self.writes.clone();
        self.thread = crate::threads::spawn(
            "vidcut-playback",
            crate::threads::Work::Interactive,
            move || {
                run_device(name, rate, channels, rx, status, writes);
            },
        )
        .ok();
        if self.thread.is_none() {
            *self.status.lock().unwrap_or_else(|e| e.into_inner()) =
                Status::Failed("could not start the playback thread".into());
            self.tx.take();
        }
    }

    fn write(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if let Some(tx) = &self.tx {
            // A full buffer means the device is behind. Drop this chunk
            // rather than block the session thread.
            let _ = tx.try_send(samples.to_vec());
        }
    }
}

fn run_device(
    name: &str,
    rate: u32,
    channels: u16,
    rx: Receiver<Vec<f32>>,
    status: Arc<Mutex<Status>>,
    writes: Arc<AtomicU64>,
) {
    let set = |s: Status| {
        *status.lock().unwrap_or_else(|e| e.into_inner()) = s;
    };
    let opened = match open_pulse(name, rate, channels) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("vidcut: playback audio is off ({name}): {e}");
            set(Status::Failed(e));
            return;
        }
    };
    set(Status::Open);
    while let Ok(samples) = rx.recv() {
        if let Err(e) = opened.write(&samples) {
            eprintln!("vidcut: playback audio stopped ({name}): {e}");
            set(Status::Failed(e));
            return;
        }
        writes.fetch_add(1, Ordering::Relaxed);
    }
}

/// `pa_sample_spec` from the Pulse simple API. `format` 5 is
/// `PA_SAMPLE_FLOAT32LE`. The trailing padding matches the C layout.
#[repr(C)]
struct SampleSpec {
    format: i32,
    rate: u32,
    channels: u8,
}

struct Pulse {
    ptr: *mut libc::c_void,
    write: unsafe extern "C" fn(
        *mut libc::c_void,
        *const libc::c_void,
        usize,
        *mut libc::c_int,
    ) -> libc::c_int,
    free: unsafe extern "C" fn(*mut libc::c_void),
}

impl Drop for Pulse {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: the pointer came from pa_simple_new on this thread, and
            // no other call is in flight.
            unsafe { (self.free)(self.ptr) };
            self.ptr = std::ptr::null_mut();
        }
    }
}

impl Pulse {
    fn write(&self, samples: &[f32]) -> Result<(), String> {
        let mut err = 0;
        // SAFETY: `samples` is the interleaved f32 buffer the spec describes,
        // and `ptr` is a live pa_simple owned by this thread.
        let rc = unsafe {
            (self.write)(
                self.ptr,
                samples.as_ptr().cast(),
                std::mem::size_of_val(samples),
                &mut err,
            )
        };
        if rc < 0 {
            Err(format!("pulse write failed: {}", pulse_error(err)))
        } else {
            Ok(())
        }
    }
}

struct Api {
    _lib: libloading::Library,
    new: unsafe extern "C" fn(
        *const libc::c_char,
        *const libc::c_char,
        libc::c_int,
        *const libc::c_char,
        *const libc::c_char,
        *const SampleSpec,
        *const libc::c_void,
        *const libc::c_void,
        *mut libc::c_int,
    ) -> *mut libc::c_void,
    write: unsafe extern "C" fn(
        *mut libc::c_void,
        *const libc::c_void,
        usize,
        *mut libc::c_int,
    ) -> libc::c_int,
    free: unsafe extern "C" fn(*mut libc::c_void),
}

fn api() -> Result<&'static Api, String> {
    use std::sync::OnceLock;
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load_api).as_ref().map_err(Clone::clone)
}

fn load_api() -> Result<Api, String> {
    // SAFETY: loading libpulse-simple runs its constructors. It has no
    // unusual initialization, and the soname is the Pulse 0 ABI.
    let lib = unsafe { libloading::Library::new("libpulse-simple.so.0") }
        .map_err(|e| format!("libpulse-simple.so.0: {e}"))?;
    // SAFETY: the signatures match pa_simple_new, pa_simple_write and
    // pa_simple_free in the Pulse simple API.
    unsafe {
        Ok(Api {
            new: *lib.get(b"pa_simple_new\0").map_err(|e| e.to_string())?,
            write: *lib.get(b"pa_simple_write\0").map_err(|e| e.to_string())?,
            free: *lib.get(b"pa_simple_free\0").map_err(|e| e.to_string())?,
            _lib: lib,
        })
    }
}

/// Pulse's `pa_strerror` names, for the codes `pa_simple_new` returns.
fn pulse_error(code: i32) -> String {
    let name = match code {
        0 => "ok",
        1 => "access denied",
        2 => "unknown command",
        3 => "invalid argument",
        4 => "entity exists",
        5 => "no such entity",
        6 => "connection refused",
        7 => "protocol error",
        _ => "unknown",
    };
    format!("{name} ({code})")
}

fn open_pulse(name: &str, rate: u32, channels: u16) -> Result<Pulse, String> {
    let api = api()?;
    let channels =
        u8::try_from(channels).map_err(|_| format!("{channels} channels is too many"))?;
    let spec = SampleSpec {
        format: 5, // PA_SAMPLE_FLOAT32LE
        rate,
        channels,
    };
    let app = std::ffi::CString::new("vidcut").map_err(|e| e.to_string())?;
    let stream = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
    let mut err = 0;
    // PA_STREAM_PLAYBACK is 1. NULL server and device select the default.
    // SAFETY: the C strings and the spec live for the call, which copies them.
    let ptr = unsafe {
        (api.new)(
            std::ptr::null(),
            app.as_ptr(),
            1,
            std::ptr::null(),
            stream.as_ptr(),
            &spec,
            std::ptr::null(),
            std::ptr::null(),
            &mut err,
        )
    };
    if ptr.is_null() {
        return Err(format!("pulse did not open: {}", pulse_error(err)));
    }
    Ok(Pulse {
        ptr,
        write: api.write,
        free: api.free,
    })
}
