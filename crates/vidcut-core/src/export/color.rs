//! The output's dynamic range and bit depth, chosen from the pictures an
//! export shows (decision D44).
//!
//! `video.hdr = auto` makes HDR when most of the shown picture is HDR and
//! the codec can carry it; an unset `video.bit_depth` keeps 10 bits when
//! most of the picture has them and the codec's 10 bits play widely.
//! Explicit values win. What is decided is pure: the facts about each
//! piece's pictures and what the encoders here can write come from the
//! caller.

use serde::Serialize;

use super::{HdrMode, VideoCodec, VideoSettings};
use crate::model::Hdr;
use crate::render::VideoPiece;

/// What a source piece's pictures are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PictureKind {
    /// The HDR transfer; `None` for SDR.
    pub hdr: Option<Hdr>,
    /// Bits per sample of the decoded pictures.
    pub depth: u8,
}

/// How much of an output's picture is HDR and how much has 10 bits or more,
/// in frames. Black (gaps, black pieces) shows no source and isn't counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PictureShare {
    /// Frames that show a source picture.
    pub frames: i64,
    /// Of them, frames in HDR with PQ.
    pub pq: i64,
    /// Frames in HDR with HLG.
    pub hlg: i64,
    /// Frames with 10 bits or more.
    pub deep: i64,
}

impl PictureShare {
    /// HDR frames.
    pub fn hdr(&self) -> i64 {
        self.pq + self.hlg
    }

    /// Adds another output's picture.
    pub fn add(&mut self, other: &PictureShare) {
        self.frames += other.frames;
        self.pq += other.pq;
        self.hlg += other.hlg;
        self.deep += other.deep;
    }

    /// More than half of the shown picture.
    fn most(&self, n: i64) -> bool {
        n * 2 > self.frames
    }

    /// `n` frames as a share of the shown picture, in words.
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

    /// The HDR transfer to keep: HLG when all HDR frames are HLG, else PQ.
    pub fn transfer(&self) -> Hdr {
        if self.pq == 0 && self.hlg > 0 {
            Hdr::Hlg
        } else {
            Hdr::Pq
        }
    }
}

/// Adds up what `video`'s source pieces show.
pub fn picture_share(
    video: &[VideoPiece],
    mut kind: impl FnMut(&VideoPiece) -> PictureKind,
) -> PictureShare {
    let mut share = PictureShare::default();
    for piece in video {
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

/// What the encoders an export may use can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Abilities {
    /// Some encoder of the codec, among those `video.encoder` allows, writes
    /// 10 bits (HDR needs it).
    pub ten_bit: bool,
    /// The encoder an 8-bit export would start with writes 10 bits too:
    /// SDR takes 10 bits only from it, rather than moving to a slower one.
    pub ten_bit_first: bool,
    /// That encoder's name, for the reasons (`hevc_nvenc`).
    pub first: Option<&'static str>,
}

/// An output's dynamic range and bit depth, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColorChoice {
    /// `sdr`, `pq` or `hlg`.
    pub hdr: HdrMode,
    pub bit_depth: u8,
    /// What was asked for: `video.hdr` and `video.bit_depth`.
    pub asked: Asked,
    /// The shown picture it was chosen from.
    pub picture: PictureShare,
    /// Why, in words: the dynamic range, then the depth.
    pub why: Vec<String>,
    /// HDR frames tone-mapped into SDR.
    pub tone_mapped: i64,
}

/// The settings a [`ColorChoice`] was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Asked {
    pub hdr: HdrMode,
    pub bit_depth: Option<u8>,
}

/// The codecs that carry HDR (BT.2100 in 10 bits).
pub fn carries_hdr(codec: VideoCodec) -> bool {
    matches!(codec, VideoCodec::Hevc | VideoCodec::Av1 | VideoCodec::Vp9)
}

fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "H.264",
        VideoCodec::Hevc => "HEVC",
        VideoCodec::Av1 => "AV1",
        VideoCodec::Vp9 => "VP9",
        VideoCodec::Prores => "ProRes",
        VideoCodec::Dnxhr => "DNxHR",
    }
}

fn transfer_name(h: Hdr) -> &'static str {
    match h {
        Hdr::Pq => "PQ",
        Hdr::Hlg => "HLG",
    }
}

/// A depth the codec and profile fix, and why.
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

