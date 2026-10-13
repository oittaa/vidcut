//! Memory over a long export, in a test binary of its own: peak memory
//! belongs to the process, and Cargo runs one test binary at a time, so
//! nothing else runs alongside.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::export::{self, encode::Availability};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::export::{
    AudioCodec as ACodec, AudioSettings, Container as OutContainer, EncoderChoice, ExportSettings,
    HdrMode, VideoCodec as VCodec, VideoSettings,
};
use vidcut_core::render::{AudioPiece, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

struct Src {
    path: PathBuf,
    origin: f64,
}

fn src(spec: FixtureSpec) -> Src {
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    Src { path, origin }
}

fn v(s: &Src, source_frame: i64, frames: i64) -> VideoPiece {
    VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: s.path.clone(),
        origin: s.origin,
        source_frame,
        frames,
        image: None,
    }
}

fn a(s: &Src, fps: Rational, rate: u32, source_frame: i64, frames: i64) -> AudioPiece {
    AudioPiece::Source {
        speed: 1.0,
        media: "m".into(),
        path: s.path.clone(),
        stream: 0,
        origin: s.origin,
        source_sample: fps.frame_to_sample(source_frame, rate),
        samples: fps.frame_to_sample(frames, rate),
        gain_db: 0.0,
    }
}

/// Lossless H.264 and PCM in MKV: every barcode survives.
fn lossless() -> ExportSettings {
    ExportSettings {
        container: OutContainer::Mkv,
        video: Some(VideoSettings {
            codec: VCodec::H264,
            encoder: EncoderChoice::Cpu,
            quality: Some(0),
            bitrate: None,
            speed: Some("ultrafast".into()),
            profile: None,
            bit_depth: None,
            width: None,
            height: None,
            hdr: HdrMode::Sdr,
            passes: None,
            first_pass: Default::default(),
            multipass: Default::default(),
            lookahead: None,
            hdr_metadata: Default::default(),
            mastering_display: None,
            max_cll: None,
        }),
        audio: Some(AudioSettings {
            codec: ACodec::PcmS16,
            bitrate: None,
            sample_rate: None,
            channels: None,
            loudness: None,
        }),
        declick: false,
    }
}

/// Barcodes of every output frame.
fn barcodes(path: &Path) -> Vec<Option<u32>> {
    let mut r = VideoReader::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        out.push(read_barcode(&f).unwrap());
    }
    out
}

/// 400 segments alternating between two sources: memory must not grow with
/// the number of segments (the decoder of the current piece and the one
/// opening ahead).
#[test]
fn many_segments_keep_memory_flat() {
    let fps = Rational::new(30, 1).unwrap();
    let x = src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 360, (30, 1), 300).gop(60));
    let y = src(FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        640,
        360,
        (30, 1),
        300,
    ));
    let mut video = Vec::new();
    let mut lane = Vec::new();
    for i in 0..400i64 {
        let (s, at) = if i % 2 == 0 {
            (&x, (i * 37) % 290)
        } else {
            (&y, (i * 53) % 290)
        };
        video.push(v(s, at, 2));
        lane.push(a(s, fps, 48_000, at, 2));
    }
    let frames = video.iter().map(VideoPiece::frames).sum();
    let p = RenderPlan {
        fps,
        width: 640,
        height: 360,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video,
        audio: vec![lane],
        range: (0, frames),
        name: "t".into(),
    };
    let dir = tempfile::Builder::new()
        .prefix("vidcut-export-many-")
        .tempdir()
        .unwrap();
    let out = dir.path().join("out.mkv");
    let before = peak_rss_mb();
    export::run(
        &p,
        &lossless(),
        &out,
        &Availability::default(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_or_else(|e| panic!("export: {e}"));
    let grew = peak_rss_mb() - before;
    eprintln!("400 segments: peak RSS grew by {grew} MB");
    // About 100 MB here; a frame kept per segment would add 280 MB.
    assert!(grew < 250, "peak RSS grew by {grew} MB");
    assert_eq!(barcodes(&out).len(), 800);
}

/// The process's peak resident memory in MB, as the system counts it: a
/// reading or a panic, never a stand-in that would pass.
fn peak_rss_mb() -> i64 {
    #[cfg(target_os = "macos")]
    // SAFETY: getrusage fills the struct it is given.
    unsafe {
        let mut r: libc::rusage = std::mem::zeroed();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut r), 0, "getrusage");
        // In bytes (Linux counts kilobytes).
        r.ru_maxrss >> 20
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let status = std::fs::read_to_string("/proc/self/status").expect("/proc/self/status");
        let kb = status
            .lines()
            .find(|l| l.starts_with("VmHWM:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .expect("VmHWM in /proc/self/status");
        kb.parse::<i64>().unwrap() / 1024
    }
    #[cfg(windows)]
    // SAFETY: fills the counters it is given, their size set first.
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        assert!(
            GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0,
            "no memory counters: {}",
            std::io::Error::last_os_error()
        );
        (c.PeakWorkingSetSize >> 20) as i64
    }
}
