//! Synthetic test fixtures: barcode video (frame index in luma, see
//! [`barcode`](crate::barcode)) plus an optional [`beep`](crate::beep) audio track,
//! encoded with real encoders into real containers.
//!
//! Fixtures are cached in [`testdata::fixtures_dir`](crate::testdata::fixtures_dir)
//! under a descriptive file name that encodes the whole spec, so tests can share
//! them across runs. Bump [`GENERATOR_VERSION`] whenever generated output changes.

use std::path::{Path, PathBuf};

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, encoder, format, frame};

/// Part of every fixture file name; bump to invalidate cached fixtures.
pub const GENERATOR_VERSION: u32 = 5;

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
    /// libx264, 8-bit 4:2:0.
    H264,
    /// libx265, 10-bit 4:2:0.
    Hevc10,
    /// prores_ks HQ, 10-bit 4:2:2, intra-only.
    ProRes422Hq,
    /// libvpx-vp9, 8-bit 4:2:0.
    Vp9,
    /// FFV1, lossless 8-bit 4:2:0, intra-only.
    Ffv1,
}

impl VideoCodec {
    fn tag(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc10 => "hevc10",
            Self::ProRes422Hq => "prores",
            Self::Vp9 => "vp9",
            Self::Ffv1 => "ffv1",
        }
    }

    fn encoder(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::Hevc10 => "libx265",
            Self::ProRes422Hq => "prores_ks",
            Self::Vp9 => "libvpx-vp9",
            Self::Ffv1 => "ffv1",
        }
    }

    fn pixel(self) -> format::Pixel {
        match self {
            Self::H264 | Self::Vp9 | Self::Ffv1 => format::Pixel::YUV420P,
            Self::Hevc10 => format::Pixel::YUV420P10LE,
            Self::ProRes422Hq => format::Pixel::YUV422P10LE,
        }
    }

    fn intra_only(self) -> bool {
        matches!(self, Self::ProRes422Hq | Self::Ffv1)
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

/// What the video frames show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Picture {
    /// The frame index as a [`barcode`](crate::barcode).
    Barcode,
    /// Video-range black (filler for gaps).
    Black,
    /// A new shot every `frames` frames (for scene detection): a flat level
    /// per shot with a slowly moving bar; every fourth shot is black.
    Shots { frames: u32 },
}

impl Picture {
    /// Luma of shot `s` of [`Picture::Shots`]: 16 (black) for every fourth.
    pub fn shot_level(s: u32) -> u8 {
        [70, 190, 120, 16][(s % 4) as usize]
    }
}

/// What the audio track carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// A [`beep`](crate::beep) at the start of every second.
    Beep,
    /// Digital silence (filler for gaps and no-audio clips).
    Silence,
    /// A steady 1 kHz sine at -20 dBFS in every channel: -20 LUFS in
    /// stereo (ITU-R BS.1770).
    Tone,
}

/// Peak amplitude of [`Sound::Tone`].
pub const TONE_AMPLITUDE: f32 = 0.1;

/// Everything that determines a fixture file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FixtureSpec {
    pub container: Container,
    pub video: VideoCodec,
    pub width: u32,
    pub height: u32,
    /// Nominal frame rate as (numerator, denominator), e.g. (30000, 1001).
    pub fps: (u32, u32),
    pub frames: u32,
    /// Keyframe interval in frames (ignored for intra-only codecs).
    pub gop: u32,
    /// Maximum consecutive B-frames (H.264/HEVC only).
    pub bframes: u32,
    /// Variable frame rate: frame durations cycle through ½, 1½, 1, 1 nominal frames.
    pub vfr: bool,
    pub audio: Option<AudioSpec>,
    /// Added to every timestamp, e.g. to emulate MPEG-TS start offsets.
    pub start_offset_ms: u32,
    /// Write a video stream; `false` makes an audio-only file of the same duration.
    pub video_enabled: bool,
    pub picture: Picture,
    pub sound: Sound,
}

