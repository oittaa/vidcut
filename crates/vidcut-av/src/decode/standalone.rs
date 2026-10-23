use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{DAMAGED_IN_A_ROW, DECODED, Input, Reading, VideoReader, thread_name};
use crate::error::{Error, Result};
use crate::ffmpeg::{self, codec, ffi, format, frame};
use crate::pool::tasks::Ordered;

const IDLE: Duration = Duration::from_secs(10);

fn in_flight() -> usize {
    crate::pool::threads_for(Reading::Stream)
}

fn stands_alone(source: &Input, params: &codec::Parameters) -> bool {
    if matches!(source, Input::Sequence { .. }) {
        return true;
    }
    // SAFETY: the parameters' own field; FFmpeg's static descriptor and codec, or null.
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

    fn same(&self, other: &Kind) -> bool {
        // SAFETY: both parameters' own fields and data, read only.
        unsafe {
            let (a, b) = (&*self.params.as_ptr(), &*other.params.as_ptr());
            let bytes = |p: *const u8, n: usize| {
                if p.is_null() || n == 0 {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(p, n)
                }
            };
            let extradata = |p: &ffi::AVCodecParameters| {
                bytes(p.extradata, usize::try_from(p.extradata_size).unwrap_or(0))
            };
            let side_data = |p: &ffi::AVCodecParameters| {
                if p.coded_side_data.is_null() {
                    return Vec::new();
                }
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

    fn restore(&self, decoder: &mut ffmpeg::decoder::Video) {
        // SAFETY: the parameters' fields, read; the opened context's, written between packets.
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

pub(super) fn one_thread() -> codec::threading::Config {
    codec::threading::Config {
        kind: codec::threading::Type::Frame,
        count: 1,
    }
}

struct Job {
    packet: ffmpeg::Packet,
    kind: Arc<Kind>,
}

enum Outcome {
    Frame(frame::Video),
    Damaged,
    Nothing,
    Failed(Error),
}

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

struct Kept {
    kind: Arc<Kind>,
    decoder: ffmpeg::decoder::Video,
    used: Instant,
}

thread_local! {
    static KEPT: RefCell<Vec<Kept>> = const { RefCell::new(Vec::new()) };
}

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

pub(super) struct Standalone {
    kind: Arc<Kind>,
    frames: Ordered<Job, Outcome>,
    own: Option<ffmpeg::decoder::Video>,
    times: Times,
}

impl Standalone {
    pub(super) fn for_stream(
        source: &Input,
        stream: &format::stream::Stream<'_>,
        reading: Reading,
    ) -> Option<Self> {
        let params = stream.parameters();
        (reading == Reading::Stream && stands_alone(source, &params)).then(|| Self {
            kind: Arc::new(Kind::new(params.clone(), stream.time_base())),
            frames: Ordered::new(on_pool),
            own: None,
            times: Times::new(),
        })
    }

    pub(super) fn flushed(&mut self) {
        self.frames.clear();
        self.times.flushed();
    }

    pub(super) fn note() -> String {
        format!("frames on the pool, {} in flight", in_flight())
    }
}

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
