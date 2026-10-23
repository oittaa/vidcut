use serde_json::{Value, json};

use crate::model::{
    LetterCase, Place, Project, Settings, SubtitleAlign, SubtitleBackground, SubtitleOutline,
    SubtitleStyle,
};
use crate::ops::subtitles::{
    TrackProps, add_cue, add_track, checked_style, set_style, set_track, set_word_style,
};
use crate::request::SubtitleFormat;
use crate::subtitles::{Timed, Writing, write};
use crate::test_support::Fixture;

fn style() -> SubtitleStyle {
    SubtitleStyle {
        font: Some(" Arial ".into()),
        size: Some(0.04),
        color: Some("#FC0".into()),
        outline: Some(SubtitleOutline {
            color: Some("#000000".into()),
            size: Some(0.0),
        }),
        background: Some(SubtitleBackground {
            color: "#000000".into(),
            opacity: Some(0.5),
        }),
        margin: Some(0.08),
        ..SubtitleStyle::default()
    }
}

#[test]
fn a_track_style_is_stored_in_canonical_form() {
    let mut p = Project::new(Settings::default());
    let id = add_track(&mut p, None);
    let style = checked_style(&style()).unwrap();
    set_track(
        &mut p,
        &id,
        &TrackProps {
            style: Some(style),
            ..TrackProps::default()
        },
    )
    .unwrap();
    assert_eq!(
        p.subtitles[0].style,
        SubtitleStyle {
            font: Some("Arial".into()),
            size: Some(0.04),
            color: Some("#ffcc00".into()),
            outline: Some(SubtitleOutline {
                color: Some("#000000".into()),
                size: Some(0.0),
            }),
            background: Some(SubtitleBackground {
                color: "#000000".into(),
                opacity: Some(0.5),
            }),
            margin: Some(0.08),
            ..SubtitleStyle::default()
        }
    );
}

#[test]
fn a_subtitle_position_outside_the_picture_is_refused() {
    let err = checked_style(&SubtitleStyle {
        position: Some([1.5, 0.4]),
        ..SubtitleStyle::default()
    })
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "a subtitle position is two fractions of the picture, from 0 to 1"
    );
}

#[test]
fn a_subtitle_size_past_half_the_picture_is_refused() {
    let err = checked_style(&SubtitleStyle {
        size: Some(0.6),
        ..SubtitleStyle::default()
    })
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "a subtitle size is a fraction of the picture height, above 0 and at most 1/2"
    );
}

#[test]
fn an_outline_thicker_than_a_tenth_is_refused() {
    let err = checked_style(&SubtitleStyle {
        outline: Some(SubtitleOutline {
            color: None,
            size: Some(0.2),
        }),
        ..SubtitleStyle::default()
    })
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "an outline's thickness is a fraction of the text height, from 0 to 1/10"
    );
}

#[test]
fn a_project_file_refuses_a_style_track_set_would_refuse() {
    let mut p = Project::new(Settings::default());
    add_track(&mut p, None);
    p.subtitles[0].style.size = Some(40.0);
    assert_eq!(
        crate::validate::violations(&p),
        vec![
            "S1: a subtitle size is a fraction of the picture height, above 0 and at most 1/2"
                .to_string()
        ]
    );
}

#[test]
fn the_usual_block_sits_at_eighty_percent() {
    let (wrap, top) = (
        SubtitleStyle::default().wrap_width(1000.0, 500.0),
        SubtitleStyle::default().block_top(Place::Bottom, 500.0, 40.0),
    );
    assert_eq!(format!("{wrap:.3} {top:.3}"), "900.000 380.000");
}

#[test]
fn a_margin_insets_a_bottom_block() {
    let style = SubtitleStyle {
        margin: Some(0.1),
        ..SubtitleStyle::default()
    };
    assert_eq!(
        format!(
            "{:.3} {:.3}",
            style.wrap_width(1000.0, 500.0),
            style.block_top(Place::Bottom, 500.0, 40.0)
        ),
        "800.000 410.000"
    );
}

