use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::model::{Media, PROXY_LONGEST};

/// Everything a person may set for vidcut as a whole. Missing keys in the
/// file take their defaults: it is edited by hand too.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub appearance: Appearance,
    pub gpu: Gpu,
    pub player: Player,
    pub thumbnails: Thumbnails,
    pub cache: Cache,
    pub proxies: Proxies,
    pub transcripts: Transcripts,
    pub services: Services,
    pub viewer: Viewer,
}

/// Viewer framing and safe-area guides (decision D90). They draw only in the
/// viewer. An export never shows them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Viewer {
    pub guides: Guides,
}

/// Framing and safe-area overlay choices for the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Guides {
    /// Framing guides (aspect crops) are drawn when this is on.
    pub enabled: bool,
    /// Which aspect crop the framing guides use. `default` is the timeline's
    /// own aspect.
    pub framing: Framing,
    /// The outer boundary of the framing crop (or of the picture).
    pub extents: bool,
    /// Action safe: 93% of the extents width and height (decision D90).
    pub action: bool,
    /// Title safe: 90% of the extents width and height (decision D90).
    pub title: bool,
    /// Horizontal and vertical center lines of the extents.
    pub center: bool,
}

impl Default for Guides {
    fn default() -> Self {
        Self {
            enabled: false,
            framing: Framing::Default,
            extents: true,
            action: true,
            title: true,
            center: false,
        }
    }
}

/// Share of the extents used for action safe (SMPTE ST 2046-1 / EBU R 95).
pub const ACTION_SAFE: f32 = 0.93;

/// Share of the extents used for title safe (SMPTE ST 2046-1 / EBU R 95).
pub const TITLE_SAFE: f32 = 0.90;

/// Aspect ratio for a framing guide. `Default` is the project's frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Framing {
    #[default]
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "social_1_1")]
    Social1_1,
    #[serde(rename = "social_4_5")]
    Social4_5,
    #[serde(rename = "social_9_16")]
    Social9_16,
    #[serde(rename = "social_1_91")]
    Social1_91,
    #[serde(rename = "social_16_9")]
    Social16_9,
    #[serde(rename = "broadcast_1_33")]
    Broadcast1_33,
    #[serde(rename = "broadcast_1_66")]
    Broadcast1_66,
    #[serde(rename = "broadcast_1_77")]
    Broadcast1_77,
    #[serde(rename = "broadcast_1_85")]
    Broadcast1_85,
    #[serde(rename = "broadcast_2_35")]
    Broadcast2_35,
}

impl Framing {
    /// Width over height for the crop. `None` for the project's own aspect.
    pub fn aspect(self) -> Option<f32> {
        Some(match self {
            Self::Default => return None,
            Self::Social1_1 => 1.0,
            Self::Social4_5 => 4.0 / 5.0,
            Self::Social9_16 => 9.0 / 16.0,
            Self::Social1_91 => 1.91,
            Self::Social16_9 => 16.0 / 9.0,
            Self::Broadcast1_33 => 1.33,
            Self::Broadcast1_66 => 1.66,
            Self::Broadcast1_77 => 1.77,
            Self::Broadcast1_85 => 1.85,
            Self::Broadcast2_35 => 2.35,
        })
    }
}

/// Axis-aligned box as `x`, `y`, `w`, `h` in the same units as the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl GuideBox {
    pub fn from_size(w: f32, h: f32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w,
            h,
        }
    }

    pub fn fit_aspect(self, aspect: f32) -> Self {
        let outer = self.w / self.h.max(1e-6);
        if outer > aspect {
            let h = self.h;
            let w = h * aspect;
            Self {
                x: self.x + (self.w - w) * 0.5,
                y: self.y,
                w,
                h,
            }
        } else {
            let w = self.w;
            let h = w / aspect.max(1e-6);
            Self {
                x: self.x,
                y: self.y + (self.h - h) * 0.5,
                w,
                h,
            }
        }
    }

    pub fn inset_fraction(self, fraction: f32) -> Self {
        let w = self.w * fraction;
        let h = self.h * fraction;
        Self {
            x: self.x + (self.w - w) * 0.5,
            y: self.y + (self.h - h) * 0.5,
            w,
            h,
        }
    }

    pub fn center(self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

/// What to paint over one picture. `crop` shades outside the framing box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideOverlay {
    pub frame: GuideBox,
    pub crop: bool,
    pub extents: Option<GuideBox>,
    pub action: Option<GuideBox>,
    pub title: Option<GuideBox>,
    pub center: Option<(f32, f32)>,
}

