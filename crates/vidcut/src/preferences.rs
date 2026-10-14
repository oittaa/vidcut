//! Application preferences on disk (`~/.config/vidcut/preferences.toml`)
//! and what changing them does to the thumbnail cache.

use std::path::PathBuf;

use serde_json::{Value, json};
use vidcut_core::preferences::{Preferences, PreferencesPatch};
use vidcut_core::{Error, Result};

use crate::{paths, thumbcache};

pub fn path() -> PathBuf {
    paths::config_dir().join("preferences.toml")
}

/// The preferences; defaults for what the file leaves out, or without one.
/// The GPU they name (`gpu.device`) decodes and encodes from then on.
pub fn load() -> Result<Preferences> {
    let p = read()?;
    vidcut_av::gpus::choose(&p.gpu.device);
    Ok(p)
}

fn read() -> Result<Preferences> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::default()),
        Err(e) => return Err(Error::internal(format!("{}: {e}", path.display()))),
    };
    let p: Preferences = toml::from_str(&text).map_err(|e| {
        Error::usage(format!(
            "{}: {e}; fix it, or delete it to start from the defaults",
            path.display()
        ))
    })?;
    p.check()
        .map_err(|e| Error::usage(format!("{}: {}", path.display(), e.message)))?;
    Ok(p)
}

/// Whether the GPU may do its share (`gpu.acceleration`). A broken
/// preferences file fails `preferences.get` with the reason; the rest goes
/// on with the default, on.
pub fn gpu_acceleration() -> bool {
    load().map_or(true, |p| p.gpu.acceleration)
}

/// The audio device the player plays on (`player.audio_device`): `None`
/// for the system's default.
pub fn audio_device() -> Option<String> {
    load()
        .ok()
        .map(|p| p.player.audio_device)
        .filter(|d| !d.is_empty())
}

/// Where decoding happens, as the preferences say ([`gpu_decoding`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDecoding {
    /// The GPU decodes the streams it can (`gpu.acceleration` and
    /// `gpu.decoding` on), the CPU the rest. Off, the CPU decodes
    /// everything.
    pub on: bool,
    /// The GPU in use (`gpu.device`, else the best this computer has), by
    /// its id; `None` where no GPU is listed.
    pub gpu: Option<String>,
}

/// Where decoding happens now, the one place that says so. The viewer's
/// player decodes on the GPU in use (`GpuDecoder::best`); exports,
/// proxies, the timeline's tiles and the two-up on a decoder of their own
/// there ([`GpuDecoding::own_decoder`]). Reading it opens no decoder:
/// whatever keeps one compares this and makes another when it changes. A
/// broken preferences file decodes as the defaults do, on the GPU.
pub fn gpu_decoding() -> GpuDecoding {
    let on = load().map_or(true, |p| p.gpu.acceleration && p.gpu.decoding);
    GpuDecoding {
        on,
        gpu: vidcut_av::gpus::in_use().map(|g| g.id),
    }
}

/// For `VIDCUT_DEBUG`: on or off, and the GPU in use by its name and id.
impl std::fmt::Display for GpuDecoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.on {
            "GPU decoding on"
        } else {
            "GPU decoding off, the CPU decodes everything"
        })?;
        match &self.gpu {
            Some(id) => {
                let name = vidcut_av::gpus::find(vidcut_av::gpus::list(), id)
                    .map_or("", |g| g.name.as_str());
                write!(f, "; the GPU in use is {name} ({id})")
            }
            None => f.write_str("; no GPU is listed here"),
        }
    }
}

impl GpuDecoding {
    /// A decoder of its own on the GPU in use (`GpuDecoder::own`): on the
    /// viewer's, work beside playback would wait in line with it. `None`
    /// while the CPU decodes, or where no GPU decodes.
    pub fn own_decoder(&self) -> Option<vidcut_av::hw::GpuDecoder> {
        self.on
            .then(vidcut_av::hw::GpuDecoder::best)
            .flatten()
            .and_then(|d| d.own().ok())
    }
}

