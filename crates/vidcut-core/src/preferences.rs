//! Application preferences, kept in `~/.config/vidcut/preferences.toml`, read with
//! `preferences.get` and changed with `preferences.set`. Project settings
//! (frame rate, size) belong to the project instead.

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
    pub gpu: Gpu,
    pub player: Player,
    pub thumbnails: Thumbnails,
    pub proxies: Proxies,
}

/// The viewer's player: memory for the decoded pictures it keeps around
/// the playhead. More memory, fewer waits: stepping and playing back
/// through files with keyframes far apart decode less again.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Player {
    /// Memory for pictures in MB. `0` (the default): the larger of 1 GB and
    /// half the computer's memory.
    pub memory_mb: u64,
    /// Video memory for pictures the GPU decoded and keeps there, in MB.
    /// `0` (the default): half the GPU's.
    pub video_memory_mb: u64,
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
}

impl Default for Gpu {
    fn default() -> Self {
        Self {
            acceleration: true,
            decoding: true,
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
    /// Memory for the timeline's filmstrip pictures in MB: above it, those
    /// shown least recently go first.
    pub timeline_memory_mb: u64,
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self {
            disk_cache: true,
            cache_limit_mb: 512,
            timeline_memory_mb: 256,
        }
    }
}

/// Playback proxies (decision D41): which media the running app makes them
/// for by itself, and their size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Proxies {
    /// While a project plays proxies, the running app makes them for the
    /// media these rules pick: those imported, and the pool's when proxies
    /// are turned on.
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
}

/// The codec proxies are made with (decision D41).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProxyCodec {
    /// AV1 on a hardware encoder when this machine has one, else H.264 with
    /// x264, which stays quick on old computers.
    #[default]
    Auto,
    /// AV1: on a hardware encoder when there is one, else with SVT-AV1.
    Av1,
    /// H.264 with x264.
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
        }
    }
}

impl Proxies {
    /// Whether the rules pick `m`: an image sequence, a video with more
    /// pixels than `larger_than`, or a video file averaging more than
    /// `above_mbps`. Stills and sound never.
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
    pub gpu: Option<GpuPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnails: Option<ThumbnailsPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxies: Option<ProxiesPatch>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GpuPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceleration: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoding: Option<bool>,
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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThumbnailsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_cache: Option<bool>,
    /// At least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_limit_mb: Option<u64>,
    /// At least 16.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_memory_mb: Option<u64>,
}

