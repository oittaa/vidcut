use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::export::{self, encode};
use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, Picture, Sound, VideoCodec};
use vidcut_core::export::{
    AudioCodec as ACodec, AudioSettings, Container as OutContainer, EncoderChoice, ExportSettings,
    HdrMode, VideoCodec as VCodec, VideoSettings,
};
use vidcut_core::render::{AudioPiece, Curve, Fade, Over, Ramp, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

fn fps() -> Rational {
    Rational::new(25, 1).unwrap()
}

fn clip(codec: VideoCodec, picture: Picture) -> PathBuf {
    let spec = FixtureSpec::new(codec, Container::Mkv, 320, 180, (25, 1), 40)
        .picture(picture)
        .no_audio();
    fixture::ensure(&spec).unwrap()
}

fn piece(path: &Path, source_frame: i64, frames: i64) -> VideoPiece {
    let origin = vidcut_av::probe::probe(path, None)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: path.to_path_buf(),
        origin,
        source_frame,
        frames,
        image: None,
    }
}

fn mix(under: VideoPiece, over: VideoPiece) -> VideoPiece {
    let of = under.frames();
    VideoPiece::Mix {
        under: Box::new(under),
        over: vec![Over {
            piece: over,
            ramp: Ramp {
                at: 0,
                of,
                falling: false,
            },
        }],
    }
}

fn plan(video: Vec<VideoPiece>) -> RenderPlan {
    let frames = video.iter().map(VideoPiece::frames).sum();
    RenderPlan {
        fps: fps(),
        width: 320,
        height: 180,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video,
        audio: vec![],
        range: (0, frames),
        name: "t".into(),
    }
}

