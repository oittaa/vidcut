use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use vidcut_core::preferences::{Preferences, PreferencesPatch};
use vidcut_core::{Error, Result};

use crate::{paths, themes, thumbcache};

pub fn path() -> PathBuf {
    paths::config_dir().join("preferences.toml")
}

pub fn load() -> Result<Preferences> {
    let p = read()?;
    vidcut_av::gpus::choose(&p.gpu.device);
    Ok(p)
}

pub fn filler_lists() -> BTreeMap<String, Vec<String>> {
    read().map(|p| p.transcripts.fillers).unwrap_or_default()
}

pub fn fillers(language: &str) -> Option<Vec<String>> {
    filler_lists().remove(language)
}

fn read() -> Result<Preferences> {
    read_noting().map(|(p, _)| p)
}

fn read_noting() -> Result<(Preferences, Vec<String>)> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Preferences::default(), Vec::new()));
        }
        Err(e) => return Err(Error::internal(format!("{}: {e}", path.display()))),
    };
    let bad = |e: &dyn std::fmt::Display| {
        Error::usage(format!(
            "{}: {e}; fix it, or reset the preferences to the defaults (Reset Preferences in \
             File > Preferences, or vidcut preferences reset)",
            path.display()
        ))
    };
    let (p, ignored) = match toml::from_str::<Preferences>(&text) {
        Ok(p) => (p, Vec::new()),
        Err(strict) => {
            let mut table: toml::Table = toml::from_str(&text).map_err(|e| bad(&e))?;
            let ignored = prune(&mut table, &known(), "");
            if ignored.is_empty() {
                return Err(bad(&strict));
            }
            let p = toml::Value::Table(table)
                .try_into()
                .map_err(|e: toml::de::Error| bad(&e))?;
            (p, ignored)
        }
    };
    p.check()
        .map_err(|e| Error::usage(format!("{}: {}", path.display(), e.message)))?;
    Ok((p, ignored))
}

