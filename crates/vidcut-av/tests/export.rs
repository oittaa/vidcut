//! Export engine tests: render real fixtures and verify the output frame by
//! frame (barcodes) and sample by sample (beep onsets).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::export::{
    self,
    encode::{Availability, EncoderStatus},
};
use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, ImageSpec, Spoil, VideoCodec};
use vidcut_core::export::{
    AudioCodec as ACodec, AudioSettings, Container as OutContainer, EncoderChoice, ExportSettings,
    HdrMode, VideoCodec as VCodec, VideoSettings,
};
use vidcut_core::render::{AudioPiece, Image, RenderPlan, VideoPiece};
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
    let info = vidcut_av::probe::probe(&path).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    Src { path, origin }
}

fn v(s: &Src, source_frame: i64, frames: i64) -> VideoPiece {
    VideoPiece::Source {
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
        media: "m".into(),
        path: s.path.clone(),
        stream: 0,
        origin: s.origin,
        source_sample: fps.frame_to_sample(source_frame, rate),
        samples: fps.frame_to_sample(frames, rate),
        gain_db: 0.0,
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
        let info = vidcut_av::probe::probe(&s.path).unwrap().video.unwrap();
        assert_eq!(info.rotation, rotation, "probed rotation");
        assert_eq!((info.width, info.height), spec.coded_size(), "stored size");
        let from = 10 * i as i64;
        pieces.push(v(&s, from, 20));
        want.extend((from..from + 20).map(|f| Some(f as u32)));
        // Pool thumbnails show upright too, at the shown aspect.
        let input = vidcut_av::decode::Input::File(s.path.clone());
        let (images, _) = vidcut_av::thumbs::filmstrip(&input, 4, 192, 108).unwrap();
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
        let info = vidcut_av::probe::probe(&s.path).unwrap().video.unwrap();
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
        let info = vidcut_av::probe::probe(&s.path).unwrap().video.unwrap();
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
        let (images, _) = vidcut_av::thumbs::filmstrip(&input, 2, 192, 108).unwrap();
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
        eprintln!(
            "SDR frame {}: black {lo}, white {hi} (10-bit codes)",
            codes.len()
        );
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
    let info = vidcut_av::probe::probe(&out).unwrap().video.unwrap();
    assert_eq!(info.transfer, "smpte2084");
    assert_eq!(info.pix_fmt, "yuv420p10le");
    let mut r = VideoReader::open(&out).unwrap();
    let mut n = 0;
    while let Some(f) = r.next_frame().unwrap() {
        let (lo, hi) = cell_levels(&f);
        eprintln!("PQ frame {n}: black {lo}, white {hi}");
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

/// (Y, Cb, Cr) in 8-bit video range BT.709 of a BT.2020 PQ color given as
/// 10-bit video-range codes, as the export converts HDR into SDR: linear
/// light with the reference white (203 nits) at 1.0, BT.709 primaries, the
/// mobius curve (knee 0.8, peak 1000 nits) on the brightest channel,
/// negative channels black, a 2.4 power.
fn pq_to_sdr((y, cb, cr): (u16, u16, u16)) -> [f64; 3] {
    let y = (f64::from(y) - 64.0) / 876.0;
    let (cb, cr) = (
        (f64::from(cb) - 512.0) / 896.0,
        (f64::from(cr) - 512.0) / 896.0,
    );
    let (kr, kb) = (0.2627, 0.0593);
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
    let linear = |v: f64| {
        let p = v.max(0.0).powf(1.0 / 78.84375);
        let l = ((p - 0.8359375).max(0.0) / (18.8515625 - 18.6875 * p)).powf(1.0 / 0.1593017578125);
        l * 10000.0 / 203.0
    };
    let lin = [linear(r), linear(g), linear(b)];
    const TO_709: [[f64; 3]; 3] = [
        [1.660491, -0.587641, -0.072850],
        [-0.124550, 1.132900, -0.008349],
        [-0.018151, -0.100579, 1.118730],
    ];
    let c = TO_709.map(|row| row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]);
    let (j, peak) = (0.8, 4.9261);
    let a = -j * j * (peak - 1.0) / (j * j - 2.0 * j + peak);
    let b = (j * j - 2.0 * j * peak + peak) / (peak - 1.0);
    let scale = (b * b + 2.0 * b * j + j * j) / (b - a);
    let sig = c[0].max(c[1]).max(c[2]).max(1e-6);
    let mapped = if sig <= j {
        sig
    } else {
        scale * (sig + a) / (sig + b)
    };
    let v = c.map(|c| (c * mapped / sig).max(0.0).powf(1.0 / 2.4));
    let y = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
    [
        16.0 + 219.0 * y,
        128.0 + 224.0 * (v[2] - y) / 1.8556,
        128.0 + 224.0 * (v[0] - y) / 1.5748,
    ]
}

/// HDR colors are read with their own matrix (BT.2020): `scale`, which
/// only scales them before `zscale` converts, is told it, or swscale took
/// them to BT.601 and saturated colors came out tens of levels off. PQ
/// stripes into SDR come out as the math says, and kept in PQ they stay as
/// they were.
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
    // Within rounding and zimg's float approximations.
    for (i, (got, color)) in stripes(&sdr).iter().zip(codes).enumerate() {
        let want = pq_to_sdr(color);
        for c in 0..3 {
            assert!(
                (got[c] - want[c]).abs() <= 1.5,
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
    // Frames for NVENC (NV12, P010) are the planar ones interleaved: zimg
    // writes the YUV, not swscale.
    use vidcut_av::export::video::VideoFrames;
    use vidcut_av::ffmpeg::format::Pixel;
    let sdr_source = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 480, (30, 1), 10)
            .picture(Picture::Colors)
            .no_audio(),
    );
    for (source, target, planar, semi) in [
        (&colors, None, Pixel::YUV420P, Pixel::NV12),
        (
            &sdr_source,
            Some(Hdr::Pq),
            Pixel::YUV420P10LE,
            Pixel::P010LE,
        ),
    ] {
        let first = |pixel| {
            VideoFrames::new(&[v(source, 0, 1)], fps, 640, 480, pixel, target)
                .next_frame()
                .unwrap()
                .unwrap()
        };
        let (a, b) = (first(planar), first(semi));
        // Samples of a plane, 10-bit ones shifted to the low bits.
        let samples = |f: &vidcut_av::ffmpeg::frame::Video, p: usize, step: usize| {
            let (w, h) = (f.plane_width(p) as usize, f.plane_height(p) as usize);
            let (data, stride) = (f.data(p), f.stride(p));
            let bytes = if matches!(f.format(), Pixel::YUV420P | Pixel::NV12) {
                1
            } else {
                2
            };
            let shift = if f.format() == Pixel::P010LE { 6 } else { 0 };
            let mut out = Vec::new();
            for y in 0..h {
                for x in 0..w * step {
                    let at = y * stride + x * bytes;
                    let v = if bytes == 1 {
                        u16::from(data[at])
                    } else {
                        u16::from_le_bytes([data[at], data[at + 1]])
                    };
                    out.push(v >> shift);
                }
            }
            out
        };
        assert_eq!(samples(&a, 0, 1), samples(&b, 0, 1), "{semi:?}: luma");
        let (cb, cr) = (samples(&a, 1, 1), samples(&a, 2, 1));
        let interleaved: Vec<u16> = cb.iter().zip(&cr).flat_map(|(b, r)| [*b, *r]).collect();
        assert_eq!(interleaved, samples(&b, 1, 2), "{semi:?}: chroma");
    }
}

/// Settings that leave the dynamic range and depth to the export
/// (decision D44): `video.hdr = auto`, no bit depth.
fn auto_color(codec: VCodec, encoder: EncoderChoice) -> ExportSettings {
    let mut s = lossless();
    s.audio = None;
    let v = s.video.as_mut().unwrap();
    v.codec = codec;
    v.encoder = encoder;
    v.hdr = HdrMode::Auto;
    v.bit_depth = None;
    if codec != VCodec::H264 {
        v.quality = Some(4);
    }
    s
}

/// Decision D44: the engine, given `auto`, reads what the files are and
/// chooses from the picture it shows.
#[test]
fn auto_color_follows_the_files() {
    use vidcut_core::model::Hdr;
    let fps = Rational::new(30, 1).unwrap();
    let hevc10 = |frames| {
        FixtureSpec::new(
            VideoCodec::Hevc10,
            Container::Mkv,
            320,
            180,
            (30, 1),
            frames,
        )
    };
    let pq = src(hevc10(30).hdr(Hdr::Pq).no_audio());
    let deep = src(hevc10(30).no_audio());
    let sdr =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 30).no_audio());
    let dir = out_dir("auto-color");
    let export = |name: &str, pieces: Vec<VideoPiece>, settings: &ExportSettings| {
        let out = dir.path().join(name);
        let report = render(&plan(fps, 320, 180, pieces, vec![]), settings, &out);
        let info = vidcut_av::probe::probe(&out).unwrap().video.unwrap();
        (report.color.expect("chosen"), info)
    };
    // Mostly PQ into HEVC: PQ in 10 bits, every frame where it belongs.
    let mostly_pq = vec![v(&pq, 5, 20), v(&sdr, 5, 10)];
    let (c, info) = export(
        "pq.mkv",
        mostly_pq.clone(),
        &auto_color(VCodec::Hevc, EncoderChoice::Cpu),
    );
    assert_eq!((c.hdr, c.bit_depth, c.picture.pq), (HdrMode::Pq, 10, 20));
    assert_eq!(
        (info.transfer.as_str(), info.pix_fmt.as_str()),
        ("smpte2084", "yuv420p10le")
    );
    // PQ's barcodes don't read, but SDR white lands at the reference white
    // in all of it, the SDR part too.
    let mut r = VideoReader::open(&dir.path().join("pq.mkv")).unwrap();
    let mut n = 0;
    while let Some(f) = r.next_frame().unwrap() {
        let (_, hi) = cell_levels(&f);
        assert!(
            hi.abs_diff(vidcut_av::fixture::PQ_WHITE) <= 12,
            "frame {n}: white {hi}"
        );
        n += 1;
    }
    assert_eq!(n, 30);
    // The same into H.264: tone-mapped SDR in 8 bits.
    let (c, info) = export(
        "sdr.mkv",
        mostly_pq,
        &auto_color(VCodec::H264, EncoderChoice::Cpu),
    );
    assert_eq!((c.hdr, c.bit_depth, c.tone_mapped), (HdrMode::Sdr, 8, 20));
    assert!(c.worth_a_note());
    assert_ne!(info.transfer, "smpte2084");
    assert_eq!(info.pix_fmt, "yuv420p");
    // Mostly SDR: SDR, the PQ part tone-mapped.
    let (c, _) = export(
        "mixed.mkv",
        vec![v(&pq, 0, 10), v(&sdr, 0, 20)],
        &auto_color(VCodec::Hevc, EncoderChoice::Cpu),
    );
    assert_eq!((c.hdr, c.bit_depth, c.tone_mapped), (HdrMode::Sdr, 8, 10));
    // 10-bit SDR stays 10-bit in HEVC, and becomes 8 in H.264.
    let (c, info) = export(
        "deep.mkv",
        vec![v(&deep, 0, 30)],
        &auto_color(VCodec::Hevc, EncoderChoice::Cpu),
    );
    assert_eq!((c.hdr, c.bit_depth), (HdrMode::Sdr, 10));
    assert_eq!(info.pix_fmt, "yuv420p10le");
    assert_ne!(info.transfer, "smpte2084");
    let (c, info) = export(
        "deep8.mkv",
        vec![v(&deep, 0, 30)],
        &auto_color(VCodec::H264, EncoderChoice::Cpu),
    );
    assert_eq!(c.bit_depth, 8);
    assert_eq!(info.pix_fmt, "yuv420p");
    // Explicit values win, and then nothing is chosen.
    let mut settings = auto_color(VCodec::Hevc, EncoderChoice::Cpu);
    settings.video.as_mut().unwrap().hdr = HdrMode::Sdr;
    settings.video.as_mut().unwrap().bit_depth = Some(8);
    let out = dir.path().join("asked.mkv");
    let report = render(
        &plan(fps, 320, 180, vec![v(&pq, 0, 30)], vec![]),
        &settings,
        &out,
    );
    assert!(report.color.is_none());
    let info = vidcut_av::probe::probe(&out).unwrap().video.unwrap();
    assert_eq!(info.pix_fmt, "yuv420p");
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

/// Bits per second of a file `seconds` long.
fn bitrate_of(path: &Path, seconds: f64) -> f64 {
    std::fs::metadata(path).unwrap().len() as f64 * 8.0 / seconds
}

/// Decision D45: two passes come closer to a target bitrate than one in
/// x264, x265 and VP9 (statistics in a file, in x265's parameters, and in
/// memory), every frame where it belongs; progress counts both passes,
/// and the statistics are gone afterwards.
#[test]
fn two_passes_reach_the_bitrate_and_keep_every_frame() {
    let fps = Rational::new(30, 1).unwrap();
    // Barcodes, easy to compress, around noise, which is hard: one pass
    // finds out late what the noise costs, two know it from the start.
    let barcode =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 300).no_audio());
    let noise = src(
        FixtureSpec::new(VideoCodec::Ffv1, Container::Mkv, 320, 180, (30, 1), 150)
            .picture(vidcut_av::fixture::Picture::Noise)
            .no_audio(),
    );
    let p = plan(
        fps,
        320,
        180,
        vec![
            v(&barcode, 0, 150),
            v(&noise, 0, 150),
            v(&barcode, 150, 150),
        ],
        vec![],
    );
    let want: Vec<Option<u32>> = (0..150).map(Some).collect();
    let seconds = 450.0 / 30.0;
    let dir = out_dir("passes");
    // One pass overshoots by 90% (x264) to 500% (VP9) here; two within 7%.
    let target = 800_000;
    for codec in [VCodec::H264, VCodec::Hevc, VCodec::Vp9] {
        let mut errors = Vec::new();
        for passes in [1, 2] {
            let out = dir.path().join(format!("{codec:?}-{passes}.mkv"));
            let mut seen = Vec::new();
            let report = export::run(
                &p,
                &at_bitrate(codec, target, passes),
                &out,
                &Availability::default(),
                &AtomicBool::new(false),
                &mut |pr| seen.push((pr.pass, pr.passes, pr.frames)),
            )
            .unwrap_or_else(|e| panic!("{codec:?} in {passes}: {e}"));
            assert_eq!(report.passes, Some(passes), "{codec:?}");
            // Every frame of every pass, in order.
            let runs = passes;
            let expect: Vec<(u8, u8, i64)> = (1..=runs)
                .flat_map(|pass| (1..=450).map(move |f| (pass, runs, f)))
                .collect();
            assert_eq!(seen, expect, "{codec:?} in {passes}: progress");
            assert!(
                !export::passes_path(&out).exists(),
                "{codec:?}: statistics left"
            );
            let got = bitrate_of(&out, seconds);
            let error = got / target as f64 - 1.0;
            eprintln!(
                "{codec:?} in {passes} pass(es): {:.0} kb/s for {} ({:+.1}%)",
                got / 1000.0,
                target / 1000,
                error * 100.0
            );
            errors.push(error.abs());
            if passes == 2 {
                let codes = barcodes(&out);
                assert_eq!(codes.len(), 450, "{codec:?}");
                assert_eq!(codes[..150], want[..], "{codec:?}: barcodes");
                assert_eq!(
                    codes[300..],
                    (150..300).map(Some).collect::<Vec<_>>()[..],
                    "{codec:?}: barcodes"
                );
            }
        }
        assert!(
            errors[1] <= errors[0].max(0.02),
            "{codec:?}: two passes off by {:.1}%, one by {:.1}%",
            errors[1] * 100.0,
            errors[0] * 100.0
        );
        assert!(
            errors[1] < 0.1,
            "{codec:?}: two passes off by {:.1}%",
            errors[1] * 100.0
        );
    }
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
    for codec in [VCodec::H264, VCodec::Hevc, VCodec::Vp9] {
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

#[test]
fn a_failed_rename_leaves_no_part_file() {
    // The finished file can't take the output's name (a folder is there):
    // the export fails and removes its `.part`.
    let fps = Rational::new(30, 1).unwrap();
    let s =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 30).no_audio());
    let p = plan(fps, 320, 180, vec![v(&s, 0, 10)], vec![]);
    let dir = out_dir("rename");
    let out = dir.path().join("taken.mkv");
    std::fs::create_dir_all(out.join("inside")).unwrap();
    // An encode that fails first also deletes `.part`. The frame count and
    // the error both have to say the file was finished and the rename failed.
    let mut encoded = 0;
    let err = export::run(
        &p,
        &lossless(),
        &out,
        &Availability::default(),
        &AtomicBool::new(false),
        &mut |pr| {
            encoded = pr.frames;
            assert_eq!(pr.total, p.frames);
        },
    )
    .expect_err("the rename can't succeed");
    assert_eq!(encoded, p.frames, "the encode ran to the end");
    let vidcut_av::Error::Io(io) = err else {
        panic!("the encode finished and the rename failed: {err}");
    };
    // Windows says it is denied.
    let refused = if cfg!(windows) {
        std::io::ErrorKind::PermissionDenied
    } else {
        std::io::ErrorKind::IsADirectory
    };
    assert_eq!(io.kind(), refused, "{io}");
    assert!(
        !export::partial_path(&out).exists(),
        "the finished .part is gone"
    );
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        left,
        vec![std::ffi::OsString::from("taken.mkv")],
        "only the folder"
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
    let info = vidcut_av::probe::probe(&s.path).unwrap();
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
fn a_rejected_audio_packet_leaves_silence() {
    // The same AAC packet, with its bytes replaced by zeros. The decoder
    // rejects it (invalid data) instead of concealing a tone, and the hole
    // is silence. A loud glitch there fails this, not only a short file.
    let fps = Rational::new(30, 1).unwrap();
    let s = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::Aac,
            48_000,
            2,
        ),
    );
    let info = vidcut_av::probe::probe(&s.path).unwrap();
    let audio = info.audio[0].index;
    let dir = out_dir("rejected");
    let damaged = dir.path().join("rejected.mkv");
    fixture::spoil(&s.path, &damaged, audio, 80, Spoil::Zero).unwrap();
    let gapped = Src {
        path: damaged,
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
    let hole = &mono[79 * 1024..80 * 1024];
    let peak = hole.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(
        hole.iter().all(|&x| x == 0.0),
        "the hole is silent, peak {peak}"
    );
}