/// The viewer's memory for pictures: `player.memory_mb` and
/// `player.video_memory_mb`, `0` the automatic sizes (`Budget::automatic`).
pub fn player_budget() -> vidcut_av::player::Budget {
    let p = load().map(|p| p.player).unwrap_or_default();
    let mb = |n: u64| usize::try_from(n.saturating_mul(1 << 20)).unwrap_or(usize::MAX);
    vidcut_av::player::Budget {
        memory: match p.memory_mb {
            0 => vidcut_av::player::Budget::automatic().memory,
            n => mb(n),
        },
        video: (p.video_memory_mb != 0).then(|| mb(p.video_memory_mb)),
    }
}

/// `preferences.get`: the preferences, where they are kept, and what the
/// thumbnail cache holds.
pub fn get() -> Result<Value> {
    Ok(describe(&load()?))
}

/// `preferences.set`: applies `patch`, saves, and brings the thumbnail
/// cache in line (emptied when turned off, trimmed to the limit). A dry run
/// only checks and describes the result.
pub fn set(patch: &PreferencesPatch, dry_run: bool) -> Result<Value> {
    // One change at a time: each reads the file the one before wrote.
    static SETTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = SETTING.lock().unwrap_or_else(|e| e.into_inner());
    let p = load()?.patched(patch)?;
    if dry_run {
        return Ok(describe(&p));
    }
    let path = path();
    let dir = path.parent().expect("in the config dir");
    let text = format!(
        "# vidcut preferences: `vidcut preferences set --help`, or File > Preferences.\n\n{}",
        toml::to_string(&p).map_err(|e| Error::internal(e.to_string()))?
    );
    let part = path.with_extension("part");
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&part, text))
        .and_then(|()| std::fs::rename(&part, &path))
        .map_err(|e| Error::internal(format!("{}: {e}", path.display())))?;
    vidcut_av::gpus::choose(&p.gpu.device);
    if p.thumbnails.disk_cache {
        thumbcache::evict(
            &thumbcache::dir(),
            p.thumbnails.cache_limit_mb.saturating_mul(1 << 20),
        );
    } else {
        thumbcache::clear()
            .map_err(|e| Error::internal(format!("{}: {e}", thumbcache::dir().display())))?;
    }
    Ok(describe(&p))
}

fn describe(p: &Preferences) -> Value {
    use vidcut_core::preferences::ProxyCodec;
    let (files, bytes) = thumbcache::usage();
    // What each proxy codec choice makes proxies with on this machine.
    let codecs: Vec<Value> = [
        ProxyCodec::Auto,
        ProxyCodec::Av1,
        ProxyCodec::H264,
        ProxyCodec::Prores,
        ProxyCodec::Dnxhr,
    ]
    .into_iter()
    .map(|c| {
        let encoders = crate::env::proxy_encoders(c, p.gpu.acceleration);
        json!({
            "codec": c,
            "encoder": encoders.first().map(|e| e.name.clone()),
            "uses": encoders.first().map(|e| e.label()),
        })
    })
    .collect();
    json!({
        "path": path(),
        "preferences": p,
        "thumbnail_cache": {
            "path": thumbcache::dir(),
            "files": files,
            "mb": (bytes as f64 / f64::from(1 << 20) * 10.0).round() / 10.0,
        },
        "proxy_codecs": codecs,
        // The GPUs `gpu.device` can name; the one it names, as found here;
        // and the one that works when none is chosen or found.
        "gpus": vidcut_av::gpus::list(),
        "gpu_chosen": vidcut_av::gpus::find(vidcut_av::gpus::list(), &p.gpu.device),
        "gpu_automatic": vidcut_av::gpus::automatic(),
        // What `player.memory_mb` 0 means here.
        "player_memory_auto_mb": vidcut_av::player::Budget::automatic().memory >> 20,
    })
}
