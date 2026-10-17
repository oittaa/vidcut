//! Frames that stand alone, decoded on the shared pool instead of frame
//! threads (docs/decisions/threads.md §4.2): every image sequence, and a
//! stream of a codec FFmpeg marks intra-only and decodes on frame threads.
//!
//! The reader demuxes on its own thread and hands each packet to its pool
//! as a task ([`crate::pool::tasks`]), at most [`in_flight`] at a time, and
//! takes the frames back in the stream's order. Each pool thread keeps a
//! decoder of one thread for each kind of stream it has decoded, which
//! serves every file of that kind and closes once unused for [`IDLE`]; one
//! that holds on to the frame it gave closes after it. A task not started
//! when its frame is wanted decodes on the reader's own thread.
//!
//! A decoder lends a frame what an earlier one left in it: its context's
//! colors and pixel aspect, and its guess of the frame's time
//! (gotchas.md). So each task gets the stream's colors back first, and the
//! reader guesses times itself, in the stream's order, as a decoder of its
//! own would.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{DAMAGED_IN_A_ROW, DECODED, Input, Reading, VideoReader, thread_name};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, codec, ffi, format, frame};
use crate::pool::tasks::Ordered;

/// A pool thread's decoder closes once unused for this long.
const IDLE: Duration = Duration::from_secs(10);

/// Frames a reader has in flight: as many as its pool and its own thread
/// decode at once, as many as frame threads would hold.
fn in_flight() -> usize {
    crate::pool::threads()
}

/// Whether the frames of a stream of `source` with `params` stand alone:
/// each file of a sequence is a picture; a file's stream is when its codec
/// is intra-only and FFmpeg decodes it on frame threads, each frame on a
/// context that saw only some of those before it. FFmpeg decodes MJPEG and
/// raw video on one thread: their frames may lean on earlier packets
/// (gotchas.md).
fn stands_alone(source: &Input, params: &codec::Parameters) -> bool {
    if matches!(source, Input::Sequence { .. }) {
        return true;
    }
    // SAFETY: the parameters' own field; FFmpeg's static descriptor and
    // codec, or null.
    unsafe {
        let id = (*params.as_ptr()).codec_id;
        let descriptor = ffi::avcodec_descriptor_get(id);
        let codec = ffi::avcodec_find_decoder(id);
        !descriptor.is_null()
            && (*descriptor).props & ffi::AV_CODEC_PROP_INTRA_ONLY != 0
            && !codec.is_null()
            && (*codec).capabilities & ffi::AV_CODEC_CAP_FRAME_THREADS as libc::c_int != 0
    }
}

/// A kind of stream: what a decoder is opened for. Decoders of one kind
/// decode any of its frames alike.
pub(super) struct Kind {
    params: codec::Parameters,
    time_base: ffmpeg::Rational,
}

// SAFETY: only read once made.
unsafe impl Sync for Kind {}

impl Kind {
    fn new(params: codec::Parameters, time_base: ffmpeg::Rational) -> Self {
        Self { params, time_base }
    }

    /// Whether a decoder of `other` decodes this kind: the same parameters
    /// but the bit rate, and the same time base.
    fn same(&self, other: &Kind) -> bool {
        // SAFETY: both parameters' own fields and data, read only.
        unsafe {
            let (a, b) = (&*self.params.as_ptr(), &*other.params.as_ptr());
            let bytes = |p: *const u8, n: usize| {
                if n == 0 {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(p, n)
                }
            };
            let extradata = |p: &ffi::AVCodecParameters| {
                bytes(p.extradata, usize::try_from(p.extradata_size).unwrap_or(0))
            };
            let side_data = |p: &ffi::AVCodecParameters| {
                let n = usize::try_from(p.nb_coded_side_data).unwrap_or(0);
                (0..n)
                    .map(|i| {
                        let sd = &*p.coded_side_data.add(i);
                        (sd.type_ as i32, bytes(sd.data, sd.size))
                    })
                    .collect::<Vec<_>>()
            };
            let ratio = |r: ffi::AVRational| (r.num, r.den);
            self.time_base == other.time_base
                && a.codec_type == b.codec_type
                && a.codec_id == b.codec_id
                && a.codec_tag == b.codec_tag
                && a.format == b.format
                && a.bits_per_coded_sample == b.bits_per_coded_sample
                && a.bits_per_raw_sample == b.bits_per_raw_sample
                && a.profile == b.profile
                && a.level == b.level
                && a.width == b.width
                && a.height == b.height
                && ratio(a.sample_aspect_ratio) == ratio(b.sample_aspect_ratio)
                && ratio(a.framerate) == ratio(b.framerate)
                && a.field_order == b.field_order
                && a.color_range == b.color_range
                && a.color_primaries == b.color_primaries
                && a.color_trc == b.color_trc
                && a.color_space == b.color_space
                && a.chroma_location == b.chroma_location
                && a.video_delay == b.video_delay
                && extradata(a) == extradata(b)
                && side_data(a) == side_data(b)
        }
    }

