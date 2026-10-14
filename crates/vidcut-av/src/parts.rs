//! The parts of FFmpeg vidcut uses that a build may lack. Encoders and
//! filters from other libraries (x264, zimg, …) are there as FFmpeg was
//! configured; FFmpeg's own are in every usual build. `doctor` lists what
//! this one lacks, and a part missing where it is needed fails as
//! [`Error::Missing`], named with what it does and where it comes from.
//!
//! `VIDCUT_FFMPEG_WITHOUT` (names, comma-separated: `zscale,libx264`) makes
//! parts count as missing, to try a build without them.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::Serialize;

use crate::error::{Context, Error, Result};
use crate::ffmpeg;

/// What a part is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Encoder,
    Filter,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Encoder => "encoder",
            Self::Filter => "filter",
        }
    }
}

/// One of a part's encoders or filters: its name in FFmpeg and the
/// library it comes with (`None`: FFmpeg's own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Name {
    pub name: &'static str,
    pub library: Option<&'static str>,
}

/// A part of FFmpeg vidcut uses, by what it does: any of its names does
/// it, the first best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Part {
    pub kind: Kind,
    pub names: &'static [Name],
    /// What vidcut does with it.
    pub used_for: &'static str,
}

const fn lib(name: &'static str, library: &'static str) -> Name {
    Name {
        name,
        library: Some(library),
    }
}

const fn own(name: &'static str) -> Name {
    Name {
        name,
        library: None,
    }
}

const fn encoder(names: &'static [Name], used_for: &'static str) -> Part {
    Part {
        kind: Kind::Encoder,
        names,
        used_for,
    }
}

const fn filter(names: &'static [Name], used_for: &'static str) -> Part {
    Part {
        kind: Kind::Filter,
        names,
        used_for,
    }
}

pub const H264: Part = encoder(
    &[lib("libx264", "x264"), lib("libopenh264", "OpenH264")],
    "H.264 on the CPU",
);
pub const HEVC: Part = encoder(
    &[lib("libx265", "x265"), lib("libkvazaar", "Kvazaar")],
    "HEVC on the CPU",
);
pub const AV1: Part = encoder(
    &[
        lib("libsvtav1", "SVT-AV1"),
        lib("libaom-av1", "libaom"),
        lib("librav1e", "rav1e"),
    ],
    "AV1 on the CPU",
);
pub const VP9: Part = encoder(&[lib("libvpx-vp9", "libvpx")], "VP9");
pub const PRORES: Part = encoder(&[own("prores_ks"), own("prores_aw")], "ProRes");
pub const DNXHR: Part = encoder(&[own("dnxhd")], "DNxHR");
pub const AAC: Part = encoder(&[own("aac")], "AAC sound");
pub const OPUS: Part = encoder(&[lib("libopus", "libopus"), own("opus")], "Opus sound");
pub const MP3: Part = encoder(&[lib("libmp3lame", "LAME")], "MP3 sound");
pub const ZSCALE: Part = filter(&[lib("zscale", "zimg")], "faster HDR conversion");

/// The parts vidcut uses.
pub const PARTS: &[Part] = &[
    H264,
    HEVC,
    AV1,
    VP9,
    PRORES,
    DNXHR,
    AAC,
    OPUS,
    MP3,
    encoder(&[own("flac")], "FLAC sound"),
    encoder(&[own("pcm_s16le")], "PCM sound"),
    encoder(&[own("pcm_s24le")], "PCM sound"),
    encoder(&[own("png")], "stills and storyboards"),
    ZSCALE,
    filter(
        &[lib("libplacebo", "libplacebo")],
        "NVDEC's pictures converted on the GPU",
    ),
    filter(&[own("buffer")], "every picture conversion"),
    filter(&[own("buffersink")], "every picture conversion"),
    filter(&[own("abuffer")], "loudness"),
    filter(&[own("abuffersink")], "loudness"),
    filter(&[own("scale")], "every picture conversion"),
    filter(&[own("pad")], "every picture conversion"),
    filter(&[own("setsar")], "every picture conversion"),
    filter(&[own("format")], "every picture conversion"),
    filter(&[own("transpose")], "turning pictures upright"),
    filter(&[own("hflip")], "turning pictures upright"),
    filter(&[own("vflip")], "turning pictures upright"),
    filter(&[own("tonemap")], "HDR into SDR"),
    filter(&[own("bwdif")], "deinterlacing"),
    filter(&[own("ebur128")], "loudness"),
];

/// The names `VIDCUT_FFMPEG_WITHOUT` makes count as missing.
fn without() -> &'static HashSet<String> {
    static WITHOUT: OnceLock<HashSet<String>> = OnceLock::new();
    WITHOUT.get_or_init(|| {
        std::env::var("VIDCUT_FFMPEG_WITHOUT")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(String::from)
            .collect()
    })
}

/// Whether this FFmpeg has the encoder `name`.
pub fn has_encoder(name: &str) -> bool {
    !without().contains(name) && ffmpeg::encoder::find_by_name(name).is_some()
}

/// Whether this FFmpeg has the filter `name`.
pub fn has_filter(name: &str) -> bool {
    !without().contains(name) && ffmpeg::filter::find(name).is_some()
}

