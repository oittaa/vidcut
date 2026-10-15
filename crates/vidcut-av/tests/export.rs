//! Export engine tests: render real fixtures and verify the output frame by
//! frame (barcodes) and sample by sample (beep onsets).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::export::{self, encode::Availability};
use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, ImageSpec, Spoil, VideoCodec};
use vidcut_core::export::{
    AudioCodec as ACodec, AudioSettings, Container as OutContainer, EncoderChoice, ExportSettings,
    HdrMode, VideoCodec as VCodec, VideoSettings,
};
use vidcut_core::render::{AudioPiece, Image, Pace, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

/// A folder for one test's outputs, removed when it drops, also when the
/// test fails.
fn out_dir(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("vidcut-export-{name}-"))
        .tempdir()
        .unwrap()
}

struct Src {
    path: PathBuf,
    origin: f64,
}

fn src(spec: FixtureSpec) -> Src {
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path, None).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    Src { path, origin }
}

fn v(s: &Src, source_frame: i64, frames: i64) -> VideoPiece {
    VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: s.path.clone(),
        origin: s.origin,
        source_frame,
        frames,
        image: None,
    }
}

fn a(s: &Src, fps: Rational, rate: u32, source_frame: i64, frames: i64) -> AudioPiece {
    AudioPiece::Source {
        speed: 1.0,
        media: "m".into(),
        path: s.path.clone(),
        stream: 0,
        origin: s.origin,
        source_sample: fps.frame_to_sample(source_frame, rate),
        samples: fps.frame_to_sample(frames, rate),
        gain_db: 0.0,
        fade: None,
    }
}

fn plan(
    fps: Rational,
    w: u32,
    h: u32,
    video: Vec<VideoPiece>,
    audio: Vec<Vec<AudioPiece>>,
) -> RenderPlan {
    let frames = video.iter().map(VideoPiece::frames).sum();
    RenderPlan {
        fps,
        width: w,
        height: h,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video,
        audio,
        range: (0, frames),
        name: "t".into(),
    }
}

/// Lossless-enough settings so barcodes and beeps survive exactly: FFV1 + PCM in MKV.
fn lossless() -> ExportSettings {
    ExportSettings {
        container: OutContainer::Mkv,
        video: Some(VideoSettings {
            codec: VCodec::H264,
            encoder: EncoderChoice::Cpu,
            quality: Some(0),
            bitrate: None,
            speed: Some("ultrafast".into()),
            profile: None,
            bit_depth: None,
            width: None,
            height: None,
            hdr: HdrMode::Sdr,
            passes: None,
            first_pass: Default::default(),
            multipass: Default::default(),
            lookahead: None,
            hdr_metadata: Default::default(),
            mastering_display: None,
            max_cll: None,
        }),
        audio: Some(AudioSettings {
            codec: ACodec::PcmS16,
            bitrate: None,
            sample_rate: None,
            channels: None,
            loudness: None,
        }),
        declick: false,
    }
}

fn render(plan: &RenderPlan, settings: &ExportSettings, out: &Path) -> export::Report {
    export::run(
        plan,
        settings,
        out,
        &Availability::default(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_or_else(|e| panic!("export: {e}"))
}

/// Barcodes of every output frame.
fn barcodes(path: &Path) -> Vec<Option<u32>> {
    let mut r = VideoReader::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        out.push(read_barcode(&f).unwrap());
    }
    out
}

/// Decodes the first audio stream to mono f32 at its own rate.
fn audio_mono(path: &Path) -> Vec<f32> {
    use vidcut_av::ffmpeg::{self, codec, format, frame, media};
    vidcut_av::init();
    let mut input = format::input(path).unwrap();
    let stream = input.streams().best(media::Type::Audio).unwrap();
    let index = stream.index();
    let mut dec = codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .audio()
        .unwrap();
    let mut out = Vec::new();
    // Tests export 48 kHz 16-bit PCM, so no resampling is needed here.
    let take = |dec: &mut ffmpeg::decoder::Audio, out: &mut Vec<f32>| {
        let mut f = frame::Audio::empty();
        while dec.receive_frame(&mut f).is_ok() {
            let ch = f.channels() as usize;
            let n = f.samples();
            match f.format() {
                format::Sample::I16(format::sample::Type::Packed) => {
                    let d = f.data(0);
                    for i in 0..n {
                        let v = i16::from_le_bytes([d[i * ch * 2], d[i * ch * 2 + 1]]);
                        out.push(f32::from(v) / 32768.0);
                    }
                }
                other => panic!("unexpected sample format {other:?}"),
            }
        }
    };
    for (s, p) in input.packets() {
        if s.index() == index {
            dec.send_packet(&p).unwrap();
            take(&mut dec, &mut out);
        }
    }
    dec.send_eof().unwrap();
    take(&mut dec, &mut out);
    out
}

#[test]
fn same_rate_cuts_are_frame_and_sample_exact() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::PcmS16,
            48_000,
            2,
        ),
    );
    let b1 = src(
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::Flac,
            48_000,
            2,
        ),
    );
    let ts = src(
        FixtureSpec::new(VideoCodec::H264, Container::MpegTs, 320, 180, (30, 1), 90)
            .start_offset_ms(1400),
    );
    let np = src(FixtureSpec::new(
        VideoCodec::Vp9,
        Container::Webm,
        320,
        180,
        (30, 1),
        60,
    ));
    // Segments: a1 10..40, b1 45..75, ts 31..61, black 5, np 3..33.
    let pieces = vec![
        v(&a1, 10, 30),
        v(&b1, 45, 30),
        v(&ts, 31, 30),
        VideoPiece::Black { frames: 5 },
        v(&np, 3, 30),
    ];
    let lane = vec![
        a(&a1, fps, 48_000, 10, 30),
        a(&b1, fps, 48_000, 45, 30),
        AudioPiece::Silence {
            samples: fps.frame_to_sample(35, 48_000),
        },
        a(&np, fps, 48_000, 3, 30),
    ];
    let p = plan(fps, 320, 180, pieces, vec![lane]);
    let dir = out_dir("exact");
    let out = dir.path().join("out.mkv");
    let report = render(&p, &lossless(), &out);
    assert_eq!(report.frames, 125);

    let codes = barcodes(&out);
    let mut want: Vec<Option<u32>> = Vec::new();
    want.extend((10..40).map(Some));
    want.extend((45..75).map(Some));
    want.extend((31..61).map(Some));
    want.extend(std::iter::repeat_n(None, 5));
    want.extend((3..33).map(Some));
    assert_eq!(codes.len(), want.len(), "frame count");
    let bad: Vec<_> = codes
        .iter()
        .zip(&want)
        .enumerate()
        .filter(|(_, (g, w))| g != w)
        .collect();
    assert!(bad.is_empty(), "frames differing from the plan: {bad:?}");

    // Audio: exact length; beeps where the sources had them.
    let mono = audio_mono(&out);
    assert_eq!(mono.len() as i64, p.samples(), "sample count");
    let onsets = vidcut_av::beep::onsets(&mono, 0.1, 4800);
    // a1 10..40 (0.333..1.333 s) has its 1 s beep at output 0.667 s;
    // b1 45..75 (1.5..2.5 s) has its 2 s beep at output 1.0 + 0.5 = 1.5 s;
    // np 3..33 (0.1..1.1 s) starts at output 3.1667 s and beeps at 1.0 s -> 4.0667 s.
    let expect = [0.6667, 1.5, 4.0667];
    assert_eq!(onsets.len(), expect.len(), "beeps at {onsets:?}");
    for (got, want) in onsets.iter().zip(expect) {
        let t = *got as f64 / 48_000.0;
        assert!(
            (t - want).abs() < 0.001,
            "beep at {t:.4}, want {want:.4} (A/V offset > 1 ms)"
        );
    }
}

