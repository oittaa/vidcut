use std::path::Path;

use serde_json::{Value, json};

use crate::model::{Cue, Place, SpeakerPaint, SubtitleStyle, SubtitleWords, WordsShow};
use crate::request::SubtitleFormat;
use crate::speakers::Speakers;
use crate::subtitles::{Read, Timed, Writing, parse, shown, write};
use crate::test_support::Fixture;
use crate::time::Rational;
use crate::transcript::{Layout, Said, Word, cues};

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn cue(speaker: Option<&str>, color: Option<&str>) -> Cue {
    Cue {
        id: "q1".into(),
        start: 0,
        duration: 25,
        text: "hi".into(),
        place: Place::Bottom,
        words: Vec::new(),
        speaker: speaker.map(String::from),
        color: color.map(String::from),
        style: None,
    }
}

fn highlight() -> SubtitleStyle {
    SubtitleStyle {
        words: Some(SubtitleWords {
            show: Some(WordsShow::Highlight),
            color: None,
            pop: None,
        }),
        ..SubtitleStyle::default()
    }
}

fn webvtt(source: &str) -> Read {
    parse(source.as_bytes(), Path::new("a.vtt")).unwrap().0
}

fn colors(text: &str, words: &[i64], style: &SubtitleStyle) -> Vec<Option<String>> {
    shown(text, words, style, 12, 25.0, Some("#00ff00"))
        .pieces
        .into_iter()
        .map(|p| p.color)
        .collect()
}

#[test]
fn a_speakers_color_paints_the_spoken_word() {
    let green = Some("#00ff00".to_string());
    assert_eq!(
        colors("one two", &[0, 10], &highlight()),
        vec![None, None, green]
    );
}

#[test]
fn a_speakers_color_paints_the_text_of_a_track_without_spoken_words() {
    let green = Some("#00ff00".to_string());
    assert_eq!(
        colors("one two", &[], &SubtitleStyle::default()),
        vec![green]
    );
}

#[test]
fn a_track_painting_none_keeps_its_own_colors() {
    let style = SubtitleStyle {
        speaker_color: Some(SpeakerPaint::None),
        ..SubtitleStyle::default()
    };
    assert_eq!(colors("one two", &[], &style), vec![None]);
}

#[test]
fn a_cues_own_color_is_over_its_speakers() {
    let mut s = Speakers::default();
    s.colors.insert("r1/2".into(), "#ff0000".into());
    let c = cue(Some("r1/2"), Some("#0000ff"));
    assert_eq!(s.color_of(&c).as_deref(), Some("#0000ff"));
}

#[test]
fn a_cue_without_its_own_color_takes_its_speakers() {
    let mut s = Speakers::default();
    s.colors.insert("r1/2".into(), "#ff0000".into());
    assert_eq!(
        s.color_of(&cue(Some("r1/2"), None)).as_deref(),
        Some("#ff0000")
    );
}

#[test]
fn a_name_finds_the_person_it_names() {
    let mut s = Speakers::default();
    s.names.insert("r1/2".into(), "Nikki".into());
    assert_eq!(s.key_for("Nikki"), "r1/2");
}

#[test]
fn a_cue_made_from_speech_keeps_its_speakers_number() {
    let said = Said {
        start: 0.0,
        end: 1.0,
        text: String::new(),
        words: vec![Word {
            start: 0.0,
            end: 0.4,
            text: "yes".into(),
            joined: false,
            speaker: Some(3),
        }],
        speech: Vec::new(),
    };
    let layout = Layout {
        chars: 42,
        lines: 1,
        gap: 0,
    };
    let made = cues(&[said], Rational::new(25, 1).unwrap(), layout);
    assert_eq!(made[0].speaker, Some(3));
}

#[test]
fn webvtt_writes_the_speaker_as_a_voice() {
    let cues = [Timed {
        start: 1.0,
        end: 2.0,
        text: "Thank you.".into(),
        place: Place::Bottom,
        words: Vec::new(),
        speaker: Some("Nikki".into()),
        style: None,
    }];
    let how = Writing {
        language: None,
        frame_rate: None,
        positions: false,
        style: &SubtitleStyle::default(),
    };
    assert_eq!(
        write(&cues, SubtitleFormat::Vtt, &how).text,
        "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<v Nikki>Thank you.\n\n"
    );
}

#[test]
fn webvtt_reads_a_voice_as_the_speaker() {
    let read = webvtt("WEBVTT\n\n00:01.000 --> 00:02.000\n<v.loud Nikki>Thank you.\n");
    assert_eq!(read.cues[0].speaker.as_deref(), Some("Nikki"));
}