/// Whether this FFmpeg's filter `name` takes the option `option`: a newer
/// FFmpeg's filters do more, and say so in their options.
pub(crate) fn filter_has_option(name: &str, option: &str) -> bool {
    let Some(filter) = ffmpeg::filter::find(name) else {
        return false;
    };
    let option = std::ffi::CString::new(option).expect("an option's name");
    // SAFETY: the filter's own class, searched without an object as
    // FFmpeg's option listing searches it.
    unsafe {
        let mut class = (*filter.as_ptr()).priv_class;
        if class.is_null() {
            return false;
        }
        !ffmpeg::ffi::av_opt_find(
            (&raw mut class).cast(),
            option.as_ptr(),
            std::ptr::null(),
            0,
            ffmpeg::ffi::AV_OPT_SEARCH_FAKE_OBJ,
        )
        .is_null()
    }
}

fn has(kind: Kind, name: &str) -> bool {
    match kind {
        Kind::Encoder => has_encoder(name),
        Kind::Filter => has_filter(name),
    }
}

/// The first of `part`'s names this FFmpeg has.
pub fn first(part: &Part) -> Option<&'static str> {
    part.names
        .iter()
        .map(|n| n.name)
        .find(|n| has(part.kind, n))
}

/// The parts this FFmpeg lacks.
pub fn missing() -> Vec<Part> {
    PARTS
        .iter()
        .filter(|p| first(p).is_none())
        .copied()
        .collect()
}

/// The parts this FFmpeg does with other than their first name, and the
/// name it does each with.
pub fn stand_ins() -> Vec<(Part, &'static str)> {
    PARTS
        .iter()
        .filter_map(|p| first(p).filter(|n| *n != p.names[0].name).map(|n| (*p, n)))
        .collect()
}

/// The error for `part`, which this FFmpeg lacks, in words: what it
/// can't do, and what it was built without.
pub fn part_missing(part: &Part) -> Error {
    let without = |n: &Name| match n.library {
        Some(library) => format!("{library} ({})", n.name),
        None => format!("its {} {}", n.name, part.kind.name()),
    };
    let all: Vec<String> = part.names.iter().map(without).collect();
    let all = all.join(" or ");
    Error::Missing(match part.kind {
        Kind::Encoder => format!(
            "this FFmpeg can't encode {}: it was built without {all}",
            part.used_for
        ),
        Kind::Filter => format!(
            "this FFmpeg can't do {}: it was built without {all}",
            part.used_for
        ),
    })
}

/// The error for a `kind` named `name` this FFmpeg lacks: its part's, when
/// it is a part's only name.
pub fn missing_error(kind: Kind, name: &str) -> Error {
    match PARTS
        .iter()
        .find(|p| p.kind == kind && p.names.len() == 1 && p.names[0].name == name)
    {
        Some(part) => part_missing(part),
        None => Error::Missing(format!("this FFmpeg has no {name} {}", kind.name())),
    }
}

/// A decoder for a stream of `id` opened, or why not: none for its codec
/// in this FFmpeg fails as [`Error::Missing`].
pub(crate) fn decoder_opened<T>(
    opened: std::result::Result<T, ffmpeg::Error>,
    id: ffmpeg::codec::Id,
    what: &str,
) -> Result<T> {
    match opened {
        Err(ffmpeg::Error::DecoderNotFound) => Err(Error::Missing(format!(
            "this FFmpeg has no {} decoder",
            id.name()
        ))),
        r => r.ctx(|| format!("opening {what} decoder")),
    }
}

/// Parses `spec` into `graph`, between its `in` and `out`. A filter this
/// FFmpeg lacks fails as [`Error::Missing`].
pub(crate) fn parse_filters(graph: &mut ffmpeg::filter::Graph, spec: &str) -> Result<()> {
    if let Some(name) = filter_names(spec).find(|n| without().contains(*n)) {
        return Err(missing_error(Kind::Filter, name));
    }
    let parsed = graph
        .output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(spec));
    // FFmpeg fails an unknown filter as not found or as an invalid
    // argument, depending on where its parser meets it.
    if parsed.is_err()
        && let Some(name) = filter_names(spec).find(|n| ffmpeg::filter::find(n).is_none())
    {
        return Err(missing_error(Kind::Filter, name));
    }
    parsed.ctx(|| format!("building filter {spec}"))
}

/// The filters `spec` names, in order: `scale` of `[a]scale=w=1:h=2`.
fn filter_names(spec: &str) -> impl Iterator<Item = &str> {
    spec.split([',', ';']).filter_map(|f| {
        let mut f = f.trim();
        while let Some(rest) = f.strip_prefix('[') {
            f = rest
                .split_once(']')
                .map_or("", |(_, after)| after)
                .trim_start();
        }
        let name = f.split(['=', '@']).next().unwrap_or("").trim();
        (!name.is_empty()).then_some(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_are_named_without_labels_options_or_instances() {
        let names: Vec<&str> =
            filter_names("[in]scale=320:180,pad=w=1:h=2 ; zscale@a=t=linear,format=rgba[out]")
                .collect();
        assert_eq!(names, ["scale", "pad", "zscale", "format"]);
    }

    #[test]
    fn a_filter_this_ffmpeg_lacks_is_named() {
        crate::init();
        let mut graph = ffmpeg::filter::Graph::new();
        let args = "video_size=16x16:pix_fmt=0:time_base=1/25:pixel_aspect=1/1";
        graph
            .add(&ffmpeg::filter::find("buffer").unwrap(), "in", args)
            .unwrap();
        graph
            .add(&ffmpeg::filter::find("buffersink").unwrap(), "out", "")
            .unwrap();
        let e = parse_filters(&mut graph, "scale=8:8,vidcut_nosuch=1").unwrap_err();
        assert!(matches!(e, Error::Missing(_)), "{e:?}");
        assert_eq!(e.to_string(), "this FFmpeg has no vidcut_nosuch filter");
    }
}