#[test]
fn an_imsc1_file_writes_the_track_style() {
    let style = checked_style(&style()).unwrap();
    let text = write(
        &[Timed {
            start: 1.0,
            end: 2.5,
            text: "Hi".into(),
            place: Place::Bottom,
            words: Vec::new(),
            speaker: None,
            style: None,
            word_styles: Vec::new(),
        }],
        SubtitleFormat::Ttml,
        &crate::subtitles::Writing {
            language: Some("en"),
            frame_rate: Some(25.0),
            positions: true,
            style: &style,
        },
    )
    .text;
    assert_eq!(
        text,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <tt xml:lang=\"en\" xmlns=\"http://www.w3.org/ns/ttml\" \
         xmlns:ttp=\"http://www.w3.org/ns/ttml#parameter\" \
         xmlns:tts=\"http://www.w3.org/ns/ttml#styling\" \
         xmlns:ittp=\"http://www.w3.org/ns/ttml/profile/imsc1#parameter\" \
         ttp:profile=\"http://www.w3.org/ns/ttml/profile/imsc1/text\" \
         ttp:timeBase=\"media\" ttp:frameRate=\"25\" ttp:cellResolution=\"100 100\">\n\
         \x20 <head>\n\
         \x20   <styling>\n\
         \x20     <style xml:id=\"s\" tts:color=\"#ffcc00\" \
         tts:fontFamily=\"Arial,proportionalSansSerif\" tts:fontSize=\"400%\" \
         tts:lineHeight=\"131%\" tts:textAlign=\"center\"/>\n\
         \x20   </styling>\n\
         \x20   <layout>\n\
         \x20     <region xml:id=\"bottom\" tts:origin=\"8% 28.2%\" tts:extent=\"84% 63.8%\" \
         tts:displayAlign=\"after\"/>\n\
         \x20     <region xml:id=\"middle\" tts:origin=\"8% 0%\" tts:extent=\"84% 100%\" \
         tts:displayAlign=\"center\"/>\n\
         \x20     <region xml:id=\"top\" tts:origin=\"8% 8%\" tts:extent=\"84% 63.8%\" \
         tts:displayAlign=\"before\"/>\n\
         \x20   </layout>\n\
         \x20 </head>\n\
         \x20 <body style=\"s\">\n\
         \x20   <div>\n\
         \x20     <p begin=\"00:00:01.000\" end=\"00:00:02.500\" region=\"bottom\"><span \
         tts:backgroundColor=\"#00000080\">Hi</span></p>\n\
         \x20   </div>\n\
         \x20 </body>\n\
         </tt>\n"
    );
}

#[test]
fn a_styled_file_keeps_each_cues_place() {
    let style = checked_style(&style()).unwrap();
    let text = write(
        &[
            Timed {
                start: 1.0,
                end: 2.0,
                text: "Bottom".into(),
                place: Place::Bottom,
                words: Vec::new(),
                speaker: None,
                style: None,
                word_styles: Vec::new(),
            },
            Timed {
                start: 3.0,
                end: 4.0,
                text: "Middle".into(),
                place: Place::Middle,
                words: Vec::new(),
                speaker: None,
                style: None,
                word_styles: Vec::new(),
            },
            Timed {
                start: 5.0,
                end: 6.0,
                text: "Top".into(),
                place: Place::Top,
                words: Vec::new(),
                speaker: None,
                style: None,
                word_styles: Vec::new(),
            },
        ],
        SubtitleFormat::Ttml,
        &crate::subtitles::Writing {
            language: Some("en"),
            frame_rate: Some(25.0),
            positions: true,
            style: &style,
        },
    )
    .text;
    let (read, _, _) =
        crate::subtitles::parse(text.as_bytes(), std::path::Path::new("cues.ttml")).unwrap();
    assert_eq!(
        read.cues.iter().map(|c| c.place).collect::<Vec<_>>(),
        vec![Place::Bottom, Place::Middle, Place::Top]
    );
}

