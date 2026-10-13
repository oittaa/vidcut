//! Playback proxies (decision D41). The fake env records a path and does not
//! encode. Export plans keep original paths; only [`preview::apply_proxies`]
//! swaps the picture.

use std::path::PathBuf;

use serde_json::json;

use crate::editor::{Editor, History};
use crate::error::ErrorCode;
use crate::model::{Project, Settings};
use crate::preview::apply_proxies;
use crate::render::{AudioPiece, VideoPiece, flatten};
use crate::request::Envelope;
use crate::test_support::{Fixture, audio_info, still_info};

fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({ "method": method, "params": params })
}

fn proxy_path(id: &str) -> String {
    // 1920×1080 fitted in 960, the default longest side.
    format!("/proxies/{id}-960x540.mkv")
}

#[test]
fn generate_records_a_proxy_and_media_get_shows_it() {
    let mut f = Fixture::new(&[(4.0, true)]);
    let r = f.ok(req("proxy.generate", json!({"media": "m1"})));
    assert_eq!(r.result["proxies"][0]["media"], "m1");
    assert_eq!(r.result["proxies"][0]["path"], proxy_path("m1"));
    assert_eq!(r.result["proxies"][0]["width"], 960);
    assert_eq!(r.result["proxies"][0]["height"], 540);
    let g = f.ok(req("media.get", json!({"media": "m1"})));
    assert_eq!(g.result["proxy"]["path"], proxy_path("m1"));
    assert_eq!(g.result["proxy"]["stale"], false);
    assert_eq!(g.result["proxy"]["codec"], "h264");
}

