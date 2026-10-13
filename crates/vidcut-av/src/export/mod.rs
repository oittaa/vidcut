//! In-process export: render plans encoded with the FFmpeg libraries.
//!
//! One pass, constant memory: for each output frame, the next video frame is
//! decoded, normalized and encoded, and exactly the audio samples that frame
//! spans are mixed and encoded. Output goes to a hidden `.part` file that is
//! renamed into place only on success.

mod ahead;
pub mod audio;
pub mod encode;
pub mod renderer;
pub mod still;
pub mod video;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use vidcut_core::export::color::{self, ColorChoice, PictureKind};
use vidcut_core::export::metadata::{self, MetadataChoice, PieceMeta};
use vidcut_core::export::{EncoderChoice, ExportSettings, HdrMetadata, HdrMode, validate};
use vidcut_core::render::{RenderPlan, VideoPiece};

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, encoder, ffi, format, frame};
use audio::AudioMix;
use encode::{AudioEncoder, Availability, Vaapi};
use video::VideoFrames;

/// Progress of a running export.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    /// Frames done in this pass, of `total`.
    pub frames: i64,
    pub total: i64,
    /// Since the export started, over all passes.
    pub elapsed: Duration,
    /// The run over the range (1 or 2), of `passes`: an encoder that makes
    /// two passes by encoding twice reports both (decision D45).
    pub pass: u8,
    pub passes: u8,
}

impl Progress {
    /// Progress of a single pass.
    pub fn single(frames: i64, total: i64, elapsed: Duration) -> Self {
        Self {
            frames,
            total,
            elapsed,
            pass: 1,
            passes: 1,
        }
    }

    /// Frames done over all passes, as frames of one: half the total
    /// when the first of two is done.
    pub fn done(&self) -> i64 {
        let passes = i64::from(self.passes.max(1));
        (i64::from(self.pass.max(1) - 1) * self.total + self.frames) / passes
    }
}

/// What an export produced.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub output: PathBuf,
    pub frames: i64,
    pub samples: i64,
    pub video_encoder: Option<String>,
    pub audio_encoder: Option<String>,
    /// Passes the video encoder made over each frame (decision D45).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passes: Option<u8>,
    /// Video frames decoded and scaled on the GPU (NVDEC, CUDA) on their way
    /// to NVENC; the rest were made on the CPU.
    pub gpu_frames: i64,
    pub seconds: f64,
    /// What loudness normalization did (`audio.loudness`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loudness: Option<Normalized>,
    /// The output's dynamic range and bit depth, and why (decision D44),
    /// when the export chose them (`video.hdr = auto`, no `bit_depth`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorChoice>,
    /// The HDR metadata it writes, and why (decision D46), when the export
    /// chose it (`video.hdr_metadata = auto`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataChoice>,
}

/// What a piece's pictures carry, read from its file: its transfer and HDR
/// metadata. Each file is probed once.
pub fn probed_metadata() -> impl FnMut(&VideoPiece) -> PieceMeta {
    let mut known: std::collections::HashMap<PathBuf, PieceMeta> = Default::default();
    move |piece| {
        let VideoPiece::Source { path, .. } = piece else {
            return PieceMeta::default();
        };
        *known.entry(path.clone()).or_insert_with(|| {
            crate::probe::probe(path)
                .ok()
                .and_then(|i| i.video)
                .map_or(PieceMeta::default(), |v| PieceMeta {
                    hdr: match v.transfer.as_str() {
                        "smpte2084" => Some(vidcut_core::model::Hdr::Pq),
                        "arib-std-b67" => Some(vidcut_core::model::Hdr::Hlg),
                        _ => None,
                    },
                    mastering: v.hdr.mastering,
                    light: v.hdr.light,
                    hdr10plus: v.hdr.hdr10plus,
                    dolby_vision: v.hdr.dolby_vision,
                })
        })
    }
}