#[test]
fn a_cue_keeps_the_track_fields_it_leaves_out() {
    let track = SubtitleStyle {
        color: Some("#ffffff".into()),
        size: Some(0.08),
        ..SubtitleStyle::default()
    };
    let cue = SubtitleStyle {
        size: Some(0.04),
        align: Some(SubtitleAlign::Left),
        position: Some([0.25, 0.4]),
        ..SubtitleStyle::default()
    };
    assert_eq!(
        track.over(&cue),
        SubtitleStyle {
            color: Some("#ffffff".into()),
            size: Some(0.04),
            align: Some(SubtitleAlign::Left),
            position: Some([0.25, 0.4]),
            ..SubtitleStyle::default()
        }
    );
}

#[test]
fn a_set_position_places_the_left_edge_on_that_point() {
    let style = SubtitleStyle {
        align: Some(SubtitleAlign::Left),
        position: Some([0.25, 0.4]),
        ..SubtitleStyle::default()
    };
    assert_eq!(
        style.block_box(Place::Bottom, (1000.0, 500.0), 200.0, 40.0),
        (250.0, 180.0)
    );
}

#[test]
fn alignment_without_a_position_keeps_the_block_centered() {
    let style = SubtitleStyle {
        align: Some(SubtitleAlign::Left),
        ..SubtitleStyle::default()
    };
    assert_eq!(
        style.block_box(Place::Bottom, (1000.0, 500.0), 200.0, 40.0),
        (400.0, 380.0)
    );
}

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

#[test]
fn setting_one_cue_leaves_the_other_on_the_track_style() {
    let mut f = Fixture::new(&[]);
    f.ok(req(
        "subtitles.add",
        json!({"text": "one", "at": "0f", "duration": "10f"}),
    ));
    f.ok(req(
        "subtitles.add",
        json!({"text": "two", "at": "20f", "duration": "10f"}),
    ));
    f.ok(req(
        "subtitles.set",
        json!({
            "cue": "q1",
            "style": {"align": "left", "position": [0.25, 0.4], "size": 0.04}
        }),
    ));
    let cues = &f.editor.project().subtitles[0].cues;
    assert_eq!(
        cues[0].style,
        Some(SubtitleStyle {
            size: Some(0.04),
            align: Some(SubtitleAlign::Left),
            position: Some([0.25, 0.4]),
            ..SubtitleStyle::default()
        })
    );
    assert_eq!(cues[1].style, None);
}

#[test]
fn a_word_keeps_the_cue_fields_it_leaves_out() {
    let cue = SubtitleStyle {
        color: Some("#ffffff".into()),
        size: Some(0.08),
        ..SubtitleStyle::default()
    };
    let word = SubtitleStyle {
        size: Some(0.04),
        bold: Some(true),
        ..SubtitleStyle::default()
    };
    assert_eq!(
        cue.for_cue(Some(&word)),
        SubtitleStyle {
            color: Some("#ffffff".into()),
            size: Some(0.04),
            bold: Some(true),
            ..SubtitleStyle::default()
        }
    );
}

#[test]
fn setting_one_word_leaves_the_other_on_the_cue_style() {
    let mut f = Fixture::new(&[]);
    f.ok(req(
        "subtitles.add",
        json!({"text": "one two", "at": "0f", "duration": "10f"}),
    ));
    f.ok(req(
        "subtitles.set_word",
        json!({"cue": "q1", "word": 0, "style": {"color": "#ff0000", "size": 0.04}}),
    ));
    let cue = &f.editor.project().subtitles[0].cues[0];
    assert_eq!(
        cue.words[0].style,
        Some(SubtitleStyle {
            color: Some("#ff0000".into()),
            size: Some(0.04),
            ..SubtitleStyle::default()
        })
    );
    assert_eq!(cue.words.get(1).and_then(|w| w.style.clone()), None);
}

