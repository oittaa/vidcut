use std::path::PathBuf;

use vidcut_core::fingerprint::Origin;
use vidcut_core::model::Media;

pub fn config_dir() -> PathBuf {
    vidcut_core::dirs::config()
}

pub fn cache_dir() -> PathBuf {
    vidcut_core::dirs::cache()
}

pub fn media_origin(media: &Media) -> Origin {
    vidcut_core::fingerprint::origin(&media.path, media.size, media.mtime_ns)
}

pub fn proxy_origin(media: &Media) -> Origin {
    match &media.fingerprint {
        Some(content) => content_origin(content, media.size, media.mtime_ns),
        None => media_origin(media),
    }
}

fn content_origin(content: &str, size: u64, mtime_ns: i64) -> Origin {
    Origin {
        name: format!("{}-{size}-{mtime_ns}", content.get(..32).unwrap_or(content)).into(),
        text: format!("{size} {mtime_ns} {content}").into(),
    }
}

pub fn media_cache(dir: &str, origin: &Origin, ext: &str) -> PathBuf {
    cache_dir().join(dir).join(format!("{}.{ext}", origin.name))
}

pub fn state_dir() -> PathBuf {
    vidcut_core::store::state_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_is_named_by_its_source_content() {
        let content = "2fd8af6d5960715e00f1226c72fe55ec1612534ac6ce5cb81a086e4ff4fc3261";
        let o = content_origin(content, 431136792955, 1791643881192984611);
        assert_eq!(
            &*o.name,
            "2fd8af6d5960715e00f1226c72fe55ec-431136792955-1791643881192984611"
        );
    }
}