/// Patch for [`Guides`]: omitted fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuidesPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framing: Option<Framing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extents: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<bool>,
}

impl GuidesPatch {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.framing.is_none()
            && self.extents.is_none()
            && self.action.is_none()
            && self.title.is_none()
            && self.center.is_none()
    }
}

impl Guides {
    pub fn patched(self, patch: &GuidesPatch) -> Self {
        Self {
            enabled: patch.enabled.unwrap_or(self.enabled),
            framing: patch.framing.unwrap_or(self.framing),
            extents: patch.extents.unwrap_or(self.extents),
            action: patch.action.unwrap_or(self.action),
            title: patch.title.unwrap_or(self.title),
            center: patch.center.unwrap_or(self.center),
        }
    }

    /// Empty patch toggles framing guides. Named fields change; others stay.
    pub fn applied(self, patch: &GuidesPatch) -> Self {
        if patch.is_empty() {
            return Self {
                enabled: !self.enabled,
                ..self
            };
        }
        self.patched(patch)
    }

    /// Geometry for the viewer. Empty when nothing would draw.
    pub fn overlay(self, picture_w: f32, picture_h: f32) -> Option<GuideOverlay> {
        if !(picture_w > 0.0 && picture_h > 0.0) {
            return None;
        }
        let picture = GuideBox::from_size(picture_w, picture_h);
        let frame = if self.enabled {
            match self.framing.aspect() {
                Some(a) => picture.fit_aspect(a),
                None => picture,
            }
        } else {
            picture
        };
        let crop = self.enabled
            && ((picture.w - frame.w).abs() > 0.5 || (picture.h - frame.h).abs() > 0.5);
        let extents = self.extents.then_some(frame);
        let action = self.action.then(|| frame.inset_fraction(ACTION_SAFE));
        let title = self.title.then(|| frame.inset_fraction(TITLE_SAFE));
        let center = self.center.then(|| frame.center());
        if !crop && extents.is_none() && action.is_none() && title.is_none() && center.is_none() {
            return None;
        }
        Some(GuideOverlay {
            frame,
            crop,
            extents,
            action,
            title,
            center,
        })
    }
}

/// Patch for [`Viewer`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewerPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guides: Option<GuidesPatch>,
}

/// The paid speech services, xAI and Google. A key comes from the
/// environment (`XAI_API_KEY`, `GEMINI_API_KEY`), else the system's keychain
/// (`services.set_key`); it is never kept in this file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Services {
    /// The GUI asks before audio is sent to xAI.
    pub ask_xai: bool,
    /// The GUI asks before audio is sent to Google.
    pub ask_google: bool,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            ask_xai: true,
            ask_google: true,
        }
    }
}

/// Transcripts: the filler words the Transcription window marks and Delete
/// Filler Words takes out, per language code (`en`, `fi`), each list in place
/// of the built-in one for that language. A project's own list goes before
/// these.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Transcripts {
    pub fillers: BTreeMap<String, Vec<String>>,
}

/// How the GUI looks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    /// The theme, by name: a built-in one (`native`, the default, light or
    /// dark as the computer is set, `omarchy`, which follows the current
    /// Omarchy theme, `starship`, `neon`, `cybertruck`, `midnight`, `studio`,
    /// `native-dark`, `native-light`, `classic`) or a theme file's in the
    /// themes folder. A path to a `.toml` file copies it into that folder
    /// and uses it.
    pub theme: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "native".into(),
        }
    }
}

/// The viewer's player: memory for the decoded pictures it keeps around
/// the playhead (more memory, fewer waits: stepping and playing back
/// through files with keyframes far apart decode less again), and where
/// its sound plays.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Player {
    /// Memory for pictures in MB. `0` (the default): the larger of 1 GB and
    /// half the computer's memory.
    pub memory_mb: u64,
    /// Video memory for pictures the GPU decoded and keeps there, in MB.
    /// `0` (the default): half the memory the system reports for the GPU
    /// that decodes them (its dedicated video memory on Windows, a discrete
    /// GPU's largest device-local Vulkan heap on Linux), and at least 1 GB.
    /// On Apple silicon, where the GPU's memory is the computer's, `0`
    /// counts them in `memory_mb` instead.
    pub video_memory_mb: u64,
    /// The audio device the player plays on, by its id (`vidcut doctor`
    /// lists them as `audio_outputs`). Empty (the default): the system's
    /// default device, which it plays on also while the chosen one isn't
    /// there.
    pub audio_device: String,
}

