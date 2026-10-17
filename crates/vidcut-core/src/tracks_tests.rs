use serde_json::json;

use crate::test_support::{Fixture, row};

fn fixture() -> Fixture {
    Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)])
}

fn three_clips() -> Fixture {
    let mut f = fixture();
    f.append("m1", 0, 100);
    f.append("m2", 10, 60);
    f.append("m1", 100, 150);
    f
}

fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({ "method": method, "params": params })
}

fn put(f: &mut Fixture, media: &str, a: i64, b: i64, at: i64, track: &str) -> String {
    let r = f.ok(req(
        "edit.overwrite",
        json!({"media": media, "range": format!("{a}f..{b}f"), "at": at, "track": track}),
    ));
    r.result["clip"].as_str().unwrap().to_string()
}

#[test]
fn deleting_a_range_takes_it_out_of_every_track() {
    let mut f = three_clips();
    put(&mut f, "m3", 0, 40, 40, "V2");
    put(&mut f, "m3", 0, 20, 170, "V2");
    f.ok(req("edit.delete_range", json!({"range": "50f..70f"})));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 50, 0),
            row("c7", 50, 30, 70),
            row("c2", 80, 50, 10),
            row("c3", 130, 50, 100)
        ]
    );
    assert_eq!(
        f.track("V2"),
        vec![
            row("c4", 40, 10, 0),
            row("c9", 50, 10, 30),
            row("c5", 150, 20, 0)
        ]
    );
}

#[test]
fn following_clips_that_collide_move_up_a_track() {
    let mut f = three_clips();
    put(&mut f, "m3", 0, 20, 110, "V2");
    put(&mut f, "m3", 30, 50, 150, "V2");
    f.ok(req("edit.delete", json!({"clips": "c2"})));
    assert_eq!(f.track("V2"), vec![row("c4", 100, 20, 0)]);
    assert_eq!(f.track("V3"), vec![row("c5", 100, 20, 30)]);
}
