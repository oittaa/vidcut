//! Minimal runtime-loaded libmpv bindings for vidcut's preview.
//!
//! libmpv is opened with `dlopen` on first use, so programs linking this crate
//! start (and the vidcut CLI works) on machines without libmpv. Only the GUI
//! needs it.

pub mod client;
pub mod ffi;
pub mod render;

pub use client::{Error, Event, Format, Mpv, Result, Value};
pub use render::{Display, GlRenderer, ProcAddress, SwRenderer};

/// Whether libmpv can be loaded, with the reason if not.
pub fn available() -> std::result::Result<(), String> {
    ffi::api().map(|_| ())
}

/// libmpv's client API version, `(major, minor)`, when it loads.
pub fn client_api_version() -> Option<(u32, u32)> {
    let api = ffi::api().ok()?;
    // SAFETY: a plain query of the loaded library.
    let v = unsafe { (api.client_api_version)() };
    Some(((v >> 16) as u32, (v & 0xffff) as u32))
}
