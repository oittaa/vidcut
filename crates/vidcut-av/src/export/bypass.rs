use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use vidcut_core::export::AlphaOut;
use vidcut_core::model::{AlphaMode, Hdr, Picture};
use vidcut_core::render::{Over, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

use super::subtitles::Cue;
use super::video::Clock;
use crate::error::{Error, Result};
use crate::ffmpeg::packet::{Mut, Ref};
use crate::ffmpeg::{self, Rescale, codec, ffi, format};

const MOST_HELD: usize = 512;
const LOOKAHEAD_MARGIN: usize = 2;
const READ_ON: f64 = 2.0;
const SEEK_BEFORE: f64 = 0.1;
const TICKS_PER_FRAME: i64 = 100;

const H264_SPS: u8 = 7;
const H264_IDR: u8 = 5;
const H264_AUD: u8 = 9;
const HEVC_BLA_W_LP: u8 = 16;
const HEVC_CRA: u8 = 21;
const HEVC_VCL_END: u8 = 32;
const HEVC_SPS: u8 = 33;
const HEVC_AUD: u8 = 35;
const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;

const H264_LADDER: [i32; 6] = [
    ffi::AV_PROFILE_H264_CONSTRAINED_BASELINE,
    ffi::AV_PROFILE_H264_MAIN,
    ffi::AV_PROFILE_H264_HIGH,
    ffi::AV_PROFILE_H264_HIGH_10,
    ffi::AV_PROFILE_H264_HIGH_422,
    ffi::AV_PROFILE_H264_HIGH_444_PREDICTIVE,
];

pub(crate) struct Output<'a> {
    pub(crate) codec: codec::Id,
    pub(crate) profile: Option<&'a str>,
    pub(crate) pixel: format::Pixel,
    pub(crate) size: (u32, u32),
    pub(crate) hdr: Option<Hdr>,
    pub(crate) alpha: Option<AlphaOut>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Refused {
    pub path: PathBuf,
    pub why: String,
}

#[derive(Clone)]
pub(crate) enum Bits {
    Nal {
        hevc: bool,
        length: Option<usize>,
        sets: Vec<u8>,
    },
    Av1 {
        header: Vec<u8>,
    },
    Plain {
        intra: bool,
    },
}

impl Bits {
    pub(crate) fn of(id: codec::Id, extra: &[u8]) -> std::result::Result<Self, String> {
        Ok(match id {
            codec::Id::H264 | codec::Id::HEVC => {
                let hevc = id == codec::Id::HEVC;
                if extra.is_empty() || is_annex_b(extra) {
                    Self::Nal {
                        hevc,
                        length: None,
                        sets: extra.to_vec(),
                    }
                } else {
                    let read = match hevc {
                        false => avcc_sets(extra),
                        true => hvcc_sets(extra),
                    };
                    let (length, sets) =
                        read.ok_or_else(|| "its stream headers are unreadable".to_string())?;
                    Self::Nal {
                        hevc,
                        length: Some(length),
                        sets,
                    }
                }
            }
            codec::Id::AV1 => Self::Av1 {
                header: match extra.first() {
                    Some(b) if b & 0x80 != 0 => extra.get(4..).unwrap_or_default().to_vec(),
                    _ => extra.to_vec(),
                },
            },
            _ => Self::Plain {
                intra: intra_only(id),
            },
        })
    }

    fn starts(&self, p: &ffmpeg::Packet) -> bool {
        let data = p.data().unwrap_or_default();
        match self {
            Self::Plain { intra: true } => true,
            Self::Plain { intra: false } => p.is_key(),
            Self::Av1 { header } => {
                p.is_key()
                    && (!header.is_empty()
                        || obus(data).iter().any(|(t, _)| *t == OBU_SEQUENCE_HEADER))
            }
            Self::Nal { hevc, length, .. } => {
                if !p.is_key() {
                    return false;
                }
                let mut kinds = nals(data, *length)
                    .into_iter()
                    .map(|r| nal_type(data[r.start], *hevc));
                match hevc {
                    true => kinds
                        .find(|t| *t < HEVC_VCL_END)
                        .is_some_and(|t| (HEVC_BLA_W_LP..=HEVC_CRA).contains(&t)),
                    false => kinds.any(|t| t == H264_IDR),
                }
            }
        }
    }

    pub(crate) fn first_packet(&self, data: &[u8]) -> Option<Vec<u8>> {
        match self {
            Self::Nal { .. } => Some(self.annex_b(data, true)),
            Self::Av1 { header } => with_sequence_header(data, header),
            Self::Plain { .. } => None,
        }
    }

    fn later_packet(&self, data: &[u8]) -> Option<Vec<u8>> {
        match self {
            Self::Nal {
                length: Some(_), ..
            } => Some(self.annex_b(data, false)),
            _ => None,
        }
    }

    fn annex_b(&self, data: &[u8], first: bool) -> Vec<u8> {
        let Self::Nal { hevc, length, sets } = self else {
            return data.to_vec();
        };
        let hevc = *hevc;
        let spans = nals(data, *length);
        let kind = |r: &Range<usize>| nal_type(data[r.start], hevc);
        let (sps, aud) = match hevc {
            true => (HEVC_SPS, HEVC_AUD),
            false => (H264_SPS, H264_AUD),
        };
        let mut put_sets = first && !spans.iter().any(|r| kind(r) == sps);
        let mut out = Vec::with_capacity(data.len() + sets.len() + 4 * spans.len());
        for r in &spans {
            let k = kind(r);
            if put_sets && k != aud {
                out.extend_from_slice(sets);
                put_sets = false;
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            let at = out.len();
            out.extend_from_slice(&data[r.clone()]);
            if first && hevc && k == HEVC_CRA {
                out[at] = (out[at] & 0x81) | (HEVC_BLA_W_LP << 1);
            }
        }
        if put_sets {
            out.extend_from_slice(sets);
        }
        out
    }
}

fn intra_only(id: codec::Id) -> bool {
    // SAFETY: FFmpeg's static descriptor table, read only.
    unsafe {
        let d = ffi::avcodec_descriptor_get(id.into());
        !d.is_null() && i64::from((*d).props) & ffi::AV_CODEC_PROP_INTRA_ONLY as i64 != 0
    }
}

fn is_annex_b(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1])
}

