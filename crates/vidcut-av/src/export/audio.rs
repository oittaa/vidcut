//! The audio of a render plan: sample-exact source reads, de-click fades at
//! cuts, and the mix of all lanes, as interleaved `f32`.

use std::path::Path;

use vidcut_core::render::AudioPiece;

use crate::error::{Context, Error, Result};
use crate::ffmpeg::{self, codec, ffi, format, frame, media};

/// Converts decoded audio of any format, layout and rate to interleaved `f32`
/// with `channels` channels at `rate` Hz (libswresample).
pub(crate) struct Resampler {
    swr: *mut ffi::SwrContext,
    channels: usize,
    input: (i32, i32, i32), // format, rate, channels the context was built for
}

// SAFETY: the context is only used through `&mut self`.
unsafe impl Send for Resampler {}

impl Drop for Resampler {
    fn drop(&mut self) {
        unsafe { ffi::swr_free(&mut self.swr) };
    }
}

impl Resampler {
    pub(crate) fn new(src: &frame::Audio, rate: u32, channels: u16) -> Result<Self> {
        unsafe {
            let f = &*src.as_ptr();
            let mut in_layout: ffi::AVChannelLayout = std::mem::zeroed();
            if f.ch_layout.order == ffi::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC
                || f.ch_layout.nb_channels == 0
            {
                ffi::av_channel_layout_default(&mut in_layout, f.ch_layout.nb_channels.max(1));
            } else {
                ffi::av_channel_layout_copy(&mut in_layout, &f.ch_layout);
            }
            let mut out_layout: ffi::AVChannelLayout = std::mem::zeroed();
            ffi::av_channel_layout_default(&mut out_layout, i32::from(channels));
            let mut swr = std::ptr::null_mut();
            let in_fmt: ffi::AVSampleFormat = std::mem::transmute(f.format);
            let ret = ffi::swr_alloc_set_opts2(
                &mut swr,
                &out_layout,
                ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                rate as i32,
                &in_layout,
                in_fmt,
                f.sample_rate,
                0,
                std::ptr::null_mut(),
            );
            ffi::av_channel_layout_uninit(&mut in_layout);
            ffi::av_channel_layout_uninit(&mut out_layout);
            if ret < 0 || ffi::swr_init(swr) < 0 {
                ffi::swr_free(&mut swr);
                return Err(Error::Invalid(format!(
                    "cannot convert audio ({} Hz, {} channels) to {rate} Hz",
                    f.sample_rate, f.ch_layout.nb_channels
                )));
            }
            Ok(Self {
                swr,
                channels: usize::from(channels),
                input: (f.format, f.sample_rate, f.ch_layout.nb_channels),
            })
        }
    }

    pub(crate) fn matches(&self, src: &frame::Audio) -> bool {
        let f = unsafe { &*src.as_ptr() };
        self.input == (f.format, f.sample_rate, f.ch_layout.nb_channels)
    }

    /// Appends the converted samples of `src` (or the resampler's remaining
    /// buffered samples when `src` is `None`) to `out`.
    pub(crate) fn convert(&mut self, src: Option<&frame::Audio>, out: &mut Vec<f32>) -> Result<()> {
        unsafe {
            let (inp, in_count) = match src {
                Some(f) => (
                    (*f.as_ptr()).extended_data as *const *const u8,
                    (*f.as_ptr()).nb_samples,
                ),
                None => (std::ptr::null(), 0),
            };
            let room = ffi::swr_get_out_samples(self.swr, in_count).max(0) as usize + 32;
            let start = out.len();
            out.resize(start + room * self.channels, 0.0);
            let dst = out[start..].as_mut_ptr() as *mut u8;
            let n = ffi::swr_convert(self.swr, &dst, room as i32, inp, in_count);
            if n < 0 {
                out.truncate(start);
                return Err(Error::Ffmpeg {
                    context: "converting audio".into(),
                    source: ffmpeg::Error::from(n),
                });
            }
            out.truncate(start + n as usize * self.channels);
        }
        Ok(())
    }
}

/// Reads `samples` samples of one audio stream starting at a raw time.
struct SourceAudio {
    input: format::context::Input,
    stream: usize,
    tick: f64,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<Resampler>,
    rate: u32,
    channels: u16,
    /// Converted samples not yet consumed, from `cursor` on.
    pending: Vec<f32>,
    cursor: usize,
    /// Samples still to drop at the start (before the target time).
    skip: usize,
    eof: bool,
}

