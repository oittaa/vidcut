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
pub fn load() -> Result<Preferences> {
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

/// Whether the GPU decodes too (`gpu.decoding`, while `gpu.acceleration`
/// is on): in the viewer, exports, proxies, the timeline's tiles and the
/// two-up.
pub fn gpu_decoding() -> bool {
    load().map_or(true, |p| p.gpu.acceleration && p.gpu.decoding)
}

/// Where exports and proxies decode, as the preferences say: all on the
/// CPU while `gpu.decoding` is off; otherwise on `decoder`, the GPU the
/// viewer takes ([`gpu_decoder`]), and NVENC's pieces on NVDEC (also
/// without one).
pub fn decoding(decoder: Option<&vidcut_av::hw::GpuDecoder>) -> vidcut_av::hw::Decoding<'_> {
    match decoder {
        _ if !gpu_decoding() => vidcut_av::hw::Decoding::Cpu,
        Some(d) => vidcut_av::hw::Decoding::Gpu(d),
        None => vidcut_av::hw::Decoding::Nvdec,
    }
}

/// The GPU decoder the timeline's pictures (tiles, the two-up) decode on
/// while `on_gpu`: the viewer's, on a stream or device of their own
/// (`GpuDecoder::own`), so they don't wait in line with playback; `None`
/// to decode on the CPU.
pub fn timeline_decoder(on_gpu: bool) -> Option<vidcut_av::hw::GpuDecoder> {
    on_gpu
        .then(gpu_decoder)
        .flatten()
        .and_then(|d| d.own().ok())
}

/// The GPU decoder the viewer takes (`GpuDecoder::best`), opened once for
/// what decodes as it does: exports, proxies, the timeline's tiles and the
/// two-up. `None` without one; callers check `gpu.acceleration`.
pub fn gpu_decoder() -> Option<vidcut_av::hw::GpuDecoder> {
    static BEST: std::sync::OnceLock<Option<vidcut_av::hw::GpuDecoder>> =
        std::sync::OnceLock::new();
    BEST.get_or_init(vidcut_av::hw::GpuDecoder::best).clone()
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
    let codecs: Vec<Value> = [ProxyCodec::Auto, ProxyCodec::Av1, ProxyCodec::H264]
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
        // What `player.memory_mb` 0 means here.
        "player_memory_auto_mb": vidcut_av::player::Budget::automatic().memory >> 20,
    })
}
