#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::all)]

mod transcribe_sys {
    include!("transcribe_sys.rs");
}

pub use transcribe_sys::*;

use std::path::Path;
use std::sync::OnceLock;

static API: OnceLock<transcribe_sys::Api> = OnceLock::new();

fn api() -> &'static transcribe_sys::Api {
    API.get().expect("transcribe.cpp's library isn't loaded")
}

pub fn loaded() -> bool {
    API.get().is_some()
}

pub fn load(path: &Path) -> Result<(), String> {
    if loaded() {
        return Ok(());
    }
    let lib = open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: each symbol is taken with the signature the bindings give it, from the library they were made for.
    let api = unsafe { transcribe_sys::Api::resolve(&lib) };
    std::mem::forget(lib);
    let _ = API.set(api);
    Ok(())
}

#[cfg(windows)]
fn open(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library};
    // SAFETY: transcribe.cpp's library, loaded once and never unloaded; its DLLs are found beside it.
    unsafe { Library::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH) }.map(Into::into)
}

#[cfg(not(windows))]
fn open(path: &Path) -> Result<libloading::Library, libloading::Error> {
    // SAFETY: transcribe.cpp's library, loaded once and never unloaded.
    unsafe { libloading::Library::new(path) }
}
