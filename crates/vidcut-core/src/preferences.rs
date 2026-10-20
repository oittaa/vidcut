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
    pub proxies: Proxies,
    pub transcripts: Transcripts,
    pub services: Services,
}

/// The paid speech services, xAI and Google. A key comes from the
/// environment (`XAI_API_KEY`, `GEMINI_API_KEY`), else the key command, else
/// the system's keychain (`services.set_key`); it is never kept in this file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Services {
    /// A command whose output is the xAI key, such as `pass show xai`, run
    /// when a key is needed.
    pub xai_key_command: String,
    /// A command whose output is the Gemini key, run when a key is needed.
    pub google_key_command: String,
    /// The GUI asks before audio is sent to xAI.
    pub ask_xai: bool,
    /// The GUI asks before audio is sent to Google.
    pub ask_google: bool,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            xai_key_command: String::new(),
            google_key_command: String::new(),
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
    /// opens with them shown instead of decoding every clip again.
    pub disk_cache: bool,
    /// The size limit of that cache in MB: the least recently used
    /// thumbnails go first.
    pub cache_limit_mb: u64,
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self {
            disk_cache: true,
            cache_limit_mb: 512,
        }
    }
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
    pub proxies: Option<ProxiesPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcripts: Option<TranscriptsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<ServicesPatch>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServicesPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xai_key_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_key_command: Option<String>,
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
    /// At least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_limit_mb: Option<u64>,
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
        if let Some(t) = &patch.thumbnails {
            if let Some(v) = t.disk_cache {
                p.thumbnails.disk_cache = v;
            }
            if let Some(v) = t.cache_limit_mb {
                p.thumbnails.cache_limit_mb = v;
            }
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
            if let Some(c) = &x.xai_key_command {
                s.xai_key_command = c.trim().to_string();
            }
            if let Some(c) = &x.google_key_command {
                s.google_key_command = c.trim().to_string();
            }
            s.ask_xai = x.ask_xai.unwrap_or(s.ask_xai);
            s.ask_google = x.ask_google.unwrap_or(s.ask_google);
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
        if self.thumbnails.cache_limit_mb == 0 {
            return Err(Error::usage("thumbnails.cache_limit_mb must be at least 1"));
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
        let p = pairs(&[
            "thumbnails.cache_limit_mb=256",
            "thumbnails.disk_cache=false",
        ])
        .unwrap();
        assert_eq!(
            p.thumbnails,
            Some(ThumbnailsPatch {
                disk_cache: Some(false),
                cache_limit_mb: Some(256),
            })
        );
        assert_eq!(pairs(&[]).unwrap(), PreferencesPatch::default());
        for bad in [
            &["thumbnails.disk_cache"][..],
            &["thumbnails.size=3"],
            &["thumbnails=3"],
            &["thumbnails.cache_limit_mb=-1"],
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
}