/// The graphics card's share of the work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Gpu {
    /// Use the GPU to decode, scale and encode: in the preview, exports and
    /// proxies. Off, all of it runs on the CPU, even when an export names a
    /// GPU encoder.
    pub acceleration: bool,
    /// While `acceleration` is on, decode H.264, HEVC, AV1 and VP9 on the
    /// GPU in the viewer, exports and proxies, and all but H.264 in the
    /// timeline's pictures. Off, the CPU decodes and scales them and the
    /// GPU only encodes, which is faster where the GPU is small and the CPU
    /// large.
    pub decoding: bool,
    /// The GPU that decodes and encodes, by its id (`preferences.get` lists
    /// them as `gpus`): its PCI vendor and device ids and its slot, the
    /// same on every system the computer starts. Empty (the default): NVIDIA's
    /// when there is one, else the first other; also while the chosen one
    /// isn't there.
    pub device: String,
}

impl Default for Gpu {
    fn default() -> Self {
        Self {
            acceleration: true,
            decoding: true,
            device: String::new(),
        }
    }
}

/// The Media Pool's thumbnails and the timeline's filmstrip pictures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Thumbnails {
    /// Keep thumbnails on disk (`~/.cache/vidcut/thumbs`), so a project
    /// opens with them shown instead of decoding every clip again. They
    /// count toward `cache`'s limit.
    pub disk_cache: bool,
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self { disk_cache: true }
    }
}

/// What vidcut keeps in its cache dir and can make again: thumbnails,
/// waveforms, the measurements of media, compiled shaders, decoder test
/// clips, and proxies kept there. Past the limit, the least recently used
/// files go first. Speech models, paid replies and transcripts stay and
/// don't count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Cache {
    /// What sets the limit: `percent` (the default) or `gb`. Either way the
    /// cache shrinks to keep 20% of its disk free.
    pub limit: CacheLimit,
    /// The limit as a share of the disk the cache dir is on: 1 to 100
    /// percent.
    pub percent: u32,
    /// The limit in GB, at least 1; needed when `limit` is `gb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gb: Option<u64>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            limit: CacheLimit::Percent,
            percent: 10,
            gb: None,
        }
    }
}

/// What sets the cache's limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CacheLimit {
    /// `percent` of the disk.
    #[default]
    Percent,
    /// `gb`.
    Gb,
}

/// Playback proxies (decision D41): which media the running app makes them
/// for by itself, and their size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Proxies {
    /// While a project plays proxies, the running app makes them for the
    /// media these rules pick: those imported, the pool's when proxies are
    /// turned on, and again for any whose proxy file is gone.
    pub auto: bool,
    /// Image sequences get one.
    pub image_sequences: bool,
    /// Videos with more pixels than this get one: `[width, height]`.
    pub larger_than: [u32; 2],
    /// Videos whose file averages more than this many Mb/s get one.
    pub above_mbps: u32,
    /// The longest side of a proxy in pixels, even, from 16 to 7680.
    pub size: u32,
    /// The codec proxies are made with.
    pub codec: ProxyCodec,
    /// Where this computer keeps proxies: a folder in the project's folder
    /// (`proxies`, the default), or an absolute path. Every project shares
    /// an absolute one, so a file used in several projects gets one proxy;
    /// with a project on a network drive, proxies can stay on a local one.
    /// A project names its proxies by file, and each computer looks for
    /// them in its own folder: after a change, move the files there or let
    /// them be made again.
    pub folder: PathBuf,
}

/// The codec proxies are made with (decision D41).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProxyCodec {
    /// The first of DNxHR LB, ProRes Proxy, AV1, HEVC and H.264 that the
    /// GPU makes and plays. Else the first the GPU plays, else the first it
    /// makes, else DNxHR LB in software. ProRes Proxy comes before AV1 and
    /// HEVC: every frame is a keyframe, and a Mac's GPU and NVIDIA's make it
    /// and show its frames from GPU memory.
    #[default]
    Auto,
    /// DNxHR LB: every frame a keyframe, the files many times larger. At
    /// least 256×120.
    Dnxhr,
    /// ProRes 422 Proxy: every frame a keyframe, the files many times larger.
    /// On Apple's ProRes hardware (VideoToolbox) when there is one, else on
    /// the GPU through Vulkan (FFmpeg 8.1 and later). A 4:2:2 source keeps
    /// its color. Where Apple's hardware takes no 4:2:2, its pictures are
    /// 4:2:0, and `preferences.get` names it `ProRes Proxy from 4:2:0`.
    Prores,
    /// AV1: on a hardware encoder when there is one, else with SVT-AV1.
    /// 10-bit for a 10-bit source where the encoder takes it.
    Av1,
    /// HEVC: on a hardware encoder when there is one (NVENC, AMF, VAAPI,
    /// QSV, VideoToolbox), else with x265. 10-bit for a 10-bit source where
    /// the encoder takes it.
    Hevc,
    /// H.264: on a hardware encoder when there is one, else with x264.
    H264,
}

