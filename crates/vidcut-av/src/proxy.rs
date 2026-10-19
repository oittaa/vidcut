use std::collections::HashMap;
use std::path::Path;

use vidcut_core::export::EncoderChoice;
use vidcut_core::model::{ColorSpace, Hdr};
use vidcut_core::preferences::ProxyCodec;

use crate::cuda::CudaFrames;
use crate::decode::{Input, VideoReader};
use crate::error::{Context, Error, Result};
use crate::export::encode::{self, Availability, Pass, Passes, Upload, VideoEncoder};
use crate::export::make::{Fitting, Maker, Output};
use crate::export::worker::{self, Inbox, Worker};
use crate::ffmpeg::{self, encoder, ffi, format, frame};
use crate::route::{Place, Went};

const KEYFRAMES: u32 = 60;

const ORIGIN: &std::ffi::CStr = c"VIDCUT_ORIGIN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyEncoder {
    pub name: String,
    pub codec: &'static str,
    pub kind: EncoderChoice,
    pub device: Option<String>,
    pub ten_bit: bool,
    pub chroma_422: bool,
}

impl ProxyEncoder {
    fn software(name: &str, codec: &'static str) -> Self {
        Self {
            name: name.into(),
            codec,
            kind: EncoderChoice::Cpu,
            device: None,
            ten_bit: match name {
                "libsvtav1" | "prores_ks" => true,
                "libx265" => encode::cpu_ten_bit(name),
                _ => false,
            },
            chroma_422: matches!(name, "prores_ks" | "dnxhd"),
        }
    }

    pub fn label(&self) -> String {
        let codec = match self.codec {
            "av1" => "AV1",
            "hevc" => "HEVC",
            "prores" if !self.chroma_422 => "ProRes Proxy from 4:2:0",
            "prores" => "ProRes Proxy",
            "dnxhr" => "DNxHR LB",
            _ => "H.264",
        };
        let what = match (self.kind, self.name.as_str()) {
            (EncoderChoice::Nvenc, _) => "NVIDIA hardware (NVENC)".to_string(),
            (EncoderChoice::Amf, _) => "AMD hardware (AMF)".to_string(),
            (EncoderChoice::Qsv, _) => "Intel hardware (QSV)".to_string(),
            (EncoderChoice::Vaapi, _) => format!(
                "VAAPI hardware ({})",
                self.device.as_deref().unwrap_or("GPU")
            ),
            (EncoderChoice::VideoToolbox, _) => "Apple hardware (VideoToolbox)".to_string(),
            (EncoderChoice::Vulkan, _) => "GPU (Vulkan)".to_string(),
            (_, "libsvtav1") => "software (SVT-AV1)".to_string(),
            (_, "libx265") => "software (x265)".to_string(),
            (_, "libx264") => "software (x264)".to_string(),
            (_, "libopenh264") => "software (OpenH264)".to_string(),
            (_, "prores_ks" | "dnxhd") => "software (FFmpeg's)".to_string(),
            (_, other) => format!("software ({other})"),
        };
        format!("{codec}, {what}")
    }

    fn pixel(&self, source: format::Pixel) -> format::Pixel {
        use EncoderChoice::{Amf, Nvenc, Qsv, Vaapi, VideoToolbox};
        use format::Pixel::{
            NV12, NV16, P010LE, P210LE, YUV420P, YUV420P10LE, YUV422P, YUV422P10LE,
        };
        let ten = deep(source) && self.ten_bit && !matches!(self.codec, "h264" | "dnxhr");
        let keeps_422 = self.chroma_422 && at_least_422(source);
        match (self.kind, self.codec) {
            (VideoToolbox, "prores") if keeps_422 && deep(source) => P210LE,
            (VideoToolbox, "prores") if keeps_422 => NV16,
            (Nvenc | Amf | Qsv | Vaapi | VideoToolbox, _) if ten => P010LE,
            (Amf | Qsv | Vaapi | VideoToolbox, _) => NV12,
            (_, "prores") => YUV422P10LE,
            (_, "dnxhr") => YUV422P,
            _ if ten => YUV420P10LE,
            _ => YUV420P,
        }
    }

    fn options(&self, ten: bool) -> Vec<(&'static str, String)> {
        let keys = KEYFRAMES.to_string();
        let mut options = match self.name.as_str() {
            "libx264" => vec![
                ("preset", "veryfast".into()),
                ("crf", "20".into()),
                (
                    "x264-params",
                    format!("bframes=0:keyint={keys}:min-keyint={keys}:scenecut=0"),
                ),
            ],
            "libx265" => vec![
                ("preset", "veryfast".into()),
                ("crf", "20".into()),
                (
                    "x265-params",
                    format!(
                        "log-level=error:keyint={keys}:min-keyint={keys}:scenecut=0:bframes=0:\
                         open-gop=0"
                    ),
                ),
            ],
            "libsvtav1" => vec![
                ("preset", "8".into()),
                ("crf", "32".into()),
                ("svtav1-params", "scd=0:pred-struct=1".into()),
            ],
            "av1_nvenc" | "hevc_nvenc" | "h264_nvenc" => vec![
                ("preset", "p4".into()),
                ("tune", "hq".into()),
                ("rc", "vbr".into()),
                ("cq", if self.codec == "av1" { "32" } else { "20" }.into()),
                ("b", "0".into()),
            ],
            "h264_amf" | "hevc_amf" => vec![
                ("rc", "cqp".into()),
                ("qp_i", "20".into()),
                ("qp_p", "20".into()),
            ],
            "h264_vaapi" | "hevc_vaapi" => vec![("rc_mode", "CQP".into()), ("qp", "20".into())],
            "h264_qsv" | "hevc_qsv" => vec![("global_quality", "20".into())],
            "h264_videotoolbox" | "hevc_videotoolbox" => vec![
                ("flags", "+qscale".into()),
                ("global_quality", (67 * ffi::FF_QP2LAMBDA).to_string()),
            ],
            "libopenh264" => vec![
                ("profile", "high".into()),
                ("rc_mode", "quality".into()),
                ("qmin", "22".into()),
                ("qmax", "22".into()),
            ],
            "prores_ks" | "prores_ks_vulkan" => {
                vec![("profile", "0".into()), ("vendor", "apl0".into())]
            }
            "prores_videotoolbox" => vec![("profile", "proxy".into()), ("allow_sw", "0".into())],
            "dnxhd" => vec![("profile", "dnxhr_lb".into())],
            "av1_vaapi" => vec![("rc_mode", "CQP".into()), ("global_quality", "128".into())],
            _ => Vec::new(),
        };
        let engine = !matches!(self.kind, EncoderChoice::Cpu | EncoderChoice::Vaapi);
        if self.codec == "hevc" && engine {
            options.push(("profile", if ten { "main10" } else { "main" }.into()));
        }
        options
    }

