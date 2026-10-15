//! The audio of a render plan: sample-exact source reads, de-click fades at
//! cuts, pieces at other speeds stretched keeping their pitch, and the mix
//! of all lanes, as interleaved `f32`.

use std::path::{Path, PathBuf};

use vidcut_core::render::AudioPiece;

use super::ahead::Ahead;
use crate::error::{Error, Result};
use crate::ffmpeg::{self, codec, ffi, format, frame, media};
use crate::stretch::Stretch;

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
    /// Where the file ends (s) on the packet clock, if the container says.
    /// A frame stamped later has a broken timestamp and plays in order.
    /// Unknown (no positive duration): a jump longer than
    /// [`crate::decode::AUDIO_GAP_BOUND`] plays in order too.
    end: Option<f64>,
    eof: bool,
}

/// How far ahead (seconds) a later piece of the same stream may start for
/// decoding on to it to beat a seek.
const DECODE_AHEAD: i64 = 1;

impl SourceAudio {
    fn open(path: &Path, ordinal: u32, start: f64, rate: u32, channels: u16) -> Result<Self> {
        crate::init();
        let input = crate::decode::open_file(path)?;
        let (index, tb, tick, stream_start, params) = {
            let stream = input
                .streams()
                .filter(|s| s.parameters().medium() == media::Type::Audio)
                .nth(ordinal as usize)
                .ok_or_else(|| {
                    Error::Invalid(format!("{}: no audio stream {ordinal}", path.display()))
                })?;
            let tb = stream.time_base();
            let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
            (
                stream.index(),
                tb,
                tick,
                stream.start_time(),
                stream.parameters(),
            )
        };
        discard_others(&input, index);
        // Duration is a length on the packet clock, not an end at zero, and
        // a missing duration does not mean the file goes on forever.
        let end = crate::decode::container_end(&input, stream_start, tick);
        let id = params.id();
        let decoder = crate::parts::decoder_opened(
            codec::context::Context::from_parameters(params).and_then(|mut c| {
                // Without the packet time base, FFmpeg can't shift a frame's
                // timestamp past skipped priming samples (Opus pre-skip, AAC
                // priming), and audio would start that much late.
                unsafe { (*c.as_mut_ptr()).pkt_timebase = tb.into() };
                c.decoder().audio()
            }),
            id,
            "audio",
        )?;
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
        let frame_end = start.map(|t| t + f.samples() as f64 / f64::from(f.rate().max(1)));
        if let (Some(t), Some(next)) = (start, std::mem::replace(&mut self.next_in, frame_end)) {
            let gap = crate::decode::audio_gap(next, t, self.end);
            if gap > 0.0 {
                // The samples before the gap first, all of them.
                if let Some(mut r) = self.resampler.take() {
                    r.convert(None, &mut self.pending)?;
                }
                self.gap = (gap * f64::from(self.rate)).round() as usize;
                self.held = Some(f);
                return Ok(());
            }
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

/// Sound a start inside a piece at another speed begins before, dropped:
/// the stretch fades its first fragment in.
const PREROLL_SECS: f64 = 0.02;

/// Source read at a time backward, in seconds: each read seeks.
const BACK_SECS: f64 = 0.5;

/// The sound of a piece at another speed (decision D48): its source read
/// forward, or backward half a second at a time and reversed, stretched to
/// the speed keeping its pitch.
struct Paced {
    speed: f64,
    stretch: Option<Stretch>,
    origin: f64,
    /// The source sample read next: forward the first of the next read,
    /// backward the one after its last.
    next: i64,
    /// Where the piece's source ends, the way it reads: silence past it,
    /// where the stretch looks ahead.
    bound: i64,
    /// Made and not given yet (interleaved), from `cursor` on.
    made: Vec<f32>,
    cursor: usize,
    /// Samples still to drop at the start.
    drop: usize,
}

impl Paced {
    fn new(
        speed: f64,
        origin: f64,
        (next, bound): (i64, i64),
        drop: usize,
        rate: u32,
        channels: u16,
    ) -> Self {
        let stretch =
            (speed.abs() != 1.0).then(|| Stretch::new(speed.abs(), rate, usize::from(channels)));
        Self {
            speed,
            stretch,
            origin,
            next,
            bound,
            made: Vec::new(),
            cursor: 0,
            drop,
        }
    }

    /// Raw time (s) its stream opens at: where it reads first.
    fn start(&self, rate: u32) -> f64 {
        let first = if self.speed < 0.0 {
            (self.next - back_samples(rate)).max(self.bound)
        } else {
            self.next
        };
        self.origin + first.max(0) as f64 / f64::from(rate)
    }

    /// Appends its next `n` samples (interleaved) to `out`, read from
    /// `source`.
    fn read(&mut self, source: &mut SourceAudio, n: usize, out: &mut Vec<f32>) -> Result<()> {
        let ch = usize::from(source.channels);
        let rate = source.rate;
        if self.speed == 0.0 {
            out.resize(out.len() + n * ch, 0.0);
            return Ok(());
        }
        let mut read = Vec::new();
        while (self.made.len() - self.cursor) / ch < n + self.drop {
            read.clear();
            let chunk = (rate / 10).max(1) as i64;
            if self.speed > 0.0 {
                let len = chunk.min(self.bound - self.next).max(0) as usize;
                source.read(len, &mut read)?;
                self.next += len as i64;
                read.resize(read.len().max(chunk as usize * ch), 0.0);
            } else {
                let from = (self.next - back_samples(rate)).max(self.bound).max(0);
                let len = (self.next - from) as usize;
                if len == 0 {
                    read.resize(chunk as usize * ch, 0.0);
                } else {
                    source.reposition(self.origin + from as f64 / f64::from(rate))?;
                    source.read(len, &mut read)?;
                    let frames: Vec<f32> = read.chunks_exact(ch).rev().flatten().copied().collect();
                    read = frames;
                    self.next = from;
                }
            }
            match self.stretch.as_mut() {
                Some(s) => {
                    for block in s.push(&read) {
                        self.made.extend_from_slice(&block.samples);
                    }
                }
                None => self.made.extend_from_slice(&read),
            }
        }
        self.cursor += std::mem::take(&mut self.drop) * ch;
        out.extend_from_slice(&self.made[self.cursor..self.cursor + n * ch]);
        self.cursor += n * ch;
        if self.cursor > 1 << 16 {
            self.made.drain(..self.cursor);
            self.cursor = 0;
        }
        Ok(())
    }
}

/// Source samples read at a time backward.
fn back_samples(rate: u32) -> i64 {
    (f64::from(rate) * BACK_SECS).round() as i64
}

/// One audio lane of a plan, read sequentially. A piece from the same stream
/// as the one before it reads on with the same decoder; a piece from another
/// stream is opened while the piece before it plays.
struct Lane {
    pieces: Vec<AudioPiece>,
    piece: usize,
    /// Samples of the current piece already produced.
    pos: i64,
    /// The current piece's stream is ready at `pos`: false at a piece's
    /// start and after a seek into one.
    started: bool,
    source: Option<SourceAudio>,
    /// The current piece's sound, when it is at another speed.
    paced: Option<Paced>,
    /// The next stream, opened ahead; named by where it opens.
    ahead: Ahead<SourceAudio, (PathBuf, u32, u64)>,
    declick: i64,
    scratch: Vec<f32>,
    /// A gain on top of each piece's own (a fader moved while playing).
    trim: f32,
}

impl Lane {
    fn new(pieces: &[AudioPiece], declick: i64) -> Self {
        Self {
            pieces: pieces.to_vec(),
            piece: 0,
            pos: 0,
            started: false,
            source: None,
            paced: None,
            ahead: Ahead::new(),
            declick,
            scratch: Vec::new(),
            trim: 1.0,
        }
    }

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
        let key = (path.clone(), stream, start.to_bits());
        self.ahead.start(i, key, move || {
            SourceAudio::open(&path, stream, start, rate, channels)
        });
    }

    /// The next samples are the lane's from sample `sample` on. The open
    /// stream stays open: a piece of it reads on or seeks it.
    fn seek(&mut self, sample: i64) {
        let mut left = sample.max(0);
        self.ahead.forget();
        self.started = false;
        for (i, piece) in self.pieces.iter().enumerate() {
            let n = piece.samples();
            if left < n {
                self.piece = i;
                self.pos = left;
                return;
            }
            left -= n;
        }
        self.piece = self.pieces.len();
        self.pos = 0;
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
                speed,
                fade: cross,
                ..
            } = &piece
            {
                // A piece starts at its first sample, or where a seek put
                // the lane.
                if take > 0 && (self.pos == 0 || !self.started) {
                    self.paced = None;
                    let start = if *speed == 1.0 {
                        origin + (*source_sample + self.pos) as f64 / f64::from(rate)
                    } else {
                        let preroll = (f64::from(rate) * PREROLL_SECS).round() as i64;
                        let from = (self.pos - preroll).max(0);
                        let at = *source_sample + (from as f64 * speed).round() as i64;
                        let bound = *source_sample + (len as f64 * speed).round() as i64;
                        let drop = (self.pos - from) as usize;
                        let paced = Paced::new(*speed, *origin, (at, bound), drop, rate, channels);
                        let start = paced.start(rate);
                        self.paced = Some(paced);
                        start
                    };
                    self.start_piece(path, *stream, start, rate, channels)?;
                    self.started = true;
                }
                self.scratch.clear();
                let source = self.source.as_mut().expect("open");
                match self.paced.as_mut() {
                    Some(paced) => paced.read(source, take, &mut self.scratch)?,
                    None => source.read(take, &mut self.scratch)?,
                }
                let gain = 10f32.powf(*gain_db as f32 / 20.0) * self.trim;
                // De-click fades at cuts inside the lane (not at the plan's
                // ends): none where the sound plays on into the next piece
                // or out of the one before, nor where a cross fade is
                // silent already (decision D50).
                let fade = self.declick.min(len / 2).max(1);
                let before = self.piece.checked_sub(1).map(|i| &self.pieces[i]);
                let after = self.pieces.get(self.piece + 1);
                let fade_in = before.is_some_and(|b| !plays_on(b, &piece))
                    && !cross.is_some_and(|f| f.starts_silent());
                let fade_out = after.is_some_and(|n| !plays_on(&piece, n))
                    && !cross.is_some_and(|f| f.ends_silent(len));
                for i in 0..take {
                    let p = self.pos + i as i64;
                    let mut g = gain;
                    if let Some(f) = cross {
                        g *= f.gain(p) as f32;
                    }
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

/// Whether `next`'s sound goes on from `piece`'s where a cross fade
/// starts or ends between them: one clip's sound split at its fade, the
/// source read on (decision D50).
fn plays_on(piece: &AudioPiece, next: &AudioPiece) -> bool {
    match (piece, next) {
        (
            AudioPiece::Source {
                path,
                stream,
                source_sample,
                samples,
                speed,
                fade,
                ..
            },
            AudioPiece::Source {
                path: p2,
                stream: s2,
                source_sample: n2,
                speed: sp2,
                fade: f2,
                ..
            },
        ) => {
            (fade.is_some() || f2.is_some())
                && path == p2
                && stream == s2
                && *speed == 1.0
                && *sp2 == 1.0
                && source_sample + samples == *n2
        }
        _ => false,
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
            lanes: lanes.iter().map(|l| Lane::new(l, declick)).collect(),
            rate,
            channels,
            gain: 1.0,
        }
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// How many lanes it mixes.
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }

    /// Changes the whole mix by `db`.
    pub fn set_gain_db(&mut self, db: f64) {
        self.gain = 10f64.powf(db / 20.0) as f32;
    }

    /// Changes lane `lane` by `db`, on top of the gains in its pieces (a
    /// fader being moved). Heard from the next [`read`](Self::read) on.
    pub fn set_lane_db(&mut self, lane: usize, db: f64) -> Result<()> {
        let Some(lane) = self.lanes.get_mut(lane) else {
            return Err(Error::Invalid(format!("no audio lane {lane}")));
        };
        lane.trim = 10f64.powf(db / 20.0) as f32;
        Ok(())
    }

    /// The next reads go on from sample `sample` of the plan (0 is its
    /// start). Each lane keeps its open stream: reading a piece of it seeks
    /// it, or decodes on when that is quicker.
    pub fn seek(&mut self, sample: i64) {
        for lane in &mut self.lanes {
            lane.seek(sample);
        }
    }

    /// Makes `lanes` the plan, reading from sample `sample`: an edited
    /// timeline. Lane `i` keeps the stream lane `i` had open, for a piece
    /// of the same stream to read on with; lane gains on top
    /// ([`set_lane_db`](Self::set_lane_db)) start over.
    pub fn replan(&mut self, lanes: &[Vec<AudioPiece>], sample: i64) {
        let declick = self.lanes.first().map_or(0, |l| l.declick);
        let mut old: Vec<Option<SourceAudio>> =
            self.lanes.iter_mut().map(|l| l.source.take()).collect();
        self.lanes = lanes
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mut lane = Lane::new(l, declick);
                lane.source = old.get_mut(i).and_then(Option::take);
                lane
            })
            .collect();
        self.seek(sample);
    }

    /// [`read`](Self::read), with each lane's peak per channel, before the
    /// mix's gain: lane by lane, `channels` peaks each.
    pub fn read_metered(&mut self, n: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let ch = usize::from(self.channels);
        let mut out = vec![0.0f32; n * ch];
        let mut lane_out = vec![0.0f32; n * ch];
        let mut peaks = vec![0.0f32; self.lanes.len() * ch];
        for (lane, peaks) in self.lanes.iter_mut().zip(peaks.chunks_mut(ch)) {
            lane_out.fill(0.0);
            lane.mix_into(&mut lane_out, self.rate, self.channels)?;
            for (frame, mixed) in lane_out.chunks(ch).zip(out.chunks_mut(ch)) {
                for ((s, m), p) in frame.iter().zip(mixed.iter_mut()).zip(peaks.iter_mut()) {
                    *m += s;
                    *p = p.max(s.abs());
                }
            }
        }
        if self.gain != 1.0 {
            for s in &mut out {
                *s *= self.gain;
            }
        }
        Ok((out, peaks))
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
