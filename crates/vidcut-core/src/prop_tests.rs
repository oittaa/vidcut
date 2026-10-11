//! Property tests: random edit sequences keep the project valid, failed
//! requests change nothing, undo restores the start, and with Ripple On the
//! magnetic V1 never gets gaps unless an edit explicitly asks for one.

use proptest::prelude::*;
use serde_json::{Value, json};

use crate::error::ErrorCode;
use crate::test_support::Fixture;
use crate::validate;

#[derive(Debug, Clone)]
enum Op {
    Append {
        media: u8,
        a: u16,
        len: u16,
    },
    SmartInsert {
        media: u8,
        a: u16,
        len: u16,
        playhead: u16,
    },
    Insert {
        media: u8,
        a: u16,
        len: u16,
        at: u16,
    },
    Overwrite {
        media: u8,
        a: u16,
        len: u16,
        at: u16,
    },
    RippleOverwrite {
        media: u8,
        a: u16,
        len: u16,
        at: u16,
    },
    Split {
        at: u16,
    },
    Delete {
        pick: u8,
        lift: bool,
    },
    DeleteRange {
        a: u16,
        len: u16,
        lift: bool,
    },
    CloseGap {
        at: u16,
    },
    Move {
        pick: u8,
        to: u16,
        overwrite: bool,
    },
    Replace {
        pick: u8,
        onto: u8,
    },
    Trim {
        pick: u8,
        head: bool,
        by: i16,
        ripple: Option<bool>,
    },
    Roll {
        pick: u8,
        head: bool,
        by: i16,
    },
    Slip {
        pick: u8,
        by: i16,
    },
    AudioRoll {
        pick: u8,
        head: bool,
        by: i16,
    },
    ChangeDuration {
        pick: u8,
        frames: u16,
    },
    Marks {
        a: u16,
        len: u16,
    },
    ClearMarks,
    MediaMarks {
        media: u8,
        a: u16,
        len: u16,
    },
    AppendMarked {
        media: u8,
    },
    Cut {
        pick: u8,
    },
    Paste {
        pick: u8,
        at: u16,
    },
    Undo,
    Redo,
}

impl Op {
    /// Whether the op may legitimately leave a gap on V1 even with Ripple On.
    fn may_gap(&self) -> bool {
        matches!(
            self,
            Op::Delete { lift: true, .. }
                | Op::DeleteRange { lift: true, .. }
                | Op::Trim {
                    ripple: Some(false),
                    ..
                }
        )
    }
}

fn op() -> impl Strategy<Value = Op> {
    let media = 0u8..3;
    prop_oneof![
        (media.clone(), 0u16..200, 1u16..80).prop_map(|(media, a, len)| Op::Append {
            media,
            a,
            len
        }),
        (media.clone(), 0u16..200, 1u16..80, 0u16..600).prop_map(|(media, a, len, playhead)| {
            Op::SmartInsert {
                media,
                a,
                len,
                playhead,
            }
        }),
        (media.clone(), 0u16..200, 1u16..80, 0u16..600)
            .prop_map(|(media, a, len, at)| Op::Insert { media, a, len, at }),
        (media.clone(), 0u16..200, 1u16..80, 0u16..600)
            .prop_map(|(media, a, len, at)| Op::Overwrite { media, a, len, at }),
        (media, 0u16..200, 1u16..80, 0u16..600)
            .prop_map(|(media, a, len, at)| Op::RippleOverwrite { media, a, len, at }),
        (0u16..600).prop_map(|at| Op::Split { at }),
        (any::<u8>(), any::<bool>()).prop_map(|(pick, lift)| Op::Delete { pick, lift }),
        (0u16..600, 1u16..100, any::<bool>()).prop_map(|(a, len, lift)| Op::DeleteRange {
            a,
            len,
            lift
        }),
        (0u16..600).prop_map(|at| Op::CloseGap { at }),
        (any::<u8>(), 0u16..600, any::<bool>()).prop_map(|(pick, to, overwrite)| Op::Move {
            pick,
            to,
            overwrite
        }),
        (any::<u8>(), any::<u8>()).prop_map(|(pick, onto)| Op::Replace { pick, onto }),
        (
            any::<u8>(),
            any::<bool>(),
            -60i16..60,
            prop::option::of(any::<bool>())
        )
            .prop_map(|(pick, head, by, ripple)| Op::Trim {
                pick,
                head,
                by,
                ripple
            }),
        (any::<u8>(), any::<bool>(), -30i16..30).prop_map(|(pick, head, by)| Op::Roll {
            pick,
            head,
            by
        }),
        (any::<u8>(), -40i16..40).prop_map(|(pick, by)| Op::Slip { pick, by }),
        (any::<u8>(), any::<bool>(), -20i16..20).prop_map(|(pick, head, by)| Op::AudioRoll {
            pick,
            head,
            by
        }),
        (any::<u8>(), 1u16..120).prop_map(|(pick, frames)| Op::ChangeDuration { pick, frames }),
        (0u16..600, 1u16..100).prop_map(|(a, len)| Op::Marks { a, len }),
        Just(Op::ClearMarks),
        (0u8..3, 0u16..200, 1u16..80).prop_map(|(media, a, len)| Op::MediaMarks { media, a, len }),
        (0u8..3).prop_map(|media| Op::AppendMarked { media }),
        any::<u8>().prop_map(|pick| Op::Cut { pick }),
        (any::<u8>(), 0u16..600).prop_map(|(pick, at)| Op::Paste { pick, at }),
        Just(Op::Undo),
        Just(Op::Redo),
    ]
}

