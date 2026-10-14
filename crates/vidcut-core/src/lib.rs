//! vidcut project model, edit operations and command protocol.
//!
//! Pure Rust: no FFmpeg, no GPU, no I/O beyond the project store, so it
//! compiles and tests fast. Media facts come from the caller (see [`editor::Env`]).

pub mod analysis;
pub mod change;
pub mod describe;
pub mod dirs;
pub mod editor;
pub mod error;
pub mod export;
pub mod fingerprint;
pub mod model;
pub mod ops;
pub mod otio;
pub mod preferences;
pub mod preview;
pub mod render;
pub mod request;
pub mod sequence;
pub mod store;
pub mod time;
pub mod validate;

pub use editor::{Editor, Env, History, ProbedMedia};
pub use error::{Error, ErrorCode, Result};
pub use model::Project;
pub use request::{Envelope, Request, Response};
pub use store::Store;

#[cfg(test)]
mod analysis_tests;
#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod images_tests;
#[cfg(test)]
mod ops_tests;
#[cfg(test)]
mod proxy_tests;
#[cfg(test)]
mod relink_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tracks_tests;