/// Chooses the HDR metadata of an export of `plan` with `settings`, whose
/// dynamic range is chosen already (decision D46); `None` without video.
pub fn choose_metadata(
    plan: &RenderPlan,
    settings: &ExportSettings,
    meta: impl FnMut(&VideoPiece) -> PieceMeta,
) -> Option<MetadataChoice> {
    let v = settings.video.as_ref()?;
    Some(metadata::choose(
        v,
        &plan.video,
        meta,
        encode::writes_dolby_vision(v),
    ))
}

/// What a piece's pictures are, read from its file: its HDR transfer and
/// bit depth. Each file is probed once.
pub fn probed_kinds() -> impl FnMut(&VideoPiece) -> PictureKind {
    let mut known: std::collections::HashMap<PathBuf, PictureKind> = Default::default();
    move |piece| {
        let VideoPiece::Source { path, .. } = piece else {
            return PictureKind::default();
        };
        *known.entry(path.clone()).or_insert_with(|| {
            crate::probe::probe(path).ok().and_then(|i| i.video).map_or(
                PictureKind::default(),
                |v| PictureKind {
                    hdr: match v.transfer.as_str() {
                        "smpte2084" => Some(vidcut_core::model::Hdr::Pq),
                        "arib-std-b67" => Some(vidcut_core::model::Hdr::Hlg),
                        _ => None,
                    },
                    depth: crate::probe::pixel_depth(&v.pix_fmt),
                },
            )
        })
    }
}

/// Chooses the dynamic range and bit depth of an export of `plan` with
/// `settings` from the pictures it shows (decision D44); `None` without
/// video. `kind` says what a piece's pictures are.
pub fn choose_color(
    plan: &RenderPlan,
    settings: &ExportSettings,
    available: &Availability,
    kind: impl FnMut(&VideoPiece) -> PictureKind,
) -> Option<ColorChoice> {
    let v = settings.video.as_ref()?;
    let share = color::picture_share(&plan.video, kind);
    Some(color::choose(v, &share, encode::abilities(v, available)))
}

/// Loudness normalization of an export: the mix was measured, then changed
/// by one gain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Normalized {
    /// The mix as measured, LUFS (`None`: silent).
    pub measured: Option<f64>,
    /// The gain applied, dB.
    pub gain_db: f64,
    /// Integrated loudness after the gain, LUFS.
    pub loudness: Option<f64>,
    /// Highest true peak after the gain, dBTP.
    pub true_peak: Option<f64>,
}

/// The ceiling loudness normalization keeps true peaks under, dBTP.
pub const TRUE_PEAK_CEILING: f64 = -1.0;

/// The gain that brings `measured` to `target`, held so the true peak stays
/// at or below [`TRUE_PEAK_CEILING`]. Silence gets none.
pub fn loudness_gain(measured: &vidcut_core::analysis::Loudness, target: f64) -> f64 {
    if !measured.integrated.is_finite() {
        return 0.0;
    }
    let gain = target - measured.integrated;
    if measured.true_peak.is_finite() {
        gain.min(TRUE_PEAK_CEILING - measured.true_peak)
    } else {
        gain
    }
}

/// Raised (as an error) when `cancel` is set during an export.
pub fn is_cancelled(e: &Error) -> bool {
    matches!(e, Error::Invalid(m) if m == CANCELLED)
}

pub(crate) const CANCELLED: &str = "export cancelled";

/// The format of frames in GPU memory for `enc`: 10-bit P010 for 10-bit
/// output, else NV12.
fn gpu_pixel(enc: &encode::VideoEncoder) -> format::Pixel {
    if enc.pixel == format::Pixel::P010LE {
        format::Pixel::P010LE
    } else {
        format::Pixel::NV12
    }
}

/// A CUDA device and frames for NVENC; `None` for other encoders or
/// without a device.
fn gpu_output(
    enc: &encode::VideoEncoder,
    w: u32,
    h: u32,
) -> Option<(crate::cuda::CudaDevice, crate::cuda::CudaFrames)> {
    if enc.kind != EncoderChoice::Nvenc {
        return None;
    }
    let device = crate::cuda::CudaDevice::open().ok()?;
    let frames = crate::cuda::CudaFrames::new(&device, gpu_pixel(enc), w, h).ok()?;
    Some((device, frames))
}