impl SourceAudio {
    fn open(path: &Path, ordinal: u32, start: f64, rate: u32, channels: u16) -> Result<Self> {
        crate::init();
        let input = format::input(path).ctx(|| format!("opening {}", path.display()))?;
        let stream = input
            .streams()
            .filter(|s| s.parameters().medium() == media::Type::Audio)
            .nth(ordinal as usize)
            .ok_or_else(|| {
                Error::Invalid(format!("{}: no audio stream {ordinal}", path.display()))
            })?;
        let index = stream.index();
        let tb = stream.time_base();
        let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
        discard_others(&input, index);
        let decoder = codec::context::Context::from_parameters(stream.parameters())
            .and_then(|mut c| {
                // Without the packet time base, FFmpeg can't shift a frame's
                // timestamp past skipped priming samples (Opus pre-skip, AAC
                // priming), and audio would start that much late.
                unsafe { (*c.as_mut_ptr()).pkt_timebase = tb.into() };
                c.decoder().audio()
            })
            .ctx(|| "opening audio decoder".into())?;
        let mut s = Self {
            input,
            stream: index,
            tick,
            decoder,
            resampler: None,
            rate,
            channels,
            pending: Vec::new(),
            cursor: 0,
            skip: 0,
            eof: false,
        };
        s.seek(start)?;
        Ok(s)
    }

    /// Positions reading at raw time `start` (seconds), sample-exactly.
    fn seek(&mut self, start: f64) -> Result<()> {
        let mut back = 0.25;
        loop {
            let ts = ((start - back) / self.tick).floor() as i64;
            let at_start = ts <= 0 || back > 60.0;
            let ret = unsafe {
                ffi::avformat_seek_file(
                    self.input.as_mut_ptr(),
                    self.stream as i32,
                    i64::MIN,
                    ts,
                    ts,
                    0,
                )
            };
            if ret < 0 || at_start {
                unsafe {
                    ffi::avformat_seek_file(
                        self.input.as_mut_ptr(),
                        -1,
                        i64::MIN,
                        i64::MIN,
                        i64::MIN,
                        0,
                    )
                };
            }
            self.decoder.flush();
            self.pending.clear();
            self.cursor = 0;
            self.eof = false;
            self.resampler = None;
            let Some(first) = self.decode()? else {
                // Nothing at all after the seek: silence.
                self.eof = true;
                return Ok(());
            };
            let t0 = first.timestamp().unwrap_or(0) as f64 * self.tick;
            if t0 > start + 0.5 / f64::from(self.rate) && !(ret < 0 || at_start) {
                back *= 2.0;
                continue;
            }
            self.skip = ((start - t0) * f64::from(self.rate)).round().max(0.0) as usize;
            self.push(&first)?;
            return Ok(());
        }
    }

    fn decode(&mut self) -> Result<Option<frame::Audio>> {
        loop {
            let mut f = frame::Audio::empty();
            match self.decoder.receive_frame(&mut f) {
                Ok(()) => return Ok(Some(f)),
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::util::error::EAGAIN,
                }) => {}
                Err(e) => {
                    return Err(Error::Ffmpeg {
                        context: "decoding audio".into(),
                        source: e,
                    });
                }
            }
            loop {
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut self.input) {
                    Ok(()) if packet.stream() == self.stream => {
                        // Corrupt packets are skipped rather than failing the export.
                        let _ = self.decoder.send_packet(&packet);
                        break;
                    }
                    Ok(()) => {}
                    Err(ffmpeg::Error::Eof) => {
                        let _ = self.decoder.send_eof();
                        break;
                    }
                    Err(e) => {
                        return Err(Error::Ffmpeg {
                            context: "reading audio".into(),
                            source: e,
                        });
                    }
                }
            }
        }
    }

    fn push(&mut self, f: &frame::Audio) -> Result<()> {
        if self.resampler.as_ref().is_none_or(|r| !r.matches(f)) {
            if let Some(mut old) = self.resampler.take() {
                old.convert(None, &mut self.pending)?;
            }
            self.resampler = Some(Resampler::new(f, self.rate, self.channels)?);
        }
        self.resampler
            .as_mut()
            .expect("set")
            .convert(Some(f), &mut self.pending)
    }

    /// Appends exactly `n` samples (interleaved) to `out`, padding with
    /// silence past the end of the stream.
    fn read(&mut self, n: usize, out: &mut Vec<f32>) -> Result<()> {
        let ch = usize::from(self.channels);
        loop {
            // Drop the samples before the start time.
            let available = (self.pending.len() - self.cursor) / ch;
            let drop = self.skip.min(available);
            self.cursor += drop * ch;
            self.skip -= drop;
            let available = (self.pending.len() - self.cursor) / ch;
            if self.skip == 0 && available >= n {
                out.extend_from_slice(&self.pending[self.cursor..self.cursor + n * ch]);
                self.cursor += n * ch;
                // Compact now and then.
                if self.cursor > 1 << 16 {
                    self.pending.drain(..self.cursor);
                    self.cursor = 0;
                }
                return Ok(());
            }
            if self.eof {
                let take = if self.skip == 0 { available.min(n) } else { 0 };
                out.extend_from_slice(&self.pending[self.cursor..self.cursor + take * ch]);
                self.cursor += take * ch;
                out.resize(out.len() + (n - take) * ch, 0.0);
                return Ok(());
            }
            match self.decode()? {
                Some(f) => self.push(&f)?,
                None => {
                    if let Some(r) = self.resampler.as_mut() {
                        r.convert(None, &mut self.pending)?;
                    }
                    self.eof = true;
                }
            }
        }
    }
}