/// Copies `src` to `out` with the audio stream's extradata replaced.
fn with_audio_extradata(src: &Path, out: &Path, extra: &[u8]) {
    use vidcut_av::ffmpeg::{self, codec, encoder, format, media};
    vidcut_av::init();
    let mut input = format::input(src).unwrap();
    let mut output = format::output(out).unwrap();
    for s in input.streams() {
        let mut o = output.add_stream(encoder::find(codec::Id::None)).unwrap();
        o.set_parameters(s.parameters());
        let audio = s.parameters().medium() == media::Type::Audio;
        let mut params = o.parameters();
        unsafe {
            let par = params.as_mut_ptr();
            (*par).codec_tag = 0;
            if audio {
                ffmpeg::ffi::av_freep(std::ptr::addr_of_mut!((*par).extradata).cast());
                let pad = ffmpeg::ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize;
                let buf = ffmpeg::ffi::av_mallocz(extra.len() + pad).cast::<u8>();
                assert!(!buf.is_null(), "allocating extradata");
                std::ptr::copy_nonoverlapping(extra.as_ptr(), buf, extra.len());
                (*par).extradata = buf;
                (*par).extradata_size = extra.len() as i32;
            }
        }
    }
    output.write_header().unwrap();
    for (s, mut packet) in input.packets() {
        let tb = output.stream(s.index()).unwrap().time_base();
        packet.rescale_ts(s.time_base(), tb);
        packet.set_position(-1);
        packet
            .write_interleaved(&mut output)
            .expect("writing packet");
    }
    output.write_trailer().unwrap();
}

