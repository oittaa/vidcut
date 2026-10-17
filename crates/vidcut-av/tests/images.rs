use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::export::{self, encode::Availability};
use vidcut_av::fixture::{self, ImageSpec};
use vidcut_core::export::{
    Container as OutContainer, EncoderChoice, ExportSettings, HdrMode, VideoCodec as VCodec,
    VideoSettings,
};
use vidcut_core::render::{Image, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

const W: u32 = 320;
const H: u32 = 180;
const FPS: i64 = 24;

fn scratch(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("vidcut-images-{name}-"))
        .tempdir()
        .unwrap()
}

fn place(spec: &ImageSpec, dir: &Path) -> (PathBuf, Vec<PathBuf>) {
    let src = fixture::ensure_images(spec).unwrap();
    let mut files = Vec::new();
    for f in &src {
        let dest = dir.join(f.file_name().unwrap());
        std::fs::copy(f, &dest).unwrap();
        files.push(dest);
    }
    let pattern = dir.join(format!("img_%0{}d.png", spec.digits));
    (pattern, files)
}

fn sequence(pattern: &Path, first: u64, count: u64) -> Image {
    Image::Sequence {
        pattern: pattern.to_path_buf(),
        first,
        count,
        rate: Rational::new(FPS, 1).unwrap(),
    }
}

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

fn plan(video: Vec<VideoPiece>) -> RenderPlan {
    let frames = video.iter().map(VideoPiece::frames).sum();
    let fps = Rational::new(FPS, 1).unwrap();
    RenderPlan {
        fps,
        width: W,
        height: H,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video,
        audio: vec![],
        range: (0, frames),
        name: "t".into(),
    }
}

fn settings() -> ExportSettings {
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
        audio: None,
        declick: false,
    }
}

fn try_render(plan: &RenderPlan, out: &Path) -> vidcut_av::Result<export::Report> {
    export::run(
        plan,
        &settings(),
        out,
        &Availability::default(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
}

fn render(plan: &RenderPlan, out: &Path) {
    try_render(plan, out).unwrap_or_else(|e| panic!("export: {e}"));
}

fn barcodes(path: &Path) -> Vec<Option<u32>> {
    let mut r = VideoReader::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        out.push(read_barcode(&f).unwrap());
    }
    out
}

fn assert_fails(plan: &RenderPlan, out: &Path) {
    try_render(plan, out).expect_err("export succeeded");
    assert!(!out.exists(), "left {}", out.display());
}

#[test]
fn missing_first_file_fails_the_open() {
    let spec = ImageSpec::sequence(10, 5, 4, W, H);
    let dir = scratch("missing-first");
    let (pattern, files) = place(&spec, dir.path());
    std::fs::remove_file(&files[0]).unwrap();
    assert!(files[1].exists(), "the next file is there");

    let probed = vidcut_av::probe::probe_sequence(&pattern, 10, 5, (FPS as i32, 1));
    assert!(
        probed.is_err(),
        "probed despite a missing start: {probed:?}"
    );

    let out = dir.path().join("out.mkv");
    assert_fails(
        &plan(vec![pictures(&files[0], sequence(&pattern, 10, 5), 0, 5)]),
        &out,
    );
}

#[test]
fn a_hole_fails_the_export_instead_of_repeating() {
    let spec = ImageSpec::sequence(10, 8, 4, W, H);
    let dir = scratch("hole");
    let (pattern, files) = place(&spec, dir.path());
    std::fs::remove_file(&files[4]).unwrap();
    assert!(files[5].exists(), "a file after the hole is there");

    let info = vidcut_av::probe::probe_sequence(&pattern, 10, 8, (FPS as i32, 1)).unwrap();
    let v = info.video.unwrap();
    assert_eq!(v.frames, Some(4), "a hole is not the caller's 8 frames");
    assert!((v.duration_secs.unwrap() - 4.0 / FPS as f64).abs() < 1e-9);

    let image = sequence(&pattern, 10, 8);
    let out = dir.path().join("prefix.mkv");
    render(&plan(vec![pictures(&files[0], image.clone(), 0, 4)]), &out);
    assert_eq!(barcodes(&out), (0..4).map(Some).collect::<Vec<_>>());

    assert_fails(
        &plan(vec![pictures(&files[0], image, 0, 8)]),
        &dir.path().join("out.mkv"),
    );
}

#[test]
fn a_sequence_slower_than_the_project_ends_on_its_last_picture() {
    for (rate, project) in [(30, 60), (12, 30)] {
        let spec = ImageSpec::sequence(1, rate, 4, W, H);
        let dir = scratch(&format!("slower-{rate}-{project}"));
        let (pattern, files) = place(&spec, dir.path());
        let image = Image::Sequence {
            pattern,
            first: 1,
            count: u64::from(rate),
            rate: Rational::new(i64::from(rate), 1).unwrap(),
        };
        let mut p = plan(vec![pictures(&files[0], image, 0, i64::from(project))]);
        p.fps = Rational::new(i64::from(project), 1).unwrap();
        let out = dir.path().join("out.mkv");
        render(&p, &out);
        let codes: Vec<u32> = barcodes(&out).into_iter().map(Option::unwrap).collect();
        assert_eq!(codes.len(), project as usize, "{rate} fps in {project}");
        let mut shown = codes.clone();
        shown.dedup();
        assert_eq!(
            shown,
            (0..rate).collect::<Vec<_>>(),
            "{rate} in {project}: {codes:?}"
        );
    }
}