/// Chooses `v`'s dynamic range and bit depth for an output showing `share`
/// (decision D44).
pub fn choose(v: &VideoSettings, share: &PictureShare, can: Abilities) -> ColorChoice {
    let asked_depth = v
        .bit_depth
        .map(|d| (d, "asked for"))
        .or_else(|| fixed_depth(v));
    let codec = codec_name(v.codec);
    let hdr_part = |s: &PictureShare| match (s.pq > 0, s.hlg > 0) {
        (true, true) => format!("{} of the picture is HDR", s.percent(s.hdr())),
        (true, false) => format!("{} of the picture is HDR (PQ)", s.percent(s.pq)),
        _ => format!("{} of the picture is HDR (HLG)", s.percent(s.hlg)),
    };
    let (hdr, hdr_why): (Option<Hdr>, String) = match v.hdr {
        HdrMode::Pq => (Some(Hdr::Pq), "HDR (PQ): asked for".into()),
        HdrMode::Hlg => (Some(Hdr::Hlg), "HDR (HLG): asked for".into()),
        HdrMode::Sdr => (None, "SDR: asked for".into()),
        HdrMode::Auto if share.hdr() == 0 => (None, "SDR: no HDR picture".into()),
        HdrMode::Auto if !share.most(share.hdr()) => {
            (None, format!("SDR: only {}, half or less", hdr_part(share)))
        }
        HdrMode::Auto if !carries_hdr(v.codec) => (
            None,
            format!("SDR: {} but {codec} can't carry HDR", hdr_part(share)),
        ),
        HdrMode::Auto if asked_depth.is_some_and(|(d, _)| d == 8) => (
            None,
            format!(
                "SDR: {} but 8 bits are set ({})",
                hdr_part(share),
                asked_depth.map_or("", |(_, w)| w)
            ),
        ),
        HdrMode::Auto if !can.ten_bit => (
            None,
            format!(
                "SDR: {} but no 10-bit {codec} encoder works here",
                hdr_part(share)
            ),
        ),
        HdrMode::Auto => {
            let t = share.transfer();
            (
                Some(t),
                format!("HDR ({}): {}", transfer_name(t), hdr_part(share)),
            )
        }
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
        tone_mapped: if hdr.is_none() { share.hdr() } else { 0 },
    }
}

impl ColorChoice {
    /// The settings with this choice in place of `auto` and an unset depth:
    /// what the encoder runs with. DNxHR HQ in 10 bits is HQX, its 10-bit
    /// form at the same data rate.
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

    /// Frames converted between transfers: HDR into SDR, and SDR or the
    /// other HDR transfer into HDR.
    pub fn converted(&self) -> i64 {
        let p = &self.picture;
        match self.hdr {
            HdrMode::Pq => p.frames - p.pq,
            HdrMode::Hlg => p.frames - p.hlg,
            HdrMode::Sdr | HdrMode::Auto => p.hdr(),
        }
    }

    /// HDR picture is tone-mapped although SDR wasn't asked for: worth a
    /// quiet note (decision D44).
    pub fn worth_a_note(&self) -> bool {
        self.tone_mapped > 0 && self.asked.hdr != HdrMode::Sdr
    }

    /// The `--set` overrides that would keep the HDR picture HDR.
    pub fn keep_hdr(&self, v: &VideoSettings) -> Vec<String> {
        keep_hdr(v, self.picture.transfer())
    }
}

/// The `--set` overrides that make `v` export HDR with `transfer`: the
/// transfer, a codec that carries it when `v`'s can't (HEVC), and no
/// profile or depth that rules out 10 bits.
pub fn keep_hdr(v: &VideoSettings, transfer: Hdr) -> Vec<String> {
    keep_hdr_settings(v, transfer).0
}