impl Default for Proxies {
    fn default() -> Self {
        Self {
            auto: true,
            image_sequences: true,
            larger_than: [3840, 2160],
            above_mbps: 150,
            size: PROXY_LONGEST,
            codec: ProxyCodec::Auto,
            folder: PathBuf::from("proxies"),
        }
    }
}

impl Proxies {
    pub fn folder_in(&self, project_dir: &Path) -> PathBuf {
        project_dir.join(&self.folder)
    }

    pub fn wants(&self, m: &Media) -> bool {
        let Some(v) = m.info.video.as_ref().filter(|v| !v.still) else {
            return false;
        };
        if m.sequence.is_some() && self.image_sequences {
            return true;
        }
        let [w, h] = self.larger_than;
        if u64::from(v.width) * u64::from(v.height) > u64::from(w) * u64::from(h) {
            return true;
        }
        let secs = m.info.duration;
        m.sequence.is_none()
            && secs > 0.0
            && m.size as f64 * 8.0 / secs / 1e6 > f64::from(self.above_mbps)
    }
}

/// Changes to the preferences: what is left out stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreferencesPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<AppearancePatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnails: Option<ThumbnailsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CachePatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxies: Option<ProxiesPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcripts: Option<TranscriptsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<ServicesPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<ViewerPatch>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServicesPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_xai: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_google: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscriptsPatch {
    /// Filler words per language code: a list replaces that language's,
    /// `null` goes back to the built-in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fillers: Option<BTreeMap<String, Option<Vec<String>>>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppearancePatch {
    /// A theme's name, or a path to a theme file to import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GpuPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceleration: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoding: Option<bool>,
    /// A GPU's id, or empty for the automatic one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayerPatch {
    /// `0` (automatic) or at least 64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    /// `0` (automatic) or at least 64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_memory_mb: Option<u64>,
    /// A device's id, or empty for the system's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_device: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProxiesPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_sequences: Option<bool>,
    /// `[width, height]`, each at least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub larger_than: Option<[u32; 2]>,
    /// At least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above_mbps: Option<u32>,
    /// Even, from 16 to 7680.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<ProxyCodec>,
    /// A folder in the project's folder, or an absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThumbnailsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_cache: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CachePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<CacheLimit>,
    /// 1 to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u32>,
    /// At least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gb: Option<u64>,
}

pub fn set_fillers(
    lists: &mut BTreeMap<String, Vec<String>>,
    changes: &BTreeMap<String, Option<Vec<String>>>,
) -> Result<()> {
    for (language, words) in changes {
        let language = language.trim().to_lowercase();
        if language.is_empty() {
            return Err(Error::usage(
                "filler words need a language code, such as `en`",
            ));
        }
        match words {
            Some(words) => {
                let words = crate::transcript::filler_list(words).map_err(Error::usage)?;
                lists.insert(language, words);
            }
            None => {
                lists.remove(&language);
            }
        }
    }
    Ok(())
}

impl PreferencesPatch {
    pub fn from_pairs(pairs: &[String]) -> Result<Self> {
        let mut root = Value::Object(Default::default());
        for pair in pairs {
            let (path, raw) = pair
                .split_once('=')
                .ok_or_else(|| Error::usage(format!("{pair:?}: expected key=value")))?;
            let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.into()));
            let keys: Vec<&str> = path.split('.').collect();
            let (last, groups) = keys.split_last().expect("split gives one key at least");
            let mut node = &mut root;
            for key in groups {
                node = node
                    .as_object_mut()
                    .ok_or_else(|| Error::usage(format!("{path}: {key} is not a group")))?
                    .entry(*key)
                    .or_insert_with(|| Value::Object(Default::default()));
            }
            node.as_object_mut()
                .ok_or_else(|| Error::usage(format!("{path}: not a group")))?
                .insert((*last).to_string(), value);
        }
        serde_json::from_value(root).map_err(|e| Error::usage(format!("preferences: {e}")))
    }
}