fn nal_type(header: u8, hevc: bool) -> u8 {
    match hevc {
        true => (header >> 1) & 0x3f,
        false => header & 0x1f,
    }
}

fn nals(data: &[u8], length: Option<usize>) -> Vec<Range<usize>> {
    let Some(n) = length else {
        return annex_b_nals(data);
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i + n <= data.len() {
        let len = data[i..i + n]
            .iter()
            .fold(0usize, |a, b| a << 8 | usize::from(*b));
        i += n;
        if len == 0 || i + len > data.len() {
            break;
        }
        out.push(i..i + len);
        i += len;
    }
    out
}

fn annex_b_nals(data: &[u8]) -> Vec<Range<usize>> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(k, &s)| {
            let mut end = starts.get(k + 1).map_or(data.len(), |&n| n - 3);
            while end > s && data[end - 1] == 0 {
                end -= 1;
            }
            s..end
        })
        .filter(|r| !r.is_empty())
        .collect()
}

fn read_u16(data: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes(
        data.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn put_nal(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}

fn avcc_sets(extra: &[u8]) -> Option<(usize, Vec<u8>)> {
    if extra.first() != Some(&1) {
        return None;
    }
    let length = usize::from(extra.get(4)? & 3) + 1;
    let mut out = Vec::new();
    let mut at = 5;
    for mask in [0x1f, 0xff] {
        let count = usize::from(extra.get(at)? & mask);
        at += 1;
        for _ in 0..count {
            let len = read_u16(extra, at)?;
            put_nal(&mut out, extra.get(at + 2..at + 2 + len)?);
            at += 2 + len;
        }
    }
    Some((length, out))
}

fn hvcc_sets(extra: &[u8]) -> Option<(usize, Vec<u8>)> {
    let length = usize::from(extra.get(21)? & 3) + 1;
    let arrays = usize::from(*extra.get(22)?);
    let mut out = Vec::new();
    let mut at = 23;
    for _ in 0..arrays {
        let count = read_u16(extra, at + 1)?;
        at += 3;
        for _ in 0..count {
            let len = read_u16(extra, at)?;
            put_nal(&mut out, extra.get(at + 2..at + 2 + len)?);
            at += 2 + len;
        }
    }
    Some((length, out))
}

fn leb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (i, b) in data.iter().take(8).enumerate() {
        value |= usize::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

fn obus(data: &[u8]) -> Vec<(u8, Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let h = data[i];
        let kind = (h >> 3) & 0x0f;
        let mut at = i + 1 + usize::from(h & 0x04 != 0);
        let end = match h & 0x02 != 0 {
            true => {
                let Some((size, n)) = data.get(at..).and_then(leb128) else {
                    break;
                };
                at += n;
                at + size
            }
            false => data.len(),
        };
        if end > data.len() {
            break;
        }
        out.push((kind, i..end));
        i = end;
    }
    out
}

fn with_sequence_header(data: &[u8], header: &[u8]) -> Option<Vec<u8>> {
    let units = obus(data);
    if header.is_empty() || units.iter().any(|(t, _)| *t == OBU_SEQUENCE_HEADER) {
        return None;
    }
    let at = match units.first() {
        Some((OBU_TEMPORAL_DELIMITER, r)) => r.end,
        _ => 0,
    };
    Some([&data[..at], header, &data[at..]].concat())
}

pub(crate) fn with_data(p: &ffmpeg::Packet, data: &[u8]) -> Result<ffmpeg::Packet> {
    let mut out = ffmpeg::Packet::copy(data);
    // SAFETY: both packets are live; the call copies fields and side data only.
    let ret = unsafe { ffi::av_packet_copy_props(out.as_mut_ptr(), p.as_ptr()) };
    if ret < 0 {
        return Err(Error::Ffmpeg {
            context: "copying a packet's fields".into(),
            source: ffmpeg::Error::from(ret),
        });
    }
    Ok(out)
}

fn layout(p: format::Pixel) -> Option<(u8, u8, i32, u8, u64)> {
    let d = p.descriptor()?;
    // SAFETY: FFmpeg's static descriptor of the format.
    let d = unsafe { &*d.as_ptr() };
    let flags = d.flags
        & (ffi::AV_PIX_FMT_FLAG_RGB | ffi::AV_PIX_FMT_FLAG_ALPHA | ffi::AV_PIX_FMT_FLAG_HWACCEL)
            as u64;
    (flags & ffi::AV_PIX_FMT_FLAG_HWACCEL as u64 == 0).then_some((
        d.log2_chroma_w,
        d.log2_chroma_h,
        d.comp[0].depth,
        d.nb_components,
        flags,
    ))
}

fn h264_named(name: &str) -> Option<i32> {
    Some(match name {
        "baseline" | "constrained_baseline" => ffi::AV_PROFILE_H264_CONSTRAINED_BASELINE,
        "main" => ffi::AV_PROFILE_H264_MAIN,
        "high" => ffi::AV_PROFILE_H264_HIGH,
        "high10" => ffi::AV_PROFILE_H264_HIGH_10,
        "high422" => ffi::AV_PROFILE_H264_HIGH_422,
        "high444" => ffi::AV_PROFILE_H264_HIGH_444_PREDICTIVE,
        _ => return None,
    })
}

fn prores_named(name: &str) -> Option<i32> {
    Some(match name {
        "proxy" => ffi::AV_PROFILE_PRORES_PROXY,
        "lt" => ffi::AV_PROFILE_PRORES_LT,
        "standard" => ffi::AV_PROFILE_PRORES_STANDARD,
        "hq" => ffi::AV_PROFILE_PRORES_HQ,
        "4444" => ffi::AV_PROFILE_PRORES_4444,
        "xq" => ffi::AV_PROFILE_PRORES_XQ,
        _ => return None,
    })
}

fn dnxhr_named(name: &str) -> Option<i32> {
    Some(match name {
        "lb" => ffi::AV_PROFILE_DNXHR_LB,
        "sq" => ffi::AV_PROFILE_DNXHR_SQ,
        "hq" => ffi::AV_PROFILE_DNXHR_HQ,
        "hqx" => ffi::AV_PROFILE_DNXHR_HQX,
        "444" => ffi::AV_PROFILE_DNXHR_444,
        _ => return None,
    })
}

fn takes_profile(out: &Output, profile: i32) -> bool {
    let named = out.profile.unwrap_or_default();
    match out.codec {
        codec::Id::H264 => {
            let rank = |p: i32| H264_LADDER.iter().position(|x| *x == p);
            matches!(
                (rank(profile), h264_named(named).and_then(rank)),
                (Some(a), Some(b)) if a <= b
            )
        }
        codec::Id::HEVC => {
            profile == ffi::AV_PROFILE_HEVC_MAIN
                || (profile == ffi::AV_PROFILE_HEVC_MAIN_10 && named == "main10")
        }
        codec::Id::AV1 => profile == ffi::AV_PROFILE_AV1_MAIN,
        codec::Id::VP9 => profile == ffi::AV_PROFILE_VP9_0 || profile == ffi::AV_PROFILE_VP9_2,
        codec::Id::PRORES => prores_named(named) == Some(profile),
        codec::Id::DNXHD => dnxhr_named(named) == Some(profile),
        _ => false,
    }
}

fn profile_name(id: codec::Id, profile: i32) -> String {
    // SAFETY: FFmpeg's static profile table; the name is a static string or null.
    let name = unsafe { ffi::avcodec_profile_name(id.into(), profile) };
    match name.is_null() {
        true => profile.to_string(),
        // SAFETY: a static NUL-terminated string from FFmpeg.
        false => unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned(),
    }
}

struct File {
    path: PathBuf,
    stream: usize,
    tb: ffmpeg::Rational,
    tick: f64,
    delay: usize,
    bits: Bits,
}

impl File {
    fn open(path: &Path, out: &Output) -> std::result::Result<Self, String> {
        let input = crate::decode::open_file(path).map_err(|e| e.to_string())?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| "it has no video".to_string())?;
        let params = stream.parameters();
        // SAFETY: the stream's parameters, read only while the input is open.
        let p = unsafe { &*params.as_ptr() };
        let id = params.id();
        if id != out.codec {
            return Err(format!("its codec is {}", id.name()));
        }
        let size = (p.width.max(0) as u32, p.height.max(0) as u32);
        if size != out.size {
            return Err(format!("its size is {}×{}", size.0, size.1));
        }
        let pixel = crate::buffers::pixel_format(p.format).map(format::Pixel::from);
        let fits = match (pixel.and_then(layout), layout(out.pixel)) {
            (Some(a), Some(b)) if id == codec::Id::PRORES => {
                (a.0, a.1, a.3, a.4) == (b.0, b.1, b.3, b.4)
            }
            (Some(a), Some(b)) => a == b,
            _ => false,
        };
        if !fits {
            let name = pixel
                .and_then(|p| p.descriptor())
                .map_or("unknown", |d| d.name());
            return Err(format!("its pixel format is {name}"));
        }
        if !takes_profile(out, p.profile) {
            return Err(format!("its profile is {}", profile_name(id, p.profile)));
        }
        if !crate::color::Source::of_stream(p).is_output(out.hdr) {
            return Err("its colors are not those of the output".into());
        }
        if !matches!(
            p.field_order,
            ffi::AVFieldOrder::AV_FIELD_PROGRESSIVE | ffi::AVFieldOrder::AV_FIELD_UNKNOWN
        ) {
            return Err("it is interlaced".into());
        }
        let sar = p.sample_aspect_ratio;
        if sar.num != 0 && sar.num != sar.den {
            return Err("its pixels are not square".into());
        }
        if crate::decode::stream_rotation(&params) != 0 {
            return Err("it is rotated".into());
        }
        let extra = match p.extradata.is_null() || p.extradata_size <= 0 {
            true => &[][..],
            // SAFETY: FFmpeg keeps `extradata_size` bytes at `extradata`.
            false => unsafe { std::slice::from_raw_parts(p.extradata, p.extradata_size as usize) },
        };
        let bits = Bits::of(id, extra)?;
        let tb = stream.time_base();
        Ok(Self {
            path: path.to_path_buf(),
            stream: stream.index(),
            tb,
            tick: f64::from(tb.numerator()) / f64::from(tb.denominator()),
            delay: p.video_delay.max(0) as usize,
            bits,
        })
    }
}

#[derive(Clone)]
struct Span {
    from: i64,
    to: i64,
    first: i64,
    piece: Arc<VideoPiece>,
    clock: Clock,
    rate: f64,
    file: usize,
}

impl Span {
    fn target(&self, j: i64) -> f64 {
        self.clock.target(self.piece.position(j - self.first))
    }

    fn at(&self, t: f64) -> i64 {
        let mut j = self.first + ((t - self.target(self.first)) * self.rate).floor() as i64;
        while self.target(j + 1) - EPS <= t {
            j += 1;
        }
        while self.target(j) - EPS > t {
            j -= 1;
        }
        j
    }
}

struct Pkt {
    packet: ffmpeg::Packet,
    t: Option<f64>,
}

struct Reader {
    file: usize,
    input: format::context::Input,
    queue: VecDeque<Pkt>,
    ended: bool,
    last: Option<f64>,
    taken: Option<f64>,
}

impl Reader {
    fn open(file: usize, f: &File) -> Result<Self> {
        let input = crate::decode::open_file(&f.path)?;
        super::audio::discard_others(&input, f.stream);
        Ok(Self {
            file,
            input,
            queue: VecDeque::new(),
            ended: false,
            last: None,
            taken: None,
        })
    }

    fn seek(&mut self, t: f64) -> Result<()> {
        let ts = (t * f64::from(ffi::AV_TIME_BASE)).floor() as i64;
        self.input.seek(ts, ..ts).map_err(|e| Error::Ffmpeg {
            context: "seeking for a packet to copy".into(),
            source: e,
        })?;
        self.queue.clear();
        self.ended = false;
        self.last = None;
        self.taken = None;
        Ok(())
    }

    fn read(&mut self, f: &File) -> Result<bool> {
        if self.ended {
            return Ok(false);
        }
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == f.stream => {
                    let t = packet.pts().map(|pts| pts as f64 * f.tick);
                    if t.is_some() {
                        self.last = t;
                    }
                    self.queue.push_back(Pkt { packet, t });
                    return Ok(true);
                }
                Ok(()) => continue,
                Err(ffmpeg::Error::Eof) => {
                    self.ended = true;
                    return Ok(false);
                }
                Err(e) => {
                    return Err(Error::Ffmpeg {
                        context: format!("reading {}", f.path.display()),
                        source: e,
                    });
                }
            }
        }
    }

    fn fill(&mut self, f: &File, n: usize) -> Result<()> {
        while self.queue.len() < n && self.read(f)? {}
        Ok(())
    }

    fn take(&mut self) -> Option<Pkt> {
        let p = self.queue.pop_front()?;
        if let Some(t) = p.t {
            self.taken = Some(self.taken.map_or(t, |l| l.max(t)));
        }
        Some(p)
    }

    fn before(&mut self, f: &File, t: f64) -> Result<Option<Option<f64>>> {
        let mut before = self.taken;
        let mut i = 1;
        loop {
            if i >= self.queue.len() && !self.read(f)? {
                return Ok(Some(before));
            }
            match self.queue[i].t {
                Some(u) if u < t - EPS => before = Some(before.map_or(u, |l| l.max(u))),
                Some(_) => return Ok(Some(before)),
                None => {}
            }
            i += 1;
            if i > MOST_HELD {
                return Ok(None);
            }
        }
    }
}