#[test]
fn pieces_at_other_speeds_walk_their_source() {
    // Decision D48: at speed 2 every other frame, at 0.5 each twice, at 0
    // one held, at -1 backward; a piece cut from inside its clip keeps its
    // clip's pace.
    let fps = Rational::new(30, 1).unwrap();
    let s = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        120,
    ));
    let paced = |source_frame, frames, speed, skip| {
        let mut p = v(&s, source_frame, frames);
        if let VideoPiece::Source { pace, .. } = &mut p {
            *pace = Pace::of(speed, skip);
        }
        p
    };
    let pieces = vec![
        paced(10, 5, 2.0, 0),
        paced(40, 4, 0.5, 0),
        paced(41, 3, 0.5, 1),
        paced(60, 3, 0.0, 0),
        paced(90, 4, -1.0, 0),
        v(&s, 20, 2),
    ];
    let p = plan(fps, 320, 180, pieces, vec![]);
    let dir = out_dir("speeds");
    let out = dir.path().join("speeds.mkv");
    render(&p, &lossless(), &out);
    let want: Vec<Option<u32>> = [
        10, 12, 14, 16, 18, 40, 40, 41, 41, 41, 42, 42, 60, 60, 60, 90, 89, 88, 87, 20, 21,
    ]
    .into_iter()
    .map(Some)
    .collect();
    assert_eq!(barcodes(&out), want);
}

#[test]
fn sound_at_other_speeds_keeps_its_time_and_pitch() {
    // Decision D48: the sound of the frames shown, stretched keeping its
    // pitch; backward, reversed.
    use vidcut_av::export::audio::AudioMix;
    use vidcut_av::fixture::Sound;
    const RATE: f64 = 48_000.0;
    let spec = |sound| {
        FixtureSpec::new(VideoCodec::H264, Container::Mov, 320, 180, (30, 1), 330)
            .with_audio(AudioCodec::PcmS16, 48_000, 2)
            .sound(sound)
    };
    let (beeps, tone) = (src(spec(Sound::Beep)), src(spec(Sound::Tone)));
    let paced = |s: &Src, from: f64, secs: f64, speed: f64| AudioPiece::Source {
        media: "m".into(),
        path: s.path.clone(),
        stream: 0,
        origin: s.origin,
        source_sample: (from * RATE).round() as i64,
        samples: (secs * RATE) as i64,
        gain_db: 0.0,
        fade: None,
        speed,
    };
    let left = |lane: Vec<AudioPiece>| -> Vec<f32> {
        let n = lane.iter().map(AudioPiece::samples).sum::<i64>() as usize;
        let mut mix = AudioMix::new(&[lane], 48_000, 2, 0);
        let both = mix.read(n).unwrap();
        both.as_chunks::<2>().0.iter().map(|f| f[0]).collect()
    };

    // From 1.5 s twice as fast, the beeps of 2 to 5 s; from 6.5 s at half
    // speed, the one of 7 s; back from 9.5 s, those of 9 and 8 s, each
    // heard from its end. A stretched one rises within half a hop of the
    // stretch's fragments (5 ms).
    let mono = left(vec![
        paced(&beeps, 1.5, 2.0, 2.0),
        paced(&beeps, 6.5, 2.0, 0.5),
        paced(&beeps, 9.5, 2.0, -1.0),
    ]);
    let onsets: Vec<f64> = vidcut_av::beep::onsets(&mono, 0.1, 4800)
        .iter()
        .map(|&i| i as f64 / RATE)
        .collect();
    let want = [0.25, 0.75, 1.25, 1.75, 3.0, 4.49, 5.49];
    assert_eq!(onsets.len(), want.len(), "beeps at {onsets:?}");
    for (got, want) in onsets.iter().zip(want) {
        assert!((got - want).abs() < 0.007, "beep at {got:.4}, want {want}");
    }

    // The 1 kHz tone stays 1 kHz at every speed.
    for speed in [2.0, 0.5, -1.0, -2.0] {
        let mono = left(vec![paced(&tone, 5.0, 1.0, speed)]);
        let rises = mono[4800..]
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!(
            (880..=920).contains(&rises),
            "{rises} cycles in 0.9 s at speed {speed}"
        );
    }
}

