//! Minimal runtime-loaded libmpv bindings.
//!
//! libmpv is opened with `dlopen` on first use, so programs linking this crate
//! start on machines without libmpv. Playback does not call into it. The
//! spikes in this crate and `vidcut`'s `mpv_gl_spike` example do.

pub mod client;
pub mod ffi;
pub mod render;

pub use client::{Error, Event, Format, Mpv, Result, Value};
pub use render::{Display, GlRenderer, ProcAddress, SwRenderer};

/// Whether libmpv can be loaded, with the reason if not.
pub fn available() -> std::result::Result<(), String> {
    ffi::api().map(|_| ())
}
