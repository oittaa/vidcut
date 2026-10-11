//! vidcut media layer on the FFmpeg libraries (in-process; never the ffmpeg CLI).

pub use ffmpeg_next as ffmpeg;

pub mod analysis;
pub mod barcode;
pub mod beep;
pub mod cancel;
pub mod color;
pub mod decode;
pub mod error;
pub mod export;
pub mod filler;
pub mod fixture;
pub mod probe;
pub mod progress;
pub mod proxy;
pub mod remux;
pub mod tails;
pub mod testdata;
pub mod thumbs;
pub mod waveform;

pub use error::{Error, Result};

/// Initializes FFmpeg once per process. FFmpeg's own log output is limited to
/// fatal messages: decoders recovering after a seek log harmless errors, and
/// real failures surface as vidcut errors anyway. Set `VIDCUT_AV_LOG` to an
/// FFmpeg level name (`error`, `warning`, `debug`, ...) to see more.
pub fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        ffmpeg::init().expect("FFmpeg initialization failed");
        use ffmpeg::util::log::Level;
        let level = match std::env::var("VIDCUT_AV_LOG").as_deref() {
            Ok("quiet") => Level::Quiet,
            Ok("error") => Level::Error,
            Ok("warning") => Level::Warning,
            Ok("info") => Level::Info,
            Ok("verbose") => Level::Verbose,
            Ok("debug") => Level::Debug,
            Ok("trace") => Level::Trace,
            _ => Level::Fatal,
        };
        ffmpeg::util::log::set_level(level);
    });
}

/// Versions of the linked FFmpeg libraries, e.g. `{"libavcodec": "62.11.100"}`.
pub fn library_versions() -> std::collections::BTreeMap<&'static str, String> {
    let v = |n: u32| format!("{}.{}.{}", n >> 16, (n >> 8) & 0xff, n & 0xff);
    [
        ("libavcodec", v(ffmpeg::codec::version())),
        ("libavformat", v(ffmpeg::format::version())),
        ("libavfilter", v(ffmpeg::filter::version())),
        ("libavutil", v(ffmpeg::util::version())),
    ]
    .into_iter()
    .collect()
}
