mod ahead;
pub mod audio;
pub mod encode;
pub(crate) mod make;
pub(crate) mod normalize;
pub mod renderer;
pub mod still;
pub mod subtitles;
pub mod video;
pub(crate) mod worker;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use vidcut_core::export::color::{self, ColorChoice, PictureKind};
use vidcut_core::export::metadata::{self, Measured, MetadataChoice, PieceMeta};
use vidcut_core::export::{
    AlphaOut, Container, EncoderChoice, ExportSettings, HdrMetadata, HdrMode, Numbering, Rounded,
    VideoSettings, validate,
};
use vidcut_core::model::{Hdr, SubtitleStyle};
use vidcut_core::render::{RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, encoder, ffi, format, frame};
use audio::AudioMix;
use encode::{AudioEncoder, Availability, Upload};
use subtitles::{BurnNote, StreamNote, SubtitleOut, Subtitles};
use video::VideoFrames;
use worker::{Inbox, Worker};

#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    pub frames: i64,
    pub total: i64,
    pub elapsed: Duration,
    pub pass: u8,
    pub passes: u8,
    pub measuring: bool,
    pub gpu_frames: i64,
    pub gpu_decoded: i64,
    pub copied: vidcut_core::editor::Copied,
    pub stage: Option<Stage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Downloading,
    Listening,
    Loading,
    Transcribing,
    Speakers,
    Uploading,
    Waiting,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "note", rename_all = "snake_case")]
