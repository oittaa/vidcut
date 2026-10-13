//! The HDR metadata an export writes, chosen from the pictures it shows
//! (decision D46).
//!
//! PQ output gets a mastering display and content light levels, given
//! (`video.mastering_display`, `video.max_cll`) or from the sources: the
//! widest color volume and the highest light levels of what it shows. HLG
//! and SDR output get none; light levels a source doesn't say are measured
//! from its frames. HDR10+ and Dolby Vision describe each frame's own
//! picture, kept as it was (the same transfer): HDR10+ is kept when some
//! shown picture has it, and frames without it get it made from their
//! measured light; Dolby Vision only when every shown picture has it, black
//! frames after one taking its RPU. What is decided is pure: the facts
//! about each piece come from the caller.

use serde::Serialize;

use super::{HdrMetadata, VideoCodec, VideoSettings};
use crate::error::{Error, Result};
use crate::model::{DolbyVision, Hdr, LightLevel, MasteringDisplay};
use crate::render::VideoPiece;

/// What a source piece's pictures carry.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PieceMeta {
    /// The HDR transfer; `None` for SDR.
    pub hdr: Option<Hdr>,
    pub mastering: Option<MasteringDisplay>,
    pub light: Option<LightLevel>,
    pub hdr10plus: bool,
    pub dolby_vision: Option<DolbyVision>,
    /// Its light levels in PQ output, measured from its frames.
    pub measured: Measured,
}

/// A piece's measured light levels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Measured {
    /// Not measured, nor to be.
    #[default]
    No,
    /// Measured when the export starts: a plan doesn't measure.
    Pending,
    Known(LightLevel),
    /// Measuring failed: its frames couldn't be read.
    Failed,
}

/// Why light levels aren't known.
enum Unknown {
    Pending,
    Missing(&'static str),
}

/// The metadata an output writes, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetadataChoice {
    /// What it keeps: `static`, `hdr10plus`, `dolby_vision`, `all` or `none`.
    pub mode: HdrMetadata,
    pub mastering: Option<MasteringDisplay>,
    pub light: Option<LightLevel>,
    /// What was asked for (`video.hdr_metadata`).
    pub asked: HdrMetadata,
    /// Why, in words: the mastering display, the light levels, HDR10+ and
    /// Dolby Vision.
    pub why: Vec<String>,
}

const BT709: [[f64; 2]; 3] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
const P3: [[f64; 2]; 3] = [[0.68, 0.32], [0.265, 0.69], [0.15, 0.06]];
const BT2020: [[f64; 2]; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];
const D65: [f64; 2] = [0.3127, 0.3290];

/// SDR white in HDR, cd/m² (ITU-R BT.2408): SDR pictures' light level.
const SDR_WHITE: u32 = 203;

/// The color volume a piece's pictures fill in PQ output, and whether the
/// file said so.
fn volume(m: &PieceMeta) -> (MasteringDisplay, bool) {
    let assumed = |primaries, min_nits, max_nits| MasteringDisplay {
        primaries,
        white: D65,
        min_nits,
        max_nits,
    };
    match (m.hdr, m.mastering) {
        (Some(Hdr::Pq), Some(given)) => (given, true),
        // The peak vidcut's own tone mapping assumes of PQ (export.md §4).
        (Some(Hdr::Pq), None) => (assumed(BT2020, 0.0001, 1000.0), false),
        // HLG's reference display (ITU-R BT.2100).
        (Some(Hdr::Hlg), _) => (assumed(BT2020, 0.005, 1000.0), false),
        (None, _) => (assumed(BT709, 0.1, f64::from(SDR_WHITE)), false),
    }
}

fn same_xy(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
}

fn same_primaries(a: &[[f64; 2]; 3], b: &[[f64; 2]; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| same_xy(*x, *y))
}

fn area(p: &[[f64; 2]; 3]) -> f64 {
    let [a, b, c] = p;
    ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
}

