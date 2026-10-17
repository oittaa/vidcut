//! Measurements for the analysis tools ([`vidcut_core::analysis`]): the
//! peak levels of an audio stream, the scene score and black share of every
//! video frame, and the loudness of a render plan's mix.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use vidcut_core::analysis::{AudioLevels, Loudness, PictureStats};
use vidcut_core::render::{AudioPiece, RenderPlan};

use crate::decode::decode_stream;
use crate::error::{Context, Error, Result};
use crate::export::audio::{AudioMix, Resampler, discard_others};
use crate::ffmpeg::{self, codec, filter, format, frame, media, software::scaling};

/// Peak levels of audio stream `ordinal` of `path` (counting audio streams
/// only): the largest absolute sample of any channel in each
/// `1 / per_second` of a second.
pub fn audio_levels(path: &Path, ordinal: u32, per_second: u32) -> Result<AudioLevels> {
    crate::cancel::stop()?;
    crate::init();
    let mut input = crate::decode::open_file(path)?;
    let stream = input
        .streams()
        .filter(|s| s.parameters().medium() == media::Type::Audio)
        .nth(ordinal as usize)
        .ok_or_else(|| Error::Invalid(format!("{}: no audio stream {ordinal}", path.display())))?;
    let (index, tb, start_pts) = (stream.index(), stream.time_base(), stream.start_time());
    let params = stream.parameters();
    let tick = f64::from(tb);
    discard_others(&input, index);
    // Same end as export: a past-end timestamp is not a hole in the levels.
    let file_end = crate::decode::container_end(&input, start_pts, tick);
    let id = params.id();
    let mut decoder = crate::parts::decoder_opened(
        codec::context::Context::from_parameters(params).and_then(|mut c| {
            unsafe { (*c.as_mut_ptr()).pkt_timebase = tb.into() };
            c.decoder().audio()
        }),
        id,
        "audio",
    )?;
    let per_second = per_second.max(1);
    let mut resampler: Option<(Resampler, u32, usize)> = None;
    let mut samples = Vec::new();
    let mut peaks: Vec<f32> = Vec::new();
    let (mut n, mut current) = (0u64, 0f32);
    let mut start: Option<f64> = None;
    let mut next_in: Option<f64> = None;
    let handle = |f: &frame::Audio| -> Result<()> {
        let t = f.timestamp().map(|t| t as f64 * tick);
        if start.is_none() {
            start = t;
        }
        if resampler.as_ref().is_none_or(|(r, _, _)| !r.matches(f)) {
            let channels = f.channels().max(1);
            resampler = Some((
                Resampler::new(f, f.rate(), channels)?,
                f.rate().max(1),
                usize::from(channels),
            ));
        }
        let (r, rate, channels) = resampler.as_mut().expect("set");
        // A gap lasts as its timestamps say, as in export: not past the
        // file's end, and not for a far jump when that end is unknown.
        let frame_end = t.map(|t| t + f.samples() as f64 / f64::from(*rate));
        if let (Some(t), Some(next)) = (t, std::mem::replace(&mut next_in, frame_end)) {
            let gap = crate::decode::audio_gap(next, t, file_end);
            if gap > 0.0 {
                n += (gap * f64::from(*rate)).round() as u64;
            }
        }
        samples.clear();
        r.convert(Some(f), &mut samples)?;
        for frame in samples.chunks_exact(*channels) {
            let bucket = (n * u64::from(per_second) / u64::from(*rate)) as usize;
            while peaks.len() < bucket {
                peaks.push(current);
                current = 0.0;
            }
            current = frame.iter().fold(current, |m, s| m.max(s.abs()));
            n += 1;
        }
        Ok(())
    };
    decode_stream(
        &mut input,
        index,
        &mut decoder,
        &mut frame::Audio::empty(),
        handle,
    )?;
    if n > 0 {
        peaks.push(current);
    }
    Ok(AudioLevels {
        per_second,
        start: start.unwrap_or(0.0),
        peaks,
    })
}

/// The size pictures are measured at: enough to tell shots apart, small
/// enough to be cheap.
const MEASURE: (u32, u32) = (96, 54);