    fn video_encoder(&self, first: Option<&frame::Video>) -> VideoEncoder {
        let pixel = self.pixel(first.map_or(format::Pixel::YUV420P, crate::hw::sw_format));
        let hdr = first.and_then(crate::color::hdr_of).filter(|_| deep(pixel));
        let metadata = match (hdr, first) {
            (Some(Hdr::Pq), Some(f)) => {
                let m = crate::hdr::of_frame(f);
                crate::hdr::Static {
                    mastering: m.mastering,
                    light: m.light,
                }
            }
            _ => Default::default(),
        };
        let mut options: Vec<(String, String)> = self
            .options(deep(pixel))
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        options.push(("g".into(), KEYFRAMES.to_string()));
        options.push(("bf".into(), "0".into()));
        match options.iter_mut().find(|(k, _)| k == "flags") {
            Some((_, flags)) => *flags = format!("+cgop{flags}"),
            None => options.push(("flags".into(), "+cgop".into())),
        }
        VideoEncoder {
            name: self.name.clone(),
            kind: self.kind,
            pixel,
            device: self.device.clone(),
            options,
            hdr,
            passes: Passes::One,
            metadata,
            dynamic: Default::default(),
        }
    }
}

pub const AUTOMATIC: [ProxyCodec; 5] = [
    ProxyCodec::Dnxhr,
    ProxyCodec::Prores,
    ProxyCodec::Av1,
    ProxyCodec::Hevc,
    ProxyCodec::H264,
];

pub fn encoders(
    choice: ProxyCodec,
    available: &Availability,
    plays: &[(&str, crate::hw::Kind)],
) -> Vec<ProxyEncoder> {
    if choice != ProxyCodec::Auto {
        return own(choice, available);
    }
    let mut all: Vec<ProxyEncoder> = Vec::new();
    for codec in AUTOMATIC {
        for e in own(codec, available) {
            if !all.iter().any(|a| a.name == e.name) {
                all.push(e);
            }
        }
    }
    all.sort_by_key(|e| round(e, plays));
    all
}

fn round(e: &ProxyEncoder, plays: &[(&str, crate::hw::Kind)]) -> u8 {
    let player = plays.iter().find(|(codec, _)| *codec == e.codec);
    match (e.kind, player.map(|&(_, kind)| kind)) {
        (EncoderChoice::Vulkan, Some(crate::hw::Kind::Vulkan)) => 0,
        (EncoderChoice::Cpu | EncoderChoice::Vulkan, Some(_)) => 1,
        (EncoderChoice::Cpu, None) => 3,
        (_, Some(_)) => 0,
        (_, None) => 2,
    }
}