    /// A decoder of one thread for it, decoding into buffers of vidcut's
    /// own, as the reader's own is.
    fn open(&self) -> Result<ffmpeg::decoder::Video> {
        let mut ctx =
            codec::context::Context::from_parameters(self.params.clone()).map_err(|e| {
                Error::Ffmpeg {
                    context: "creating video decoder context".into(),
                    source: e,
                }
            })?;
        // SAFETY: the context isn't opened yet.
        unsafe { (*ctx.as_mut_ptr()).pkt_timebase = self.time_base.into() };
        crate::buffers::own(&mut ctx);
        ctx.set_threading(one_thread());
        crate::parts::decoder_opened(ctx.decoder().video(), self.params.id(), "video")
    }

    /// Gives `decoder`'s context the stream's colors and pixel aspect
    /// again, which a decoder lends a frame whose own header leaves them
    /// out: those an earlier frame's header left there would.
    fn restore(&self, decoder: &mut ffmpeg::decoder::Video) {
        // SAFETY: the parameters' own fields, read; the opened context's,
        // written between packets.
        unsafe {
            let p = &*self.params.as_ptr();
            let ctx = &mut *decoder.as_mut_ptr();
            ctx.color_primaries = p.color_primaries;
            ctx.color_trc = p.color_trc;
            ctx.colorspace = p.color_space;
            ctx.color_range = p.color_range;
            ctx.chroma_sample_location = p.chroma_location;
            ctx.sample_aspect_ratio = p.sample_aspect_ratio;
        }
    }
}

/// One thread, no frame threads.
pub(super) fn one_thread() -> codec::threading::Config {
    codec::threading::Config {
        kind: codec::threading::Type::Frame,
        count: 1,
    }
}

/// One packet to decode, and what decodes it.
struct Job {
    packet: ffmpeg::Packet,
    kind: Arc<Kind>,
}

/// What a packet decodes to.
enum Outcome {
    Frame(frame::Video),
    /// The decoder rejected it as damaged.
    Damaged,
    /// No frame, and no error.
    Nothing,
    Failed(Error),
}

/// Decodes `job`'s packet with `decoder`, opened for the job's kind when
/// there is none, and left empty for the next one. A decoder that still
/// holds the frame it gave closes (FFmpeg's PNG decoder keeps its last
/// picture): kept, it would keep the frame's memory once the frame is let
/// go, and a decoder on each pool thread a frame.
fn decode(decoder: &mut Option<ffmpeg::decoder::Video>, job: &Job) -> Outcome {
    let d = match decoder {
        Some(d) => d,
        None => match job.kind.open() {
            Ok(d) => decoder.insert(d),
            Err(e) => return Outcome::Failed(e),
        },
    };
    job.kind.restore(d);
    let mut frame = frame::Video::empty();
    let decoded = d
        .send_packet(&job.packet)
        .and_then(|()| d.receive_frame(&mut frame));
    d.flush();
    // SAFETY: the frame's own buffer, or null.
    let shared = unsafe {
        let buf = (*frame.as_ptr()).buf[0];
        !buf.is_null() && ffi::av_buffer_get_ref_count(buf) > 1
    };
    if shared {
        *decoder = None;
    }
    match decoded {
        Ok(()) => Outcome::Frame(frame),
        Err(ffmpeg::Error::InvalidData) => Outcome::Damaged,
        Err(
            ffmpeg::Error::Eof
            | ffmpeg::Error::Other {
                errno: ffmpeg::util::error::EAGAIN,
            },
        ) => Outcome::Nothing,
        Err(e) => Outcome::Failed(Error::Ffmpeg {
            context: "decoding video".into(),
            source: e,
        }),
    }
}

