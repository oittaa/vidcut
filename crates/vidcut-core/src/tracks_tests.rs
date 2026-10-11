//! Multi-track editing tests (M6). Each cites the manual page or decision it
//! checks. Media: m1 = 250 frames with audio, m2 = 100 frames with audio,
//! m3 = 200 frames without audio (25 fps).

use serde_json::json;

use crate::error::ErrorCode;
use crate::test_support::{Fixture, row};

fn fixture() -> Fixture {
    Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)])
}

/// c1 = m1 0..100 at 0, c2 = m2 10..60 at 100, c3 = m1 100..150 at 150.
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

/// Overwrites `media` frames `a..b` onto `track` at `at`; returns the clip id.
fn put(f: &mut Fixture, media: &str, a: i64, b: i64, at: i64, track: &str) -> String {
    let r = f.ok(req(
        "edit.overwrite",
        json!({"media": media, "range": format!("{a}f..{b}f"), "at": at, "track": track}),
    ));
    r.result["clip"].as_str().unwrap().to_string()
}

#[test]
fn tracks_are_added_removed_and_renumbered() {
    // p. 648: Add Track; ids are positional, so removing renumbers.
    let mut f = fixture();
    let r = f.ok(req("track.add", json!({"kind": "video", "count": 2})));
    assert_eq!(r.result["tracks"], json!(["V2", "V3"]));
    f.ok(req("track.add", json!({"kind": "audio"})));
    assert_eq!(f.track_ids(), ["V1", "V2", "V3", "A1"]);
    put(&mut f, "m3", 0, 10, 0, "V2");
    let e = f.err(req("track.remove", json!({"track": "V2"})));
    assert_eq!(
        e.code,
        ErrorCode::InvalidEdit,
        "a track with clips needs force"
    );
    f.err(req("track.remove", json!({"track": "V1", "force": true})));
    f.ok(req("track.remove", json!({"track": "V2", "force": true})));
    assert_eq!(f.track_ids(), ["V1", "V2", "A1"]);
    // The mixer (p. 650): solo, fader, name.
    f.ok(req(
        "track.set",
        json!({"track": "A1", "solo": true, "volume_db": -6.0, "name": "Music"}),
    ));
    let t = f.editor.project().track("A1").unwrap().clone();
    assert!(t.solo && t.volume_db == -6.0 && t.name.as_deref() == Some("Music"));
    f.err(req("track.set", json!({"track": "A1", "volume_db": 30.0})));
    f.err(req(
        "track.set",
        json!({"track": "A1", "video_enabled": false}),
    ));
}

#[test]
fn edit_functions_use_the_target_track() {
    // p. 696 step 3: pick the track; p. 697: only Track 1 is pushed by an
    // insert.
    let mut f = three_clips();
    // The next new track is made on the way.
    put(&mut f, "m3", 0, 20, 90, "V2");
    assert_eq!(f.track("V2"), vec![row("c4", 90, 20, 0)]);
    assert_eq!(f.v1().len(), 3);
    f.ok(req(
        "edit.smart_insert",
        json!({"media": "m3", "range": "50f..60f", "playhead": 104, "track": "V2"}),
    ));
    assert_eq!(
        f.track("V2"),
        vec![row("c4", 90, 10, 0), row("c5", 100, 10, 50)]
    );
    assert_eq!(f.editor.project().end(), 200, "Track 1 didn't move");
    // Ripple Overwrite on V2 replaces the clip there without moving others.
    f.ok(req(
        "edit.ripple_overwrite",
        json!({"media": "m3", "range": "0f..30f", "at": 95, "track": "V2"}),
    ));
    assert_eq!(f.track("V2")[0], row("c7", 90, 30, 0));
    assert_eq!(f.v1()[2], row("c3", 150, 50, 100));
    let e = f.err(req("edit.append", json!({"media": "m3", "track": "V4"})));
    assert_eq!(e.code, ErrorCode::NotFound);
}

#[test]
fn audio_only_clips_go_on_audio_tracks() {
    // p. 648: music and narration go on audio-only tracks; p. 694: Audio Only.
    let mut f = three_clips();
    let music = f.add_music("music.wav", 20.0);
    let r = f.ok(req(
        "edit.append",
        json!({"media": music, "range": "0f..300f"}),
    ));
    assert_eq!(f.track_ids(), ["V1", "A1"]);
    let c = f
        .editor
        .project()
        .clip(r.result["clip"].as_str().unwrap())
        .unwrap()
        .clone();
    assert!(!c.video && c.audio.is_some());
    assert_eq!((c.start, c.duration), (0, 300));
    // Audio Only takes just the audio of a video clip, onto V2 when asked.
    let id = {
        let r = f.ok(req(
            "edit.overwrite",
            json!({"media": "m1", "range": "0f..50f", "at": 20, "audio_only": true, "track": "V2"}),
        ));
        r.result["clip"].as_str().unwrap().to_string()
    };
    assert!(!f.editor.project().clip(&id).unwrap().video);
    // A video clip put on an audio track brings only its audio.
    let id = put(&mut f, "m1", 0, 50, 300, "A1");
    assert!(!f.editor.project().clip(&id).unwrap().video);
    f.err(req(
        "edit.overwrite",
        json!({"media": "m3", "range": "0f..50f", "at": 400, "track": "A1"}),
    ));
    f.err(req(
        "edit.append",
        json!({"media": "m3", "audio_only": true}),
    ));
}