fn own(codec: ProxyCodec, available: &Availability) -> Vec<ProxyEncoder> {
    use EncoderChoice::{Amf, Nvenc, Qsv, Vaapi, VideoToolbox, Vulkan};
    use format::Pixel::{NV12, NV16, P010LE, P210LE, YUV420P, YUV422P10LE};
    let hardware: &[(&str, &'static str, EncoderChoice, format::Pixel)] = match codec {
        ProxyCodec::Av1 => &[
            ("av1_nvenc", "av1", Nvenc, YUV420P),
            ("av1_vaapi", "av1", Vaapi, NV12),
        ],
        ProxyCodec::Hevc => &[
            ("hevc_nvenc", "hevc", Nvenc, YUV420P),
            ("hevc_amf", "hevc", Amf, NV12),
            ("hevc_vaapi", "hevc", Vaapi, NV12),
            ("hevc_qsv", "hevc", Qsv, NV12),
            ("hevc_videotoolbox", "hevc", VideoToolbox, NV12),
        ],
        ProxyCodec::H264 => &[
            ("h264_nvenc", "h264", Nvenc, YUV420P),
            ("h264_amf", "h264", Amf, NV12),
            ("h264_vaapi", "h264", Vaapi, NV12),
            ("h264_qsv", "h264", Qsv, NV12),
            ("h264_videotoolbox", "h264", VideoToolbox, NV12),
        ],
        ProxyCodec::Prores => &[
            ("prores_videotoolbox", "prores", VideoToolbox, NV12),
            ("prores_ks_vulkan", "prores", Vulkan, YUV422P10LE),
        ],
        ProxyCodec::Dnxhr | ProxyCodec::Auto => &[],
    };
    let mut out = Vec::new();
    for &(name, codec, kind, pixel) in hardware {
        if let Some(status) = available.find(name, pixel) {
            let works = |p| available.find(name, p).is_some();
            out.push(ProxyEncoder {
                name: name.into(),
                codec,
                kind,
                device: status.device.clone(),
                ten_bit: pixel == YUV422P10LE || works(P010LE),
                chroma_422: pixel == YUV422P10LE
                    || (codec == "prores" && works(NV16) && works(P210LE)),
            });
        }
    }
    let software: &[(&str, &'static str)] = match codec {
        ProxyCodec::Av1 => &[
            ("libsvtav1", "av1"),
            ("libx264", "h264"),
            ("libopenh264", "h264"),
            ("prores_ks", "prores"),
        ],
        ProxyCodec::Hevc => &[
            ("libx265", "hevc"),
            ("libx264", "h264"),
            ("libopenh264", "h264"),
            ("prores_ks", "prores"),
        ],
        ProxyCodec::H264 => &[
            ("libx264", "h264"),
            ("libopenh264", "h264"),
            ("prores_ks", "prores"),
        ],
        ProxyCodec::Prores => &[("prores_ks", "prores")],
        ProxyCodec::Dnxhr => &[("dnxhd", "dnxhr")],
        ProxyCodec::Auto => &[],
    };
    if let Some((name, codec)) = software.iter().find(|(n, _)| crate::parts::has_encoder(n)) {
        out.push(ProxyEncoder::software(name, codec));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenProxy {
    pub path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub encoder: ProxyEncoder,
    pub went: Option<Went>,
    pub decoder: Option<crate::hw::Kind>,
    pub copies: vidcut_core::editor::Copied,
}

#[allow(clippy::too_many_arguments)]
pub fn write(
    input: &Input,
    width: u32,
    height: u32,
    dest: &Path,
    total: Option<u64>,
    encoders: &[ProxyEncoder],
    decoder: Option<&crate::hw::GpuDecoder>,
    origin: &str,
    color_space: ColorSpace,
) -> Result<WrittenProxy> {
    crate::init();
    let copies = std::sync::Arc::new(crate::copies::Copies::default());
    let _counting = crate::copies::Guard::set(copies.clone());
    if width < 2 || height < 2 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(Error::Invalid(format!(
            "proxy size {width}x{height} must be even and at least 2"
        )));
    }
    if encoders.is_empty() {
        return Err(Error::Invalid("no encoder to make the proxy with".into()));
    }
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Invalid(format!("creating {}: {e}", parent.display())))?;
    }
    let tmp = dest.with_extension("part.mkv");
    let shown = Shown {
        dest,
        total: total.map_or(0, |t| t as i64),
        started: std::time::Instant::now(),
    };
    let done = |frames, enc: &ProxyEncoder, went, decoder| -> Result<WrittenProxy> {
        std::fs::rename(&tmp, dest)
            .map_err(|e| Error::Invalid(format!("replacing {}: {e}", dest.display())))?;
        Ok(WrittenProxy {
            path: dest.to_path_buf(),
            width,
            height,
            frames,
            encoder: enc.clone(),
            went,
            decoder,
            copies: copies.counted(),
        })
    };
    let mut errors = Vec::new();
    for enc in encoders {
        let mut reader = match decoder {
            Some(d) => VideoReader::open_gpu_or_cpu(input, d)?,
            None => VideoReader::open_input(input)?,
        };
        reader.set_color_space(color_space);
        if errors.is_empty() {
            crate::progress::note(crate::export::Note::Source(crate::export::SourceNote {
                path: input.path().to_path_buf(),
                decoding: (**reader.decoding()).clone(),
            }));
        }
        match encode_into(
            &mut reader,
            enc,
            (width, height),
            (&tmp, origin),
            &shown,
            decoder,
        ) {
            Ok((frames, went)) => return done(frames, enc, went, reader.gpu()),
            Err(Failed::Encoding(e)) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            Err(Failed::Opening(e)) => {
                let _ = std::fs::remove_file(&tmp);
                crate::progress::note(crate::export::Note::Tried(crate::export::Tried {
                    encoder: Some(enc.name.clone()),
                    error: e.to_string(),
                }));
                errors.push(e);
            }
        }
    }
    Err(errors.remove(0))
}

pub fn origin(path: &Path) -> Option<String> {
    crate::init();
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: a NUL-terminated path; the tag's value is copied before the context closes.
    unsafe {
        let mut ctx = std::ptr::null_mut();
        let opened = ffi::avformat_open_input(
            &mut ctx,
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        if opened < 0 {
            return None;
        }
        let tag = ffi::av_dict_get((*ctx).metadata, ORIGIN.as_ptr(), std::ptr::null(), 0);
        let origin = (!tag.is_null()).then(|| {
            std::ffi::CStr::from_ptr((*tag).value)
                .to_string_lossy()
                .into_owned()
        });
        ffi::avformat_close_input(&mut ctx);
        origin
    }
}

fn encode_time_base(src: ffmpeg::Rational) -> ffmpeg::Rational {
    let (n, d) = (src.numerator(), src.denominator());
    if n > 0 && (1..=1_000_000).contains(&d) {
        src
    } else {
        ffmpeg::Rational(1, 1_000_000)
    }
}

fn scale_pts(pts: i64, from: ffmpeg::Rational, to: ffmpeg::Rational) -> i64 {
    if from == to {
        return pts;
    }
    let n = i128::from(pts) * i128::from(from.numerator()) * i128::from(to.denominator());
    let d = i128::from(from.denominator().max(1)) * i128::from(to.numerator().max(1));
    (n / d) as i64
}

struct Shown<'a> {
    dest: &'a Path,
    total: i64,
    started: std::time::Instant,
}

impl Shown<'_> {
    fn frame(&self, frames: u64) {
        crate::progress::report(
            self.dest,
            crate::export::Progress::single(
                frames as i64,
                self.total.max(frames as i64),
                self.started.elapsed(),
            ),
        );
    }
}

enum Failed {
    Opening(Error),
    Encoding(Error),
}

enum MadeOn<'a> {
    Cuda(&'a crate::cuda::CudaDevice),
    Vulkan(&'a crate::hw::Device),
}

struct Writer<'a> {
    writing: Worker<Timed, u64>,
    upload: Option<Upload>,
    src_tb: ffmpeg::Rational,
    enc_tb: ffmpeg::Rational,
    pending: Option<(frame::Video, i64)>,
    last_dur: i64,
    frames: u64,
    shown: &'a Shown<'a>,
    hdr: Option<(ffmpeg::codec::Id, crate::hdr::Static)>,
}

