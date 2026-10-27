#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{Mixer, Overlay};

#[cfg(not(target_os = "macos"))]
mod elsewhere;
#[cfg(not(target_os = "macos"))]
pub(crate) use elsewhere::{Mixer, Overlay};
