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
    crate::init();
    let mut input = format::input(path).ctx(|| format!("opening {}", path.display()))?;
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
    let file_end = crate::decode::container_end(&mut input, index, start_pts, tick);
    let mut decoder = codec::context::Context::from_parameters(params)
        .and_then(|mut c| {
            unsafe { (*c.as_mut_ptr()).pkt_timebase = tb.into() };
            c.decoder().audio()
        })
        .ctx(|| "opening audio decoder".into())?;
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
pub fn picture_stats(source: &crate::decode::Input) -> Result<PictureStats> {
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
        count: 0,
    });
    let mut decoder = ctx
        .decoder()
        .video()
        .ctx(|| "opening video decoder".into())?;
    discard_others(&input, index);
    let (w, h) = MEASURE;
    let count = (w * h) as usize;
    let mut scaler: Option<scaling::Context> = None;
    let mut stats = PictureStats::default();
    let mut prev: Option<Vec<u8>> = None;
    let mut prev_mafd = 0f64;
    let handle = |decoded: &frame::Video| -> Result<()> {
        let scaler = match &mut scaler {
            Some(s) if s.input().format == decoded.format() => s,
            _ => scaler.insert(
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
        let score = match &prev {
            Some(p) => {
                let sad: u64 = luma
                    .iter()
                    .zip(p)
                    .map(|(&a, &b)| u64::from(a.abs_diff(b)))
                    .sum();
                let mafd = sad as f64 * 100.0 / count as f64 / 256.0;
                let diff = (mafd - prev_mafd).abs();
                prev_mafd = mafd;
                mafd.min(diff).clamp(0.0, 100.0) as f32
            }
            None => 0.0,
        };
        let pts = decoded
            .timestamp()
            .map(|t| t as f64 * tick)
            .or_else(|| stats.pts.last().map(|p| p + 1.0 / rate.max(1.0)))
            .unwrap_or(0.0);
        stats.pts.push(pts);
        stats.score.push(score);
        stats.black.push(black);
        prev = Some(luma);
        Ok(())
    };
    decode_stream(
        &mut input,
        index,
        &mut decoder,
        &mut frame::Video::empty(),
        handle,
    )?;
    Ok(stats)
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
    crate::init();
    let channels = channels.max(1);
    let mut mix = AudioMix::new(lanes, rate, channels, declick_ms);
    let mut graph = filter::Graph::new();
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
    graph
        .output("in", 0)
        .and_then(|p| p.input("out", 0))
        .and_then(|p| p.parse(spec))
        .ctx(|| format!("building filter {spec}"))?;
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
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
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
