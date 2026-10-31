use serde::Serialize;

use super::{HdrMode, VideoCodec, VideoSettings};
use crate::model::Hdr;
use crate::render::VideoPiece;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PictureKind {
    pub hdr: Option<Hdr>,
    pub depth: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PictureShare {
    pub frames: i64,
    pub pq: i64,
    pub hlg: i64,
    pub deep: i64,
}

impl PictureShare {
    pub fn hdr(&self) -> i64 {
        self.pq + self.hlg
    }

    pub fn add(&mut self, other: &PictureShare) {
        self.frames += other.frames;
        self.pq += other.pq;
        self.hlg += other.hlg;
        self.deep += other.deep;
    }

    fn most(&self, n: i64) -> bool {
        n * 2 > self.frames
    }

    pub fn mostly_hdr(&self) -> bool {
        self.most(self.hdr())
    }

    fn percent(&self, n: i64) -> String {
        if self.frames == 0 {
            return "0%".into();
        }
        let p = n as f64 * 100.0 / self.frames as f64;
        match p {
            p if p > 0.0 && p < 1.0 => "under 1%".into(),
            p if p < 100.0 && p > 99.0 => "over 99%".into(),
            p => format!("{p:.0}%"),
        }
    }

    pub fn transfer(&self) -> Hdr {
        if self.pq == 0 && self.hlg > 0 {
            Hdr::Hlg
        } else {
            Hdr::Pq
        }
    }
}

pub fn picture_share(
    video: &[VideoPiece],
    mut kind: impl FnMut(&VideoPiece) -> PictureKind,
) -> PictureShare {
    let mut share = PictureShare::default();
    for (piece, _) in video.iter().flat_map(VideoPiece::layers) {
        let VideoPiece::Source { frames, .. } = piece else {
            continue;
        };
        let k = kind(piece);
        share.frames += frames;
        match k.hdr {
            Some(Hdr::Pq) => share.pq += frames,
            Some(Hdr::Hlg) => share.hlg += frames,
            None => {}
        }
        if k.depth >= 10 {
            share.deep += frames;
        }
    }
    share
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Abilities {
    pub ten_bit: bool,
    pub ten_bit_first: bool,
    pub first: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColorChoice {
    pub hdr: HdrMode,
    pub bit_depth: u8,
    pub asked: Asked,
    pub picture: PictureShare,
    pub why: Vec<String>,
    pub tone_mapped: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Asked {
    pub hdr: HdrMode,
    pub bit_depth: Option<u8>,
}

pub fn carries_hdr(codec: VideoCodec) -> bool {
    matches!(codec, VideoCodec::Hevc | VideoCodec::Av1 | VideoCodec::Vp9)
}

fn codec_name(codec: VideoCodec) -> &'static str {
    codec.title()
}

fn transfer_name(h: Hdr) -> &'static str {
    match h {
        Hdr::Pq => "PQ",
        Hdr::Hlg => "HLG",
    }
}

fn fixed_depth(v: &VideoSettings) -> Option<(u8, &'static str)> {
    match (v.codec, v.profile.as_deref()) {
        (VideoCodec::Prores, _) => Some((10, "ProRes is 10-bit")),
        (VideoCodec::Dnxhr, Some("hqx")) => Some((10, "DNxHR HQX is 10-bit")),
        (VideoCodec::Dnxhr, Some("444")) => Some((10, "DNxHR 444 is 10-bit")),
        (VideoCodec::Dnxhr, Some("lb")) => Some((8, "DNxHR LB is 8-bit")),
        (VideoCodec::Dnxhr, Some("sq")) => Some((8, "DNxHR SQ is 8-bit")),
        (VideoCodec::Hevc, Some("main10")) => Some((10, "the main10 profile is 10-bit")),
        (VideoCodec::Hevc, Some("main")) => Some((8, "the main profile is 8-bit")),
        _ => None,
    }
}

pub fn choose(
    v: &VideoSettings,
    share: &PictureShare,
    can: Abilities,
    timeline: Option<Hdr>,
) -> ColorChoice {
    let asked_depth = v
        .bit_depth
        .map(|d| (d, "asked for"))
        .or_else(|| fixed_depth(v));
    let codec = codec_name(v.codec);
    let timeline_part = |t: Hdr| format!("the timeline is HDR ({})", transfer_name(t));
    let (hdr, hdr_why): (Option<Hdr>, String) = match (v.hdr, timeline) {
        (HdrMode::Pq, _) => (Some(Hdr::Pq), "HDR (PQ): asked for".into()),
        (HdrMode::Hlg, _) => (Some(Hdr::Hlg), "HDR (HLG): asked for".into()),
        (HdrMode::Sdr, _) => (None, "SDR: asked for".into()),
        (HdrMode::Auto, None) => (None, "SDR: the timeline is SDR".into()),
        (HdrMode::Auto, Some(t)) if !carries_hdr(v.codec) => (
            None,
            format!("SDR: {} but {codec} can't carry HDR", timeline_part(t)),
        ),
        (HdrMode::Auto, Some(t)) if asked_depth.is_some_and(|(d, _)| d == 8) => (
            None,
            format!(
                "SDR: {} but 8 bits are set ({})",
                timeline_part(t),
                asked_depth.map_or("", |(_, w)| w)
            ),
        ),
        (HdrMode::Auto, Some(t)) if !can.ten_bit => (
            None,
            format!(
                "SDR: {} but no 10-bit {codec} encoder works here",
                timeline_part(t)
            ),
        ),
        (HdrMode::Auto, Some(t)) => (
            Some(t),
            format!("HDR ({}): as the timeline", transfer_name(t)),
        ),
    };
    let deep = || {
        format!(
            "{} of the picture has 10 bits or more",
            share.percent(share.deep)
        )
    };
    let (bit_depth, depth_why) = if hdr.is_some() {
        (10, "10 bits: HDR".to_string())
    } else if let Some((d, why)) = asked_depth {
        (d, format!("{d} bits: {why}"))
    } else if v.codec == VideoCodec::H264 {
        (8, "8 bits: H.264's 10 bits play in few places".into())
    } else if share.deep == 0 {
        (8, "8 bits: no picture has more".into())
    } else if !share.most(share.deep) {
        (8, format!("8 bits: only {}, half or less", deep()))
    } else if !can.ten_bit_first {
        let who = can.first.unwrap_or(codec);
        (8, format!("8 bits: {} but {who} can't write 10", deep()))
    } else {
        (10, format!("10 bits: {}", deep()))
    };
    ColorChoice {
        hdr: match hdr {
            Some(Hdr::Pq) => HdrMode::Pq,
            Some(Hdr::Hlg) => HdrMode::Hlg,
            None => HdrMode::Sdr,
        },
        bit_depth,
        asked: Asked {
            hdr: v.hdr,
            bit_depth: v.bit_depth,
        },
        picture: *share,
        why: vec![hdr_why, depth_why],
        tone_mapped: if hdr.is_none() && timeline.is_some() {
            share.hdr()
        } else {
            0
        },
    }
}

impl ColorChoice {
    pub fn apply(&self, v: &mut VideoSettings) {
        v.hdr = self.hdr;
        v.bit_depth = Some(self.bit_depth);
        if v.codec == VideoCodec::Dnxhr
            && self.bit_depth == 10
            && matches!(v.profile.as_deref(), None | Some("hq"))
        {
            v.profile = Some("hqx".into());
        }
    }

    pub fn worth_a_note(&self) -> bool {
        self.tone_mapped > 0 && self.asked.hdr != HdrMode::Sdr
    }

    pub fn keep_hdr(&self, v: &VideoSettings) -> Vec<String> {
        keep_hdr(v, self.picture.transfer())
    }
}

pub fn keep_hdr(v: &VideoSettings, transfer: Hdr) -> Vec<String> {
    keep_hdr_settings(v, transfer).0
}

pub fn keep_hdr_settings(v: &VideoSettings, transfer: Hdr) -> (Vec<String>, VideoSettings) {
    let mut set = Vec::new();
    let mut out = v.clone();
    if !carries_hdr(v.codec) {
        set.push("video.codec=hevc".to_string());
        out.codec = VideoCodec::Hevc;
        if v.profile.is_some() {
            set.push("video.profile=null".into());
            out.profile = None;
        }
    } else if v.codec == VideoCodec::Hevc && v.profile.as_deref() == Some("main") {
        set.push("video.profile=null".into());
        out.profile = None;
    }
    if v.bit_depth == Some(8) {
        set.push("video.bit_depth=null".into());
        out.bit_depth = None;
    }
    let (t, mode) = match transfer {
        Hdr::Pq => ("pq", HdrMode::Pq),
        Hdr::Hlg => ("hlg", HdrMode::Hlg),
    };
    set.push(format!("video.hdr={t}"));
    out.hdr = mode;
    (set, out)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::export::EncoderChoice;

    fn piece(media: &str, frames: i64) -> VideoPiece {
        VideoPiece::Source {
            pace: Default::default(),
            media: media.into(),
            path: PathBuf::from(format!("/{media}.mov")),
            origin: 0.0,
            source_frame: 0,
            frames,
            image: None,
            color_space: Default::default(),
            super_scale: Default::default(),
            retime: crate::model::Retime::Nearest,
            placed: Vec::new(),
            cache: Default::default(),
        }
    }

    fn kind(p: &VideoPiece) -> PictureKind {
        let VideoPiece::Source { media, .. } = p else {
            unreachable!()
        };
        match media.chars().next() {
            Some('p') => PictureKind {
                hdr: Some(Hdr::Pq),
                depth: 10,
            },
            Some('h') => PictureKind {
                hdr: Some(Hdr::Hlg),
                depth: 10,
            },
            Some('d') => PictureKind {
                hdr: None,
                depth: 10,
            },
            _ => PictureKind {
                hdr: None,
                depth: 8,
            },
        }
    }

    fn share(pieces: &[(&str, i64)]) -> PictureShare {
        let video: Vec<VideoPiece> = pieces
            .iter()
            .map(|&(m, n)| {
                if m == "black" {
                    VideoPiece::black(n)
                } else {
                    piece(m, n)
                }
            })
            .collect();
        picture_share(&video, kind)
    }

    fn video(codec: VideoCodec) -> VideoSettings {
        VideoSettings {
            codec,
            encoder: EncoderChoice::Auto,
            quality: None,
            bitrate: None,
            speed: None,
            profile: None,
            bit_depth: None,
            width: None,
            height: None,
            hdr: HdrMode::Auto,
            passes: None,
            first_pass: Default::default(),
            multipass: Default::default(),
            lookahead: None,
            hdr_metadata: Default::default(),
            mastering_display: None,
            max_cll: None,
        }
    }

    #[test]
    fn black_is_not_counted() {
        let s = share(&[("pq", 30), ("black", 100), ("sdr", 20), ("deep", 10)]);
        assert_eq!(
            s,
            PictureShare {
                frames: 60,
                pq: 30,
                hlg: 0,
                deep: 40
            }
        );
    }

    #[test]
    fn what_the_encoders_can_write_limits_it() {
        let v = video(VideoCodec::Hevc);
        let first_only_8 = Abilities {
            ten_bit: true,
            ten_bit_first: false,
            first: Some("hevc_vaapi"),
        };
        let c = choose(&v, &share(&[("deep", 100)]), first_only_8, None);
        assert_eq!(c.bit_depth, 8);
        assert!(
            c.why[1].contains("hevc_vaapi can't write 10"),
            "{:?}",
            c.why
        );
        let c = choose(&v, &share(&[("pq", 100)]), first_only_8, Some(Hdr::Pq));
        assert_eq!((c.hdr, c.bit_depth), (HdrMode::Pq, 10));
        let none = Abilities {
            ten_bit: false,
            ten_bit_first: false,
            first: None,
        };
        let c = choose(&v, &share(&[("pq", 100)]), none, Some(Hdr::Pq));
        assert_eq!((c.hdr, c.bit_depth), (HdrMode::Sdr, 8));
        assert!(c.why[0].contains("no 10-bit HEVC encoder"), "{:?}", c.why);
    }
}