/// Whether point `q` lies in triangle `t` (with a little slack).
fn inside(t: &[[f64; 2]; 3], q: [f64; 2]) -> bool {
    let sign =
        |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (q[1] - a[1]) - (q[0] - a[0]) * (b[1] - a[1]);
    let d = [sign(t[0], t[1]), sign(t[1], t[2]), sign(t[2], t[0])];
    let eps = 1e-4;
    d.iter().all(|&x| x >= -eps) || d.iter().all(|&x| x <= eps)
}

/// The smallest volume holding them all: the primaries of the one whose
/// triangle holds the others' (BT.2020's otherwise), the highest peak and
/// the lowest black.
fn widest(volumes: &[MasteringDisplay]) -> MasteringDisplay {
    let first = volumes[0];
    let primaries = if volumes
        .iter()
        .all(|v| same_primaries(&v.primaries, &first.primaries))
    {
        first.primaries
    } else {
        let biggest = volumes
            .iter()
            .max_by(|a, b| area(&a.primaries).total_cmp(&area(&b.primaries)))
            .expect("some");
        if volumes
            .iter()
            .flat_map(|v| v.primaries)
            .all(|q| inside(&biggest.primaries, q))
        {
            biggest.primaries
        } else {
            BT2020
        }
    };
    let white = if volumes.iter().all(|v| same_xy(v.white, first.white)) {
        first.white
    } else {
        D65
    };
    MasteringDisplay {
        primaries,
        white,
        min_nits: volumes
            .iter()
            .map(|v| v.min_nits)
            .fold(f64::INFINITY, f64::min),
        max_nits: volumes.iter().map(|v| v.max_nits).fold(0.0, f64::max),
    }
}

/// The light levels of what `metas` show in PQ output: the highest of
/// theirs (measured where they say none, or for all with `measure_all`),
/// SDR at its white; or why there are none.
fn light_levels(
    metas: &[PieceMeta],
    measure_all: bool,
) -> std::result::Result<LightLevel, Unknown> {
    let mut out = LightLevel {
        max_cll: 0,
        max_fall: 0,
    };
    for m in metas {
        let measured = |none| match m.measured {
            Measured::Known(l) => Ok(l),
            Measured::Pending => Err(Unknown::Pending),
            Measured::No => Err(Unknown::Missing(none)),
            Measured::Failed => Err(Unknown::Missing("a picture couldn't be measured")),
        };
        let l = match (m.hdr, m.light) {
            _ if measure_all => measured("a picture wasn't measured")?,
            (Some(Hdr::Pq), Some(l)) => l,
            (Some(Hdr::Pq), None) => measured("a PQ source has none")?,
            (Some(Hdr::Hlg), _) => measured("HLG pictures have none")?,
            (None, _) => LightLevel {
                max_cll: SDR_WHITE,
                max_fall: SDR_WHITE,
            },
        };
        out.max_cll = out.max_cll.max(l.max_cll);
        out.max_fall = out.max_fall.max(l.max_fall);
    }
    Ok(out)
}

/// A volume in words: `P3 D65, 0.0001–1000 cd/m²`.
pub fn describe_mastering(m: &MasteringDisplay) -> String {
    let gamut = [(BT709, "BT.709"), (P3, "P3"), (BT2020, "BT.2020")]
        .iter()
        .find(|(p, _)| same_primaries(p, &m.primaries))
        .map_or("other primaries", |(_, n)| n);
    let white = if same_xy(m.white, D65) { " D65" } else { "" };
    format!(
        "{gamut}{white}, {}–{} cd/m²",
        trim(m.min_nits),
        trim(m.max_nits)
    )
}