fn settings(encoder: EncoderChoice) -> ExportSettings {
    ExportSettings {
        container: OutContainer::Mkv,
        video: Some(VideoSettings {
            codec: VCodec::H264,
            encoder,
            quality: Some(if encoder == EncoderChoice::Cpu { 0 } else { 1 }),
            bitrate: None,
            speed: if encoder == EncoderChoice::Cpu {
                Some("ultrafast".into())
            } else {
                None
            },
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
        audio: None,
        declick: false,
    }
}

fn export_on(
    plan: &RenderPlan,
    settings: &ExportSettings,
    available: &encode::Availability,
    decoder: Option<&vidcut_av::hw::GpuDecoder>,
    out: &Path,
) {
    export::run_decoding_on(
        plan,
        settings,
        out,
        available,
        decoder,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_or_else(|e| panic!("export: {e}"));
}

fn first_stripe(path: &Path) -> Vec<[f64; 3]> {
    let mut r = VideoReader::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        let mean = |p: usize| {
            let sub = if p == 0 { 1 } else { 2 };
            let (stride, data) = (f.stride(p), f.data(p));
            let h = f.height() as usize / sub;
            let (mut sum, mut n) = (0.0, 0.0);
            for y in h / 4..h * 3 / 4 {
                for x in 8 / sub..72 / sub {
                    sum += f64::from(data[y * stride + x]);
                    n += 1.0;
                }
            }
            sum / n
        };
        out.push([mean(0), mean(1), mean(2)]);
    }
    out
}

fn out_dir(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("vidcut-dissolve-{name}-"))
        .tempdir()
        .unwrap()
}

#[test]
fn a_fade_from_black_shows_each_frame_at_its_level() {
    let stripes = clip(VideoCodec::Ffv1, Picture::Colors);
    let p = plan(vec![
        mix(VideoPiece::Black { frames: 10 }, piece(&stripes, 0, 10)),
        piece(&stripes, 10, 5),
    ]);
    let dir = out_dir("fade");
    let out = dir.path().join("fade.mkv");
    export_on(
        &p,
        &settings(EncoderChoice::Cpu),
        &Default::default(),
        None,
        &out,
    );
    let luma: Vec<f64> = first_stripe(&out).iter().map(|m| m[0]).collect();
    assert_eq!(
        luma,
        [
            20.0, 28.0, 36.0, 44.0, 52.0, 60.0, 68.0, 76.0, 84.0, 92.0, 96.0, 96.0, 96.0, 96.0,
            96.0
        ]
    );
}

#[test]
fn a_fade_from_black_takes_the_chroma_from_its_middle() {
    let stripes = clip(VideoCodec::Ffv1, Picture::Colors);
    let p = plan(vec![
        mix(VideoPiece::Black { frames: 10 }, piece(&stripes, 0, 10)),
        piece(&stripes, 10, 1),
    ]);
    let dir = out_dir("chroma");
    let out = dir.path().join("fade.mkv");
    export_on(
        &p,
        &settings(EncoderChoice::Cpu),
        &Default::default(),
        None,
        &out,
    );
    let cb: Vec<f64> = first_stripe(&out).iter().map(|m| m[1]).collect();
    assert_eq!(
        cb,
        [
            126.0, 123.0, 120.0, 116.0, 113.0, 109.0, 106.0, 103.0, 99.0, 96.0, 94.0
        ]
    );
}

#[test]
fn a_cross_dissolve_mixes_two_files_frame_by_frame() {
    let stripes = clip(VideoCodec::Ffv1, Picture::Colors);
    let black = clip(VideoCodec::Ffv1, Picture::Black);
    let p = plan(vec![
        piece(&stripes, 0, 2),
        mix(piece(&stripes, 2, 10), piece(&black, 0, 10)),
        piece(&black, 10, 2),
    ]);
    let dir = out_dir("cross");
    let out = dir.path().join("cross.mkv");
    export_on(
        &p,
        &settings(EncoderChoice::Cpu),
        &Default::default(),
        None,
        &out,
    );
    let luma: Vec<f64> = first_stripe(&out).iter().map(|m| m[0]).collect();
    assert_eq!(
        luma,
        [
            96.0, 96.0, 92.0, 84.0, 76.0, 68.0, 60.0, 52.0, 44.0, 36.0, 28.0, 20.0, 16.0, 16.0
        ]
    );
}

#[test]
fn a_dissolve_inside_one_file_reads_it_twice_and_reads_on() {
    let codes = clip(VideoCodec::H264, Picture::Barcode);
    let p = plan(vec![
        piece(&codes, 0, 5),
        mix(piece(&codes, 5, 4), piece(&codes, 20, 4)),
        piece(&codes, 24, 5),
    ]);
    let dir = out_dir("one-file");
    let out = dir.path().join("one.mkv");
    export_on(
        &p,
        &settings(EncoderChoice::Cpu),
        &Default::default(),
        None,
        &out,
    );
    let mut r = VideoReader::open(&out).unwrap();
    let mut shown = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        shown.push(read_barcode(&f).unwrap());
    }
    let plain: Vec<Option<u32>> = shown[..5].iter().chain(&shown[9..]).copied().collect();
    let want: Vec<Option<u32>> = (0..5).chain(24..29).map(Some).collect();
    assert_eq!(plain, want);
}

fn envelope(path: &Path) -> Vec<f32> {
    use vidcut_av::ffmpeg::{codec, format, frame, media};
    vidcut_av::init();
    let mut input = format::input(path).unwrap();
    let stream = input.streams().best(media::Type::Audio).unwrap();
    let index = stream.index();
    let mut dec = codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .audio()
        .unwrap();
    let mut mono = Vec::new();
    let mut take = |dec: &mut vidcut_av::ffmpeg::decoder::Audio| {
        let mut f = frame::Audio::empty();
        while dec.receive_frame(&mut f).is_ok() {
            let (ch, d) = (f.channels() as usize, f.data(0));
            for i in 0..f.samples() {
                let v = i16::from_le_bytes([d[i * ch * 2], d[i * ch * 2 + 1]]);
                mono.push(f32::from(v) / 32768.0);
            }
        }
    };
    for (s, p) in input.packets() {
        if s.index() == index {
            dec.send_packet(&p).unwrap();
            take(&mut dec);
        }
    }
    dec.send_eof().unwrap();
    take(&mut dec);
    mono.chunks(48)
        .map(|w| w.iter().fold(0f32, |m, v| m.max(v.abs())))
        .collect()
}