pub(crate) struct Run {
    span: usize,
    start_t: f64,
    end_t: f64,
    last_t: Option<f64>,
    held: Vec<Pkt>,
    first: bool,
    done: bool,
    from: i64,
}

pub(crate) struct Batch {
    pub(crate) packets: Vec<(i64, ffmpeg::Packet)>,
    pub(crate) covered: i64,
}

pub(crate) struct Copying {
    spans: Vec<Span>,
    files: Vec<File>,
    fps: Rational,
    stream_tb: ffmpeg::Rational,
    reader: Option<Reader>,
    at: usize,
    start: Option<(usize, i64)>,
    after: i64,
    copied: i64,
}

impl Copying {
    pub(crate) fn new(
        plan: &RenderPlan,
        out: &Output,
        burned: &[Cue],
    ) -> (Option<Self>, Vec<Refused>) {
        let mut spans = Vec::new();
        let mut files: Vec<File> = Vec::new();
        let mut refused: Vec<Refused> = Vec::new();
        let mut first = 0;
        for piece in &plan.video {
            let frames = piece.frames();
            if let Some((path, clock, source)) = copyable(piece, plan.fps, out.alpha) {
                let file = match files.iter().position(|f| f.path == path) {
                    Some(i) => Some(i),
                    None if refused.iter().any(|r| r.path == path) => None,
                    None => match File::open(path, out) {
                        Ok(f) => {
                            files.push(f);
                            Some(files.len() - 1)
                        }
                        Err(why) => {
                            refused.push(Refused {
                                path: path.to_path_buf(),
                                why,
                            });
                            None
                        }
                    },
                };
                if let Some(file) = file {
                    let source = Arc::new(source.clone());
                    for (from, to) in outside(burned, first, first + frames) {
                        spans.push(Span {
                            from,
                            to,
                            first,
                            piece: source.clone(),
                            clock,
                            rate: plan.fps.as_f64(),
                            file,
                        });
                    }
                }
            }
            first += frames;
        }
        if spans.is_empty() {
            return (None, refused);
        }
        let copying = Self {
            spans,
            files,
            fps: plan.fps,
            stream_tb: ffmpeg::Rational(plan.fps.den() as i32, plan.fps.num() as i32),
            reader: None,
            at: 0,
            start: None,
            after: 0,
            copied: 0,
        };
        (Some(copying), refused)
    }