#[test]
fn a_decoder_error_other_than_invalid_data_fails_the_export() {
    // AudioSpecificConfig's object type is the first five bits. Type 0
    // (Null) is one this AAC decoder does not implement: it returns
    // ENOSYS, not invalid data, and invalid data is the only decoder
    // error the export skips.
    use vidcut_av::ffmpeg::{format, media};
    let fps = Rational::new(30, 1).unwrap();
    let s = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 30).with_audio(
            AudioCodec::Aac,
            48_000,
            2,
        ),
    );
    vidcut_av::init();
    let input = format::input(&s.path).unwrap();
    let stream = input
        .streams()
        .find(|st| st.parameters().medium() == media::Type::Audio)
        .expect("audio");
    let mut extra = unsafe {
        let p = &*stream.parameters().as_ptr();
        std::slice::from_raw_parts(p.extradata, p.extradata_size as usize).to_vec()
    };
    drop(input);
    assert!(extra.len() >= 2, "{extra:?}");
    extra[0] &= 0x07;
    let dir = out_dir("unimplemented");
    let bad = dir.path().join("bad.mkv");
    with_audio_extradata(&s.path, &bad, &extra);
    let broken = Src {
        path: bad,
        origin: s.origin,
    };
    let p = plan(
        fps,
        320,
        180,
        vec![v(&broken, 0, 30)],
        vec![vec![a(&broken, fps, 48_000, 0, 30)]],
    );
    let out = dir.path().join("out.mkv");
    let err = export::run(
        &p,
        &lossless(),
        &out,
        &Availability::default(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .expect_err("an unimplemented audio config must fail");
    match err {
        vidcut_av::Error::Ffmpeg { context, source } => {
            assert_eq!(context, "opening audio decoder");
            assert!(
                !matches!(source, vidcut_av::ffmpeg::Error::InvalidData),
                "invalid data is skipped; this error must fail the export: {source}"
            );
            assert!(
                source.to_string().contains("Function not implemented"),
                "{source}"
            );
        }
        other => panic!("expected a decoder error, got {other}"),
    }
    assert!(!out.exists(), "no finished file");
    assert!(!export::partial_path(&out).exists(), "no .part left");
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
    let audio = vidcut_av::probe::probe(&s.path).unwrap().audio[0].index;
    let dir = out_dir("late");
    let lossy = dir.path().join("late.mkv");
    fixture::spoil(&s.path, &lossy, audio, 0, Spoil::Drop).unwrap();
    let first = vidcut_av::probe::probe(&lossy).unwrap().audio[0]
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
    let video = vidcut_av::probe::probe(&s.path)
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
    let video = vidcut_av::probe::probe(&s.path)
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
    // Output frame j at j/30 s, minus half a frame: first 25 fps frame at or after.
    let want: Vec<u32> = (0..12)
        .map(|j| (((j as f64 / 30.0 - 1.0 / 60.0) * 25.0).ceil().max(0.0)) as u32)
        .collect();
    assert_eq!(codes, want);
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
        let info = vidcut_av::probe::probe(&out).unwrap_or_else(|e| panic!("{}: {e}", preset.name));
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
fn still_frames_follow_the_sampling_rule() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let png = vidcut_av::export::still::png(&v(&a1, 37, 1), fps, 320, 180).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let dir = out_dir("still");
    let path = dir.path().join("f.png");
    std::fs::write(&path, &png).unwrap();
    let mut r = VideoReader::open(&path).unwrap();
    let f = r.next_frame().unwrap().unwrap();
    assert_eq!(read_barcode(&f).unwrap(), Some(37));
}

#[test]
fn a_still_renderer_reads_on_and_back_exactly() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        150,
    ));
    let dir = out_dir("still-renderer");
    let mut renderer = vidcut_av::export::renderer::StillRenderer::default();
    // Forward a frame or a few at a time decodes on; back, or far ahead,
    // seeks. Every picture is the frame asked for.
    // Back a frame at a time: the frames rendered on the way to the first
    // one back stand in for the next.
    let back = (100..=120).rev();
    for (i, frame) in [37, 38, 39, 45, 44, 10, 11, 140, 2]
        .into_iter()
        .chain(back)
        .enumerate()
    {
        let img = renderer.rgba(&v(&a1, frame, 1), 0, fps, 320, 180).unwrap();
        let png = vidcut_av::export::still::png_rgba(img.width, img.height, &img.rgba).unwrap();
        let path = dir.path().join(format!("{i}.png"));
        std::fs::write(&path, &png).unwrap();
        let f = VideoReader::open(&path)
            .unwrap()
            .next_frame()
            .unwrap()
            .unwrap();
        assert_eq!(read_barcode(&f).unwrap(), Some(frame as u32), "picture {i}");
    }
}

