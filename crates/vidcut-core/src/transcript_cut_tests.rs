use serde_json::json;

use crate::test_support::{Fixture, row};
use crate::time::Rational;
use crate::transcript::{Said, Speech, Timing, Word, spoken, word_frames};

fn speech(model: &str, timing: Timing, said: Vec<Said>) -> Speech {
    Speech {
        model: model.into(),
        device: "CPU".into(),
        language: Some("en".into()),
        timing,
        pieces: 1,
        longest_piece: 10.0,
        said,
        warnings: Vec::new(),
    }
}

fn word(text: &str, start: f64, end: f64) -> Word {
    Word {
        start,
        end,
        text: text.into(),
        joined: false,
    }
}

#[test]
fn only_a_models_own_word_times_are_kept() {
    let heard = speech(
        "Parakeet Ultra",
        Timing::Words,
        vec![
            Said {
                start: 0.0,
                end: 2.0,
                text: "hello there".into(),
                words: vec![word("hello", 1.0, 1.4)],
                speech: vec![(0.0, 2.0)],
            },
            Said {
                start: 2.0,
                end: 4.0,
                text: "guessed words".into(),
                words: Vec::new(),
                speech: vec![(2.0, 4.0)],
            },
        ],
    );
    let words = spoken(&heard).unwrap();
    assert_eq!(words, vec![word("hello", 1.0, 1.4)]);
    let fps = Rational::new(25, 1).unwrap();
    assert_eq!(
        word_frames(&words, fps, 100, 200),
        vec![("hello".into(), 125, 135)]
    );
}

#[test]
fn a_model_that_times_phrases_is_refused() {
    let heard = speech("Whisper large-v3", Timing::Phrases, Vec::new());
    assert_eq!(
        spoken(&heard).unwrap_err(),
        "Whisper large-v3 times phrases, not words"
    );
}

#[test]
fn a_model_with_no_times_is_refused() {
    let heard = speech("Qwen3-ASR 1.7B", Timing::Pauses, Vec::new());
    assert_eq!(
        spoken(&heard).unwrap_err(),
        "Qwen3-ASR 1.7B has no word times"
    );
}

fn fixture() -> Fixture {
    Fixture::new(&[(10.0, true)])
}

fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({ "method": method, "params": params })
}

#[test]
fn cutting_words_closes_the_gap_with_ripple_off() {
    let mut f = fixture();
    f.append("m1", 0, 100);
    f.ok(req("project.set", json!({"ripple_on": false})));
    let r = f.ok(req(
        "edit.ripple_delete_words",
        json!({
            "words": [
                {"text": "one", "start": 10, "end": 20},
                {"text": "two", "start": 20, "end": 30},
                {"text": "four", "start": 50, "end": 60}
            ]
        }),
    ));
    assert_eq!(r.result["frames"], 30);
    assert_eq!(
        r.result["removed"],
        json!([{"start": 10, "end": 30}, {"start": 50, "end": 60}])
    );
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 10, 0),
            row("c5", 10, 20, 30),
            row("c3", 30, 40, 60)
        ]
    );
    assert!(!f.editor.project().settings.ripple_on);
}
