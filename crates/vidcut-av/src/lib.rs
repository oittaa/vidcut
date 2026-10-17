pub use ffmpeg_next as ffmpeg;

pub mod analysis;
pub mod barcode;
pub mod beep;
pub(crate) mod buffers;
pub mod cancel;
pub mod color;
pub mod copies;
pub mod cuda;
pub mod decode;
pub mod ending;
pub mod error;
pub mod export;
pub mod first;
pub mod fixture;
pub mod gpus;
pub mod hdr;
pub mod hw;
pub mod light;
pub mod parts;
pub mod player;
pub mod pool;
pub mod probe;
pub mod progress;
pub mod proxy;
pub mod route;
pub(crate) mod stretch;
pub mod testdata;
pub mod thumbs;
pub mod waveform;

pub use error::{Error, Result};

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

pub fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIDCUT_DEBUG").is_some())
}

#[macro_export]
macro_rules! debug {
    ($area:literal, $($arg:tt)*) => {
        if $crate::debug() {
            eprintln!("[{}] {}", $area, format_args!($($arg)*));
        }
    };
}

pub fn ffmpeg_version() -> String {
    // SAFETY: a static string FFmpeg keeps for the life of the process.
    unsafe { std::ffi::CStr::from_ptr(ffmpeg::ffi::av_version_info()) }
        .to_string_lossy()
        .into_owned()
}

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

pub fn cpu_seconds() -> f64 {
    #[cfg(unix)]
    // SAFETY: getrusage fills the struct it is given.
    unsafe {
        let mut r: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut r);
        let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
        t(r.ru_utime) + t(r.ru_stime)
    }
    #[cfg(windows)]
    // SAFETY: fills the four times it is given, for this process.
    unsafe {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let mut times: [FILETIME; 4] = std::mem::zeroed();
        let [created, exited, kernel, user] = &mut times;
        GetProcessTimes(GetCurrentProcess(), created, exited, kernel, user);
        let t = |f: &FILETIME| {
            (u64::from(f.dwHighDateTime) << 32 | u64::from(f.dwLowDateTime)) as f64 / 1e7
        };
        t(&times[2]) + t(&times[3])
    }
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