/// VFR frame durations in quarter-frame ticks (average 4 = one nominal frame).
const VFR_PATTERN: [i64; 4] = [2, 6, 4, 4];

impl FixtureSpec {
    /// A spec with sensible defaults: GOP of about one second, two B-frames for
    /// inter codecs, 48 kHz stereo AAC (PCM for MOV, Opus for WebM), CFR.
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
        }
    }

    /// Audio-only file (the video settings still define the duration).
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

    /// Descriptive, unique file name for this spec.
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
        };
        format!(
            "{content}{v}_{codec}_{w}x{h}_{n}-{d}{vfr}_g{g}b{b}_{f}f_{audio}_o{o}.{ext}",
            v = GENERATOR_VERSION,
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

    /// Time base of the video timestamps this generator writes.
    pub fn video_time_base(&self) -> ffmpeg::Rational {
        let (n, d) = (self.fps.0 as i32, self.fps.1 as i32);
        if self.vfr {
            ffmpeg::Rational(d, n * 4)
        } else {
            ffmpeg::Rational(d, n)
        }
    }

    /// Presentation timestamp of frame `k` in [`video_time_base`](Self::video_time_base),
    /// excluding the start offset.
    pub fn frame_pts(&self, k: u32) -> i64 {
        if !self.vfr {
            return i64::from(k);
        }
        let k = i64::from(k);
        let cycle: i64 = VFR_PATTERN.iter().sum();
        (k / 4) * cycle + VFR_PATTERN[..(k % 4) as usize].iter().sum::<i64>()
    }

    /// Presentation time of frame `k` in seconds, excluding the start offset.
    pub fn frame_time(&self, k: u32) -> f64 {
        let tb = self.video_time_base();
        self.frame_pts(k) as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
    }

    /// Total duration in seconds (end of the last frame).
    pub fn duration(&self) -> f64 {
        self.frame_time(self.frames)
    }
}

/// Returns the path of the fixture for `spec`, generating it first if needed.
pub fn ensure(spec: &FixtureSpec) -> Result<PathBuf> {
    ensure_in(&crate::testdata::fixtures_dir()?, spec)
}

/// Like [`ensure`], caching in `dir`.
pub fn ensure_in(dir: &Path, spec: &FixtureSpec) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(spec.file_name());
    if !path.exists() {
        // Generate under a temporary name so an interrupted run never leaves a
        // truncated fixture that later runs would trust.
        // Unique per thread: tests generate fixtures concurrently within one
        // process, and two writers sharing a temp file corrupt it.
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
        std::fs::rename(&tmp, &path)?;
    }
    Ok(path)
}

