use crate::model::{Place, Project, Settings, SubtitleBackground, SubtitleOutline, SubtitleStyle};
use crate::ops::subtitles::{TrackProps, add_track, checked_style, set_track};
use crate::request::SubtitleFormat;
use crate::subtitles::{Timed, write};

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
        }
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
        }],
        SubtitleFormat::Ttml,
        &crate::subtitles::Writing {
            language: Some("en"),
            frame_rate: Some(25.0),
            positions: true,
            style: &style,
        },
    );
    assert_eq!(
        text,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <tt xml:lang=\"en\" xmlns=\"http://www.w3.org/ns/ttml\" \
         xmlns:ttp=\"http://www.w3.org/ns/ttml#parameter\" \
         xmlns:tts=\"http://www.w3.org/ns/ttml#styling\" \
         xmlns:ittp=\"http://www.w3.org/ns/ttml/profile/imsc1#parameter\" \
         ttp:profile=\"http://www.w3.org/ns/ttml/profile/imsc1/text\" \
         ttp:timeBase=\"media\" ttp:frameRate=\"25\" ttp:cellResolution=\"50 100\">\n\
         \x20 <head>\n\
         \x20   <styling>\n\
         \x20     <style xml:id=\"s\" tts:color=\"#ffcc00\" tts:fontFamily=\"Arial\" \
         tts:fontSize=\"4c\" tts:textAlign=\"center\"/>\n\
         \x20   </styling>\n\
         \x20   <layout>\n\
         \x20     <region xml:id=\"bottom\" tts:origin=\"8% 0%\" tts:extent=\"84% 92%\" \
         tts:displayAlign=\"after\"/>\n\
         \x20     <region xml:id=\"middle\" tts:origin=\"8% 0%\" tts:extent=\"84% 100%\" \
         tts:displayAlign=\"center\"/>\n\
         \x20     <region xml:id=\"top\" tts:origin=\"8% 8%\" tts:extent=\"84% 92%\" \
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
