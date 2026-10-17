use std::os::raw::{c_int, c_uint, c_void};
use std::sync::OnceLock;

use vidcut_core::model::{DolbyVision, LightLevel, MasteringDisplay};

use crate::error::{Error, Result};
use crate::ffmpeg::{self, codec, ffi, frame};

use ffi::AVFrameSideDataType as Sd;

#[repr(C)]
struct AvMastering {
    display_primaries: [[ffi::AVRational; 2]; 3],
    white_point: [ffi::AVRational; 2],
    min_luminance: ffi::AVRational,
    max_luminance: ffi::AVRational,
    has_primaries: c_int,
    has_luminance: c_int,
}

#[repr(C)]
struct AvLight {
    max_cll: c_uint,
    max_fall: c_uint,
}

#[repr(C)]
struct AvDoviConfig {
    dv_version_major: u8,
    dv_version_minor: u8,
    dv_profile: u8,
    dv_level: u8,
    rpu_present_flag: u8,
    el_present_flag: u8,
    bl_present_flag: u8,
    dv_bl_signal_compatibility_id: u8,
}

unsafe extern "C" {
    fn av_mastering_display_metadata_alloc_size(size: *mut usize) -> *mut AvMastering;
    fn av_content_light_metadata_alloc(size: *mut usize) -> *mut AvLight;
    fn av_dynamic_hdr_plus_to_t35(s: *const c_void, data: *mut *mut u8, size: *mut usize) -> c_int;
    fn av_dynamic_hdr_plus_create_side_data(frame: *mut ffi::AVFrame) -> *mut c_void;
    fn av_dynamic_hdr_plus_from_t35(s: *mut c_void, data: *const u8, size: usize) -> c_int;
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Metadata {
    pub mastering: Option<MasteringDisplay>,
    pub light: Option<LightLevel>,
    pub hdr10plus: bool,
    pub dolby_vision: Option<DolbyVision>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Static {
    pub mastering: Option<MasteringDisplay>,
    pub light: Option<LightLevel>,
}

impl Static {
    pub fn is_empty(&self) -> bool {
        self.mastering.is_none() && self.light.is_none()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dynamic {
    pub hdr10plus: bool,
    pub dolby_vision: bool,
}

fn q2f(q: ffi::AVRational) -> f64 {
    if q.den == 0 {
        0.0
    } else {
        f64::from(q.num) / f64::from(q.den)
    }
}

fn f2q(v: f64, den: i32) -> ffi::AVRational {
    ffi::AVRational {
        num: (v * f64::from(den)).round().clamp(0.0, f64::from(i32::MAX)) as i32,
        den,
    }
}

const CHROMA: i32 = 50_000;
const LUMA: i32 = 10_000;

unsafe fn mastering_at(data: *const u8, size: usize) -> Option<MasteringDisplay> {
    if data.is_null() || size < std::mem::size_of::<AvMastering>() {
        return None;
    }
    let d = unsafe { &*(data as *const AvMastering) };
    if d.has_primaries == 0 || d.has_luminance == 0 {
        return None;
    }
    let xy = |q: &[ffi::AVRational; 2]| [q2f(q[0]), q2f(q[1])];
    Some(MasteringDisplay {
        primaries: [
            xy(&d.display_primaries[0]),
            xy(&d.display_primaries[1]),
            xy(&d.display_primaries[2]),
        ],
        white: xy(&d.white_point),
        min_nits: q2f(d.min_luminance),
        max_nits: q2f(d.max_luminance),
    })
}

unsafe fn light_at(data: *const u8, size: usize) -> Option<LightLevel> {
    if data.is_null() || size < std::mem::size_of::<AvLight>() {
        return None;
    }
    let d = unsafe { &*(data as *const AvLight) };
    (d.max_cll > 0).then_some(LightLevel {
        max_cll: d.max_cll,
        max_fall: d.max_fall,
    })
}

pub fn of_frame(f: &frame::Video) -> Metadata {
    unsafe {
        let f = f.as_ptr();
        let get = |t| {
            let sd = ffi::av_frame_get_side_data(f, t);
            if sd.is_null() {
                (std::ptr::null(), 0)
            } else {
                ((*sd).data as *const u8, (*sd).size)
            }
        };
        let (m, ms) = get(Sd::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA);
        let (l, ls) = get(Sd::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL);
        Metadata {
            mastering: mastering_at(m, ms),
            light: light_at(l, ls),
            hdr10plus: !get(Sd::AV_FRAME_DATA_DYNAMIC_HDR_PLUS).0.is_null(),
            dolby_vision: None,
        }
    }
}

pub fn of_stream(params: &codec::Parameters) -> Static {
    use ffi::AVPacketSideDataType as Pd;
    unsafe {
        let p = &*params.as_ptr();
        let get = |t| {
            let sd = ffi::av_packet_side_data_get(p.coded_side_data, p.nb_coded_side_data, t);
            if sd.is_null() {
                (std::ptr::null(), 0)
            } else {
                ((*sd).data as *const u8, (*sd).size)
            }
        };
        let (m, ms) = get(Pd::AV_PKT_DATA_MASTERING_DISPLAY_METADATA);
        let (l, ls) = get(Pd::AV_PKT_DATA_CONTENT_LIGHT_LEVEL);
        Static {
            mastering: mastering_at(m, ms),
            light: light_at(l, ls),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameHdr {
    pub pts: i64,
    pub metadata: Metadata,
    pub hdr10plus: Option<Vec<u8>>,
    pub dolby_vision: bool,
}

pub fn read_frames(path: &std::path::Path, container: bool, max: usize) -> Result<Vec<FrameHdr>> {
    use crate::error::Context;
    use ffi::AVPacketSideDataType as Pd;
    let mut input = ffmpeg::format::input(path).ctx(|| format!("opening {}", path.display()))?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| Error::Invalid(format!("{}: no video", path.display())))?;
    let index = stream.index();
    let params = stream.parameters();
    let mut ctx = codec::context::Context::from_parameters(params)
        .ctx(|| "reading video parameters".into())?;
    if !container {
        unsafe {
            let c = &mut *ctx.as_mut_ptr();
            for t in [
                Pd::AV_PKT_DATA_MASTERING_DISPLAY_METADATA,
                Pd::AV_PKT_DATA_CONTENT_LIGHT_LEVEL,
            ] {
                ffi::av_packet_side_data_remove(c.coded_side_data, &mut c.nb_coded_side_data, t);
            }
        }
    }
    let mut decoder = ctx.decoder().video().ctx(|| "opening the decoder".into())?;
    let mut out = Vec::new();
    let mut frame = frame::Video::empty();
    let mut take = |decoder: &mut ffmpeg::decoder::Video, out: &mut Vec<FrameHdr>| {
        while decoder.receive_frame(&mut frame).is_ok() {
            out.push(FrameHdr {
                pts: frame.pts().unwrap_or(i64::MIN),
                metadata: of_frame(&frame),
                hdr10plus: hdr10plus_t35(&frame),
                dolby_vision: unsafe {
                    !ffi::av_frame_get_side_data(frame.as_ptr(), Sd::AV_FRAME_DATA_DOVI_METADATA)
                        .is_null()
                },
            });
        }
    };
    for (s, packet) in input.packets() {
        if s.index() == index {
            decoder.send_packet(&packet).ctx(|| "decoding".into())?;
            take(&mut decoder, &mut out);
            if out.len() >= max.saturating_add(16) {
                break;
            }
        }
    }
    decoder.send_eof().ctx(|| "decoding".into())?;
    take(&mut decoder, &mut out);
    out.sort_by_key(|f| f.pts);
    out.truncate(max);
    Ok(out)
}

pub fn dolby_vision(params: &codec::Parameters) -> Option<DolbyVision> {
    unsafe {
        let p = &*params.as_ptr();
        let sd = ffi::av_packet_side_data_get(
            p.coded_side_data,
            p.nb_coded_side_data,
            ffi::AVPacketSideDataType::AV_PKT_DATA_DOVI_CONF,
        );
        if sd.is_null() || (*sd).size < std::mem::size_of::<AvDoviConfig>() {
            return None;
        }
        let c = &*((*sd).data as *const AvDoviConfig);
        (c.rpu_present_flag != 0).then_some(DolbyVision {
            profile: c.dv_profile,
            compatibility: c.dv_bl_signal_compatibility_id,
        })
    }
}

fn sizes() -> (usize, usize) {
    static SIZES: OnceLock<(usize, usize)> = OnceLock::new();
    *SIZES.get_or_init(|| unsafe {
        let (mut m, mut l) = (0usize, 0usize);
        ffi::av_free(av_mastering_display_metadata_alloc_size(&mut m).cast());
        ffi::av_free(av_content_light_metadata_alloc(&mut l).cast());
        (
            m.max(std::mem::size_of::<AvMastering>()),
            l.max(std::mem::size_of::<AvLight>()),
        )
    })
}

unsafe fn fill(sd: *mut ffi::AVFrameSideData, s: &Static) {
    unsafe {
        std::ptr::write_bytes((*sd).data, 0, (*sd).size);
        match (*sd).type_ {
            Sd::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA => {
                let m = s.mastering.expect("asked for");
                let d = &mut *((*sd).data as *mut AvMastering);
                for (to, from) in d.display_primaries.iter_mut().zip(m.primaries) {
                    *to = [f2q(from[0], CHROMA), f2q(from[1], CHROMA)];
                }
                d.white_point = [f2q(m.white[0], CHROMA), f2q(m.white[1], CHROMA)];
                d.min_luminance = f2q(m.min_nits, LUMA);
                d.max_luminance = f2q(m.max_nits, LUMA);
                d.has_primaries = 1;
                d.has_luminance = 1;
            }
            _ => {
                let l = s.light.expect("asked for");
                let d = &mut *((*sd).data as *mut AvLight);
                d.max_cll = l.max_cll;
                d.max_fall = l.max_fall;
            }
        }
    }
}

fn types(s: &Static) -> Vec<(Sd, usize)> {
    let (m, l) = sizes();
    let mut out = Vec::new();
    if s.mastering.is_some() {
        out.push((Sd::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA, m));
    }
    if s.light.is_some() {
        out.push((Sd::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL, l));
    }
    out
}

/// # Safety
/// `ctx` is an encoder context not opened yet.
pub unsafe fn set_on_encoder(ctx: *mut ffi::AVCodecContext, s: &Static) -> Result<()> {
    for (t, size) in types(s) {
        unsafe {
            let sd = ffi::av_frame_side_data_new(
                &mut (*ctx).decoded_side_data,
                &mut (*ctx).nb_decoded_side_data,
                t,
                size,
                ffi::AV_FRAME_SIDE_DATA_FLAG_UNIQUE as u32,
            );
            if sd.is_null() {
                return Err(Error::Invalid("out of memory for HDR metadata".into()));
            }
            fill(sd, s);
        }
    }
    Ok(())
}

pub fn attach(f: &mut frame::Video, s: &Static) -> Result<()> {
    for (t, size) in types(s) {
        unsafe {
            ffi::av_frame_remove_side_data(f.as_mut_ptr(), t);
            let sd = ffi::av_frame_new_side_data(f.as_mut_ptr(), t, size);
            if sd.is_null() {
                return Err(Error::Invalid("out of memory for HDR metadata".into()));
            }
            fill(sd, s);
        }
    }
    Ok(())
}

const ALL: [Sd; 5] = [
    Sd::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA,
    Sd::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
    Sd::AV_FRAME_DATA_DYNAMIC_HDR_PLUS,
    Sd::AV_FRAME_DATA_DOVI_METADATA,
    Sd::AV_FRAME_DATA_DOVI_RPU_BUFFER,
];

pub fn strip(f: &mut frame::Video) {
    for t in ALL {
        unsafe { ffi::av_frame_remove_side_data(f.as_mut_ptr(), t) };
    }
}

pub fn copy_dynamic(from: &frame::Video, to: &mut frame::Video, keep: Dynamic) -> Result<()> {
    let mut wanted = Vec::new();
    if keep.hdr10plus {
        wanted.push(Sd::AV_FRAME_DATA_DYNAMIC_HDR_PLUS);
    }
    if keep.dolby_vision {
        wanted.push(Sd::AV_FRAME_DATA_DOVI_METADATA);
    }
    for t in wanted {
        unsafe {
            let sd = ffi::av_frame_get_side_data(from.as_ptr(), t);
            if sd.is_null() {
                continue;
            }
            let buf = ffi::av_buffer_ref((*sd).buf);
            if buf.is_null()
                || ffi::av_frame_new_side_data_from_buf(to.as_mut_ptr(), t, buf).is_null()
            {
                return Err(Error::Invalid("out of memory for HDR metadata".into()));
            }
        }
    }
    Ok(())
}

const HDR10PLUS_T35: [u8; 6] = [0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04];

pub fn hdr10plus_t35(f: &frame::Video) -> Option<Vec<u8>> {
    unsafe {
        let sd = ffi::av_frame_get_side_data(f.as_ptr(), Sd::AV_FRAME_DATA_DYNAMIC_HDR_PLUS);
        if sd.is_null() {
            return None;
        }
        let (mut data, mut size) = (std::ptr::null_mut(), 0usize);
        if av_dynamic_hdr_plus_to_t35((*sd).data as *const c_void, &mut data, &mut size) < 0 {
            return None;
        }
        let mut out = HDR10PLUS_T35.to_vec();
        out.extend_from_slice(std::slice::from_raw_parts(data, size));
        ffi::av_free(data.cast());
        Some(out)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hdr10Plus {
    pub targeted_max_luminance: u32,
    pub maxscl: [f64; 3],
    pub average_maxrgb: f64,
    pub percentiles: Vec<(u8, f64)>,
}

impl Hdr10Plus {
    pub fn of_frame(f: &frame::Video) -> Option<Self> {
        let t35 = hdr10plus_t35(f)?;
        let mut bits = BitReader {
            data: t35.get(HDR10PLUS_T35.len()..)?,
            at: 0,
        };
        let _version = bits.take(8)?;
        let windows = bits.take(2)?;
        for _ in 1..windows {
            bits.take(16 * 4 + 16 * 2 + 8 + 16 * 3 + 1)?;
        }
        let targeted_max_luminance = bits.take(27)?;
        if bits.take(1)? == 1 {
            let (rows, columns) = (bits.take(5)?, bits.take(5)?);
            bits.take(rows * columns * 4)?;
        }
        let mut light = || bits.take(17).map(|v| f64::from(v) / 10.0);
        let maxscl = [light()?, light()?, light()?];
        let average_maxrgb = light()?;
        let n = bits.take(4)?;
        let mut percentiles = Vec::new();
        for _ in 0..n {
            let percentage = bits.take(7)? as u8;
            let nits = f64::from(bits.take(17)?) / 10.0;
            percentiles.push((percentage, nits));
        }
        Some(Self {
            targeted_max_luminance,
            maxscl,
            average_maxrgb,
            percentiles,
        })
    }

    pub fn brightest(mut self, other: &Self) -> Self {
        self.targeted_max_luminance = self
            .targeted_max_luminance
            .max(other.targeted_max_luminance);
        for (a, b) in self.maxscl.iter_mut().zip(other.maxscl) {
            *a = a.max(b);
        }
        let same = self.percentiles.len() == other.percentiles.len()
            && self
                .percentiles
                .iter()
                .zip(&other.percentiles)
                .all(|(a, b)| a.0 == b.0);
        if same {
            for (a, b) in self.percentiles.iter_mut().zip(&other.percentiles) {
                a.1 = a.1.max(b.1);
            }
        } else if other.average_maxrgb > self.average_maxrgb {
            self.percentiles = other.percentiles.clone();
        }
        self.average_maxrgb = self.average_maxrgb.max(other.average_maxrgb);
        self
    }

    pub fn t35(&self) -> Vec<u8> {
        struct Bits(Vec<u8>, u32);
        impl Bits {
            fn put(&mut self, value: u32, n: u32) {
                for i in (0..n).rev() {
                    if self.1.is_multiple_of(8) {
                        self.0.push(0);
                    }
                    let bit = (value >> i) & 1;
                    *self.0.last_mut().expect("pushed") |= (bit as u8) << (7 - self.1 % 8);
                    self.1 += 1;
                }
            }
        }
        let light = |nits: f64| ((nits * 10.0).round() as u32).min((1 << 17) - 1);
        let mut b = Bits(HDR10PLUS_T35.to_vec(), 48);
        b.put(1, 8);
        b.put(1, 2);
        b.put(self.targeted_max_luminance, 27);
        b.put(0, 1);
        for c in self.maxscl {
            b.put(light(c), 17);
        }
        b.put(light(self.average_maxrgb), 17);
        let percentiles = &self.percentiles[..self.percentiles.len().min(15)];
        b.put(percentiles.len() as u32, 4);
        for &(percentage, nits) in percentiles {
            b.put(u32::from(percentage.min(100)), 7);
            b.put(light(nits), 17);
        }
        b.put(0, 10);
        b.put(0, 1);
        b.put(0, 1);
        b.put(0, 1);
        b.0
    }

    pub fn attach(&self, f: &mut frame::Video) -> Result<()> {
        let t35 = self.t35();
        let payload = &t35[HDR10PLUS_T35.len()..];
        unsafe {
            ffi::av_frame_remove_side_data(f.as_mut_ptr(), Sd::AV_FRAME_DATA_DYNAMIC_HDR_PLUS);
            let sd = av_dynamic_hdr_plus_create_side_data(f.as_mut_ptr());
            if sd.is_null() {
                return Err(Error::Invalid("out of memory for HDR metadata".into()));
            }
            if av_dynamic_hdr_plus_from_t35(sd, payload.as_ptr(), payload.len()) < 0 {
                return Err(Error::Invalid("HDR10+ that FFmpeg doesn't read".into()));
            }
        }
        Ok(())
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    at: usize,
}

impl BitReader<'_> {
    fn take(&mut self, n: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.at / 8)?;
            let bit = (byte >> (7 - self.at % 8)) & 1;
            value = value.wrapping_shl(1) | u32::from(bit);
            self.at += 1;
        }
        Some(value)
    }
}

pub fn has_hdr10plus(f: &frame::Video) -> bool {
    unsafe {
        !ffi::av_frame_get_side_data(f.as_ptr(), Sd::AV_FRAME_DATA_DYNAMIC_HDR_PLUS).is_null()
    }
}

pub fn insert_hdr10plus(packet: &mut ffmpeg::Packet, id: codec::Id, t35: &[u8]) -> Result<()> {
    let Some(data) = packet.data() else {
        return Ok(());
    };
    let out = match id {
        codec::Id::HEVC => hevc_with_sei(data, t35),
        codec::Id::AV1 => av1_with_metadata(data, t35),
        _ => return Ok(()),
    }
    .ok_or_else(|| Error::Invalid("no picture in the packet for HDR10+".into()))?;
    use ffmpeg::packet::{Mut, Ref};
    let mut new = ffmpeg::Packet::copy(&out);
    if unsafe { ffi::av_packet_copy_props(new.as_mut_ptr(), packet.as_ptr()) } < 0 {
        return Err(Error::Invalid("out of memory for HDR10+".into()));
    }
    *packet = new;
    Ok(())
}

pub fn insert_static(packet: &mut ffmpeg::Packet, id: codec::Id, s: &Static) -> Result<()> {
    if s.is_empty() || !packet.is_key() {
        return Ok(());
    }
    let Some(data) = packet.data() else {
        return Ok(());
    };
    let out = match id {
        codec::Id::HEVC if !hevc_has_static(data) => hevc_with_static(data, s),
        codec::Id::AV1 if !av1_has_static(data) => av1_with_static(data, s),
        _ => return Ok(()),
    };
    let Some(out) = out else {
        return Ok(());
    };
    use ffmpeg::packet::{Mut, Ref};
    let mut new = ffmpeg::Packet::copy(&out);
    if unsafe { ffi::av_packet_copy_props(new.as_mut_ptr(), packet.as_ptr()) } < 0 {
        return Err(Error::Invalid("out of memory for HDR metadata".into()));
    }
    *packet = new;
    Ok(())
}

fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn sei_types(rbsp: &[u8]) -> Vec<usize> {
    let mut types = Vec::new();
    let mut at = 0;
    let ff = |at: &mut usize| -> Option<usize> {
        let mut v = 0;
        loop {
            let b = *rbsp.get(*at)?;
            *at += 1;
            v += usize::from(b);
            if b != 255 {
                return Some(v);
            }
        }
    };
    while at < rbsp.len() && !(at + 1 == rbsp.len() && rbsp[at] == 0x80) {
        let (Some(t), Some(size)) = (ff(&mut at), ff(&mut at)) else {
            break;
        };
        types.push(t);
        at += size;
    }
    types
}

fn hevc_has_static(data: &[u8]) -> bool {
    let Some((_, units)) = nal_units(data) else {
        return false;
    };
    units.iter().any(|&(_, s, e)| {
        e > s + 2
            && (data[s] >> 1) & 0x3F == 39
            && sei_types(&unescape(&data[s + 2..e]))
                .iter()
                .any(|&t| t == 137 || t == 144)
    })
}

fn hevc_with_static(data: &[u8], s: &Static) -> Option<Vec<u8>> {
    let mut rbsp = Vec::new();
    if let Some(m) = s.mastering {
        let c = |v: f64| ((v * 50_000.0).round() as u16).to_be_bytes();
        let l = |v: f64| ((v * 10_000.0).round() as u32).to_be_bytes();
        let mut payload = Vec::with_capacity(24);
        for xy in [m.primaries[1], m.primaries[2], m.primaries[0], m.white] {
            payload.extend_from_slice(&c(xy[0]));
            payload.extend_from_slice(&c(xy[1]));
        }
        payload.extend_from_slice(&l(m.max_nits));
        payload.extend_from_slice(&l(m.min_nits));
        ff_coded(137, &mut rbsp);
        ff_coded(payload.len(), &mut rbsp);
        rbsp.extend(payload);
    }
    if let Some(light) = s.light {
        let v = |n: u32| (n.min(u32::from(u16::MAX)) as u16).to_be_bytes();
        ff_coded(144, &mut rbsp);
        ff_coded(4, &mut rbsp);
        rbsp.extend_from_slice(&v(light.max_cll));
        rbsp.extend_from_slice(&v(light.max_fall));
    }
    rbsp.push(0x80);
    let (annex_b, units) = nal_units(data)?;
    let &(prefix, start, _) = units
        .iter()
        .find(|&&(_, s, e)| e > s + 1 && (data[s] >> 1) & 0x3F < 32)?;
    let mut sei = vec![39 << 1, data[start + 1] & 0x07];
    sei.extend(escape(&rbsp));
    let mut out = Vec::with_capacity(data.len() + sei.len() + 4);
    out.extend_from_slice(&data[..prefix]);
    if annex_b {
        out.extend_from_slice(&[0, 0, 0, 1]);
    } else {
        out.extend_from_slice(&(sei.len() as u32).to_be_bytes());
    }
    out.extend_from_slice(&sei);
    out.extend_from_slice(&data[prefix..]);
    Some(out)
}

fn obus(data: &[u8]) -> Option<Vec<(u8, usize, usize, usize)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let start = at;
        let header = data[at];
        at += 1 + usize::from(header & 0x04 != 0);
        let size = if header & 0x02 != 0 {
            read_leb128(data, &mut at)?
        } else {
            data.len().checked_sub(at)?
        };
        let end = at.checked_add(size).filter(|&e| e <= data.len())?;
        out.push(((header >> 3) & 0x0F, start, at, end));
        at = end;
    }
    Some(out)
}

fn av1_has_static(data: &[u8]) -> bool {
    obus(data).is_some_and(|o| {
        o.iter().any(|&(kind, _, p, e)| {
            let mut at = p;
            kind == 5 && matches!(read_leb128(&data[..e], &mut at), Some(1 | 2))
        })
    })
}

fn av1_with_static(data: &[u8], s: &Static) -> Option<Vec<u8>> {
    let obu = |metadata_type: usize, body: &[u8]| {
        let mut payload = Vec::new();
        leb128(metadata_type, &mut payload);
        payload.extend_from_slice(body);
        payload.push(0x80);
        let mut o = vec![(5 << 3) | 0x02];
        leb128(payload.len(), &mut o);
        o.extend(payload);
        o
    };
    let mut add = Vec::new();
    if let Some(light) = s.light {
        let v = |n: u32| (n.min(u32::from(u16::MAX)) as u16).to_be_bytes();
        add.extend(obu(1, &[v(light.max_cll), v(light.max_fall)].concat()));
    }
    if let Some(m) = s.mastering {
        let c = |v: f64| ((v * 65_536.0).round().min(65_535.0) as u16).to_be_bytes();
        let mut body = Vec::with_capacity(24);
        for xy in [m.primaries[0], m.primaries[1], m.primaries[2], m.white] {
            body.extend_from_slice(&c(xy[0]));
            body.extend_from_slice(&c(xy[1]));
        }
        body.extend_from_slice(&((m.max_nits * 256.0).round() as u32).to_be_bytes());
        body.extend_from_slice(&((m.min_nits * 16_384.0).round() as u32).to_be_bytes());
        add.extend(obu(2, &body));
    }
    let first = obus(data)?
        .into_iter()
        .find(|&(kind, ..)| kind == 3 || kind == 6)?
        .1;
    let mut out = Vec::with_capacity(data.len() + add.len());
    out.extend_from_slice(&data[..first]);
    out.extend(add);
    out.extend_from_slice(&data[first..]);
    Some(out)
}

fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 64 + 4);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn ff_coded(mut v: usize, out: &mut Vec<u8>) {
    while v >= 255 {
        out.push(255);
        v -= 255;
    }
    out.push(v as u8);
}

fn hevc_sei(t35: &[u8], tid1: u8) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(t35.len() + 8);
    ff_coded(4, &mut rbsp);
    ff_coded(t35.len(), &mut rbsp);
    rbsp.extend_from_slice(t35);
    rbsp.push(0x80);
    let mut nal = vec![39 << 1, tid1];
    nal.extend(escape(&rbsp));
    nal
}

type Unit = (usize, usize, usize);

fn nal_units(data: &[u8]) -> Option<(bool, Vec<Unit>)> {
    let annex_b = data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1]);
    let mut units = Vec::new();
    if annex_b {
        let mut starts = Vec::new();
        let mut i = 0;
        while i + 3 <= data.len() {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                let prefix = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
                starts.push((prefix, i + 3));
                i += 3;
            } else {
                i += 1;
            }
        }
        for (k, &(prefix, start)) in starts.iter().enumerate() {
            let end = starts.get(k + 1).map_or(data.len(), |n| n.0);
            units.push((prefix, start, end));
        }
    } else {
        let mut i = 0;
        while i + 4 <= data.len() {
            let n = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            if n == 0 || i + 4 + n > data.len() {
                return None;
            }
            units.push((i, i + 4, i + 4 + n));
            i += 4 + n;
        }
    }
    Some((annex_b, units))
}