struct Timed {
    picture: frame::Video,
    pts: i64,
    dur: i64,
}

struct Encoding {
    octx: format::context::Output,
    encoder: encoder::Video,
    kind: EncoderChoice,
    enc_tb: ffmpeg::Rational,
    durations: HashMap<i64, i64>,
    hdr: Option<(ffmpeg::codec::Id, crate::hdr::Static)>,
}

impl Encoding {
    fn run(mut self, mut pictures: Inbox<Timed>) -> Result<u64> {
        let mut frames = 0;
        while let Some(Timed { picture, pts, dur }) = pictures.recv()? {
            self.durations.insert(pts, dur);
            crate::cuda::share::wait_for(&picture)?;
            crate::copies::to_encoder(&picture, self.kind);
            self.encoder
                .send_frame(&picture)
                .ctx(|| "encoding a proxy frame".into())?;
            self.drain()?;
            frames += 1;
        }
        self.encoder
            .send_eof()
            .ctx(|| "flushing the proxy encoder".into())?;
        self.drain()?;
        self.octx
            .write_trailer()
            .ctx(|| "finishing the proxy".into())?;
        Ok(frames)
    }

    fn drain(&mut self) -> Result<()> {
        drain(
            &mut self.encoder,
            &mut self.octx,
            self.enc_tb,
            &mut self.durations,
            self.hdr.as_ref(),
        )
    }
}

impl<'a> Writer<'a> {
    fn new(
        (tmp, origin): (&Path, &str),
        video: &VideoEncoder,
        (width, height): (u32, u32),
        (src_tb, rate): (ffmpeg::Rational, Option<ffmpeg::Rational>),
        made_on: Option<MadeOn<'_>>,
        decoder: Option<&crate::hw::GpuDecoder>,
        shown: &'a Shown<'a>,
    ) -> std::result::Result<(Self, Output), Failed> {
        let enc_tb = encode_time_base(src_tb);
        let mut octx = format::output_as(tmp, "matroska")
            .ctx(|| format!("creating {}", tmp.display()))
            .map_err(Failed::Encoding)?;
        let mut tags = ffmpeg::Dictionary::new();
        tags.set(ORIGIN.to_str().expect("an ASCII name"), origin);
        octx.set_metadata(tags);
        let global = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
        if video.name == "dnxhd" && (width < 256 || height < 120) {
            return Err(Failed::Opening(Error::Invalid(format!(
                "DNxHR proxies are at least 256×120 pixels, and this one would be \
                 {width}×{height}: make the proxy size larger or choose another codec"
            ))));
        }
        let times = (enc_tb, rate);
        let cuda = match made_on {
            Some(MadeOn::Cuda(cuda)) => Some(cuda),
            _ => None,
        };
        let on_cuda = cuda.and_then(|cuda| {
            let device = cuda.with_own_stream().ok()?;
            let frames = CudaFrames::new(&device, cuda_layout(video.pixel), width, height).ok()?;
            let gpu_frames = Some(frames.as_ptr());
            let (opened, _) = encode::open_video(
                video,
                width,
                height,
                times,
                global,
                gpu_frames,
                None,
                Pass::Only,
            )
            .ok()?;
            Some((opened, (device, frames)))
        });
        let (encoder, upload, output) = match on_cuda {
            Some((opened, gpu)) => {
                let output = Output::for_encoder(video.kind, Some(gpu), None, None);
                (opened, None, output)
            }
            None => {
                let device = video.device.as_deref();
                let shared = decoder.filter(|d| encode::shares(video.kind, device, d));
                let (opened, upload) = encode::open_video(
                    video,
                    width,
                    height,
                    times,
                    global,
                    None,
                    shared,
                    Pass::Only,
                )
                .map_err(Failed::Opening)?;
                let fit_on = shared.map(|d| d.kind().place());
                let output = match (&made_on, &upload) {
                    (Some(MadeOn::Vulkan(device)), Some(up)) if up.is_on(device) => {
                        Ok(Output::Vulkan)
                    }
                    _ => Output::for_encoder(video.kind, None, fit_on, upload.as_ref()),
                };
                (opened, upload, output)
            }
        };
        let output = output.map_err(Failed::Opening)?;
        crate::progress::note(crate::export::Note::Video(crate::export::VideoNote {
            encoder: video.name.clone(),
            device: video.device.clone(),
            pixel: encode::pixel_name(video.pixel),
            bits: encode::pixel_bits(video.pixel),
            passes: 1,
            gpu_frames_in: !matches!(output, Output::Fitted),
        }));
        let mut header = || -> Result<()> {
            let mut ost = octx
                .add_stream(encoder::find_by_name(&video.name))
                .ctx(|| "adding the proxy stream".into())?;
            ost.set_parameters(&encoder);
            ost.set_time_base(enc_tb);
            octx.write_header()
                .ctx(|| "writing the proxy header".into())?;
            Ok(())
        };
        header().map_err(Failed::Encoding)?;
        let hdr = video.hdr.map(|_| (encoder.id(), video.metadata));
        let encoding = Encoding {
            octx,
            encoder,
            kind: video.kind,
            enc_tb,
            durations: HashMap::new(),
            hdr,
        };
        let writing = Worker::spawn(worker::AHEAD, move |pictures| encoding.run(pictures))
            .map_err(Failed::Encoding)?;
        let writer = Self {
            writing,
            upload,
            src_tb,
            enc_tb,
            pending: None,
            last_dur: 1,
            frames: 0,
            shown,
            hdr,
        };
        Ok((writer, output))
    }