fn pick(f: &Fixture, i: u8) -> Option<String> {
    let clips = f.v1();
    (!clips.is_empty()).then(|| clips[i as usize % clips.len()].0.clone())
}

fn request(f: &Fixture, op: &Op) -> Option<Value> {
    let m = |i: &u8| format!("m{}", i + 1);
    let src = |media: &u8, a: &u16, len: &u16| json!({"media": m(media), "range": format!("{a}f..+{len}f")});
    let with = |mut base: Value, key: &str, v: Value| {
        base[key] = v;
        base
    };
    let edge = |head: &bool| if *head { "head" } else { "tail" };
    let (method, params) = match op {
        Op::Append { media, a, len } => ("edit.append", src(media, a, len)),
        Op::SmartInsert {
            media,
            a,
            len,
            playhead,
        } => (
            "edit.smart_insert",
            with(src(media, a, len), "playhead", json!(playhead)),
        ),
        Op::Insert { media, a, len, at } => {
            ("edit.insert", with(src(media, a, len), "at", json!(at)))
        }
        Op::Overwrite { media, a, len, at } => {
            ("edit.overwrite", with(src(media, a, len), "at", json!(at)))
        }
        Op::RippleOverwrite { media, a, len, at } => (
            "edit.ripple_overwrite",
            with(src(media, a, len), "at", json!(at)),
        ),
        Op::Split { at } => ("edit.split", json!({"at": at})),
        Op::Delete { pick: i, lift } => {
            ("edit.delete", json!({"clips": pick(f, *i)?, "lift": lift}))
        }
        Op::DeleteRange { a, len, lift } => (
            "edit.delete_range",
            json!({"range": format!("{a}f..+{len}f"), "lift": lift}),
        ),
        Op::CloseGap { at } => ("edit.close_gap", json!({"at": at})),
        Op::Move {
            pick: i,
            to,
            overwrite,
        } => (
            "edit.move",
            json!({"clips": pick(f, *i)?, "to": to, "mode": if *overwrite { "overwrite" } else { "insert" }}),
        ),
        Op::Replace { pick: i, onto } => (
            "edit.replace",
            json!({"clip": pick(f, *i)?, "onto": pick(f, *onto)?}),
        ),
        Op::Trim {
            pick: i,
            head,
            by,
            ripple,
        } => {
            let mut p = json!({"clip": pick(f, *i)?, "edge": edge(head), "by": by});
            if let Some(r) = ripple {
                p["ripple"] = json!(r);
            }
            ("edit.trim", p)
        }
        Op::Roll { pick: i, head, by } => (
            "edit.roll",
            json!({"clip": pick(f, *i)?, "edge": edge(head), "by": by}),
        ),
        Op::Slip { pick: i, by } => ("edit.slip", json!({"clip": pick(f, *i)?, "by": by})),
        Op::AudioRoll { pick: i, head, by } => (
            "edit.audio_roll",
            json!({"clip": pick(f, *i)?, "edge": edge(head), "by": by}),
        ),
        Op::ChangeDuration { pick: i, frames } => (
            "edit.change_duration",
            json!({"clips": pick(f, *i)?, "duration": frames}),
        ),
        Op::Marks { a, len } => ("marks.set", json!({"in": a, "out": a + len})),
        Op::ClearMarks => ("marks.clear", json!({})),
        Op::MediaMarks { media, a, len } => (
            "media.marks.set",
            json!({"media": m(media), "in": a, "out": a + len}),
        ),
        Op::AppendMarked { media } => ("edit.append", json!({"media": m(media)})),
        Op::Cut { pick: i } => ("edit.cut", json!({"clips": pick(f, *i)?})),
        Op::Paste { pick: i, at } => {
            let clip = f.editor.project().clip(&pick(f, *i)?)?.clone();
            ("edit.paste", json!({"clips": [clip], "at": at}))
        }
        Op::Undo => ("history.undo", json!({})),
        Op::Redo => ("history.redo", json!({})),
    };
    Some(json!({"method": method, "params": params}))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn random_edits_keep_the_project_consistent(ops in prop::collection::vec(op(), 1..40)) {
        // m1: 10 s with audio, m2: 4 s with audio, m3: 8 s without audio (25 fps).
        let mut f = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
        let start = f.editor.project().clone();
        let mut gapless_expected = true;
        for op in &ops {
            let Some(req) = request(&f, op) else { continue };
            let before = f.editor.project().clone();
            match f.try_req(req.clone()) {
                Ok(_) => {
                    prop_assert!(validate::violations(f.editor.project()).is_empty());
                    if op.may_gap() {
                        gapless_expected = false;
                    }
                }
                Err(e) => {
                    prop_assert_ne!(e.code, ErrorCode::Internal, "{} -> {}", req, e.message);
                    prop_assert_eq!(f.editor.project(), &before, "failed request changed the project: {}", req);
                }
            }
            if gapless_expected && !matches!(op, Op::Undo | Op::Redo) {
                prop_assert!(validate::v1_gapless(f.editor.project()), "gap after {}: {:?}", req, f.v1());
            }
        }
        // Undoing everything returns to the start (revisions keep growing).
        let steps = f.editor.history().undo.len();
        if steps > 0 {
            f.try_req(json!({"method": "history.undo", "params": {"steps": steps}})).unwrap();
        }
        let p = f.editor.project();
        prop_assert_eq!(&p.tracks, &start.tracks);
        prop_assert_eq!(&p.marks, &start.marks);
    }
}