#[test]
fn pieces_of_one_file_read_on_seek_back_and_open_ahead() {
    // Export keeps a file open from piece to piece: a piece a little ahead
    // decodes on, one behind or far ahead seeks, and a piece of another file
    // was opened while the one before it encoded. Every way lands exactly.
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 300).with_audio(
            AudioCodec::PcmS16,
            48_000,
            2,
        ),
    );
    let b1 = src(
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::Flac,
            48_000,
            2,
        ),
    );
    let parts: [(&Src, i64, i64); 9] = [
        (&a1, 100, 30), // opens a1
        (&a1, 10, 30),  // behind: seeks
        (&a1, 45, 15),  // 5 frames ahead: decodes on
        (&a1, 59, 11),  // repeats the frame before
        (&a1, 0, 5),    // stands for 5 frames of black and silence
        (&a1, 200, 10), // far ahead: seeks
        (&b1, 20, 20),  // another file, opened ahead
        (&a1, 210, 10), // back to a1, opened ahead
        (&a1, 220, 10), // right after: reads on
    ];
    let mut pieces = Vec::new();
    let mut lane = Vec::new();
    let mut want: Vec<Option<u32>> = Vec::new();
    for (i, &(s, from, n)) in parts.iter().enumerate() {
        if i == 4 {
            pieces.push(VideoPiece::Black { frames: n });
            lane.push(AudioPiece::Silence {
                samples: fps.frame_to_sample(n, 48_000),
            });
            want.extend(std::iter::repeat_n(None, n as usize));
        } else {
            pieces.push(v(s, from, n));
            lane.push(a(s, fps, 48_000, from, n));
            want.extend((from..from + n).map(|f| Some(f as u32)));
        }
    }
    let p = plan(fps, 320, 180, pieces, vec![lane]);
    let dir = out_dir("reuse");
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);

    let codes = barcodes(&out);
    assert_eq!(codes.len(), want.len(), "frame count");
    let bad: Vec<_> = codes
        .iter()
        .zip(&want)
        .enumerate()
        .filter(|(_, (g, w))| g != w)
        .collect();
    assert!(bad.is_empty(), "frames differing from the plan: {bad:?}");

    // The sources beep at every whole second. Output times of the beeps:
    // a1 100..130 has 4 s at 0.6667; a1 10..40 has 1 s at 1.0 + 0.6667;
    // a1 59..70 (from 2.5 s) has 2 s at 2.5333; b1 20..40 (from 3.3667 s)
    // has 1 s at 3.7; a1 210..220 starts on 7 s, at 4.0333.
    let mono = audio_mono(&out);
    assert_eq!(mono.len() as i64, p.samples(), "sample count");
    let onsets = vidcut_av::beep::onsets(&mono, 0.1, 4800);
    let expect = [0.6667, 1.6667, 2.5333, 3.7, 4.0333];
    assert_eq!(onsets.len(), expect.len(), "beeps at {onsets:?}");
    for (got, want) in onsets.iter().zip(expect) {
        let t = *got as f64 / 48_000.0;
        assert!(
            (t - want).abs() < 0.001,
            "beep at {t:.4}, want {want:.4} (A/V offset > 1 ms)"
        );
    }
}

#[test]
fn rotated_sources_export_upright() {
    // Phone video stores pictures sideways with a rotation in the display
    // matrix. The fixtures store the barcode turned against it, so it reads
    // only where the rotation is applied.
    let fps = Rational::new(30, 1).unwrap();
    let mut pieces = Vec::new();
    let mut want: Vec<Option<u32>> = Vec::new();
    for (i, rotation) in [90u16, 180, 270].into_iter().enumerate() {
        let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 60)
            .rotation(rotation)
            .no_audio();
        let s = src(spec.clone());
        let info = vidcut_av::probe::probe(&s.path, None)
            .unwrap()
            .video
            .unwrap();
        assert_eq!(info.rotation, rotation, "probed rotation");
        assert_eq!((info.width, info.height), spec.coded_size(), "stored size");
        let from = 10 * i as i64;
        pieces.push(v(&s, from, 20));
        want.extend((from..from + 20).map(|f| Some(f as u32)));
        // Pool thumbnails show upright too, at the shown aspect.
        let input = vidcut_av::decode::Input::File(s.path.clone());
        let (images, _) = vidcut_av::thumbs::filmstrip(&input, 4, 192, 108, None).unwrap();
        for img in &images {
            assert_eq!((img.width, img.height), (192, 108), "rotation {rotation}");
            let w = img.width as usize;
            let code =
                vidcut_av::barcode::read_packed(&img.rgba, w, img.height as usize, w * 4, 4, 1);
            assert!(code.is_some(), "rotation {rotation}: thumbnail not upright");
        }
    }
    let p = plan(fps, 320, 180, pieces, vec![]);
    let dir = out_dir("rotated");
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);
    let codes = barcodes(&out);
    let bad: Vec<_> = codes
        .iter()
        .zip(&want)
        .enumerate()
        .filter(|(_, (g, w))| g != w)
        .collect();
    assert_eq!(codes.len(), want.len());
    assert!(bad.is_empty(), "frames differing from the plan: {bad:?}");
}