#[test]
fn a_bold_cue_stores_a_word_that_is_not_bold() {
    let mut p = Project::new(Settings::default());
    add_track(&mut p, None);
    let id = add_cue(&mut p, 0, 0, None, Some(10), "one", None, Vec::new()).unwrap();
    set_style(
        &mut p,
        &id,
        SubtitleStyle {
            bold: Some(true),
            ..SubtitleStyle::default()
        },
    )
    .unwrap();
    set_word_style(
        &mut p,
        &id,
        0,
        SubtitleStyle {
            bold: Some(false),
            ..SubtitleStyle::default()
        },
    )
    .unwrap();
    let cue = &p.subtitles[0].cues[0];
    let stored = cue
        .words
        .first()
        .and_then(|w| w.style.as_ref())
        .and_then(|s| s.bold);
    let cue_style = cue.style.clone();
    let text = cue.text.clone();
    let times = cue.times();
    let word_styles: Vec<_> = cue.words.iter().map(|w| w.style.clone()).collect();
    let style = p.subtitles[0].style.for_cue(cue_style.as_ref());
    let shown = crate::subtitles::shown(
        &text,
        &times,
        &word_styles,
        &style,
        0,
        p.settings.fps.as_f64(),
        None,
    );
    let drawn = shown
        .pieces
        .iter()
        .find(|piece| piece.text == "one")
        .map(|piece| piece.bold);
    assert_eq!(stored, Some(false));
    assert_eq!(drawn, Some(false));
}

#[test]
fn a_bold_track_stores_a_cue_that_is_not_bold() {
    let mut p = Project::new(Settings::default());
    let track = add_track(&mut p, None);
    set_track(
        &mut p,
        &track,
        &TrackProps {
            style: Some(SubtitleStyle {
                bold: Some(true),
                ..SubtitleStyle::default()
            }),
            ..TrackProps::default()
        },
    )
    .unwrap();
    let id = add_cue(&mut p, 0, 0, None, Some(10), "one", None, Vec::new()).unwrap();
    set_style(
        &mut p,
        &id,
        SubtitleStyle {
            bold: Some(false),
            ..SubtitleStyle::default()
        },
    )
    .unwrap();
    let cue = &p.subtitles[0].cues[0];
    let stored = cue.style.as_ref().and_then(|s| s.bold);
    let cue_style = cue.style.clone();
    let text = cue.text.clone();
    let times = cue.times();
    let word_styles: Vec<_> = cue.words.iter().map(|w| w.style.clone()).collect();
    let style = p.subtitles[0].style.for_cue(cue_style.as_ref());
    let shown = crate::subtitles::shown(
        &text,
        &times,
        &word_styles,
        &style,
        0,
        p.settings.fps.as_f64(),
        None,
    );
    let drawn = shown
        .pieces
        .iter()
        .find(|piece| piece.text == "one")
        .map(|piece| piece.bold);
    assert_eq!((stored, drawn), (Some(false), Some(false)));
}

#[test]
fn the_viewer_paints_one_word_and_leaves_the_other() {
    let cue = SubtitleStyle {
        color: Some("#ffffff".into()),
        ..SubtitleStyle::default()
    };
    let word = SubtitleStyle {
        color: Some("#ff0000".into()),
        size: Some(0.11),
        bold: Some(true),
        case: Some(LetterCase::Upper),
        ..SubtitleStyle::default()
    };
    let shown = crate::subtitles::shown("one two", &[], &[None, Some(word)], &cue, 0, 25.0, None);
    let got: Vec<(String, Option<String>, bool, Option<u32>)> = shown
        .pieces
        .iter()
        .map(|p| (p.text.clone(), p.color.clone(), p.bold, p.size_scale))
        .collect();
    assert_eq!(
        got,
        vec![
            ("one".into(), None, false, None),
            (" ".into(), None, false, None),
            ("TWO".into(), Some("#ff0000".into()), true, Some(2000)),
        ]
    );
}

