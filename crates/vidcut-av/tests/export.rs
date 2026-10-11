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
    VideoCodec as VCodec, VideoSettings,
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
            hdr: None,
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
    video.hdr = Some(Hdr::Pq);
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
    assert_eq!(io.kind(), std::io::ErrorKind::IsADirectory, "{io}");
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

fn peak_rss_mb() -> i64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l.split_whitespace().nth(1).unwrap().parse::<i64>().unwrap() / 1024)
        })
        .unwrap_or(0)
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
                hdr: None,
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
    // A still lasts as long as its clip (p. 1194), from any source frame:
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
    // Numbered images are one clip (p. 366) at a rate: here 30 fps on a 30
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