/// Mean difference between each luma row of the top eighth and the average
/// of its neighbors: near 0 for a progressive picture, large for woven
/// fields that differ.
fn combing(f: &vidcut_av::ffmpeg::frame::Video) -> f64 {
    let (w, h, stride) = (f.width() as usize, f.height() as usize / 8, f.stride(0));
    let y = f.data(0);
    let mut sum = 0.0;
    for r in 1..h - 1 {
        for x in 0..w {
            let (a, b, c) = (
                f64::from(y[(r - 1) * stride + x]),
                f64::from(y[r * stride + x]),
                f64::from(y[(r + 1) * stride + x]),
            );
            sum += (b - (a + c) / 2.0).abs();
        }
    }
    sum / ((h - 2) * w) as f64
}

#[test]
fn interlaced_sources_are_deinterlaced() {
    // Both fields of frame k show barcode k, and in the top eighth a block
    // moves 40 px each field: woven, it combs there. Deinterlaced (bwdif,
    // one frame per frame), it doesn't, and the frames stay exact.
    let fps = Rational::new(25, 1).unwrap();
    let mut pieces = Vec::new();
    let mut want: Vec<Option<u32>> = Vec::new();
    for top in [true, false] {
        let spec = FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (25, 1), 50)
            .interlaced(top)
            .no_audio();
        let s = src(spec);
        let info = vidcut_av::probe::probe(&s.path, None)
            .unwrap()
            .video
            .unwrap();
        assert_eq!(info.top_field_first, Some(top), "probed field order");
        pieces.push(v(&s, 5, 20));
        want.extend((5..25).map(Some));
    }
    let p = plan(fps, 320, 180, pieces, vec![]);
    let dir = out_dir("interlaced");
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);
    let mut r = VideoReader::open(&out).unwrap();
    let mut codes = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        let c = combing(&f);
        assert!(c < 10.0, "frame {} combs: {c:.1}", codes.len());
        codes.push(read_barcode(&f).unwrap());
    }
    assert_eq!(codes, want);
}

/// The darkest and brightest luma (as 10-bit codes) in the middle of the
/// barcode's top band: its black and white cells.
fn cell_levels(f: &vidcut_av::ffmpeg::frame::Video) -> (u16, u16) {
    use vidcut_av::ffmpeg::format::Pixel;
    let (w, h, stride) = (f.width() as usize, f.height() as usize, f.stride(0));
    let data = f.data(0);
    let ten = matches!(f.format(), Pixel::YUV420P10LE | Pixel::YUV422P10LE);
    let (mut lo, mut hi) = (u16::MAX, 0);
    for y in h / 4..h * 3 / 8 {
        for x in (w / 40..w).step_by(w / 20) {
            let v = if ten {
                u16::from_le_bytes([data[y * stride + 2 * x], data[y * stride + 2 * x + 1]])
            } else {
                u16::from(data[y * stride + x]) << 2
            };
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    (lo, hi)
}

#[test]
fn hdr_is_tone_mapped_for_sdr_and_kept_for_hdr() {
    use vidcut_core::model::Hdr;
    // The fixtures' barcode white is SDR reference white in HDR: 203 nits in
    // PQ, 75% in HLG. An SDR export tone-maps it to near white (it would be
    // gray unconverted, and PQ's barcode wouldn't read).
    let fps = Rational::new(30, 1).unwrap();
    let mut pieces = Vec::new();
    let mut want: Vec<Option<u32>> = Vec::new();
    for hdr in [Hdr::Pq, Hdr::Hlg] {
        let spec = FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 30)
            .hdr(hdr)
            .no_audio();
        let s = src(spec);
        let info = vidcut_av::probe::probe(&s.path, None)
            .unwrap()
            .video
            .unwrap();
        let transfer = if hdr == Hdr::Pq {
            "smpte2084"
        } else {
            "arib-std-b67"
        };
        assert_eq!(info.transfer, transfer);
        pieces.push(v(&s, 5, 10));
        want.extend((5..15).map(Some));
        // Pool thumbnails are tone-mapped too.
        let input = vidcut_av::decode::Input::File(s.path.clone());
        let (images, _) = vidcut_av::thumbs::filmstrip(&input, 2, 192, 108, None).unwrap();
        for img in &images {
            let w = img.width as usize;
            let code =
                vidcut_av::barcode::read_packed(&img.rgba, w, img.height as usize, w * 4, 4, 1);
            assert!(code.is_some(), "{hdr:?} thumbnail not tone-mapped");
        }
    }
    let p = plan(fps, 320, 180, pieces.clone(), vec![]);
    let dir = out_dir("hdr");
    let out = dir.path().join("sdr.mkv");
    render(&p, &lossless(), &out);
    let mut r = VideoReader::open(&out).unwrap();
    let mut codes = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        let (lo, hi) = cell_levels(&f);
        // 95% white (about 220 of 8-bit's 235); black stays black.
        assert!((850..=940).contains(&hi), "white {hi}");
        assert!(lo <= 80, "black {lo}");
        codes.push(read_barcode(&f).unwrap());
    }
    assert_eq!(codes, want);

    // HDR out (PQ, 10-bit HEVC): SDR white lands at the reference white, the
    // PQ source keeps its level, and the stream says PQ.
    let sdr =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 30).no_audio());
    pieces.insert(0, v(&sdr, 5, 10));
    let p = plan(fps, 320, 180, pieces, vec![]);
    let mut settings = lossless();
    let video = settings.video.as_mut().unwrap();
    video.codec = VCodec::Hevc;
    video.quality = Some(4);
    video.speed = Some("ultrafast".into());
    video.hdr = HdrMode::Pq;
    let out = dir.path().join("pq.mkv");
    render(&p, &settings, &out);
    let info = vidcut_av::probe::probe(&out, None).unwrap().video.unwrap();
    assert_eq!(info.transfer, "smpte2084");
    assert_eq!(info.pix_fmt, "yuv420p10le");
    let mut r = VideoReader::open(&out).unwrap();
    let mut n = 0;
    while let Some(f) = r.next_frame().unwrap() {
        let (lo, hi) = cell_levels(&f);
        let white = vidcut_av::fixture::PQ_WHITE;
        assert!(
            hi.abs_diff(white) <= 12,
            "frame {n}: white {hi}, want {white}"
        );
        assert!(lo <= 80, "black {lo}");
        n += 1;
    }
    assert_eq!(n, 30);
}

