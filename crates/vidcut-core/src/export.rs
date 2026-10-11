//! Export settings and presets (pure data; the encoder engine is in vidcut-av).
//!
//! Built-in presets follow the Quick Export set of spec §10, plus a few more
//! (AV1, VP9, DNxHR and audio-only). User presets are TOML files with the
//! same fields.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

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

    /// FFmpeg muxer name.
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

    /// The container for a file extension.
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

/// Which encoder implementation to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EncoderChoice {
    /// GPU when available (NVENC, then VAAPI), else CPU.
    #[default]
    Auto,
    Cpu,
    Nvenc,
    Vaapi,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoSettings {
    pub codec: VideoCodec,
    #[serde(default)]
    pub encoder: EncoderChoice,
    /// Constant-quality level (CRF/CQ/QP style: lower is better), unless
    /// `bitrate` is set.
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
    /// 8 or 10; default per codec.
    #[serde(default)]
    pub bit_depth: Option<u8>,
    /// Output frame size; the picture is fit inside and padded. Default: the project's.
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// HDR output with this transfer (BT.2100: PQ or HLG): BT.2020 colors,
    /// 10 bits, HEVC, AV1 or VP9. SDR sources show their white at 203 nits.
    /// Default: SDR (BT.709), HDR sources tone-mapped to it.
    #[serde(default)]
    pub hdr: Option<crate::model::Hdr>,
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
        hdr: None,
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

/// The built-in presets.
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
            av(C::Mp4, sized(1080, 1920, 20), audio(A::Aac, Some(256))),
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

/// Applies `key=value` overrides with dotted paths into the settings, e.g.
/// `video.quality=20`, `video.encoder=nvenc`, `audio.bitrate=192000`,
/// `video=null` (drop video). Values are JSON when they parse as JSON,
/// strings otherwise.
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

/// Checks codecs against the container and each other.
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
        let gpu = matches!(v.encoder, EncoderChoice::Nvenc | EncoderChoice::Vaapi);
        if gpu && matches!(v.codec, V::Prores | V::Dnxhr | V::Vp9) {
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
        if v.hdr.is_some() {
            if !matches!(v.codec, V::Hevc | V::Av1 | V::Vp9) {
                return Err(Error::usage(format!(
                    "HDR needs HEVC, AV1 or VP9, not {:?}",
                    v.codec
                )));
            }
            if v.bit_depth == Some(8) {
                return Err(Error::usage("HDR needs 10 bits"));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_presets_are_valid_and_unique() {
        let presets = builtin_presets();
        let mut names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), presets.len());
        for p in &presets {
            validate(&p.settings).unwrap_or_else(|e| panic!("{}: {e}", p.name));
        }
    }

    #[test]
    fn overrides_edit_nested_settings() {
        let mut s = builtin_presets()
            .into_iter()
            .find(|p| p.name == "youtube-1080p")
            .unwrap()
            .settings;
        apply_overrides(
            &mut s,
            &[
                "video.quality=22".into(),
                "video.encoder=nvenc".into(),
                "audio.bitrate=192000".into(),
            ],
        )
        .unwrap();
        let v = s.video.as_ref().unwrap();
        assert_eq!((v.quality, v.encoder), (Some(22), EncoderChoice::Nvenc));
        assert_eq!(s.audio.as_ref().unwrap().bitrate, Some(192_000));
        apply_overrides(&mut s, &["video=null".into(), "container=m4a".into()]).unwrap();
        assert!(s.video.is_none());
        validate(&s).unwrap();
        assert!(
            apply_overrides(&mut s, &["audio.bitrat=1".into()]).is_err(),
            "unknown keys are rejected"
        );
        assert!(apply_overrides(&mut s, &["video.quality".into()]).is_err());
    }

    #[test]
    fn validation_catches_bad_combinations() {
        let mut s = builtin_presets()
            .into_iter()
            .find(|p| p.name == "prores-422hq")
            .unwrap()
            .settings;
        s.container = Container::Mp4;
        assert!(validate(&s).is_err());
        let mut s = builtin_presets()
            .into_iter()
            .find(|p| p.name == "prores-422hq")
            .unwrap()
            .settings;
        s.video.as_mut().unwrap().encoder = EncoderChoice::Nvenc;
        assert!(validate(&s).is_err());
        s.video.as_mut().unwrap().encoder = EncoderChoice::Cpu;
        s.video.as_mut().unwrap().profile = Some("ultra".into());
        assert!(validate(&s).is_err());
    }
}