#[test]
fn cancelling_leaves_no_files() {
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let p = plan(fps, 320, 180, vec![v(&a1, 0, 90)], vec![]);
    let dir = out_dir("cancel");
    let out = dir.path().join("out.mkv");
    let cancel = AtomicBool::new(false);
    let mut settings = lossless();
    settings.audio = None;
    let err = export::run(
        &p,
        &settings,
        &out,
        &Availability::default(),
        &cancel,
        &mut |pr| {
            if pr.frames == 10 {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        },
    )
    .unwrap_err();
    assert!(export::is_cancelled(&err), "{err}");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "no output or .part left"
    );
    // Cancelled before it starts: it makes nothing at all.
    let err = export::run(
        &p,
        &settings,
        &out,
        &Availability::default(),
        &cancel,
        &mut |_| {},
    )
    .unwrap_err();
    assert!(export::is_cancelled(&err), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
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

#[test]
fn auto_moves_on_from_a_gpu_encoder_that_wont_open() {
    // A GPU encoder that tested fine can fail to open when an export starts
    // (NVENC allows a few sessions at a time). Here the VAAPI render node is
    // gone: `auto` encodes on the CPU instead.
    let fps = Rational::new(30, 1).unwrap();
    let s = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ));
    let p = plan(fps, 320, 180, vec![v(&s, 0, 30)], vec![]);
    let mut settings = lossless();
    settings.audio = None;
    let video = settings.video.as_mut().unwrap();
    video.encoder = EncoderChoice::Auto;
    // A speed meant for the GPU encoder: libx264 has no such preset, and
    // takes its own default.
    video.speed = Some("p4".into());
    let available = Availability {
        encoders: vec![EncoderStatus {
            encoder: "h264_vaapi".into(),
            kind: EncoderChoice::Vaapi,
            device: Some("/dev/dri/renderD999".into()),
            pixel: "nv12".into(),
            ok: true,
            error: None,
        }],
    };
    let first = export::encode::resolve_video(settings.video.as_ref().unwrap(), &available);
    assert_eq!(first.unwrap().name, "h264_vaapi");
    let dir = out_dir("fallback");
    let out = dir.path().join("out.mkv");
    let report = export::run(
        &p,
        &settings,
        &out,
        &available,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(report.video_encoder.as_deref(), Some("libx264"));
    assert_eq!(barcodes(&out), (0..30).map(Some).collect::<Vec<_>>());
    // An encoder asked for by name doesn't give way.
    settings.video.as_mut().unwrap().encoder = EncoderChoice::Vaapi;
    let err = export::run(
        &p,
        &settings,
        &dir.path().join("named.mkv"),
        &available,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(err.to_string().contains("VAAPI"), "{err}");
}

#[test]
fn many_segments_keep_memory_flat() {
    // 400 segments alternating between two sources: memory must not grow with
    // the number of segments (the decoder of the current piece and the one
    // opening ahead). Peak RSS belongs to the process, and the other tests
    // here run alongside, so the measurement runs in a process of its own.
    if std::env::var_os("VIDCUT_MEMORY_TEST").is_none() {
        let run = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "many_segments_keep_memory_flat", "--nocapture"])
            .env("VIDCUT_MEMORY_TEST", "1")
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&run.stderr);
        assert!(
            run.status.success(),
            "{}{log}",
            String::from_utf8_lossy(&run.stdout)
        );
        eprint!("{log}");
        return;
    }
    let fps = Rational::new(30, 1).unwrap();
    let x = src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 360, (30, 1), 300).gop(60));
    let y = src(FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        640,
        360,
        (30, 1),
        300,
    ));
    let mut video = Vec::new();
    let mut lane = Vec::new();
    for i in 0..400i64 {
        let (s, at) = if i % 2 == 0 {
            (&x, (i * 37) % 290)
        } else {
            (&y, (i * 53) % 290)
        };
        video.push(v(s, at, 2));
        lane.push(a(s, fps, 48_000, at, 2));
    }
    let p = plan(fps, 640, 360, video, vec![lane]);
    let dir = out_dir("many");
    let out = dir.path().join("out.mkv");
    let before = peak_rss_mb();
    render(&p, &lossless(), &out);
    let grew = peak_rss_mb() - before;
    eprintln!("400 segments: peak RSS grew by {grew} MB");
    // About 100 MB here; a frame kept per segment would add 280 MB.
    assert!(grew < 250, "peak RSS grew by {grew} MB");
    assert_eq!(barcodes(&out).len(), 800);
}