/// Tells the demuxer to skip every stream but `keep`: less I/O, and Matroska
/// otherwise complains about the other tracks' non-keyframes after a seek.
pub(crate) fn discard_others(input: &format::context::Input, keep: usize) {
    unsafe {
        let ctx = &*input.as_ptr();
        for i in 0..ctx.nb_streams as usize {
            if i != keep {
                (**ctx.streams.add(i)).discard = ffi::AVDiscard::AVDISCARD_ALL;
            }
        }
    }
}

/// One audio lane of a plan, read sequentially.
struct Lane {
    pieces: Vec<AudioPiece>,
    piece: usize,
    /// Samples of the current piece already produced.
    pos: i64,
    source: Option<SourceAudio>,
    declick: i64,
    scratch: Vec<f32>,
}

impl Lane {
    /// Adds the lane's next `n` samples into `out` (interleaved, `n * ch` long).
    fn mix_into(&mut self, out: &mut [f32], rate: u32, channels: u16) -> Result<()> {
        let ch = usize::from(channels);
        let mut done = 0usize;
        let n = out.len() / ch;
        while done < n {
            let Some(piece) = self.pieces.get(self.piece).cloned() else {
                return Ok(()); // past the end: silence
            };
            let len = piece.samples();
            let take = ((len - self.pos) as usize).min(n - done);
            if let AudioPiece::Source {
                path,
                stream,
                origin,
                source_sample,
                gain_db,
                ..
            } = &piece
            {
                if self.source.is_none() {
                    let start = origin + *source_sample as f64 / f64::from(rate);
                    self.source = Some(SourceAudio::open(path, *stream, start, rate, channels)?);
                }
                self.scratch.clear();
                self.source
                    .as_mut()
                    .expect("open")
                    .read(take, &mut self.scratch)?;
                let gain = 10f32.powf(*gain_db as f32 / 20.0);
                // De-click fades at cuts inside the lane (not at the plan's ends).
                let fade = self.declick.min(len / 2).max(1);
                let fade_in = self.piece > 0;
                let fade_out = self.piece + 1 < self.pieces.len();
                for i in 0..take {
                    let p = self.pos + i as i64;
                    let mut g = gain;
                    if fade_in && p < fade {
                        g *= (p + 1) as f32 / fade as f32;
                    }
                    if fade_out && p >= len - fade {
                        g *= (len - p) as f32 / fade as f32;
                    }
                    for c in 0..ch {
                        out[(done + i) * ch + c] += self.scratch[i * ch + c] * g;
                    }
                }
            }
            done += take;
            self.pos += take as i64;
            if self.pos >= len {
                self.piece += 1;
                self.pos = 0;
                self.source = None;
            }
        }
        Ok(())
    }
}

/// Mixes all lanes of a plan.
pub struct AudioMix {
    lanes: Vec<Lane>,
    rate: u32,
    channels: u16,
}

impl AudioMix {
    /// `declick_ms` > 0 fades `declick_ms` in and out at every cut.
    pub fn new(lanes: &[Vec<AudioPiece>], rate: u32, channels: u16, declick_ms: u32) -> Self {
        let declick = i64::from(rate) * i64::from(declick_ms) / 1000;
        Self {
            lanes: lanes
                .iter()
                .map(|l| Lane {
                    pieces: l.clone(),
                    piece: 0,
                    pos: 0,
                    source: None,
                    declick,
                    scratch: Vec::new(),
                })
                .collect(),
            rate,
            channels,
        }
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// The next `n` mixed samples, interleaved.
    pub fn read(&mut self, n: usize) -> Result<Vec<f32>> {
        let mut out = vec![0.0f32; n * usize::from(self.channels)];
        for lane in &mut self.lanes {
            lane.mix_into(&mut out, self.rate, self.channels)?;
        }
        Ok(out)
    }
}