    pub(crate) fn time_base(&self) -> ffmpeg::Rational {
        let fps = (self.fps.num(), self.fps.den());
        let mut d = fps.0 / gcd(fps.0, fps.1);
        for f in &self.files {
            let (a, b) = (i64::from(f.tb.numerator()), i64::from(f.tb.denominator()));
            d = lcm(d, b / gcd(a, b).max(1));
        }
        let per_frame = |d: i64| d * fps.1 / fps.0.max(1);
        while per_frame(d) < TICKS_PER_FRAME {
            d *= 2;
        }
        match i32::try_from(d) {
            Ok(d) => ffmpeg::Rational(1, d),
            Err(_) => ffmpeg::Rational(fps.1 as i32, (fps.0 * TICKS_PER_FRAME) as i32),
        }
    }

    pub(crate) fn set_stream_tb(&mut self, tb: ffmpeg::Rational) {
        self.stream_tb = tb;
    }

    pub(crate) fn copied(&self) -> i64 {
        self.copied
    }

    pub(crate) fn reach(&self) -> i64 {
        self.spans.iter().map(|s| s.to - s.from).sum()
    }

    fn frame_tb(&self) -> ffmpeg::Rational {
        ffmpeg::Rational(self.fps.den() as i32, self.fps.num() as i32)
    }

