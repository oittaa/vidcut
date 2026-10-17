use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

pub mod color;
pub mod metadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    Mp4,
    Mov,
    Mkv,
    Webm,
    /// Audio-only containers.
    M4a,
    Mp3,
    Flac,
    Wav,
    Ogg,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Mkv => "mkv",
            Self::Webm => "webm",
            Self::M4a => "m4a",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Ogg => "ogg",
        }
    }

    pub fn muxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Mkv => "matroska",
            Self::Webm => "webm",
            Self::M4a => "ipod",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Ogg => "ogg",
        }
    }

    pub fn audio_only(self) -> bool {
        matches!(
            self,
            Self::M4a | Self::Mp3 | Self::Flac | Self::Wav | Self::Ogg
        )
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "mp4" => Self::Mp4,
            "mov" => Self::Mov,
            "mkv" => Self::Mkv,
            "webm" => Self::Webm,
            "m4a" => Self::M4a,
            "mp3" => Self::Mp3,
            "flac" => Self::Flac,
            "wav" => Self::Wav,
            "ogg" | "opus" => Self::Ogg,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
    Prores,
    Dnxhr,
}

impl VideoCodec {
    pub const ALL: [Self; 6] = [
        Self::H264,
        Self::Hevc,
        Self::Av1,
        Self::Vp9,
        Self::Prores,
        Self::Dnxhr,
    ];

    pub fn title(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Prores => "ProRes",
            VideoCodec::Dnxhr => "DNxHR",
        }
    }

    pub fn worst_quality(self) -> u32 {
        match self {
            VideoCodec::H264 | VideoCodec::Hevc => 51,
            _ => 63,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Aac,
    Opus,
    Mp3,
    Flac,
    PcmS16,
    PcmS24,
}

impl AudioCodec {
    pub const ALL: [Self; 6] = [
        Self::Aac,
        Self::Opus,
        Self::Mp3,
        Self::Flac,
        Self::PcmS16,
        Self::PcmS24,
    ];
}

/// The output's dynamic range (`video.hdr`, decision D44).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HdrMode {
    /// HDR when most of the shown picture is HDR and the codec can carry it
    /// (PQ, or HLG when all of it is HLG); otherwise SDR.
    #[default]
    Auto,
    /// SDR (BT.709): HDR pictures are tone-mapped.
    Sdr,
    /// HDR with the perceptual quantizer (BT.2100 PQ, HDR10).
    Pq,
    /// HDR with hybrid log-gamma (BT.2100 HLG).
    Hlg,
}

impl HdrMode {
    pub fn transfer(self) -> Option<crate::model::Hdr> {
        match self {
            Self::Pq => Some(crate::model::Hdr::Pq),
            Self::Hlg => Some(crate::model::Hdr::Hlg),
            Self::Auto | Self::Sdr => None,
        }
    }
}

/// HDR metadata an export writes (`video.hdr_metadata`, decision D46).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HdrMetadata {
    /// From the sources: the mastering display and content light levels
    /// (PQ output), HDR10+ when any picture has it (made for the frames
    /// without), and Dolby Vision when every picture has it.
    #[default]
    Auto,
    /// The mastering display and content light levels only.
    Static,
    /// Those, and HDR10+ kept from the sources' frames.
    Hdr10plus,
    /// Those, and Dolby Vision kept from the sources' frames (libx265 and
    /// libsvtav1 write it).
    DolbyVision,
    /// Those, HDR10+ and Dolby Vision.
    All,
    /// None.
    None,
}

impl HdrMetadata {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn hdr10plus(self) -> bool {
        matches!(self, Self::Hdr10plus | Self::All)
    }

    pub fn dolby_vision(self) -> bool {
        matches!(self, Self::DolbyVision | Self::All)
    }
}