#[test]
fn clips_on_other_tracks_follow_track_one() {
    // p. 649: superimposed clips stay in sync with Track 1 when it ripples;
    // p. 723: they travel with Track 1 clips that move; p. 727: trimming the
    // picture under one away deletes it. Decision D21.
    let mut f = three_clips();
    put(&mut f, "m3", 0, 20, 110, "V2"); // c4 over c2
    let music = f.add_music("music.wav", 20.0);
    f.ok(req(
        "edit.overwrite",
        json!({"media": music, "range": "0f..40f", "at": 160}),
    )); // c5 on A1, over c3
    f.ok(req("edit.delete", json!({"clips": "c1"})));
    assert_eq!(f.track("V2"), vec![row("c4", 10, 20, 0)]);
    assert_eq!(f.track("A1"), vec![row("c5", 60, 40, 0)]);
    f.ok(req(
        "edit.insert",
        json!({"media": "m2", "range": "0f..25f", "at": 0}),
    )); // c6 0..25; c2 25..75; c3 75..125
    assert_eq!(f.track("V2")[0].1, 35);
    assert_eq!(f.track("A1")[0].1, 85);
    // c2 to the end: its B-roll goes along, the music stays with c3.
    f.ok(req("edit.move", json!({"clips": "c2", "to": 125})));
    assert_eq!(
        f.v1(),
        vec![
            row("c6", 0, 25, 0),
            row("c3", 25, 50, 100),
            row("c2", 75, 50, 10)
        ]
    );
    assert_eq!(f.track("V2")[0].1, 85);
    assert_eq!(f.track("A1")[0].1, 35);
    // A ripple trim of c2's Out to before the B-roll deletes it.
    f.ok(req(
        "edit.trim",
        json!({"clip": "c2", "edge": "tail", "to": 80}),
    ));
    assert!(f.track("V2").is_empty());
    // A lift doesn't move anything.
    let a1 = f.track("A1");
    f.ok(req("edit.delete", json!({"clips": "c6", "lift": true})));
    assert_eq!(f.track("A1"), a1);
}

#[test]
fn trimming_a_head_moves_what_started_over_it() {
    // Decision D21: a clip whose first frame's picture is trimmed off the
    // head moves to the clip's new start.
    let mut f = three_clips();
    put(&mut f, "m3", 0, 20, 105, "V2"); // 5 frames into c2
    f.ok(req(
        "edit.trim",
        json!({"clip": "c2", "edge": "head", "by": 10}),
    )); // c2 keeps its place, its first 10 frames go
    assert_eq!(f.v1()[1], row("c2", 100, 40, 20));
    assert_eq!(f.track("V2"), vec![row("c4", 100, 20, 0)]);
    // Clips over later Track 1 clips move with them as c2 gets shorter.
    put(&mut f, "m3", 0, 10, 140, "V2"); // over c3's first frame
    let before = f.track("V2")[1].clone();
    f.ok(req(
        "edit.trim",
        json!({"clip": "c2", "edge": "head", "by": 5}),
    ));
    assert_eq!(f.track("V2")[1].1, before.1 - 5);
}