impl Preferences {
    pub fn patched(&self, patch: &PreferencesPatch) -> Result<Self> {
        let mut p = self.clone();
        if let Some(t) = patch.appearance.as_ref().and_then(|a| a.theme.as_ref()) {
            p.appearance.theme = t.clone();
        }
        if let Some(g) = &patch.gpu {
            p.gpu.acceleration = g.acceleration.unwrap_or(p.gpu.acceleration);
            p.gpu.decoding = g.decoding.unwrap_or(p.gpu.decoding);
            if let Some(d) = &g.device {
                p.gpu.device = d.clone();
            }
        }
        if let Some(x) = &patch.player {
            p.player.memory_mb = x.memory_mb.unwrap_or(p.player.memory_mb);
            p.player.video_memory_mb = x.video_memory_mb.unwrap_or(p.player.video_memory_mb);
            if let Some(d) = &x.audio_device {
                p.player.audio_device = d.clone();
            }
        }
        if let Some(v) = patch.thumbnails.as_ref().and_then(|t| t.disk_cache) {
            p.thumbnails.disk_cache = v;
        }
        if let Some(x) = &patch.cache {
            let c = &mut p.cache;
            c.limit = x.limit.unwrap_or(c.limit);
            c.percent = x.percent.unwrap_or(c.percent);
            c.gb = x.gb.or(c.gb);
        }
        if let Some(x) = &patch.proxies {
            let p = &mut p.proxies;
            p.auto = x.auto.unwrap_or(p.auto);
            p.image_sequences = x.image_sequences.unwrap_or(p.image_sequences);
            p.larger_than = x.larger_than.unwrap_or(p.larger_than);
            p.above_mbps = x.above_mbps.unwrap_or(p.above_mbps);
            p.size = x.size.unwrap_or(p.size);
            p.codec = x.codec.unwrap_or(p.codec);
            if let Some(f) = &x.folder {
                p.folder = f.clone();
            }
        }
        if let Some(fillers) = patch.transcripts.as_ref().and_then(|t| t.fillers.as_ref()) {
            set_fillers(&mut p.transcripts.fillers, fillers)?;
        }
        if let Some(x) = &patch.services {
            let s = &mut p.services;
            s.ask_xai = x.ask_xai.unwrap_or(s.ask_xai);
            s.ask_google = x.ask_google.unwrap_or(s.ask_google);
        }
        if let Some(g) = patch.viewer.as_ref().and_then(|v| v.guides.as_ref()) {
            p.viewer.guides = p.viewer.guides.patched(g);
        }
        p.check()?;
        Ok(p)
    }