/// Which encoder implementation to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EncoderChoice {
    /// The encoder of the GPU in use (the preferences' `gpu.device`):
    /// NVENC on NVIDIA's, AMF on AMD's and QSV on Intel's (Windows), VAAPI
    /// (Linux), VideoToolbox (macOS); else the CPU's.
    #[default]
    Auto,
    Cpu,
    /// NVIDIA's.
    Nvenc,
    /// VA-API (Linux): AMD's and Intel's GPUs.
    Vaapi,
    /// AMD's AMF (Windows).
    Amf,
    /// Intel's Quick Sync Video, through oneVPL.
    Qsv,
    /// Apple's VideoToolbox (macOS).
    VideoToolbox,
    /// FFmpeg's Vulkan encoders on the GPU in use (FFmpeg 8.1 and later).
    /// Only ProRes proxies use it: exports don't.
    Vulkan,
}

impl EncoderChoice {
    pub const ALL: [Self; 7] = [
        Self::Auto,
        Self::Cpu,
        Self::Nvenc,
        Self::Vaapi,
        Self::Amf,
        Self::Qsv,
        Self::VideoToolbox,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "the CPU",
            Self::Nvenc => "NVENC",
            Self::Vaapi => "VAAPI",
            Self::Amf => "AMF",
            Self::Qsv => "QSV",
            Self::VideoToolbox => "VideoToolbox",
            Self::Vulkan => "Vulkan",
        }
    }

    pub fn gpu(self) -> bool {
        !matches!(self, Self::Auto | Self::Cpu)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoSettings {
    pub codec: VideoCodec,
    #[serde(default)]
    pub encoder: EncoderChoice,
    /// Constant-quality level (CRF/CQ/QP style: lower is better), unless
    /// `bitrate` is set: up to 51 for H.264 and HEVC, 63 for AV1 and VP9
    /// (more is taken as that).
    #[serde(default)]
    pub quality: Option<u32>,
    /// Target bitrate in bits per second.
    #[serde(default)]
    pub bitrate: Option<u64>,
    /// Encoder speed preset (e.g. `slow`, `p5`, `8`); default per encoder.
    #[serde(default)]
    pub speed: Option<String>,
    /// ProRes: proxy, lt, standard, hq, 4444. DNxHR: lb, sq, hq, hqx, 444.
    #[serde(default)]
    pub profile: Option<String>,
    /// 8 or 10. Unset: 10 for HDR, and for SDR when most of the shown
    /// picture has 10 bits or more and the codec's 10 bits play widely
    /// (not H.264), else 8 (decision D44).
    #[serde(default)]
    pub bit_depth: Option<u8>,
    /// Output frame size; the picture is fit inside and padded. Default: the project's.
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Dynamic range: `auto` (HDR when most of the shown picture is HDR and
    /// the codec carries it), `sdr` (BT.709, HDR sources tone-mapped), or
    /// HDR with `pq` or `hlg` (BT.2100: BT.2020 colors, 10 bits, HEVC, AV1
    /// or VP9; SDR sources show their white at 203 nits). Decision D44.
    #[serde(default)]
    pub hdr: HdrMode,
    /// 1 or 2. Two passes reach a target bitrate more closely: x264, x265
    /// and VP9 encode the range twice, the first time to measure; NVENC
    /// looks at each frame twice in one encode (`multipass`). They need
    /// `bitrate`. Unset: 2 with a bitrate, 1 with constant quality
    /// (decision D45).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes: Option<u8>,
    /// The first of two passes (x264, x265, VP9): `fast` (the default)
    /// or `full`, which costs more time for little.
    #[serde(default, skip_serializing_if = "FirstPass::is_default")]
    pub first_pass: FirstPass,
    /// NVENC's first look at each frame when it makes two passes: at a
    /// `quarter` of the resolution (the default) or `full`.
    #[serde(default, skip_serializing_if = "Multipass::is_default")]
    pub multipass: Multipass,
    /// Frames NVENC looks ahead for rate control, 0 to 32 (0: none).
    /// Unset: 32 (decision D45).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookahead: Option<u32>,
    /// HDR metadata (decision D46): `auto` writes the mastering display and
    /// content light levels of the sources into PQ output, keeps HDR10+
    /// when any picture has it (made for the frames without), and Dolby
    /// Vision when every picture has it; `static`, `hdr10plus`,
    /// `dolby_vision` and `all` say which; `none` writes none.
    #[serde(default, skip_serializing_if = "HdrMetadata::is_default")]
    pub hdr_metadata: HdrMetadata,
    /// The mastering display PQ output says it was made on, as x265 writes
    /// it: `G(x,y)B(x,y)R(x,y)WP(x,y)L(max,min)`, chromaticity in 1/50000,
    /// luminance in 1/10000 cd/m². Unset: from the sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mastering_display: Option<String>,
    /// MaxCLL and MaxFALL in cd/m² (`1000,400`), or `measure`: measured
    /// in every picture shown. Unset: from the sources, measured where they
    /// say none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cll: Option<String>,
}

impl VideoSettings {
    pub fn new(codec: VideoCodec, encoder: EncoderChoice) -> Self {
        Self {
            codec,
            encoder,
            quality: None,
            bitrate: None,
            speed: None,
            profile: None,
            bit_depth: None,
            width: None,
            height: None,
            hdr: HdrMode::default(),
            passes: None,
            first_pass: FirstPass::default(),
            multipass: Multipass::default(),
            lookahead: None,
            hdr_metadata: HdrMetadata::default(),
            mastering_display: None,
            max_cll: None,
        }
    }
}

pub const MAX_LOOKAHEAD: u32 = 32;

pub const DEFAULT_LOOKAHEAD: u32 = 32;

/// The first of two passes (`video.first_pass`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FirstPass {
    /// Faster settings for the measuring pass (x264's fast first pass,
    /// x265 without its slow first pass, VP9 at speed 4 or faster).
    #[default]
    Fast,
    /// The second pass's settings.
    Full,
}

impl FirstPass {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// NVENC's first pass of two (`video.multipass`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Multipass {
    /// At a quarter of the resolution (NVENC's `qres`).
    #[default]
    Quarter,
    /// At full resolution (`fullres`).
    Full,
}

impl Multipass {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioSettings {
    pub codec: AudioCodec,
    /// Bits per second (lossy codecs).
    #[serde(default)]
    pub bitrate: Option<u64>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub channels: Option<u16>,
    /// Bring the mix to this integrated loudness (LUFS, EBU R128): -14 for
    /// YouTube, -23 for broadcast. The export measures first, then applies
    /// one gain, held so true peaks stay at or below -1 dBTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportSettings {
    pub container: Container,
    #[serde(default)]
    pub video: Option<VideoSettings>,
    #[serde(default)]
    pub audio: Option<AudioSettings>,
    /// 3 ms fades where audio is cut, so cuts don't click.
    #[serde(default = "yes")]
    pub declick: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Preset {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Shown in Quick Export.
    #[serde(default)]
    pub quick_export: bool,
    pub settings: ExportSettings,
}

fn video(codec: VideoCodec, quality: Option<u32>) -> VideoSettings {
    VideoSettings {
        codec,
        encoder: EncoderChoice::Auto,
        quality,
        bitrate: None,
        speed: None,
        profile: None,
        bit_depth: None,
        width: None,
        height: None,
        hdr: HdrMode::Auto,
        passes: None,
        first_pass: FirstPass::Fast,
        multipass: Multipass::Quarter,
        lookahead: None,
        hdr_metadata: Default::default(),
        mastering_display: None,
        max_cll: None,
    }
}

fn audio(codec: AudioCodec, kbps: Option<u64>) -> AudioSettings {
    AudioSettings {
        codec,
        bitrate: kbps.map(|k| k * 1000),
        sample_rate: None,
        channels: None,
        loudness: None,
    }
}

fn preset(
    name: &str,
    title: &str,
    description: &str,
    quick: bool,
    settings: ExportSettings,
) -> Preset {
    Preset {
        name: name.into(),
        title: title.into(),
        description: description.into(),
        quick_export: quick,
        settings,
    }
}

fn av(container: Container, v: VideoSettings, a: AudioSettings) -> ExportSettings {
    ExportSettings {
        container,
        video: Some(v),
        audio: Some(a),
        declick: true,
    }
}

fn audio_only(container: Container, a: AudioSettings) -> ExportSettings {
    ExportSettings {
        container,
        video: None,
        audio: Some(a),
        declick: true,
    }
}

pub fn builtin_presets() -> Vec<Preset> {
    use AudioCodec as A;
    use Container as C;
    use VideoCodec as V;
    let sized = |w, h, q| VideoSettings {
        width: Some(w),
        height: Some(h),
        ..video(V::H264, Some(q))
    };
    vec![
        preset(
            "h265-master",
            "H.265 Master",
            "QuickTime, 10-bit HEVC (Main10), AAC 320 kb/s",
            true,
            av(
                C::Mov,
                VideoSettings {
                    bit_depth: Some(10),
                    ..video(V::Hevc, Some(18))
                },
                audio(A::Aac, Some(320)),
            ),
        ),
        preset(
            "av1",
            "AV1",
            "MP4, 10-bit AV1, AAC: small files for YouTube and the web",
            true,
            av(
                C::Mp4,
                VideoSettings {
                    bit_depth: Some(10),
                    ..video(V::Av1, Some(30))
                },
                audio(A::Aac, Some(320)),
            ),
        ),
        preset(
            "prores-422hq",
            "ProRes 422 HQ",
            "QuickTime ProRes 422 HQ, 24-bit PCM: for further editing",
            true,
            av(
                C::Mov,
                VideoSettings {
                    profile: Some("hq".into()),
                    ..video(V::Prores, None)
                },
                audio(A::PcmS24, None),
            ),
        ),
        preset(
            "youtube-1080p",
            "YouTube 1080p",
            "MP4, H.264 1920×1080, AAC",
            true,
            av(C::Mp4, sized(1920, 1080, 18), audio(A::Aac, Some(320))),
        ),
        preset(
            "youtube-2160p",
            "YouTube 2160p",
            "MP4, H.264 3840×2160, AAC",
            true,
            av(C::Mp4, sized(3840, 2160, 18), audio(A::Aac, Some(320))),
        ),
        preset(
            "tiktok",
            "TikTok",
            "MP4, vertical H.264 1080×1920, AAC",
            true,
            av(
                C::Mp4,
                VideoSettings {
                    hdr: HdrMode::Sdr,
                    ..sized(1080, 1920, 20)
                },
                audio(A::Aac, Some(256)),
            ),
        ),
        preset(
            "h264-master",
            "H.264 Master",
            "QuickTime, high-quality H.264, AAC 320 kb/s",
            false,
            av(C::Mov, video(V::H264, Some(16)), audio(A::Aac, Some(320))),
        ),
        preset(
            "youtube-720p",
            "YouTube 720p",
            "MP4, H.264 1280×720, AAC",
            false,
            av(C::Mp4, sized(1280, 720, 18), audio(A::Aac, Some(320))),
        ),
        preset(
            "vimeo-1080p",
            "Vimeo 1080p",
            "MP4, H.264 1920×1080, AAC",
            false,
            av(C::Mp4, sized(1920, 1080, 18), audio(A::Aac, Some(320))),
        ),
        preset(
            "vp9-webm",
            "VP9 WebM",
            "WebM, VP9 and Opus: for the web",
            false,
            av(C::Webm, video(V::Vp9, Some(32)), audio(A::Opus, Some(160))),
        ),
        preset(
            "dnxhr-hq",
            "DNxHR HQ",
            "QuickTime DNxHR HQ, 16-bit PCM: for further editing",
            false,
            av(
                C::Mov,
                VideoSettings {
                    profile: Some("hq".into()),
                    ..video(V::Dnxhr, None)
                },
                audio(A::PcmS16, None),
            ),
        ),
        preset(
            "audio-wav",
            "Audio: WAV",
            "24-bit PCM WAV",
            false,
            audio_only(C::Wav, audio(A::PcmS24, None)),
        ),
        preset(
            "audio-flac",
            "Audio: FLAC",
            "Lossless FLAC",
            false,
            audio_only(C::Flac, audio(A::Flac, None)),
        ),
        preset(
            "audio-mp3",
            "Audio: MP3",
            "MP3 320 kb/s",
            false,
            audio_only(C::Mp3, audio(A::Mp3, Some(320))),
        ),
        preset(
            "audio-aac",
            "Audio: AAC",
            "AAC 256 kb/s in M4A",
            false,
            audio_only(C::M4a, audio(A::Aac, Some(256))),
        ),
        preset(
            "audio-opus",
            "Audio: Opus",
            "Opus 160 kb/s in Ogg",
            false,
            audio_only(C::Ogg, audio(A::Opus, Some(160))),
        ),
    ]
}

pub fn apply_overrides(settings: &mut ExportSettings, sets: &[String]) -> Result<()> {
    let mut v = serde_json::to_value(&*settings).map_err(|e| Error::internal(e.to_string()))?;
    for set in sets {
        let (path, raw) = set
            .split_once('=')
            .ok_or_else(|| Error::usage(format!("--set {set:?}: expected key=value")))?;
        let value: Value =
            serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
        let mut node = &mut v;
        let keys: Vec<&str> = path.split('.').collect();
        for key in &keys[..keys.len() - 1] {
            if node.get(*key).is_none_or(Value::is_null) {
                node[*key] = Value::Object(Default::default());
            }
            node = node.get_mut(*key).expect("just ensured");
        }
        node[*keys.last().expect("non-empty")] = value;
    }
    *settings =
        serde_json::from_value(v).map_err(|e| Error::usage(format!("invalid --set: {e}")))?;
    Ok(())
}

pub fn validate(s: &ExportSettings) -> Result<()> {
    use AudioCodec as A;
    use Container as C;
    use VideoCodec as V;
    if let Some(l) = s.audio.as_ref().and_then(|a| a.loudness)
        && !(-60.0..=-5.0).contains(&l)
    {
        return Err(Error::usage(format!(
            "audio.loudness {l} LUFS is outside -60..-5"
        )));
    }
    if s.video.is_none() && s.audio.is_none() {
        return Err(Error::usage("export needs video or audio"));
    }
    if let Some(v) = &s.video {
        let ok = match s.container {
            C::Mp4 => matches!(v.codec, V::H264 | V::Hevc | V::Av1 | V::Vp9),
            C::Mov => matches!(v.codec, V::H264 | V::Hevc | V::Prores | V::Dnxhr),
            C::Mkv => true,
            C::Webm => matches!(v.codec, V::Vp9 | V::Av1),
            _ => false,
        };
        if !ok {
            return Err(Error::usage(format!(
                "{:?} video can't go in {}",
                v.codec,
                s.container.extension()
            )));
        }
        if v.encoder.gpu() && matches!(v.codec, V::Prores | V::Dnxhr) {
            return Err(Error::usage(format!(
                "{:?} has no GPU encoder; use encoder cpu or auto",
                v.codec
            )));
        }
        let profiles: &[&str] = match v.codec {
            V::Prores => &["proxy", "lt", "standard", "hq", "4444"],
            V::Dnxhr => &["lb", "sq", "hq", "hqx", "444"],
            V::H264 => &["baseline", "main", "high"],
            V::Hevc => &["main", "main10"],
            V::Av1 | V::Vp9 => &[],
        };
        if let Some(p) = &v.profile
            && !profiles.contains(&p.as_str())
        {
            return Err(Error::usage(format!(
                "profile {p:?} for {:?}; choose from {profiles:?}",
                v.codec
            )));
        }
        if let Some(d) = v.bit_depth
            && d != 8
            && d != 10
        {
            return Err(Error::usage("bit_depth must be 8 or 10"));
        }
        let fixed = match (v.codec, v.profile.as_deref()) {
            (V::Prores, _) | (V::Dnxhr, Some("hqx" | "444")) | (V::Hevc, Some("main10")) => {
                Some(10)
            }
            (V::Dnxhr, Some("lb" | "sq")) | (V::Hevc, Some("main")) => Some(8),
            _ => None,
        };
        if let (Some(d), Some(f)) = (v.bit_depth, fixed)
            && d != f
        {
            let what = match (v.codec, &v.profile) {
                (V::Prores, _) => "ProRes".to_string(),
                (c, Some(p)) => format!("{c:?} profile {p}"),
                (c, None) => format!("{c:?}"),
            };
            return Err(Error::usage(format!(
                "{what} is {f}-bit; bit_depth {d} can't be"
            )));
        }
        if v.hdr.transfer().is_some() {
            if !color::carries_hdr(v.codec) {
                return Err(Error::usage(format!(
                    "HDR needs HEVC, AV1 or VP9, not {:?}",
                    v.codec
                )));
            }
            if v.bit_depth.or(fixed) == Some(8) {
                return Err(Error::usage("HDR needs 10 bits"));
            }
        }
        match v.passes {
            None | Some(1) => {}
            Some(2) if matches!(v.codec, V::Prores | V::Dnxhr) => {
                return Err(Error::usage(format!(
                    "{:?} has no rate control to make two passes for",
                    v.codec
                )));
            }
            Some(2) if v.bitrate.is_none() => {
                return Err(Error::usage(
                    "two passes need a target bitrate (video.bitrate); with constant quality \
                     one pass gives the same quality",
                ));
            }
            Some(2) => {}
            Some(n) => return Err(Error::usage(format!("passes {n}: 1 or 2"))),
        }
        if let Some(m) = &v.mastering_display {
            metadata::parse_mastering(m)?;
        }
        if let Some(l) = v.max_cll.as_deref().filter(|l| *l != metadata::MEASURE) {
            metadata::parse_light(l)?;
        }
        if v.hdr_metadata.dolby_vision() && !matches!(v.codec, V::Hevc | V::Av1) {
            return Err(Error::usage("Dolby Vision needs HEVC or AV1"));
        }
        if v.lookahead.is_some_and(|n| n > MAX_LOOKAHEAD) {
            return Err(Error::usage(format!(
                "lookahead is 0 to {MAX_LOOKAHEAD} frames"
            )));
        }
        if v.width.is_some() != v.height.is_some() {
            return Err(Error::usage("set both video.width and video.height"));
        }
        if v.width.is_some_and(|w| w < 16 || w % 2 != 0)
            || v.height.is_some_and(|h| h < 16 || h % 2 != 0)
        {
            return Err(Error::usage(
                "output width and height must be even and at least 16",
            ));
        }
    } else if !s.container.audio_only() && !matches!(s.container, C::Mkv | C::Mov | C::Mp4) {
        return Err(Error::usage(format!(
            "{} needs video",
            s.container.extension()
        )));
    }
    if let Some(a) = &s.audio {
        let ok = match s.container {
            C::Mp4 => matches!(a.codec, A::Aac | A::Mp3 | A::Opus | A::Flac),
            C::Mov => matches!(a.codec, A::Aac | A::PcmS16 | A::PcmS24),
            C::Mkv => true,
            C::Webm | C::Ogg => matches!(a.codec, A::Opus),
            C::M4a => matches!(a.codec, A::Aac),
            C::Mp3 => matches!(a.codec, A::Mp3),
            C::Flac => matches!(a.codec, A::Flac),
            C::Wav => matches!(a.codec, A::PcmS16 | A::PcmS24),
        };
        if !ok {
            return Err(Error::usage(format!(
                "{:?} audio can't go in {}",
                a.codec,
                s.container.extension()
            )));
        }
        if a.channels.is_some_and(|c| !(1..=8).contains(&c)) {
            return Err(Error::usage("audio channels must be 1..8"));
        }
    } else if s.container.audio_only() {
        return Err(Error::usage(format!(
            "{} is an audio format; it needs audio settings",
            s.container.extension()
        )));
    }
    Ok(())
}