    pub(crate) fn run_at(&mut self, j: i64, cancel: &AtomicBool) -> Result<Option<Run>> {
        while self.spans.get(self.at).is_some_and(|s| s.to <= j) {
            self.at += 1;
            self.start = None;
        }
        let Some(span) = self.spans.get(self.at).cloned() else {
            return Ok(None);
        };
        if j < span.from {
            return Ok(None);
        }
        let from = j.max(self.after);
        if self.start.is_none_or(|(i, s)| i != self.at || s < from) {
            let s = self.find_start(&span, from, cancel)?;
            self.start = Some((self.at, s));
        }
        let Some((_, s)) = self.start else {
            return Ok(None);
        };
        if s != j {
            return Ok(None);
        }
        self.start = None;
        let reader = self.reader.as_ref().expect("a start was found");
        let start_t = reader
            .queue
            .front()
            .and_then(|p| p.t)
            .expect("the start packet waits first");
        Ok(Some(Run {
            span: self.at,
            start_t,
            end_t: span.target(span.to) - EPS,
            last_t: None,
            held: Vec::new(),
            first: true,
            done: false,
            from: j,
        }))
    }

    fn reader_for(&mut self, span: &Span, target: f64) -> Result<()> {
        let f = &self.files[span.file];
        if self.reader.as_ref().is_none_or(|r| r.file != span.file) {
            self.reader = None;
            let mut r = Reader::open(span.file, f)?;
            r.seek(target - SEEK_BEFORE)?;
            self.reader = Some(r);
            return Ok(());
        }
        let r = self.reader.as_mut().expect("checked");
        let at = r.queue.front().and_then(|p| p.t).or(r.last);
        if at.is_none_or(|t| t > target + EPS || target - t > READ_ON) {
            r.seek(target - SEEK_BEFORE)?;
        }
        Ok(())
    }

