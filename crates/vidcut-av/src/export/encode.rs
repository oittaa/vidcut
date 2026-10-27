use serde::{Deserialize, Serialize};
use vidcut_core::export::color::Abilities;
use vidcut_core::export::{
    AudioCodec, AudioSettings, EncoderChoice, FirstPass, Multipass, VideoCodec, VideoSettings,
};

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, encoder, ffi, format, frame};

#[derive(Debug, Clone)]
pub struct VideoEncoder {
    pub name: String,
    pub kind: EncoderChoice,
    pub pixel: format::Pixel,
    pub device: Option<String>,
    pub options: Vec<(String, String)>,
    pub hdr: Option<vidcut_core::model::Hdr>,
    pub passes: Passes,
    pub metadata: crate::hdr::Static,
    pub dynamic: crate::hdr::Dynamic,
    pub chosen: Chosen,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Chosen {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl Chosen {
    fn speed(&mut self, key: &str, value: impl ToString) -> (String, String) {
        let o = opt(key, value);
        self.speed = Some(o.1.clone());
        o
    }

    fn profile(&mut self, key: &str, value: impl ToString) -> (String, String) {
        let o = opt(key, value);
        self.profile = Some(o.1.clone());
        o
    }
}

pub fn hdr_metadata(v: &VideoSettings) -> (crate::hdr::Static, crate::hdr::Dynamic) {
    use vidcut_core::export::{HdrMetadata, metadata};
    let Some(transfer) = v.hdr.transfer() else {
        return Default::default();
    };
    if v.hdr_metadata == HdrMetadata::None {
        return Default::default();
    }
    let pq = transfer == vidcut_core::model::Hdr::Pq;
    let stat = crate::hdr::Static {
        mastering: v
            .mastering_display
            .as_deref()
            .filter(|_| pq)
            .and_then(|m| metadata::parse_mastering(m).ok()),
        light: v
            .max_cll
            .as_deref()
            .filter(|_| pq)
            .and_then(|l| metadata::parse_light(l).ok()),
    };
    let dynamic = crate::hdr::Dynamic {
        hdr10plus: pq && v.hdr_metadata.hdr10plus(),
        dolby_vision: v.hdr_metadata.dolby_vision(),
    };
    (stat, dynamic)
}

pub fn writes_dolby_vision(v: &VideoSettings) -> bool {
    matches!(v.encoder, EncoderChoice::Auto | EncoderChoice::Cpu)
        && matches!(v.codec, VideoCodec::Hevc | VideoCodec::Av1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Passes {
    One,
    TwoRuns { fast_first: bool },
    TwoInside,
}

impl Passes {
    pub fn count(self) -> u8 {
        match self {
            Self::One => 1,
            Self::TwoRuns { .. } | Self::TwoInside => 2,
        }
    }

    pub fn runs(self) -> u8 {
        match self {
            Self::TwoRuns { .. } => 2,
            Self::One | Self::TwoInside => 1,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Pass<'a> {
    Only,
    First {
        stats: &'a std::path::Path,
    },
    Second {
        stats: &'a std::path::Path,
        buffer: Option<&'a str>,
    },
}

fn passes(v: &VideoSettings, name: &str) -> Result<Passes> {
    let two = match v.passes {
        Some(n) => n == 2,
        None => v.bitrate.is_some(),
    };
    if !two {
        return Ok(Passes::One);
    }
    let fast_first = v.first_pass == FirstPass::Fast;
    match name {
        "libx264" | "libx265" | "libvpx-vp9" => Ok(Passes::TwoRuns { fast_first }),
        n if n.ends_with("_nvenc") => Ok(Passes::TwoInside),
        _ if v.passes.is_none() => Ok(Passes::One),
        n if ["_vaapi", "_amf", "_qsv", "_videotoolbox"]
            .iter()
            .any(|s| n.ends_with(s)) =>
        {
            Err(Error::Invalid(format!(
                "{n} makes one pass; two need encoder cpu or nvenc"
            )))
        }
        "libsvtav1" => Err(Error::Invalid(
            "libsvtav1 makes one pass in this FFmpeg (its rate control looks ahead on its \
             own); for two, use encoder nvenc or another codec"
                .into(),
        )),
        n => Err(Error::Invalid(format!("{n} makes one pass"))),
    }
}

#[derive(Debug, Clone)]
pub struct AudioEncoder {
    pub name: String,
    pub sample: format::Sample,
    pub rate: u32,
    pub channels: u16,
    pub bitrate: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncoderStatus {
    pub encoder: String,
    pub kind: EncoderChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub pixel: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Availability {
    pub encoders: Vec<EncoderStatus>,
}

impl Availability {
    pub fn in_use(&self) -> Self {
        match crate::gpus::in_use() {
            Some(gpu) => self.on(&gpu),
            None => self.clone(),
        }
    }

    pub fn on(&self, gpu: &crate::gpus::Gpu) -> Self {
        Self {
            encoders: self
                .encoders
                .iter()
                .filter(|e| gpu.ran(e))
                .cloned()
                .collect(),
        }
    }

    pub(crate) fn find(&self, name: &str, pixel: format::Pixel) -> Option<&EncoderStatus> {
        let pix = pixel_name(pixel);
        self.encoders
            .iter()
            .find(|e| e.ok && e.encoder == name && e.pixel == pix)
    }
}

pub fn pixel_name(p: format::Pixel) -> String {
    p.descriptor()
        .map(|d| d.name().to_string())
        .unwrap_or_default()
}

pub fn pixel_bits(p: format::Pixel) -> u8 {
    // SAFETY: FFmpeg's static description of the format, or null.
    unsafe { ffi::av_pix_fmt_desc_get(p.into()).as_ref() }.map_or(8, |d| d.comp[0].depth as u8)
}

fn ten_bit(v: &VideoSettings) -> bool {
    v.bit_depth == Some(10) || v.profile.as_deref() == Some("main10") || v.hdr.transfer().is_some()
}

fn cpu_part(codec: VideoCodec) -> &'static crate::parts::Part {
    use crate::parts;
    match codec {
        VideoCodec::H264 => &parts::H264,
        VideoCodec::Hevc => &parts::HEVC,
        VideoCodec::Av1 => &parts::AV1,
        VideoCodec::Vp9 => &parts::VP9,
        VideoCodec::Prores => &parts::PRORES,
        VideoCodec::Dnxhr => &parts::DNXHR,
    }
}

pub(crate) fn cpu_ten_bit(name: &str) -> bool {
    use format::Pixel;
    encoder::find_by_name(name)
        .and_then(|c| c.video().ok())
        .and_then(|v| v.formats())
        .is_some_and(|mut f| f.any(|p| matches!(p, Pixel::YUV420P10LE | Pixel::YUV422P10LE)))
}

const AUTO_GPU: &[EncoderChoice] = if cfg!(windows) {
    &[EncoderChoice::Nvenc, EncoderChoice::Amf, EncoderChoice::Qsv]
} else if cfg!(target_os = "macos") {
    &[EncoderChoice::VideoToolbox]
} else {
    &[EncoderChoice::Nvenc, EncoderChoice::Vaapi]
};

fn kinds(choice: EncoderChoice) -> Vec<EncoderChoice> {
    match choice {
        EncoderChoice::Auto => AUTO_GPU
            .iter()
            .copied()
            .chain([EncoderChoice::Cpu])
            .collect(),
        k => vec![k],
    }
}

pub fn abilities(v: &VideoSettings, available: &Availability) -> Abilities {
    let kinds = kinds(v.encoder);
    let name = |kind: EncoderChoice| match kind {
        EncoderChoice::Cpu => crate::parts::first(cpu_part(v.codec)),
        k => gpu_encoder(v, k),
    };
    let writes = |kind: EncoderChoice, ten: bool| match (kind, name(kind)) {
        (_, None) => false,
        (EncoderChoice::Cpu, Some(n)) => !ten || cpu_ten_bit(n),
        (k, Some(n)) => available.find(n, gpu_pixel(v, k, ten)).is_some(),
    };
    let first = kinds.iter().copied().find(|&k| writes(k, false));
    Abilities {
        ten_bit: kinds.iter().any(|&k| writes(k, true)),
        ten_bit_first: first.is_some_and(|k| writes(k, true)),
        first: first.and_then(name),
    }
}

fn gpu_encoder(v: &VideoSettings, kind: EncoderChoice) -> Option<&'static str> {
    gpu_name(v.codec, kind).filter(|_| v.bitrate.is_some() || !bitrate_only(v.codec, kind))
}

pub fn bitrate_only(codec: VideoCodec, kind: EncoderChoice) -> bool {
    codec == VideoCodec::Vp9 && kind == EncoderChoice::Qsv
}

fn gpu_name(codec: VideoCodec, kind: EncoderChoice) -> Option<&'static str> {
    Some(match (codec, kind) {
        (VideoCodec::H264, EncoderChoice::Nvenc) => "h264_nvenc",
        (VideoCodec::Hevc, EncoderChoice::Nvenc) => "hevc_nvenc",
        (VideoCodec::Av1, EncoderChoice::Nvenc) => "av1_nvenc",
        (VideoCodec::H264, EncoderChoice::Vaapi) => "h264_vaapi",
        (VideoCodec::Hevc, EncoderChoice::Vaapi) => "hevc_vaapi",
        (VideoCodec::Av1, EncoderChoice::Vaapi) => "av1_vaapi",
        (VideoCodec::Vp9, EncoderChoice::Vaapi) => "vp9_vaapi",
        (VideoCodec::H264, EncoderChoice::Amf) => "h264_amf",
        (VideoCodec::Hevc, EncoderChoice::Amf) => "hevc_amf",
        (VideoCodec::Av1, EncoderChoice::Amf) => "av1_amf",
        (VideoCodec::H264, EncoderChoice::Qsv) => "h264_qsv",
        (VideoCodec::Hevc, EncoderChoice::Qsv) => "hevc_qsv",
        (VideoCodec::Av1, EncoderChoice::Qsv) => "av1_qsv",
        (VideoCodec::Vp9, EncoderChoice::Qsv) => "vp9_qsv",
        (VideoCodec::H264, EncoderChoice::VideoToolbox) => "h264_videotoolbox",
        (VideoCodec::Hevc, EncoderChoice::VideoToolbox) => "hevc_videotoolbox",
        (VideoCodec::Prores, EncoderChoice::VideoToolbox) => "prores_videotoolbox",
        _ => return None,
    })
}

fn gpu_pixel(v: &VideoSettings, kind: EncoderChoice, ten: bool) -> format::Pixel {
    match (kind, ten) {
        _ if v.codec == VideoCodec::Prores && v.profile.as_deref() == Some("4444") => {
            format::Pixel::P416LE
        }
        _ if v.codec == VideoCodec::Prores => format::Pixel::P210LE,
        (_, true) => format::Pixel::P010LE,
        (EncoderChoice::Nvenc, false) => format::Pixel::YUV420P,
        _ => format::Pixel::NV12,
    }
}

fn av1_qindex(q: u32) -> u32 {
    (q.min(63) * 255 + 31) / 63
}

fn icq(codec: VideoCodec, q: u32) -> u32 {
    let q = match codec {
        VideoCodec::Av1 => (q.min(63) * 51 + 31) / 63,
        _ => q,
    };
    q.clamp(1, 51)
}

fn opt(k: &str, v: impl ToString) -> (String, String) {
    (k.to_string(), v.to_string())
}

pub fn resolve_video(v: &VideoSettings, available: &Availability) -> Result<VideoEncoder> {
    let mut errors = Vec::new();
    for candidate in video_encoders(v, available) {
        match candidate {
            Ok(e) => return Ok(e),
            Err(e) => errors.push(e),
        }
    }
    Err(errors.remove(0))
}

pub fn video_encoders(v: &VideoSettings, available: &Availability) -> Vec<Result<VideoEncoder>> {
    let kinds = match v.encoder {
        EncoderChoice::Auto if v.hdr_metadata.dolby_vision() => vec![EncoderChoice::Cpu],
        k => kinds(k),
    };
    let fallback = VideoSettings {
        speed: None,
        ..v.clone()
    };
    let mut found: Vec<Result<VideoEncoder>> = Vec::new();
    for kind in kinds {
        let settings = if found.iter().any(Result::is_ok) {
            &fallback
        } else {
            v
        };
        match video_encoder(settings, kind, v.encoder == kind, available) {
            Ok(Some(e)) => found.push(Ok(e)),
            Ok(None) => {}
            Err(e) => found.push(Err(e)),
        }
    }
    if found.is_empty() {
        found.push(Err(Error::Invalid(format!(
            "no encoder for {}",
            v.codec.title()
        ))));
    }
    found
}

fn video_encoder(
    v: &VideoSettings,
    kind: EncoderChoice,
    asked: bool,
    available: &Availability,
) -> Result<Option<VideoEncoder>> {
    let ten = ten_bit(v);
    if kind == EncoderChoice::Cpu {
        return cpu_video(v, ten).map(Some);
    }
    if kind == EncoderChoice::Vulkan {
        return Err(Error::Invalid(
            "exports don't encode with Vulkan: it only makes ProRes proxies".into(),
        ));
    }
    let (title, codec) = (kind.title(), v.codec.title());
    let Some(name) = gpu_name(v.codec, kind) else {
        if asked {
            return Err(Error::Invalid(format!("{title} doesn't support {codec}")));
        }
        return Ok(None);
    };
    let pixel = gpu_pixel(v, kind, ten);
    let Some(status) = available.find(name, pixel) else {
        if asked {
            return Err(Error::Invalid(format!(
                "{title} isn't available on this machine ({name}, {})",
                pixel_name(pixel)
            )));
        }
        return Ok(None);
    };
    if gpu_encoder(v, kind).is_none() {
        if asked {
            return Err(Error::Invalid(format!(
                "{title} encodes {codec} only at a bitrate: set video.bitrate"
            )));
        }
        return Ok(None);
    }
    let q = v
        .quality
        .unwrap_or(match v.codec {
            VideoCodec::Av1 => 30,
            VideoCodec::Vp9 => 32,
            _ => 20,
        })
        .min(v.codec.worst_quality());
    let passes = passes(v, name)?;
    let mut chosen = Chosen::default();
    let c = &mut chosen;
    let options = match kind {
        EncoderChoice::Nvenc => nvenc_options(v, q, ten, passes, c),
        EncoderChoice::Amf => amf_options(v, q, ten, c),
        EncoderChoice::Qsv => qsv_options(v, q, ten, c),
        EncoderChoice::VideoToolbox => videotoolbox_options(v, q, ten, c),
        _ => vaapi_options(v, q),
    };
    let (metadata, dynamic) = hdr_metadata(v);
    Ok(Some(VideoEncoder {
        name: name.into(),
        kind,
        pixel,
        device: status.device.clone(),
        options,
        hdr: v.hdr.transfer(),
        passes,
        metadata,
        dynamic,
        chosen,
    }))
}

fn nvenc_options(
    v: &VideoSettings,
    q: u32,
    ten: bool,
    passes: Passes,
    c: &mut Chosen,
) -> Vec<(String, String)> {
    let mut options = vec![
        c.speed("preset", v.speed.as_deref().unwrap_or("p5")),
        opt("tune", "hq"),
        opt("rc", "vbr"),
    ];

    if passes == Passes::TwoInside {
        let first = match v.multipass {
            Multipass::Quarter => "qres",
            Multipass::Full => "fullres",
        };
        options.push(opt("multipass", first));
    }
    let lookahead = v
        .lookahead
        .unwrap_or(vidcut_core::export::DEFAULT_LOOKAHEAD);
    if lookahead > 0 {
        options.push(opt("rc-lookahead", lookahead));
    }
    match v.bitrate {
        Some(b) => {
            options.push(opt("b", b));
            options.push(opt("maxrate", b * 3 / 2));
        }
        None => {
            options.push(opt("cq", q));
            options.push(opt("b", 0));
        }
    }
    match v.codec {
        VideoCodec::Hevc => options.push(c.profile("profile", if ten { "main10" } else { "main" })),
        VideoCodec::H264 => {
            options.push(c.profile("profile", v.profile.as_deref().unwrap_or("high")))
        }
        _ => {}
    }
    options
}

fn vaapi_options(v: &VideoSettings, q: u32) -> Vec<(String, String)> {
    match (v.bitrate, v.codec) {
        (Some(b), _) => vec![opt("rc_mode", "VBR"), opt("b", b)],
        (None, VideoCodec::Av1 | VideoCodec::Vp9) => {
            vec![opt("rc_mode", "CQP"), opt("global_quality", av1_qindex(q))]
        }
        (None, _) => vec![opt("rc_mode", "CQP"), opt("qp", q)],
    }
}

fn amf_options(v: &VideoSettings, q: u32, ten: bool, c: &mut Chosen) -> Vec<(String, String)> {
    let mut options = vec![c.speed("quality", v.speed.as_deref().unwrap_or("quality"))];
    match v.bitrate {
        Some(b) => {
            options.push(opt("rc", "vbr_peak"));
            options.push(opt("b", b));
            options.push(opt("maxrate", b * 3 / 2));
        }
        None => {
            let qp = match v.codec {
                VideoCodec::Av1 => av1_qindex(q),
                _ => q,
            };
            options.push(opt("rc", "cqp"));
            options.push(opt("qp_i", qp));
            options.push(opt("qp_p", qp));
            if v.codec != VideoCodec::Hevc {
                options.push(opt("qp_b", qp));
            }
        }
    }
    match v.codec {
        VideoCodec::Hevc => options.push(c.profile("profile", if ten { "main10" } else { "main" })),
        VideoCodec::H264 => {
            let profile = match v.profile.as_deref() {
                Some("baseline") => "constrained_baseline",
                Some(p) => p,
                None => "high",
            };
            options.push(c.profile("profile", profile));
        }
        _ => {}
    }
    options
}

fn videotoolbox_options(
    v: &VideoSettings,
    q: u32,
    ten: bool,
    c: &mut Chosen,
) -> Vec<(String, String)> {
    if v.codec == VideoCodec::Prores {
        return vec![c.profile("profile", v.profile.as_deref().unwrap_or("hq"))];
    }
    let mut options = Vec::new();
    match v.bitrate {
        Some(b) => options.push(opt("b", b)),
        None => {
            let percent = 100 - q.min(51) * 100 / 60;
            options.push(opt("flags", "+qscale"));
            options.push(opt("global_quality", percent * ffi::FF_QP2LAMBDA as u32));
        }
    }
    match v.codec {
        VideoCodec::Hevc => options.push(c.profile("profile", if ten { "main10" } else { "main" })),
        VideoCodec::H264 => {
            options.push(c.profile("profile", v.profile.as_deref().unwrap_or("high")))
        }
        _ => {}
    }
    options
}

fn qsv_options(v: &VideoSettings, q: u32, ten: bool, c: &mut Chosen) -> Vec<(String, String)> {
    let mut options = vec![c.speed("preset", v.speed.as_deref().unwrap_or("medium"))];
    match v.bitrate {
        Some(b) => {
            options.push(opt("b", b));
            options.push(opt("maxrate", b * 3 / 2));
        }
        None => options.push(opt("global_quality", icq(v.codec, q))),
    }
    match v.codec {
        VideoCodec::Hevc => options.push(c.profile("profile", if ten { "main10" } else { "main" })),
        VideoCodec::H264 => {
            options.push(c.profile("profile", v.profile.as_deref().unwrap_or("high")))
        }
        _ => {}
    }
    options
}

fn cpu_video(v: &VideoSettings, ten: bool) -> Result<VideoEncoder> {
    use format::Pixel;
    let (metadata, dynamic) = hdr_metadata(v);
    let x265 = {
        let mut params = "log-level=error".to_string();
        if !metadata.is_empty() || dynamic.hdr10plus || dynamic.dolby_vision {
            params += ":repeat-headers=1";
        }
        if v.hdr.transfer() == Some(vidcut_core::model::Hdr::Pq) {
            params += ":hdr10-opt=1";
        }
        if dynamic.dolby_vision {
            let max = v.bitrate.map_or(160_000, |b| b * 3 / 2 / 1000);
            params += &format!(":vbv-maxrate={max}:vbv-bufsize={}", 2 * max);
        }
        params
    };
    let mut c = Chosen::default();
    let speed = |default: &str| v.speed.as_deref().unwrap_or(default).to_string();
    let quality = |key: &str, default: u32| match v.bitrate {
        Some(b) => opt("b", b),
        None => opt(
            key,
            v.quality.unwrap_or(default).min(v.codec.worst_quality()),
        ),
    };
    let part = cpu_part(v.codec);
    let Some(name) = crate::parts::first(part) else {
        return Err(crate::parts::part_missing(part));
    };
    if dynamic.dolby_vision && !matches!(name, "libx265" | "libsvtav1") {
        return Err(Error::Invalid(format!(
            "{name} writes no Dolby Vision (libx265 and libsvtav1 do)"
        )));
    }
    let deep = if ten {
        Pixel::YUV420P10LE
    } else {
        Pixel::YUV420P
    };
    let q = |default: u32| v.quality.unwrap_or(default).min(v.codec.worst_quality());
    let numbered = |most: u32, default: &str| {
        v.speed
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok())
            .map_or(default.to_string(), |n| n.min(most).to_string())
    };
    let (pixel, options) = match name {
        "libx264" => {
            let profile = match (ten, v.quality == Some(0) && v.bitrate.is_none()) {
                (_, true) => "high444",
                (true, false) => "high10",
                (false, false) => v.profile.as_deref().unwrap_or("high"),
            };
            (
                deep,
                vec![
                    c.speed("preset", speed("medium")),
                    quality("crf", 18),
                    c.profile("profile", profile),
                ],
            )
        }
        "libopenh264" => {
            let mut o = vec![c.profile("profile", "high")];
            match v.bitrate {
                Some(b) => o.push(opt("b", b)),
                None => o.extend([
                    opt("rc_mode", "quality"),
                    opt("qmin", q(18)),
                    opt("qmax", q(18)),
                ]),
            }
            (deep, o)
        }
        "libx265" => (deep, {
            let mut o = vec![
                c.speed("preset", speed("medium")),
                quality("crf", 20),
                opt("x265-params", x265),
            ];
            if dynamic.dolby_vision {
                o.push(opt("dolbyvision", 1));
            }
            o
        }),
        "libkvazaar" => {
            let preset = speed("medium");
            c.speed = Some(preset.clone());
            let params = match v.bitrate {
                Some(_) => format!("preset={preset}"),
                None => format!("preset={preset},qp={}", q(20)),
            };
            let mut o = vec![opt("kvazaar-params", params)];
            if let Some(b) = v.bitrate {
                o.push(opt("b", b));
            }
            (deep, o)
        }
        "libsvtav1" => (deep, {
            let mut o = vec![c.speed("preset", speed("8")), quality("crf", 30)];
            if dynamic.dolby_vision {
                o.push(opt("dolbyvision", 1));
            }
            o
        }),
        "libaom-av1" => {
            let mut o = vec![c.speed("cpu-used", numbered(8, "6")), opt("row-mt", 1)];
            match v.bitrate {
                Some(b) => o.push(opt("b", b)),
                None => o.extend([opt("crf", q(30)), opt("b", 0)]),
            }
            (deep, o)
        }
        "librav1e" => {
            let mut o = vec![c.speed("speed", numbered(10, "6"))];
            match v.bitrate {
                Some(b) => o.push(opt("b", b)),
                None => o.push(opt("qp", av1_qindex(q(30)))),
            }
            (deep, o)
        }
        "libvpx-vp9" => (deep, {
            let mut o = vec![
                quality("crf", 32),
                opt("row-mt", 1),
                c.speed("cpu-used", speed("4")),
            ];
            if v.bitrate.is_none() {
                o.push(opt("b", 0));
            }
            o
        }),
        "prores_ks" | "prores_aw" => {
            let named = v.profile.as_deref().unwrap_or("hq");
            c.profile = Some(named.to_string());
            let (profile, pixel) = match named {
                "proxy" => (0, Pixel::YUV422P10LE),
                "lt" => (1, Pixel::YUV422P10LE),
                "standard" => (2, Pixel::YUV422P10LE),
                "4444" => (4, Pixel::YUVA444P10LE),
                _ => (3, Pixel::YUV422P10LE),
            };
            (pixel, vec![opt("profile", profile), opt("vendor", "apl0")])
        }
        "dnxhd" => {
            let p = v.profile.as_deref().unwrap_or("hq");
            c.profile = Some(p.to_string());
            let pixel = match p {
                "hqx" => Pixel::YUV422P10LE,
                "444" => Pixel::YUV444P10LE,
                _ => Pixel::YUV422P,
            };
            (pixel, vec![opt("profile", format!("dnxhr_{p}"))])
        }
        n => return Err(Error::Invalid(format!("{n}: no settings for it"))),
    };
    Ok(VideoEncoder {
        name: name.into(),
        kind: EncoderChoice::Cpu,
        pixel,
        device: None,
        options,
        hdr: v.hdr.transfer(),
        passes: passes(v, name)?,
        metadata,
        dynamic,
        chosen: c,
    })
}

pub fn resolve_audio(a: &AudioSettings, rate: u32, channels: u16) -> Result<AudioEncoder> {
    use format::Sample;
    use format::sample::Type::{Packed, Planar};
    let (name, sample, default_kbps) = match a.codec {
        AudioCodec::Aac => ("aac", Sample::F32(Planar), Some(256)),
        AudioCodec::Opus => match crate::parts::first(&crate::parts::OPUS) {
            Some("opus") => ("opus", Sample::F32(Planar), Some(160)),
            Some(_) => ("libopus", Sample::F32(Packed), Some(160)),
            None => return Err(crate::parts::part_missing(&crate::parts::OPUS)),
        },
        AudioCodec::Mp3 => ("libmp3lame", Sample::F32(Planar), Some(320)),
        AudioCodec::Flac => ("flac", Sample::I32(Packed), None),
        AudioCodec::PcmS16 => ("pcm_s16le", Sample::I16(Packed), None),
        AudioCodec::PcmS24 => ("pcm_s24le", Sample::I32(Packed), None),
    };
    if !crate::parts::has_encoder(name) {
        return Err(crate::parts::missing_error(
            crate::parts::Kind::Encoder,
            name,
        ));
    }
    let mut rate = a.sample_rate.unwrap_or(rate);
    if a.codec == AudioCodec::Opus {
        rate = 48_000;
    }
    Ok(AudioEncoder {
        name: name.into(),
        sample,
        rate,
        channels: a.channels.unwrap_or(channels),
        bitrate: a.bitrate.or(default_kbps.map(|k| k * 1000)),
    })
}

pub(crate) struct Upload {
    device: *mut ffi::AVBufferRef,
    pub(crate) frames: *mut ffi::AVBufferRef,
}

impl Drop for Upload {
    fn drop(&mut self) {
        unsafe {
            ffi::av_buffer_unref(&mut self.frames);
            ffi::av_buffer_unref(&mut self.device);
        }
    }
}

impl Upload {
    pub(crate) fn vaapi(
        node: &str,
        sw: format::Pixel,
        width: u32,
        height: u32,
        shared: Option<&crate::hw::GpuDecoder>,
    ) -> Result<Self> {
        let path =
            std::ffi::CString::new(node).map_err(|_| Error::Invalid("bad device path".into()))?;
        unsafe {
            let mut device = std::ptr::null_mut();
            let ret = match shared {
                Some(d) => {
                    device = ffi::av_buffer_ref(d.as_ptr());
                    if device.is_null() {
                        ffi::AVERROR(ffi::ENOMEM)
                    } else {
                        0
                    }
                }
                None => ffi::av_hwdevice_ctx_create(
                    &mut device,
                    ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                    path.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                ),
            };
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: format!("opening VAAPI device {node}"),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let mut frames = ffi::av_hwframe_ctx_alloc(device);
            if frames.is_null() {
                ffi::av_buffer_unref(&mut device);
                return Err(Error::Invalid("cannot allocate VAAPI frames".into()));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            fc.sw_format = sw.into();
            fc.width = width as i32;
            fc.height = height as i32;
            fc.initial_pool_size = 20;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                ffi::av_buffer_unref(&mut frames);
                ffi::av_buffer_unref(&mut device);
                return Err(Error::Ffmpeg {
                    context: format!("setting up VAAPI frames on {node}"),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Self { device, frames })
        }
    }

    pub(crate) fn is_on(&self, device: &crate::hw::Device) -> bool {
        // SAFETY: `self.device` and `device` are live device references; only their `data` is read.
        unsafe { (*self.device).data == (*device.as_ptr()).data }
    }

    pub(crate) fn vulkan(gpu: &str, sw: format::Pixel, width: u32, height: u32) -> Result<Self> {
        let found = crate::gpus::list()
            .iter()
            .find(|g| g.id == gpu)
            .ok_or_else(|| Error::Invalid(format!("GPU {gpu} isn't here")))?;
        let device = crate::hw::Device::vulkan_on(found)?;
        // SAFETY: `frames` holds its own reference to the live device; ours to unref if init fails.
        unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(device.as_ptr());
            if frames.is_null() {
                return Err(Error::Invalid("cannot allocate Vulkan frames".into()));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = ffi::AVPixelFormat::AV_PIX_FMT_VULKAN;
            fc.sw_format = sw.into();
            fc.width = width as i32;
            fc.height = height as i32;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                ffi::av_buffer_unref(&mut frames);
                return Err(Error::Ffmpeg {
                    context: format!("setting up Vulkan frames on {}", found.name),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Self {
                device: ffi::av_buffer_ref(device.as_ptr()),
                frames,
            })
        }
    }

    #[cfg(windows)]
    pub(crate) fn d3d11(
        decoder: &crate::hw::GpuDecoder,
        sw: format::Pixel,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        // SAFETY: the decoder's live device, another reference to it.
        unsafe {
            let mut device = ffi::av_buffer_ref(decoder.as_ptr());
            if device.is_null() {
                return Err(Error::Invalid("no memory for the device".into()));
            }
            match crate::hw::d3d11::encoder_frames(device, sw.into(), (width, height)) {
                Ok(frames) => Ok(Self { device, frames }),
                Err(e) => {
                    ffi::av_buffer_unref(&mut device);
                    Err(e)
                }
            }
        }
    }

    pub(crate) fn videotoolbox(
        decoder: &crate::hw::GpuDecoder,
        sw: format::Pixel,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        // SAFETY: a new reference to the decoder's live device and frames on it, freed on failure.
        unsafe {
            let mut device = ffi::av_buffer_ref(decoder.as_ptr());
            if device.is_null() {
                return Err(Error::Invalid("no memory for the device".into()));
            }
            let mut frames = ffi::av_hwframe_ctx_alloc(device);
            if frames.is_null() {
                ffi::av_buffer_unref(&mut device);
                return Err(Error::Invalid("cannot allocate VideoToolbox frames".into()));
            }
            let fc = &mut *((*frames).data as *mut ffi::AVHWFramesContext);
            fc.format = ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
            fc.sw_format = sw.into();
            fc.width = width as i32;
            fc.height = height as i32;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                ffi::av_buffer_unref(&mut frames);
                ffi::av_buffer_unref(&mut device);
                return Err(Error::Ffmpeg {
                    context: "setting up VideoToolbox frames".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            Ok(Self { device, frames })
        }
    }

    pub(crate) fn upload(&self, src: &frame::Video) -> Result<frame::Video> {
        let mut hw = frame::Video::empty();
        unsafe {
            let ret = ffi::av_hwframe_get_buffer(self.frames, hw.as_mut_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "getting a GPU frame to upload to".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
            let ret = ffi::av_hwframe_transfer_data(hw.as_mut_ptr(), src.as_ptr(), 0);
            if ret < 0 {
                return Err(Error::Ffmpeg {
                    context: "uploading a frame to the GPU".into(),
                    source: ffmpeg::Error::from(ret),
                });
            }
        }
        crate::copies::up(src);
        hw.set_pts(src.pts());
        // SAFETY: both frames are valid; plain fields.
        unsafe {
            let (o, i) = (hw.as_mut_ptr(), src.as_ptr());
            (*o).color_range = (*i).color_range;
            (*o).colorspace = (*i).colorspace;
            (*o).color_primaries = (*i).color_primaries;
            (*o).color_trc = (*i).color_trc;
            (*o).chroma_location = (*i).chroma_location;
            (*o).sample_aspect_ratio = (*i).sample_aspect_ratio;
        }
        Ok(hw)
    }
}

pub(crate) fn shares(
    kind: EncoderChoice,
    device: Option<&str>,
    decoder: &crate::hw::GpuDecoder,
) -> bool {
    pairs(kind, decoder.kind())
        && (kind == EncoderChoice::VideoToolbox || device == Some(decoder.on()))
}

fn pairs(kind: EncoderChoice, decoder: crate::hw::Kind) -> bool {
    matches!(
        (kind, decoder),
        (EncoderChoice::Vaapi, crate::hw::Kind::Vaapi)
            | (EncoderChoice::Amf, crate::hw::Kind::D3d11va)
            | (EncoderChoice::VideoToolbox, crate::hw::Kind::VideoToolbox)
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn open_video(
    enc: &VideoEncoder,
    width: u32,
    height: u32,
    (time_base, rate): (ffmpeg::Rational, Option<ffmpeg::Rational>),
    global_header: bool,
    gpu_frames: Option<*mut ffi::AVBufferRef>,
    shared: Option<&crate::hw::GpuDecoder>,
    pass: Pass<'_>,
) -> Result<(encoder::Video, Option<Upload>)> {
    let codec = encoder::find_by_name(&enc.name)
        .ok_or_else(|| Error::Invalid(format!("no {} encoder", enc.name)))?;
    if enc.name == "libkvazaar" && !(width.is_multiple_of(8) && height.is_multiple_of(8)) {
        return Err(Error::Invalid(format!(
            "this FFmpeg's HEVC encoder, Kvazaar, only takes sizes in multiples of 8, not {width}×{height}"
        )));
    }
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| format!("creating {}", enc.name))?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_time_base(time_base);
    if let Some(r) = rate {
        ctx.set_frame_rate(Some(r));
    }
    crate::color::tag(unsafe { ctx.as_mut_ptr() }, enc.hdr);
    if enc.hdr.is_some() && !enc.metadata.is_empty() {
        unsafe { crate::hdr::set_on_encoder(ctx.as_mut_ptr(), &enc.metadata)? };
    }
    let mut flags = codec::Flags::empty();
    if global_header {
        flags |= codec::Flags::GLOBAL_HEADER;
    }
    match pass {
        Pass::Only => {}
        Pass::First { .. } => flags |= codec::Flags::PASS1,
        Pass::Second { .. } => flags |= codec::Flags::PASS2,
    }
    ctx.set_flags(flags);
    let vaapi = match &enc.device {
        _ if gpu_frames.is_some() => {
            ctx.set_format(format::Pixel::CUDA);
            let frames = unsafe { ffi::av_buffer_ref(gpu_frames.expect("checked")) };
            if frames.is_null() {
                return Err(Error::Invalid("no memory for the GPU frames".into()));
            }
            unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = frames };
            None
        }
        Some(node) if enc.kind == EncoderChoice::Vaapi => {
            let v = Upload::vaapi(node, enc.pixel, width, height, shared)?;
            ctx.set_format(format::Pixel::VAAPI);
            let frames = unsafe { ffi::av_buffer_ref(v.frames) };
            if frames.is_null() {
                return Err(Error::Invalid("no memory for the VAAPI frames".into()));
            }
            unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = frames };
            Some(v)
        }
        Some(gpu) if enc.kind == EncoderChoice::Vulkan => {
            let up = Upload::vulkan(gpu, enc.pixel, width, height)?;
            ctx.set_format(format::Pixel::VULKAN);
            let frames = unsafe { ffi::av_buffer_ref(up.frames) };
            if frames.is_null() {
                return Err(Error::Invalid("no memory for the Vulkan frames".into()));
            }
            unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = frames };
            Some(up)
        }
        #[cfg(windows)]
        Some(_)
            if enc.kind == EncoderChoice::Amf
                && shared.is_some_and(|d| d.kind() == crate::hw::Kind::D3d11va) =>
        {
            let up = Upload::d3d11(shared.expect("checked"), enc.pixel, width, height)?;
            ctx.set_format(format::Pixel::D3D11);
            let frames = unsafe { ffi::av_buffer_ref(up.frames) };
            if frames.is_null() {
                return Err(Error::Invalid("no memory for the encoder's frames".into()));
            }
            unsafe { (*ctx.as_mut_ptr()).hw_frames_ctx = frames };
            Some(up)
        }
        _ if enc.kind == EncoderChoice::VideoToolbox
            && shared.is_some_and(|d| d.kind() == crate::hw::Kind::VideoToolbox) =>
        {
            let up = Upload::videotoolbox(shared.expect("checked"), enc.pixel, width, height)?;
            ctx.set_format(format::Pixel::VIDEOTOOLBOX);
            let frames = unsafe { ffi::av_buffer_ref(up.frames) };
            if frames.is_null() {
                return Err(Error::Invalid("no memory for the encoder's frames".into()));
            }
            unsafe {
                (*ctx.as_mut_ptr()).sw_pix_fmt = enc.pixel.into();
                (*ctx.as_mut_ptr()).hw_frames_ctx = frames;
            }
            Some(up)
        }
        Some(on) if matches!(enc.kind, EncoderChoice::Qsv | EncoderChoice::Amf) => {
            on_device(&mut ctx, enc.kind, on)?;
            ctx.set_format(enc.pixel);
            None
        }
        _ => {
            ctx.set_format(enc.pixel);
            None
        }
    };
    let mut dict = ffmpeg::Dictionary::new();
    for (k, v) in open_options(enc, pass) {
        dict.set(&k, &v);
    }
    if enc.kind == EncoderChoice::Nvenc
        && let Some(cuda) = &enc.device
    {
        dict.set("gpu", cuda);
    }
    if let Pass::Second {
        buffer: Some(stats),
        ..
    } = pass
    {
        unsafe {
            let raw = ctx.as_mut_ptr();
            ffi::av_freep(&mut (*raw).stats_in as *mut *mut std::ffi::c_char as *mut _);
            let copy = ffi::av_malloc(stats.len() + 1) as *mut u8;
            if copy.is_null() {
                return Err(Error::Invalid(
                    "no memory for the first pass's statistics".into(),
                ));
            }
            std::ptr::copy_nonoverlapping(stats.as_ptr(), copy, stats.len());
            *copy.add(stats.len()) = 0;
            (*raw).stats_in = copy as *mut std::ffi::c_char;
        }
    }
    let opened = ctx.open_as_with(codec, dict);
    let mut opened = opened.ctx(|| format!("opening {}", enc.name))?;
    unsafe {
        let raw = opened.as_mut_ptr();
        ffi::av_freep(&mut (*raw).stats_in as *mut *mut std::ffi::c_char as *mut _);
    }
    Ok((opened, vaapi))
}

fn open_options(enc: &VideoEncoder, pass: Pass<'_>) -> Vec<(String, String)> {
    let mut options = enc.options.clone();
    if enc.kind == EncoderChoice::Cpu {
        options.push(opt("threads", "auto"));
    }
    if enc.name == "dnxhd" {
        options.push(opt("thread_type", "slice"));
    }
    let fast = matches!(enc.passes, Passes::TwoRuns { fast_first: true });
    let set = |options: &mut Vec<(String, String)>, k: &str, v: String| {
        options.retain(|(key, _)| key != k);
        options.push((k.to_string(), v));
    };
    let (stats, first) = match pass {
        Pass::Only => return options,
        Pass::First { stats } => (stats, true),
        Pass::Second { stats, .. } => (stats, false),
    };
    match enc.name.as_str() {
        "libx264" => {
            set(&mut options, "stats", stats.display().to_string());
            if first {
                set(&mut options, "fastfirstpass", u8::from(fast).to_string());
            }
        }
        "libx265" => {
            let escaped: String = stats
                .display()
                .to_string()
                .chars()
                .flat_map(|c| match c {
                    ':' | '=' | '\\' | '\'' => vec!['\\', c],
                    c => vec![c],
                })
                .collect();
            let mut params = options
                .iter()
                .find(|(k, _)| k == "x265-params")
                .map_or(String::new(), |(_, v)| format!("{v}:"));
            params += &format!("pass={}:stats={escaped}", if first { 1 } else { 2 });
            if first && fast {
                params += ":slow-firstpass=0";
            }
            set(&mut options, "x265-params", params);
        }
        "libvpx-vp9" if first && fast => {
            let speed = options
                .iter()
                .find(|(k, _)| k == "cpu-used")
                .and_then(|(_, v)| v.parse::<i32>().ok())
                .unwrap_or(0);
            set(&mut options, "cpu-used", speed.max(4).to_string());
        }
        _ => {}
    }
    options
}

pub(crate) fn on_device(
    ctx: &mut encoder::video::Video,
    kind: EncoderChoice,
    on: &str,
) -> Result<()> {
    let device = match kind {
        EncoderChoice::Qsv => crate::hw::Device::qsv(on)?,
        _ => crate::hw::Device::of_type(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, on)?,
    };
    // SAFETY: the context takes a reference of its own.
    let device = unsafe { ffi::av_buffer_ref(device.as_ptr()) };
    if device.is_null() {
        return Err(Error::Invalid("no memory for a GPU device".into()));
    }
    unsafe { (*ctx.as_mut_ptr()).hw_device_ctx = device };
    Ok(())
}

fn try_gpu(
    name: &str,
    kind: EncoderChoice,
    pixel: format::Pixel,
    device: Option<&str>,
) -> Result<()> {
    test_encode(&VideoEncoder {
        name: name.into(),
        kind,
        pixel,
        device: device.map(String::from),
        options: Vec::new(),
        hdr: None,
        passes: Passes::One,
        metadata: Default::default(),
        dynamic: Default::default(),
        chosen: Chosen::default(),
    })
}

fn test_encode(enc: &VideoEncoder) -> Result<()> {
    let (name, pixel) = (enc.name.as_str(), enc.pixel);
    let (w, h) = (640, 360);
    let (mut encoder, vaapi) = open_video(
        enc,
        w,
        h,
        (ffmpeg::Rational(1, 30), Some(ffmpeg::Rational(30, 1))),
        false,
        None,
        None,
        Pass::Only,
    )?;
    let mut sw = frame::Video::new(pixel, w, h);
    crate::export::video::fill_black(&mut sw)?;
    let mut packets = 0;
    for i in 0..3 {
        sw.set_pts(Some(i));
        let f = match &vaapi {
            Some(v) => v.upload(&sw)?,
            None => crate::export::video::frame_ref(&sw)?,
        };
        encoder.send_frame(&f).ctx(|| format!("{name}: encoding"))?;
        let mut p = ffmpeg::Packet::empty();
        while encoder.receive_packet(&mut p).is_ok() {
            packets += 1;
        }
    }
    encoder.send_eof().ctx(|| format!("{name}: flushing"))?;
    let mut p = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut p).is_ok() {
        packets += 1;
    }
    if packets == 0 {
        return Err(Error::Invalid(format!("{name} produced no output")));
    }
    Ok(())
}

pub fn render_nodes() -> Vec<(String, String)> {
    let mut nodes: Vec<(String, String)> = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        .map(|e| {
            let driver = std::fs::read_link(e.path().join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            (
                format!("/dev/dri/{}", e.file_name().to_string_lossy()),
                driver,
            )
        })
        .collect();
    nodes.sort();
    nodes
}

pub fn detect(vulkan: bool) -> Availability {
    crate::init();
    let mut encoders = Vec::new();
    let mut check =
        |name: &str, kind: EncoderChoice, pixel: format::Pixel, device: Option<&str>| {
            let result = if encoder::find_by_name(name).is_none() {
                Err(Error::Invalid(format!("{name} not built into this FFmpeg")))
            } else if kind == EncoderChoice::Vulkan && !vulkan {
                Err(Error::Invalid(
                    "not tested: vidcut stopped during the last test of the GPU encoders".into(),
                ))
            } else {
                try_gpu(name, kind, pixel, device)
            };
            encoders.push(EncoderStatus {
                encoder: name.into(),
                kind,
                device: device.map(String::from),
                pixel: pixel_name(pixel),
                ok: result.is_ok(),
                error: result.err().map(|e| e.to_string()),
            });
        };
    let nv12_p010 = &[format::Pixel::NV12, format::Pixel::P010LE][..];
    if cfg!(target_os = "macos") {
        check(
            "h264_videotoolbox",
            EncoderChoice::VideoToolbox,
            format::Pixel::NV12,
            None,
        );
        for &p in nv12_p010 {
            check("hevc_videotoolbox", EncoderChoice::VideoToolbox, p, None);
        }
        let prores = [
            format::Pixel::NV16,
            format::Pixel::P210LE,
            format::Pixel::P416LE,
        ];
        for &p in nv12_p010.iter().chain(&prores) {
            check("prores_videotoolbox", EncoderChoice::VideoToolbox, p, None);
        }
    }
    use crate::gpus::{AMD, INTEL, NVIDIA};
    for gpu in crate::gpus::list() {
        let mut kinds = Vec::new();
        if let Some(cuda) = gpu.cuda {
            kinds.push((EncoderChoice::Nvenc, cuda.to_string()));
        }
        match (gpu.adapter, gpu.pci_vendor) {
            (Some(a), AMD) => kinds.push((EncoderChoice::Amf, a.to_string())),
            (Some(a), INTEL) => kinds.push((EncoderChoice::Qsv, a.to_string())),
            _ => {}
        }
        if let Some(node) = gpu.node.as_ref().filter(|_| gpu.pci_vendor != NVIDIA) {
            kinds.push((EncoderChoice::Vaapi, node.clone()));
            if gpu.pci_vendor == INTEL {
                kinds.push((EncoderChoice::Qsv, node.clone()));
            }
        }
        for (kind, on) in kinds {
            let codecs: &[&str] = match kind {
                EncoderChoice::Nvenc => &["h264_nvenc", "hevc_nvenc", "av1_nvenc"],
                EncoderChoice::Amf => &["h264_amf", "hevc_amf", "av1_amf"],
                EncoderChoice::Qsv => &["h264_qsv", "hevc_qsv", "av1_qsv", "vp9_qsv"],
                _ => &["h264_vaapi", "hevc_vaapi", "av1_vaapi", "vp9_vaapi"],
            };
            let pixels: &[format::Pixel] = if kind == EncoderChoice::Nvenc {
                &[format::Pixel::YUV420P, format::Pixel::P010LE]
            } else {
                nv12_p010
            };
            for &codec in codecs {
                let pixels = if codec.starts_with("h264") {
                    &pixels[..1]
                } else {
                    pixels
                };
                for &p in pixels {
                    check(codec, kind, p, Some(&on));
                }
            }
        }
        if gpu.encodes_with(EncoderChoice::Vulkan) {
            check(
                "prores_ks_vulkan",
                EncoderChoice::Vulkan,
                format::Pixel::YUV422P10LE,
                Some(&gpu.id),
            );
        }
    }
    Availability { encoders }
}

#[cfg(test)]
mod tests {
    use super::{Pass, Passes, VideoEncoder, open_options, pairs, pixel_bits};
    use crate::ffmpeg::format::Pixel;
    use crate::hw::Kind;
    use vidcut_core::export::EncoderChoice;

    #[test]
    fn an_nv12_picture_has_8_bits() {
        assert_eq!(pixel_bits(Pixel::NV12), 8);
    }

    #[test]
    fn a_p010_picture_has_10_bits() {
        assert_eq!(pixel_bits(Pixel::P010LE), 10);
    }

    fn encoder(name: &str, kind: EncoderChoice) -> VideoEncoder {
        VideoEncoder {
            name: name.into(),
            kind,
            pixel: Pixel::YUV420P,
            device: None,
            options: vec![("preset".into(), "medium".into())],
            hdr: None,
            passes: Passes::One,
            metadata: Default::default(),
            dynamic: Default::default(),
            chosen: Default::default(),
        }
    }

    #[test]
    fn a_cpu_encoder_opens_with_the_automatic_thread_count() {
        let x265 = encoder("libx265", EncoderChoice::Cpu);
        assert_eq!(
            open_options(&x265, Pass::Only),
            [
                ("preset".to_string(), "medium".to_string()),
                ("threads".to_string(), "auto".to_string()),
            ]
        );
    }

    #[test]
    fn dnxhr_opens_with_slice_threads() {
        let dnxhd = encoder("dnxhd", EncoderChoice::Cpu);
        assert_eq!(
            open_options(&dnxhd, Pass::Only),
            [
                ("preset".to_string(), "medium".to_string()),
                ("threads".to_string(), "auto".to_string()),
                ("thread_type".to_string(), "slice".to_string()),
            ]
        );
    }

    #[test]
    fn a_gpu_encoder_opens_with_no_thread_count() {
        let nvenc = encoder("hevc_nvenc", EncoderChoice::Nvenc);
        assert_eq!(
            open_options(&nvenc, Pass::Only),
            [("preset".to_string(), "medium".to_string())]
        );
    }

    #[test]
    fn vaapi_pairs_with_va_api() {
        assert!(pairs(EncoderChoice::Vaapi, Kind::Vaapi));
    }

    #[test]
    fn amf_pairs_with_direct3d_11() {
        assert!(pairs(EncoderChoice::Amf, Kind::D3d11va));
    }

    #[test]
    fn videotoolbox_pairs_with_its_decoder() {
        assert!(pairs(EncoderChoice::VideoToolbox, Kind::VideoToolbox));
    }

    #[test]
    fn nvenc_doesnt_pair_with_nvdec() {
        assert!(!pairs(EncoderChoice::Nvenc, Kind::Nvdec));
    }
}