/// Scene score and black share of every frame of `path`'s video. The score
/// is FFmpeg scdet's: the mean absolute difference from the previous frame
/// (0..100), or its change from the previous difference if smaller, so a
/// steady pan doesn't read as a cut.
///
/// An image sequence is read by image2, which continues while the file
/// numbers do. Pass that sequence's own length as `pictures`; frames after
/// it are not measured and not opened. `None` reads the whole source.
/// The video decodes on `gpu` where it decodes the stream, else on the CPU.
pub fn picture_stats(
    source: &crate::decode::Input,
    pictures: Option<u64>,
    gpu: Option<&crate::hw::GpuDecoder>,
) -> Result<PictureStats> {
    // Decoded in memory instead, or not at all: the GPU's decoder wouldn't
    // take the stream (a profile it lacks), so the CPU's does.
    if gpu.is_some()
        && let Ok(Some(stats)) = stats_on(source, pictures, gpu)
    {
        return Ok(stats);
    }
    stats_on(source, pictures, None).map(Option::unwrap_or_default)
}

/// [`picture_stats`] decoded on `gpu`, or on the CPU without it. `None`
/// when the first frame comes out in memory though `gpu` was given, or no
/// frame does.
fn stats_on(
    source: &crate::decode::Input,
    pictures: Option<u64>,
    gpu: Option<&crate::hw::GpuDecoder>,
) -> Result<Option<PictureStats>> {
    crate::cancel::stop()?;
    if pictures == Some(0) {
        return Ok(Some(PictureStats::default()));
    }
    let path = source.path();
    let mut input = source.open()?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| Error::Invalid(format!("{}: no video stream", path.display())))?;
    let (index, tb) = (stream.index(), stream.time_base());
    let tick = f64::from(tb);
    let rate = f64::from(stream.avg_frame_rate());
    let mut ctx = codec::context::Context::from_parameters(stream.parameters())
        .ctx(|| "creating video decoder context".into())?;
    unsafe { (*ctx.as_mut_ptr()).pkt_timebase = tb.into() };
    ctx.set_threading(codec::threading::Config {
        kind: codec::threading::Type::Frame,
        count: crate::pool::threads_for(crate::decode::Reading::Stream),
    });
    let id = stream.parameters().id();
    let mut decoder =
        match crate::decode::open_video(ctx, id, gpu, || crate::decode::h264_open_gop(source)) {
            Ok(decoder) => decoder,
            // A decoder this FFmpeg lacks, as `parts::decoder_opened` says.
            Err(Error::Ffmpeg {
                source: ffmpeg::Error::DecoderNotFound,
                ..
            }) => {
                return Err(Error::Missing(format!(
                    "this FFmpeg has no {} decoder",
                    id.name()
                )));
            }
            Err(e) => return Err(e),
        };
    discard_others(&input, index);
    // A thread of its own measures each frame (copied down whole, scaled
    // and scored, in order) while the next ones decode.
    let (to_measure, inbox) = std::sync::mpsc::sync_channel::<frame::Video>(4);
    let (mut told, mut refused) = (false, false);
    let (decoded, stats) = std::thread::scope(|scope| {
        let measuring = scope.spawn(move || -> Result<PictureStats> {
            let mut m = Measuring::new(tick, rate);
            for f in inbox {
                m.add(&f)?;
            }
            Ok(m.stats)
        });
        let handle = |decoded: &frame::Video| -> Result<()> {
            if !std::mem::replace(&mut told, true) {
                let on = crate::decode::decoded_on(decoded, gpu);
                crate::debug!("decoding", "{}: scene analysis on {on}", path.display());
                if gpu.is_some() && !crate::hw::on_gpu(decoded) {
                    refused = true;
                    return Err(Error::Invalid("decoded in memory".into()));
                }
            }
            // Measuring stopped: its error follows.
            to_measure
                .send(crate::export::video::frame_ref(decoded)?)
                .map_err(|_| Error::Invalid("measuring stopped".into()))
        };
        let decoded = match pictures {
            Some(n) => decode_pictures(&mut input, index, &mut decoder, n, handle),
            None => decode_stream(
                &mut input,
                index,
                &mut decoder,
                &mut frame::Video::empty(),
                handle,
            ),
        };
        drop(to_measure);
        (decoded, measuring.join().expect("measuring pictures"))
    });
    let stats = stats?;
    if gpu.is_some() && (refused || stats.pts.is_empty()) {
        return Ok(None);
    }
    decoded?;
    Ok(Some(stats))
}

/// Scene scores and black shares, measured a frame at a time in order.
struct Measuring {
    tick: f64,
    rate: f64,
    scaler: Option<scaling::Context>,
    prev: Option<Vec<u8>>,
    prev_mafd: f64,
    stats: PictureStats,
}

