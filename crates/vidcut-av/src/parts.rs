use std::collections::HashSet;
use std::sync::OnceLock;

use serde::Serialize;

use crate::error::{Context, Error, Result};
use crate::ffmpeg;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Name {
    pub name: &'static str,
    pub library: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Part {
    pub kind: Kind,
    pub names: &'static [Name],
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
    filter(&[own("buffer")], "every picture conversion"),
    filter(&[own("buffersink")], "every picture conversion"),
    filter(&[own("abuffer")], "loudness"),
    filter(&[own("abuffersink")], "loudness"),
    filter(&[own("scale")], "every picture conversion"),
    filter(&[own("format")], "every picture conversion"),
    filter(&[own("bwdif")], "deinterlacing"),
    filter(&[own("ebur128")], "loudness"),
];

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

pub fn has_encoder(name: &str) -> bool {
    !without().contains(name) && ffmpeg::encoder::find_by_name(name).is_some()
}

pub fn has_filter(name: &str) -> bool {
    !without().contains(name) && ffmpeg::filter::find(name).is_some()
}

pub(crate) fn filter_has_option(name: &str, option: &str) -> bool {
    let Some(filter) = ffmpeg::filter::find(name) else {
        return false;
    };
    let option = std::ffi::CString::new(option).expect("an option's name");
    // SAFETY: the filter's static class, not null, searched with `AV_OPT_SEARCH_FAKE_OBJ`.
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

pub fn first(part: &Part) -> Option<&'static str> {
    part.names
        .iter()
        .map(|n| n.name)
        .find(|n| has(part.kind, n))
}

pub fn missing() -> Vec<Part> {
    PARTS
        .iter()
        .filter(|p| first(p).is_none())
        .copied()
        .collect()
}

pub fn stand_ins() -> Vec<(Part, &'static str)> {
    PARTS
        .iter()
        .filter_map(|p| first(p).filter(|n| *n != p.names[0].name).map(|n| (*p, n)))
        .collect()
}

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

pub fn missing_error(kind: Kind, name: &str) -> Error {
    match PARTS
        .iter()
        .find(|p| p.kind == kind && p.names.len() == 1 && p.names[0].name == name)
    {
        Some(part) => part_missing(part),
        None => Error::Missing(format!("this FFmpeg has no {name} {}", kind.name())),
    }
}

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

pub(crate) fn parse_filters(graph: &mut ffmpeg::filter::Graph, spec: &str) -> Result<()> {
    if let Some(name) = filter_names(spec).find(|n| without().contains(*n)) {
        return Err(missing_error(Kind::Filter, name));
    }
    let parsed = graph
        .output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(spec));
    if parsed.is_err()
        && let Some(name) = filter_names(spec).find(|n| ffmpeg::filter::find(n).is_none())
    {
        return Err(missing_error(Kind::Filter, name));
    }
    parsed.ctx(|| format!("building filter {spec}"))
}

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
