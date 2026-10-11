//! The audio of a render plan: sample-exact source reads, de-click fades at
//! cuts, and the mix of all lanes, as interleaved `f32`.

use std::path::{Path, PathBuf};

use vidcut_core::render::AudioPiece;

use super::ahead::Ahead;
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
            // Bindgen's `swr_convert` takes `*mut *mut u8` and `*mut *const u8`
            // on current FFmpeg headers (`*const` for the input pointer on
            // older ones). A mut outer pointer still coerces to const.
            let mut dst = out[start..].as_mut_ptr() as *mut u8;
            let n = ffi::swr_convert(self.swr, &mut dst, room as i32, inp.cast_mut(), in_count);
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

/// Reads one audio stream of a file from a raw time on, for one piece or
/// several in a row.
struct SourceAudio {
    path: PathBuf,
    /// Which of the file's audio streams.
    ordinal: u32,
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
    /// Raw time (s) of the first sample decoded after the latest seek, and
    /// the index (in output samples from it) of the next sample `read` gives.
    t0: f64,
    at: i64,
    /// Raw time (s) where the next decoded frame should start. One that
    /// starts later follows a gap (a damaged packet, or one the file lacks):
    /// it waits in `held` while `gap` samples of silence go first, so what
    /// follows stays where its timestamps put it.
    next_in: Option<f64>,
    gap: usize,
    held: Option<frame::Audio>,
    /// Raw time (s) where the file ends: a frame stamped later has a broken
    /// timestamp, and plays in order rather than after a gap.
    end: f64,
    eof: bool,
}

