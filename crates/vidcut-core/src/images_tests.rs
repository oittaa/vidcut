use serde_json::json;

use crate::editor::{Editor, History};
use crate::error::ErrorCode;
use crate::model::{Project, Settings};
use crate::test_support::{Fixture, media_info, row, still_info};
use crate::time::Rational;

fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({ "method": method, "params": params })
}

fn with_still() -> Fixture {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.env = std::mem::take(&mut f.env).with("/media/photo.png", still_info(4000, 3000));
    f.ok(req("media.add", json!({"paths": ["/media/photo.png"]})));
    f
}

#[test]
fn the_still_duration_is_a_project_setting() {
    let mut f = with_still();
    f.ok(req("project.set", json!({"still_duration": "3s"})));
    f.ok(req("edit.append", json!({"media": "m2"})));
    assert_eq!(f.v1(), vec![row("c1", 0, 75, 0)]);
    let e = f.err(req("project.set", json!({"still_duration": "0f"})));
    assert_eq!(e.code, ErrorCode::InvalidEdit, "{e}");
}

#[test]
fn a_still_leaves_the_project_format_to_video() {
    let mut f = Fixture::new(&[]);
    f.editor = Editor::new(Project::new(Settings::default()), History::default());
    f.env = std::mem::take(&mut f.env)
        .with("/media/photo.png", still_info(4000, 3000))
        .with(
            "/media/cam.mp4",
            media_info(10.0, Rational::new(25, 1).unwrap(), true),
        );
    f.ok(req("media.add", json!({"paths": ["/media/photo.png"]})));
    let s = &f.editor.project().settings;
    assert!(s.auto_format);
    assert_eq!((s.width, s.fps), (1920, Rational::new(30, 1).unwrap()));
    f.ok(req("media.add", json!({"paths": ["/media/cam.mp4"]})));
    let s = &f.editor.project().settings;
    assert!(!s.auto_format);
    assert_eq!(s.fps, Rational::new(25, 1).unwrap());
}