fn trim(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The mastering display as x265 writes it:
/// `G(x,y)B(x,y)R(x,y)WP(x,y)L(max,min)`, chromaticity in 1/50000,
/// luminance in 1/10000 cd/m².
pub fn format_mastering(m: &MasteringDisplay) -> String {
    let c = |v: f64| (v * 50_000.0).round() as u64;
    let l = |v: f64| (v * 10_000.0).round() as u64;
    let [r, g, b] = m.primaries;
    format!(
        "G({},{})B({},{})R({},{})WP({},{})L({},{})",
        c(g[0]),
        c(g[1]),
        c(b[0]),
        c(b[1]),
        c(r[0]),
        c(r[1]),
        c(m.white[0]),
        c(m.white[1]),
        l(m.max_nits),
        l(m.min_nits)
    )
}

/// Reads [`format_mastering`]'s form.
pub fn parse_mastering(s: &str) -> Result<MasteringDisplay> {
    let bad = || {
        Error::usage(format!(
            "mastering display {s:?}: write G(x,y)B(x,y)R(x,y)WP(x,y)L(max,min), \
             chromaticity in 1/50000 and luminance in 1/10000 cd/m²"
        ))
    };
    let mut rest = s.trim();
    let mut pair = |name: &str| -> Result<(f64, f64)> {
        let r = rest.strip_prefix(name).ok_or_else(bad)?;
        let r = r.strip_prefix('(').ok_or_else(bad)?;
        let (inner, after) = r.split_once(')').ok_or_else(bad)?;
        let (a, b) = inner.split_once(',').ok_or_else(bad)?;
        let n = |t: &str| t.trim().parse::<u64>().map_err(|_| bad());
        rest = after;
        Ok((n(a)? as f64, n(b)? as f64))
    };
    let g = pair("G")?;
    let b = pair("B")?;
    let r = pair("R")?;
    let wp = pair("WP")?;
    let l = pair("L")?;
    if !rest.trim().is_empty() || l.0 <= l.1 {
        return Err(bad());
    }
    let xy = |p: (f64, f64)| [p.0 / 50_000.0, p.1 / 50_000.0];
    Ok(MasteringDisplay {
        primaries: [xy(r), xy(g), xy(b)],
        white: xy(wp),
        min_nits: l.1 / 10_000.0,
        max_nits: l.0 / 10_000.0,
    })
}

/// `video.max_cll` for light levels measured from every picture shown.
pub const MEASURE: &str = "measure";

/// MaxCLL and MaxFALL as `video.max_cll` writes them: `1000,400`.
pub fn format_light(l: &LightLevel) -> String {
    format!("{},{}", l.max_cll, l.max_fall)
}

/// Reads [`format_light`]'s form.
pub fn parse_light(s: &str) -> Result<LightLevel> {
    let bad = || {
        Error::usage(format!(
            "max_cll {s:?}: write MaxCLL,MaxFALL in cd/m², like 1000,400"
        ))
    };
    let (a, b) = s.split_once(',').ok_or_else(bad)?;
    let n = |t: &str| t.trim().parse::<u32>().map_err(|_| bad());
    let l = LightLevel {
        max_cll: n(a)?,
        max_fall: n(b)?,
    };
    if l.max_cll == 0 || l.max_fall > l.max_cll {
        return Err(bad());
    }
    Ok(l)
}

/// The share of `n` of `total` frames in words.
fn percent(n: i64, total: i64) -> String {
    let p = n as f64 * 100.0 / total.max(1) as f64;
    match p {
        p if p > 0.0 && p < 1.0 => "under 1%".into(),
        p if p < 100.0 && p > 99.0 => "over 99%".into(),
        p => format!("{p:.0}%"),
    }
}

/// Chooses the metadata of an output of `video` with settings `v`, whose
/// dynamic range is chosen already (`v.hdr` isn't `auto`). `dv_writer`:
/// an encoder the export may use writes Dolby Vision (libx265,
/// libsvtav1).
pub fn choose(
    v: &VideoSettings,
    video: &[VideoPiece],
    mut meta: impl FnMut(&VideoPiece) -> PieceMeta,
    dv_writer: bool,
) -> MetadataChoice {
    let asked = v.hdr_metadata;
    let out = v.hdr.transfer();
    let mut choice = MetadataChoice {
        mode: HdrMetadata::None,
        mastering: None,
        light: None,
        asked,
        why: Vec::new(),
    };
    if asked == HdrMetadata::None {
        choice.why.push("None: asked for".into());
        return choice;
    }
    let Some(out) = out else {
        choice.why.push("None: SDR carries no HDR metadata".into());
        return choice;
    };
    let shown: Vec<(i64, PieceMeta)> = video
        .iter()
        .filter_map(|p| match p {
            VideoPiece::Source { frames, .. } if *frames > 0 => Some((*frames, meta(p))),
            _ => None,
        })
        .collect();
    let total: i64 = shown.iter().map(|(n, _)| n).sum();
    let metas: Vec<PieceMeta> = shown.iter().map(|(_, m)| *m).collect();
    // Black frames (gaps) show no source: they don't count for the values,
    // but an encoder writing Dolby Vision needs an RPU on every frame.
    let black_in = |pieces: &mut dyn Iterator<Item = &VideoPiece>| -> i64 {
        pieces
            .map(|p| match p {
                VideoPiece::Black { frames } => *frames,
                _ => 0,
            })
            .sum()
    };
    let black = black_in(&mut video.iter());
    let leading_black = black_in(
        &mut video
            .iter()
            .take_while(|p| !matches!(p, VideoPiece::Source { frames, .. } if *frames > 0)),
    );

    // The static values, PQ only.
    if out == Hdr::Pq {
        choice.mastering = match &v.mastering_display {
            Some(s) => {
                choice.why.push("Mastering display: given".into());
                parse_mastering(s).ok()
            }
            None if metas.is_empty() => None,
            None => {
                let volumes: Vec<(MasteringDisplay, bool)> = metas.iter().map(volume).collect();
                let all: Vec<MasteringDisplay> = volumes.iter().map(|(v, _)| *v).collect();
                let m = widest(&all);
                let assumed = volumes.iter().filter(|(_, given)| !given).count();
                let from = if assumed == 0 {
                    "from the sources".to_string()
                } else if metas
                    .iter()
                    .all(|m| m.hdr == Some(Hdr::Pq) && m.mastering.is_none())
                {
                    "the PQ sources say none: BT.2020 at 1000 cd/m², the peak vidcut's tone \
                     mapping assumes"
                        .to_string()
                } else {
                    "the widest of the sources (SDR at 203 cd/m², PQ without values at 1000)"
                        .to_string()
                };
                choice.why.push(format!(
                    "Mastering display: {} ({from})",
                    describe_mastering(&m)
                ));
                Some(m)
            }
        };
        let measure_all = v.max_cll.as_deref() == Some(MEASURE);
        choice.light = match &v.max_cll {
            Some(s) if !measure_all => {
                choice.why.push("Light levels: given".into());
                parse_light(s).ok()
            }
            _ if metas.is_empty() => None,
            _ => match light_levels(&metas, measure_all) {
                Ok(l) => {
                    let measured = |m: &PieceMeta| matches!(m.measured, Measured::Known(_));
                    let from = if metas.iter().all(measured) {
                        "measured"
                    } else if metas.iter().any(measured) {
                        "the highest of the sources, measured where they say none"
                    } else {
                        "the highest of the sources"
                    };
                    choice.why.push(format!(
                        "MaxCLL {}, MaxFALL {}: {from}",
                        l.max_cll, l.max_fall
                    ));
                    Some(l)
                }
                Err(Unknown::Pending) => {
                    choice
                        .why
                        .push("MaxCLL and MaxFALL: measured when the export starts".into());
                    None
                }
                Err(Unknown::Missing(why)) => {
                    choice.why.push(format!("No light levels: {why}"));
                    None
                }
            },
        };
    } else {
        choice
            .why
            .push("No mastering display or light levels: HLG is scene-referred".into());
    }

    // Dynamic metadata, kept as it was.
    let auto = asked == HdrMetadata::Auto;
    let kept_as_is = |m: &PieceMeta| m.hdr == Some(out);
    let mut hdr10plus = false;
    if auto || asked.hdr10plus() {
        let with: i64 = shown
            .iter()
            .filter(|(_, m)| m.hdr10plus && kept_as_is(m))
            .map(|(n, _)| n)
            .sum();
        if out != Hdr::Pq {
            if asked.hdr10plus() || with > 0 {
                choice.why.push("No HDR10+: it needs PQ".into());
            }
        } else if with > 0 || asked.hdr10plus() {
            // Frames without it get it made from their measured light.
            hdr10plus = true;
            let all = total + black;
            choice.why.push(if with == 0 {
                "HDR10+: made from each frame's measured light".into()
            } else if with < all {
                format!(
                    "HDR10+: kept, and made from the measured light of the {} without it",
                    percent(all - with, all)
                )
            } else {
                "HDR10+: kept, every source has it".into()
            });
        }
    }
    let mut dolby_vision = false;
    if auto || asked.dolby_vision() {
        let first = metas.iter().find_map(|m| m.dolby_vision);
        let usable = |d: DolbyVision| {
            matches!(
                (d.profile, d.compatibility, out),
                (8 | 10, 1, Hdr::Pq) | (8 | 10, 4, Hdr::Hlg)
            )
        };
        let with: i64 = shown
            .iter()
            .filter(|(_, m)| m.dolby_vision.is_some() && m.dolby_vision == first && kept_as_is(m))
            .map(|(n, _)| n)
            .sum();
        let why = match first {
            None if asked.dolby_vision() => Some("no picture has it".to_string()),
            None => None,
            Some(d) if !usable(d) => Some(format!(
                "profile {}.{} can't be kept in {} output",
                d.profile,
                d.compatibility,
                if out == Hdr::Pq { "PQ" } else { "HLG" }
            )),
            Some(_) if with < total => Some(format!(
                "{} of the picture has none (or another profile)",
                percent(total - with, total)
            )),
            // Black after a picture takes its RPU; before the first there
            // is none to take.
            Some(_) if leading_black > 0 => Some(format!(
                "the black frames at the start ({}) have none, and every frame needs it",
                percent(leading_black, total + black)
            )),
            Some(_) if !matches!(v.codec, VideoCodec::Hevc | VideoCodec::Av1) => {
                Some("it needs HEVC or AV1".into())
            }
            Some(_) if !dv_writer => {
                Some("only libx265 and libsvtav1 write it, and the encoder is set".into())
            }
            Some(d) => {
                dolby_vision = true;
                choice.why.push(format!(
                    "Dolby Vision: kept (profile {}.{}), every picture has it",
                    if v.codec == VideoCodec::Av1 { 10 } else { 8 },
                    d.compatibility
                ));
                None
            }
        };
        if let Some(why) = why {
            choice.why.push(format!("No Dolby Vision: {why}"));
        }
    }

    let has_static = choice.mastering.is_some() || choice.light.is_some();
    choice.mode = match (hdr10plus, dolby_vision) {
        (true, true) => HdrMetadata::All,
        (true, false) => HdrMetadata::Hdr10plus,
        (false, true) => HdrMetadata::DolbyVision,
        (false, false) if has_static => HdrMetadata::Static,
        (false, false) => HdrMetadata::None,
    };
    choice
}

impl MetadataChoice {
    /// The settings with this choice in place of `auto` and unset values:
    /// what the encoder runs with.
    pub fn apply(&self, v: &mut VideoSettings) {
        v.hdr_metadata = self.mode;
        v.mastering_display = self.mastering.as_ref().map(format_mastering);
        v.max_cll = self.light.as_ref().map(format_light);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::export::{HdrMode, builtin_presets};

    fn piece(frames: i64, media: &str) -> VideoPiece {
        VideoPiece::Source {
            media: media.into(),
            path: PathBuf::from(format!("/{media}.mkv")),
            origin: 0.0,
            source_frame: 0,
            frames,
            image: None,
        }
    }

    fn settings(hdr: HdrMode, asked: HdrMetadata) -> VideoSettings {
        let mut v = builtin_presets()
            .into_iter()
            .find(|p| p.name == "h265-master")
            .unwrap()
            .settings
            .video
            .unwrap();
        v.hdr = hdr;
        v.hdr_metadata = asked;
        v
    }

    fn pq(max: f64, light: Option<(u32, u32)>) -> PieceMeta {
        PieceMeta {
            hdr: Some(Hdr::Pq),
            mastering: Some(MasteringDisplay {
                primaries: P3,
                white: D65,
                min_nits: 0.0001,
                max_nits: max,
            }),
            light: light.map(|(max_cll, max_fall)| LightLevel { max_cll, max_fall }),
            ..PieceMeta::default()
        }
    }

    fn run(v: &VideoSettings, pieces: &[(i64, PieceMeta)], dv_writer: bool) -> MetadataChoice {
        let video: Vec<VideoPiece> = pieces
            .iter()
            .enumerate()
            .map(|(i, (n, _))| piece(*n, &format!("m{i}")))
            .collect();
        choose(
            v,
            &video,
            |p| {
                let VideoPiece::Source { media, .. } = p else {
                    unreachable!()
                };
                pieces[media[1..].parse::<usize>().unwrap()].1
            },
            dv_writer,
        )
    }

    #[test]
    fn black_frames_take_dolby_vision_from_the_picture_before() {
        let v = settings(HdrMode::Pq, HdrMetadata::Auto);
        let mut dv = pq(1000.0, Some((1000, 400)));
        dv.dolby_vision = Some(DolbyVision {
            profile: 8,
            compatibility: 1,
        });
        dv.hdr10plus = true;
        let video = vec![piece(30, "m0"), VideoPiece::Black { frames: 10 }];
        let c = choose(&v, &video, |_| dv, true);
        assert_eq!(c.mode, HdrMetadata::All, "{:?}", c.why);
        // Before the first picture there is none to take.
        let video = vec![VideoPiece::Black { frames: 10 }, piece(30, "m0")];
        let c = choose(&v, &video, |_| dv, true);
        assert_eq!(c.mode, HdrMetadata::Hdr10plus, "{:?}", c.why);
        assert!(
            c.why.iter().any(|w| w
                == "No Dolby Vision: the black frames at the start (25%) have none, and every \
                    frame needs it"),
            "{:?}",
            c.why
        );
    }

    #[test]
    fn measured_light_stands_in_where_the_sources_say_none() {
        let v = settings(HdrMode::Pq, HdrMetadata::Auto);
        let level = |max_cll, max_fall| LightLevel { max_cll, max_fall };
        let mut bare = pq(1000.0, None);
        bare.measured = Measured::Pending;
        let c = run(&v, &[(10, bare), (10, pq(1000.0, Some((800, 300))))], false);
        assert_eq!(c.light, None);
        assert!(
            c.why
                .iter()
                .any(|w| w == "MaxCLL and MaxFALL: measured when the export starts"),
            "{:?}",
            c.why
        );
        bare.measured = Measured::Known(level(1200, 250));
        let c = run(&v, &[(10, bare), (10, pq(1000.0, Some((800, 300))))], false);
        assert_eq!(c.light, Some(level(1200, 300)), "{:?}", c.why);
        assert!(
            c.why[1].ends_with("measured where they say none"),
            "{:?}",
            c.why
        );
        // HLG in PQ output has none of its own.
        let hlg = PieceMeta {
            hdr: Some(Hdr::Hlg),
            measured: Measured::Known(level(900, 150)),
            ..PieceMeta::default()
        };
        let c = run(&v, &[(10, hlg)], false);
        assert_eq!(c.light, Some(level(900, 150)));
        // `measure`: measured for every picture, the files' values unused.
        let mut all = v.clone();
        all.max_cll = Some(MEASURE.into());
        let mut said = pq(1000.0, Some((4000, 1000)));
        said.measured = Measured::Known(level(700, 100));
        let c = run(&all, &[(10, said), (10, hlg)], false);
        assert_eq!(c.light, Some(level(900, 150)), "{:?}", c.why);
        said.measured = Measured::No;
        let c = run(&all, &[(10, said)], false);
        assert_eq!(c.light, None);
        // Measuring failed: said so, not that the source has none.
        bare.measured = Measured::Failed;
        let c = run(&v, &[(10, bare), (10, pq(1000.0, Some((800, 300))))], false);
        assert_eq!(c.light, None);
        assert!(
            c.why
                .iter()
                .any(|w| w == "No light levels: a picture couldn't be measured"),
            "{:?}",
            c.why
        );
    }

    #[test]
    fn the_x265_forms_round_trip() {
        let m =
            parse_mastering("G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)")
                .unwrap();
        assert_eq!(m.primaries, P3);
        assert_eq!((m.min_nits, m.max_nits), (0.0001, 1000.0));
        assert_eq!(
            format_mastering(&m),
            "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)"
        );
        assert!(parse_mastering("G(1,2)B(3,4)").is_err());
        assert!(
            parse_mastering("G(1,2)B(3,4)R(5,6)WP(7,8)L(1,10)").is_err(),
            "min over max"
        );
        assert_eq!(
            parse_light("1000,400").unwrap(),
            LightLevel {
                max_cll: 1000,
                max_fall: 400
            }
        );
        assert!(parse_light("0,0").is_err() && parse_light("400,1000").is_err());
        assert_eq!(describe_mastering(&m), "P3 D65, 0.0001–1000 cd/m²");
    }

    #[test]
    fn agreeing_sources_give_their_values() {
        let v = settings(HdrMode::Pq, HdrMetadata::Auto);
        let c = run(
            &v,
            &[
                (50, pq(1000.0, Some((900, 300)))),
                (50, pq(1000.0, Some((1000, 400)))),
            ],
            false,
        );
        assert_eq!(c.mode, HdrMetadata::Static);
        assert_eq!(c.mastering.unwrap().primaries, P3);
        assert_eq!(c.mastering.unwrap().max_nits, 1000.0);
        assert_eq!(
            c.light,
            Some(LightLevel {
                max_cll: 1000,
                max_fall: 400
            })
        );
        let mut applied = v.clone();
        c.apply(&mut applied);
        assert_eq!(applied.max_cll.as_deref(), Some("1000,400"));
        assert_eq!(applied.hdr_metadata, HdrMetadata::Static);
    }

    #[test]
    fn mixed_sources_give_the_widest_volume() {
        let v = settings(HdrMode::Pq, HdrMetadata::Auto);
        let mut wide = pq(4000.0, Some((3000, 500)));
        wide.mastering.as_mut().unwrap().primaries = BT2020;
        let sdr = PieceMeta::default();
        let c = run(
            &v,
            &[(10, pq(1000.0, Some((900, 300)))), (10, wide), (10, sdr)],
            false,
        );
        let m = c.mastering.unwrap();
        assert_eq!(m.primaries, BT2020, "BT.2020 holds P3 and BT.709");
        assert_eq!((m.min_nits, m.max_nits), (0.0001, 4000.0));
        assert_eq!(
            c.light,
            Some(LightLevel {
                max_cll: 3000,
                max_fall: 500
            })
        );
        // P3 PQ and SDR: P3 holds BT.709; SDR counts at 203 cd/m².
        let c = run(&v, &[(10, pq(1000.0, Some((150, 100)))), (10, sdr)], false);
        assert_eq!(c.mastering.unwrap().primaries, P3);
        assert_eq!(
            c.light,
            Some(LightLevel {
                max_cll: 203,
                max_fall: 203
            })
        );
    }

    #[test]
    fn unknown_values_are_assumed_or_left_out() {
        let v = settings(HdrMode::Pq, HdrMetadata::Auto);
        let bare = PieceMeta {
            hdr: Some(Hdr::Pq),
            ..PieceMeta::default()
        };
        let c = run(&v, &[(10, bare)], false);
        let m = c.mastering.unwrap();
        assert_eq!((m.primaries, m.max_nits), (BT2020, 1000.0));
        assert_eq!(c.light, None, "{:?}", c.why);
        assert!(c.why[1].contains("a PQ source has none"), "{:?}", c.why);
        // Given values win.
        let mut given = v.clone();
        given.max_cll = Some("500,100".into());
        let c = run(&given, &[(10, bare)], false);
        assert_eq!(
            c.light,
            Some(LightLevel {
                max_cll: 500,
                max_fall: 100
            })
        );
    }

    #[test]
    fn hlg_sdr_and_none_get_no_static_values() {
        let hlg = PieceMeta {
            hdr: Some(Hdr::Hlg),
            ..PieceMeta::default()
        };
        let c = run(
            &settings(HdrMode::Hlg, HdrMetadata::Auto),
            &[(10, hlg)],
            false,
        );
        assert_eq!(
            (c.mode, c.mastering, c.light),
            (HdrMetadata::None, None, None)
        );
        let c = run(
            &settings(HdrMode::Sdr, HdrMetadata::Auto),
            &[(10, pq(1000.0, None))],
            false,
        );
        assert_eq!(c.mode, HdrMetadata::None);
        let c = run(
            &settings(HdrMode::Pq, HdrMetadata::None),
            &[(10, pq(1000.0, None))],
            false,
        );
        assert_eq!((c.mode, c.mastering), (HdrMetadata::None, None));
    }

    #[test]
    fn dynamic_metadata_is_kept_or_made() {
        let v = settings(HdrMode::Pq, HdrMetadata::Auto);
        let mut plus = pq(1000.0, Some((1000, 400)));
        plus.hdr10plus = true;
        let c = run(&v, &[(30, plus), (30, plus)], false);
        assert_eq!(c.mode, HdrMetadata::Hdr10plus);
        // A clip without it gets it made from its measured light.
        let c = run(
            &v,
            &[(30, plus), (10, pq(1000.0, Some((1000, 400))))],
            false,
        );
        assert_eq!(c.mode, HdrMetadata::Hdr10plus);
        assert!(
            c.why.iter().any(
                |w| w == "HDR10+: kept, and made from the measured light of the 25% without it"
            ),
            "{:?}",
            c.why
        );
        // None has it: only when asked for.
        let c = run(&v, &[(10, pq(1000.0, None))], false);
        assert_eq!(c.mode, HdrMetadata::Static);
        let c = run(
            &settings(HdrMode::Pq, HdrMetadata::Hdr10plus),
            &[(10, pq(1000.0, None))],
            false,
        );
        assert_eq!(c.mode, HdrMetadata::Hdr10plus);
        // Static only, asked for.
        let c = run(
            &settings(HdrMode::Pq, HdrMetadata::Static),
            &[(30, plus)],
            false,
        );
        assert_eq!(c.mode, HdrMetadata::Static);

        // Dolby Vision 8.1, with an encoder that writes it.
        let mut dv = pq(1000.0, Some((1000, 400)));
        dv.dolby_vision = Some(DolbyVision {
            profile: 8,
            compatibility: 1,
        });
        let c = run(&v, &[(30, dv)], true);
        assert_eq!(c.mode, HdrMetadata::DolbyVision);
        let c = run(&v, &[(30, dv)], false);
        assert_eq!(c.mode, HdrMetadata::Static);
        assert!(
            c.why
                .iter()
                .any(|w| w.starts_with("No Dolby Vision: only libx265")),
            "{:?}",
            c.why
        );
        // 8.4 (HLG) only into HLG; profile 5 never.
        let mut hlg_dv = PieceMeta {
            hdr: Some(Hdr::Hlg),
            dolby_vision: Some(DolbyVision {
                profile: 8,
                compatibility: 4,
            }),
            ..PieceMeta::default()
        };
        let c = run(
            &settings(HdrMode::Hlg, HdrMetadata::Auto),
            &[(30, hlg_dv)],
            true,
        );
        assert_eq!(c.mode, HdrMetadata::DolbyVision);
        hlg_dv.dolby_vision = Some(DolbyVision {
            profile: 5,
            compatibility: 0,
        });
        let c = run(
            &settings(HdrMode::Hlg, HdrMetadata::Auto),
            &[(30, hlg_dv)],
            true,
        );
        assert_eq!(c.mode, HdrMetadata::None);
        // Converted pictures keep none: HLG with HDR10+? no; PQ into HLG.
        let c = run(
            &settings(HdrMode::Hlg, HdrMetadata::Auto),
            &[(30, plus)],
            true,
        );
        assert_eq!(c.mode, HdrMetadata::None);
    }
}