impl Measuring {
    fn new(tick: f64, rate: f64) -> Self {
        Self {
            tick,
            rate,
            scaler: None,
            prev: None,
            prev_mafd: 0.0,
            stats: PictureStats::default(),
        }
    }

    fn add(&mut self, decoded: &frame::Video) -> Result<()> {
        let (w, h) = MEASURE;
        let count = (w * h) as usize;
        // Whole, so the measures are the CPU's to the bit.
        let down;
        let decoded = if crate::hw::on_gpu(decoded) {
            down = crate::hw::download(decoded)?;
            &down
        } else {
            decoded
        };
        let scaler = match &mut self.scaler {
            Some(s) if s.input().format == decoded.format() => s,
            _ => self.scaler.insert(
                scaling::Context::get(
                    decoded.format(),
                    decoded.width(),
                    decoded.height(),
                    format::Pixel::YUV420P,
                    w,
                    h,
                    scaling::Flags::AREA,
                )
                .ctx(|| "creating the measuring scaler".into())?,
            ),
        };
        let mut small = frame::Video::empty();
        scaler
            .run(decoded, &mut small)
            .ctx(|| "scaling a frame to measure".into())?;
        let stride = small.stride(0);
        let luma: Vec<u8> = (0..h as usize)
            .flat_map(|y| small.data(0)[y * stride..y * stride + w as usize].to_vec())
            .collect();
        // Black: luma at most 10% of the way from black to white.
        let full = decoded.color_range() == ffmpeg::color::Range::JPEG;
        let limit = if full { 25 } else { 16 + 22 };
        let black = luma.iter().filter(|&&y| y <= limit).count() as f32 / count as f32;
        let score = match &self.prev {
            Some(p) => {
                let sad: u64 = luma
                    .iter()
                    .zip(p)
                    .map(|(&a, &b)| u64::from(a.abs_diff(b)))
                    .sum();
                let mafd = sad as f64 * 100.0 / count as f64 / 256.0;
                let diff = (mafd - self.prev_mafd).abs();
                self.prev_mafd = mafd;
                mafd.min(diff).clamp(0.0, 100.0) as f32
            }
            None => 0.0,
        };
        let stats = &mut self.stats;
        let pts = decoded
            .timestamp()
            .map(|t| t as f64 * self.tick)
            .or_else(|| stats.pts.last().map(|p| p + 1.0 / self.rate.max(1.0)))
            .unwrap_or(0.0);
        stats.pts.push(pts);
        stats.score.push(score);
        stats.black.push(black);
        self.prev = Some(luma);
        Ok(())
    }
}

/// Decodes at most `pictures` frames. The packet after the last one is not
/// read: for an image sequence that packet is the next numbered file.
fn decode_pictures(
    input: &mut format::context::Input,
    index: usize,
    decoder: &mut ffmpeg::decoder::Opened,
    pictures: u64,
    mut each: impl FnMut(&frame::Video) -> Result<()>,
) -> Result<()> {
    let mut frame = frame::Video::empty();
    let mut damaged = 0u32;
    let mut got = 0u64;
    let mut eof = false;
    loop {
        if got >= pictures {
            return Ok(());
        }
        crate::first::wait()?;
        match decoder.receive_frame(&mut frame) {
            Ok(()) => {
                damaged = 0;
                each(&frame)?;
                got += 1;
                continue;
            }
            Err(ffmpeg::Error::InvalidData) if damaged < crate::decode::DAMAGED_IN_A_ROW => {
                damaged += 1;
                continue;
            }
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(ffmpeg::Error::Other {
                errno: ffmpeg::util::error::EAGAIN,
            }) => {}
            Err(e) => {
                return Err(Error::Ffmpeg {
                    context: "decoding".into(),
                    source: e,
                });
            }
        }
        if eof || got >= pictures {
            return Ok(());
        }
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(input) {
            Ok(()) if packet.stream() == index => {
                crate::decode::send_packet(decoder, &packet)?;
            }
            Ok(()) => continue,
            Err(ffmpeg::Error::Eof) => {
                decoder.send_eof().ctx(|| "flushing decoder".into())?;
                eof = true;
            }
            Err(e) => {
                return Err(Error::Ffmpeg {
                    context: "reading packet".into(),
                    source: e,
                });
            }
        }
    }
}

