use crate::speakers::{Assigned, Speakers};
use crate::time::Rational;
use crate::transcript::{Layout, Said, Turn, Word, cues, give_speakers};

fn word(text: &str, start: f64, end: f64, speaker: Option<u32>) -> Word {
    Word {
        start,
        end,
        text: text.into(),
        joined: false,
        speaker,
    }
}

fn said(words: Vec<Word>) -> Said {
    Said {
        start: 0.0,
        end: 10.0,
        text: String::new(),
        words,
        speech: Vec::new(),
    }
}

fn turn(start: f64, end: f64, speaker: u32) -> Turn {
    Turn {
        start,
        end,
        speaker,
    }
}

#[test]
fn a_word_takes_the_speaker_heard_over_most_of_it() {
    let mut heard = vec![said(vec![word("hello", 1.0, 2.0, None)])];
    give_speakers(&mut heard, &[turn(0.0, 1.3, 1), turn(1.3, 3.0, 2)]);
    assert_eq!(heard[0].words[0].speaker, Some(2));
}

#[test]
fn a_word_between_turns_has_no_speaker() {
    let mut heard = vec![said(vec![word("the", 2.0, 2.2, None)])];
    give_speakers(&mut heard, &[turn(0.0, 1.5, 1), turn(2.5, 4.0, 2)]);
    assert_eq!(heard[0].words[0].speaker, None);
}

#[test]
fn a_word_outside_every_turn_has_no_speaker() {
    let mut heard = vec![said(vec![word("hello", 5.0, 6.0, Some(3))])];
    give_speakers(&mut heard, &[turn(0.0, 1.0, 1)]);
    assert_eq!(heard[0].words[0].speaker, None);
}

#[test]
fn a_cue_ends_where_the_speaker_changes() {
    let heard = vec![said(vec![
        word("yes", 0.0, 0.4, Some(1)),
        word("no", 0.5, 0.9, Some(2)),
    ])];
    let layout = Layout {
        chars: 42,
        lines: 1,
        gap: 0,
    };
    let texts: Vec<String> = cues(&heard, Rational::new(25, 1).unwrap(), layout)
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(texts, ["yes", "no"]);
}

#[test]
fn a_speaker_is_the_runs_person_until_linked() {
    let s = Speakers::default();
    assert_eq!(s.person("m1", 0, "r7", 2, 1.0), "r7/2");
}

#[test]
fn a_linked_speaker_is_the_person_it_was_linked_to() {
    let mut s = Speakers::default();
    s.links.insert("m2/0/1".into(), "r7/2".into());
    assert_eq!(s.person("m2", 0, "r9", 1, 1.0), "r7/2");
}

#[test]
fn an_assigned_word_is_its_new_persons() {
    let mut s = Speakers::default();
    s.assign(Assigned {
        media: "m1".into(),
        stream: 0,
        start: 4.0,
        end: 6.0,
        person: "r7/1".into(),
    });
    assert_eq!(s.person("m1", 0, "r7", 2, 5.0), "r7/1");
}

#[test]
fn an_assigned_word_without_a_speaker_is_its_new_persons() {
    let mut s = Speakers::default();
    s.assign(Assigned {
        media: "m1".into(),
        stream: 0,
        start: 4.0,
        end: 6.0,
        person: "will".into(),
    });
    let w = word("road", 5.0, 5.4, None);
    assert_eq!(s.word("m1", 0, Some("r7"), &w).as_deref(), Some("will"));
}

#[test]
fn assigning_no_one_gives_the_words_back_to_their_speaker() {
    let mut s = Speakers::default();
    let a = |person: &str| Assigned {
        media: "m1".into(),
        stream: 0,
        start: 4.0,
        end: 6.0,
        person: person.into(),
    };
    s.assign(a("r7/1"));
    s.assign(a(""));
    assert!(s.assigned.is_empty());
}

#[test]
fn assigning_inside_a_stretch_keeps_the_words_around_it() {
    let mut s = Speakers::default();
    let a = |start: f64, end: f64, person: &str| Assigned {
        media: "m1".into(),
        stream: 0,
        start,
        end,
        person: person.into(),
    };
    s.assign(a(0.0, 10.0, "will"));
    s.assign(a(4.0, 6.0, "nikki"));
    assert_eq!(s.assigned_to("m1", 0, 8.0).as_deref(), Some("will"));
}

#[test]
fn a_person_made_up_by_name_is_called_that() {
    assert_eq!(Speakers::default().name("Will"), "Will");
}

#[test]
fn an_unnamed_person_is_speaker_and_its_number() {
    assert_eq!(Speakers::default().name("r7/3"), "Speaker 3");
}
