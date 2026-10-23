use crate::model::{LetterCase, Place, SubtitleStyle, SubtitleWords, WordsShow};
use crate::ops::subtitles::checked_style;
use crate::request::SubtitleFormat;
use crate::subtitles::{Piece, Timed, Writing, shown, write};

const TEXT: &str = "one two three";
const WORDS: [i64; 3] = [0, 10, 20];

fn animated(show: WordsShow, pop: Option<f64>) -> SubtitleStyle {
    SubtitleStyle {
        words: Some(SubtitleWords {
            show: Some(show),
            color: None,
            pop,
        }),
        ..SubtitleStyle::default()
    }
}

fn texts(pieces: &[Piece]) -> Vec<(String, Option<String>, bool)> {
    pieces
        .iter()
        .map(|p| (p.text.clone(), p.color.clone(), p.hidden))
        .collect()
}

fn piece(text: &str, color: Option<&str>, hidden: bool) -> (String, Option<String>, bool) {
    (text.into(), color.map(String::from), hidden)
}

#[test]
fn the_spoken_word_is_colored() {
    let s = shown(
        TEXT,
        &WORDS,
        &animated(WordsShow::Highlight, None),
        12,
        25.0,
        None,
    );
    assert_eq!(
        texts(&s.pieces),
        vec![
            piece("one", None, false),
            piece(" ", None, false),
            piece("two", Some("#ffd400"), false),
            piece(" ", None, false),
            piece("three", None, false),
        ]
    );
}

#[test]
fn reveal_hides_the_words_not_yet_spoken() {
    let s = shown(
        TEXT,
        &WORDS,
        &animated(WordsShow::Reveal, None),
        12,
        25.0,
        None,
    );
    assert_eq!(
        texts(&s.pieces),
        vec![
            piece("one", None, false),
            piece(" ", None, false),
            piece("two", None, false),
            piece(" ", None, false),
            piece("three", None, true),
        ]
    );
}

#[test]
fn only_the_spoken_word_shows_as_a_word() {
    let s = shown(
        TEXT,
        &WORDS,
        &animated(WordsShow::Word, None),
        12,
        25.0,
        None,
    );
    assert_eq!(texts(&s.pieces), vec![piece("two", None, false)]);
}

#[test]
fn a_spoken_word_starts_at_its_pop_and_settles_in_a_fifth_of_a_second() {
    let style = animated(WordsShow::Highlight, Some(1.5));
    let pops: Vec<f64> = [10, 15]
        .iter()
        .map(|&f| shown(TEXT, &WORDS, &style, f, 25.0, None).pop)
        .collect();
    assert_eq!(pops, vec![1.5, 1.0]);
}

#[test]
fn a_pop_larger_than_two_is_refused() {
    let err = checked_style(&animated(WordsShow::Highlight, Some(2.5))).unwrap_err();
    assert_eq!(
        err.to_string(),
        "a spoken word's pop is a multiple of the text's size, from 1 to 2"
    );
}

#[test]
fn an_imsc1_file_writes_capitals_and_bold() {
    let style = SubtitleStyle {
        bold: Some(true),
        case: Some(LetterCase::Upper),
        ..SubtitleStyle::default()
    };
    let text = write(
        &[Timed {
            start: 1.0,
            end: 2.0,
            text: "Hi <i>there</i>".into(),
            place: Place::Bottom,
            words: Vec::new(),
            speaker: None,
            style: None,
        }],
        SubtitleFormat::Ttml,
        &Writing {
            language: Some("en"),
            frame_rate: Some(25.0),
            positions: true,
            style: &style,
        },
    )
    .text;
    let found = (
        text.contains("tts:fontWeight=\"bold\""),
        text.contains(">HI <span tts:fontStyle=\"italic\">THERE</span></p>"),
    );
    assert_eq!(found, (true, true));
}