/// Encodes `plan` with `settings` into `output`.
///
/// The plan's sample rate must equal the audio encoder's (build the plan at the
/// export's sample rate). `progress` is called after every frame.
pub fn run(
    plan: &RenderPlan,
    settings: &ExportSettings,
    output: &Path,
    available: &Availability,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report> {
    crate::init();
    validate(settings).map_err(|e| Error::Invalid(e.message))?;
    // Settings left to the export choose from the files (decision D44).
    let mut settings = settings.clone();
    let mut chosen = None;
    if let Some(v) = settings.video.as_mut()
        && (v.hdr == HdrMode::Auto || v.bit_depth.is_none())
    {
        let c = choose_color(plan, &settings, available, probed_kinds())
            .expect("the settings have video");
        c.apply(settings.video.as_mut().expect("checked"));
        chosen = Some(c);
    }
    let mut metadata_chosen = None;
    if settings
        .video
        .as_ref()
        .is_some_and(|v| v.hdr_metadata == HdrMetadata::Auto)
    {
        let c =
            choose_metadata(plan, &settings, probed_metadata()).expect("the settings have video");
        c.apply(settings.video.as_mut().expect("checked"));
        metadata_chosen = Some(c);
    }
    let settings = &settings;
    // Nothing is made once cancelled: a GUI that quit without waiting for
    // a job removes only the partial files of what it heard was started.
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Invalid(CANCELLED.into()));
    }
    let part = partial_path(output);
    let result = encode(plan, settings, &part, output, available, cancel, progress);
    match result.map(|r| Report {
        color: chosen,
        metadata: metadata_chosen,
        ..r
    }) {
        Ok(report) => match std::fs::rename(&part, output) {
            Ok(()) => Ok(report),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(Error::Io(e))
            }
        },
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// The hidden folder beside `output` that holds a two-pass export's
/// statistics while it runs; removed when it ends, however it ends.
pub fn passes_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map_or_else(|| "export".into(), |n| n.to_string_lossy().into_owned());
    output.with_file_name(format!(".{name}.passes"))
}

/// A two-pass export's statistics folder ([`passes_path`]), removed when
/// dropped: after the second pass, or on failure or cancellation.
struct PassFiles {
    dir: PathBuf,
}

impl PassFiles {
    fn new(output: &Path) -> Result<Self> {
        let dir = passes_path(output);
        // A run that was killed may have left one.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn stats(&self) -> PathBuf {
        self.dir.join("stats")
    }
}

impl Drop for PassFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The first of two runs (decision D45): every frame of the range through
/// the encoder opened for it, its packets dropped, so it writes its
/// statistics. Returns the ones it hands back in memory (libvpx); x264 and
/// x265 finish their files as the encoder closes, here.
#[allow(clippy::too_many_arguments)]
fn first_run(
    plan: &RenderPlan,
    enc: &encode::VideoEncoder,
    mut encoder: encoder::Video,
    (w, h): (u32, u32),
    cancel: &AtomicBool,
    started: Instant,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<String>> {
    let mut frames =
        VideoFrames::new(&plan.video, plan.fps, w, h, enc.pixel, enc.hdr).with_dynamic(enc.dynamic);
    let mut p = ffmpeg::Packet::empty();
    for j in 0..plan.frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Invalid(CANCELLED.into()));
        }
        let mut f = frames
            .next_frame()?
            .ok_or_else(|| Error::Invalid("plan has fewer video frames than expected".into()))?;
        f.set_pts(Some(j));
        f.set_kind(ffmpeg::picture::Type::None);
        encoder
            .send_frame(&f)
            .ctx(|| format!("first pass: frame {j} with {}", enc.name))?;
        while encoder.receive_packet(&mut p).is_ok() {}
        progress(Progress {
            frames: j + 1,
            total: plan.frames,
            elapsed: started.elapsed(),
            pass: 1,
            passes: 2,
        });
    }
    encoder
        .send_eof()
        .ctx(|| "flushing the first pass".into())?;
    while encoder.receive_packet(&mut p).is_ok() {}
    // Ours to free; the encoder leaves it when it closes.
    let stats = unsafe {
        let raw = encoder.as_mut_ptr();
        let out = (*raw).stats_out;
        let text =
            (!out.is_null()).then(|| std::ffi::CStr::from_ptr(out).to_string_lossy().into_owned());
        ffi::av_freep(&mut (*raw).stats_out as *mut *mut std::ffi::c_char as *mut _);
        text
    };
    drop(encoder);
    Ok(stats)
}