/// The process's peak resident memory in MB, as the system counts it: a
/// reading or a panic, never a stand-in that would pass.
fn peak_rss_mb() -> i64 {
    #[cfg(unix)]
    {
        let status = std::fs::read_to_string("/proc/self/status").expect("/proc/self/status");
        let kb = status
            .lines()
            .find(|l| l.starts_with("VmHWM:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .expect("VmHWM in /proc/self/status");
        kb.parse::<i64>().unwrap() / 1024
    }
    #[cfg(windows)]
    // SAFETY: fills the counters it is given, their size set first.
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        assert!(
            GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0,
            "no memory counters: {}",
            std::io::Error::last_os_error()
        );
        (c.PeakWorkingSetSize >> 20) as i64
    }
}

#[cfg(feature = "gpu-tests")]
#[test]
fn gpu_encoders_produce_valid_frame_exact_output() {
    let available = vidcut_av::export::encode::detect();
    let fps = Rational::new(30, 1).unwrap();
    let a1 = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        640,
        360,
        (30, 1),
        90,
    ));
    let p = plan(
        fps,
        640,
        360,
        vec![v(&a1, 15, 45)],
        vec![vec![a(&a1, fps, 48_000, 15, 45)]],
    );
    let dir = out_dir("gpu");
    let mut tested = 0;
    for status in available.encoders.iter().filter(|e| e.ok) {
        let codec = match status.encoder.split('_').next().unwrap() {
            "h264" => VCodec::H264,
            "hevc" => VCodec::Hevc,
            "av1" => VCodec::Av1,
            other => panic!("unexpected {other}"),
        };
        let ten = status.pixel == "p010le";
        let settings = ExportSettings {
            container: OutContainer::Mp4,
            video: Some(VideoSettings {
                codec,
                encoder: status.kind,
                quality: Some(18),
                bitrate: None,
                speed: None,
                profile: None,
                bit_depth: Some(if ten { 10 } else { 8 }),
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
                codec: ACodec::Aac,
                bitrate: None,
                sample_rate: None,
                channels: None,
                loudness: None,
            }),
            declick: true,
        };
        let out = dir
            .path()
            .join(format!("{}-{}.mp4", status.encoder, status.pixel));
        let report = export::run(
            &p,
            &settings,
            &out,
            &available,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_or_else(|e| panic!("{} {}: {e}", status.encoder, status.pixel));
        assert_eq!(
            report.video_encoder.as_deref(),
            Some(status.encoder.as_str())
        );
        let codes = barcodes(&out);
        assert_eq!(codes.len(), 45, "{}", status.encoder);
        let want: Vec<Option<u32>> = (15..60).map(Some).collect();
        assert_eq!(codes, want, "{} {}: frames", status.encoder, status.pixel);
        tested += 1;
    }
    eprintln!("verified {tested} GPU encoder configurations");
    assert!(tested > 0, "no GPU encoder available");
}

/// A piece of image `path` (a still, or a sequence's first file).
fn pictures(path: &Path, image: Image, source_frame: i64, frames: i64) -> VideoPiece {
    VideoPiece::Source {
        media: "m".into(),
        path: path.to_path_buf(),
        origin: 0.0,
        source_frame,
        frames,
        image: Some(image),
    }
}

#[test]
fn stills_show_one_picture_for_as_long_as_they_last() {
    // A still lasts as long as its clip, from any source frame:
    // every frame is its one picture. PNG and JPEG alike.
    let fps = Rational::new(30, 1).unwrap();
    let png = fixture::ensure_images(&ImageSpec::still(77, 320, 180, "png")).unwrap();
    let jpg = fixture::ensure_images(&ImageSpec::still(88, 640, 360, "jpg")).unwrap();
    let info = vidcut_av::probe::probe(&png[0]).unwrap();
    let v = info.video.unwrap();
    assert!(v.still, "{}", info.format);
    assert_eq!((v.width, v.height), (320, 180));
    assert!(
        vidcut_av::probe::probe(&jpg[0])
            .unwrap()
            .video
            .unwrap()
            .still
    );
    let p = plan(
        fps,
        320,
        180,
        vec![
            pictures(&png[0], Image::Still, 0, 20),
            pictures(&jpg[0], Image::Still, 4000, 10),
            pictures(&png[0], Image::Still, 90_000, 5),
        ],
        vec![],
    );
    let dir = out_dir("stills");
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);
    let mut want = vec![Some(77); 20];
    want.extend([Some(88); 10]);
    want.extend([Some(77); 5]);
    assert_eq!(barcodes(&out), want);
}

#[test]
fn sequences_play_their_pictures_at_their_rate() {
    // Numbered images are one clip at a rate: here 30 fps on a 30
    // fps timeline, frame for frame, then 24 fps, by the sampling rule.
    let fps = Rational::new(30, 1).unwrap();
    let files = fixture::ensure_images(&ImageSpec::sequence(1000, 60, 4, 320, 180)).unwrap();
    let pattern = files[0].with_file_name("img_%04d.png");
    let at = |rate: i64| Image::Sequence {
        pattern: pattern.clone(),
        first: 1000,
        count: 60,
        rate: Rational::new(rate, 1).unwrap(),
    };
    let info = vidcut_av::probe::probe_sequence(&pattern, 1000, 60, (24, 1)).unwrap();
    let v = info.video.unwrap();
    assert!(!v.still);
    assert_eq!((v.frames, v.avg_frame_rate), (Some(60), (24, 1)));
    assert!((v.duration_secs.unwrap() - 2.5).abs() < 1e-9);
    // Pool thumbnails spread over the pictures, as over a video's frames.
    let input = vidcut_av::decode::Input::Sequence {
        pattern: pattern.clone(),
        first: 1000,
        rate: (24, 1),
    };
    // (At the pictures' own size: barcodes don't survive shrinking.)
    let (images, times) = vidcut_av::thumbs::filmstrip(&input, 4, 320, 180).unwrap();
    assert_eq!(images.len(), 4);
    for (img, t) in images.iter().zip(&times) {
        let w = img.width as usize;
        let code = vidcut_av::barcode::read_packed(&img.rgba, w, img.height as usize, w * 4, 4, 1)
            .unwrap();
        // A thumbnail may be up to half the spacing (2.5 s / 4) early.
        let early = ((t - 0.3125) * 24.0).floor() as u32;
        let late = (t * 24.0).ceil() as u32;
        assert!((early..=late).contains(&code), "{code} for {t} s");
    }
    let p = plan(
        fps,
        320,
        180,
        vec![
            pictures(&files[0], at(30), 5, 20),
            pictures(&files[0], at(24), 10, 30),
        ],
        vec![],
    );
    let dir = out_dir("sequences");
    let out = dir.path().join("out.mkv");
    render(&p, &lossless(), &out);
    let mut want: Vec<Option<u32>> = (5..25).map(Some).collect();
    // Timeline frame j of a 30 fps project shows the first 24 fps picture
    // at or after j/30 less half a frame.
    want.extend((10..40).map(|j| {
        let t = (f64::from(j) - 0.5) / 30.0;
        Some((t * 24.0 - 1e-6).ceil() as u32)
    }));
    assert_eq!(barcodes(&out), want);
}

