use serde_json::json;

use crate::looks::{
    LookKind, Styled, SubtitleLook, TitleLook, checked_name, delete, find, read, save,
};
use crate::model::{SubtitleStyle, TitleKind, TitleStyle};
use crate::test_support::{FakeEnv, Fixture};

fn yellow() -> Styled {
    Styled::Subtitles(SubtitleLook {
        chars: Some(30),
        lines: Some(2),
        style: SubtitleStyle {
            color: Some("#ffff00".into()),
            ..SubtitleStyle::default()
        },
    })
}

fn boxed() -> TitleLook {
    TitleLook {
        kind: TitleKind::LowerThird,
        style: TitleStyle {
            size: Some(0.04),
            ..TitleStyle::default()
        },
        ..TitleLook::default()
    }
}

#[test]
fn a_saved_look_reads_back_the_same() {
    let dir = tempfile::tempdir().unwrap();
    save(dir.path(), "Big Yellow", &yellow(), false, false).unwrap();
    let listing = read(dir.path(), LookKind::Subtitles);
    assert_eq!(
        listing
            .looks
            .iter()
            .map(|l| (l.name.as_str(), &l.styled))
            .collect::<Vec<_>>(),
        vec![("Big Yellow", &yellow())]
    );
}

#[test]
fn a_built_in_look_is_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let e = save(dir.path(), "pop", &yellow(), true, false).unwrap_err();
    assert_eq!(
        e.message,
        "pop is a built-in look and can't be changed; save it under another name"
    );
}

#[test]
fn a_built_in_look_is_not_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let e = delete(dir.path(), LookKind::Titles, "Headline", false).unwrap_err();
    assert_eq!(
        e.message,
        "Headline is a built-in look and can't be deleted"
    );
}

#[test]
fn saving_over_your_look_needs_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    save(dir.path(), "Mine", &yellow(), false, false).unwrap();
    let e = save(dir.path(), "mine", &yellow(), false, false).unwrap_err();
    assert_eq!(
        e.message,
        "you have a look called mine; pass overwrite to replace it"
    );
}

#[test]
fn a_name_in_another_case_replaces_the_file() {
    let dir = tempfile::tempdir().unwrap();
    save(dir.path(), "mine", &yellow(), false, false).unwrap();
    save(dir.path(), "Mine", &yellow(), true, false).unwrap();
    let names: Vec<String> = read(dir.path(), LookKind::Subtitles)
        .looks
        .into_iter()
        .map(|l| l.name)
        .collect();
    assert_eq!(names, vec!["Mine".to_string()]);
}

#[test]
fn a_bad_look_file_is_refused_with_its_reason() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("titles");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Huge.toml"), "[style]\nsize = 0.9\n").unwrap();
    let listing = read(dir.path(), LookKind::Titles);
    assert_eq!(
        listing
            .refused
            .iter()
            .map(|r| (r.name.as_str(), r.why.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            "Huge",
            "a title size is a fraction of the picture height, above 0 and at most 1/2"
        )]
    );
}

#[test]
fn a_look_name_with_a_slash_is_refused() {
    assert!(checked_name("a/b").is_err());
}

#[test]
fn look_names_match_whatever_their_case() {
    let found = find(&FakeEnv::default(), LookKind::Subtitles, "HIGHLIGHT").unwrap();
    let Styled::Subtitles(l) = found else {
        panic!("a subtitle look");
    };
    assert_eq!(l.chars, Some(32));
}

#[test]
fn a_look_sets_a_subtitle_tracks_style() {
    let dir = tempfile::tempdir().unwrap();
    save(dir.path(), "Big Yellow", &yellow(), false, false).unwrap();
    let mut f = Fixture::new(&[]);
    f.env.looks = Some(dir.path().to_path_buf());
    f.ok(json!({"method": "track.add", "params": {"kind": "subtitle"}}));
    f.ok(json!({"method": "track.set", "params": {"track": "S1", "look": "big yellow"}}));
    assert_eq!(
        f.editor.project().subtitles[0].style.color.as_deref(),
        Some("#ffff00")
    );
}

#[test]
fn a_title_look_keeps_the_titles_text() {
    let dir = tempfile::tempdir().unwrap();
    save(dir.path(), "Boxed", &Styled::Titles(boxed()), false, false).unwrap();
    let mut f = Fixture::new(&[]);
    f.env.looks = Some(dir.path().to_path_buf());
    let r = f.ok(json!({"method": "titles.add", "params": {"text": "Hello", "at": 0}}));
    let clip = r.result["clip"].as_str().unwrap().to_string();
    f.ok(json!({"method": "titles.set", "params": {"clip": clip, "look": "Boxed"}}));
    let t = f
        .editor
        .project()
        .clip(&clip)
        .unwrap()
        .title
        .clone()
        .unwrap();
    assert_eq!((t.text.as_str(), t.kind), ("Hello", TitleKind::LowerThird));
}

#[test]
fn fields_given_go_on_top_of_a_title_look() {
    let mut f = Fixture::new(&[]);
    let r = f.ok(json!({"method": "titles.add", "params": {
        "text": "Hi", "at": 0, "look": "Callout", "style": {"color": "red"}
    }}));
    let clip = r.result["clip"].as_str().unwrap();
    let t = f
        .editor
        .project()
        .clip(clip)
        .unwrap()
        .title
        .clone()
        .unwrap();
    assert_eq!(t.style.color.as_deref(), Some("#ff0000"));
}