#[test]
fn a_voice_is_not_part_of_the_text() {
    let read = webvtt("WEBVTT\n\n00:01.000 --> 00:02.000\n<v Nikki>Thank you.\n");
    assert_eq!(read.cues[0].text, "Thank you.");
}

#[test]
fn matching_a_speaker_gives_their_other_cues_its_place() {
    let mut f = Fixture::new(&[]);
    for at in ["0f", "50f", "100f"] {
        f.ok(req(
            "subtitles.add",
            json!({"text": "hi", "at": at, "duration": "25f", "speaker": "Nikki"}),
        ));
    }
    f.ok(req("subtitles.set", json!({"cue": "q2", "place": "top"})));
    f.ok(req("subtitles.match_speaker", json!({"cue": "q2"})));
    let places: Vec<Place> = f.editor.project().subtitles[0]
        .cues
        .iter()
        .map(|c| c.place)
        .collect();
    assert_eq!(places, [Place::Top, Place::Top, Place::Top]);
}

#[test]
fn matching_a_speaker_leaves_other_peoples_cues() {
    let mut f = Fixture::new(&[]);
    f.ok(req(
        "subtitles.add",
        json!({"text": "hi", "at": "0f", "duration": "25f", "speaker": "Nikki", "place": "top"}),
    ));
    f.ok(req(
        "subtitles.add",
        json!({"text": "hi", "at": "50f", "duration": "25f", "speaker": "Will"}),
    ));
    f.ok(req("subtitles.match_speaker", json!({"cue": "q1"})));
    let (ti, ci) = f.editor.project().find_cue("q2").unwrap();
    assert_eq!(
        f.editor.project().subtitles[ti].cues[ci].place,
        Place::Bottom
    );
}

#[test]
fn applying_to_all_makes_the_cues_color_its_speakers() {
    let mut f = Fixture::new(&[]);
    f.ok(req(
        "subtitles.add",
        json!({"text": "hi", "at": "0f", "duration": "25f", "speaker": "Nikki", "color": "#00ff88"}),
    ));
    f.ok(req("subtitles.match_speaker", json!({"cue": "q1"})));
    let p = f.editor.project();
    assert_eq!(
        p.speakers.colors.get("Nikki").map(String::as_str),
        Some("#00ff88")
    );
}

#[test]
fn applying_to_all_takes_away_the_speakers_own_cue_colors() {
    let mut f = Fixture::new(&[]);
    f.ok(req(
        "subtitles.add",
        json!({"text": "hi", "at": "0f", "duration": "25f", "speaker": "Nikki"}),
    ));
    f.ok(req(
        "subtitles.add",
        json!({"text": "hi", "at": "50f", "duration": "25f", "speaker": "Nikki", "color": "red"}),
    ));
    f.ok(req("subtitles.match_speaker", json!({"cue": "q1"})));
    let (ti, ci) = f.editor.project().find_cue("q2").unwrap();
    assert_eq!(f.editor.project().subtitles[ti].cues[ci].color, None);
}

#[test]
fn joining_gives_a_persons_cues_to_the_other() {
    let mut f = Fixture::new(&[]);
    for (at, who) in [("0f", "r1/5"), ("50f", "r1/2")] {
        f.ok(req(
            "subtitles.add",
            json!({"text": "hi", "at": at, "duration": "25f", "speaker": who}),
        ));
    }
    f.ok(req(
        "speakers.join",
        json!({"person": "r1/5", "into": "r1/2"}),
    ));
    let who: Vec<Option<String>> = f.editor.project().subtitles[0]
        .cues
        .iter()
        .map(|c| c.speaker.clone())
        .collect();
    assert_eq!(who, [Some("r1/2".to_string()), Some("r1/2".to_string())]);
}

#[test]
fn a_joined_person_keeps_their_own_color() {
    let mut s = Speakers::default();
    s.colors.insert("r1/5".into(), "#ff0000".into());
    s.colors.insert("r1/2".into(), "#00ff00".into());
    s.join("r1/5", "r1/2", Vec::new());
    assert_eq!(s.colors.get("r1/2").map(String::as_str), Some("#00ff00"));
}

#[test]
fn a_joined_person_without_a_color_takes_the_others() {
    let mut s = Speakers::default();
    s.colors.insert("r1/5".into(), "#ff0000".into());
    s.join("r1/5", "r1/2", Vec::new());
    assert_eq!(s.colors.get("r1/2").map(String::as_str), Some("#ff0000"));
}

#[test]
fn joining_links_the_files_speakers_that_were_them() {
    let mut s = Speakers::default();
    s.join("r1/5", "r1/2", vec!["m1/0/5".into()]);
    assert_eq!(s.person("m1", 0, "r1", 5, 1.0), "r1/2");
}
