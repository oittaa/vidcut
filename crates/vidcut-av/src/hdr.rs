//! HDR metadata: the mastering display and content light levels a stream
//! has once (static), and HDR10+ and Dolby Vision on each frame (dynamic).
//! Read from decoded frames, written to encoders and frames, and HDR10+ put
//! into HEVC and AV1 bitstreams, which FFmpeg's encoders don't do.

use std::os::raw::{c_int, c_uint, c_void};
use std::sync::OnceLock;

use vidcut_core::model::{DolbyVision, LightLevel, MasteringDisplay};

use crate::error::{Error, Result};
use crate::ffmpeg::{self, codec, ffi, frame};

use ffi::AVFrameSideDataType as Sd;

// FFmpeg's structs (libavutil/mastering_display_metadata.h, dovi_meta.h),
// which ffmpeg-sys-next doesn't bind. Their sizes aren't ABI: buffers come
// from FFmpeg's allocators, and only these leading fields are used.
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

/// What a frame (and its stream) says of its HDR.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Metadata {
    pub mastering: Option<MasteringDisplay>,
    pub light: Option<LightLevel>,
    pub hdr10plus: bool,
    pub dolby_vision: Option<DolbyVision>,
}

/// The static values an output carries.
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

/// The dynamic metadata an output keeps from its sources' frames.
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

/// `v` as a fraction of `den`, as HEVC's SEI codes it: chromaticity in
/// 1/50000, luminance in 1/10000 cd/m².
fn f2q(v: f64, den: i32) -> ffi::AVRational {
    ffi::AVRational {
        num: (v * f64::from(den)).round().clamp(0.0, f64::from(i32::MAX)) as i32,
        den,
    }
}

const CHROMA: i32 = 50_000;
const LUMA: i32 = 10_000;

/// A mastering display from FFmpeg's struct at `data` (`size` bytes).
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

/// Light levels from FFmpeg's struct at `data` (`size` bytes); none when
/// MaxCLL is 0 (unknown).
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

/// What `f` carries: its static values, and whether it has HDR10+ or a
/// Dolby Vision RPU (the profile comes from the stream, [`dolby_vision`]).
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

/// The static values in a stream's parameters: what a container's boxes
/// (MP4 `mdcv`/`clli`, Matroska's) said, or what an encoder was given.
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

/// One decoded frame's HDR metadata, its HDR10+ as T.35, and whether it has
/// a Dolby Vision RPU.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameHdr {
    pub pts: i64,
    pub metadata: Metadata,
    pub hdr10plus: Option<Vec<u8>>,
    pub dolby_vision: bool,
}

/// The HDR metadata of a file's first `max` frames or so (decoding stops
/// once that many came out), in presentation order: with `container`, as a
/// player sees it; without, from the bitstream alone (the stream's own
/// values removed before decoding). For tests and checks.
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
            // Some past `max`: reordered frames still to come are earlier.
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

/// The Dolby Vision a stream declares (MP4 `dvcC`/`dvvC`, Matroska's
/// configuration), if it carries an RPU.
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

/// FFmpeg's sizes of the two structs, which may grow.
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

/// Fills side data `sd` (zeroed, `size` long) with `s`'s value of its type.
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

/// The side data types `s` has, with their sizes.
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

/// Gives an encoder about to be opened `s`: libx265, libsvtav1 and NVENC
/// write it into the bitstream, and FFmpeg copies it into the stream's
/// parameters, from which MP4, MOV, Matroska and WebM write it.
///
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

/// Attaches `s` to a frame, for encoders that read it frame by frame
/// (VAAPI) or prefer that (NVENC).
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

/// Removes all HDR metadata from a frame: what reaches the encoder is
/// what the output was given, not what a source had.
pub fn strip(f: &mut frame::Video) {
    for t in ALL {
        unsafe { ffi::av_frame_remove_side_data(f.as_mut_ptr(), t) };
    }
}

/// Copies the dynamic metadata `keep` names from a source frame onto an
/// output frame made from it.
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

/// The ITU-T T.35 header of HDR10+ (SMPTE ST 2094-40): the US, Samsung,
/// provider-oriented code 1, application 4.
const HDR10PLUS_T35: [u8; 6] = [0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04];

/// A frame's HDR10+ as an ITU-T T.35 payload, header included: what an
/// HEVC SEI or an AV1 metadata OBU carries.
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

/// One frame's HDR10+ (SMPTE ST 2094-40) with one window and no tone
/// mapping curve: its light in cd/m². What frames without it are given
/// (decision D46).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hdr10Plus {
    /// The display it was graded for, cd/m²; 0 for none.
    pub targeted_max_luminance: u32,
    /// Each channel's brightest, red, green, blue.
    pub maxscl: [f64; 3],
    /// The brightest channel of each pixel, averaged.
    pub average_maxrgb: f64,
    /// That brightest channel at percentages of the pixels.
    pub percentiles: Vec<(u8, f64)>,
}