pub enum Note {
    Video(VideoNote),
    Audio(AudioNote),
    Color {
        #[serde(skip_serializing_if = "Option::is_none")]
        color: Option<ColorChoice>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<MetadataChoice>,
    },
    Tried(Tried),
    Source(SourceNote),
    Rounded(Rounded),
    Upscaled(UpscaleNote),
    Downloading {
        name: String,
    },
    Speech(SpeechNote),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpeechNote {
    pub model: String,
    pub find: vidcut_core::request::FindSpeech,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speakers_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VideoNote {
    pub encoder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub pixel: String,
    pub bits: u8,
    pub passes: u8,
    pub gpu_frames_in: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AudioNote {
    pub encoder: String,
    pub rate: u32,
    pub channels: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loudness: Option<Normalized>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tried {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpscaleNote {
    pub path: PathBuf,
    pub how: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceNote {
    pub path: PathBuf,
    pub decoding: crate::decode::Decoding,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub went: Option<String>,
}

fn tried(encoder: Option<&str>, e: &Error) -> Tried {
    Tried {
        encoder: encoder.map(String::from),
        error: e.to_string(),
    }
}

impl Tried {
    pub fn text(&self) -> String {
        match &self.encoder {
            Some(e) if !self.error.starts_with(e.as_str()) => format!("{e}: {}", self.error),
            _ => self.error.clone(),
        }
    }
}

impl Progress {
    pub fn single(frames: i64, total: i64, elapsed: Duration) -> Self {
        Self {
            frames,
            total,
            elapsed,
            pass: 1,
            passes: 1,
            ..Self::default()
        }
    }

    pub fn stage(stage: Stage, done: i64, total: i64, elapsed: Duration) -> Self {
        Self {
            stage: Some(stage),
            ..Self::single(done, total, elapsed)
        }
    }

    pub fn measuring(share: f64, elapsed: Duration) -> Self {
        Self {
            frames: (share.clamp(0.0, 1.0) * 1000.0) as i64,
            total: 1000,
            measuring: true,
            ..Self::single(0, 0, elapsed)
        }
    }

    pub fn done(&self) -> i64 {
        let passes = i64::from(self.passes.max(1));
        (i64::from(self.pass.max(1) - 1) * self.total + self.frames) / passes
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub output: PathBuf,
    pub frames: i64,
    pub samples: i64,
    pub video_encoder: Option<String>,
    pub audio_encoder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passes: Option<u8>,
    pub gpu_frames: i64,
    pub gpu_decoded: i64,
    pub seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loudness: Option<Normalized>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataChoice>,
    pub copies: vidcut_core::editor::Copied,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub through_memory: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tried: Vec<Tried>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceNote>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtitle_streams: Vec<StreamNote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burned_in: Option<BurnNote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rounded: Option<Rounded>,
}

pub fn probed_metadata<'a>(
    fps: Rational,
    v: &'a VideoSettings,
    decoder: Option<&'a crate::hw::GpuDecoder>,
    cancel: Option<&'a AtomicBool>,
) -> impl FnMut(&VideoPiece) -> PieceMeta + 'a {
    let mut probe = probed_files(decoder);
    let lights = crate::light::Lights::default();
    move |piece| {
        let mut m = probe(piece);
        m.measured = measured_light(&lights, piece, fps, v, &m, true, decoder, cancel);
        m
    }
}

pub fn needs_light(v: &VideoSettings, m: &PieceMeta) -> bool {
    if v.hdr.transfer() != Some(Hdr::Pq)
        || v.hdr_metadata == HdrMetadata::None
        || v.image_bits().is_some()
    {
        return false;
    }
    match v.max_cll.as_deref() {
        Some(metadata::MEASURE) => true,
        Some(_) => false,
        None => match m.hdr {
            Some(Hdr::Pq) => m.light.is_none(),
            Some(Hdr::Hlg) => true,
            None => false,
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub fn measured_light(
    lights: &crate::light::Lights,
    piece: &VideoPiece,
    fps: Rational,
    v: &VideoSettings,
    m: &PieceMeta,
    measure_now: bool,
    decoder: Option<&crate::hw::GpuDecoder>,
    cancel: Option<&AtomicBool>,
) -> Measured {
    if !needs_light(v, m) {
        return Measured::No;
    }
    match lights.piece_level(piece, fps, measure_now, decoder, cancel, &mut |_| {}) {
        Ok(Some(l)) => Measured::Known(l),
        Ok(None) if !measure_now => Measured::Pending,
        Err(e) if is_cancelled(&e) => Measured::No,
        Ok(None) | Err(_) => Measured::Failed,
    }
}

pub fn makes_hdr10plus(v: &VideoSettings, m: &PieceMeta) -> bool {
    v.hdr.transfer() == Some(Hdr::Pq) && v.hdr_metadata.hdr10plus() && !m.hdr10plus
}

pub fn probed_files(
    decoder: Option<&crate::hw::GpuDecoder>,
) -> impl FnMut(&VideoPiece) -> PieceMeta + '_ {
    let mut known: std::collections::HashMap<PathBuf, PieceMeta> = Default::default();
    move |piece| {
        let VideoPiece::Source { path, .. } = piece else {
            return PieceMeta::default();
        };
        *known.entry(path.clone()).or_insert_with(|| {
            crate::probe::probe(path, decoder)
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
                    measured: Measured::No,
                })
        })
    }
}

pub fn choose_metadata(
    plan: &RenderPlan,
    settings: &ExportSettings,
    mut meta: impl FnMut(&VideoPiece) -> PieceMeta,
) -> Option<MetadataChoice> {
    let v = settings.video.as_ref()?;
    if plan.has_lut() {
        return Some(metadata::choose_through_lut(v));
    }
    let sdr = plan.colors.is_none();
    Some(metadata::choose(
        v,
        &plan.video,
        |piece| match sdr {
            true => PieceMeta::default(),
            false => meta(piece),
        },
        encode::writes_dolby_vision(v),
    ))
}

pub fn probed_kinds(
    decoder: Option<&crate::hw::GpuDecoder>,
) -> impl FnMut(&VideoPiece) -> PictureKind + '_ {
    let mut known: std::collections::HashMap<PathBuf, PictureKind> = Default::default();
    move |piece| {
        let VideoPiece::Source { path, .. } = piece else {
            return PictureKind::default();
        };
        *known.entry(path.clone()).or_insert_with(|| {
            crate::probe::probe(path, decoder)
                .ok()
                .and_then(|i| i.video)
                .map_or(PictureKind::default(), |v| PictureKind {
                    hdr: match v.transfer.as_str() {
                        "smpte2084" => Some(vidcut_core::model::Hdr::Pq),
                        "arib-std-b67" => Some(vidcut_core::model::Hdr::Hlg),
                        _ => None,
                    },
                    depth: crate::probe::pixel_depth(&v.pix_fmt),
                })
        })
    }
}

pub fn choose_color(
    plan: &RenderPlan,
    settings: &ExportSettings,
    available: &Availability,
    kind: impl FnMut(&VideoPiece) -> PictureKind,
) -> Option<ColorChoice> {
    let v = settings.video.as_ref()?;
    let share = color::picture_share(&plan.video, kind);
    Some(color::choose(
        v,
        &share,
        encode::abilities(v, available),
        plan.colors,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Normalized {
    pub measured: Option<f64>,
    pub gain_db: f64,
    pub loudness: Option<f64>,
    pub true_peak: Option<f64>,
}

pub const TRUE_PEAK_CEILING: f64 = -1.0;

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

pub fn is_cancelled(e: &Error) -> bool {
    matches!(e, Error::Invalid(m) if m == CANCELLED)
}

pub(crate) const CANCELLED: &str = "export cancelled";

pub(crate) fn gpu_pixel(enc: &encode::VideoEncoder) -> format::Pixel {
    if enc.pixel == format::Pixel::P010LE {
        format::Pixel::P010LE
    } else {
        format::Pixel::NV12
    }
}

pub(crate) fn gpu_output(
    enc: &encode::VideoEncoder,
    w: u32,
    h: u32,
) -> Option<(crate::cuda::CudaDevice, crate::cuda::CudaFrames)> {
    if enc.kind != EncoderChoice::Nvenc {
        return None;
    }
    let device = crate::cuda::CudaDevice::open()
        .and_then(|d| d.with_own_stream())
        .ok()?;
    let frames = crate::cuda::CudaFrames::new(&device, gpu_pixel(enc), w, h).ok()?;
    Some((device, frames))
}

pub fn run(
    plan: &RenderPlan,
    settings: &ExportSettings,
    output: &Path,
    available: &Availability,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report> {
    let none = Subtitles::default();
    run_decoding_on(
        plan,
        settings,
        output,
        Numbering {
            first: plan.range.0,
            record: plan.range.0,
            drop_frame: false,
        },
        available,
        None,
        &none,
        cancel,
        progress,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_decoding_on(
    plan: &RenderPlan,
    settings: &ExportSettings,
    output: &Path,
    numbering: Numbering,
    available: &Availability,
    decoder: Option<&crate::hw::GpuDecoder>,
    subtitles: &Subtitles,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report> {
    crate::init();
    crate::screen::retry_lost();
    validate(settings).map_err(|e| Error::Invalid(e.message))?;
    let copies = std::sync::Arc::new(crate::copies::Copies::default());
    let _counting = crate::copies::Guard::set(copies.clone());
    let mut settings = settings.clone();
    let mut chosen = None;
    if let Some(v) = settings.video.as_mut()
        && (v.hdr == HdrMode::Auto || v.bit_depth.is_none())
    {
        let c = choose_color(plan, &settings, available, probed_kinds(decoder))
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
        let v = settings.video.clone().expect("checked");
        let probed = probed_metadata(plan.fps, &v, decoder, Some(cancel));
        let c = choose_metadata(plan, &settings, probed).expect("the settings have video");
        c.apply(settings.video.as_mut().expect("checked"));
        metadata_chosen = Some(c);
    }
    if chosen.is_some() || metadata_chosen.is_some() {
        crate::progress::note(Note::Color {
            color: chosen.clone(),
            metadata: metadata_chosen.clone(),
        });
    }
    let settings = &settings;
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Invalid(CANCELLED.into()));
    }
    let (part, file) = vidcut_core::temp::file(output)?;
    drop(file);
    let report = encode(
        plan,
        settings,
        part.path(),
        output,
        numbering,
        available,
        decoder,
        subtitles,
        cancel,
        progress,
    )?;
    if !settings.container.image_sequence() {
        part.keep(output)?;
    }
    Ok(Report {
        color: chosen,
        metadata: metadata_chosen,
        copies: copies.counted(),
        ..report
    })
}

struct FrameFiles {
    dir: vidcut_core::temp::Temp,
    settings: ExportSettings,
    output: PathBuf,
    numbering: Numbering,
    fps: Rational,
    written: i64,
    associated: bool,
}

const EXR_START: [u8; 8] = [0x76, 0x2f, 0x31, 0x01, 2, 0, 0, 0];
const EXR_TIMECODE_NAME: &[u8] = b"timeCode";
const EXR_TIMECODE_TYPE: &[u8] = b"timecode";
const EXR_LINE_ORDER_NAME: &[u8] = b"lineOrder";

fn exr_layout(data: &[u8]) -> Option<(usize, usize)> {
    if !data.starts_with(&EXR_START) {
        return None;
    }
    let zero = |from: usize| Some(from + data.get(from..)?.iter().position(|&b| b == 0)?);
    let mut at = EXR_START.len();
    while *data.get(at)? != 0 {
        let name = zero(at)?;
        if &data[at..name] == EXR_TIMECODE_NAME {
            return None;
        }
        let kind = zero(name + 1)?;
        if &data[at..name] == EXR_LINE_ORDER_NAME && *data.get(kind + 5)? != 0 {
            return None;
        }
        let size = i32::from_le_bytes(data.get(kind + 1..kind + 5)?.try_into().ok()?);
        at = kind + 5 + usize::try_from(size).ok()?;
    }
    let table = at + 1;
    let first = u64::from_le_bytes(data.get(table..table + 8)?.try_into().ok()?);
    let first = usize::try_from(first).ok()?;
    let length = first.checked_sub(table)?;
    (length > 0 && length % 8 == 0 && first <= data.len()).then_some((at, first))
}

fn exr_timecode(time: u32) -> Vec<u8> {
    let mut attribute = [EXR_TIMECODE_NAME, b"\0", EXR_TIMECODE_TYPE, b"\0"].concat();
    attribute.extend(8i32.to_le_bytes());
    attribute.extend(time.to_le_bytes());
    attribute.extend(0u32.to_le_bytes());
    attribute
}

const TIFF_EXTRA_SAMPLES: u16 = 338;
const TIFF_SHORT: u16 = 3;
const TIFF_ASSOCIATED_ALPHA: u16 = 1;

fn tiff_extra_samples(data: &[u8]) -> Option<usize> {
    if !data.starts_with(b"II*\0") {
        return None;
    }
    let short = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
    let long = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let ifd = usize::try_from(long(4)?).ok()?;
    (0..usize::from(short(ifd)?))
        .map(|i| ifd + 2 + i * 12)
        .find_map(|entry| {
            let found = short(entry)? == TIFF_EXTRA_SAMPLES
                && short(entry + 2)? == TIFF_SHORT
                && long(entry + 4)? == 1;
            found.then_some(entry + 8)
        })
}

impl FrameFiles {
    fn new(
        settings: &ExportSettings,
        output: &Path,
        numbering: Numbering,
        fps: Rational,
    ) -> Result<Self> {
        let associated = settings.container == Container::Tiff
            && settings
                .video
                .as_ref()
                .is_some_and(|v| v.alpha && v.alpha_mode == AlphaOut::Premultiplied);
        Ok(Self {
            dir: vidcut_core::temp::dir(output)?,
            settings: settings.clone(),
            output: output.to_path_buf(),
            numbering,
            fps,
            written: 0,
            associated,
        })
    }

    fn write(&mut self, packet: &ffmpeg::Packet) -> Result<()> {
        use std::io::Write;
        let path = self.dir.path().join(self.written.to_string());
        let data = packet.data().unwrap_or_default();
        let alpha = self.associated.then(|| tiff_extra_samples(data)).flatten();
        let exr = (self.settings.container == Container::Exr)
            .then(|| exr_layout(data))
            .flatten();
        let time = vidcut_core::time::packed_timecode(
            self.numbering.record + self.written,
            self.fps,
            self.numbering.drop_frame,
        );
        let mut file = std::fs::File::create(path)?;
        match (alpha, exr.zip(time)) {
            (Some(at), _) => {
                file.write_all(&data[..at])?;
                file.write_all(&TIFF_ASSOCIATED_ALPHA.to_le_bytes())?;
                file.write_all(&data[at + 2..])?;
            }
            (None, Some(((end, first), time))) => {
                let attribute = exr_timecode(time);
                let shift = attribute.len() as u64;
                let offsets: Vec<u8> = data[end + 1..first]
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .flat_map(|&o| (u64::from_le_bytes(o) + shift).to_le_bytes())
                    .collect();
                file.write_all(&data[..end])?;
                file.write_all(&attribute)?;
                file.write_all(&data[end..end + 1])?;
                file.write_all(&offsets)?;
                file.write_all(&data[first..])?;
            }
            (None, None) => file.write_all(data)?,
        }
        self.written += 1;
        Ok(())
    }

    fn keep(&self) -> Result<()> {
        for i in 0..self.written {
            let file = self
                .settings
                .frame_file(&self.output, self.numbering.first + i);
            std::fs::rename(self.dir.path().join(i.to_string()), file)?;
        }
        Ok(())
    }
}

struct PassFiles {
    dir: vidcut_core::temp::Temp,
}

impl PassFiles {
    fn new(output: &Path) -> Result<Self> {
        Ok(Self {
            dir: vidcut_core::temp::dir(output)?,
        })
    }

    fn stats(&self) -> PathBuf {
        self.dir.path().join("stats")
    }
}

fn made_hdr10plus_light(
    plan: &RenderPlan,
    enc: &encode::VideoEncoder,
    decoder: Option<&crate::hw::GpuDecoder>,
    cancel: &AtomicBool,
) -> Result<std::collections::HashMap<PathBuf, std::sync::Arc<crate::light::LightAnalysis>>> {
    let mut out = std::collections::HashMap::new();
    if !enc.dynamic.hdr10plus {
        return Ok(out);
    }
    let mut probe = probed_files(decoder);
    let lights = crate::light::Lights::default();
    for (piece, _) in plan.video.iter().flat_map(VideoPiece::layers) {
        let Some(span) = crate::light::Span::of(piece, plan.fps) else {
            continue;
        };
        if probe(piece).hdr10plus {
            continue;
        }
        match lights.analysis(&span, true, decoder, Some(cancel), &mut |_| {}) {
            Ok(Some(a)) => {
                out.insert(span.input.path().to_path_buf(), a);
            }
            Err(e) if is_cancelled(&e) => return Err(e),
            _ => {}
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn first_run(
    plan: &RenderPlan,
    enc: &encode::VideoEncoder,
    mut encoder: encoder::Video,
    ((w, h), written): ((u32, u32), (u32, u32)),
    decoder: Option<&crate::hw::GpuDecoder>,
    burn: (Vec<subtitles::Cue>, SubtitleStyle),
    cancel: &AtomicBool,
    started: Instant,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<String>> {
    let mut frames = VideoFrames::new(&plan.video, plan.fps, w, h, enc.pixel, enc.hdr)
        .written_at(written)
        .with_timeline(plan.colors)?
        .with_output_lut(plan.output_lut.as_deref())?
        .with_dynamic(enc.dynamic)
        .with_burn(burn.0, burn.1, plan.height);
    if let Some(d) = decoder {
        frames = frames.decode_on(d.clone());
    }
    let (name, kind) = (enc.name.clone(), enc.kind);
    let mut passing = Worker::spawn(
        worker::AHEAD,
        move |mut pictures: Inbox<(i64, frame::Video)>| {
            let mut p = ffmpeg::Packet::empty();
            while let Some((j, f)) = pictures.recv()? {
                crate::copies::to_encoder(&f, kind);
                encoder
                    .send_frame(&f)
                    .ctx(|| format!("first pass: frame {j} with {name}"))?;
                while encoder.receive_packet(&mut p).is_ok() {}
            }
            encoder
                .send_eof()
                .ctx(|| "flushing the first pass".into())?;
            while encoder.receive_packet(&mut p).is_ok() {}
            let stats = unsafe {
                let raw = encoder.as_mut_ptr();
                let out = (*raw).stats_out;
                let text = (!out.is_null())
                    .then(|| std::ffi::CStr::from_ptr(out).to_string_lossy().into_owned());
                ffi::av_freep(&mut (*raw).stats_out as *mut *mut std::ffi::c_char as *mut _);
                text
            };
            drop(encoder);
            Ok(stats)
        },
    )?;
    let made = (|| -> Result<()> {
        for j in 0..plan.frames {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Invalid(CANCELLED.into()));
            }
            let mut f = frames.next_frame()?.ok_or_else(|| {
                Error::Invalid("plan has fewer video frames than expected".into())
            })?;
            f.set_pts(Some(j));
            f.set_kind(ffmpeg::picture::Type::None);
            passing.send((j, f))?;
            progress(Progress {
                frames: j + 1,
                total: plan.frames,
                elapsed: started.elapsed(),
                pass: 1,
                passes: 2,
                measuring: false,
                gpu_frames: frames.gpu_frames(),
                gpu_decoded: frames.gpu_decoded(),
                copied: crate::copies::counted(),
                stage: None,
            });
        }
        Ok(())
    })();
    match made {
        Ok(()) => passing.finish(),
        Err(e) => Err(passing.abandon(e)),
    }
}

pub const DECLICK_MS: u32 = 3;

pub(crate) fn write_packet(
    octx: &mut format::context::Output,
    packet: &ffmpeg::Packet,
) -> Result<()> {
    packet
        .write_interleaved(octx)
        .ctx(|| "writing packet".into())
}

struct VideoIn {
    frames: VideoFrames,
    vaapi: Option<Upload>,
    name: String,
    passes: encode::Passes,
    _pass_files: Option<PassFiles>,
    metadata: crate::hdr::Static,
    hdr10plus: bool,
}

impl VideoIn {
    fn next(&mut self, j: i64) -> Result<(frame::Video, Option<Vec<u8>>)> {
        let mut f = self
            .frames
            .next_frame()?
            .ok_or_else(|| Error::Invalid("plan has fewer video frames than expected".into()))?;
        f.set_pts(Some(j));
        f.set_kind(ffmpeg::picture::Type::None);
        let t35 = self
            .hdr10plus
            .then(|| crate::hdr::hdr10plus_t35(&f))
            .flatten();
        let mut f = match &self.vaapi {
            Some(hw) if !crate::hw::on_gpu(&f) => hw.upload(&f)?,
            _ => f,
        };
        if !self.metadata.is_empty() {
            crate::hdr::attach(&mut f, &self.metadata)?;
        }
        Ok((f, t35))
    }
}

struct VideoOut {
    encoder: encoder::Video,
    enc_tb: ffmpeg::Rational,
    stream_tb: ffmpeg::Rational,
    name: String,
    metadata: crate::hdr::Static,
    hdr10plus: Option<(ffmpeg::codec::Id, std::collections::HashMap<i64, Vec<u8>>)>,
    codec: ffmpeg::codec::Id,
    kind: EncoderChoice,
    files: Option<FrameFiles>,
}

impl VideoOut {
    fn encode(
        &mut self,
        octx: &mut format::context::Output,
        j: i64,
        mut f: frame::Video,
        t35: Option<Vec<u8>>,
        spent: &mut Spent,
    ) -> Result<()> {
        if let (Some((_, waiting)), Some(t35)) = (self.hdr10plus.as_mut(), t35) {
            waiting.insert(j, t35);
        }
        let clock = Instant::now();
        let (gpu, copy) = crate::screen::filled(&mut f)?;
        crate::cuda::share::wait_for(&f)?;
        spent.draws += clock.elapsed();
        spent.gpu += gpu;
        spent.copy += copy;
        let clock = Instant::now();
        crate::copies::to_encoder(&f, self.kind);
        self.encoder
            .send_frame(&f)
            .ctx(|| format!("encoding frame {j} with {}", self.name))?;
        let drained = self.drain(octx);
        spent.video += clock.elapsed();
        drained
    }

    fn drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut p = ffmpeg::Packet::empty();
        while self.encoder.receive_packet(&mut p).is_ok() {
            if let Some(files) = &mut self.files {
                files.write(&p)?;
                continue;
            }
            if let Some((id, waiting)) = self.hdr10plus.as_mut()
                && let Some(t35) = p.pts().and_then(|pts| waiting.remove(&pts))
            {
                crate::hdr::insert_hdr10plus(&mut p, *id, &t35)?;
            }
            crate::hdr::insert_static(&mut p, self.codec, &self.metadata)?;
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
    stream: usize,
    stream_tb: ffmpeg::Rational,
    frame_size: usize,
    fifo: Vec<f32>,
    sent: i64,
}

impl AudioOut {
    fn frame(&self, samples: &[f32]) -> frame::Audio {
        use format::Sample;
        use format::sample::Type::{Packed, Planar};
        let ch = usize::from(self.spec.channels);
        let n = samples.len() / ch;
        let layout = audio::channel_layout(self.spec.channels);
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

enum Out {
    Picture {
        j: i64,
        picture: frame::Video,
        t35: Option<Vec<u8>>,
    },
    Sound(i64, Vec<f32>),
}

struct Writer {
    octx: format::context::Output,
    video: Option<VideoOut>,
    audio: Option<AudioOut>,
    subtitles: Vec<SubtitleOut>,
}

#[derive(Default)]
struct Spent {
    waiting: Duration,
    draws: Duration,
    gpu: Duration,
    copy: Duration,
    video: Duration,
    sound: Duration,
    end: Duration,
}

impl Writer {
    fn subtitles_until(&mut self, j: i64) -> Result<()> {
        for s in &mut self.subtitles {
            s.write_until(&mut self.octx, j)?;
        }
        Ok(())
    }

    fn run(mut self, mut outs: Inbox<Out>) -> Result<Spent> {
        let mut spent = Spent::default();
        loop {
            let clock = Instant::now();
            let out = outs.recv()?;
            spent.waiting += clock.elapsed();
            let Some(out) = out else {
                break;
            };
            match out {
                Out::Picture { j, picture, t35 } => {
                    self.subtitles_until(j)?;
                    let v = self.video.as_mut().expect("a video encoder");
                    v.encode(&mut self.octx, j, picture, t35, &mut spent)?;
                }
                Out::Sound(j, samples) => {
                    let clock = Instant::now();
                    self.subtitles_until(j)?;
                    let a = self.audio.as_mut().expect("an audio encoder");
                    a.fifo.extend_from_slice(&samples);
                    a.encode(&mut self.octx, false)?;
                    spent.sound += clock.elapsed();
                }
            }
        }
        let clock = Instant::now();
        self.end()?;
        spent.end = clock.elapsed();
        Ok(spent)
    }

    fn end(&mut self) -> Result<()> {
        self.subtitles_until(i64::MAX)?;
        if let Some(a) = self.audio.as_mut() {
            a.encode(&mut self.octx, true)?;
            a.encoder
                .send_eof()
                .ctx(|| "flushing audio encoder".into())?;
            a.drain(&mut self.octx)?;
        }
        if let Some(v) = self.video.as_mut() {
            v.encoder
                .send_eof()
                .ctx(|| "flushing video encoder".into())?;
            v.drain(&mut self.octx)?;
        }
        self.octx.write_trailer().ctx(|| "writing trailer".into())?;
        match self.video.as_ref().and_then(|v| v.files.as_ref()) {
            Some(files) => files.keep(),
            None => Ok(()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn encode(
    plan: &RenderPlan,
    settings: &ExportSettings,
    part: &Path,
    output: &Path,
    numbering: Numbering,
    available: &Availability,
    decoder: Option<&crate::hw::GpuDecoder>,
    subs: &Subtitles,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report> {
    let started = Instant::now();
    let burn = || {
        subs.burn
            .as_ref()
            .map(|t| (t.cues.clone(), t.style.clone()))
            .unwrap_or_else(|| (Vec::new(), SubtitleStyle::default()))
    };
    let fps = ffmpeg::Rational(plan.fps.num() as i32, plan.fps.den() as i32);
    let times = (fps.invert(), Some(fps));
    let mut octx = format::output_as(part, settings.container.muxer())
        .ctx(|| format!("creating {}", output.display()))?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

    let timeline = (plan.width, plan.height);
    let mut tried_encoders = Vec::new();
    let (mut video, mut video_out) = match &settings.video {
        Some(v) => {
            let drawn = v.drawn_size(timeline);
            let (w, h) = v.written_size(timeline);
            let mut failed: Vec<(Option<String>, Error)> = Vec::new();
            let mut opened = None;
            let mut pass_files = None;
            for candidate in encode::video_encoders(v, available) {
                let enc = match candidate {
                    Ok(enc) => enc,
                    Err(e) => {
                        crate::progress::note(Note::Tried(tried(None, &e)));
                        failed.push((None, e));
                        continue;
                    }
                };
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
                if stats.is_none()
                    && let Some((device, frames)) = gpu_output(&enc, w, h)
                    && let Ok(o) = encode::open_video(
                        &enc,
                        w,
                        h,
                        times,
                        global_header,
                        Some(frames.as_ptr()),
                        None,
                        pass,
                    )
                {
                    opened = Some((enc, o, Some((device, frames))));
                    break;
                }
                let shared = decoder.filter(|d| encode::shares(enc.kind, enc.device.as_deref(), d));
                match encode::open_video(&enc, w, h, times, global_header, None, shared, pass) {
                    Ok(o) => {
                        opened = Some((enc, o, None));
                        break;
                    }
                    Err(e) => {
                        crate::progress::note(Note::Tried(tried(Some(&enc.name), &e)));
                        failed.push((Some(enc.name.clone()), e));
                    }
                }
            }
            let Some((enc, (encoder, vaapi), gpu)) = opened else {
                if failed.len() == 1 {
                    return Err(failed.remove(0).1);
                }
                let why: Vec<String> = failed
                    .iter()
                    .map(|(name, e)| tried(name.as_deref(), e).text())
                    .collect();
                return Err(Error::Invalid(format!(
                    "no video encoder would open: {}",
                    why.join("; ")
                )));
            };
            tried_encoders = failed
                .iter()
                .map(|(name, e)| tried(name.as_deref(), e))
                .collect();
            let light = made_hdr10plus_light(plan, &enc, decoder, cancel)?;
            let (encoder, vaapi) = match &pass_files {
                Some(files) if enc.passes.runs() == 2 => {
                    let buffer = first_run(
                        plan,
                        &enc,
                        encoder,
                        (drawn, (w, h)),
                        decoder,
                        burn(),
                        cancel,
                        started,
                        progress,
                    )?;
                    let stats = files.stats();
                    let second = encode::Pass::Second {
                        stats: &stats,
                        buffer: buffer.as_deref(),
                    };
                    encode::open_video(&enc, w, h, times, global_header, None, None, second)?
                }
                _ => (encoder, vaapi),
            };
            let handoff = video::Handoff::of(enc.kind, gpu, vaapi.as_ref())?;
            let mut ost = octx
                .add_stream(encoder::find_by_name(&enc.name))
                .ctx(|| "adding video stream".into())?;
            ost.set_parameters(&encoder);
            ost.set_time_base(fps.invert());
            ost.set_avg_frame_rate(fps);
            crate::progress::note(Note::Video(VideoNote {
                encoder: enc.name.clone(),
                device: enc.device.clone(),
                pixel: encode::pixel_name(enc.pixel),
                bits: encode::pixel_bits(enc.pixel),
                passes: enc.passes.count(),
                gpu_frames_in: handoff.is_some(),
            }));
            if let Some(r) = v.rounded(timeline) {
                crate::progress::note(Note::Rounded(r));
            }
            let burned = burn();
            let pixel = match handoff {
                Some(video::Handoff::Cuda(..)) => gpu_pixel(&enc),
                _ => enc.pixel,
            };
            let mut frames =
                VideoFrames::new(&plan.video, plan.fps, drawn.0, drawn.1, pixel, enc.hdr)
                    .encode_on(handoff)
                    .written_at((w, h))
                    .with_timeline(plan.colors)?
                    .with_output_lut(plan.output_lut.as_deref())?
                    .with_dynamic(enc.dynamic)
                    .with_light(light)
                    .with_burn(burned.0, burned.1, plan.height)
                    .writing_alpha(v.alpha.then_some(v.alpha_mode));
            if let Some(d) = decoder {
                frames = frames.decode_on(d.clone());
            }
            let id = encoder.id();
            let hdr10plus = (enc.dynamic.hdr10plus
                && matches!(id, ffmpeg::codec::Id::HEVC | ffmpeg::codec::Id::AV1))
            .then(|| (id, std::collections::HashMap::new()));
            let made = VideoIn {
                frames,
                vaapi,
                name: enc.name.clone(),
                passes: enc.passes,
                _pass_files: pass_files,
                metadata: enc.metadata,
                hdr10plus: hdr10plus.is_some(),
            };
            let encoding = VideoOut {
                encoder,
                enc_tb: fps.invert(),
                stream_tb: fps.invert(),
                name: enc.name.clone(),
                metadata: enc.metadata,
                hdr10plus,
                codec: id,
                kind: enc.kind,
                files: match settings.container.image_sequence() {
                    true => Some(FrameFiles::new(settings, output, numbering, plan.fps)?),
                    false => None,
                },
            };
            (Some(made), Some(encoding))
        }
        None => (None, None),
    };

    let mut normalized = None;
    let (mut mix, mut audio_out) = match &settings.audio {
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
            ctx.set_channel_layout(audio::channel_layout(spec.channels));
            ctx.set_format(spec.sample);
            ctx.set_time_base(ffmpeg::Rational(1, spec.rate as i32));
            if let Some(b) = spec.bitrate {
                ctx.set_bit_rate(b as usize);
            }
            if spec.name == "pcm_s24le" || spec.name == "flac" {
                unsafe { (*ctx.as_mut_ptr()).bits_per_raw_sample = 24 };
            }
            if spec.name == "opus" {
                unsafe {
                    (*ctx.as_mut_ptr()).strict_std_compliance =
                        ffmpeg::ffi::FF_COMPLIANCE_EXPERIMENTAL
                };
            }
            if global_header {
                ctx.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
            }
            let encoder = ctx
                .open_as(codec)
                .ctx(|| format!("opening {} with {} channels", spec.name, spec.channels))?;
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
            crate::isolate::check(&plan.audio)?;
            let mut mix = AudioMix::new(&plan.audio, plan.sample_rate, spec.channels, declick)
                .ducked(&plan.ducking);
            if let Some(target) = a.loudness {
                let measured = crate::analysis::mix_loudness(
                    &plan.audio,
                    &plan.ducking,
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
            crate::progress::note(Note::Audio(AudioNote {
                encoder: spec.name.clone(),
                rate: spec.rate,
                channels: spec.channels,
                bitrate: spec.bitrate,
                loudness: normalized,
            }));
            let encoding = AudioOut {
                encoder,
                spec,
                stream: index,
                stream_tb: ffmpeg::Rational(1, plan.sample_rate as i32),
                frame_size,
                fifo: Vec::new(),
                sent: 0,
            };
            (Some(mix), Some(encoding))
        }
        None => (None, None),
    };

    let mut subtitle_outs = Vec::new();
    let mut subtitle_streams = Vec::new();
    for track in &subs.streams {
        let (out, note) = SubtitleOut::add(&mut octx, track, plan.fps)?;
        subtitle_outs.push(out);
        subtitle_streams.push(note);
    }
    let burned_in = match (&subs.burn, &video) {
        (Some(t), Some(_)) => Some(BurnNote {
            track: t.id.clone(),
            cues: t.cues.len(),
        }),
        (Some(_), None) => {
            return Err(Error::Invalid(
                "an export without video has no picture to burn subtitles into".into(),
            ));
        }
        (None, _) => None,
    };

    let mut opts = ffmpeg::Dictionary::new();
    if matches!(settings.container.muxer(), "mp4" | "mov" | "ipod") {
        opts.set("movflags", "+faststart");
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
    if let Some(v) = video_out.as_mut() {
        v.stream_tb = octx.stream(0).expect("video stream").time_base();
    }
    if let Some(a) = audio_out.as_mut() {
        a.stream_tb = octx.stream(a.stream).expect("audio stream").time_base();
    }
    for s in &mut subtitle_outs {
        s.opened(&octx);
    }
    let audio_encoder = audio_out.as_ref().map(|a| a.spec.name.clone());

    let holds = worker::AHEAD * (usize::from(video.is_some()) + usize::from(mix.is_some())).max(1);
    let writer = Writer {
        octx,
        video: video_out,
        audio: audio_out,
        subtitles: subtitle_outs,
    };
    let mut writing = Worker::spawn(holds, move |outs| writer.run(outs))?;
    let mut produced = 0i64;
    let runs = video.as_ref().map_or(1, |v| v.passes.runs());
    let looped = Instant::now();
    let (mut sound, mut handing) = (Duration::ZERO, Duration::ZERO);
    let made = (|| -> Result<()> {
        for j in 0..plan.frames {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Invalid(CANCELLED.into()));
            }
            if let Some(v) = video.as_mut() {
                let (picture, t35) = v.next(j)?;
                let clock = Instant::now();
                writing.send(Out::Picture { j, picture, t35 })?;
                handing += clock.elapsed();
            }
            if let Some(mix) = mix.as_mut() {
                let until = plan.fps.frame_to_sample(j + 1, plan.sample_rate);
                let clock = Instant::now();
                let samples = mix.read((until - produced) as usize)?;
                sound += clock.elapsed();
                produced = until;
                let clock = Instant::now();
                writing.send(Out::Sound(j, samples))?;
                handing += clock.elapsed();
            }
            progress(Progress {
                frames: j + 1,
                total: plan.frames,
                elapsed: started.elapsed(),
                pass: runs,
                passes: runs,
                measuring: false,
                gpu_frames: video.as_ref().map_or(0, |v| v.frames.gpu_frames()),
                gpu_decoded: video.as_ref().map_or(0, |v| v.frames.gpu_decoded()),
                copied: crate::copies::counted(),
                stage: None,
            });
        }
        Ok(())
    })();
    let clock = Instant::now();
    let spent = match made {
        Ok(()) => writing.finish()?,
        Err(e) => return Err(writing.abandon(e)),
    };
    handing += clock.elapsed();
    let (decoding, drawing) = video.as_ref().map(|v| v.frames.spent()).unwrap_or_default();
    let parts = video
        .as_ref()
        .map(|v| v.frames.drawing())
        .unwrap_or_default();
    let total = looped.elapsed();
    let other = total.saturating_sub(decoding + drawing + sound + handing);
    let s = |d: Duration| d.as_secs_f64();
    crate::debug!(
        "export",
        "{} frames in {:.2} s. Export thread: decoding {:.2} s, drawing {:.2} s \
         (picture in {:.2} s, draw {:.2} s, waiting for an output buffer {:.2} s, \
         output planes {:.2} s), sound {:.2} s, waiting for the encoder's thread {:.2} s, \
         other {:.2} s. Encoder's thread: waiting for frames {:.2} s, waiting for the draw \
         {:.2} s (the GPU {:.2} s, the copy to the encoder {:.2} s), video {:.2} s, \
         sound {:.2} s, end {:.2} s",
        plan.frames,
        s(total),
        s(decoding),
        s(drawing),
        s(parts.picture_in),
        s(parts.draw),
        s(parts.buffer),
        s(parts.planes),
        s(sound),
        s(handing),
        s(other),
        s(spent.waiting),
        s(spent.draws),
        s(spent.gpu),
        s(spent.copy),
        s(spent.video),
        s(spent.sound),
        s(spent.end)
    );
    let rounded = settings.video.as_ref().and_then(|v| {
        v.rounded(timeline).or_else(|| {
            let encoder = video.as_ref().map(|m| m.name.as_str())?;
            rounded_by_encoder(part, v.drawn_size(timeline), encoder)
        })
    });
    Ok(Report {
        output: settings.first_file(output, numbering.first),
        frames: plan.frames,
        samples: if audio_encoder.is_some() {
            plan.samples()
        } else {
            0
        },
        gpu_frames: video.as_ref().map_or(0, |v| v.frames.gpu_frames()),
        gpu_decoded: video.as_ref().map_or(0, |v| v.frames.gpu_decoded()),
        through_memory: video
            .as_ref()
            .and_then(|v| v.frames.through_memory().map(String::from)),
        tried: tried_encoders,
        sources: video
            .as_ref()
            .map_or_else(Vec::new, |v| v.frames.sources().to_vec()),
        passes: video.as_ref().map(|v| v.passes.count()),
        video_encoder: video.map(|v| v.name),
        audio_encoder,
        seconds: started.elapsed().as_secs_f64(),
        loudness: normalized,
        color: None,
        metadata: None,
        copies: Default::default(),
        subtitle_streams,
        burned_in,
        rounded,
    })
}

fn rounded_by_encoder(part: &Path, asked: (u32, u32), encoder: &str) -> Option<Rounded> {
    if asked.0.is_multiple_of(2) && asked.1.is_multiple_of(2) {
        return None;
    }
    let v = crate::probe::probe(part, None).ok()?.video?;
    let written = (v.width, v.height);
    (written != asked).then(|| {
        let r = Rounded::by_encoder(asked, written, encoder);
        crate::progress::note(Note::Rounded(r.clone()));
        r
    })
}