#[test]
fn naming_audio_or_a_still_is_an_error_and_generate_all_skips_them() {
    let mut f = Fixture::new(&[(4.0, true)]);
    f.env = std::mem::take(&mut f.env).with("/media/photo.png", still_info(4000, 3000));
    f.ok(req("media.add", json!({"paths": ["/media/photo.png"]})));
    let voice = {
        f.env
            .files
            .insert(PathBuf::from("/media/voice.wav"), audio_info(3.0));
        f.ok(req("media.add", json!({"paths": ["/media/voice.wav"]})))
    };
    assert_eq!(voice.result["media"][0], "m3");
    let e = f.err(req("proxy.generate", json!({"media": "m3"})));
    assert_eq!(e.code, ErrorCode::InvalidEdit, "{e}");
    let e = f.err(req("proxy.generate", json!({"media": "m2"})));
    assert_eq!(e.code, ErrorCode::InvalidEdit, "{e}");
    let e = f.err(req("proxy.generate", json!({"width": 961})));
    assert_eq!(e.code, ErrorCode::InvalidEdit, "{e}");
    let r = f.ok(req("proxy.generate", json!({})));
    assert_eq!(r.result["proxies"].as_array().unwrap().len(), 1);
    assert_eq!(r.result["proxies"][0]["media"], "m1");
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
fn preview_swaps_a_fresh_proxy_and_leaves_a_stale_or_missing_one() {
    let mut f = Fixture::new(&[(4.0, true)]);
    f.env = std::mem::take(&mut f.env).with_sequence("/renders", "frame_%04d.png", 1, 10);
    f.ok(req(
        "media.add",
        json!({"paths": ["/renders/frame_0001.png"], "sequence": true}),
    ));
    f.ok(req("proxy.generate", json!({})));
    f.ok(req("project.set", json!({"use_proxies": true})));
    f.append("m1", 10, 20);
    f.append("m2", 0, 10);
    let project = f.editor.project().clone();
    let mut plan = flatten(&project, 0, 30).unwrap();
    let before = plan.clone();
    apply_proxies(&mut plan, &project, |_| true);
    match &plan.video[0] {
        VideoPiece::Source {
            path,
            origin,
            source_frame,
            image,
            ..
        } => {
            assert_eq!(path, &PathBuf::from(proxy_path("m1")));
            assert_eq!(*origin, 0.0);
            assert_eq!(*source_frame, 10);
            assert!(image.is_none());
        }
        other => panic!("{other:?}"),
    }
    match &plan.video[1] {
        VideoPiece::Source {
            path,
            origin,
            source_frame,
            image,
            ..
        } => {
            assert_eq!(path, &PathBuf::from(proxy_path("m2")));
            assert_eq!(*origin, 0.0);
            assert_eq!(*source_frame, 0);
            assert!(image.is_none(), "a sequence plays as the proxy video");
        }
        other => panic!("{other:?}"),
    }
    match &plan.audio[0][0] {
        AudioPiece::Source { path, .. } => assert_eq!(path, &PathBuf::from("/media/1.mp4")),
        other => panic!("{other:?}"),
    }

    // The switch off, or a missing file: the original, a sequence as one.
    let mut off = before.clone();
    let mut settings = project.clone();
    settings.settings.use_proxies = false;
    apply_proxies(&mut off, &settings, |_| true);
    assert_eq!(off.video, before.video);
    let mut missing = before.clone();
    apply_proxies(&mut missing, &project, |_| false);
    assert_eq!(missing.video, before.video);

    // A changed source (mtime) is stale: not used, and media.get says so.
    let mut stale_project = project.clone();
    stale_project.media[0].mtime_ns = 99;
    let mut stale = flatten(&stale_project, 0, 30).unwrap();
    let original = stale.clone();
    apply_proxies(&mut stale, &stale_project, |_| true);
    assert_eq!(stale.video[0], original.video[0]);
    let mut ed = Editor::new(stale_project, History::default());
    let g = ed
        .apply(
            Envelope::from_json(req("media.get", json!({"media": "m1"}))).unwrap(),
            &f.env,
            None,
        )
        .unwrap();
    assert_eq!(g.result["proxy"]["stale"], true);
    assert_eq!(g.result["proxy"]["path"], proxy_path("m1"));
}

#[test]
fn remove_clears_the_proxy_and_relink_drops_a_stale_one() {
    let mut f = Fixture::new(&[(4.0, true)]);
    f.ok(req("proxy.generate", json!({"media": "m1"})));
    let r = f.ok(req("proxy.remove", json!({"media": "m1"})));
    assert_eq!(r.result["cleared"], json!(["m1"]));
    let g = f.ok(req("media.get", json!({"media": "m1"})));
    assert!(g.result.get("proxy").is_none());
    f.ok(req("proxy.generate", json!({})));
    f.env.files.insert(
        PathBuf::from("/media/other.mp4"),
        crate::test_support::media_info(4.0, crate::time::Rational::new(25, 1).unwrap(), true),
    );
    // Another file (another size): its proxy would show other pictures.
    f.env.sizes.insert(PathBuf::from("/media/other.mp4"), 2);
    f.ok(req(
        "media.relink",
        json!({"media": "m1", "path": "/media/other.mp4"}),
    ));
    let g = f.ok(req("media.get", json!({"media": "m1"})));
    assert!(
        g.result.get("proxy").is_none(),
        "relink must not keep the old proxy"
    );
    assert_eq!(g.result["path"], "/media/other.mp4");
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
    assert_eq!(r.result["proxies"][0]["path"], proxy_path("m1"));
    assert_eq!(f.editor.project().rev, rev);
    assert!(f.editor.project().media[0].proxy.is_none());
}

#[test]
fn a_project_without_proxy_fields_still_loads() {
    let f = Fixture::new(&[(1.0, false)]);
    let mut v = serde_json::to_value(f.editor.project()).unwrap();
    v["settings"].as_object_mut().unwrap().remove("use_proxies");
    v["media"][0].as_object_mut().unwrap().remove("proxy");
    let p: Project = serde_json::from_value(v).unwrap();
    assert!(!p.settings.use_proxies);
    assert!(p.media[0].proxy.is_none());
    let s = Settings::default();
    assert!(!s.use_proxies);
}

#[test]
fn proxy_size_keeps_aspect_and_does_not_enlarge() {
    use crate::model::proxy_size;
    assert_eq!(proxy_size(1920, 1080, 960), (960, 540));
    assert_eq!(proxy_size(320, 180, 960), (320, 180));
    assert_eq!(proxy_size(1080, 1920, 960), (540, 960));
}

/// Decision D41: `auto` takes what the rules pick and has no fresh proxy,
/// and finding nothing is fine.
#[test]
fn auto_generates_only_what_the_rules_pick() {
    let mut f = Fixture::new(&[(4.0, true)]);
    f.env = std::mem::take(&mut f.env).with_sequence("/renders", "frame_%04d.png", 1, 10);
    f.ok(req(
        "media.add",
        json!({"paths": ["/renders/frame_0001.png"], "sequence": true}),
    ));
    // m1 is a 1080p video of a small file: the GPU plays it. m2 is a sequence.
    let r = f.ok(req("proxy.generate", json!({"auto": true})));
    let made: Vec<_> = r.result["proxies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["media"].clone())
        .collect();
    assert_eq!(made, [json!("m2")]);
    let again = f.ok(req("proxy.generate", json!({"auto": true})));
    assert_eq!(again.result["proxies"], json!([]));
    assert!(again.changes.is_empty());
    // Limited to the media named; the preferences' size is the default.
    f.env.proxy_rules.larger_than = [1280, 720];
    f.env.proxy_rules.size = 640;
    let r = f.ok(req(
        "proxy.generate",
        json!({"auto": true, "media": ["m1"]}),
    ));
    assert_eq!(r.result["proxies"][0]["media"], "m1");
    assert_eq!(r.result["proxies"][0]["width"], 640);
}