/// [`keep_hdr`]'s overrides, and the settings they make of `v`.
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
    use crate::export::{EncoderChoice, builtin_presets};

    fn piece(media: &str, frames: i64) -> VideoPiece {
        VideoPiece::Source {
            media: media.into(),
            path: PathBuf::from(format!("/{media}.mov")),
            origin: 0.0,
            source_frame: 0,
            frames,
            image: None,
        }
    }

    /// `m` pieces: `pq…`, `hlg…`, `deep…` (10-bit SDR), anything else 8-bit SDR.
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
                    VideoPiece::Black { frames: n }
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

    const ALL: Abilities = Abilities {
        ten_bit: true,
        ten_bit_first: true,
        first: Some("libx265"),
    };

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
    fn frames_converted_between_transfers_are_counted() {
        let s = share(&[("pq", 30), ("hlg", 20), ("sdr", 10), ("black", 5)]);
        let into = |hdr| ColorChoice {
            hdr,
            ..choose(&video(VideoCodec::Hevc), &s, ALL)
        };
        assert_eq!(into(HdrMode::Sdr).converted(), 50);
        assert_eq!(into(HdrMode::Pq).converted(), 30);
        assert_eq!(into(HdrMode::Hlg).converted(), 40);
        let sdr = share(&[("sdr", 10), ("deep", 10)]);
        assert_eq!(choose(&video(VideoCodec::H264), &sdr, ALL).converted(), 0);
    }

    #[test]
    fn an_hdr_timeline_stays_hdr_where_the_codec_carries_it() {
        // Decision D44: auto keeps HDR in HEVC, AV1 and VP9, in 10 bits.
        let s = share(&[("pq", 100)]);
        for codec in [VideoCodec::Hevc, VideoCodec::Av1, VideoCodec::Vp9] {
            let c = choose(&video(codec), &s, ALL);
            assert_eq!((c.hdr, c.bit_depth, c.tone_mapped), (HdrMode::Pq, 10, 0));
            assert!(!c.worth_a_note());
        }
        // H.264 can't: tone-mapped SDR in 8 bits, with a note that names
        // the way to keep it.
        let v = video(VideoCodec::H264);
        let c = choose(&v, &s, ALL);
        assert_eq!((c.hdr, c.bit_depth, c.tone_mapped), (HdrMode::Sdr, 8, 100));
        assert!(c.why[0].contains("H.264 can't carry HDR"), "{:?}", c.why);
        assert!(c.worth_a_note());
        assert_eq!(c.keep_hdr(&v), ["video.codec=hevc", "video.hdr=pq"]);
        // So can't ProRes, which stays 10-bit.
        let c = choose(&video(VideoCodec::Prores), &s, ALL);
        assert_eq!((c.hdr, c.bit_depth), (HdrMode::Sdr, 10));
    }

    #[test]
    fn most_of_the_picture_decides() {
        let v = video(VideoCodec::Hevc);
        let c = choose(&v, &share(&[("pq", 51), ("sdr", 49)]), ALL);
        assert_eq!(c.hdr, HdrMode::Pq);
        // Half isn't most: SDR, and the HDR half is tone-mapped.
        let c = choose(&v, &share(&[("pq", 50), ("sdr", 50)]), ALL);
        assert_eq!((c.hdr, c.tone_mapped), (HdrMode::Sdr, 50));
        assert!(c.why[0].contains("half or less"), "{:?}", c.why);
        // Black doesn't dilute it.
        let c = choose(&v, &share(&[("pq", 30), ("black", 500), ("sdr", 20)]), ALL);
        assert_eq!(c.hdr, HdrMode::Pq);
        // HLG only when every HDR frame is HLG; PQ and HLG mixed make PQ.
        let c = choose(&v, &share(&[("hlg", 30), ("sdr", 20)]), ALL);
        assert_eq!(c.hdr, HdrMode::Hlg);
        let c = choose(&v, &share(&[("hlg", 30), ("pq", 1), ("sdr", 20)]), ALL);
        assert_eq!(c.hdr, HdrMode::Pq);
        // Nothing but black: SDR, 8 bits, nothing to note.
        let c = choose(&v, &share(&[("black", 50)]), ALL);
        assert_eq!(
            (c.hdr, c.bit_depth, c.worth_a_note()),
            (HdrMode::Sdr, 8, false)
        );
    }

    #[test]
    fn sdr_depth_follows_the_picture_where_ten_bits_play() {
        let deep = share(&[("deep", 80), ("sdr", 20)]);
        for (codec, want) in [
            (VideoCodec::Hevc, 10),
            (VideoCodec::Av1, 10),
            (VideoCodec::Vp9, 10),
            (VideoCodec::H264, 8),
            (VideoCodec::Prores, 10),
        ] {
            let c = choose(&video(codec), &deep, ALL);
            assert_eq!((c.hdr, c.bit_depth), (HdrMode::Sdr, want), "{codec:?}");
        }
        let c = choose(
            &video(VideoCodec::Hevc),
            &share(&[("deep", 40), ("sdr", 60)]),
            ALL,
        );
        assert_eq!(c.bit_depth, 8);
        // DNxHR HQ becomes HQX, its 10-bit form; LB has none.
        let mut v = video(VideoCodec::Dnxhr);
        v.profile = Some("hq".into());
        let c = choose(&v, &deep, ALL);
        assert_eq!(c.bit_depth, 10);
        c.apply(&mut v);
        assert_eq!(v.profile.as_deref(), Some("hqx"));
        let mut v = video(VideoCodec::Dnxhr);
        v.profile = Some("lb".into());
        assert_eq!(choose(&v, &deep, ALL).bit_depth, 8);
    }

    #[test]
    fn what_the_encoders_can_write_limits_it() {
        let v = video(VideoCodec::Hevc);
        // The encoder an 8-bit export starts with can't write 10 bits: SDR
        // stays 8 rather than move to a slower encoder.
        let first_only_8 = Abilities {
            ten_bit: true,
            ten_bit_first: false,
            first: Some("hevc_vaapi"),
        };
        let c = choose(&v, &share(&[("deep", 100)]), first_only_8);
        assert_eq!(c.bit_depth, 8);
        assert!(
            c.why[1].contains("hevc_vaapi can't write 10"),
            "{:?}",
            c.why
        );
        // HDR takes any encoder that writes 10 bits.
        let c = choose(&v, &share(&[("pq", 100)]), first_only_8);
        assert_eq!((c.hdr, c.bit_depth), (HdrMode::Pq, 10));
        // None does: SDR.
        let none = Abilities {
            ten_bit: false,
            ten_bit_first: false,
            first: None,
        };
        let c = choose(&v, &share(&[("pq", 100)]), none);
        assert_eq!((c.hdr, c.bit_depth), (HdrMode::Sdr, 8));
        assert!(c.why[0].contains("no 10-bit HEVC encoder"), "{:?}", c.why);
    }

    #[test]
    fn explicit_values_win() {
        let hdr = share(&[("pq", 100)]);
        let sdr = share(&[("sdr", 100)]);
        let mut v = video(VideoCodec::Hevc);
        v.hdr = HdrMode::Sdr;
        let c = choose(&v, &hdr, ALL);
        assert_eq!((c.hdr, c.bit_depth, c.tone_mapped), (HdrMode::Sdr, 10, 100));
        // SDR asked for: the facts are reported, but no note.
        assert!(!c.worth_a_note());
        v.hdr = HdrMode::Hlg;
        assert_eq!(choose(&v, &sdr, ALL).hdr, HdrMode::Hlg);
        v.hdr = HdrMode::Auto;
        v.bit_depth = Some(8);
        let c = choose(&v, &hdr, ALL);
        assert_eq!((c.hdr, c.bit_depth), (HdrMode::Sdr, 8));
        assert_eq!(c.keep_hdr(&v), ["video.bit_depth=null", "video.hdr=pq"]);
        v.bit_depth = Some(10);
        assert_eq!(choose(&v, &sdr, ALL).bit_depth, 10);
        v.bit_depth = None;
        v.profile = Some("main".into());
        let c = choose(&v, &hdr, ALL);
        assert_eq!((c.hdr, c.bit_depth), (HdrMode::Sdr, 8));
        let mut h264 = video(VideoCodec::H264);
        h264.bit_depth = Some(10);
        assert_eq!(choose(&h264, &share(&[("deep", 1)]), ALL).bit_depth, 10);
    }

    #[test]
    fn applied_settings_are_concrete_and_valid() {
        // Every built-in preset, on each kind of timeline, resolves to
        // settings that name a dynamic range and a depth and validate.
        for p in builtin_presets() {
            let Some(v) = &p.settings.video else { continue };
            for s in [
                share(&[("pq", 100)]),
                share(&[("hlg", 100)]),
                share(&[("deep", 100)]),
                share(&[("sdr", 100)]),
            ] {
                let c = choose(v, &s, ALL);
                let mut settings = p.settings.clone();
                let resolved = settings.video.as_mut().unwrap();
                c.apply(resolved);
                assert_ne!(resolved.hdr, HdrMode::Auto);
                assert!(resolved.bit_depth.is_some());
                crate::export::validate(&settings)
                    .unwrap_or_else(|e| panic!("{} on {s:?}: {e}", p.name));
            }
        }
        // TikTok asks for SDR (phones); the masters follow the sources.
        let preset = |name: &str| {
            builtin_presets()
                .into_iter()
                .find(|p| p.name == name)
                .unwrap()
                .settings
                .video
                .unwrap()
        };
        let hdr = share(&[("pq", 100)]);
        assert_eq!(choose(&preset("tiktok"), &hdr, ALL).hdr, HdrMode::Sdr);
        assert!(!choose(&preset("tiktok"), &hdr, ALL).worth_a_note());
        for name in ["h265-master", "av1", "vp9-webm"] {
            assert_eq!(choose(&preset(name), &hdr, ALL).hdr, HdrMode::Pq, "{name}");
        }
        assert!(choose(&preset("youtube-1080p"), &hdr, ALL).worth_a_note());
    }
}