/// The hidden file an export of `output` is written to, renamed into place
/// once it succeeds.
pub fn partial_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map_or_else(|| "export".into(), |n| n.to_string_lossy().into_owned());
    output.with_file_name(format!(".{name}.part"))
}

/// Fade length at cuts when de-clicking (the export setting `declick`).
pub const DECLICK_MS: u32 = 3;

pub(crate) fn write_packet(
    octx: &mut format::context::Output,
    packet: &ffmpeg::Packet,
) -> Result<()> {
    packet
        .write_interleaved(octx)
        .ctx(|| "writing packet".into())
}

struct VideoOut {
    encoder: encoder::Video,
    vaapi: Option<Vaapi>,
    frames: VideoFrames,
    enc_tb: ffmpeg::Rational,
    stream_tb: ffmpeg::Rational,
    name: String,
    passes: encode::Passes,
    /// Kept until the export ends: x264 and x265 read their statistics
    /// as they encode the second pass.
    _pass_files: Option<PassFiles>,
    /// Static HDR metadata for each frame (VAAPI reads it there).
    metadata: crate::hdr::Static,
    /// HEVC or AV1 keeping HDR10+, which vidcut puts into the packets:
    /// each frame's by pts.
    hdr10plus: Option<(ffmpeg::codec::Id, std::collections::HashMap<i64, Vec<u8>>)>,
}

impl VideoOut {
    fn drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut p = ffmpeg::Packet::empty();
        while self.encoder.receive_packet(&mut p).is_ok() {
            if let Some((id, waiting)) = self.hdr10plus.as_mut()
                && let Some(t35) = p.pts().and_then(|pts| waiting.remove(&pts))
            {
                crate::hdr::insert_hdr10plus(&mut p, *id, &t35)?;
            }
            p.set_stream(0);
            p.set_duration(1);
            p.rescale_ts(self.enc_tb, self.stream_tb);
            write_packet(octx, &p)?;
        }
        Ok(())
    }
}

struct AudioOut {
    encoder: encoder::Audio,
    spec: AudioEncoder,
    mix: AudioMix,
    stream: usize,
    stream_tb: ffmpeg::Rational,
    frame_size: usize,
    fifo: Vec<f32>,
    /// Samples sent to the encoder (its pts).
    sent: i64,
}

