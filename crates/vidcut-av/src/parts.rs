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

/// A part of FFmpeg vidcut uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Part {
    pub kind: Kind,
    pub name: &'static str,
    /// The library it comes with; `None` for FFmpeg's own.
    pub library: Option<&'static str>,
    /// What vidcut does with it.
    pub used_for: &'static str,
}

const fn lib(
    kind: Kind,
    name: &'static str,
    library: &'static str,
    used_for: &'static str,
) -> Part {
    Part {
        kind,
        name,
        library: Some(library),
        used_for,
    }
}

const fn own(kind: Kind, name: &'static str, used_for: &'static str) -> Part {
    Part {
        kind,
        name,
        library: None,
        used_for,
    }
}

/// The parts from other libraries.
pub const OPTIONAL: &[Part] = &[
    lib(Kind::Encoder, "libx264", "x264", "H.264 on the CPU"),
    lib(Kind::Encoder, "libx265", "x265", "HEVC on the CPU"),
    lib(Kind::Encoder, "libsvtav1", "SVT-AV1", "AV1 on the CPU"),
    lib(Kind::Encoder, "libvpx-vp9", "libvpx", "VP9"),
    lib(Kind::Encoder, "libopus", "libopus", "Opus sound"),
    lib(Kind::Encoder, "libmp3lame", "LAME", "MP3 sound"),
    lib(Kind::Filter, "zscale", "zimg", "HDR conversion"),
    lib(
        Kind::Filter,
        "libplacebo",
        "libplacebo",
        "NVDEC's pictures converted on the GPU",
    ),
];

/// FFmpeg's own parts vidcut uses.
pub const OWN: &[Part] = &[
    own(Kind::Encoder, "aac", "AAC sound"),
    own(Kind::Encoder, "flac", "FLAC sound"),
    own(Kind::Encoder, "pcm_s16le", "PCM sound"),
    own(Kind::Encoder, "pcm_s24le", "PCM sound"),
    own(Kind::Encoder, "prores_ks", "ProRes"),
    own(Kind::Encoder, "dnxhd", "DNxHR"),
    own(Kind::Encoder, "png", "stills and storyboards"),
    own(Kind::Filter, "buffer", "every picture conversion"),
    own(Kind::Filter, "buffersink", "every picture conversion"),
    own(Kind::Filter, "abuffer", "loudness"),
    own(Kind::Filter, "abuffersink", "loudness"),
    own(Kind::Filter, "scale", "every picture conversion"),
    own(Kind::Filter, "pad", "every picture conversion"),
    own(Kind::Filter, "setsar", "every picture conversion"),
    own(Kind::Filter, "format", "every picture conversion"),
    own(Kind::Filter, "transpose", "turning pictures upright"),
    own(Kind::Filter, "hflip", "turning pictures upright"),
    own(Kind::Filter, "vflip", "turning pictures upright"),
    own(Kind::Filter, "tonemap", "HDR into SDR"),
    own(Kind::Filter, "bwdif", "deinterlacing"),
    own(Kind::Filter, "ebur128", "loudness"),
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

fn has(part: &Part) -> bool {
    match part.kind {
        Kind::Encoder => has_encoder(part.name),
        Kind::Filter => has_filter(part.name),
    }
}

/// The parts this FFmpeg lacks, FFmpeg's own first.
pub fn missing() -> Vec<Part> {
    OWN.iter()
        .chain(OPTIONAL)
        .filter(|p| !has(p))
        .copied()
        .collect()
}

/// The error for a `kind` named `name` this FFmpeg lacks.
pub fn missing_error(kind: Kind, name: &str) -> Error {
    let k = kind.name();
    let part = OWN
        .iter()
        .chain(OPTIONAL)
        .find(|p| p.kind == kind && p.name == name);
    Error::Missing(match part {
        Some(Part {
            library: Some(library),
            used_for,
            ..
        }) => format!("this FFmpeg has no {name} {k} ({used_for}; it comes with {library})"),
        Some(Part { used_for, .. }) => format!("this FFmpeg has no {name} {k} ({used_for})"),
        None => format!("this FFmpeg has no {name} {k}"),
    })
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
    fn a_missing_part_says_what_it_does_and_where_it_comes_from() {
        let e = missing_error(Kind::Filter, "zscale").to_string();
        assert_eq!(
            e,
            "this FFmpeg has no zscale filter (HDR conversion; it comes with zimg)"
        );
        let e = missing_error(Kind::Encoder, "png").to_string();
        assert_eq!(e, "this FFmpeg has no png encoder (stills and storyboards)");
        let e = missing_error(Kind::Filter, "nosuch").to_string();
        assert_eq!(e, "this FFmpeg has no nosuch filter");
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

    /// FFmpeg's own parts are in the builds the tests run on: the list
    /// names real parts.
    #[test]
    fn ffmpegs_own_parts_are_here() {
        crate::init();
        let lacking: Vec<&str> = OWN.iter().filter(|p| !has(p)).map(|p| p.name).collect();
        assert!(lacking.is_empty(), "{lacking:?}");
    }
}