/// Loudness of `plan`'s audio mixed as an export mixes it (FFmpeg's
/// ebur128 filter: EBU R128 integrated loudness, range and true peak).
pub fn loudness(plan: &RenderPlan) -> Result<Loudness> {
    mix_loudness(
        &plan.audio,
        plan.sample_rate,
        plan.channels,
        crate::export::DECLICK_MS,
        plan.samples(),
        None,
    )
}

/// The export's own flag, or the one installed on this thread for an analysis.
fn stopped(cancel: Option<&AtomicBool>) -> bool {
    crate::cancel::cancelled() || cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

/// Loudness of `total` samples of `lanes` mixed at `rate` into `channels`
/// (the export's own mix, before any normalization). `cancel` stops it.
pub fn mix_loudness(
    lanes: &[Vec<AudioPiece>],
    rate: u32,
    channels: u16,
    declick_ms: u32,
    total: i64,
    cancel: Option<&AtomicBool>,
) -> Result<Loudness> {
    // The export passes its flag; an analysis relies on the thread's guard.
    // Either one stopping the client stops the measure.
    if stopped(cancel) {
        return Err(Error::Invalid(crate::export::CANCELLED.into()));
    }
    crate::init();
    let channels = channels.max(1);
    let mut mix = AudioMix::new(lanes, rate, channels, declick_ms);
    let mut graph = crate::pool::graph(crate::decode::Reading::Stream);
    let layout = match channels {
        1 => "mono",
        2 => "stereo",
        6 => "5.1",
        _ => {
            return Err(Error::Invalid(format!(
                "{channels} channels: can't measure"
            )));
        }
    };
    let args =
        format!("sample_rate={rate}:sample_fmt=flt:channel_layout={layout}:time_base=1/{rate}");
    graph
        .add(
            &filter::find("abuffer").ok_or_else(|| Error::Invalid("no abuffer filter".into()))?,
            "in",
            &args,
        )
        .ctx(|| "creating the audio source".into())?;
    graph
        .add(
            &filter::find("abuffersink")
                .ok_or_else(|| Error::Invalid("no abuffersink filter".into()))?,
            "out",
            "",
        )
        .ctx(|| "creating the audio sink".into())?;
    let spec = "ebur128=peak=true:metadata=1";
    crate::parts::parse_filters(&mut graph, spec)?;
    graph
        .validate()
        .ctx(|| format!("configuring filter {spec}"))?;
    let mut result = Loudness {
        integrated: f64::NEG_INFINITY,
        range: 0.0,
        true_peak: f64::NEG_INFINITY,
    };
    let drain = |graph: &mut filter::Graph, result: &mut Loudness| {
        let mut out = frame::Audio::empty();
        while graph
            .get("out")
            .expect("sink")
            .sink()
            .frame(&mut out)
            .is_ok()
        {
            let meta = out.metadata();
            let value = |k: &str| meta.get(k).and_then(|v| v.parse::<f64>().ok());
            if let Some(i) = value("lavfi.r128.I") {
                result.integrated = i;
            }
            if let Some(lra) = value("lavfi.r128.LRA") {
                result.range = lra;
            }
            if let Some(peak) = value("lavfi.r128.true_peak") {
                result.true_peak = 20.0 * peak.max(1e-12).log10();
            }
        }
    };
    let ch = usize::from(channels);
    let mut done = 0i64;
    while done < total {
        if stopped(cancel) {
            return Err(Error::Invalid(crate::export::CANCELLED.into()));
        }
        let n = (total - done).min(4096) as usize;
        let samples = mix.read(n)?;
        let mut f = frame::Audio::new(
            format::Sample::F32(format::sample::Type::Packed),
            n,
            ffmpeg::ChannelLayout::default(i32::from(channels)),
        );
        f.set_rate(rate);
        f.set_pts(Some(done));
        let data = f.data_mut(0);
        for (i, s) in samples.iter().take(n * ch).enumerate() {
            data[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        graph
            .get("in")
            .expect("source")
            .source()
            .add(&f)
            .ctx(|| "measuring loudness".into())?;
        drain(&mut graph, &mut result);
        done += n as i64;
    }
    graph
        .get("in")
        .expect("source")
        .source()
        .flush()
        .ctx(|| "measuring loudness".into())?;
    drain(&mut graph, &mut result);
    // Below the absolute gate (-70 LUFS), ebur128 reports -70: silence.
    if result.integrated <= -70.0 {
        result.integrated = f64::NEG_INFINITY;
    }
    Ok(result)
}