    fn push(&mut self, picture: frame::Video, pts: Option<i64>) -> Result<()> {
        let Some(raw) = pts else {
            return Err(Error::Invalid(
                "a source frame has no timestamp to copy onto the proxy".into(),
            ));
        };
        let mut pts = scale_pts(raw, self.src_tb, self.enc_tb);
        if let Some((_, prev)) = &self.pending
            && pts <= *prev
        {
            pts = prev.saturating_add(1);
        }
        if let Some((prev, prev_pts)) = self.pending.take() {
            let dur = (pts - prev_pts).max(1);
            self.last_dur = dur;
            self.encode(prev, prev_pts, dur)?;
        }
        self.pending = Some((picture, pts));
        Ok(())
    }

    fn encode(&mut self, mut picture: frame::Video, pts: i64, dur: i64) -> Result<()> {
        picture.set_pts(Some(pts));
        picture.set_kind(ffmpeg::picture::Type::None);
        unsafe { (*picture.as_mut_ptr()).duration = dur };
        let mut picture = match &self.upload {
            Some(up) if Place::of(&picture) == Place::Memory => up.upload(&picture)?,
            _ => picture,
        };
        picture.set_pts(Some(pts));
        if let Some((_, metadata)) = &self.hdr {
            crate::hdr::strip(&mut picture);
            if !metadata.is_empty() {
                crate::hdr::attach(&mut picture, metadata)?;
            }
        }
        self.writing.send(Timed { picture, pts, dur })?;
        self.frames += 1;
        self.shown.frame(self.frames);
        Ok(())
    }

    fn last(&mut self, source: &Path) -> Result<()> {
        if let Some((prev, prev_pts)) = self.pending.take() {
            let dur = self.last_dur;
            self.encode(prev, prev_pts, dur)?;
        }
        if self.frames == 0 {
            return Err(Error::Invalid(format!(
                "{}: no pictures to proxy",
                source.display()
            )));
        }
        Ok(())
    }

    fn finish(self) -> Result<u64> {
        self.writing.finish()
    }

    fn abandon(self, why: Error) -> Error {
        self.writing.abandon(why)
    }
}

fn encode_into(
    reader: &mut VideoReader,
    enc: &ProxyEncoder,
    (width, height): (u32, u32),
    (tmp, origin): (&Path, &str),
    shown: &Shown<'_>,
    decoder: Option<&crate::hw::GpuDecoder>,
) -> std::result::Result<(u64, Option<Went>), Failed> {
    let rotation = reader.rotation();
    let times = (reader.time_base(), reader.frame_rate());
    let size = (width, height);
    let decoded_on_gpu = reader.gpu().is_some();
    let video = enc.video_encoder(reader.peek());
    let (pixel, target) = (video.pixel, video.hdr);
    let scaled = match enc.kind {
        EncoderChoice::Nvenc => {
            let layout = cuda_layout(pixel);
            Fitting::on_gpu(reader, size, layout, target, false, Place::Cuda)
        }
        EncoderChoice::Vulkan => Fitting::on_gpu(reader, size, pixel, target, false, Place::Vulkan),
        _ => Fitting::none(),
    };
    let made_on = match (enc.kind, &scaled.scaler) {
        (EncoderChoice::Nvenc, Some(_)) => reader.decoders_cuda().map(MadeOn::Cuda),
        (EncoderChoice::Vulkan, Some(s)) => s.vulkan().map(MadeOn::Vulkan),
        _ => None,
    };
    let (mut writer, output) =
        Writer::new((tmp, origin), &video, size, times, made_on, decoder, shown)?;
    let (layout, mut fitting) = match output {
        Output::Cuda { .. } => (cuda_layout(pixel), scaled),
        Output::Vulkan => (pixel, scaled),
        _ => (pixel, Fitting::none()),
    };
    let mut maker = Maker::new(
        (width, height, layout),
        target,
        output,
        crate::decode::Reading::Stream,
    );
    let mut went = None;
    let mut encoding = || -> Result<()> {
        while let Some(src) = reader.next_frame()? {
            let (pts, frame_size) = (src.pts(), (src.width(), src.height()));
            let Some(picture) = maker.make(&mut fitting, &src, rotation, reader.path())? else {
                continue;
            };
            if went.is_none() {
                went = fitting.went.map(|w| match w {
                    Went::Cpu if decoded_on_gpu => Went::Down {
                        size: frame_size,
                        whole: true,
                    },
                    w => w,
                });
            }
            let pts = match Place::of(&picture) {
                Place::Cuda | Place::Vulkan => picture.pts(),
                _ => pts,
            };
            let picture = match maker.output() {
                Output::Cuda { frames, .. } if Place::of(&picture) == Place::Memory => {
                    frames.upload(&picture)?
                }
                _ => picture,
            };
            writer.push(picture, pts)?;
        }
        for picture in maker.rest(&mut fitting)? {
            went.get_or_insert(Went::Stayed);
            let pts = picture.pts();
            writer.push(picture, pts)?;
        }
        Ok(())
    };
    if let Err(e) = encoding().and_then(|()| writer.last(reader.path())) {
        return Err(Failed::Encoding(writer.abandon(e)));
    }
    let frames = writer.finish().map_err(Failed::Encoding)?;
    Ok((frames, went))
}

fn cuda_layout(pixel: format::Pixel) -> format::Pixel {
    match pixel {
        format::Pixel::P010LE => format::Pixel::P010LE,
        _ => format::Pixel::NV12,
    }
}

fn deep(pixel: format::Pixel) -> bool {
    // SAFETY: FFmpeg's static descriptor of the format.
    pixel
        .descriptor()
        .is_some_and(|d| unsafe { (*d.as_ptr()).comp[0].depth } > 8)
}

fn at_least_422(pixel: format::Pixel) -> bool {
    pixel.descriptor().is_some_and(|d| d.log2_chroma_h() == 0)
}