/// A pool thread's decoder of a kind, and when it last decoded.
struct Kept {
    kind: Arc<Kind>,
    decoder: ffmpeg::decoder::Video,
    used: Instant,
}

thread_local! {
    /// The decoders a pool thread keeps, one for each kind it has decoded.
    static KEPT: RefCell<Vec<Kept>> = const { RefCell::new(Vec::new()) };
}

/// Decodes `job` on a pool thread, with its decoder of the job's kind.
fn on_pool(job: Job) -> Outcome {
    KEPT.with_borrow_mut(|kept| {
        let found = kept
            .iter()
            .position(|k| Arc::ptr_eq(&k.kind, &job.kind) || k.kind.same(&job.kind));
        let mut decoder = found.map(|i| kept.swap_remove(i).decoder);
        let outcome = decode(&mut decoder, &job);
        if let Some(decoder) = decoder {
            kept.push(Kept {
                kind: job.kind.clone(),
                decoder,
                used: Instant::now(),
            });
            if found.is_none() {
                crate::debug!(
                    "decoding",
                    "{}: keeps a decoder for frames that stand alone, {} ({} kept)",
                    thread_name(),
                    job.kind.params.id().name(),
                    kept.len()
                );
            }
        }
        outcome
    })
}

/// Closes the calling pool thread's decoders unused for [`IDLE`], and says
/// when the next of the others will be.
pub(crate) fn close_idle() -> Option<Instant> {
    KEPT.with_borrow_mut(|kept| {
        let before = kept.len();
        kept.retain(|k| k.used.elapsed() < IDLE);
        if kept.len() < before {
            crate::debug!(
                "decoding",
                "{}: closed {} idle decoders for frames that stand alone ({} kept)",
                thread_name(),
                before - kept.len(),
                kept.len()
            );
        }
        kept.iter().map(|k| k.used + IDLE).min()
    })
}

/// FFmpeg's guess of a frame's time from its packet's (`guess_correct_pts`
/// in decode.c): the pts, unless more of the pts than of the dts have gone
/// back so far. Kept for a reader across its frames, as its decoder keeps
/// it.
struct Times {
    faulty_pts: u64,
    faulty_dts: u64,
    last_pts: i64,
    last_dts: i64,
}

impl Times {
    fn new() -> Self {
        Self {
            faulty_pts: 0,
            faulty_dts: 0,
            last_pts: i64::MIN,
            last_dts: i64::MIN,
        }
    }

    /// After a flush, as FFmpeg's flush leaves it.
    fn flushed(&mut self) {
        self.last_pts = i64::MIN;
        self.last_dts = i64::MIN;
    }

    fn guess(&mut self, pts: Option<i64>, dts: Option<i64>) -> Option<i64> {
        if let Some(dts) = dts {
            self.faulty_dts += u64::from(dts <= self.last_dts);
            self.last_dts = dts;
        } else if let Some(pts) = pts {
            self.last_dts = pts;
        }
        if let Some(pts) = pts {
            self.faulty_pts += u64::from(pts <= self.last_pts);
            self.last_pts = pts;
        } else if let Some(dts) = dts {
            self.last_pts = dts;
        }
        match pts {
            Some(_) if self.faulty_pts <= self.faulty_dts || dts.is_none() => pts,
            _ => dts,
        }
    }
}

/// A reader's frames that stand alone, in flight on the pool.
pub(super) struct Standalone {
    kind: Arc<Kind>,
    frames: Ordered<Job, Outcome>,
    /// What decodes those that come to the reader's own thread.
    own: Option<ffmpeg::decoder::Video>,
    times: Times,
}

