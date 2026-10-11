//! vidcut media layer on the FFmpeg libraries (in-process; never the ffmpeg CLI).

pub use ffmpeg_next as ffmpeg;

pub mod barcode;
pub mod beep;
pub mod decode;
pub mod error;
pub mod fixture;
pub mod probe;
pub mod remux;
pub mod testdata;

pub use error::{Error, Result};

/// Initializes FFmpeg once per process. Library log output is limited to errors
/// unless `VIDCUT_AV_LOG` is set to an FFmpeg level name (e.g. `debug`).
pub fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        ffmpeg::init().expect("FFmpeg initialization failed");
        use ffmpeg::util::log::Level;
        let level = match std::env::var("VIDCUT_AV_LOG").as_deref() {
            Ok("quiet") => Level::Quiet,
            Ok("warning") => Level::Warning,
            Ok("info") => Level::Info,
            Ok("verbose") => Level::Verbose,
            Ok("debug") => Level::Debug,
            Ok("trace") => Level::Trace,
            _ => Level::Error,
        };
        ffmpeg::util::log::set_level(level);
    });
}