impl Hdr10Plus {
    /// As an ITU-T T.35 payload, header included ([`hdr10plus_t35`]).
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
        // Light in 1/100000 of 10000 cd/m², 17 bits.
        let light = |nits: f64| ((nits * 10.0).round() as u32).min((1 << 17) - 1);
        let mut b = Bits(HDR10PLUS_T35.to_vec(), 48);
        b.put(1, 8); // application_version
        b.put(1, 2); // num_windows
        b.put(self.targeted_max_luminance, 27);
        b.put(0, 1); // targeted_system_display_actual_peak_luminance_flag
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
        b.put(0, 10); // fraction_bright_pixels
        b.put(0, 1); // mastering_display_actual_peak_luminance_flag
        b.put(0, 1); // tone_mapping_flag
        b.put(0, 1); // color_saturation_mapping_flag
        b.0
    }

    /// Puts it on `f`, in place of any it had.
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

/// Whether `f` has HDR10+.
pub fn has_hdr10plus(f: &frame::Video) -> bool {
    unsafe {
        !ffi::av_frame_get_side_data(f.as_ptr(), Sd::AV_FRAME_DATA_DYNAMIC_HDR_PLUS).is_null()
    }
}

/// Puts `t35` (HDR10+, from [`hdr10plus_t35`]) into an encoded HEVC or AV1
/// packet: a prefix SEI before its first slice (Annex B or 4-byte length
/// prefixed NAL units), or a metadata OBU before its first frame.
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
    // New data; the timing, flags and side data stay.
    use ffmpeg::packet::{Mut, Ref};
    let mut new = ffmpeg::Packet::copy(&out);
    if unsafe { ffi::av_packet_copy_props(new.as_mut_ptr(), packet.as_ptr()) } < 0 {
        return Err(Error::Invalid("out of memory for HDR10+".into()));
    }
    *packet = new;
    Ok(())
}

/// Puts `s` into an encoded HEVC or AV1 keyframe packet that doesn't carry
/// it yet: a mastering display colour volume (137) and content light level
/// (144) SEI, or HDR_MDCV and HDR_CLL metadata OBUs. Encoders that write
/// none (NVENC built without its SDK 13 headers) then carry them in the
/// bitstream too; those that do keep theirs.
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

/// Removes emulation prevention bytes.
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

/// The payload types of an SEI NAL unit's messages (its RBSP after the
/// two header bytes).
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
    // Messages until the trailing bits.
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
        // HEVC orders the primaries green, blue, red.
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

/// An AV1 temporal unit's OBUs: (type, start of the OBU, start of its
/// payload, end).
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
        // Chromaticity in 0.16 fixed point, red, green, blue; luminance in
        // 24.8 (max) and 18.14 (min).
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

/// Inserts emulation prevention bytes: no `00 00 0x` with x ≤ 3 in an RBSP.
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

/// SEI's ff-coded numbers: 255s, then the rest.
fn ff_coded(mut v: usize, out: &mut Vec<u8>) {
    while v >= 255 {
        out.push(255);
        v -= 255;
    }
    out.push(v as u8);
}

/// A prefix SEI NAL unit (type 39) with one user data registered by ITU-T
/// T.35 message (payload type 4), at temporal id `tid1` (plus one).
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

/// Where a NAL unit lies in an access unit: its start code or length,
/// itself, and its end.
type Unit = (usize, usize, usize);

/// The NAL units of an HEVC access unit, and whether it uses Annex B start
/// codes (else 4-byte lengths).
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
    // The first slice: VCL NAL unit types are 0 to 31.
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