    fn find_start(&mut self, span: &Span, from: i64, cancel: &AtomicBool) -> Result<i64> {
        let end_t = span.target(span.to) - EPS;
        self.reader_for(span, span.target(from))?;
        let f = &self.files[span.file];
        let r = self.reader.as_mut().expect("opened");
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Invalid(super::CANCELLED.into()));
            }
            if r.queue.is_empty() && !r.read(f)? {
                return Ok(i64::MAX);
            }
            let p = r.queue.front().expect("read");
            if let Some(t) = p.t {
                if t >= end_t {
                    return Ok(i64::MAX);
                }
                let j = span.at(t);
                if j >= from && f.bits.starts(&p.packet) {
                    let shown = match r.before(f, t)? {
                        Some(Some(u)) => span.at(u) < j,
                        Some(None) => t <= span.clock.origin + EPS,
                        None => false,
                    };
                    if shown {
                        return Ok(j);
                    }
                }
            }
            r.take();
        }
    }

    pub(crate) fn next(&mut self, run: &mut Run) -> Result<Option<Batch>> {
        if run.done {
            return Ok(None);
        }
        let span = self.spans[run.span].clone();
        let f = &self.files[span.file];
        let r = self.reader.as_mut().expect("a run reads");
        let need = f.delay + LOOKAHEAD_MARGIN;
        loop {
            if r.queue.is_empty() {
                r.read(f)?;
            }
            let stop = match r.queue.front().map(|p| p.t) {
                None | Some(None) => true,
                Some(Some(t)) if t >= run.end_t => true,
                Some(Some(t)) if t < run.start_t - EPS => {
                    r.take();
                    continue;
                }
                Some(Some(_)) => false,
            };
            if !stop {
                run.held.push(r.take().expect("checked"));
            }
            if run.held.is_empty() {
                run.done = true;
                return Ok(None);
            }
            r.fill(f, need)?;
            let most = run
                .held
                .iter()
                .filter_map(|p| p.t)
                .fold(f64::NEG_INFINITY, f64::max);
            let closed = r
                .queue
                .iter()
                .take(need)
                .all(|p| p.t.is_some_and(|t| t > most || t < run.start_t - EPS));
            if !closed {
                if stop || run.held.len() > MOST_HELD {
                    run.held.clear();
                    run.done = true;
                    return Ok(None);
                }
                continue;
            }
            let Some(slots) = shown_slots(&span, run.last_t, &run.held) else {
                run.held.clear();
                run.done = true;
                return Ok(None);
            };
            run.done = stop;
            let batch = self.emit(run, &span, slots)?;
            return Ok(Some(batch));
        }
    }

    fn emit(&mut self, run: &mut Run, span: &Span, slots: Vec<i64>) -> Result<Batch> {
        let frame_tb = self.frame_tb();
        let f = &self.files[span.file];
        let mut packets = Vec::with_capacity(run.held.len());
        let mut covered = run.from;
        for (pkt, slot) in std::mem::take(&mut run.held).into_iter().zip(slots) {
            let j = slot.max(run.from);
            covered = covered.max(j + 1);
            let data = pkt.packet.data().unwrap_or_default();
            let changed = match std::mem::take(&mut run.first) {
                true => f.bits.first_packet(data),
                false => f.bits.later_packet(data),
            };
            let mut p = match changed {
                Some(d) => with_data(&pkt.packet, &d)?,
                None => pkt.packet,
            };
            let pts = j.rescale(frame_tb, self.stream_tb);
            let dts = match (p.pts(), p.dts()) {
                (Some(s), Some(d)) => pts - (s - d).max(0).rescale(f.tb, self.stream_tb),
                _ => pts,
            };
            p.set_pts(Some(pts));
            p.set_dts(Some(dts));
            p.set_duration(1.rescale(frame_tb, self.stream_tb));
            p.set_stream(0);
            p.set_position(-1);
            if let Some(t) = pkt.t {
                run.last_t = Some(run.last_t.map_or(t, |l| l.max(t)));
            }
            packets.push((j, p));
        }
        Ok(Batch { packets, covered })
    }

    pub(crate) fn end(&mut self, run: &Run) -> i64 {
        let span = &self.spans[run.span];
        let e = match run.last_t {
            Some(t) => (span.at(t) + 1).min(span.to),
            None => run.from,
        };
        match e > run.from {
            true => self.copied += e - run.from,
            false => self.after = run.from + 1,
        }
        e
    }
}