    pub fn check(&self) -> Result<()> {
        for (key, mb) in [
            ("player.memory_mb", self.player.memory_mb),
            ("player.video_memory_mb", self.player.video_memory_mb),
        ] {
            if (1..64).contains(&mb) {
                return Err(Error::usage(format!(
                    "{key} must be 0 (automatic) or at least 64"
                )));
            }
        }
        let c = &self.cache;
        if !(1..=100).contains(&c.percent) {
            return Err(Error::usage("cache.percent must be from 1 to 100"));
        }
        if c.gb == Some(0) {
            return Err(Error::usage("cache.gb must be at least 1"));
        }
        if c.limit == CacheLimit::Gb && c.gb.is_none() {
            return Err(Error::usage("cache.limit is gb: set cache.gb too"));
        }
        let x = &self.proxies;
        if x.larger_than.contains(&0) {
            return Err(Error::usage(
                "proxies.larger_than needs a width and height of at least 1",
            ));
        }
        if x.above_mbps == 0 {
            return Err(Error::usage("proxies.above_mbps must be at least 1"));
        }
        if !(16..=7680).contains(&x.size) || !x.size.is_multiple_of(2) {
            return Err(Error::usage("proxies.size must be even, from 16 to 7680"));
        }
        if x.folder.as_os_str().is_empty() {
            return Err(Error::usage(
                "proxies.folder names a folder: `proxies`, the default, is one in the project's folder",
            ));
        }
        if x.folder.starts_with("~") {
            return Err(Error::usage(
                "proxies.folder: write the home folder out; `~` is not expanded",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_make_patches() {
        let pairs = |p: &[&str]| {
            PreferencesPatch::from_pairs(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        let p = pairs(&["cache.percent=5", "thumbnails.disk_cache=false"]).unwrap();
        assert_eq!(
            p.thumbnails,
            Some(ThumbnailsPatch {
                disk_cache: Some(false),
            })
        );
        assert_eq!(p.cache.and_then(|c| c.percent), Some(5));
        assert_eq!(pairs(&[]).unwrap(), PreferencesPatch::default());
        for bad in [
            &["thumbnails.disk_cache"][..],
            &["thumbnails.size=3"],
            &["thumbnails=3"],
            &["cache.percent=-1"],
            &["thumbnails.disk_cache=1", "thumbnails.disk_cache.x=2"],
        ] {
            let e = pairs(bad).unwrap_err();
            assert_eq!(e.code, crate::ErrorCode::Usage, "{bad:?}: {e:?}");
        }
    }

    #[test]
    fn a_relative_proxy_folder_is_in_the_project_folder() {
        let rules = Proxies::default();
        assert_eq!(
            rules.folder_in(Path::new("/films/talk")),
            PathBuf::from("/films/talk/proxies")
        );
    }

    #[test]
    fn an_absolute_proxy_folder_is_shared() {
        let rules = Proxies {
            folder: PathBuf::from("/local/proxies"),
            ..Proxies::default()
        };
        assert_eq!(
            rules.folder_in(Path::new("/nas/talk")),
            PathBuf::from("/local/proxies")
        );
    }

    #[test]
    fn a_proxy_folder_with_a_tilde_is_refused() {
        let patch = PreferencesPatch {
            proxies: Some(ProxiesPatch {
                folder: Some(PathBuf::from("~/proxies")),
                ..ProxiesPatch::default()
            }),
            ..PreferencesPatch::default()
        };
        let e = Preferences::default().patched(&patch).unwrap_err();
        assert_eq!(
            e.message,
            "proxies.folder: write the home folder out; `~` is not expanded"
        );
    }

    #[test]
    fn action_safe_is_ninety_three_percent_of_extents() {
        let g = Guides {
            enabled: true,
            framing: Framing::Default,
            extents: true,
            action: true,
            title: false,
            center: false,
        };
        let o = g.overlay(1920.0, 1080.0).unwrap();
        let a = o.action.unwrap();
        assert!((a.w - 1920.0 * ACTION_SAFE).abs() < 0.01);
        assert!((a.h - 1080.0 * ACTION_SAFE).abs() < 0.01);
        assert!((a.x - 1920.0 * (1.0 - ACTION_SAFE) * 0.5).abs() < 0.01);
    }

    #[test]
    fn title_safe_is_ninety_percent_of_extents() {
        let g = Guides {
            enabled: true,
            framing: Framing::Default,
            extents: false,
            action: false,
            title: true,
            center: false,
        };
        let o = g.overlay(1000.0, 1000.0).unwrap();
        let t = o.title.unwrap();
        assert!((t.w - 900.0).abs() < 0.01);
        assert!((t.h - 900.0).abs() < 0.01);
        assert!((t.x - 50.0).abs() < 0.01);
    }

    #[test]
    fn social_nine_sixteen_crops_a_wide_picture() {
        let g = Guides {
            enabled: true,
            framing: Framing::Social9_16,
            extents: true,
            action: false,
            title: false,
            center: false,
        };
        let o = g.overlay(1920.0, 1080.0).unwrap();
        let e = o.extents.unwrap();
        assert!((e.h - 1080.0).abs() < 0.01);
        assert!((e.w - 1080.0 * 9.0 / 16.0).abs() < 0.01);
        assert!((e.x - (1920.0 - e.w) * 0.5).abs() < 0.01);
    }

    #[test]
    fn guides_patch_sets_only_named_fields() {
        let g = Guides::default().patched(&GuidesPatch {
            enabled: Some(true),
            framing: Some(Framing::Broadcast2_35),
            ..GuidesPatch::default()
        });
        assert!(g.enabled);
        assert_eq!(g.framing, Framing::Broadcast2_35);
        assert!(g.extents);
        assert!(!g.center);
    }

    #[test]
    fn nothing_to_draw_yields_no_overlay() {
        let g = Guides {
            enabled: true,
            framing: Framing::Default,
            extents: false,
            action: false,
            title: false,
            center: false,
        };
        assert_eq!(g.overlay(100.0, 100.0), None);
    }
}