/// A metadata OBU (type 5) of ITU-T T.35 metadata (type 4), with its size.
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
    // Before the first frame header or frame OBU (types 3 and 6).
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
    use super::*;

    #[test]
    fn the_mirrors_fit_in_what_ffmpeg_allocates() {
        crate::init();
        let (m, l) = unsafe {
            let (mut m, mut l) = (0usize, 0usize);
            ffi::av_free(av_mastering_display_metadata_alloc_size(&mut m).cast());
            ffi::av_free(av_content_light_metadata_alloc(&mut l).cast());
            (m, l)
        };
        assert!(m >= std::mem::size_of::<AvMastering>(), "{m}");
        assert!(l >= std::mem::size_of::<AvLight>(), "{l}");
    }

    #[test]
    fn values_round_trip_through_a_frame() {
        crate::init();
        let s = Static {
            mastering: Some(MasteringDisplay {
                primaries: [[0.708, 0.292], [0.17, 0.797], [0.131, 0.046]],
                white: [0.3127, 0.329],
                min_nits: 0.0001,
                max_nits: 1000.0,
            }),
            light: Some(LightLevel {
                max_cll: 1000,
                max_fall: 400,
            }),
        };
        let mut f = frame::Video::new(ffmpeg::format::Pixel::YUV420P10LE, 16, 16);
        attach(&mut f, &s).unwrap();
        let m = of_frame(&f);
        assert_eq!((m.mastering, m.light), (s.mastering, s.light));
        strip(&mut f);
        assert_eq!(of_frame(&f), Metadata::default());
    }

    #[test]
    fn emulation_prevention_and_ff_coding() {
        assert_eq!(
            escape(&[0, 0, 1, 0, 0, 0, 0, 4]),
            [0, 0, 3, 1, 0, 0, 3, 0, 0, 4]
        );
        let mut v = Vec::new();
        ff_coded(300, &mut v);
        assert_eq!(v, [255, 45]);
        let mut v = Vec::new();
        leb128(300, &mut v);
        assert_eq!(v, [0xAC, 0x02]);
        let mut at = 0;
        assert_eq!(read_leb128(&v, &mut at), Some(300));
    }

    #[test]
    fn the_sei_goes_before_the_first_slice() {
        // An AUD (35), a VPS (32) and an IDR slice (19), Annex B.
        let au = [
            0, 0, 0, 1, 70, 1, 0x50, 0, 0, 0, 1, 64, 1, 0xAA, 0, 0, 1, 38, 1, 0xBB,
        ];
        let t35 = [0xB5, 0, 0x3C, 0, 1, 4, 1];
        let out = hevc_with_sei(&au, &t35).unwrap();
        let (_, units) = nal_units(&out).unwrap();
        let types: Vec<u8> = units
            .iter()
            .map(|&(_, s, _)| (out[s] >> 1) & 0x3F)
            .collect();
        assert_eq!(types, [35, 32, 39, 19]);
        let (_, s, e) = units[2];
        assert_eq!(&out[s..e], hevc_sei(&t35, 1).as_slice());
        // Length prefixed, as MP4 stores it.
        let mut mp4 = Vec::new();
        for nal in [&[64u8, 1, 0xAA][..], &[38, 1, 0xBB]] {
            mp4.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            mp4.extend_from_slice(nal);
        }
        let out = hevc_with_sei(&mp4, &t35).unwrap();
        let (annex_b, units) = nal_units(&out).unwrap();
        assert!(!annex_b);
        let types: Vec<u8> = units
            .iter()
            .map(|&(_, s, _)| (out[s] >> 1) & 0x3F)
            .collect();
        assert_eq!(types, [32, 39, 19]);
    }

    #[test]
    fn static_values_go_into_keyframes_without_them() {
        let s = Static {
            mastering: Some(MasteringDisplay {
                primaries: [[0.68, 0.32], [0.265, 0.69], [0.15, 0.06]],
                white: [0.3127, 0.329],
                min_nits: 0.0001,
                max_nits: 1000.0,
            }),
            light: Some(LightLevel {
                max_cll: 1000,
                max_fall: 400,
            }),
        };
        let au = [0, 0, 0, 1, 64, 1, 0xAA, 0, 0, 1, 38, 1, 0xBB];
        assert!(!hevc_has_static(&au));
        let out = hevc_with_static(&au, &s).unwrap();
        assert!(hevc_has_static(&out));
        let (_, units) = nal_units(&out).unwrap();
        let (_, st, e) = units[1];
        assert_eq!(sei_types(&unescape(&out[st + 2..e])), [137, 144]);
        // Green first: 0.265 × 50000 = 13250.
        assert_eq!(&out[st + 4..st + 6], &13_250u16.to_be_bytes());
        let tu = [0x12, 0, 0x0A, 2, 0xAA, 0xBB, 0x32, 1, 0xCC];
        assert!(!av1_has_static(&tu));
        let out = av1_with_static(&tu, &s).unwrap();
        assert!(av1_has_static(&out));
        let kinds: Vec<u8> = obus(&out).unwrap().iter().map(|o| o.0).collect();
        assert_eq!(kinds, [2, 1, 5, 5, 6]);
    }

    #[test]
    fn the_obu_goes_before_the_first_frame() {
        // A temporal delimiter, a sequence header and a frame, with sizes.
        let tu = [0x12, 0, 0x0A, 2, 0xAA, 0xBB, 0x32, 1, 0xCC];
        let t35 = [0xB5, 0, 0x3C, 0, 1, 4, 1];
        let out = av1_with_metadata(&tu, &t35).unwrap();
        let obu = av1_metadata(&t35);
        assert_eq!(&out[..6], &tu[..6]);
        assert_eq!(&out[6..6 + obu.len()], obu.as_slice());
        assert_eq!(&out[6 + obu.len()..], &tu[6..]);
        assert_eq!(obu[0] >> 3, 5);
    }
}