const EPS: f64 = 1e-6;

fn shown_slots(span: &Span, last: Option<f64>, held: &[Pkt]) -> Option<Vec<i64>> {
    let mut order: Vec<usize> = (0..held.len()).collect();
    order.sort_by(|a, b| {
        held[*a]
            .t
            .partial_cmp(&held[*b].t)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut slots = vec![0; held.len()];
    let mut prev = last;
    for i in order {
        let t = held[i].t?;
        slots[i] = match prev {
            None => span.at(t),
            Some(p) => {
                let q = span.at(p) + 1;
                if q > span.at(t) {
                    return None;
                }
                q
            }
        };
        prev = Some(t);
    }
    Some(slots)
}

fn copyable(
    piece: &VideoPiece,
    fps: Rational,
    alpha: Option<AlphaOut>,
) -> Option<(&Path, Clock, &VideoPiece)> {
    let (piece, alone) = match (piece, alpha) {
        (VideoPiece::Mix { under, over }, Some(_))
            if matches!(**under, VideoPiece::Black { .. }) =>
        {
            match over.as_slice() {
                [
                    Over {
                        piece,
                        ramp: None,
                        smooth: None,
                    },
                ] => (piece, true),
                _ => return None,
            }
        }
        _ => (piece, false),
    };
    let VideoPiece::Source {
        path,
        origin,
        per_frame,
        image,
        pace,
        color_space,
        alpha_mode,
        upscale,
        retime,
        placed,
        made,
        input_lut,
        ..
    } = piece
    else {
        return None;
    };
    let plain = image.is_none()
        && pace.is_normal()
        && color_space.is_tagged()
        && upscale.is_off()
        && retime.is_nearest()
        && match (alone, placed.as_slice()) {
            (false, []) => true,
            (true, [p]) => {
                plain_picture(&p.picture) && p.ramp.is_none() && !p.crossing && p.moving.is_none()
            }
            _ => false,
        }
        && made.is_none()
        && input_lut.is_none()
        && match alpha {
            None => true,
            Some(AlphaOut::Premultiplied) => *alpha_mode == AlphaMode::Premultiplied,
            Some(AlphaOut::Straight) => *alpha_mode == AlphaMode::Straight,
        };
    plain.then(|| (path.as_path(), Clock::of(fps, *origin, *per_frame), piece))
}

fn plain_picture(picture: &Picture) -> bool {
    match &picture.grade {
        Some(g) if !g.is_active() => Picture {
            grade: None,
            ..picture.clone()
        }
        .is_usual(),
        _ => picture.is_usual(),
    }
}

fn outside(cues: &[Cue], from: i64, to: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut at = from;
    let mut covered: Vec<(i64, i64)> = cues
        .iter()
        .filter(|c| c.end > from && c.start < to)
        .map(|c| (c.start, c.end))
        .collect();
    covered.sort_unstable();
    for (a, b) in covered {
        if a > at {
            out.push((at, a.min(to)));
        }
        at = at.max(b);
    }
    if at < to {
        out.push((at, to));
    }
    out
}

fn gcd(a: i64, b: i64) -> i64 {
    match b {
        0 => a.abs(),
        _ => gcd(b, a % b),
    }
}

fn lcm(a: i64, b: i64) -> i64 {
    a / gcd(a, b).max(1) * b
}

pub(crate) struct FramesRef(*mut ffi::AVBufferRef);

// SAFETY: an AVBufferRef's count is atomic; the owner only refs and unrefs it.
unsafe impl Send for FramesRef {}

impl FramesRef {
    pub(crate) fn new(frames: *mut ffi::AVBufferRef) -> Result<Self> {
        // SAFETY: `frames` is a live reference that the caller holds.
        let own = unsafe { ffi::av_buffer_ref(frames) };
        match own.is_null() {
            true => Err(Error::Invalid("no memory for the encoder's frames".into())),
            false => Ok(Self(own)),
        }
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::AVBufferRef {
        self.0
    }
}

impl Drop for FramesRef {
    fn drop(&mut self) {
        // SAFETY: the reference that `new` took.
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}