/// With NVENC, video pieces decode on NVDEC and scale there, padded to the
/// output when their aspect differs (4:3 and 2.4:1 in 16:9), turned and
/// converted (SD) by libplacebo, deinterlaced by the reader; black is made
/// on the CPU and uploaded. Every frame is the one the CPU export shows:
/// across cuts, files, a seek back, 10-bit and AV1 sources, other rates (60,
/// 25 interlaced and 24 fps into 30), in 8 and 10 bits.
#[cfg(feature = "gpu-tests")]
#[test]
fn nvenc_exports_decode_on_the_gpu_and_match_the_cpu() {
    let available = vidcut_av::export::encode::detect();
    let fps = Rational::new(30, 1).unwrap();
    let hd = |codec, rate: (u32, u32), frames| {
        src(FixtureSpec::new(
            codec,
            Container::Mkv,
            1280,
            720,
            rate,
            frames,
        ))
    };
    let a = hd(VideoCodec::H264, (30, 1), 90);
    let b = hd(VideoCodec::Hevc10, (30, 1), 60);
    let c = hd(VideoCodec::H264, (60, 1), 120);
    let d = hd(VideoCodec::H264, (24, 1), 48);
    let e = hd(VideoCodec::Av1, (30, 1), 30);
    let sd = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ));
    // Black at the sides, and above and below.
    let four_three = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        1440,
        1080,
        (30, 1),
        30,
    ));
    let scope = src(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        1920,
        800,
        (30, 1),
        30,
    ));
    // Stored turned, as phones do: libplacebo turns them upright.
    let turned = |rotation| {
        src(
            FixtureSpec::new(VideoCodec::H264, Container::Mp4, 1280, 720, (30, 1), 30)
                .rotation(rotation),
        )
    };
    let (r90, r180, r270) = (turned(90), turned(180), turned(270));
    let r90_deep =
        src(
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mp4, 1280, 720, (30, 1), 30)
                .rotation(90),
        );
    // Interlaced: the reader deinterlaces on the GPU in decode order, so
    // other rates and a seek back pick the frames the CPU does.
    let interlaced = |w, h, rate| {
        src(FixtureSpec::new(VideoCodec::H264, Container::Mkv, w, h, rate, 50).interlaced(true))
    };
    let (inter, inter_sd) = (
        interlaced(1280, 720, (25, 1)),
        interlaced(640, 360, (30, 1)),
    );
    let video = vec![
        v(&inter, 5, 20),
        v(&inter_sd, 0, 10),
        v(&inter, 2, 10),
        v(&a, 10, 30),
        VideoPiece::Black { frames: 10 },
        v(&b, 5, 30),
        v(&c, 0, 30),
        v(&sd, 0, 15),
        v(&d, 0, 30),
        v(&four_three, 0, 10),
        v(&scope, 0, 10),
        v(&r90, 0, 10),
        v(&r180, 5, 10),
        v(&r270, 10, 10),
        v(&r90_deep, 0, 10),
        v(&e, 0, 20),
        v(&a, 50, 20),
    ];
    // All but black. The small untagged pieces are BT.601 and the turned
    // ones need turning, which libplacebo does; in 10 bits it pads too
    // (FFmpeg's pad_cuda takes 8 bits only).
    let on_gpu = |_bits| 275;
    let p = plan(fps, 1280, 720, video, vec![]);
    let dir = out_dir("gpu-pipeline");
    let mut reference = lossless();
    reference.audio = None;
    let cpu = dir.path().join("cpu.mkv");
    render(&p, &reference, &cpu);
    let want = barcodes(&cpu);
    assert_eq!(want.len(), 285);
    for (codec, bits) in [(VCodec::H264, 8), (VCodec::Hevc, 10)] {
        let settings = ExportSettings {
            container: OutContainer::Mp4,
            video: Some(VideoSettings {
                codec,
                encoder: EncoderChoice::Nvenc,
                quality: Some(18),
                bitrate: None,
                speed: None,
                profile: None,
                bit_depth: Some(bits),
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
            audio: None,
            declick: false,
        };
        let out = dir.path().join(format!("gpu-{bits}.mp4"));
        let report = export::run(
            &p,
            &settings,
            &out,
            &available,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_or_else(|e| panic!("{bits}-bit: {e}"));
        assert_eq!(
            report.gpu_frames,
            on_gpu(bits),
            "{bits}-bit: frames on the GPU"
        );
        let got = barcodes(&out);
        assert_eq!(got.len(), want.len(), "{bits}-bit");
        for (k, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g, w, "{bits}-bit: frame {k}");
        }
        // The interlaced pieces are deinterlaced: no combing.
        if bits == 8 {
            let mut r = VideoReader::open(&out).unwrap();
            for k in 0..40 {
                let c = combing(&r.next_frame().unwrap().unwrap());
                assert!(c < 10.0, "frame {k} combs: {c:.1}");
            }
        }
        // The pillarboxed and letterboxed pictures start where the CPU's
        // do (at even offsets).
        for k in [190, 200] {
            assert_eq!(
                picture_starts(&frame_at(&out, k)),
                picture_starts(&frame_at(&cpu, k)),
                "{bits}-bit: frame {k}"
            );
        }
        // Black is video-range black in 10 bits too: P010 keeps its 10 bits
        // high.
        let mut r = VideoReader::open(&out).unwrap();
        let black = (0..=75).map(|_| r.next_frame().unwrap().unwrap()).last();
        let m = region_means(&black.unwrap(), (0, 1280));
        let near = |v: f64, want: f64| (v - want).abs() <= 1.0;
        assert!(
            near(m[0], 16.0) && near(m[1], 128.0) && near(m[2], 128.0),
            "{bits}-bit: black is {m:?}"
        );
    }
}

/// Frame `k` of `path`.
fn frame_at(path: &Path, k: usize) -> vidcut_av::ffmpeg::frame::Video {
    let mut r = VideoReader::open(path).unwrap();
    (0..=k)
        .map(|_| r.next_frame().unwrap().unwrap())
        .last()
        .unwrap()
}

/// The first column and row of a picture on black: where the luma along
/// the middle row and column first rises above black.
#[cfg(feature = "gpu-tests")]
fn picture_starts(f: &vidcut_av::ffmpeg::frame::Video) -> (usize, usize) {
    use vidcut_av::ffmpeg::format::Pixel;
    let ten = f.format() == Pixel::YUV420P10LE;
    let (w, h, stride, data) = (
        f.width() as usize,
        f.height() as usize,
        f.stride(0),
        f.data(0),
    );
    let luma = |x: usize, y: usize| {
        if ten {
            u16::from_le_bytes([data[y * stride + 2 * x], data[y * stride + 2 * x + 1]]) / 4
        } else {
            u16::from(data[y * stride + x])
        }
    };
    let x = (0..w).find(|&x| luma(x, h / 2) > 40).unwrap_or(w);
    let y = (0..h).find(|&y| luma(w / 2, y) > 40).unwrap_or(h);
    (x, y)
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

/// The mean (Y, Cb, Cr) in 8-bit units of the black left of a fitted
/// [`Picture::Colors`] picture running from `x0` to `x1` across, then of
/// each of its stripes: the middle half of each, in the middle rows.
#[cfg(feature = "gpu-tests")]
fn stripe_means(f: &vidcut_av::ffmpeg::frame::Video, (x0, x1): (usize, usize)) -> Vec<[f64; 3]> {
    let w = (x1 - x0) / 4;
    let mut regions = vec![(x0 / 4, x0 * 3 / 4)];
    regions.extend((0..4).map(|i| (x0 + i * w + w / 4, x0 + i * w + 3 * w / 4)));
    regions.into_iter().map(|r| region_means(f, r)).collect()
}

/// (Y, Cb, Cr) in 8-bit video range of BT.601 converted to BT.709, the
/// matrix only.
#[cfg(feature = "gpu-tests")]
fn bt601_to_bt709((y, cb, cr): (u8, u8, u8)) -> [f64; 3] {
    let y = (f64::from(y) - 16.0) / 219.0;
    let (cb, cr) = (
        (f64::from(cb) - 128.0) / 224.0,
        (f64::from(cr) - 128.0) / 224.0,
    );
    let r = y + 1.402 * cr;
    let g = y - 0.344136 * cb - 0.714136 * cr;
    let b = y + 1.772 * cb;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    [
        16.0 + 219.0 * y,
        128.0 + 224.0 * (b - y) / 1.8556,
        128.0 + 224.0 * (r - y) / 1.5748,
    ]
}

/// BT.601 pictures (untagged SD) go to BT.709 on the GPU, through
/// libplacebo, with the colors the CPU gives them: each stripe of a colored
/// 4:3 picture and the black beside it, from 8 and 10 bits, to 8 bits
/// (libplacebo converts, the CUDA filters scale and pad) and 10 (libplacebo
/// does it all). libplacebo is the more exact (swscale is up to two levels
/// off).
#[cfg(feature = "gpu-tests")]
#[test]
fn nvenc_exports_convert_bt601_on_the_gpu_as_the_cpu_does() {
    use vidcut_av::fixture::Picture;
    let available = vidcut_av::export::encode::detect();
    let fps = Rational::new(30, 1).unwrap();
    // 8 and 10 bits in: libplacebo reads 10-bit frames as P016.
    for source in [VideoCodec::H264, VideoCodec::Hevc10] {
        let colors = src(
            FixtureSpec::new(source, Container::Mp4, 640, 480, (30, 1), 30)
                .picture(Picture::Colors),
        );
        let p = plan(fps, 1280, 720, vec![v(&colors, 0, 30)], vec![]);
        let dir = out_dir(&format!("gpu-bt601-{source:?}"));
        let frame = |path: &Path| {
            let mut r = VideoReader::open(path).unwrap();
            let mut frames = Vec::new();
            while let Some(f) = r.next_frame().unwrap() {
                frames.push(f);
            }
            assert_eq!(frames.len(), 30, "{}", path.display());
            frames.swap_remove(15)
        };
        let exact: Vec<[f64; 3]> = std::iter::once([16.0, 128.0, 128.0])
            .chain(Picture::COLORS.into_iter().map(bt601_to_bt709))
            .collect();
        let near = |got: &[[f64; 3]], want: &[[f64; 3]], by: f64, what: &str| {
            for (i, (g, w)) in got.iter().zip(want).enumerate() {
                for c in 0..3 {
                    assert!(
                        (g[c] - w[c]).abs() <= by,
                        "{what}: region {i} channel {c}: {g:?}, not {w:?}"
                    );
                }
            }
        };
        let mut reference = lossless();
        reference.audio = None;
        let cpu = dir.path().join("cpu.mkv");
        render(&p, &reference, &cpu);
        // The picture is 960 wide in the middle.
        let cpu = stripe_means(&frame(&cpu), (160, 1120));
        near(&cpu, &exact, 2.0, &format!("{source:?} on the CPU"));
        for (codec, bits) in [(VCodec::H264, 8), (VCodec::Hevc, 10)] {
            let settings = ExportSettings {
                container: OutContainer::Mp4,
                video: Some(VideoSettings {
                    codec,
                    encoder: EncoderChoice::Nvenc,
                    quality: Some(18),
                    bitrate: None,
                    speed: None,
                    profile: None,
                    bit_depth: Some(bits),
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
                audio: None,
                declick: false,
            };
            let out = dir.path().join(format!("gpu-{bits}.mp4"));
            let report = export::run(
                &p,
                &settings,
                &out,
                &available,
                &AtomicBool::new(false),
                &mut |_| {},
            )
            .unwrap_or_else(|e| panic!("{bits}-bit: {e}"));
            assert_eq!(report.gpu_frames, 30, "{bits}-bit: frames on the GPU");
            let gpu = stripe_means(&frame(&out), (160, 1120));
            // scale_cuda truncates to 8 bits, where it could round.
            near(
                &gpu,
                &exact,
                1.5,
                &format!("{source:?} to {bits} bits on the GPU"),
            );
            near(
                &gpu,
                &cpu,
                2.0,
                &format!("{source:?} to {bits} bits against the CPU"),
            );
        }
    }
}

/// HDR sources into SDR (tone mapping), and SDR and HDR sources into PQ and
/// HLG, are converted on the GPU by libplacebo with a shader doing the
/// CPU's math: every frame but black is made on the GPU, its barcode is the
/// CPU export's, the barcode's black and white cells are within two levels
/// of the CPU's, and so are colored stripes (BT.2020 PQ, and BT.601 SD into
/// HDR) and the black beside them. Among the pieces: HLG stored turned (a
/// phone's), 4:3 PQ with black bars, and 8 and 10-bit SDR output.
#[cfg(feature = "gpu-tests")]
#[test]
fn nvenc_exports_convert_hdr_on_the_gpu_as_the_cpu_does() {
    use vidcut_av::fixture::Picture;
    use vidcut_core::model::Hdr;
    let available = vidcut_av::export::encode::detect();
    let fps = Rational::new(30, 1).unwrap();
    let hdr = |w, h, hdr, rotation| {
        src(
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mp4, w, h, (30, 1), 30)
                .hdr(hdr)
                .rotation(rotation)
                .no_audio(),
        )
    };
    let pq = hdr(1280, 720, Hdr::Pq, 0);
    let hlg_turned = hdr(1280, 720, Hdr::Hlg, 90);
    let pq_four_three = hdr(960, 720, Hdr::Pq, 0);
    let pq_colors =
        src(
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 640, 480, (30, 1), 30)
                .hdr(Hdr::Pq)
                .picture(Picture::Colors)
                .no_audio(),
        );
    let sdr =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 1280, 720, (30, 1), 30).no_audio());
    let sd_colors = src(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 480, (30, 1), 30)
            .picture(Picture::Colors)
            .no_audio(),
    );
    let video = vec![
        v(&pq, 5, 15),
        v(&hlg_turned, 0, 10),
        v(&pq_four_three, 0, 10),
        v(&pq_colors, 0, 10),
        v(&sdr, 0, 15),
        v(&sd_colors, 0, 10),
        VideoPiece::Black { frames: 5 },
    ];
    // Frames of barcodes, and of colored stripes.
    let barcode_frames: Vec<usize> = (0..35).chain(45..60).collect();
    let stripe_frames = [40, 65];
    let p = plan(fps, 1280, 720, video, vec![]);
    let dir = out_dir("gpu-hdr");
    let settings = |codec, bits, hdr, encoder, quality| ExportSettings {
        container: OutContainer::Mkv,
        video: Some(VideoSettings {
            codec,
            encoder,
            quality: Some(quality),
            bitrate: None,
            speed: None,
            profile: None,
            bit_depth: Some(bits),
            width: None,
            height: None,
            hdr: match hdr {
                None => HdrMode::Sdr,
                Some(Hdr::Pq) => HdrMode::Pq,
                Some(Hdr::Hlg) => HdrMode::Hlg,
            },
            passes: None,
            first_pass: Default::default(),
            multipass: Default::default(),
            lookahead: None,
            hdr_metadata: Default::default(),
            mastering_display: None,
            max_cll: None,
        }),
        audio: None,
        declick: false,
    };
    for (what, codec, bits, target) in [
        ("SDR", VCodec::H264, 8, None),
        ("10-bit SDR", VCodec::Hevc, 10, None),
        ("PQ", VCodec::Hevc, 10, Some(Hdr::Pq)),
        ("HLG", VCodec::Hevc, 10, Some(Hdr::Hlg)),
    ] {
        let cpu = dir.path().join(format!("cpu-{what}.mkv"));
        // Lossless H.264; HEVC nearly so.
        let quality = if bits == 8 { 0 } else { 2 };
        render(
            &p,
            &settings(codec, bits, target, EncoderChoice::Cpu, quality),
            &cpu,
        );
        let gpu = dir.path().join(format!("gpu-{what}.mkv"));
        let report = export::run(
            &p,
            &settings(codec, bits, target, EncoderChoice::Nvenc, 12),
            &gpu,
            &available,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(report.gpu_frames, 70, "{what}: frames on the GPU");
        let (want, got) = (barcodes(&cpu), barcodes(&gpu));
        assert_eq!(got.len(), 75, "{what}");
        assert_eq!(got, want, "{what}: barcodes");
        // Tone-mapped barcodes read (a pillarboxed one doesn't anyway).
        if target.is_none() {
            for k in (0..25).chain(45..60) {
                assert!(got[k].is_some(), "{what}: frame {k} has no barcode");
            }
        }
        let (mut cpu_frames, mut gpu_frames) = (Vec::new(), Vec::new());
        for (path, frames) in [(&cpu, &mut cpu_frames), (&gpu, &mut gpu_frames)] {
            let mut r = VideoReader::open(path).unwrap();
            while let Some(f) = r.next_frame().unwrap() {
                frames.push(f);
            }
        }
        let mut worst = (0, 0.0f64);
        for &k in &barcode_frames {
            let (c, g) = (cell_levels(&cpu_frames[k]), cell_levels(&gpu_frames[k]));
            // 10-bit codes: two levels are eight.
            assert!(
                c.0.abs_diff(g.0) <= 8 && c.1.abs_diff(g.1) <= 8,
                "{what}: frame {k}: black and white {g:?}, the CPU's {c:?}"
            );
            worst.0 = worst.0.max(c.0.abs_diff(g.0)).max(c.1.abs_diff(g.1));
        }
        // The 4:3 picture starts where the CPU's does.
        assert_eq!(
            picture_starts(&gpu_frames[30]),
            picture_starts(&cpu_frames[30]),
            "{what}: black bars"
        );
        for k in stripe_frames {
            // The 4:3 picture is 960 wide in the middle.
            let c = stripe_means(&cpu_frames[k], (160, 1120));
            let g = stripe_means(&gpu_frames[k], (160, 1120));
            for (i, (c, g)) in c.iter().zip(&g).enumerate() {
                for ch in 0..3 {
                    assert!(
                        (c[ch] - g[ch]).abs() <= 2.0,
                        "{what}: frame {k} region {i} channel {ch}: {g:?}, the CPU's {c:?}"
                    );
                    worst.1 = worst.1.max((c[ch] - g[ch]).abs());
                }
            }
        }
        eprintln!(
            "{what}: cells within {} codes of the CPU's (10-bit), stripes within {:.2} levels",
            worst.0, worst.1
        );
    }
}

