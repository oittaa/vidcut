use std::path::{Path, PathBuf};

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, encoder, format, frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    Mp4,
    Mov,
    Mkv,
    MpegTs,
    Webm,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Mkv => "mkv",
            Self::MpegTs => "ts",
            Self::Webm => "webm",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Mkv => "matroska",
            Self::MpegTs => "mpegts",
            Self::Webm => "webm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    H264,
    Hevc10,
    ProRes422Hq,
    ProRes4444,
    ProRes4444Alpha,
    DnxHrHq,
    Vp9,
    Ffv1,
    Av1,
}

impl VideoCodec {
    fn tag(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc10 => "hevc10",
            Self::ProRes422Hq => "prores",
            Self::ProRes4444 => "prores4444",
            Self::ProRes4444Alpha => "prores4444a",
            Self::DnxHrHq => "dnxhr",
            Self::Vp9 => "vp9",
            Self::Ffv1 => "ffv1",
            Self::Av1 => "av1",
        }
    }

    fn encoder(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::Hevc10 => "libx265",
            Self::ProRes422Hq | Self::ProRes4444 | Self::ProRes4444Alpha => "prores_ks",
            Self::DnxHrHq => "dnxhd",
            Self::Vp9 => "libvpx-vp9",
            Self::Ffv1 => "ffv1",
            Self::Av1 => "libsvtav1",
        }
    }

    fn pixel(self) -> format::Pixel {
        match self {
            Self::H264 | Self::Vp9 | Self::Ffv1 => format::Pixel::YUV420P,
            Self::Hevc10 | Self::Av1 => format::Pixel::YUV420P10LE,
            Self::ProRes422Hq => format::Pixel::YUV422P10LE,
            Self::ProRes4444 => format::Pixel::YUV444P10LE,
            Self::ProRes4444Alpha => format::Pixel::YUVA444P10LE,
            Self::DnxHrHq => format::Pixel::YUV422P,
        }
    }

    fn intra_only(self) -> bool {
        matches!(
            self,
            Self::ProRes422Hq
                | Self::ProRes4444
                | Self::ProRes4444Alpha
                | Self::DnxHrHq
                | Self::Ffv1
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioCodec {
    Aac,
    PcmS16,
    Flac,
    Opus,
}

impl AudioCodec {
    fn tag(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::PcmS16 => "pcm",
            Self::Flac => "flac",
            Self::Opus => "opus",
        }
    }

    fn encoder(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::PcmS16 => "pcm_s16le",
            Self::Flac => "flac",
            Self::Opus => "libopus",
        }
    }

    fn sample(self) -> format::Sample {
        use format::sample::Type;
        match self {
            Self::Aac => format::Sample::F32(Type::Planar),
            Self::Opus => format::Sample::F32(Type::Packed),
            Self::PcmS16 | Self::Flac => format::Sample::I16(Type::Packed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioSpec {
    pub codec: AudioCodec,
    pub rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Picture {
    Barcode,
    Black,
    Shots { frames: u32 },
    Colors,
    Noise,
    Busy,
    Highlight,
}

impl Picture {
    pub const COLORS: [(u8, u8, u8); 4] = [
        (100, 90, 170),
        (150, 160, 100),
        (120, 100, 110),
        (80, 150, 150),
    ];

    pub fn highlight(w: u32, h: u32) -> (u32, u32) {
        ((w / 2) | 1, (h / 2) | 1)
    }

    pub fn shot_level(s: u32) -> u8 {
        [70, 190, 120, 16][(s % 4) as usize]
    }
}

fn paint_chroma<const N: usize>(
    f: &mut frame::Video,
    p: usize,
    rotation: u16,
    bytes: impl Fn(u8) -> [u8; N],
) {
    let (lw, lh) = (f.width() as usize, f.height() as usize);
    let (w, h, stride) = (
        f.plane_width(p) as usize,
        f.plane_height(p) as usize,
        f.stride(p),
    );
    let upright = if rotation % 180 == 90 { lh } else { lw };
    let data = f.data_mut(p);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x * lw.div_ceil(w), y * lh.div_ceil(h));
            let shown = match rotation {
                90 => upright - 1 - sy,
                180 => upright - 1 - sx,
                270 => sy,
                _ => sx,
            };
            let (_, cb, cr) = Picture::COLORS[shown * 4 / upright];
            let at = y * stride + N * x;
            data[at..at + N].copy_from_slice(&bytes(if p == 1 { cb } else { cr }));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    Beep,
    Silence,
    Tone,
}

pub const TONE_AMPLITUDE: f32 = 0.1;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FixtureSpec {
    pub container: Container,
    pub video: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: (u32, u32),
    pub frames: u32,
    pub gop: u32,
    pub bframes: u32,
    pub vfr: bool,
    pub audio: Option<AudioSpec>,
    pub start_offset_ms: u32,
    pub video_enabled: bool,
    pub picture: Picture,
    pub sound: Sound,
    pub rotation: u16,
    pub top_field_first: Option<bool>,
    pub hdr: Option<vidcut_core::model::Hdr>,
    pub timecode: Option<String>,
    pub hdr_meta: Option<HdrMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HdrMeta {
    pub max_nits: u32,
    pub light: Option<(u32, u32)>,
    pub hdr10plus: bool,
    pub hdr10plus_frames: Option<u32>,
}

impl HdrMeta {
    pub fn values(&self) -> crate::hdr::Static {
        crate::hdr::Static {
            mastering: Some(mastering(self.max_nits)),
            light: self
                .light
                .map(|(max_cll, max_fall)| vidcut_core::model::LightLevel { max_cll, max_fall }),
        }
    }
}

pub fn mastering(max_nits: u32) -> vidcut_core::model::MasteringDisplay {
    vidcut_core::model::MasteringDisplay {
        primaries: [[0.68, 0.32], [0.265, 0.69], [0.15, 0.06]],
        white: [0.3127, 0.329],
        min_nits: 0.0001,
        max_nits: f64::from(max_nits),
    }
}

pub fn hdr10plus_t35(k: u32) -> Vec<u8> {
    let light = |raw: u32| f64::from(raw) / 10.0;
    crate::hdr::Hdr10Plus {
        targeted_max_luminance: 400,
        maxscl: [0, 1, 2].map(|c| light(10_000 + 100 * k + c)),
        average_maxrgb: light(5_000 + 100 * k),
        percentiles: Vec::new(),
    }
    .t35()
}

pub const PQ_WHITE: u16 = 573;
pub const HLG_WHITE: u16 = 721;

const VFR_PATTERN: [i64; 4] = [2, 6, 4, 4];

impl FixtureSpec {
    pub fn new(
        video: VideoCodec,
        container: Container,
        width: u32,
        height: u32,
        fps: (u32, u32),
        frames: u32,
    ) -> Self {
        let audio_codec = match container {
            Container::Mov => AudioCodec::PcmS16,
            Container::Webm => AudioCodec::Opus,
            _ => AudioCodec::Aac,
        };
        Self {
            container,
            video,
            width,
            height,
            fps,
            frames,
            gop: if video.intra_only() {
                1
            } else {
                fps.0.div_ceil(fps.1)
            },
            bframes: if matches!(video, VideoCodec::H264 | VideoCodec::Hevc10) {
                2
            } else {
                0
            },
            vfr: false,
            audio: Some(AudioSpec {
                codec: audio_codec,
                rate: 48_000,
                channels: 2,
            }),
            start_offset_ms: 0,
            video_enabled: true,
            picture: Picture::Barcode,
            sound: Sound::Beep,
            rotation: 0,
            top_field_first: None,
            hdr: None,
            timecode: None,
            hdr_meta: None,
        }
    }

    pub fn hdr_meta(mut self, meta: HdrMeta) -> Self {
        self.hdr_meta = Some(meta);
        self
    }

    pub fn timecode(mut self, tc: &str) -> Self {
        self.timecode = Some(tc.to_string());
        self
    }

    pub fn hdr(mut self, hdr: vidcut_core::model::Hdr) -> Self {
        self.hdr = Some(hdr);
        self
    }

    pub fn interlaced(mut self, top_first: bool) -> Self {
        self.top_field_first = Some(top_first);
        self
    }

    pub fn rotation(mut self, degrees: u16) -> Self {
        self.rotation = degrees;
        self
    }

    pub fn coded_size(&self) -> (u32, u32) {
        if self.rotation % 180 == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    pub fn audio_only(mut self) -> Self {
        self.video_enabled = false;
        self
    }

    pub fn picture(mut self, picture: Picture) -> Self {
        self.picture = picture;
        self
    }

    pub fn sound(mut self, sound: Sound) -> Self {
        self.sound = sound;
        self
    }

    pub fn no_audio(mut self) -> Self {
        self.audio = None;
        self
    }

    pub fn with_audio(mut self, codec: AudioCodec, rate: u32, channels: u16) -> Self {
        self.audio = Some(AudioSpec {
            codec,
            rate,
            channels,
        });
        self
    }

    pub fn vfr(mut self) -> Self {
        self.vfr = true;
        self
    }

    pub fn gop(mut self, gop: u32) -> Self {
        self.gop = gop;
        self
    }

    pub fn bframes(mut self, bframes: u32) -> Self {
        self.bframes = bframes;
        self
    }

    pub fn start_offset_ms(mut self, ms: u32) -> Self {
        self.start_offset_ms = ms;
        self
    }

    pub fn file_name(&self) -> String {
        let audio = match self.audio {
            Some(a) => format!(
                "{}{}k{}{}",
                a.codec.tag(),
                a.rate / 1000,
                a.channels,
                match self.sound {
                    Sound::Silence => "sil",
                    Sound::Tone => "tone",
                    Sound::Beep => "",
                }
            ),
            None => "noaudio".into(),
        };
        let content = match (self.video_enabled, self.picture) {
            (false, _) => "novideo".to_string(),
            (true, Picture::Barcode) => "bc".to_string(),
            (true, Picture::Black) => "black".to_string(),
            (true, Picture::Shots { frames }) => format!("shots{frames}-"),
            (true, Picture::Colors) => "colors".to_string(),
            (true, Picture::Noise) => "noise".to_string(),
            (true, Picture::Busy) => "busy".to_string(),
            (true, Picture::Highlight) => "hl".to_string(),
        };
        let rot = match self.rotation {
            0 => String::new(),
            r => format!("r{r}"),
        } + match self.top_field_first {
            None => "",
            Some(true) => "itff",
            Some(false) => "ibff",
        } + match self.hdr {
            None => "",
            Some(vidcut_core::model::Hdr::Pq) => "pq",
            Some(vidcut_core::model::Hdr::Hlg) => "hlg",
        } + &self
            .hdr_meta
            .map(|m| {
                let light = m.light.map_or(String::new(), |(c, f)| format!("l{c}-{f}"));
                let plus = match (m.hdr10plus, m.hdr10plus_frames) {
                    (false, _) => String::new(),
                    (true, None) => "p".into(),
                    (true, Some(n)) => format!("p{n}"),
                };
                format!("m{}{light}{plus}", m.max_nits)
            })
            .unwrap_or_default()
            + &self
                .timecode
                .as_ref()
                .map(|tc| {
                    let digits: String = tc.chars().filter(char::is_ascii_digit).collect();
                    let drop = if tc.contains(';') { "d" } else { "" };
                    format!("tc{digits}{drop}")
                })
                .unwrap_or_default();
        format!(
            "{content}_{codec}_{w}x{h}{rot}_{n}-{d}{vfr}_g{g}b{b}_{f}f_{audio}_o{o}.{ext}",
            codec = self.video.tag(),
            w = self.width,
            h = self.height,
            n = self.fps.0,
            d = self.fps.1,
            vfr = if self.vfr { "vfr" } else { "" },
            g = self.gop,
            b = self.bframes,
            f = self.frames,
            o = self.start_offset_ms,
            ext = self.container.extension(),
        )
    }

    pub fn video_time_base(&self) -> ffmpeg::Rational {
        let (n, d) = (self.fps.0 as i32, self.fps.1 as i32);
        if self.vfr {
            ffmpeg::Rational(d, n * 4)
        } else {
            ffmpeg::Rational(d, n)
        }
    }

    pub fn frame_pts(&self, k: u32) -> i64 {
        if !self.vfr {
            return i64::from(k);
        }
        let k = i64::from(k);
        let cycle: i64 = VFR_PATTERN.iter().sum();
        (k / 4) * cycle + VFR_PATTERN[..(k % 4) as usize].iter().sum::<i64>()
    }

    pub fn frame_time(&self, k: u32) -> f64 {
        let tb = self.video_time_base();
        self.frame_pts(k) as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
    }

    pub fn duration(&self) -> f64 {
        self.frame_time(self.frames)
    }
}

pub fn ensure(spec: &FixtureSpec) -> Result<PathBuf> {
    ensure_in(&crate::testdata::fixtures_dir()?, spec)
}

pub fn ensure_in(dir: &Path, spec: &FixtureSpec) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(spec.file_name());
    if !path.exists() {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!(
            ".{}.{}.{n}.part",
            spec.file_name(),
            std::process::id()
        ));
        let result = generate(spec, &tmp);
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => {}
            Err(_) if path.exists() => {
                let _ = std::fs::remove_file(&tmp);
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageSpec {
    pub width: u32,
    pub height: u32,
    pub barcode: u32,
    pub count: u32,
    pub first: u64,
    pub digits: usize,
    pub ext: &'static str,
}

impl ImageSpec {
    pub fn still(barcode: u32, width: u32, height: u32, ext: &'static str) -> Self {
        Self {
            width,
            height,
            barcode,
            count: 1,
            first: 0,
            digits: 0,
            ext,
        }
    }

    pub fn sequence(first: u64, count: u32, digits: usize, width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            barcode: 0,
            count,
            first,
            digits,
            ext: "png",
        }
    }

    pub fn file_name(&self, k: u32) -> String {
        let n = self.first + u64::from(k);
        format!("img_{n:0width$}.{}", self.ext, width = self.digits)
    }

    fn folder_name(&self) -> String {
        format!(
            "images-{}x{}-b{}-n{}-f{}-d{}-{}",
            self.width, self.height, self.barcode, self.count, self.first, self.digits, self.ext
        )
    }
}

pub fn ensure_images(spec: &ImageSpec) -> Result<Vec<PathBuf>> {
    crate::init();
    let dir = crate::testdata::fixtures_dir()?.join(spec.folder_name());
    let files = |dir: &Path| -> Vec<PathBuf> {
        (0..spec.count)
            .map(|k| dir.join(spec.file_name(k)))
            .collect()
    };
    if dir.exists() {
        return Ok(files(&dir));
    }
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.with_file_name(format!(
        ".{}.{}.{n}.part",
        spec.folder_name(),
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp)?;
    let (w, h) = (spec.width as usize, spec.height as usize);
    let mut luma = vec![0u8; w * h];
    for (k, path) in files(&tmp).iter().enumerate() {
        crate::barcode::paint(spec.barcode + k as u32, &mut luma, w, h, w);
        let bytes = match spec.ext {
            "jpg" => jpeg_gray(spec.width, spec.height, &luma)?,
            _ => {
                let rgba: Vec<u8> = luma.iter().flat_map(|&y| [y, y, y, 255]).collect();
                crate::export::still::png_rgba(spec.width, spec.height, &rgba)?
            }
        };
        std::fs::write(path, bytes)?;
    }
    match std::fs::rename(&tmp, &dir) {
        Ok(()) => {}
        Err(_) if dir.exists() => {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        Err(e) => return Err(e.into()),
    }
    Ok(files(&dir))
}

fn jpeg_gray(width: u32, height: u32, luma: &[u8]) -> Result<Vec<u8>> {
    let mut f = frame::Video::new(format::Pixel::YUVJ420P, width, height);
    let (w, h) = (width as usize, height as usize);
    let stride = f.stride(0);
    for y in 0..h {
        f.data_mut(0)[y * stride..y * stride + w].copy_from_slice(&luma[y * w..(y + 1) * w]);
    }
    for plane in [1, 2] {
        f.data_mut(plane).fill(128);
    }
    f.set_pts(Some(0));
    let codec = encoder::find_by_name("mjpeg")
        .ok_or_else(|| Error::Invalid("this FFmpeg has no JPEG encoder".into()))?;
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .ctx(|| "creating JPEG encoder".into())?;
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(format::Pixel::YUVJ420P);
    ctx.set_time_base(ffmpeg::Rational(1, 25));
    ctx.set_flags(codec::Flags::QSCALE);
    unsafe { (*ctx.as_mut_ptr()).global_quality = 2 * ffmpeg::ffi::FF_QP2LAMBDA };
    let mut enc = ctx.open_as(codec).ctx(|| "opening JPEG encoder".into())?;
    enc.send_frame(&f).ctx(|| "encoding JPEG".into())?;
    enc.send_eof().ctx(|| "encoding JPEG".into())?;
    let mut packet = ffmpeg::Packet::empty();
    enc.receive_packet(&mut packet)
        .ctx(|| "encoding JPEG".into())?;
    Ok(packet.data().unwrap_or_default().to_vec())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spoil {
    Drop,
    Zero,
}

pub fn spoil(src: &Path, out: &Path, index: usize, n: usize, how: Spoil) -> Result<()> {
    crate::init();
    let mut input = format::input(src).ctx(|| format!("opening {}", src.display()))?;
    let mut output = format::output(out).ctx(|| format!("creating {}", out.display()))?;
    for s in input.streams() {
        let mut o = output
            .add_stream(encoder::find(codec::Id::None))
            .ctx(|| "adding stream".into())?;
        o.set_parameters(s.parameters());
        unsafe { (*o.parameters().as_mut_ptr()).codec_tag = 0 };
    }
    output.write_header().ctx(|| "writing header".into())?;
    let mut seen = 0;
    for (s, mut packet) in input.packets() {
        if s.index() == index {
            seen += 1;
            if seen == n + 1 {
                match how {
                    Spoil::Drop => continue,
                    Spoil::Zero => unsafe {
                        if ffmpeg::ffi::av_packet_make_writable(ffmpeg::packet::Mut::as_mut_ptr(
                            &mut packet,
                        )) < 0
                        {
                            return Err(Error::Invalid("packet not writable".into()));
                        }
                        if let Some(data) = packet.data_mut() {
                            data.fill(0);
                        }
                    },
                }
            }
        }
        let tb = output.stream(s.index()).expect("copied").time_base();
        packet.rescale_ts(s.time_base(), tb);
        packet.set_position(-1);
        packet
            .write_interleaved(&mut output)
            .ctx(|| "writing packet".into())?;
    }
    output.write_trailer().ctx(|| "writing trailer".into())?;
    Ok(())
}

pub fn generate(spec: &FixtureSpec, path: &Path) -> Result<()> {
    crate::init();
    if spec.width < 80 || spec.height < 8 {
        return Err(Error::Invalid("fixture must be at least 80x8".into()));
    }
    if !spec.video_enabled && spec.audio.is_none() {
        return Err(Error::Invalid("fixture needs video or audio".into()));
    }
    if !spec.rotation.is_multiple_of(90) || spec.rotation >= 360 {
        return Err(Error::Invalid("rotation must be 0, 90, 180 or 270".into()));
    }
    if spec.rotation != 0 && !matches!(spec.container, Container::Mp4 | Container::Mov) {
        return Err(Error::Invalid("rotation needs MP4 or MOV".into()));
    }
    if spec.top_field_first.is_some()
        && !matches!(spec.video, VideoCodec::H264 | VideoCodec::ProRes422Hq)
    {
        return Err(Error::Invalid(
            "interlaced fixtures are H.264 or ProRes".into(),
        ));
    }
    if spec.hdr.is_some() && !matches!(spec.video, VideoCodec::Hevc10 | VideoCodec::Av1) {
        return Err(Error::Invalid("HDR fixtures are 10-bit HEVC or AV1".into()));
    }
    let mut octx = format::output_as(path, spec.container.muxer())
        .ctx(|| format!("creating {}", path.display()))?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

    let mut video = match spec.video_enabled {
        true => Some(VideoTrack::new(spec, &mut octx, global_header)?),
        false => None,
    };
    let audio_index = usize::from(spec.video_enabled);
    let mut audio = match spec.audio {
        Some(a) => Some(AudioTrack::new(
            spec,
            a,
            &mut octx,
            global_header,
            audio_index,
        )?),
        None => None,
    };
    if let Some(tc) = &spec.timecode {
        let mut tags = ffmpeg::Dictionary::new();
        tags.set("timecode", tc);
        octx.set_metadata(tags);
    }
    octx.write_header().ctx(|| "writing header".into())?;
    if let Some(v) = video.as_mut() {
        v.stream_tb = octx.stream(0).expect("video stream").time_base();
    }
    if let Some(a) = audio.as_mut() {
        a.stream_tb = octx.stream(audio_index).expect("audio stream").time_base();
    }

    let total_samples = spec
        .audio
        .map(|a| (spec.duration() * f64::from(a.rate)).round() as u64);
    for k in 0..spec.frames {
        if let Some(v) = video.as_mut() {
            v.encode_frame(spec, k, &mut octx)?;
        }
        if let (Some(a), Some(total)) = (audio.as_mut(), total_samples) {
            let until = ((spec.frame_time(k + 1)) * f64::from(a.spec.rate)).round() as u64;
            a.encode_until(until.min(total), false, &mut octx)?;
        }
    }
    if let (Some(a), Some(total)) = (audio.as_mut(), total_samples) {
        a.encode_until(total, true, &mut octx)?;
        a.finish(&mut octx)?;
    }
    if let Some(v) = video.as_mut() {
        v.finish(&mut octx)?;
    }
    octx.write_trailer().ctx(|| "writing trailer".into())?;
    Ok(())
}

struct VideoTrack {
    encoder: encoder::Video,
    enc_tb: ffmpeg::Rational,
    stream_tb: ffmpeg::Rational,
    offset: i64,
    luma8: Vec<u8>,
    stored: Vec<u8>,
    durations: std::collections::HashMap<i64, i64>,
    hdr10plus: std::collections::HashMap<i64, Vec<u8>>,
}

impl VideoTrack {
    fn new(
        spec: &FixtureSpec,
        octx: &mut format::context::Output,
        global_header: bool,
    ) -> Result<Self> {
        let name = spec.video.encoder();
        let codec = encoder::find_by_name(name)
            .ok_or_else(|| Error::Invalid(format!("encoder {name} not available")))?;
        let mut ost = octx
            .add_stream(codec)
            .ctx(|| "adding video stream".into())?;
        let mut enc = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .ctx(|| "creating video encoder".into())?;
        let tb = spec.video_time_base();
        let (coded_w, coded_h) = spec.coded_size();
        enc.set_width(coded_w);
        enc.set_height(coded_h);
        enc.set_format(spec.video.pixel());
        enc.set_time_base(tb);
        enc.set_frame_rate(Some(ffmpeg::Rational(spec.fps.0 as i32, spec.fps.1 as i32)));
        enc.set_gop(spec.gop);
        enc.set_max_b_frames(spec.bframes as usize);
        let mut flags = codec::Flags::empty();
        if global_header {
            flags |= codec::Flags::GLOBAL_HEADER;
        }
        if let Some(top) = spec.top_field_first {
            flags |= codec::Flags::INTERLACED_DCT | codec::Flags::INTERLACED_ME;
            unsafe {
                (*enc.as_mut_ptr()).field_order = if top {
                    ffmpeg::ffi::AVFieldOrder::AV_FIELD_TT
                } else {
                    ffmpeg::ffi::AVFieldOrder::AV_FIELD_BB
                };
            }
        }
        enc.set_flags(flags);
        if spec.hdr.is_some() {
            crate::color::tag(unsafe { enc.as_mut_ptr() }, spec.hdr);
        }
        if let Some(meta) = spec.hdr_meta {
            unsafe { crate::hdr::set_on_encoder(enc.as_mut_ptr(), &meta.values())? };
        }
        let mut opts = ffmpeg::Dictionary::new();
        match spec.video {
            VideoCodec::H264 => {
                opts.set("preset", "veryfast");
                let fields = match spec.top_field_first {
                    None => "",
                    Some(true) => ":tff=1",
                    Some(false) => ":bff=1",
                };
                opts.set("x264-params", &format!("scenecut=0{fields}"));
            }
            VideoCodec::Hevc10 => {
                opts.set("preset", "ultrafast");
                if spec.picture == Picture::Busy {
                    opts.set("crf", "14");
                }
                opts.set(
                    "x265-params",
                    &format!(
                        "log-level=error:scenecut=0:open-gop=0:keyint={g}:min-keyint={g}:bframes={b}{r}",
                        g = spec.gop,
                        b = spec.bframes,
                        r = if spec.hdr_meta.is_some() {
                            ":repeat-headers=1"
                        } else {
                            ""
                        },
                    ),
                );
            }
            VideoCodec::ProRes422Hq => opts.set("profile", "3"),
            VideoCodec::ProRes4444 | VideoCodec::ProRes4444Alpha => opts.set("profile", "4"),
            VideoCodec::DnxHrHq => opts.set("profile", "dnxhr_hq"),
            VideoCodec::Vp9 => {
                opts.set("deadline", "realtime");
                opts.set("cpu-used", "8");
                opts.set("lag-in-frames", "0");
                opts.set("crf", "20");
                opts.set("b", "0");
            }
            VideoCodec::Ffv1 => {}
            VideoCodec::Av1 => {
                opts.set("preset", "10");
                opts.set("crf", "18");
                opts.set("svtav1-params", "scd=0:enable-tf=0");
            }
        }
        let encoder = enc
            .open_as_with(codec, opts)
            .ctx(|| format!("opening {name}"))?;
        ost.set_parameters(&encoder);
        ost.set_time_base(tb);
        if spec.timecode.is_some() {
            ost.set_avg_frame_rate(ffmpeg::Rational(spec.fps.0 as i32, spec.fps.1 as i32));
        }
        if spec.rotation != 0 {
            unsafe {
                let par = &mut *(*ost.as_mut_ptr()).codecpar;
                let sd = ffmpeg::ffi::av_packet_side_data_new(
                    &mut par.coded_side_data,
                    &mut par.nb_coded_side_data,
                    ffmpeg::ffi::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
                    9 * 4,
                    0,
                );
                if sd.is_null() {
                    return Err(Error::Invalid("no memory for the display matrix".into()));
                }
                ffmpeg::ffi::av_display_rotation_set(
                    (*sd).data as *mut i32,
                    f64::from(spec.rotation),
                );
            }
        }
        let offset = i64::from(spec.start_offset_ms) * i64::from(tb.denominator())
            / (1000 * i64::from(tb.numerator()));
        Ok(Self {
            encoder,
            enc_tb: tb,
            stream_tb: tb,
            offset,
            luma8: vec![0; (spec.width * spec.height) as usize],
            stored: vec![0; (spec.width * spec.height) as usize],
            durations: std::collections::HashMap::new(),
            hdr10plus: std::collections::HashMap::new(),
        })
    }

    fn encode_frame(
        &mut self,
        spec: &FixtureSpec,
        k: u32,
        octx: &mut format::context::Output,
    ) -> Result<()> {
        let (w, h) = (spec.width as usize, spec.height as usize);
        let pixel = spec.video.pixel();
        let (coded_w, coded_h) = spec.coded_size();
        let mut frame = frame::Video::new(pixel, coded_w, coded_h);
        match spec.picture {
            Picture::Barcode if spec.top_field_first.is_some() => {
                crate::barcode::paint(k, &mut self.luma8, w, h, w);
                let first_even = spec.top_field_first == Some(true);
                let block = w / 8;
                for y in 0..h / 8 {
                    let second = (y % 2 == 0) != first_even;
                    let field = 2 * k as usize + usize::from(second);
                    let x0 = (field * 40) % (w - block);
                    let row = &mut self.luma8[y * w..(y + 1) * w];
                    row.fill(16);
                    row[x0..x0 + block].fill(235);
                }
            }
            Picture::Barcode => crate::barcode::paint(k, &mut self.luma8, w, h, w),
            Picture::Black => self.luma8.fill(16),
            Picture::Highlight => {
                self.luma8.fill(16);
                let (x, y) = Picture::highlight(spec.width, spec.height);
                let i = y as usize * w + x as usize;
                self.luma8[i..i + 2].fill(235);
            }
            Picture::Noise => noise(k, &mut self.luma8),
            Picture::Busy => paint_busy(k, &mut self.luma8, w, h),
            Picture::Colors => {
                for (i, px) in self.luma8.iter_mut().enumerate() {
                    *px = Picture::COLORS[(i % w) * 4 / w].0;
                }
            }
            Picture::Shots { frames } => {
                let level = Picture::shot_level(k / frames.max(1));
                self.luma8.fill(level);
                if level != 16 {
                    let bar = (w / 10).max(1);
                    let x0 = (2 * (k % frames.max(1)) as usize) % (w - bar).max(1);
                    for y in 0..h {
                        self.luma8[y * w + x0..y * w + x0 + bar].fill(level.saturating_add(40));
                    }
                }
            }
        }
        let (cw, ch) = (coded_w as usize, coded_h as usize);
        for sy in 0..ch {
            for sx in 0..cw {
                let (x, y) = match spec.rotation {
                    90 => (w - 1 - sy, sx),
                    180 => (w - 1 - sx, h - 1 - sy),
                    270 => (sy, h - 1 - sx),
                    _ => (sx, sy),
                };
                self.stored[sy * cw + sx] = self.luma8[y * w + x];
            }
        }
        let (w, h) = (cw, ch);
        let ten_bit = matches!(
            pixel,
            format::Pixel::YUV420P10LE
                | format::Pixel::YUV422P10LE
                | format::Pixel::YUV444P10LE
                | format::Pixel::YUVA444P10LE
        );
        if ten_bit {
            let stride = frame.stride(0);
            let plane = frame.data_mut(0);
            let white = match spec.hdr {
                Some(vidcut_core::model::Hdr::Pq) => PQ_WHITE,
                Some(vidcut_core::model::Hdr::Hlg) => HLG_WHITE,
                None => 940,
            };
            for y in 0..h {
                for x in 0..w {
                    let v = match self.stored[y * w + x] {
                        235 => white,
                        v8 => u16::from(v8) << 2,
                    };
                    plane[y * stride + 2 * x..y * stride + 2 * x + 2]
                        .copy_from_slice(&v.to_le_bytes());
                }
            }
            for p in 1..frame.planes() {
                for px in frame.data_mut(p).as_chunks_mut::<2>().0 {
                    *px = 512u16.to_le_bytes();
                }
                if spec.picture == Picture::Colors && p < 3 {
                    paint_chroma(&mut frame, p, spec.rotation, |c| {
                        (u16::from(c) << 2).to_le_bytes()
                    });
                }
            }
        } else {
            let stride = frame.stride(0);
            let plane = frame.data_mut(0);
            for y in 0..h {
                plane[y * stride..y * stride + w].copy_from_slice(&self.stored[y * w..(y + 1) * w]);
            }
            for p in 1..frame.planes() {
                frame.data_mut(p).fill(128);
                if spec.picture == Picture::Colors {
                    paint_chroma(&mut frame, p, spec.rotation, |c| [c]);
                }
            }
        }
        if let Some(top) = spec.top_field_first {
            unsafe {
                let f = &mut *frame.as_mut_ptr();
                f.flags |= ffmpeg::ffi::AV_FRAME_FLAG_INTERLACED;
                if top {
                    f.flags |= ffmpeg::ffi::AV_FRAME_FLAG_TOP_FIELD_FIRST;
                }
            }
        }
        let pts = self.offset + spec.frame_pts(k);
        let duration = spec.frame_pts(k + 1) - spec.frame_pts(k);
        frame.set_pts(Some(pts));
        unsafe { (*frame.as_mut_ptr()).duration = duration };
        self.durations.insert(pts, duration);
        if spec
            .hdr_meta
            .is_some_and(|m| m.hdr10plus && m.hdr10plus_frames.is_none_or(|n| k < n))
        {
            self.hdr10plus.insert(pts, hdr10plus_t35(k));
        }
        self.encoder
            .send_frame(&frame)
            .ctx(|| format!("encoding frame {k}"))?;
        self.drain(octx)
    }

    fn drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut packet = ffmpeg::Packet::empty();
        let id = self.encoder.id();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            if let Some(d) = packet.pts().and_then(|pts| self.durations.remove(&pts)) {
                packet.set_duration(d);
            }
            if let Some(t35) = packet.pts().and_then(|pts| self.hdr10plus.remove(&pts)) {
                crate::hdr::insert_hdr10plus(&mut packet, id, &t35)?;
            }
            packet.set_stream(0);
            packet.rescale_ts(self.enc_tb, self.stream_tb);
            packet
                .write_interleaved(octx)
                .ctx(|| "writing video packet".into())?;
        }
        Ok(())
    }

    fn finish(&mut self, octx: &mut format::context::Output) -> Result<()> {
        self.encoder
            .send_eof()
            .ctx(|| "flushing video encoder".into())?;
        self.drain(octx)
    }
}

struct AudioTrack {
    spec: AudioSpec,
    sound: Sound,
    encoder: encoder::Audio,
    stream_index: usize,
    enc_tb: ffmpeg::Rational,
    stream_tb: ffmpeg::Rational,
    frame_size: usize,
    offset: i64,
    next_sample: u64,
}

impl AudioTrack {
    fn new(
        spec: &FixtureSpec,
        audio: AudioSpec,
        octx: &mut format::context::Output,
        global_header: bool,
        stream_index: usize,
    ) -> Result<Self> {
        let name = audio.codec.encoder();
        let codec = encoder::find_by_name(name)
            .ok_or_else(|| Error::Invalid(format!("encoder {name} not available")))?;
        let mut ost = octx
            .add_stream(codec)
            .ctx(|| "adding audio stream".into())?;
        let mut enc = codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()
            .ctx(|| "creating audio encoder".into())?;
        let tb = ffmpeg::Rational(1, audio.rate as i32);
        enc.set_rate(audio.rate as i32);
        enc.set_channel_layout(ffmpeg::ChannelLayout::default(i32::from(audio.channels)));
        enc.set_format(audio.codec.sample());
        enc.set_time_base(tb);
        if audio.codec == AudioCodec::Aac {
            enc.set_bit_rate(192_000);
        }
        if global_header {
            enc.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        let encoder = enc.open_as(codec).ctx(|| format!("opening {name}"))?;
        ost.set_parameters(&encoder);
        ost.set_time_base(tb);
        let frame_size = match encoder.frame_size() {
            0 => 1024,
            n => n as usize,
        };
        Ok(Self {
            spec: audio,
            sound: spec.sound,
            encoder,
            stream_index,
            enc_tb: tb,
            stream_tb: tb,
            frame_size,
            offset: i64::from(spec.start_offset_ms) * i64::from(audio.rate) / 1000,
            next_sample: 0,
        })
    }

    fn encode_until(
        &mut self,
        until: u64,
        last: bool,
        octx: &mut format::context::Output,
    ) -> Result<()> {
        let channels = usize::from(self.spec.channels);
        let layout = ffmpeg::ChannelLayout::default(i32::from(self.spec.channels));
        while self.next_sample < until {
            let n = (self.frame_size as u64).min(until - self.next_sample) as usize;
            if n < self.frame_size && !last {
                break;
            }
            let samples = match self.sound {
                Sound::Beep => crate::beep::generate(self.spec.rate, channels, self.next_sample, n),
                Sound::Silence => vec![0.0; n * channels],
                Sound::Tone => (0..n)
                    .flat_map(|i| {
                        let t = (self.next_sample + i as u64) as f64 / f64::from(self.spec.rate);
                        let v = (f64::from(TONE_AMPLITUDE)
                            * (2.0 * std::f64::consts::PI * 1000.0 * t).sin())
                            as f32;
                        std::iter::repeat_n(v, channels)
                    })
                    .collect(),
            };
            let fmt = self.spec.codec.sample();
            let mut frame = frame::Audio::new(fmt, n, layout);
            frame.set_rate(self.spec.rate);
            match fmt {
                format::Sample::F32(format::sample::Type::Planar) => {
                    for c in 0..channels {
                        let plane = frame.plane_mut::<f32>(c);
                        for (i, s) in plane.iter_mut().enumerate().take(n) {
                            *s = samples[i * channels + c];
                        }
                    }
                }
                format::Sample::F32(format::sample::Type::Packed) => {
                    let data = frame.data_mut(0);
                    for (i, s) in samples.iter().enumerate() {
                        data[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
                    }
                }
                format::Sample::I16(format::sample::Type::Packed) => {
                    let data = frame.data_mut(0);
                    for (i, s) in samples.iter().enumerate() {
                        let v = (s * 32767.0).round() as i16;
                        data[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
                    }
                }
                other => {
                    return Err(Error::Invalid(format!(
                        "unsupported fixture sample format {other:?}"
                    )));
                }
            }
            frame.set_pts(Some(self.offset + self.next_sample as i64));
            self.encoder
                .send_frame(&frame)
                .ctx(|| "encoding audio".into())?;
            self.drain(octx)?;
            self.next_sample += n as u64;
        }
        Ok(())
    }

    fn drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut packet = ffmpeg::Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.stream_index);
            packet.rescale_ts(self.enc_tb, self.stream_tb);
            packet
                .write_interleaved(octx)
                .ctx(|| "writing audio packet".into())?;
        }
        Ok(())
    }

    fn finish(&mut self, octx: &mut format::context::Output) -> Result<()> {
        self.encoder
            .send_eof()
            .ctx(|| "flushing audio encoder".into())?;
        self.drain(octx)
    }
}

fn noise(k: u32, luma: &mut [u8]) {
    let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ u64::from(k).wrapping_mul(0x2545_F491);
    for px in luma.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *px = 16 + (x % 220) as u8;
    }
}

fn paint_busy(k: u32, luma: &mut [u8], width: usize, height: usize) {
    crate::barcode::paint(k, luma, width, height, width);
    let mut texture = vec![0; 2 * width];
    let mut grain = vec![0; width];
    for (from, to) in [
        (0, height / 8),
        (height * 3 / 8, height * 5 / 8),
        (height * 7 / 8, height),
    ] {
        for y in from..to {
            noise((y as u32) << 12, &mut texture);
            noise(k ^ (y as u32) << 12, &mut grain);
            let pan = 8 * k as usize;
            for (x, px) in luma[y * width..(y + 1) * width].iter_mut().enumerate() {
                let t = texture[(x + pan) / 4 % texture.len()];
                *px = t.saturating_add(grain[x] % 16).min(235);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_busy_picture_reads_as_its_frame() {
        let mut luma = vec![0; 160 * 90];
        super::paint_busy(7, &mut luma, 160, 90);
        assert_eq!(crate::barcode::read(&luma, 160, 90, 160), Some(7));
    }
}