/// How far ahead (seconds) a later piece of the same stream may start for
/// decoding on to it to beat a seek.
const DECODE_AHEAD: i64 = 1;

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
        let end = unsafe {
            let ctx = &*input.as_ptr();
            match (ctx.start_time, ctx.duration) {
                (_, d) if d == ffi::AV_NOPTS_VALUE || d <= 0 => f64::INFINITY,
                (s, d) if s == ffi::AV_NOPTS_VALUE => d as f64 / f64::from(ffi::AV_TIME_BASE),
                (s, d) => (s + d) as f64 / f64::from(ffi::AV_TIME_BASE),
            }
        };
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
            path: path.to_path_buf(),
            ordinal,
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
            t0: start,
            at: 0,
            next_in: None,
            gap: 0,
            held: None,
            end,
            eof: false,
        };
        s.seek(start)?;
        Ok(s)
    }

    /// Readies the stream for another piece starting at raw time `start`: a
    /// piece that starts a little ahead decodes on to it, any other seeks.
    fn reposition(&mut self, start: f64) -> Result<()> {
        let want = ((start - self.t0) * f64::from(self.rate)).round() as i64;
        let ahead = want - self.at;
        if (0..=DECODE_AHEAD * i64::from(self.rate)).contains(&ahead) {
            self.skip += ahead as usize;
            self.at = want;
            Ok(())
        } else {
            self.seek(start)
        }
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
            self.skip = 0;
            self.eof = false;
            self.resampler = None;
            (self.next_in, self.gap, self.held) = (None, 0, None);
            let Some(first) = self.decode()? else {
                // Nothing at all after the seek: silence.
                self.eof = true;
                (self.t0, self.at) = (start, 0);
                return Ok(());
            };
            let t0 = first.timestamp().unwrap_or(0) as f64 * self.tick;
            if t0 > start + 0.5 / f64::from(self.rate) && !(ret < 0 || at_start) {
                back *= 2.0;
                continue;
            }
            if t0 - start > crate::decode::AUDIO_GAP {
                // The stream starts later (audio that begins after the
                // picture, or a first packet lost): silence until it does.
                (self.t0, self.at) = (start, 0);
                self.next_in = Some(start);
            } else {
                self.skip = ((start - t0) * f64::from(self.rate)).round().max(0.0) as usize;
                (self.t0, self.at) = (t0, self.skip as i64);
            }
            self.push(first)?;
            return Ok(());
        }
    }

    fn decode(&mut self) -> Result<Option<frame::Audio>> {
        let mut damaged = 0;
        loop {
            let mut f = frame::Audio::empty();
            match self.decoder.receive_frame(&mut f) {
                Ok(()) => return Ok(Some(f)),
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::util::error::EAGAIN,
                }) => {}
                // A damaged frame: on to the next.
                Err(ffmpeg::Error::InvalidData) if damaged < crate::decode::DAMAGED_IN_A_ROW => {
                    damaged += 1;
                    continue;
                }
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
                        crate::decode::send_packet(&mut self.decoder, &packet)?;
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

    /// Takes a decoded frame: converts it, or holds it back behind a gap.
    fn push(&mut self, f: frame::Audio) -> Result<()> {
        let start = f.timestamp().map(|t| t as f64 * self.tick);
        let end = start.map(|t| t + f.samples() as f64 / f64::from(f.rate().max(1)));
        if let (Some(t), Some(next)) = (start, std::mem::replace(&mut self.next_in, end))
            && t - next > crate::decode::AUDIO_GAP
            && t <= self.end
        {
            // The samples before the gap first, all of them.
            if let Some(mut r) = self.resampler.take() {
                r.convert(None, &mut self.pending)?;
            }
            self.gap = ((t - next) * f64::from(self.rate)).round() as usize;
            self.held = Some(f);
            return Ok(());
        }
        self.convert(&f)
    }

    fn convert(&mut self, f: &frame::Audio) -> Result<()> {
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
        self.at += n as i64;
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
            if self.gap > 0 {
                // Silence for a gap, a second at a time however long it is.
                let fill = self.gap.min(self.rate as usize);
                self.pending.resize(self.pending.len() + fill * ch, 0.0);
                self.gap -= fill;
                continue;
            }
            if let Some(f) = self.held.take() {
                self.convert(&f)?;
                continue;
            }
            if self.eof {
                let take = if self.skip == 0 { available.min(n) } else { 0 };
                out.extend_from_slice(&self.pending[self.cursor..self.cursor + take * ch]);
                self.cursor += take * ch;
                out.resize(out.len() + (n - take) * ch, 0.0);
                return Ok(());
            }
            match self.decode()? {
                Some(f) => self.push(f)?,
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

/// One audio lane of a plan, read sequentially. A piece from the same stream
/// as the one before it reads on with the same decoder; a piece from another
/// stream is opened while the piece before it plays.
struct Lane {
    pieces: Vec<AudioPiece>,
    piece: usize,
    /// Samples of the current piece already produced.
    pos: i64,
    source: Option<SourceAudio>,
    ahead: Ahead<SourceAudio>,
    declick: i64,
    scratch: Vec<f32>,
}

impl Lane {
    /// Readies the current piece's stream: the open one if it is the same
    /// stream, the one opened ahead for this piece, or a new one. Then starts
    /// opening the next stream.
    fn start_piece(
        &mut self,
        path: &Path,
        stream: u32,
        start: f64,
        rate: u32,
        channels: u16,
    ) -> Result<()> {
        let source = match self.source.take() {
            Some(mut s) if s.path == path && s.ordinal == stream => {
                s.reposition(start)?;
                s
            }
            _ => match self.ahead.take(self.piece) {
                Some(opened) => opened?,
                None => SourceAudio::open(path, stream, start, rate, channels)?,
            },
        };
        self.source = Some(source);
        self.open_next(path, stream, rate, channels);
        Ok(())
    }

    /// Starts opening the stream of the next source piece, unless it is the
    /// current one, which that piece then reads on with.
    fn open_next(&mut self, current: &Path, current_stream: u32, rate: u32, channels: u16) {
        let next = self
            .pieces
            .iter()
            .enumerate()
            .skip(self.piece + 1)
            .find_map(|(i, p)| match p {
                AudioPiece::Source {
                    path,
                    stream,
                    origin,
                    source_sample,
                    samples,
                    ..
                } if *samples > 0 => Some((i, path.clone(), *stream, *origin, *source_sample)),
                _ => None,
            });
        let Some((i, path, stream, origin, source_sample)) = next else {
            return;
        };
        if path == current && stream == current_stream {
            return;
        }
        let start = origin + source_sample as f64 / f64::from(rate);
        self.ahead.start(i, move || {
            SourceAudio::open(&path, stream, start, rate, channels)
        });
    }

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
                if self.pos == 0 && take > 0 {
                    let start = origin + *source_sample as f64 / f64::from(rate);
                    self.start_piece(path, *stream, start, rate, channels)?;
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
                // The stream stays open: a later piece may read it too.
                self.piece += 1;
                self.pos = 0;
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
    /// Applied to the whole mix (loudness normalization).
    gain: f32,
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
                    ahead: Ahead::new(),
                    declick,
                    scratch: Vec::new(),
                })
                .collect(),
            rate,
            channels,
            gain: 1.0,
        }
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Changes the whole mix by `db`.
    pub fn set_gain_db(&mut self, db: f64) {
        self.gain = 10f64.powf(db / 20.0) as f32;
    }

    /// The next `n` mixed samples, interleaved.
    pub fn read(&mut self, n: usize) -> Result<Vec<f32>> {
        let mut out = vec![0.0f32; n * usize::from(self.channels)];
        for lane in &mut self.lanes {
            lane.mix_into(&mut out, self.rate, self.channels)?;
        }
        if self.gain != 1.0 {
            for s in &mut out {
                *s *= self.gain;
            }
        }
        Ok(out)
    }
}