impl AudioOut {
    fn frame(&self, samples: &[f32]) -> frame::Audio {
        use format::Sample;
        use format::sample::Type::{Packed, Planar};
        let ch = usize::from(self.spec.channels);
        let n = samples.len() / ch;
        let layout = ffmpeg::ChannelLayout::default(i32::from(self.spec.channels));
        let mut f = frame::Audio::new(self.spec.sample, n, layout);
        f.set_rate(self.spec.rate);
        match self.spec.sample {
            Sample::F32(Planar) => {
                for c in 0..ch {
                    let plane = f.plane_mut::<f32>(c);
                    for i in 0..n {
                        plane[i] = samples[i * ch + c];
                    }
                }
            }
            Sample::F32(Packed) => {
                let data = f.data_mut(0);
                for (i, s) in samples.iter().enumerate() {
                    data[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
                }
            }
            Sample::I16(Packed) => {
                let data = f.data_mut(0);
                for (i, s) in samples.iter().enumerate() {
                    let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                    data[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
            Sample::I32(Packed) => {
                let data = f.data_mut(0);
                for (i, s) in samples.iter().enumerate() {
                    let v = (f64::from(s.clamp(-1.0, 1.0)) * 2_147_483_647.0).round() as i32;
                    data[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            other => unreachable!("unsupported encoder sample format {other:?}"),
        }
        f
    }

    /// Encodes whole frames from the fifo (and the rest when `last`).
    fn encode(&mut self, octx: &mut format::context::Output, last: bool) -> Result<()> {
        let ch = usize::from(self.spec.channels);
        let mut start = 0;
        while self.fifo.len() - start >= self.frame_size * ch || (last && self.fifo.len() > start) {
            let take = (self.fifo.len() - start).min(self.frame_size * ch);
            let mut f = self.frame(&self.fifo[start..start + take]);
            f.set_pts(Some(self.sent));
            self.sent += (take / ch) as i64;
            start += take;
            self.encoder
                .send_frame(&f)
                .ctx(|| format!("encoding audio with {}", self.spec.name))?;
            self.drain(octx)?;
        }
        self.fifo.drain(..start);
        Ok(())
    }

    fn drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut p = ffmpeg::Packet::empty();
        let enc_tb = ffmpeg::Rational(1, self.spec.rate as i32);
        while self.encoder.receive_packet(&mut p).is_ok() {
            p.set_stream(self.stream);
            p.rescale_ts(enc_tb, self.stream_tb);
            write_packet(octx, &p)?;
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn encode(
    plan: &RenderPlan,
    settings: &ExportSettings,
    part: &Path,
    output: &Path,
    available: &Availability,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report> {
    let started = Instant::now();
    let fps = ffmpeg::Rational(plan.fps.num() as i32, plan.fps.den() as i32);
    let mut octx = format::output_as(part, settings.container.muxer())
        .ctx(|| format!("creating {}", output.display()))?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

    let mut video = match &settings.video {
        Some(v) => {
            let (w, h) = (
                v.width.unwrap_or(plan.width),
                v.height.unwrap_or(plan.height),
            );
            // The first encoder that opens; if none does, why each didn't.
            let mut failed: Vec<(Option<String>, Error)> = Vec::new();
            let mut opened = None;
            let mut pass_files = None;
            for candidate in encode::video_encoders(v, available) {
                let enc = match candidate {
                    Ok(enc) => enc,
                    Err(e) => {
                        failed.push((None, e));
                        continue;
                    }
                };
                // Two runs open it for the first, writing statistics.
                let stats = match enc.passes {
                    encode::Passes::TwoRuns { .. } => {
                        let files = PassFiles::new(output)?;
                        let stats = files.stats();
                        pass_files = Some(files);
                        Some(stats)
                    }
                    _ => None,
                };
                let pass = match &stats {
                    Some(stats) => encode::Pass::First { stats },
                    None => encode::Pass::Only,
                };
                // NVENC with SDR output takes frames in GPU memory, so pieces
                // decoded on NVDEC never leave it; frames in memory if not.
                if let Some((device, frames)) = gpu_output(&enc, w, h)
                    && let Ok(o) = encode::open_video(
                        &enc,
                        w,
                        h,
                        fps,
                        global_header,
                        Some(frames.as_ptr()),
                        pass,
                    )
                {
                    opened = Some((enc, o, Some((device, frames))));
                    break;
                }
                match encode::open_video(&enc, w, h, fps, global_header, None, pass) {
                    Ok(o) => {
                        opened = Some((enc, o, None));
                        break;
                    }
                    Err(e) => failed.push((Some(enc.name.clone()), e)),
                }
            }
            let Some((enc, (encoder, vaapi), gpu)) = opened else {
                if failed.len() == 1 {
                    return Err(failed.remove(0).1);
                }
                let why: Vec<String> = failed
                    .iter()
                    .map(|(name, e)| match name {
                        Some(n) => format!("{n}: {e}"),
                        None => e.to_string(),
                    })
                    .collect();
                return Err(Error::Invalid(format!(
                    "no video encoder would open: {}",
                    why.join("; ")
                )));
            };
            // Two runs: the first measures, then the encoder opens again
            // for the second with what it wrote.
            let (encoder, vaapi) = match &pass_files {
                Some(files) if enc.passes.runs() == 2 => {
                    let buffer = first_run(plan, &enc, encoder, (w, h), cancel, started, progress)?;
                    let stats = files.stats();
                    let second = encode::Pass::Second {
                        stats: &stats,
                        buffer: buffer.as_deref(),
                    };
                    encode::open_video(&enc, w, h, fps, global_header, None, second)?
                }
                _ => (encoder, vaapi),
            };
            let mut ost = octx
                .add_stream(encoder::find_by_name(&enc.name))
                .ctx(|| "adding video stream".into())?;
            ost.set_parameters(&encoder);
            ost.set_time_base(fps.invert());
            ost.set_avg_frame_rate(fps);
            let frames = match gpu {
                Some((device, frames)) => {
                    VideoFrames::new(&plan.video, plan.fps, w, h, gpu_pixel(&enc), enc.hdr)
                        .with_gpu(device, frames)
                }
                None => VideoFrames::new(&plan.video, plan.fps, w, h, enc.pixel, enc.hdr),
            }
            .with_dynamic(enc.dynamic);
            // HDR10+ goes into HEVC and AV1 packets here; libvpx-vp9 takes
            // it from the frames, and WebM and Matroska carry it.
            let id = encoder.id();
            let hdr10plus = (enc.dynamic.hdr10plus
                && matches!(id, ffmpeg::codec::Id::HEVC | ffmpeg::codec::Id::AV1))
            .then(|| (id, std::collections::HashMap::new()));
            Some(VideoOut {
                encoder,
                vaapi,
                frames,
                enc_tb: fps.invert(),
                stream_tb: fps.invert(),
                name: enc.name.clone(),
                passes: enc.passes,
                _pass_files: pass_files,
                metadata: enc.metadata,
                hdr10plus,
            })
        }
        None => None,
    };

    let mut normalized = None;
    let mut audio = match &settings.audio {
        Some(a) => {
            let spec = encode::resolve_audio(a, plan.sample_rate, plan.channels)?;
            if spec.rate != plan.sample_rate {
                return Err(Error::Invalid(format!(
                    "plan is at {} Hz but the audio encoder runs at {} Hz",
                    plan.sample_rate, spec.rate
                )));
            }
            let codec = encoder::find_by_name(&spec.name)
                .ok_or_else(|| Error::Invalid(format!("no {}", spec.name)))?;
            let mut ctx = ffmpeg::codec::context::Context::new_with_codec(codec)
                .encoder()
                .audio()
                .ctx(|| format!("creating {}", spec.name))?;
            ctx.set_rate(spec.rate as i32);
            ctx.set_channel_layout(ffmpeg::ChannelLayout::default(i32::from(spec.channels)));
            ctx.set_format(spec.sample);
            ctx.set_time_base(ffmpeg::Rational(1, spec.rate as i32));
            if let Some(b) = spec.bitrate {
                ctx.set_bit_rate(b as usize);
            }
            if spec.name == "pcm_s24le" || spec.name == "flac" {
                unsafe { (*ctx.as_mut_ptr()).bits_per_raw_sample = 24 };
            }
            if global_header {
                ctx.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
            }
            let encoder = ctx
                .open_as(codec)
                .ctx(|| format!("opening {}", spec.name))?;
            let index = usize::from(video.is_some());
            let mut ost = octx
                .add_stream(codec)
                .ctx(|| "adding audio stream".into())?;
            ost.set_parameters(&encoder);
            ost.set_time_base(ffmpeg::Rational(1, spec.rate as i32));
            let frame_size = match encoder.frame_size() {
                0 => 1024,
                n => n as usize,
            };
            let declick = if settings.declick { DECLICK_MS } else { 0 };
            let mut mix = AudioMix::new(&plan.audio, plan.sample_rate, spec.channels, declick);
            if let Some(target) = a.loudness {
                // Measure the mix exactly as it will be encoded, then gain.
                let measured = crate::analysis::mix_loudness(
                    &plan.audio,
                    plan.sample_rate,
                    spec.channels,
                    declick,
                    plan.samples(),
                    Some(cancel),
                )?;
                let gain = loudness_gain(&measured, target);
                mix.set_gain_db(gain);
                let finite = |v: f64| v.is_finite().then_some(v);
                normalized = Some(Normalized {
                    measured: finite(measured.integrated),
                    gain_db: gain,
                    loudness: finite(measured.integrated + gain),
                    true_peak: finite(measured.true_peak + gain),
                });
            }
            Some(AudioOut {
                encoder,
                mix,
                spec,
                stream: index,
                stream_tb: ffmpeg::Rational(1, plan.sample_rate as i32),
                frame_size,
                fifo: Vec::new(),
                sent: 0,
            })
        }
        None => None,
    };

    let mut opts = ffmpeg::Dictionary::new();
    if matches!(settings.container.muxer(), "mp4" | "mov" | "ipod") {
        opts.set("movflags", "+faststart");
        // MP4 and MOV write Dolby Vision's `dvcC`/`dvvC` box only so.
        if settings
            .video
            .as_ref()
            .is_some_and(|v| v.hdr_metadata.dolby_vision())
        {
            opts.set("strict", "unofficial");
        }
    }
    octx.write_header_with(opts)
        .ctx(|| "writing header".into())?;
    if let Some(v) = video.as_mut() {
        v.stream_tb = octx.stream(0).expect("video stream").time_base();
    }
    if let Some(a) = audio.as_mut() {
        a.stream_tb = octx.stream(a.stream).expect("audio stream").time_base();
    }

    let mut produced = 0i64;
    // This is the last run over the range: the second of two, or the only.
    let runs = video.as_ref().map_or(1, |v| v.passes.runs());
    for j in 0..plan.frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Invalid(CANCELLED.into()));
        }
        if let Some(v) = video.as_mut() {
            let mut f = v.frames.next_frame()?.ok_or_else(|| {
                Error::Invalid("plan has fewer video frames than expected".into())
            })?;
            f.set_pts(Some(j));
            // A decoded picture keeps its source's type (I, P, B), which
            // encoders take as an order; the encoder chooses its own, as
            // in the ffmpeg command line.
            f.set_kind(ffmpeg::picture::Type::None);
            if let Some((_, waiting)) = v.hdr10plus.as_mut()
                && let Some(t35) = crate::hdr::hdr10plus_t35(&f)
            {
                waiting.insert(j, t35);
            }
            let mut f = match &v.vaapi {
                Some(hw) => hw.upload(&f)?,
                None => f,
            };
            if !v.metadata.is_empty() {
                crate::hdr::attach(&mut f, &v.metadata)?;
            }
            v.encoder
                .send_frame(&f)
                .ctx(|| format!("encoding frame {j} with {}", v.name))?;
            v.drain(&mut octx)?;
        }
        if let Some(a) = audio.as_mut() {
            let until = plan.fps.frame_to_sample(j + 1, plan.sample_rate);
            let samples = a.mix.read((until - produced) as usize)?;
            produced = until;
            a.fifo.extend_from_slice(&samples);
            a.encode(&mut octx, false)?;
        }
        progress(Progress {
            frames: j + 1,
            total: plan.frames,
            elapsed: started.elapsed(),
            pass: runs,
            passes: runs,
        });
    }
    if let Some(a) = audio.as_mut() {
        a.encode(&mut octx, true)?;
        a.encoder
            .send_eof()
            .ctx(|| "flushing audio encoder".into())?;
        a.drain(&mut octx)?;
    }
    if let Some(v) = video.as_mut() {
        v.encoder
            .send_eof()
            .ctx(|| "flushing video encoder".into())?;
        v.drain(&mut octx)?;
    }
    octx.write_trailer().ctx(|| "writing trailer".into())?;
    Ok(Report {
        output: output.to_path_buf(),
        frames: plan.frames,
        samples: if audio.is_some() { plan.samples() } else { 0 },
        gpu_frames: video.as_ref().map_or(0, |v| v.frames.gpu_frames()),
        passes: video.as_ref().map(|v| v.passes.count()),
        video_encoder: video.map(|v| v.name),
        audio_encoder: audio.map(|a| a.spec.name),
        seconds: started.elapsed().as_secs_f64(),
        loudness: normalized,
        color: None,
        metadata: None,
    })
}