#[test]
fn a_cross_fade_to_silence_follows_its_curve() {
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (25, 1), 40)
        .with_audio(AudioCodec::PcmS16, 48_000, 2)
        .sound(Sound::Tone);
    let tone = fixture::ensure(&spec).unwrap();
    let origin = vidcut_av::probe::probe(&tone, None)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    let sound = |source_sample, samples, fade| AudioPiece::Source {
        media: "m".into(),
        path: tone.clone(),
        stream: 0,
        origin,
        source_sample,
        samples,
        gain_db: 0.0,
        speed: 1.0,
        fade,
    };
    let fade = Fade {
        at: 0,
        of: 19_200,
        falling: true,
        curve: Curve::ZeroDb,
    };
    let mut p = plan(vec![VideoPiece::Black { frames: 20 }]);
    p.audio = vec![vec![
        sound(0, 9_600, None),
        sound(9_600, 19_200, Some(fade)),
        AudioPiece::Silence { samples: 9_600 },
    ]];
    let mut settings = settings(EncoderChoice::Cpu);
    settings.audio = Some(AudioSettings {
        codec: ACodec::PcmS16,
        bitrate: None,
        sample_rate: None,
        channels: None,
        loudness: None,
    });
    settings.declick = true;
    let dir = out_dir("sound");
    let out = dir.path().join("fade.mkv");
    export_on(&p, &settings, &Default::default(), None, &out);
    let peaks = envelope(&out);
    let whole = vidcut_av::fixture::TONE_AMPLITUDE;
    assert!(peaks[199] > whole * 0.98, "before the fade: {}", peaks[199]);
    assert!(peaks[200] > whole * 0.98, "as it starts: {}", peaks[200]);
    for (ms, gain) in [(300, 0.75), (400, 0.5), (500, 0.25)] {
        let got = peaks[ms] / whole;
        assert!((got - gain).abs() < 0.01, "{ms} ms: {got}, want {gain}");
    }
}

#[test]
fn nvenc_fades_as_the_cpu_does() {
    let Some(gpu) = vidcut_av::hw::GpuDecoder::best().filter(|g| g.cuda().is_some()) else {
        return;
    };
    let available = encode::detect(false);
    if !available
        .encoders
        .iter()
        .any(|e| e.ok && e.encoder == "h264_nvenc")
    {
        return;
    }
    let stripes = clip(VideoCodec::H264, Picture::Colors);
    let black = clip(VideoCodec::H264, Picture::Black);
    let p = plan(vec![
        mix(VideoPiece::Black { frames: 10 }, piece(&stripes, 0, 10)),
        mix(piece(&stripes, 10, 10), piece(&black, 0, 10)),
    ]);
    let dir = out_dir("nvenc");
    let (cpu, nvenc) = (dir.path().join("cpu.mkv"), dir.path().join("nvenc.mkv"));
    export_on(&p, &settings(EncoderChoice::Cpu), &available, None, &cpu);
    export_on(
        &p,
        &settings(EncoderChoice::Nvenc),
        &available,
        Some(&gpu),
        &nvenc,
    );
    let (a, b) = (first_stripe(&cpu), first_stripe(&nvenc));
    assert_eq!(a.len(), 20);
    assert_eq!(b.len(), 20);
    for (k, (x, y)) in a.iter().zip(&b).enumerate() {
        for c in 0..3 {
            assert!(
                (x[c] - y[c]).abs() <= 1.0,
                "frame {k}, plane {c}: CPU {x:?}, NVENC {y:?}"
            );
        }
    }
}
