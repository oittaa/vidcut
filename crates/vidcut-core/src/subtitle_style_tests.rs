use serde_json::{Value, json};

use crate::model::{
    Place, Project, Settings, SubtitleAlign, SubtitleBackground, SubtitleOutline, SubtitleStyle,
};
use crate::ops::subtitles::{TrackProps, add_track, checked_style, set_track};
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
            },
            Timed {
                start: 3.0,
                end: 4.0,
                text: "Middle".into(),
                place: Place::Middle,
                words: Vec::new(),
                speaker: None,
                style: None,
            },
            Timed {
                start: 5.0,
                end: 6.0,
                text: "Top".into(),
                place: Place::Top,
                words: Vec::new(),
                speaker: None,
                style: None,
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
