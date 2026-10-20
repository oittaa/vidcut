use std::path::Path;

use serde_json::{Value, json};

use crate::retime::{Spoken, word_starts};
use crate::subtitles::{fill_times, parse};
use crate::test_support::Fixture;
use crate::time::Rational;
use crate::transcript::{Layout, Said, Word, cues};

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn word(text: &str, start: f64, end: f64) -> Word {
    Word {
        start,
        end,
        text: text.into(),
        joined: false,
    }
}

fn spoken(text: &str, start: f64) -> Spoken {
    Spoken {
        start,
        end: start + 0.3,
        text: text.into(),
    }
}

fn words_of(f: &Fixture, cue: &str) -> Vec<i64> {
    let (ti, ci) = f.editor.project().find_cue(cue).unwrap();
    f.editor.project().subtitles[ti].cues[ci].words.clone()
}

fn one_cue(f: &mut Fixture) {
    f.ok(req(
        "subtitles.add",
        json!({"text": "one two three", "at": "0f", "duration": "100f", "words": [0, 40, 80]}),
    ));
}

#[test]
fn a_cue_from_speech_keeps_its_words_starts() {
    let said = Said {
        start: 1.0,
        end: 2.5,
        text: "Hello big world.".into(),
        words: vec![
            word("Hello", 1.0, 1.3),
            word("big", 1.4, 1.7),
            word("world.", 2.0, 2.5),
        ],
        speech: vec![(1.0, 2.5)],
    };
    let fps = Rational::new(25, 1).unwrap();
    let layout = Layout {
        chars: 42,
        lines: 1,
        gap: 0,
    };
    let made = cues(&[said], fps, layout);
    assert_eq!(made[0].start, 25);
    assert_eq!(made[0].words, vec![0, 10, 25]);
}

#[test]
fn trimming_a_cues_start_shifts_its_words() {
    let mut f = Fixture::new(&[]);
    one_cue(&mut f);
    f.ok(req("subtitles.set", json!({"cue": "q1", "start": "30f"})));
    assert_eq!(words_of(&f, "q1"), vec![0, 10, 50]);
}

#[test]
fn a_ripple_delete_inside_a_cue_moves_the_cut_word_to_the_cut() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 250);
    one_cue(&mut f);
    f.ok(req("edit.delete_range", json!({"range": "30f..50f"})));
    assert_eq!(words_of(&f, "q1"), vec![0, 30, 60]);
}

#[test]
fn splitting_a_cue_rebases_the_right_parts_words() {
    let mut f = Fixture::new(&[]);
    one_cue(&mut f);
    let r = f.ok(req("subtitles.split", json!({"cue": "q1", "at": "50f"})));
    let right = r.result["cue"].as_str().unwrap().to_string();
    assert_eq!(words_of(&f, &right), vec![0, 0, 30]);
}

#[test]
fn new_text_with_more_words_spreads_them_by_their_characters() {
    let mut f = Fixture::new(&[]);
    one_cue(&mut f);
    f.ok(req(
        "subtitles.set",
        json!({"cue": "q1", "text": "one two three four"}),
    ));
    assert_eq!(words_of(&f, "q1"), vec![0, 21, 42, 73]);
}

#[test]
fn word_times_that_dont_match_the_text_are_refused() {
    let mut f = Fixture::new(&[]);
    one_cue(&mut f);
    let e = f.err(req("subtitles.set", json!({"cue": "q1", "words": [0, 10]})));
    assert_eq!(e.message, "the text has 3 word(s) and 2 word time(s)");
}

#[test]
fn a_cues_words_take_the_heard_words_times() {
    let cue = vec!["Hello,".to_string(), "big".into(), "world".into()];
    let heard = [
        spoken("hello", 1.0),
        spoken("big", 1.5),
        spoken("world", 2.0),
    ];
    assert_eq!(
        word_starts(&[cue], &heard),
        vec![vec![Some(1.0), Some(1.5), Some(2.0)]]
    );
}

#[test]
fn an_unmatched_word_is_spread_between_its_neighbors() {
    assert_eq!(
        fill_times(&[Some(1.0), None, Some(3.0)], &[2.0, 2.0, 2.0], 0.0, 4.0),
        vec![1.0, 2.0, 3.0]
    );
}

#[test]
fn webvtt_timestamps_become_word_times() {
    let vtt =
        "WEBVTT\n\n00:00:01.000 --> 00:00:04.000\none <00:00:02.000>two <00:00:03.000>three\n";
    let (read, _, _) = parse(vtt.as_bytes(), Path::new("a.vtt")).unwrap();
    assert_eq!(read.cues[0].words, vec![1.0, 2.0, 3.0]);
}
