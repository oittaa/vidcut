//! vidcut media layer on the FFmpeg libraries (in-process; never the ffmpeg CLI).

pub use ffmpeg_next as ffmpeg;

pub mod analysis;
pub mod barcode;
pub mod beep;
pub mod cancel;
pub mod color;
pub mod cuda;
pub mod decode;
pub mod error;
pub mod export;
pub mod fixture;
pub mod player;
pub mod probe;
pub mod progress;
pub mod proxy;
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

/// FFmpeg's version as its build says it: `8.0.1-3ubuntu2` from a
/// distribution, `N-…-g…` from git.
pub fn ffmpeg_version() -> String {
    // SAFETY: a static string FFmpeg keeps for the life of the process.
    unsafe { std::ffi::CStr::from_ptr(ffmpeg::ffi::av_version_info()) }
        .to_string_lossy()
        .into_owned()
}

/// The release in [`ffmpeg_version`]: `8.0.1` of `8.0.1-3ubuntu2`; a git
/// build's whole string.
pub fn ffmpeg_release() -> String {
    release_of(&ffmpeg_version())
}

fn release_of(version: &str) -> String {
    match version.split_once('-') {
        Some((release, _)) if release.starts_with(|c: char| c.is_ascii_digit()) => {
            release.to_string()
        }
        _ => version.to_string(),
    }
}

/// FFmpeg's version and those of the linked libraries, e.g.
/// `{"version": "8.0.1-3ubuntu2", "libavcodec": "62.11.100"}`.
pub fn library_versions() -> std::collections::BTreeMap<&'static str, String> {
    let v = |n: u32| format!("{}.{}.{}", n >> 16, (n >> 8) & 0xff, n & 0xff);
    [
        ("version", ffmpeg_version()),
        ("libavcodec", v(ffmpeg::codec::version())),
        ("libavformat", v(ffmpeg::format::version())),
        ("libavfilter", v(ffmpeg::filter::version())),
        ("libavutil", v(ffmpeg::util::version())),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn releases_drop_the_build() {
        assert_eq!(super::release_of("8.0.1-3ubuntu2"), "8.0.1");
        assert_eq!(super::release_of("8.0.1"), "8.0.1");
        assert_eq!(super::release_of("N-118451-g1a2b3c4"), "N-118451-g1a2b3c4");
    }
}