impl PreferencesPatch {
    /// A patch from `key=value` pairs with dotted keys, such as
    /// `thumbnails.cache_limit_mb=256`. Values are JSON when they parse as
    /// JSON, strings otherwise.
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
    /// These preferences with `patch` applied.
    pub fn patched(&self, patch: &PreferencesPatch) -> Result<Self> {
        let mut p = self.clone();
        if let Some(g) = &patch.gpu {
            p.gpu.acceleration = g.acceleration.unwrap_or(p.gpu.acceleration);
            p.gpu.decoding = g.decoding.unwrap_or(p.gpu.decoding);
        }
        if let Some(x) = &patch.player {
            p.player.memory_mb = x.memory_mb.unwrap_or(p.player.memory_mb);
            p.player.video_memory_mb = x.video_memory_mb.unwrap_or(p.player.video_memory_mb);
        }
        if let Some(t) = &patch.thumbnails {
            if let Some(v) = t.disk_cache {
                p.thumbnails.disk_cache = v;
            }
            if let Some(v) = t.cache_limit_mb {
                p.thumbnails.cache_limit_mb = v;
            }
            if let Some(v) = t.timeline_memory_mb {
                p.thumbnails.timeline_memory_mb = v;
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
        }
        p.check()?;
        Ok(p)
    }

    /// Fails with `USAGE` for values out of range.
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
        if self.thumbnails.timeline_memory_mb < 16 {
            return Err(Error::usage(
                "thumbnails.timeline_memory_mb must be at least 16",
            ));
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_acceleration_is_on_until_turned_off() {
        let p = Preferences::default();
        assert!(p.gpu.acceleration);
        let off = p
            .patched(&PreferencesPatch::from_pairs(&["gpu.acceleration=false".into()]).unwrap())
            .unwrap();
        assert!(!off.gpu.acceleration);
        assert_eq!(off.thumbnails, p.thumbnails);
        // A file without the key keeps it on.
        let read: Preferences = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(read.gpu.acceleration);
    }

    #[test]
    fn gpu_decoding_goes_off_on_its_own() {
        let p = Preferences::default();
        assert!(p.gpu.decoding);
        let off = p
            .patched(&PreferencesPatch::from_pairs(&["gpu.decoding=false".into()]).unwrap())
            .unwrap();
        assert!(!off.gpu.decoding);
        assert!(off.gpu.acceleration);
        // A file from before it keeps decoding on.
        let read: Preferences =
            serde_json::from_value(serde_json::json!({ "gpu": { "acceleration": true } })).unwrap();
        assert!(read.gpu.decoding);
    }

    #[test]
    fn the_player_takes_automatic_memory_unless_set() {
        let p = Preferences::default();
        assert_eq!((p.player.memory_mb, p.player.video_memory_mb), (0, 0));
        let set = p
            .patched(&PreferencesPatch::from_pairs(&["player.memory_mb=4096".into()]).unwrap())
            .unwrap();
        assert_eq!(
            (set.player.memory_mb, set.player.video_memory_mb),
            (4096, 0)
        );
        // 0 is automatic; otherwise at least 64 MB.
        for bad in ["player.memory_mb=8", "player.video_memory_mb=63"] {
            let patch = PreferencesPatch::from_pairs(&[bad.into()]).unwrap();
            assert_eq!(p.patched(&patch).unwrap_err().code, crate::ErrorCode::Usage);
        }
    }

    #[test]
    fn patches_change_only_what_they_name() {
        let p = Preferences::default();
        assert!(p.thumbnails.disk_cache);
        let off = p
            .patched(&PreferencesPatch {
                thumbnails: Some(ThumbnailsPatch {
                    disk_cache: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap();
        assert!(!off.thumbnails.disk_cache);
        assert_eq!(off.thumbnails.cache_limit_mb, 512);
        assert_eq!(p.patched(&PreferencesPatch::default()).unwrap(), p);
        let zero = PreferencesPatch {
            thumbnails: Some(ThumbnailsPatch {
                cache_limit_mb: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(p.patched(&zero).unwrap_err().code, crate::ErrorCode::Usage);
        // The timeline's pictures get at least 16 MB; 256 unless set.
        assert_eq!(p.thumbnails.timeline_memory_mb, 256);
        let tiny = PreferencesPatch {
            thumbnails: Some(ThumbnailsPatch {
                timeline_memory_mb: Some(8),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(p.patched(&tiny).unwrap_err().code, crate::ErrorCode::Usage);
        // Proxies: the size must suit the encoder, the limits be positive.
        let proxies = |x: ProxiesPatch| PreferencesPatch {
            proxies: Some(x),
            ..Default::default()
        };
        let smaller = p
            .patched(&proxies(ProxiesPatch {
                size: Some(540),
                ..Default::default()
            }))
            .unwrap();
        assert_eq!(
            (smaller.proxies.size, smaller.proxies.above_mbps),
            (540, 150)
        );
        for bad in [
            ProxiesPatch {
                size: Some(961),
                ..Default::default()
            },
            ProxiesPatch {
                size: Some(8000),
                ..Default::default()
            },
            ProxiesPatch {
                above_mbps: Some(0),
                ..Default::default()
            },
            ProxiesPatch {
                larger_than: Some([0, 1080]),
                ..Default::default()
            },
        ] {
            let e = p.patched(&proxies(bad.clone())).unwrap_err();
            assert_eq!(e.code, crate::ErrorCode::Usage, "{bad:?}");
        }
    }

    /// Spec D41: image sequences, very large pictures and high bitrates.
    #[test]
    fn the_proxy_rules_pick_heavy_media() {
        use crate::time::Rational;
        let fps = Rational::new(25, 1).unwrap();
        let rules = Proxies::default();
        // A 10 s file of `bytes` with `w`×`h` pictures.
        let media = |w: u32, h: u32, bytes: u64| {
            let mut info = crate::test_support::media_info(10.0, fps, true);
            let v = info.video.as_mut().unwrap();
            (v.width, v.height) = (w, h);
            serde_json::from_value::<Media>(serde_json::json!({
                "id": "m1", "path": "/media/a.mov", "name": "a.mov",
                "size": bytes, "mtime_ns": 0, "info": info,
            }))
            .unwrap()
        };
        // 4K at 100 Mb/s (125 MB in 10 s): the GPU plays it.
        assert!(!rules.wants(&media(3840, 2160, 125_000_000)));
        // 8K, or 200 Mb/s at 1080p (a ProRes file, say).
        assert!(rules.wants(&media(7680, 4320, 125_000_000)));
        assert!(rules.wants(&media(1920, 1080, 250_000_000)));
        // An image sequence, at any size, unless sequences are left out.
        let mut seq = media(320, 180, 1000);
        seq.sequence = Some(crate::model::ImageSequence {
            pattern: "f_%04d.png".into(),
            first: 1,
            count: 60,
        });
        assert!(rules.wants(&seq));
        let no_seq = Proxies {
            image_sequences: false,
            ..Proxies::default()
        };
        assert!(!no_seq.wants(&seq));
        // Stills and sound never.
        let mut still = media(7680, 4320, 125_000_000);
        still.info.video.as_mut().unwrap().still = true;
        assert!(!rules.wants(&still));
        let mut sound = media(7680, 4320, 125_000_000);
        sound.info.video = None;
        assert!(!rules.wants(&sound));
    }

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
                timeline_memory_mb: None,
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
    fn the_file_may_leave_keys_out_but_not_misspell_them() {
        let p: Preferences =
            serde_json::from_value(serde_json::json!({ "thumbnails": { "cache_limit_mb": 64 } }))
                .unwrap();
        assert!(p.thumbnails.disk_cache);
        assert_eq!(p.thumbnails.cache_limit_mb, 64);
        let empty: Preferences = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(empty, Preferences::default());
        let typo = serde_json::json!({ "thumbnails": { "disk_cahce": false } });
        assert!(serde_json::from_value::<Preferences>(typo).is_err());
    }
}