fn known() -> toml::Table {
    match toml::Value::try_from(Preferences::default()) {
        Ok(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    }
}

fn prune(table: &mut toml::Table, known: &toml::Table, at: &str) -> Vec<String> {
    let mut ignored = Vec::new();
    table.retain(|key, value| {
        let name = match at {
            "" => key.to_string(),
            _ => format!("{at}.{key}"),
        };
        match (known.get(key), value) {
            (None, _) => {
                ignored.push(name);
                false
            }
            (Some(toml::Value::Table(k)), toml::Value::Table(t)) if !k.is_empty() => {
                ignored.extend(prune(t, k, &name));
                true
            }
            _ => true,
        }
    });
    ignored
}

pub fn gpu_acceleration() -> bool {
    load().map_or(true, |p| p.gpu.acceleration)
}

pub fn audio_device() -> Option<String> {
    load()
        .ok()
        .map(|p| p.player.audio_device)
        .filter(|d| !d.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDecoding {
    pub on: bool,
    pub gpu: Option<String>,
}

pub fn gpu_decoding() -> GpuDecoding {
    let on = load().map_or(true, |p| p.gpu.acceleration && p.gpu.decoding);
    GpuDecoding {
        on,
        gpu: vidcut_av::gpus::in_use().map(|g| g.id),
    }
}

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
    pub fn own_decoder(&self) -> Option<vidcut_av::hw::GpuDecoder> {
        self.on
            .then(vidcut_av::hw::GpuDecoder::best)
            .flatten()
            .and_then(|d| d.own().ok())
    }
}

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

pub fn get() -> Result<Value> {
    let (p, ignored) = read_noting()?;
    vidcut_av::gpus::choose(&p.gpu.device);
    let mut v = describe(&p);
    v["ignored"] = json!(ignored);
    Ok(v)
}

static SETTING: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn reset(dry_run: bool) -> Result<Value> {
    let _one = SETTING.lock().unwrap_or_else(|e| e.into_inner());
    let path = path();
    let backup = path.exists().then(|| free_backup(&path));
    let p = Preferences::default();
    if !dry_run {
        if let Some(b) = &backup {
            std::fs::copy(&path, b)
                .map_err(|e| Error::internal(format!("{}: {e}", b.display())))?;
        }
        write(&p)?;
    }
    let mut v = describe(&p);
    v["backup"] = json!(backup);
    Ok(v)
}

fn free_backup(path: &std::path::Path) -> PathBuf {
    let first = path.with_extension("toml.bak");
    std::iter::once(first.clone())
        .chain((2..).map(|n| path.with_extension(format!("toml.bak.{n}"))))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

pub fn set(patch: &PreferencesPatch, dry_run: bool) -> Result<Value> {
    let _one = SETTING.lock().unwrap_or_else(|e| e.into_inner());
    let mut patch = patch.clone();
    if let Some(theme) = patch.appearance.as_mut().and_then(|a| a.theme.as_mut()) {
        if themes::is_file(theme) {
            *theme = themes::import(std::path::Path::new(theme), dry_run)?;
        } else {
            themes::load(theme)?;
        }
    }
    let p = load()?.patched(&patch)?;
    if dry_run {
        return Ok(describe(&p));
    }
    write(&p)?;
    Ok(describe(&p))
}

fn write(p: &Preferences) -> Result<()> {
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
    Ok(())
}

fn system_look() -> Value {
    let look = crate::look::current();
    let [r, g, b, _] = look.accent.base.to_array();
    json!({
        "light": look.light,
        "accent": format!("#{r:02X}{g:02X}{b:02X}"),
    })
}

fn memory_auto_mb() -> u64 {
    (vidcut_av::player::Budget::automatic().memory >> 20) as u64
}

fn video_memory_auto_mb(p: &Preferences) -> u64 {
    if vidcut_av::player::SHARED_MEMORY {
        return match p.player.memory_mb {
            0 => memory_auto_mb(),
            mb => mb,
        };
    }
    let gpu = vidcut_av::gpus::in_use()
        .filter(|_| p.gpu.acceleration && p.gpu.decoding)
        .and_then(|g| g.memory_mb);
    (vidcut_av::player::Budget::gpu_video(gpu) >> 20) as u64
}

fn describe(p: &Preferences) -> Value {
    use vidcut_core::preferences::ProxyCodec;
    let (files, bytes) = thumbcache::usage();
    let decoding = p.gpu.acceleration && p.gpu.decoding;
    let played = crate::export::plays(decoding);
    let codecs: Vec<Value> = std::iter::once(ProxyCodec::Auto)
        .chain(vidcut_av::proxy::AUTOMATIC)
        .map(|c| {
            let encoders = crate::env::proxy_encoders(c, p.gpu.acceleration, decoding);
            let first = encoders.first();
            let plays_on = played.as_ref().and_then(|played| {
                let e = first?;
                played
                    .iter()
                    .find(|(codec, _)| codec == e.codec)
                    .map(|(_, decoder)| decoder.clone())
            });
            json!({
                "codec": c,
                "encoder": first.map(|e| e.name.clone()),
                "uses": first.map(|e| e.label()),
                "plays_on": plays_on,
            })
        })
        .collect();
    json!({
        "path": path(),
        "preferences": p,
        "themes": themes::list(),
        "themes_folder": themes::dir(),
        "system_look": system_look(),
        "thumbnail_cache": {
            "path": thumbcache::dir(),
            "files": files,
            "mb": (bytes as f64 / f64::from(1 << 20) * 10.0).round() / 10.0,
        },
        "proxy_codecs": codecs,
        "gpus": vidcut_av::gpus::list(),
        "gpu_chosen": vidcut_av::gpus::find(vidcut_av::gpus::list(), &p.gpu.device),
        "gpu_automatic": vidcut_av::gpus::automatic(),
        "player_memory_auto_mb": memory_auto_mb(),
        "player_video_memory_auto_mb": video_memory_auto_mb(p),
        "player_video_memory_shared": vidcut_av::player::SHARED_MEMORY,
        "system_memory_mb": vidcut_av::player::system_memory().map(|b| b >> 20),
    })
}