/// HDR colors are read with their own matrix, BT.2020. Read as BT.601,
/// saturated colors come out tens of levels off. PQ stripes exported to SDR
/// get the colors below, and kept in PQ they stay as they were.
#[test]
fn hdr_colors_keep_their_matrix() {
    use vidcut_av::fixture::Picture;
    use vidcut_core::model::Hdr;
    let fps = Rational::new(30, 1).unwrap();
    let colors = src(
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 640, 480, (30, 1), 10)
            .hdr(Hdr::Pq)
            .picture(Picture::Colors)
            .no_audio(),
    );
    let p = plan(fps, 640, 480, vec![v(&colors, 0, 10)], vec![]);
    let dir = out_dir("hdr-colors");
    // The middle half of each of the four stripes.
    let stripes = |path: &Path| -> Vec<[f64; 3]> {
        let f = frame_at(path, 5);
        (0..4)
            .map(|i| region_means(&f, (i * 160 + 40, i * 160 + 120)))
            .collect()
    };
    let codes = Picture::COLORS
        .map(|(y, cb, cr)| (u16::from(y) << 2, u16::from(cb) << 2, u16::from(cr) << 2));
    let sdr = dir.path().join("sdr.mkv");
    render(&p, &lossless(), &sdr);
    // The stripes tone-mapped into SDR BT.709, within rounding and zimg's
    // float approximations. Those differ by CPU: on a Mac (ARM) the third
    // stripe's luma is 1.9 low.
    const SDR: [[f64; 3]; 4] = [
        [62.5, 102.4, 239.8],
        [98.0, 203.9, 74.8],
        [155.9, 50.9, 37.1],
        [57.2, 187.5, 194.1],
    ];
    for (i, (got, want)) in stripes(&sdr).iter().zip(SDR).enumerate() {
        for c in 0..3 {
            assert!(
                (got[c] - want[c]).abs() <= 2.0,
                "SDR stripe {i}: {got:?}, not {want:?}"
            );
        }
    }
    let mut settings = lossless();
    let video = settings.video.as_mut().unwrap();
    video.codec = VCodec::Hevc;
    video.quality = Some(4);
    video.hdr = HdrMode::Pq;
    let pq = dir.path().join("pq.mkv");
    render(&p, &settings, &pq);
    for (i, (got, (y, cb, cr))) in stripes(&pq).iter().zip(codes).enumerate() {
        let want = [y, cb, cr].map(|c| f64::from(c) / 4.0);
        for c in 0..3 {
            assert!(
                (got[c] - want[c]).abs() <= 1.0,
                "PQ stripe {i}: {got:?}, not {want:?}"
            );
        }
    }
}

/// Settings for a target bitrate with `passes` (decision D45), on the CPU.
fn at_bitrate(codec: VCodec, bitrate: u64, passes: u8) -> ExportSettings {
    let mut s = lossless();
    s.audio = None;
    let v = s.video.as_mut().unwrap();
    v.codec = codec;
    v.quality = None;
    v.bitrate = Some(bitrate);
    v.passes = Some(passes);
    v.speed = None;
    v.bit_depth = Some(8);
    s
}

/// Decision D45: cancelling in either pass leaves no file, no partial file
/// and no statistics.
#[test]
fn cancelling_either_pass_leaves_nothing() {
    let fps = Rational::new(30, 1).unwrap();
    let a =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 90).no_audio());
    let p = plan(fps, 320, 180, vec![v(&a, 0, 90)], vec![]);
    let dir = out_dir("passes-cancel");
    for codec in [VCodec::H264] {
        for pass in [1, 2] {
            let out = dir.path().join("out.mkv");
            let cancel = AtomicBool::new(false);
            let mut stats_seen = false;
            let err = export::run(
                &p,
                &at_bitrate(codec, 200_000, 2),
                &out,
                &Availability::default(),
                &cancel,
                &mut |pr| {
                    stats_seen |= export::passes_path(&out).exists();
                    if pr.pass == pass && pr.frames == 10 {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                },
            )
            .unwrap_err();
            assert!(export::is_cancelled(&err), "{codec:?} pass {pass}: {err}");
            assert!(stats_seen, "{codec:?}: statistics written while it ran");
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                0,
                "{codec:?} pass {pass}: something left"
            );
        }
    }
}