#[test]
fn a_bold_cue_draws_a_word_plain_when_the_word_is_not_bold() {
    let cue = SubtitleStyle {
        bold: Some(true),
        ..SubtitleStyle::default()
    };
    let word = SubtitleStyle {
        bold: Some(false),
        ..SubtitleStyle::default()
    };
    let shown = crate::subtitles::shown(
        "<b>one two</b>",
        &[],
        &[Some(word), None],
        &cue,
        0,
        25.0,
        None,
    );
    let bold = shown
        .pieces
        .iter()
        .find(|p| p.text == "one")
        .map(|p| p.bold);
    assert_eq!(bold, Some(false));
}

#[test]
fn a_bold_cue_keeps_a_word_bold_when_that_word_leaves_bold_out() {
    let cue = SubtitleStyle {
        bold: Some(true),
        ..SubtitleStyle::default()
    };
    let word = SubtitleStyle {
        bold: Some(false),
        ..SubtitleStyle::default()
    };
    let shown = crate::subtitles::shown("one two", &[], &[Some(word), None], &cue, 0, 25.0, None);
    let bold = shown
        .pieces
        .iter()
        .find(|p| p.text == "two")
        .map(|p| p.bold);
    assert_eq!(bold, Some(true));
}

#[test]
fn a_plain_cue_draws_a_bold_word_bold() {
    let word = SubtitleStyle {
        bold: Some(true),
        ..SubtitleStyle::default()
    };
    let shown = crate::subtitles::shown(
        "one two",
        &[],
        &[Some(word), None],
        &SubtitleStyle::default(),
        0,
        25.0,
        None,
    );
    let bold = shown
        .pieces
        .iter()
        .find(|p| p.text == "one")
        .map(|p| p.bold);
    assert_eq!(bold, Some(true));
}

#[test]
fn a_bold_run_closes_around_a_word_that_is_not_bold() {
    let word = SubtitleStyle {
        bold: Some(false),
        ..SubtitleStyle::default()
    };
    let text = crate::subtitles::marked(
        "<b>one two</b>",
        &SubtitleStyle::default(),
        &[None, Some(word)],
    );
    assert_eq!(text, "<b>one </b>two");
}

#[test]
fn an_imsc1_file_writes_one_words_style() {
    let word = SubtitleStyle {
        color: Some("#ff0000".into()),
        size: Some(0.11),
        bold: Some(true),
        case: Some(LetterCase::Upper),
        ..SubtitleStyle::default()
    };
    let text = write(
        &[Timed {
            start: 1.0,
            end: 2.0,
            text: "one two".into(),
            place: Place::Bottom,
            words: Vec::new(),
            speaker: None,
            style: None,
            word_styles: vec![None, Some(word)],
        }],
        SubtitleFormat::Ttml,
        &Writing {
            language: None,
            frame_rate: None,
            positions: false,
            style: &SubtitleStyle::default(),
        },
    )
    .text;
    let want =
        "one <span tts:color=\"#ff0000\" tts:fontSize=\"1100%\" tts:fontWeight=\"bold\">TWO</span>";
    let at = text.find(want);
    assert_eq!(at.map(|i| &text[i..i + want.len()]), Some(want));
}

#[test]
fn webvtt_writes_a_cues_alignment_and_position() {
    let style = SubtitleStyle {
        align: Some(SubtitleAlign::Left),
        position: Some([0.25, 0.4]),
        ..SubtitleStyle::default()
    };
    let text = write(
        &[Timed {
            start: 1.0,
            end: 2.0,
            text: "Hi".into(),
            place: Place::Bottom,
            words: Vec::new(),
            speaker: None,
            style: Some(style),
            word_styles: Vec::new(),
        }],
        SubtitleFormat::Vtt,
        &Writing {
            language: None,
            frame_rate: None,
            positions: true,
            style: &SubtitleStyle::default(),
        },
    )
    .text;
    assert_eq!(
        text,
        "WEBVTT\n\n00:00:01.000 --> 00:00:02.000 position:25% line:40% align:left\nHi\n\n"
    );
}