fn hevc_with_sei(data: &[u8], t35: &[u8]) -> Option<Vec<u8>> {
    let (annex_b, units) = nal_units(data)?;
    let &(prefix, start, _) = units
        .iter()
        .find(|&&(_, s, e)| e > s + 1 && (data[s] >> 1) & 0x3F < 32)?;
    let sei = hevc_sei(t35, data[start + 1] & 0x07);
    let mut out = Vec::with_capacity(data.len() + sei.len() + 4);
    out.extend_from_slice(&data[..prefix]);
    if annex_b {
        out.extend_from_slice(&[0, 0, 0, 1]);
    } else {
        out.extend_from_slice(&(sei.len() as u32).to_be_bytes());
    }
    out.extend_from_slice(&sei);
    out.extend_from_slice(&data[prefix..]);
    Some(out)
}

fn leb128(mut v: usize, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn read_leb128(data: &[u8], at: &mut usize) -> Option<usize> {
    let mut v = 0usize;
    for i in 0..8 {
        let b = *data.get(*at)?;
        *at += 1;
        v |= usize::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn av1_metadata(t35: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(t35.len() + 2);
    leb128(4, &mut payload);
    payload.extend_from_slice(t35);
    payload.push(0x80);
    let mut obu = vec![(5 << 3) | 0x02];
    leb128(payload.len(), &mut obu);
    obu.extend(payload);
    obu
}

fn av1_with_metadata(data: &[u8], t35: &[u8]) -> Option<Vec<u8>> {
    let mut at = 0;
    let first = loop {
        let start = at;
        let header = *data.get(at)?;
        let kind = (header >> 3) & 0x0F;
        at += 1 + usize::from(header & 0x04 != 0);
        let size = if header & 0x02 != 0 {
            read_leb128(data, &mut at)?
        } else {
            data.len().checked_sub(at)?
        };
        if kind == 3 || kind == 6 {
            break start;
        }
        at = at.checked_add(size).filter(|&e| e <= data.len())?;
    };
    let obu = av1_metadata(t35);
    let mut out = Vec::with_capacity(data.len() + obu.len());
    out.extend_from_slice(&data[..first]);
    out.extend_from_slice(&obu);
    out.extend_from_slice(&data[first..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::Hdr10Plus;
    use crate::ffmpeg::{format::Pixel, frame};

    fn light(maxscl: [f64; 3], average: f64, percentiles: &[(u8, f64)]) -> Hdr10Plus {
        Hdr10Plus {
            targeted_max_luminance: 0,
            maxscl,
            average_maxrgb: average,
            percentiles: percentiles.to_vec(),
        }
    }

    #[test]
    fn hdr10plus_on_a_frame_reads_back_as_written() {
        let written = light([100.0, 200.5, 50.0], 30.2, &[(50, 20.0), (99, 150.0)]);
        let mut f = frame::Video::new(Pixel::YUV420P10LE, 16, 16);
        written.attach(&mut f).unwrap();
        assert_eq!(Hdr10Plus::of_frame(&f), Some(written));
    }

    #[test]
    fn the_brightest_of_two_takes_each_values_higher() {
        let a = light([100.0, 20.0, 50.0], 30.0, &[(50, 20.0), (99, 150.0)]);
        let b = light([10.0, 200.0, 5.0], 40.0, &[(50, 25.0), (99, 90.0)]);
        assert_eq!(
            a.brightest(&b),
            light([100.0, 200.0, 50.0], 40.0, &[(50, 25.0), (99, 150.0)])
        );
    }
}
