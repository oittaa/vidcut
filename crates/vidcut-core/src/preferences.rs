//! Application preferences, the counterpart of Resolve's Preferences window
//! (p. 94): kept in `~/.config/vidcut/preferences.toml`, read with
//! `preferences.get` and changed with `preferences.set`. Project settings
//! (frame rate, size) belong to the project instead.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

/// Everything a person may set for vidcut as a whole. Missing keys in the
/// file take their defaults: it is edited by hand too.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub thumbnails: Thumbnails,
}

/// The Media Pool's thumbnails.
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

/// Changes to the preferences: what is left out stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreferencesPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnails: Option<ThumbnailsPatch>,
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
        if let Some(t) = &patch.thumbnails {
            if let Some(v) = t.disk_cache {
                p.thumbnails.disk_cache = v;
            }
            if let Some(v) = t.cache_limit_mb {
                p.thumbnails.cache_limit_mb = v;
            }
        }
        p.check()?;
        Ok(p)
    }

    /// Fails with `USAGE` for values out of range.
    pub fn check(&self) -> Result<()> {
        if self.thumbnails.cache_limit_mb == 0 {
            return Err(Error::usage("thumbnails.cache_limit_mb must be at least 1"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_change_only_what_they_name() {
        let p = Preferences::default();
        assert!(p.thumbnails.disk_cache);
        let off = p
            .patched(&PreferencesPatch {
                thumbnails: Some(ThumbnailsPatch {
                    disk_cache: Some(false),
                    cache_limit_mb: None,
                }),
            })
            .unwrap();
        assert!(!off.thumbnails.disk_cache);
        assert_eq!(off.thumbnails.cache_limit_mb, 512);
        assert_eq!(p.patched(&PreferencesPatch::default()).unwrap(), p);
        let zero = PreferencesPatch {
            thumbnails: Some(ThumbnailsPatch {
                disk_cache: None,
                cache_limit_mb: Some(0),
            }),
        };
        assert_eq!(p.patched(&zero).unwrap_err().code, crate::ErrorCode::Usage);
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