/// An encoder that fails partway through stops the export with its error
/// and leaves no file: x264's second pass, whose frame-by-frame statistics
/// (its MB-tree file) are cut in half as it starts.
#[test]
fn an_encoder_failing_partway_leaves_nothing() {
    let fps = Rational::new(30, 1).unwrap();
    let a =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 90).no_audio());
    let p = plan(fps, 320, 180, vec![v(&a, 0, 90)], vec![]);
    let dir = out_dir("encoder-fails");
    let out = dir.path().join("out.mkv");
    let mut cut = false;
    let err = export::run(
        &p,
        &at_bitrate(VCodec::H264, 200_000, 2),
        &out,
        &Availability::default(),
        &AtomicBool::new(false),
        &mut |pr| {
            if pr.pass == 2 && pr.frames == 1 {
                let stats = std::fs::read_dir(export::passes_path(&out)).unwrap();
                let mbtree = stats
                    .map(|e| e.unwrap().path())
                    .find(|f| f.extension().is_some_and(|x| x == "mbtree"))
                    .expect("x264's MB-tree statistics");
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(mbtree)
                    .unwrap();
                file.set_len(file.metadata().unwrap().len() / 2).unwrap();
                cut = true;
            }
        },
    )
    .unwrap_err();
    assert!(cut, "the second pass started");
    assert!(
        matches!(
            err,
            vidcut_av::Error::Ffmpeg {
                source: vidcut_av::ffmpeg::Error::External,
                ..
            }
        ),
        "x264's own error: {err}"
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "something left"
    );
}

#[test]
fn audio_after_a_lost_packet_keeps_its_time() {
    // An AAC packet before the 2 s beep is gone (damaged files lose some,
    // and the decoder skips packets it can't read): the beep stays at 2 s,
    // and the gap is silent.
    let fps = Rational::new(30, 1).unwrap();
    let s = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::Aac,
            48_000,
            2,
        ),
    );
    let info = vidcut_av::probe::probe(&s.path, None).unwrap();
    let audio = info.audio[0].index;
    // AAC packets are 1024 samples: number 80 starts at 1.7 s.
    let dir = out_dir("lost");
    let lossy = dir.path().join("lossy.mkv");
    fixture::spoil(&s.path, &lossy, audio, 80, Spoil::Drop).unwrap();
    let gapped = Src {
        path: lossy,
        origin: s.origin,
    };
    let p = plan(
        fps,
        320,
        180,
        vec![v(&gapped, 0, 90)],
        vec![vec![a(&gapped, fps, 48_000, 0, 90)]],
    );
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);
    let mono = audio_mono(&out);
    assert_eq!(mono.len() as i64, p.samples(), "sample count");
    let onsets = vidcut_av::beep::onsets(&mono, 0.1, 4800);
    let expect = [0.0, 1.0, 2.0];
    assert_eq!(onsets.len(), expect.len(), "beeps at {onsets:?}");
    for (got, want) in onsets.iter().zip(expect) {
        let t = *got as f64 / 48_000.0;
        assert!((t - want).abs() < 0.001, "beep at {t:.4}, want {want:.4}");
    }
    // The hole (the packet after the AAC priming one, 79 × 1024 samples in)
    // is digital silence. A loud sample here fails; the length check above
    // would not.
    let hole = &mono[79 * 1024..80 * 1024];
    assert!(hole.iter().all(|&x| x == 0.0), "the hole is silent");
}

#[test]
fn audio_that_starts_late_keeps_its_time() {
    // The first FLAC packet is gone, so the audio starts a packet after the
    // picture (as some recordings do): silence until it starts, and what
    // follows stays on time.
    let fps = Rational::new(30, 1).unwrap();
    let s = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::Flac,
            48_000,
            2,
        ),
    );
    let audio = vidcut_av::probe::probe(&s.path, None).unwrap().audio[0].index;
    let dir = out_dir("late");
    let lossy = dir.path().join("late.mkv");
    fixture::spoil(&s.path, &lossy, audio, 0, Spoil::Drop).unwrap();
    let first = vidcut_av::probe::probe(&lossy, None).unwrap().audio[0]
        .first_secs
        .unwrap();
    assert!(first > 0.01, "starts at {first}");
    let late = Src {
        path: lossy,
        origin: s.origin,
    };
    let p = plan(
        fps,
        320,
        180,
        vec![v(&late, 0, 90)],
        vec![vec![a(&late, fps, 48_000, 0, 90)]],
    );
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);
    let mono = audio_mono(&out);
    let lead = (first * 48_000.0).floor() as usize;
    assert!(
        mono[..lead].iter().all(|&x| x == 0.0),
        "silent for the first {first} s"
    );
    // The 0 s beep was in the lost packet.
    let onsets = vidcut_av::beep::onsets(&mono, 0.1, 4800);
    let expect = [1.0, 2.0];
    assert_eq!(onsets.len(), expect.len(), "beeps at {onsets:?}");
    for (got, want) in onsets.iter().zip(expect) {
        let t = *got as f64 / 48_000.0;
        assert!((t - want).abs() < 0.001, "beep at {t:.4}, want {want:.4}");
    }
}