/// Decision D44 through NVENC: with `auto`, a mostly-PQ timeline stays PQ
/// in 10 bits and a 10-bit SDR one 10-bit SDR, as on the CPU, the frames
/// made on the GPU and within two levels of the CPU's; the `auto` encoder
/// chooses the same.
#[cfg(feature = "gpu-tests")]
#[test]
fn nvenc_auto_color_chooses_as_the_cpu_does() {
    use vidcut_core::model::Hdr;
    let available = vidcut_av::export::encode::detect();
    if !available
        .encoders
        .iter()
        .any(|e| e.ok && e.encoder == "hevc_nvenc" && e.pixel == "p010le")
    {
        eprintln!("skipping: no 10-bit hevc_nvenc here");
        return;
    }
    let fps = Rational::new(30, 1).unwrap();
    let hevc10 = || FixtureSpec::new(VideoCodec::Hevc10, Container::Mp4, 1280, 720, (30, 1), 30);
    let pq = src(hevc10().hdr(Hdr::Pq).no_audio());
    let deep = src(hevc10().no_audio());
    let sdr =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 1280, 720, (30, 1), 30).no_audio());
    let dir = out_dir("gpu-auto-color");
    for (what, video, want) in [
        ("PQ", vec![v(&pq, 5, 20), v(&sdr, 5, 10)], (HdrMode::Pq, 10)),
        (
            "10-bit SDR",
            vec![v(&deep, 5, 25), v(&sdr, 5, 5)],
            (HdrMode::Sdr, 10),
        ),
    ] {
        let p = plan(fps, 1280, 720, video, vec![]);
        let cpu = dir.path().join(format!("cpu-{what}.mkv"));
        let mut settings = auto_color(VCodec::Hevc, EncoderChoice::Cpu);
        settings.video.as_mut().unwrap().quality = Some(2);
        let c = render(&p, &settings, &cpu).color.unwrap();
        assert_eq!((c.hdr, c.bit_depth), want, "{what} on the CPU");
        for encoder in [EncoderChoice::Nvenc, EncoderChoice::Auto] {
            let gpu = dir.path().join(format!("gpu-{what}-{encoder:?}.mkv"));
            let mut settings = auto_color(VCodec::Hevc, encoder);
            let video = settings.video.as_mut().unwrap();
            video.quality = Some(12);
            video.speed = None;
            let report = export::run(
                &p,
                &settings,
                &gpu,
                &available,
                &AtomicBool::new(false),
                &mut |_| {},
            )
            .unwrap_or_else(|e| panic!("{what}: {e}"));
            let c = report.color.as_ref().unwrap();
            assert_eq!((c.hdr, c.bit_depth), want, "{what} with {encoder:?}");
            assert_eq!(report.video_encoder.as_deref(), Some("hevc_nvenc"));
            assert_eq!(report.gpu_frames, 30, "{what}: frames on the GPU");
            let info = vidcut_av::probe::probe(&gpu).unwrap().video.unwrap();
            assert_eq!(info.pix_fmt, "yuv420p10le", "{what}");
            assert_eq!(
                info.transfer == "smpte2084",
                want.0 == HdrMode::Pq,
                "{what}"
            );
            let (mut cpu_frames, mut gpu_frames) = (Vec::new(), Vec::new());
            for (path, frames) in [(&cpu, &mut cpu_frames), (&gpu, &mut gpu_frames)] {
                let mut r = VideoReader::open(path).unwrap();
                while let Some(f) = r.next_frame().unwrap() {
                    frames.push(f);
                }
            }
            assert_eq!(gpu_frames.len(), 30, "{what}");
            for k in 0..30 {
                let (c, g) = (cell_levels(&cpu_frames[k]), cell_levels(&gpu_frames[k]));
                assert!(
                    c.0.abs_diff(g.0) <= 8 && c.1.abs_diff(g.1) <= 8,
                    "{what}: frame {k}: black and white {g:?}, the CPU's {c:?}"
                );
            }
        }
    }
}