impl Standalone {
    /// For a stream of `source` read as `reading` on the CPU, when its
    /// frames stand alone ([`stands_alone`]).
    pub(super) fn for_stream(
        source: &Input,
        stream: &format::stream::Stream<'_>,
        reading: Reading,
    ) -> Option<Self> {
        let params = stream.parameters();
        // A copy of its own: the stream's keeps the file open.
        (reading == Reading::Stream && stands_alone(source, &params)).then(|| Self {
            kind: Arc::new(Kind::new(params.clone(), stream.time_base())),
            frames: Ordered::new(on_pool),
            own: None,
            times: Times::new(),
        })
    }

    /// Drops the frames in flight: the reader's decoder was flushed.
    pub(super) fn flushed(&mut self) {
        self.frames.clear();
        self.times.flushed();
    }

    /// Frames in flight, for `VIDCUT_DEBUG`.
    pub(super) fn note() -> String {
        format!("frames on the pool, {} in flight", in_flight())
    }
}

/// [`VideoReader::decode_one`] for frames that stand alone: the next frame,
/// with packets handed to the pool to keep [`in_flight`] of them decoding.
pub(super) fn decode_one(r: &mut VideoReader) -> Result<Option<frame::Video>> {
    let mut damaged = 0;
    loop {
        crate::cancel::stop()?;
        crate::first::wait()?;
        fill(r)?;
        let s = r.standalone.as_mut().expect("frames that stand alone");
        let own = &mut s.own;
        let taken = s
            .frames
            .take(|job| decode(own, &job), crate::cancel::stop)?;
        match taken {
            None => return Ok(None),
            Some(Outcome::Frame(mut f)) => {
                // SAFETY: the frame's own field.
                let dts = unsafe { (*f.as_ptr()).pkt_dts };
                let dts = (dts != ffi::AV_NOPTS_VALUE).then_some(dts);
                let pts = s.times.guess(f.pts(), dts);
                f.set_pts(pts);
                DECODED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(Some(f));
            }
            Some(Outcome::Damaged) if damaged < DAMAGED_IN_A_ROW => damaged += 1,
            Some(Outcome::Damaged) => {
                return Err(Error::Ffmpeg {
                    context: "decoding video".into(),
                    source: ffmpeg::Error::InvalidData,
                });
            }
            Some(Outcome::Nothing) => {}
            Some(Outcome::Failed(e)) => return Err(e),
        }
    }
}

/// Hands the pool the stream's next packets, the one a seek read first,
/// until [`in_flight`] are, or the stream ends.
fn fill(r: &mut VideoReader) -> Result<()> {
    let s = r.standalone.as_mut().expect("frames that stand alone");
    while s.frames.len() < in_flight() && !r.draining {
        let packet = match r.packet.take() {
            Some(p) => Some(p),
            None => loop {
                crate::cancel::stop()?;
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut r.input) {
                    Ok(()) if packet.stream() == r.stream => break Some(packet),
                    Ok(()) => continue,
                    Err(ffmpeg::Error::Eof) => break None,
                    Err(e) => {
                        return Err(Error::Ffmpeg {
                            context: "reading packet".into(),
                            source: e,
                        });
                    }
                }
            },
        };
        match packet {
            Some(packet) => s.frames.push(Job {
                packet,
                kind: s.kind.clone(),
            }),
            None => r.draining = true,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Times;

    #[test]
    fn a_frame_without_a_pts_takes_its_dts() {
        assert_eq!(Times::new().guess(None, Some(5)), Some(5));
    }

    #[test]
    fn dts_gone_back_more_often_than_pts_leave_the_pts() {
        let mut times = Times::new();
        times.guess(Some(10), Some(4));
        assert_eq!(times.guess(Some(11), Some(3)), Some(11));
    }

    #[test]
    fn pts_gone_back_more_often_than_dts_give_the_dts() {
        let mut times = Times::new();
        times.guess(Some(10), Some(1));
        assert_eq!(times.guess(Some(9), Some(2)), Some(2));
    }
}