#[test]
fn a_damaged_video_packet_is_skipped() {
    // A ProRes frame whose data is zeros: the decoder rejects it, and the
    // export goes on as players do. The next frame shows in its place (the
    // sampling rule).
    let fps = Rational::new(30, 1).unwrap();
    let s = src(FixtureSpec::new(
        VideoCodec::ProRes422Hq,
        Container::Mov,
        320,
        180,
        (30, 1),
        30,
    ));
    let video = vidcut_av::probe::probe(&s.path, None)
        .unwrap()
        .video
        .unwrap()
        .index;
    let dir = out_dir("damaged");
    let damaged = dir.path().join("damaged.mov");
    fixture::spoil(&s.path, &damaged, video, 10, Spoil::Zero).unwrap();
    let d = Src {
        path: damaged,
        origin: s.origin,
    };
    let p = plan(fps, 320, 180, vec![v(&d, 0, 30)], vec![]);
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);
    let mut want: Vec<Option<u32>> = (0..30).map(Some).collect();
    want[10] = Some(11);
    assert_eq!(barcodes(&out), want);

    // H.264 with a damaged packet in its second second (keyframes every
    // second, B-frames): what refers to it shows whatever the decoder makes
    // of it, and the seconds before and after are exact.
    let s = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mkv,
        320,
        180,
        (30, 1),
        90,
    ));
    let video = vidcut_av::probe::probe(&s.path, None)
        .unwrap()
        .video
        .unwrap()
        .index;
    let damaged = dir.path().join("damaged.mkv");
    fixture::spoil(&s.path, &damaged, video, 40, Spoil::Zero).unwrap();
    let d = Src {
        path: damaged,
        origin: s.origin,
    };
    let p = plan(fps, 320, 180, vec![v(&d, 0, 90)], vec![]);
    let out = dir.path().join("out-h264.mkv");
    render(&p, &lossless(), &out);
    let codes = barcodes(&out);
    assert_eq!(codes.len(), 90);
    let exact = |r: std::ops::Range<u32>| r.map(Some).collect::<Vec<_>>();
    assert_eq!(codes[..30], exact(0..30));
    assert_eq!(codes[60..], exact(60..90));
}

#[test]
fn other_rates_are_resampled_by_nearest_frame() {
    // A 25 fps source on a 30 fps timeline: 5 source frames per 6 output frames.
    let fps = Rational::new(30, 1).unwrap();
    let s25 = src(FixtureSpec::new(
        VideoCodec::ProRes422Hq,
        Container::Mov,
        320,
        180,
        (25, 1),
        50,
    )
    .no_audio());
    let p = plan(fps, 320, 180, vec![v(&s25, 0, 12)], vec![]);
    let dir = out_dir("rate");
    let out = dir.path().join("out.mkv");
    let mut settings = lossless();
    settings.audio = None;
    render(&p, &settings, &out);
    let codes: Vec<u32> = barcodes(&out).into_iter().map(Option::unwrap).collect();
    assert_eq!(codes, vec![0, 1, 2, 3, 3, 4, 5, 6, 7, 8, 8, 9]);
}

/// The encoder places its own keyframes: a decoded frame's picture type
/// (the source's keyframes here) would force itself on it.
#[test]
fn the_encoder_chooses_its_own_keyframes() {
    let fps = Rational::new(30, 1).unwrap();
    // Keyframes every 30 frames.
    let s = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        150,
    ));
    let p = plan(fps, 320, 180, vec![v(&s, 0, 150)], vec![]);
    let dir = out_dir("keyframes");
    let out = dir.path().join("out.mkv");
    let mut settings = lossless();
    settings.audio = None;
    render(&p, &settings, &out);
    let mut input = vidcut_av::ffmpeg::format::input(&out).unwrap();
    let video = input
        .streams()
        .best(vidcut_av::ffmpeg::media::Type::Video)
        .unwrap()
        .index();
    let keys: Vec<usize> = input
        .packets()
        .filter(|(s, _)| s.index() == video)
        .enumerate()
        .filter(|(_, (_, p))| p.is_key())
        .map(|(n, _)| n)
        .collect();
    // x264's own interval (250 frames, ultrafast: no scene cuts) is longer.
    assert_eq!(keys, [0]);
}

#[test]
fn scaling_padding_and_presets() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let p = plan(
        fps,
        320,
        180,
        vec![v(&a1, 0, 60)],
        vec![vec![a(&a1, fps, 48_000, 0, 60)]],
    );
    let dir = out_dir("presets");
    for preset in vidcut_core::export::builtin_presets() {
        let mut settings = preset.settings.clone();
        if let Some(v) = settings.video.as_mut() {
            v.encoder = EncoderChoice::Cpu;
            // Keep the test quick: small frames even for the 4K preset.
            if v.width.is_some() {
                v.width = Some((v.width.unwrap() / 4) & !1);
                v.height = Some((v.height.unwrap() / 4) & !1);
            }
            if v.codec == VCodec::Av1 {
                v.speed = Some("12".into());
            }
        }
        let mut p = p.clone();
        if let Some(a) = &settings.audio
            && a.codec == ACodec::Opus
        {
            p.sample_rate = 48_000;
        }
        let out = dir.path().join(format!(
            "{}.{}",
            preset.name,
            settings.container.extension()
        ));
        let report = render(&p, &settings, &out);
        let info =
            vidcut_av::probe::probe(&out, None).unwrap_or_else(|e| panic!("{}: {e}", preset.name));
        if let Some(vs) = &settings.video {
            let vi = info
                .video
                .as_ref()
                .unwrap_or_else(|| panic!("{}: no video", preset.name));
            let (w, h) = (vs.width.unwrap_or(320), vs.height.unwrap_or(180));
            assert_eq!((vi.width, vi.height), (w, h), "{}", preset.name);
            // Decodable, and the barcode survives scaling and lossy coding.
            let codes = barcodes(&out);
            assert_eq!(codes.len(), 60, "{}: frames", preset.name);
            if w * 180 == h * 320 {
                assert_eq!(codes[10], Some(10), "{}: barcode", preset.name);
            }
        } else {
            assert!(info.video.is_none(), "{}", preset.name);
        }
        assert_eq!(info.audio.len(), 1, "{}: audio", preset.name);
        assert!(report.audio_encoder.is_some());
    }
}