/// Writes the fixture for `spec` to `path`.
pub fn generate(spec: &FixtureSpec, path: &Path) -> Result<()> {
    crate::init();
    if spec.width < 80 || spec.height < 8 {
        return Err(Error::Invalid("fixture must be at least 80x8".into()));
    }
    if !spec.video_enabled && spec.audio.is_none() {
        return Err(Error::Invalid("fixture needs video or audio".into()));
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
            // Keep audio roughly level with video for sane interleaving.
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
    /// Duration of each submitted frame by pts. Delayed encoders (x264 with
    /// B-frames) don't reliably carry frame durations onto packets; without them
    /// the MP4 muxer gives the last sample duration 0 and its edit list drops it.
    durations: std::collections::HashMap<i64, i64>,
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
        enc.set_width(spec.width);
        enc.set_height(spec.height);
        enc.set_format(spec.video.pixel());
        enc.set_time_base(tb);
        enc.set_frame_rate(Some(ffmpeg::Rational(spec.fps.0 as i32, spec.fps.1 as i32)));
        enc.set_gop(spec.gop);
        enc.set_max_b_frames(spec.bframes as usize);
        if global_header {
            enc.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        let mut opts = ffmpeg::Dictionary::new();
        match spec.video {
            VideoCodec::H264 => {
                opts.set("preset", "veryfast");
                opts.set("x264-params", "scenecut=0");
            }
            VideoCodec::Hevc10 => {
                opts.set("preset", "ultrafast");
                opts.set(
                    "x265-params",
                    &format!(
                        "log-level=error:scenecut=0:open-gop=0:keyint={g}:min-keyint={g}:bframes={b}",
                        g = spec.gop,
                        b = spec.bframes
                    ),
                );
            }
            VideoCodec::ProRes422Hq => opts.set("profile", "3"),
            VideoCodec::Vp9 => {
                opts.set("deadline", "realtime");
                opts.set("cpu-used", "8");
                opts.set("lag-in-frames", "0");
                opts.set("crf", "20");
                opts.set("b", "0");
            }
            VideoCodec::Ffv1 => {}
        }
        let encoder = enc
            .open_as_with(codec, opts)
            .ctx(|| format!("opening {name}"))?;
        ost.set_parameters(&encoder);
        ost.set_time_base(tb);
        let offset = i64::from(spec.start_offset_ms) * i64::from(tb.denominator())
            / (1000 * i64::from(tb.numerator()));
        Ok(Self {
            encoder,
            enc_tb: tb,
            stream_tb: tb,
            offset,
            luma8: vec![0; (spec.width * spec.height) as usize],
            durations: std::collections::HashMap::new(),
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
        let mut frame = frame::Video::new(pixel, spec.width, spec.height);
        match spec.picture {
            Picture::Barcode => crate::barcode::paint(k, &mut self.luma8, w, h, w),
            Picture::Black => self.luma8.fill(16),
            Picture::Shots { frames } => {
                let level = Picture::shot_level(k / frames.max(1));
                self.luma8.fill(level);
                if level != 16 {
                    // A bar a tenth of the width moving 2 px a frame.
                    let bar = (w / 10).max(1);
                    let x0 = (2 * (k % frames.max(1)) as usize) % (w - bar).max(1);
                    for y in 0..h {
                        self.luma8[y * w + x0..y * w + x0 + bar].fill(level.saturating_add(40));
                    }
                }
            }
        }
        let ten_bit = matches!(
            pixel,
            format::Pixel::YUV420P10LE | format::Pixel::YUV422P10LE
        );
        if ten_bit {
            let stride = frame.stride(0);
            let plane = frame.data_mut(0);
            for y in 0..h {
                for x in 0..w {
                    let v = u16::from(self.luma8[y * w + x]) << 2;
                    plane[y * stride + 2 * x..y * stride + 2 * x + 2]
                        .copy_from_slice(&v.to_le_bytes());
                }
            }
            for p in 1..frame.planes() {
                for px in frame.data_mut(p).as_chunks_mut::<2>().0 {
                    *px = 512u16.to_le_bytes();
                }
            }
        } else {
            let stride = frame.stride(0);
            let plane = frame.data_mut(0);
            for y in 0..h {
                plane[y * stride..y * stride + w].copy_from_slice(&self.luma8[y * w..(y + 1) * w]);
            }
            for p in 1..frame.planes() {
                frame.data_mut(p).fill(128);
            }
        }
        let pts = self.offset + spec.frame_pts(k);
        let duration = spec.frame_pts(k + 1) - spec.frame_pts(k);
        frame.set_pts(Some(pts));
        unsafe { (*frame.as_mut_ptr()).duration = duration };
        self.durations.insert(pts, duration);
        self.encoder
            .send_frame(&frame)
            .ctx(|| format!("encoding frame {k}"))?;
        self.drain(octx)
    }

    fn drain(&mut self, octx: &mut format::context::Output) -> Result<()> {
        let mut packet = ffmpeg::Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            if let Some(d) = packet.pts().and_then(|pts| self.durations.remove(&pts)) {
                packet.set_duration(d);
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

    /// Encodes beep audio up to (not including) absolute sample `until`. Only
    /// whole encoder frames are written unless `last` is set, because fixed
    /// frame-size codecs (AAC, Opus) accept a short frame only at the end.
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