fn drain(
    enc: &mut encoder::Video,
    octx: &mut format::context::Output,
    enc_tb: ffmpeg::Rational,
    durations: &mut HashMap<i64, i64>,
    hdr: Option<&(ffmpeg::codec::Id, crate::hdr::Static)>,
) -> Result<()> {
    let stream_tb = octx.stream(0).expect("proxy stream").time_base();
    let mut packet = ffmpeg::Packet::empty();
    while enc.receive_packet(&mut packet).is_ok() {
        if let Some(d) = packet.pts().and_then(|pts| durations.remove(&pts)) {
            packet.set_duration(d);
        }
        if let Some((id, metadata)) = hdr {
            crate::hdr::insert_static(&mut packet, *id, metadata)?;
        }
        packet.set_stream(0);
        packet.rescale_ts(enc_tb, stream_tb);
        packet
            .write_interleaved(octx)
            .ctx(|| "writing a proxy packet".into())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use EncoderChoice::{Amf, Nvenc, Qsv, Vaapi, VideoToolbox, Vulkan};

    use crate::hw::Kind;

    const NVDEC: Kind = Kind::Nvdec;
    const AMDS: Kind = Kind::D3d11va;
    const APPLES: Kind = Kind::VideoToolbox;

    fn working(list: &[(&str, EncoderChoice, &str)]) -> Availability {
        let status = |&(encoder, kind, pixel): &(&str, EncoderChoice, &str)| {
            crate::export::encode::EncoderStatus {
                encoder: encoder.into(),
                kind,
                device: Some("0".into()),
                pixel: pixel.into(),
                ok: true,
                error: None,
            }
        };
        Availability {
            encoders: list.iter().map(status).collect(),
        }
    }

    fn names(choice: ProxyCodec, available: &Availability, plays: &[(&str, Kind)]) -> Vec<String> {
        crate::init();
        encoders(choice, available, plays)
            .into_iter()
            .map(|e| e.name)
            .collect()
    }

    #[test]
    fn prores_takes_apples_hardware_where_it_works() {
        let apple = working(&[("prores_videotoolbox", VideoToolbox, "nv12")]);
        let got = names(ProxyCodec::Prores, &apple, &[]);
        assert_eq!(got, ["prores_videotoolbox", "prores_ks"]);
    }

    #[test]
    fn prores_takes_vulkan_where_it_works() {
        let gpu = working(&[("prores_ks_vulkan", Vulkan, "yuv422p10le")]);
        let got = names(ProxyCodec::Prores, &gpu, &[]);
        assert_eq!(got, ["prores_ks_vulkan", "prores_ks"]);
    }

    #[test]
    fn hevc_takes_the_gpus_video_engines_in_their_order() {
        let gpus = working(&[
            ("hevc_videotoolbox", VideoToolbox, "nv12"),
            ("hevc_qsv", Qsv, "nv12"),
            ("hevc_vaapi", Vaapi, "nv12"),
            ("hevc_amf", Amf, "nv12"),
            ("hevc_nvenc", Nvenc, "yuv420p"),
        ]);
        let got = names(ProxyCodec::Hevc, &gpus, &[]);
        let want = [
            "hevc_nvenc",
            "hevc_amf",
            "hevc_vaapi",
            "hevc_qsv",
            "hevc_videotoolbox",
        ];
        assert_eq!(got[..5], want);
    }

    #[test]
    fn automatic_takes_what_the_gpu_both_makes_and_plays() {
        let nvidia = working(&[
            ("av1_nvenc", Nvenc, "yuv420p"),
            ("h264_nvenc", Nvenc, "yuv420p"),
            ("prores_ks_vulkan", Vulkan, "yuv422p10le"),
        ]);
        let got = names(
            ProxyCodec::Auto,
            &nvidia,
            &[("av1", NVDEC), ("h264", NVDEC)],
        );
        assert_eq!(got[..2], ["av1_nvenc", "h264_nvenc"]);
    }

    #[test]
    fn automatic_takes_nvidias_av1() {
        let nvidia = working(&[
            ("h264_nvenc", Nvenc, "yuv420p"),
            ("hevc_nvenc", Nvenc, "yuv420p"),
            ("hevc_nvenc", Nvenc, "p010le"),
            ("av1_nvenc", Nvenc, "yuv420p"),
            ("av1_nvenc", Nvenc, "p010le"),
        ]);
        let plays = [("h264", NVDEC), ("hevc", NVDEC), ("av1", NVDEC)];
        let got = names(ProxyCodec::Auto, &nvidia, &plays);
        assert_eq!(got[0], "av1_nvenc");
    }

    #[test]
    fn automatic_takes_apples_prores_hardware() {
        let apple = working(&[
            ("h264_videotoolbox", VideoToolbox, "nv12"),
            ("hevc_videotoolbox", VideoToolbox, "nv12"),
            ("hevc_videotoolbox", VideoToolbox, "p010le"),
            ("prores_videotoolbox", VideoToolbox, "nv12"),
            ("prores_videotoolbox", VideoToolbox, "p010le"),
        ]);
        let plays = [
            ("h264", APPLES),
            ("hevc", APPLES),
            ("av1", APPLES),
            ("prores", APPLES),
        ];
        let got = names(ProxyCodec::Auto, &apple, &plays);
        assert_eq!(got[0], "prores_videotoolbox");
    }

    #[test]
    fn automatic_takes_amds_hevc() {
        let amd = working(&[
            ("h264_amf", Amf, "nv12"),
            ("hevc_amf", Amf, "nv12"),
            ("hevc_amf", Amf, "p010le"),
        ]);
        let plays = [("h264", AMDS), ("hevc", AMDS), ("av1", AMDS)];
        let got = names(ProxyCodec::Auto, &amd, &plays);
        assert_eq!(got[0], "hevc_amf");
    }

    #[test]
    fn automatic_puts_the_gpus_video_engine_before_vulkan_where_prores_plays_another_way() {
        let apple = working(&[
            ("h264_videotoolbox", VideoToolbox, "nv12"),
            ("prores_ks_vulkan", Vulkan, "yuv422p10le"),
        ]);
        let plays = [("h264", APPLES), ("prores", APPLES)];
        let got = names(ProxyCodec::Auto, &apple, &plays);
        assert_eq!(got[0], "h264_videotoolbox");
    }

    #[test]
    fn automatic_takes_vulkans_prores_where_the_gpu_plays_prores_on_vulkan() {
        let nvidia = working(&[
            ("av1_nvenc", Nvenc, "yuv420p"),
            ("hevc_nvenc", Nvenc, "yuv420p"),
            ("h264_nvenc", Nvenc, "yuv420p"),
            ("prores_ks_vulkan", Vulkan, "yuv422p10le"),
        ]);
        let plays = [
            ("h264", NVDEC),
            ("av1", NVDEC),
            ("hevc", NVDEC),
            ("prores", Kind::Vulkan),
        ];
        let got = names(ProxyCodec::Auto, &nvidia, &plays);
        assert_eq!(got[0], "prores_ks_vulkan");
    }

    #[test]
    fn automatic_takes_what_the_gpu_plays_before_what_it_makes() {
        let gpu = working(&[("av1_nvenc", Nvenc, "yuv420p")]);
        let got = names(ProxyCodec::Auto, &gpu, &[("prores", Kind::Vulkan)]);
        assert_eq!(got[..2], ["prores_ks", "av1_nvenc"]);
    }

    #[test]
    fn automatic_takes_what_the_gpu_makes_before_software() {
        let apple = working(&[("h264_videotoolbox", VideoToolbox, "nv12")]);
        let got = names(ProxyCodec::Auto, &apple, &[]);
        assert_eq!(got[0], "h264_videotoolbox");
    }

    #[test]
    fn automatic_without_a_gpu_makes_dnxhr() {
        let got = names(ProxyCodec::Auto, &Availability::default(), &[]);
        assert_eq!(got[0], "dnxhd");
    }

    fn nvencs_av1() -> ProxyEncoder {
        ProxyEncoder {
            name: "av1_nvenc".into(),
            codec: "av1",
            kind: Nvenc,
            device: Some("0".into()),
            ten_bit: true,
            chroma_422: false,
        }
    }

    #[test]
    fn an_encoder_working_with_p010_here_takes_ten_bits() {
        crate::init();
        let nvidia = working(&[
            ("av1_nvenc", Nvenc, "yuv420p"),
            ("av1_nvenc", Nvenc, "p010le"),
        ]);
        assert!(own(ProxyCodec::Av1, &nvidia)[0].ten_bit);
    }

    #[test]
    fn nvencs_av1_takes_a_ten_bit_sources_pictures_as_p010() {
        let got = nvencs_av1().pixel(format::Pixel::YUV420P10LE);
        assert_eq!(got, format::Pixel::P010LE);
    }

    #[test]
    fn nvencs_av1_takes_an_eight_bit_sources_pictures_in_eight_bits() {
        let got = nvencs_av1().pixel(format::Pixel::YUV420P);
        assert_eq!(got, format::Pixel::YUV420P);
    }

    #[test]
    fn nvencs_h264_takes_a_ten_bit_sources_pictures_in_eight_bits() {
        crate::init();
        let nvidia = working(&[("h264_nvenc", Nvenc, "yuv420p")]);
        let h264 = own(ProxyCodec::H264, &nvidia).remove(0);
        assert_eq!(
            h264.pixel(format::Pixel::YUV420P10LE),
            format::Pixel::YUV420P
        );
    }

    #[test]
    fn h264_stays_eight_bit_where_its_encoder_takes_ten_bits() {
        let vaapi = ProxyEncoder {
            name: "h264_vaapi".into(),
            codec: "h264",
            kind: Vaapi,
            device: Some("/dev/dri/renderD128".into()),
            ten_bit: true,
            chroma_422: false,
        };
        assert_eq!(vaapi.pixel(format::Pixel::YUV420P10LE), format::Pixel::NV12);
    }

    #[test]
    fn svt_av1_takes_a_ten_bit_sources_pictures_in_ten_bits() {
        let svt = ProxyEncoder::software("libsvtav1", "av1");
        let got = svt.pixel(format::Pixel::YUV420P10LE);
        assert_eq!(got, format::Pixel::YUV420P10LE);
    }

    #[test]
    fn apples_hevc_takes_a_ten_bit_sources_pictures_as_p010() {
        let apple = ProxyEncoder {
            name: "hevc_videotoolbox".into(),
            codec: "hevc",
            kind: VideoToolbox,
            device: None,
            ten_bit: true,
            chroma_422: false,
        };
        assert_eq!(apple.pixel(format::Pixel::P010LE), format::Pixel::P010LE);
    }

    #[test]
    fn hevc_on_nvenc_is_main_10_for_ten_bit_pictures() {
        let nvenc = ProxyEncoder {
            name: "hevc_nvenc".into(),
            codec: "hevc",
            kind: Nvenc,
            device: Some("0".into()),
            ten_bit: true,
            chroma_422: false,
        };
        let source = first(format::Pixel::YUV420P10LE, None);
        let options = nvenc.video_encoder(Some(&source)).options;
        let profile: Vec<&str> = options
            .iter()
            .filter(|(k, _)| k == "profile")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(profile, ["main10"]);
    }

    fn apples_prores(chroma_422: bool) -> ProxyEncoder {
        ProxyEncoder {
            name: "prores_videotoolbox".into(),
            codec: "prores",
            kind: VideoToolbox,
            device: None,
            ten_bit: true,
            chroma_422,
        }
    }

    #[test]
    fn apples_prores_takes_a_prores_sources_frames_as_p210() {
        let got = apples_prores(true).pixel(format::Pixel::P210LE);
        assert_eq!(got, format::Pixel::P210LE);
    }

    #[test]
    fn apples_prores_takes_an_eight_bit_4_2_2_sources_pictures_as_nv16() {
        let got = apples_prores(true).pixel(format::Pixel::YUV422P);
        assert_eq!(got, format::Pixel::NV16);
    }

    #[test]
    fn apples_prores_takes_a_ten_bit_4_2_0_sources_frames_as_p010() {
        let got = apples_prores(true).pixel(format::Pixel::P010LE);
        assert_eq!(got, format::Pixel::P010LE);
    }

    #[test]
    fn apples_prores_without_4_2_2_takes_a_prores_sources_frames_as_p010() {
        let got = apples_prores(false).pixel(format::Pixel::P210LE);
        assert_eq!(got, format::Pixel::P010LE);
    }

    #[test]
    fn apples_prores_found_working_with_nv16_and_p210_takes_4_2_2() {
        crate::init();
        let apple = working(&[
            ("prores_videotoolbox", VideoToolbox, "nv12"),
            ("prores_videotoolbox", VideoToolbox, "nv16"),
            ("prores_videotoolbox", VideoToolbox, "p210le"),
        ]);
        assert!(own(ProxyCodec::Prores, &apple)[0].chroma_422);
    }

    #[test]
    fn apples_prores_found_working_with_nv12_alone_takes_no_4_2_2() {
        crate::init();
        let apple = working(&[("prores_videotoolbox", VideoToolbox, "nv12")]);
        assert!(!own(ProxyCodec::Prores, &apple)[0].chroma_422);
    }

    #[test]
    fn apples_prores_without_4_2_2_says_its_pictures_are_4_2_0() {
        let label = apples_prores(false).label();
        assert_eq!(
            label,
            "ProRes Proxy from 4:2:0, Apple hardware (VideoToolbox)"
        );
    }

    #[test]
    fn apples_prores_with_4_2_2_is_called_prores_proxy() {
        let label = apples_prores(true).label();
        assert_eq!(label, "ProRes Proxy, Apple hardware (VideoToolbox)");
    }

    #[test]
    fn nvenc_opens_with_its_settings_and_the_proxies_keyframes() {
        let enc = ProxyEncoder {
            name: "h264_nvenc".into(),
            codec: "h264",
            kind: Nvenc,
            device: Some("0".into()),
            ten_bit: false,
            chroma_422: false,
        };
        let want = [
            ("preset", "p4"),
            ("tune", "hq"),
            ("rc", "vbr"),
            ("cq", "20"),
            ("b", "0"),
            ("g", "60"),
            ("bf", "0"),
            ("flags", "+cgop"),
        ];
        let want = want.map(|(k, v)| (k.to_string(), v.to_string()));
        assert_eq!(enc.video_encoder(None).options, want);
    }

    #[test]
    fn videotoolbox_closes_its_gops_in_its_one_flags_option() {
        let enc = ProxyEncoder {
            name: "h264_videotoolbox".into(),
            codec: "h264",
            kind: VideoToolbox,
            device: None,
            ten_bit: false,
            chroma_422: false,
        };
        let options = enc.video_encoder(None).options;
        let flags: Vec<&str> = options
            .iter()
            .filter(|(k, _)| k == "flags")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(flags, ["+cgop+qscale"]);
    }

    fn first(pixel: format::Pixel, hdr: Option<Hdr>) -> frame::Video {
        use ffi::AVColorTransferCharacteristic as T;
        let mut f = frame::Video::new(pixel, 64, 36);
        let transfer = match hdr {
            Some(Hdr::Pq) => T::AVCOL_TRC_SMPTE2084,
            Some(Hdr::Hlg) => T::AVCOL_TRC_ARIB_STD_B67,
            None => T::AVCOL_TRC_UNSPECIFIED,
        };
        // SAFETY: a plain field of its own frame.
        unsafe { (*f.as_mut_ptr()).color_trc = transfer };
        f
    }

    fn hdr10() -> crate::hdr::Static {
        crate::hdr::Static {
            mastering: Some(crate::fixture::mastering(1000)),
            light: Some(vidcut_core::model::LightLevel {
                max_cll: 1000,
                max_fall: 400,
            }),
        }
    }

    #[test]
    fn a_ten_bit_proxy_of_a_pq_source_stays_pq() {
        let source = first(format::Pixel::YUV420P10LE, Some(Hdr::Pq));
        assert_eq!(nvencs_av1().video_encoder(Some(&source)).hdr, Some(Hdr::Pq));
    }

    #[test]
    fn a_ten_bit_proxy_of_an_hlg_source_stays_hlg() {
        let source = first(format::Pixel::YUV420P10LE, Some(Hdr::Hlg));
        let encoder = nvencs_av1().video_encoder(Some(&source));
        assert_eq!(encoder.hdr, Some(Hdr::Hlg));
    }

    #[test]
    fn an_eight_bit_proxy_of_a_pq_source_is_sdr() {
        let h264 = ProxyEncoder::software("libx264", "h264");
        let source = first(format::Pixel::YUV420P10LE, Some(Hdr::Pq));
        assert_eq!(h264.video_encoder(Some(&source)).hdr, None);
    }

    #[test]
    fn a_ten_bit_proxy_of_an_sdr_source_is_sdr() {
        let source = first(format::Pixel::YUV420P10LE, None);
        assert_eq!(nvencs_av1().video_encoder(Some(&source)).hdr, None);
    }

    #[test]
    fn a_pq_proxy_writes_the_sources_static_metadata() {
        let mut source = first(format::Pixel::YUV420P10LE, Some(Hdr::Pq));
        crate::hdr::attach(&mut source, &hdr10()).unwrap();
        let encoder = nvencs_av1().video_encoder(Some(&source));
        assert_eq!(encoder.metadata, hdr10());
    }

    #[test]
    fn an_hlg_proxy_writes_no_static_metadata() {
        let mut source = first(format::Pixel::YUV420P10LE, Some(Hdr::Hlg));
        crate::hdr::attach(&mut source, &hdr10()).unwrap();
        let encoder = nvencs_av1().video_encoder(Some(&source));
        assert_eq!(encoder.metadata, crate::hdr::Static::default());
    }
}