#[test]
fn normalizing_hot_lanes_keeps_true_peaks_under_the_ceiling() {
    // Two lanes of the same beeps, each raised to full scale: their mix
    // peaks near +6 dBFS. Short beeps are loud for their loudness, so
    // reaching -5 LUFS needs the true-peak cap: the gain stops where peaks
    // reach -1 dBTP, in the report and in the file as measured again.
    let fps = Rational::new(30, 1).unwrap();
    let s = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 300).with_audio(
            AudioCodec::Flac,
            48_000,
            2,
        ),
    );
    let mut hot = a(&s, fps, 48_000, 0, 300);
    if let AudioPiece::Source { gain_db, .. } = &mut hot {
        *gain_db = 20.0 * 2f64.log10();
    }
    let p = plan(
        fps,
        320,
        180,
        vec![v(&s, 0, 300)],
        vec![vec![hot.clone()], vec![hot]],
    );
    let mut settings = lossless();
    settings.audio.as_mut().unwrap().loudness = Some(-5.0);
    let dir = out_dir("hot");
    let out = dir.path().join("out.mkv");
    let n = render(&p, &settings, &out).loudness.expect("normalized");
    let before = n.true_peak.unwrap() - n.gain_db;
    assert!(before > 5.5, "the mix peaks above full scale: {n:?}");
    assert!(n.true_peak.unwrap() <= -1.0 + 1e-9, "{n:?}");
    assert!(n.loudness.unwrap() < -6.0, "the cap held the gain: {n:?}");
    let o = Src {
        path: out,
        origin: 0.0,
    };
    let again = vidcut_av::analysis::loudness(&plan(
        fps,
        320,
        180,
        vec![v(&o, 0, 300)],
        vec![vec![a(&o, fps, 48_000, 0, 300)]],
    ))
    .unwrap();
    assert!(again.true_peak <= -0.95, "{again:?} after {n:?}");
    assert!(
        (again.integrated - n.loudness.unwrap()).abs() < 0.3,
        "{again:?} after {n:?}"
    );
}

/// A piece of image `path` (a still, or a sequence's first file).
fn pictures(path: &Path, image: Image, source_frame: i64, frames: i64) -> VideoPiece {
    VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: path.to_path_buf(),
        origin: 0.0,
        source_frame,
        frames,
        image: Some(image),
    }
}

#[test]
fn sequences_play_their_pictures_at_their_rate() {
    // Numbered images at 30 fps on a 30 fps timeline: picture for frame.
    let fps = Rational::new(30, 1).unwrap();
    let files = fixture::ensure_images(&ImageSpec::sequence(1000, 60, 4, 320, 180)).unwrap();
    let sequence = Image::Sequence {
        pattern: files[0].with_file_name("img_%04d.png"),
        first: 1000,
        count: 60,
        rate: fps,
    };
    let p = plan(
        fps,
        320,
        180,
        vec![pictures(&files[0], sequence, 5, 20)],
        vec![],
    );
    let dir = out_dir("sequences");
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);
    let want: Vec<Option<u32>> = (5..25).map(Some).collect();
    assert_eq!(barcodes(&out), want);
}

/// Frame `k` of `path`.
fn frame_at(path: &Path, k: usize) -> vidcut_av::ffmpeg::frame::Video {
    let mut r = VideoReader::open(path).unwrap();
    (0..=k)
        .map(|_| r.next_frame().unwrap().unwrap())
        .last()
        .unwrap()
}

/// The mean (Y, Cb, Cr) in 8-bit units of the columns from `a` to `b` of
/// `f`, in its middle rows.
fn region_means(f: &vidcut_av::ffmpeg::frame::Video, (a, b): (usize, usize)) -> [f64; 3] {
    use vidcut_av::ffmpeg::format::Pixel;
    let ten = f.format() == Pixel::YUV420P10LE;
    let mean = |p: usize| {
        let sub = if p == 0 { 1 } else { 2 };
        let (stride, data) = (f.stride(p), f.data(p));
        let h = f.height() as usize / sub;
        let (mut sum, mut n) = (0.0, 0.0);
        for y in h / 4..h * 3 / 4 {
            for x in a / sub..b / sub {
                sum += if ten {
                    f64::from(u16::from_le_bytes([
                        data[y * stride + 2 * x],
                        data[y * stride + 2 * x + 1],
                    ])) / 4.0
                } else {
                    f64::from(data[y * stride + x])
                };
                n += 1.0;
            }
        }
        sum / n
    };
    [mean(0), mean(1), mean(2)]
}
