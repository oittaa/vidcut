use std::path::PathBuf;

use serde_json::{Value, json};

use crate::editor::{Editor, History};
use crate::error::ErrorCode;
use crate::model::{ImageSequence, Project, Settings};
use crate::test_support::{Fixture, media_info};
use crate::time::Rational;

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn fps(n: i64) -> Rational {
    Rational::new(n, 1).unwrap()
}

fn shoot(files: &[&str], others: &[&str]) -> Fixture {
    let mut f = Fixture::new(&[]);
    for path in files {
        f.env = std::mem::take(&mut f.env).with(path, media_info(4.0, fps(25), true));
    }
    f.env.others = others.iter().map(PathBuf::from).collect();
    f
}

fn bins_of_media(f: &Fixture) -> Vec<(String, String)> {
    f.editor
        .project()
        .media
        .iter()
        .map(|m| (m.name.clone(), m.bin.clone()))
        .collect()
}

#[test]
fn a_folder_comes_in_as_a_bin_with_a_bin_for_each_folder_in_it() {
    let mut f = shoot(&["/shoot/b.mp4", "/shoot/Day 1/c.mp4", "/shoot/a.mp4"], &[]);
    let r = f.ok(req("media.add_folder", json!({"path": "/shoot"})));
    assert_eq!(r.result["bin"], "shoot");
    assert_eq!(r.result["media"], json!(["m1", "m2", "m3"]));
    assert_eq!(f.editor.project().bins, ["shoot", "shoot/Day 1"]);
    assert_eq!(
        bins_of_media(&f),
        [
            ("a.mp4".into(), "shoot".into()),
            ("b.mp4".into(), "shoot".into()),
            ("c.mp4".into(), "shoot/Day 1".into()),
        ]
    );
}

#[test]
fn files_that_are_not_media_are_skipped_and_listed() {
    let mut f = shoot(
        &["/shoot/a.mp4"],
        &["/shoot/notes.txt", "/shoot/broken.mp4"],
    );
    let r = f.ok(req("media.add_folder", json!({"path": "/shoot"})));
    assert_eq!(r.result["media"], json!(["m1"]));
    assert_eq!(
        r.result["skipped"],
        json!([
            {"path": PathBuf::from("/shoot/broken.mp4"), "why": "cannot open /shoot/broken.mp4"},
            {"path": PathBuf::from("/shoot/notes.txt"), "why": "not a media file"},
        ])
    );
}

#[test]
fn the_folder_bin_goes_into_the_bin_given() {
    let mut f = shoot(&["/shoot/a.mp4"], &[]);
    f.ok(req("bins.add", json!({"path": "Projects"})));
    let r = f.ok(req(
        "media.add_folder",
        json!({"path": "/shoot", "bin": "Projects"}),
    ));
    assert_eq!(r.result["bin"], "Projects/shoot");
    assert_eq!(f.editor.project().media[0].bin, "Projects/shoot");
}

#[test]
fn the_bin_given_must_exist() {
    let mut f = shoot(&["/shoot/a.mp4"], &[]);
    let e = f.err(req(
        "media.add_folder",
        json!({"path": "/shoot", "bin": "Projects"}),
    ));
    assert_eq!(e.code, ErrorCode::NotFound, "{e}");
}

#[test]
fn a_folder_without_any_media_is_an_error() {
    let mut f = shoot(&[], &["/docs/notes.txt"]);
    let e = f.err(req("media.add_folder", json!({"path": "/docs"})));
    assert_eq!(e.code, ErrorCode::Media);
    assert_eq!(
        e.message,
        format!(
            "no media in {} or the folders in it (skipped notes.txt: not a media file)",
            PathBuf::from("/docs").display()
        )
    );
}

#[test]
fn importing_a_folder_again_adds_only_its_new_files() {
    let mut f = shoot(&["/shoot/a.mp4"], &[]);
    f.ok(req("media.add_folder", json!({"path": "/shoot"})));
    f.env = std::mem::take(&mut f.env).with("/shoot/b.mp4", media_info(2.0, fps(25), true));
    let r = f.ok(req("media.add_folder", json!({"path": "/shoot"})));
    assert_eq!(r.result["media"], json!(["m1", "m2"]));
    assert_eq!(r.changes.created, ["m2"]);
}

#[test]
fn media_already_in_the_pool_move_into_the_folder_bin() {
    let mut f = shoot(&["/shoot/a.mp4"], &[]);
    f.ok(req("media.add", json!({"paths": ["/shoot/a.mp4"]})));
    f.ok(req("media.add_folder", json!({"path": "/shoot"})));
    assert_eq!(bins_of_media(&f), [("a.mp4".into(), "shoot".into())]);
}

#[test]
fn the_first_video_by_name_sets_the_format_whatever_its_case() {
    let mut f = shoot(&[], &[]);
    f.editor = Editor::new(Project::new(Settings::default()), History::default());
    f.env = std::mem::take(&mut f.env)
        .with("/shoot/B.mp4", media_info(4.0, fps(24), true))
        .with("/shoot/a.mp4", media_info(4.0, fps(50), true));
    f.ok(req("media.add_folder", json!({"path": "/shoot"})));
    assert_eq!(f.editor.project().settings.fps, fps(50));
}

#[test]
fn numbered_images_come_in_as_one_clip_with_sequence() {
    let mut f = shoot(&["/shoot/a.mp4"], &[]);
    let frames = ImageSequence {
        pattern: "img_%04d.png".into(),
        first: 1,
        count: 12,
    };
    f.env
        .sequences
        .insert(PathBuf::from("/shoot/img_0001.png"), (frames, 320, 180));
    let r = f.ok(req(
        "media.add_folder",
        json!({"path": "/shoot", "sequence": true, "fps": "24"}),
    ));
    assert_eq!(r.result["media"], json!(["m1", "m2"]));
    let m = &f.editor.project().media[1];
    assert_eq!(m.name, "img_[0001-0012].png");
    assert_eq!(m.info.video.as_ref().unwrap().fps, fps(24));
}

#[test]
fn the_whole_folder_is_one_undo_step() {
    let mut f = shoot(&["/shoot/a.mp4", "/shoot/Day 1/b.mp4"], &[]);
    f.ok(req("media.add_folder", json!({"path": "/shoot"})));
    f.ok(req("history.undo", json!({})));
    assert!(f.editor.project().media.is_empty());
    assert!(f.editor.project().bins.is_empty());
}