/// Decision D45 through NVENC: a bitrate makes it look at each frame twice
/// within one encode, the frames decoded and scaled on the GPU as in one
/// pass, every frame where it belongs; one run, so one pass reported in
/// progress.
#[cfg(feature = "gpu-tests")]
#[test]
fn nvenc_makes_two_passes_in_one_encode() {
    use vidcut_core::export::Multipass;
    let available = vidcut_av::export::encode::detect();
    if !available
        .encoders
        .iter()
        .any(|e| e.ok && e.encoder == "h264_nvenc")
    {
        eprintln!("skipping: no h264_nvenc here");
        return;
    }
    let fps = Rational::new(30, 1).unwrap();
    let a =
        src(FixtureSpec::new(VideoCodec::H264, Container::Mp4, 1280, 720, (30, 1), 90).no_audio());
    let p = plan(fps, 1280, 720, vec![v(&a, 0, 90)], vec![]);
    let dir = out_dir("gpu-passes");
    for (multipass, passes) in [
        (Multipass::Quarter, None),
        (Multipass::Full, Some(2)),
        (Multipass::Quarter, Some(1)),
    ] {
        let mut settings = at_bitrate(VCodec::H264, 2_000_000, 1);
        let video = settings.video.as_mut().unwrap();
        video.encoder = EncoderChoice::Nvenc;
        video.passes = passes;
        video.multipass = multipass;
        video.lookahead = Some(16);
        let out = dir.path().join(format!("{multipass:?}-{passes:?}.mp4"));
        let mut runs = Vec::new();
        let report = export::run(
            &p,
            &settings,
            &out,
            &available,
            &AtomicBool::new(false),
            &mut |pr| runs.push((pr.pass, pr.passes)),
        )
        .unwrap_or_else(|e| panic!("{multipass:?}: {e}"));
        let want_passes = if passes == Some(1) { 1 } else { 2 };
        assert_eq!(report.passes, Some(want_passes), "{multipass:?} {passes:?}");
        assert!(runs.iter().all(|&r| r == (1, 1)), "one run");
        assert_eq!(report.gpu_frames, 90, "frames on the GPU");
        let want: Vec<Option<u32>> = (0..90).map(Some).collect();
        assert_eq!(barcodes(&out), want, "{multipass:?} {passes:?}");
        assert!(!export::passes_path(&out).exists());
    }
}