#[test]
fn deleting_a_range_takes_it_out_of_every_track() {
    // p. 725: Delete with In/Out set removes that part of the timeline.
    let mut f = three_clips();
    put(&mut f, "m3", 0, 40, 40, "V2"); // c4 40..80
    put(&mut f, "m3", 0, 20, 170, "V2"); // c5 170..190
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
fn other_tracks_ripple_only_where_track_one_is_empty() {
    // p. 721: ripple on other tracks only without Track 1 video under them.
    let mut f = three_clips(); // Track 1 ends at 200
    let music = f.add_music("music.wav", 20.0);
    for at in [0, 220, 260] {
        f.ok(req(
            "edit.overwrite",
            json!({"media": music, "range": "0f..30f", "at": at}),
        ));
    }
    let ids: Vec<String> = f.track("A1").into_iter().map(|c| c.0).collect();
    f.ok(req("edit.delete", json!({"clips": ids[0]})));
    assert_eq!(f.track("A1")[0].1, 220, "over Track 1: the gap stays");
    f.ok(req("edit.delete", json!({"clips": ids[1]})));
    assert_eq!(
        f.track("A1"),
        vec![row(&ids[2], 230, 30, 0)],
        "past Track 1: it closes"
    );
    f.ok(req(
        "edit.trim",
        json!({"clip": ids[2], "edge": "head", "by": 10}),
    ));
    assert_eq!(f.track("A1"), vec![row(&ids[2], 230, 20, 10)]);
}

#[test]
fn video_tracks_ripple_only_where_track_one_is_empty() {
    // p. 721, decision D22, on V2: over Track 1's picture a delete or trim
    // leaves a gap; past Track 1's end the clips after it close up.
    let mut f = three_clips(); // Track 1 ends at 200
    let a = put(&mut f, "m3", 0, 20, 20, "V2");
    let b = put(&mut f, "m3", 20, 40, 60, "V2");
    let c = put(&mut f, "m3", 40, 60, 210, "V2");
    let d = put(&mut f, "m3", 60, 80, 240, "V2");
    f.ok(req("edit.delete", json!({"clips": a})));
    assert_eq!(
        f.track("V2")[0],
        row(&b, 60, 20, 20),
        "over Track 1: the gap stays"
    );
    f.ok(req("edit.delete", json!({"clips": c})));
    assert_eq!(
        f.track("V2"),
        vec![row(&b, 60, 20, 20), row(&d, 220, 20, 60)],
        "past Track 1: it closes"
    );
    f.ok(req(
        "edit.trim",
        json!({"clip": b, "edge": "head", "by": 5}),
    ));
    f.ok(req(
        "edit.trim",
        json!({"clip": d, "edge": "head", "by": 5}),
    ));
    assert_eq!(
        f.track("V2"),
        vec![row(&b, 65, 15, 25), row(&d, 220, 15, 65)],
        "a head trim leaves a gap over Track 1, and ripples past it"
    );

    // A hole in Track 1 isn't its end: with a picture after the hole, a V2
    // clip deleted in the hole leaves its gap too.
    let mut f = three_clips();
    f.ok(req("edit.delete", json!({"clips": "c2", "lift": true}))); // hole 100..150
    let a = put(&mut f, "m3", 0, 20, 110, "V2");
    let b = put(&mut f, "m3", 20, 40, 160, "V2");
    f.ok(req("edit.delete", json!({"clips": a})));
    assert_eq!(f.track("V2"), vec![row(&b, 160, 20, 20)], "the gap stays");
}

#[test]
fn place_on_top_goes_above_everything_there() {
    // pp. 699–700: Place On Top.
    let mut f = three_clips();
    let r = f.ok(req(
        "edit.place_on_top",
        json!({"media": "m3", "range": "0f..20f", "at": 30}),
    ));
    assert_eq!(r.result["track"], "V2");
    assert_eq!(f.track("V2"), vec![row("c4", 30, 20, 0)]);
    let r = f.ok(req(
        "edit.place_on_top",
        json!({"media": "m3", "range": "0f..20f", "at": 40}),
    ));
    assert_eq!(r.result["track"], "V3", "made for it");
    // Within 5 frames of an edit (the Smart Indicator) it lines up with it.
    let r = f.ok(req(
        "edit.place_on_top",
        json!({"media": "m3", "range": "0f..10f", "at": 103}),
    ));
    assert_eq!(r.result["track"], "V2");
    assert_eq!(f.track("V2")[1], row("c6", 100, 10, 0));
    // A timeline In wins over the playhead; a lone Out backtimes.
    f.ok(req("marks.set", json!({"in": 160})));
    let r = f.ok(req(
        "edit.place_on_top",
        json!({"media": "m3", "range": "0f..10f", "at": 0}),
    ));
    let id = r.result["clip"].as_str().unwrap();
    assert_eq!(f.editor.project().clip(id).unwrap().start, 160);
    f.ok(req("marks.clear", json!({})));
    f.ok(req("marks.set", json!({"out": 60})));
    let r = f.ok(req(
        "edit.place_on_top",
        json!({"media": "m3", "range": "0f..10f", "at": 0}),
    ));
    let id = r.result["clip"].as_str().unwrap();
    assert_eq!(f.editor.project().clip(id).unwrap().start, 50);
    // Audio-only media: the first audio track with room.
    f.ok(req("marks.clear", json!({})));
    let music = f.add_music("music.wav", 20.0);
    let r = f.ok(req(
        "edit.place_on_top",
        json!({"media": music, "range": "0f..50f", "at": 0}),
    ));
    assert_eq!(r.result["track"], "A1");
    // It picks its own track: naming one is a mistake, not a hint.
    let e = f.err(req(
        "edit.place_on_top",
        json!({"media": "m3", "range": "0f..10f", "at": 0, "track": "V2"}),
    ));
    assert_eq!(e.code, ErrorCode::Usage);
}

#[test]
fn following_clips_that_collide_move_up_a_track() {
    // Decision D21: a following clip never overwrites; it moves up.
    let mut f = three_clips();
    put(&mut f, "m3", 0, 20, 110, "V2"); // c4 over c2
    put(&mut f, "m3", 30, 50, 150, "V2"); // c5 over c3
    f.ok(req("edit.delete", json!({"clips": "c2"})));
    // c3 moved to 100 and c5 with it; c4 lost its clip and went to where
    // Track 1 closed up, also 100.
    assert_eq!(f.track("V2"), vec![row("c4", 100, 20, 0)]);
    assert_eq!(f.track("V3"), vec![row("c5", 100, 20, 30)]);
}

#[test]
fn split_cuts_every_unlocked_track_and_locked_tracks_stay() {
    // Decision D2: Split cuts every clip under the playhead on unlocked tracks.
    let mut f = three_clips();
    put(&mut f, "m3", 0, 100, 20, "V2"); // c4 20..120
    f.ok(req("edit.split", json!({"at": 50})));
    assert_eq!(f.v1().len(), 4);
    assert_eq!(f.track("V2").len(), 2);
    f.ok(req("track.set", json!({"track": "V2", "locked": true})));
    f.ok(req("edit.split", json!({"at": 30})));
    assert_eq!(f.track("V2").len(), 2, "locked: not split");
    let v2 = f.track("V2");
    f.ok(req("edit.delete", json!({"clips": "c1"})));
    assert_eq!(f.track("V2"), v2, "locked: doesn't follow");
    f.err(req("edit.delete", json!({"clips": v2[0].0})));
}

#[test]
fn paste_puts_clips_back_on_their_tracks() {
    // p. 723: paste goes to the tracks the clips were copied from.
    let mut f = three_clips();
    put(&mut f, "m3", 0, 20, 10, "V2"); // c4
    let r = f.ok(req("edit.copy", json!({"clips": ["c1", "c4"]})));
    assert_eq!(r.result["clips"][1]["track"], "V2");
    f.ok(req(
        "edit.paste",
        json!({"clips": r.result["clips"], "at": 150}),
    ));
    assert_eq!(f.track("V2")[1].1, 160);
    // Every clip says its track.
    let bare = json!([{"id": "c1", "media": "m2", "start": 0, "duration": 10,
                       "source_in": 0, "video": true, "enabled": true}]);
    let e = f.err(req("edit.paste", json!({"clips": bare, "at": 0})));
    assert_eq!(e.code, ErrorCode::Usage);
}

#[test]
fn clips_move_between_tracks() {
    // p. 721: a Track 1 clip dropped on another track overwrites there, and
    // Track 1 closes up.
    let mut f = three_clips();
    f.ok(req(
        "edit.move",
        json!({"clips": "c2", "to": 120, "track": "V2", "mode": "overwrite"}),
    ));
    assert_eq!(f.v1(), vec![row("c1", 0, 100, 0), row("c3", 100, 50, 100)]);
    assert_eq!(f.track("V2"), vec![row("c2", 120, 50, 10)]);
    // And back into a Track 1 edit point.
    f.ok(req(
        "edit.move",
        json!({"clips": "c2", "to": 100, "track": "V1"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c2", 100, 50, 10),
            row("c3", 150, 50, 100)
        ]
    );
    assert!(f.track("V2").is_empty());
    f.ok(req("track.add", json!({"kind": "audio"})));
    f.err(req(
        "edit.move",
        json!({"clips": "c1", "to": 0, "track": "A1"}),
    ));
}

#[test]
fn bins_hold_media() {
    // p. 655: bins in the Media Pool. Removing a bin keeps its media.
    let mut f = fixture();
    f.ok(req("bins.add", json!({"path": "Interviews/Day 1"})));
    assert_eq!(f.editor.project().bins, ["Interviews", "Interviews/Day 1"]);
    f.ok(req(
        "media.move",
        json!({"media": ["m1", "m2"], "bin": "Interviews/Day 1"}),
    ));
    f.ok(req(
        "bins.rename",
        json!({"path": "Interviews", "to": "Talks"}),
    ));
    let p = f.editor.project();
    assert_eq!(p.bins, ["Talks", "Talks/Day 1"]);
    assert_eq!(p.media("m1").unwrap().bin, "Talks/Day 1");
    f.ok(req("bins.remove", json!({"path": "Talks/Day 1"})));
    assert_eq!(f.editor.project().media("m1").unwrap().bin, "Talks");
    f.err(req("media.move", json!({"media": "m1", "bin": "Nope"})));
    f.err(req("bins.add", json!({"path": "a//b"})));
    f.err(req(
        "bins.rename",
        json!({"path": "Talks", "to": "Talks/Sub"}),
    ));
}
