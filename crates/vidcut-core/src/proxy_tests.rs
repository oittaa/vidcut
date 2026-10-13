//! Playback proxies (decision D41). The fake env records a path and does not
//! encode. Export plans keep original paths; only [`preview::apply_proxies`]
//! swaps the picture.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::render::{AudioPiece, VideoPiece, flatten};
use crate::test_support::Fixture;

fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({ "method": method, "params": params })
}

fn proxy_path(id: &str) -> PathBuf {
    // 1920×1080 fitted in 960, the default longest side.
    Path::new("/proxies").join(format!("{id}-960x540.mkv"))
}

#[test]
fn use_proxies_does_not_change_the_export_plan() {
    let mut f = Fixture::new(&[(4.0, true)]);
    f.ok(req("proxy.generate", json!({"media": "m1"})));
    f.ok(req("project.set", json!({"use_proxies": true})));
    f.append("m1", 0, 25);
    let plan = flatten(f.editor.project(), 0, 25).unwrap();
    match &plan.video[0] {
        VideoPiece::Source {
            path,
            origin,
            source_frame,
            image,
            ..
        } => {
            assert_eq!(path, &PathBuf::from("/media/1.mp4"));
            assert_eq!(*origin, 0.0);
            assert_eq!(*source_frame, 0);
            assert!(image.is_none());
        }
        other => panic!("video piece {other:?}"),
    }
    match &plan.audio[0][0] {
        AudioPiece::Source { path, .. } => assert_eq!(path, &PathBuf::from("/media/1.mp4")),
        other => panic!("audio piece {other:?}"),
    }
}

#[test]
fn dry_run_reports_the_path_without_saving_it() {
    let mut f = Fixture::new(&[(4.0, true)]);
    let rev = f.editor.project().rev;
    let r = f.ok(json!({
        "method": "proxy.generate",
        "dry_run": true,
        "params": { "media": "m1" }
    }));
    assert!(r.dry_run);
    assert_eq!(r.result["proxies"][0]["path"], json!(proxy_path("m1")));
    assert_eq!(f.editor.project().rev, rev);
    assert!(f.editor.project().media[0].proxy.is_none());
}

#[test]
fn proxy_size_keeps_aspect_and_does_not_enlarge() {
    use crate::model::proxy_size;
    assert_eq!(proxy_size(1920, 1080, 960), (960, 540));
    assert_eq!(proxy_size(320, 180, 960), (320, 180));
    assert_eq!(proxy_size(1080, 1920, 960), (540, 960));
}
