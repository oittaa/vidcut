use std::collections::VecDeque;

use serde::Serialize;
use vidcut_core::model::{Place, SubtitleStyle};
use vidcut_core::subtitles::{Tag, Token, three_letter, tokens};
use vidcut_core::time::Rational;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, ffi, format};

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: i64,
    pub end: i64,
    pub text: String,
    pub place: Place,
    pub words: Vec<i64>,
    pub word_styles: Vec<Option<SubtitleStyle>>,
    pub color: Option<String>,
    pub style: Option<SubtitleStyle>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub language: Option<String>,
    pub shown: bool,
    pub style: SubtitleStyle,
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Subtitles {
    pub streams: Vec<Track>,
    pub burn: Option<Track>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamNote {
    pub track: String,
    pub codec: String,
    pub cues: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BurnNote {
    pub track: String,
    pub cues: usize,
}

const HEADER: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 384\nPlayResY: 288\n\
ScaledBorderAndShadow: yes\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, \
SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, \
Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,16,&Hffffff,&Hffffff,&H0,&H0,0,0,0,0,100,100,0,0,1,1,0,2,10,10,10,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

pub(crate) fn codec_id(name: &str) -> Option<ffmpeg::codec::Id> {
    match name {
        "mov_text" => Some(ffmpeg::codec::Id::MOV_TEXT),
        "subrip" => Some(ffmpeg::codec::Id::SUBRIP),
        "webvtt" => Some(ffmpeg::codec::Id::WEBVTT),
        _ => None,
    }
}

fn container_codec(muxer: &str) -> Option<&'static str> {
    match muxer {
        "mp4" | "mov" | "ipod" => Some("mov_text"),
        "matroska" => Some("subrip"),
        "webm" => Some("webvtt"),
        _ => None,
    }
}

fn escape(text: &str, html: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push_str("\\N"),
            '\\' if matches!(chars.peek(), Some('n' | 'N')) => out.push_str("\\\u{2060}"),
            '{' if chars.peek() == Some(&'\\') => out.push_str("{\u{2060}"),
            '&' if html => out.push_str("&amp;"),
            '<' if html => out.push_str("&lt;"),
            '>' if html => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

fn turn_on(tag: &Tag) -> String {
    match tag {
        Tag::I => "{\\i1}".into(),
        Tag::B => "{\\b1}".into(),
        Tag::U => "{\\u1}".into(),
        Tag::Color(hex) => {
            let hex = hex.trim_start_matches('#');
            let part = |at: usize| hex.get(at..at + 2).unwrap_or("ff");
            format!("{{\\c&H{}{}{}&}}", part(4), part(2), part(0))
        }
    }
}

fn word_override(cue: &SubtitleStyle, word: &SubtitleStyle) -> String {
    let mut out = String::new();
    if word.bold == Some(true) {
        out.push_str("{\\b1}");
    }
    if word.color.is_some() {
        let [r, g, b] = cue.for_cue(Some(word)).ink();
        out.push_str(&format!("{{\\c&H{b:02x}{g:02x}{r:02x}&}}"));
    }
    if let Some(font) = &word.font {
        out.push_str(&format!("{{\\fn{font}}}"));
    }
    if let Some(size) = word.size {
        let px = (size * 288.0).round().max(1.0) as i64;
        out.push_str(&format!("{{\\fs{px}}}"));
    }
    out
}

fn dialog_words(
    mut out: String,
    text: &str,
    html: bool,
    word_styles: &[Option<SubtitleStyle>],
    cue: &SubtitleStyle,
) -> String {
    let mut current = None;
    for (piece, word) in vidcut_core::subtitles::parts(text) {
        if word != current {
            if current.is_some() {
                out.push_str("{\\r}");
            }
            current = word;
            if let Some(over) = word
                .and_then(|i| word_styles.get(i))
                .and_then(|s| s.as_ref())
            {
                out.push_str(&word_override(cue, over));
            }
        }
        let case = word
            .and_then(|i| word_styles.get(i))
            .and_then(|s| s.as_ref())
            .and_then(|s| s.case)
            .or(cue.case);
        let text = vidcut_core::subtitles::cased(&piece.text, case);
        if piece.italic {
            out.push_str("{\\i1}");
        }
        if piece.bold {
            out.push_str("{\\b1}");
        }
        if piece.underline {
            out.push_str("{\\u1}");
        }
        if let Some(color) = &piece.color {
            let hex = color.trim_start_matches('#');
            let part = |at: usize| hex.get(at..at + 2).unwrap_or("ff");
            out.push_str(&format!("{{\\c&H{}{}{}&}}", part(4), part(2), part(0)));
        }
        out.push_str(&escape(&text, html));
    }
    out
}

pub(crate) fn dialog(
    text: &str,
    place: Place,
    align: vidcut_core::model::SubtitleAlign,
    html: bool,
    word_styles: &[Option<SubtitleStyle>],
    cue: &SubtitleStyle,
) -> String {
    let mut out = String::from("0,0,Default,,0,0,0,,");
    if let Some(tag) = vidcut_core::subtitles::ass_align(place, align) {
        out.push_str(tag);
    }
    if word_styles.iter().any(|s| s.is_some()) {
        return dialog_words(out, text, html, word_styles, cue);
    }
    let mut open: Vec<Tag> = Vec::new();
    for t in tokens(text) {
        match t {
            Token::Text(s) => out.push_str(&escape(&s, html)),
            Token::Open(tag) => {
                out.push_str(&turn_on(&tag));
                open.push(tag);
            }
            Token::Close(tag) => {
                if let Some(at) = open.iter().rposition(|o| o.same(&tag)) {
                    open.remove(at);
                }
                match tag {
                    Tag::I => out.push_str("{\\i0}"),
                    Tag::B => out.push_str("{\\b0}"),
                    Tag::U => out.push_str("{\\u0}"),
                    Tag::Color(_) => {
                        out.push_str("{\\r}");
                        for o in &open {
                            out.push_str(&turn_on(o));
                        }
                    }
                }
            }
        }
    }
    out
}

pub(crate) fn segments(cues: &[Cue], by_place: bool) -> Vec<Cue> {
    let mut times: Vec<i64> = cues.iter().flat_map(|c| [c.start, c.end]).collect();
    times.sort_unstable();
    times.dedup();
    let places: &[Place] = if by_place {
        &[Place::Top, Place::Middle, Place::Bottom]
    } else {
        &[Place::Bottom]
    };
    let mut out: Vec<Cue> = Vec::new();
    for w in times.windows(2) {
        let (a, b) = (w[0], w[1]);
        for &place in places {
            let active: Vec<&Cue> = cues
                .iter()
                .filter(|c| c.start <= a && c.end >= b && (!by_place || c.place == place))
                .collect();
            let Some(first) = active.first() else {
                continue;
            };
            let text = active
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let place = if by_place { place } else { first.place };
            let style = first.style.clone();
            let word_styles = if active.len() == 1 {
                first.word_styles.clone()
            } else {
                Vec::new()
            };
            match out.iter_mut().rev().find(|c| c.place == place || !by_place) {
                Some(last)
                    if last.end == a
                        && last.text == text
                        && last.place == place
                        && last.style == style
                        && last.word_styles == word_styles =>
                {
                    last.end = b;
                }
                _ => out.push(Cue {
                    start: a,
                    end: b,
                    text,
                    place,
                    words: Vec::new(),
                    word_styles,
                    color: None,
                    style,
                }),
            }
        }
    }
    out.sort_by_key(|c| c.start);
    out
}

fn encode_one(ctx: *mut ffi::AVCodecContext, line: &str, buf: &mut [u8]) -> Result<usize> {
    let line = std::ffi::CString::new(line.replace('\0', ""))
        .map_err(|_| Error::Invalid("a subtitle's text has a NUL".into()))?;
    // SAFETY: a zeroed rect is a valid empty one; the encoder only reads `ass`.
    let mut rect: ffi::AVSubtitleRect = unsafe { std::mem::zeroed() };
    rect.type_ = ffi::AVSubtitleType::SUBTITLE_ASS;
    rect.ass = line.as_ptr().cast_mut();
    let mut rects = [&raw mut rect];
    // SAFETY: a zeroed subtitle is a valid empty one.
    let mut sub: ffi::AVSubtitle = unsafe { std::mem::zeroed() };
    sub.format = 1;
    sub.num_rects = 1;
    sub.rects = rects.as_mut_ptr();
    let size = i32::try_from(buf.len()).unwrap_or(i32::MAX);
    // SAFETY: an open subtitle encoder; `sub` and its rect outlive the call, `buf` holds `size` bytes.
    let n = unsafe { ffi::avcodec_encode_subtitle(ctx, buf.as_mut_ptr(), size, &sub) };
    if n < 0 {
        return Err(Error::Ffmpeg {
            context: "encoding a subtitle".into(),
            source: ffmpeg::Error::from(n),
        });
    }
    Ok(n as usize)
}

fn settings(place: Place, style: &SubtitleStyle) -> Option<Vec<u8>> {
    let mut parts = Vec::new();
    match style.position {
        Some([x, y]) => {
            parts.push(format!("position:{}%", (x * 100.0).round() as i64));
            parts.push(format!("line:{}%", (y * 100.0).round() as i64));
        }
        None => match place {
            Place::Top => parts.push("line:0".into()),
            Place::Middle => parts.push("line:50%".into()),
            Place::Bottom => {}
        },
    }
    match style.line_align() {
        vidcut_core::model::SubtitleAlign::Left => parts.push("align:left".into()),
        vidcut_core::model::SubtitleAlign::Right => parts.push("align:right".into()),
        vidcut_core::model::SubtitleAlign::Center => {}
    }
    (!parts.is_empty()).then(|| parts.join(" ").into_bytes())
}

pub(crate) struct SubtitleOut {
    stream: usize,
    frame_tb: ffmpeg::Rational,
    stream_tb: ffmpeg::Rational,
    packets: VecDeque<(i64, ffmpeg::Packet)>,
}

impl SubtitleOut {
    pub(crate) fn add(
        octx: &mut format::context::Output,
        track: &Track,
        fps: Rational,
    ) -> Result<(Self, StreamNote)> {
        let muxer = octx.format().name().to_string();
        let name = container_codec(&muxer)
            .ok_or_else(|| Error::Invalid(format!("{muxer} files don't hold subtitle streams")))?;
        let id = codec_id(name).expect("a known codec");
        let codec = ffmpeg::encoder::find(id)
            .ok_or_else(|| Error::Missing(format!("this FFmpeg has no {name} encoder")))?;
        let frame_tb = ffmpeg::Rational(fps.den() as i32, fps.num() as i32);
        let mut ctx = ffmpeg::codec::context::Context::new_with_codec(codec)
            .encoder()
            .subtitle()
            .ctx(|| format!("creating {name}"))?;
        ctx.set_time_base(frame_tb);
        // SAFETY: the header goes into memory FFmpeg allocates, which the context frees.
        unsafe {
            let raw = ctx.as_mut_ptr();
            let header = ffi::av_malloc(HEADER.len() + 1) as *mut u8;
            if header.is_null() {
                return Err(Error::Invalid("no memory for a subtitle header".into()));
            }
            std::ptr::copy_nonoverlapping(HEADER.as_ptr(), header, HEADER.len());
            *header.add(HEADER.len()) = 0;
            (*raw).subtitle_header = header;
            (*raw).subtitle_header_size = HEADER.len() as i32;
        }
        let mut encoder = ctx.open_as(codec).ctx(|| format!("opening {name}"))?;
        let nonstrict = octx.format().flags().contains(format::Flags::TS_NONSTRICT);
        let by_place = name != "mov_text" && nonstrict;
        let html = name == "webvtt";
        let mut buf = vec![0u8; 1 << 20];
        let mut packets = VecDeque::new();
        for cue in segments(&track.cues, by_place) {
            let look = track.style.for_cue(cue.style.as_ref());
            let line = dialog(
                &cue.text,
                cue.place,
                look.line_align(),
                html,
                &cue.word_styles,
                &look,
            );
            // SAFETY: `encoder` is open.
            let n = encode_one(unsafe { encoder.as_mut_ptr() }, &line, &mut buf)?;
            if n == 0 {
                continue;
            }
            let mut p = ffmpeg::Packet::copy(&buf[..n]);
            p.set_pts(Some(cue.start));
            p.set_dts(Some(cue.start));
            p.set_duration(cue.end - cue.start);
            if html && let Some(s) = settings(cue.place, &look) {
                // SAFETY: a live packet; the side data has room for `s`.
                unsafe {
                    let data = ffi::av_packet_new_side_data(
                        ffmpeg::packet::Mut::as_mut_ptr(&mut p),
                        ffi::AVPacketSideDataType::AV_PKT_DATA_WEBVTT_SETTINGS,
                        s.len(),
                    );
                    if data.is_null() {
                        return Err(Error::Invalid("no memory for a cue's settings".into()));
                    }
                    std::ptr::copy_nonoverlapping(s.as_ptr(), data, s.len());
                }
            }
            packets.push_back((cue.start, p));
        }
        let index = octx.nb_streams() as usize;
        let mut ost = octx
            .add_stream(codec)
            .ctx(|| "adding a subtitle stream".into())?;
        ost.set_parameters(&encoder);
        ost.set_time_base(frame_tb);
        let mut metadata = ffmpeg::Dictionary::new();
        metadata.set("title", &track.name);
        let terminology = name == "mov_text";
        if let Some(l) = track
            .language
            .as_deref()
            .and_then(|l| three_letter(l, terminology))
        {
            metadata.set("language", &l);
        }
        ost.set_metadata(metadata);
        if track.shown {
            // SAFETY: the stream just added, owned by `octx`.
            unsafe { (*ost.as_mut_ptr()).disposition |= ffi::AV_DISPOSITION_DEFAULT };
        }
        let note = StreamNote {
            track: track.id.clone(),
            codec: name.to_string(),
            cues: track.cues.len(),
        };
        Ok((
            Self {
                stream: index,
                frame_tb,
                stream_tb: frame_tb,
                packets,
            },
            note,
        ))
    }

    pub(crate) fn opened(&mut self, octx: &format::context::Output) {
        if let Some(s) = octx.stream(self.stream) {
            self.stream_tb = s.time_base();
        }
    }

    pub(crate) fn write_until(
        &mut self,
        octx: &mut format::context::Output,
        frame: i64,
    ) -> Result<()> {
        while self
            .packets
            .front()
            .is_some_and(|(start, _)| *start <= frame)
        {
            let (_, mut p) = self.packets.pop_front().expect("checked");
            p.set_stream(self.stream);
            p.rescale_ts(self.frame_tb, self.stream_tb);
            super::write_packet(octx, &p)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::dialog;
    use vidcut_core::model::{Place, SubtitleAlign, SubtitleStyle};

    #[test]
    fn a_stream_paints_one_word_and_leaves_the_other() {
        let cue = SubtitleStyle {
            color: Some("#ffffff".into()),
            ..SubtitleStyle::default()
        };
        let word = SubtitleStyle {
            color: Some("#ff0000".into()),
            bold: Some(true),
            ..SubtitleStyle::default()
        };
        let line = dialog(
            "one two",
            Place::Bottom,
            SubtitleAlign::Center,
            false,
            &[None, Some(word)],
            &cue,
        );
        assert_eq!(line, "0,0,Default,,0,0,0,,one{\\r} {\\b1}{\\c&H0000ff&}two");
    }
}
