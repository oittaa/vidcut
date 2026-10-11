//! Image sequences and stills through probe and export.
//!
//! A missing first file fails the open. A hole or a run shorter than the
//! piece fails the export instead of repeating the last picture. A real run
//! maps picture `k` to file `first + k`. A still repeats its one picture.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::export::{self, encode::Availability};
use vidcut_av::fixture::{self, ImageSpec};
use vidcut_core::export::{
    Container as OutContainer, EncoderChoice, ExportSettings, VideoCodec as VCodec, VideoSettings,
};
use vidcut_core::render::{Image, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

const W: u32 = 320;
const H: u32 = 180;
const FPS: i64 = 24;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vidcut-images-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Copies `spec` into `dir` and returns image2's pattern plus the files.
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
            hdr: None,
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

fn assert_fails(plan: &RenderPlan, out: &Path, needle: &str) {
    let msg = try_render(plan, out)
        .expect_err("export succeeded")
        .to_string();
    assert!(msg.contains(needle), "{msg}");
    assert!(!out.exists(), "left {}", out.display());
}

#[test]
fn missing_first_file_fails_the_open() {
    // image2's default start_number_range is 5, so a missing img_0010.png
    // would open img_0011.png as picture 0. The range is 1: the open fails.
    let spec = ImageSpec::sequence(10, 5, 4, W, H);
    let dir = scratch("missing-first");
    let (pattern, files) = place(&spec, &dir);
    std::fs::remove_file(&files[0]).unwrap();
    assert!(files[1].exists(), "the next file is there");

    let opened = Input::Sequence {
        pattern: pattern.clone(),
        first: 10,
        rate: (FPS as i32, 1),
    }
    .open();
    assert!(opened.is_err(), "opened despite a missing start");

    let probed = vidcut_av::probe::probe_sequence(&pattern, 10, 5, (FPS as i32, 1));
    assert!(
        probed.is_err(),
        "probed despite a missing start: {probed:?}"
    );

    let out = dir.join("out.mkv");
    assert_fails(
        &plan(vec![pictures(&files[0], sequence(&pattern, 10, 5), 0, 5)]),
        &out,
        "opening",
    );
}

#[test]
fn a_hole_fails_the_export_instead_of_repeating() {
    // Files 10..17 with 14 missing. image2 stops at the hole (the search
    // checks the next number first), so the run is 4 pictures, not 8.
    let spec = ImageSpec::sequence(10, 8, 4, W, H);
    let dir = scratch("hole");
    let (pattern, files) = place(&spec, &dir);
    std::fs::remove_file(&files[4]).unwrap();
    assert!(files[5].exists(), "a file after the hole is there");

    let info = vidcut_av::probe::probe_sequence(&pattern, 10, 8, (FPS as i32, 1)).unwrap();
    let v = info.video.unwrap();
    assert_eq!(v.frames, Some(4), "a hole is not the caller's 8 frames");
    assert!((v.duration_secs.unwrap() - 4.0 / FPS as f64).abs() < 1e-9);

    let image = sequence(&pattern, 10, 8);
    let out = dir.join("prefix.mkv");
    render(&plan(vec![pictures(&files[0], image.clone(), 0, 4)]), &out);
    assert_eq!(barcodes(&out), (0..4).map(Some).collect::<Vec<_>>());

    assert_fails(
        &plan(vec![pictures(&files[0], image, 0, 8)]),
        &dir.join("out.mkv"),
        "ends before",
    );
}

#[test]
fn a_short_run_is_not_the_requested_length() {
    // Two files, asked for as sixty. The probe used to report 60, and the
    // export then repeated the last picture out to the end of the piece.
    let spec = ImageSpec::sequence(100, 2, 4, W, H);
    let dir = scratch("short");
    let (pattern, files) = place(&spec, &dir);

    let info = vidcut_av::probe::probe_sequence(&pattern, 100, 60, (FPS as i32, 1)).unwrap();
    let v = info.video.unwrap();
    assert_eq!(v.frames, Some(2));
    assert_eq!(v.avg_frame_rate, (FPS as i32, 1));
    assert!((v.duration_secs.unwrap() - 2.0 / FPS as f64).abs() < 1e-9);
    assert!((info.duration.unwrap() - 2.0 / FPS as f64).abs() < 1e-9);

    let image = sequence(&pattern, 100, 60);
    let out = dir.join("fit.mkv");
    render(&plan(vec![pictures(&files[0], image.clone(), 0, 2)]), &out);
    assert_eq!(barcodes(&out), vec![Some(0), Some(1)]);

    assert_fails(
        &plan(vec![pictures(&files[0], image, 0, 10)]),
        &dir.join("out.mkv"),
        "ends before",
    );
}

#[test]
fn a_contiguous_sequence_maps_picture_k_to_its_file() {
    // Picture k is file first+k, barcode k (the fixture paints k).
    let first = 1000u64;
    let spec = ImageSpec::sequence(first, 12, 4, W, H);
    let dir = scratch("contiguous");
    let (pattern, files) = place(&spec, &dir);
    for (k, file) in files.iter().enumerate() {
        let n = first + k as u64;
        assert_eq!(
            file.file_name().unwrap().to_str().unwrap(),
            format!("img_{n:04}.png")
        );
        let mut r = VideoReader::open(file).unwrap();
        let frame = r.next_frame().unwrap().unwrap();
        assert_eq!(read_barcode(&frame).unwrap(), Some(k as u32));
    }

    let info = vidcut_av::probe::probe_sequence(&pattern, first, 12, (FPS as i32, 1)).unwrap();
    let v = info.video.unwrap();
    assert_eq!((v.frames, v.avg_frame_rate), (Some(12), (FPS as i32, 1)));
    assert!((v.duration_secs.unwrap() - 12.0 / FPS as f64).abs() < 1e-9);
    // A request longer than the run reports the run; a shorter one keeps
    // the caller's count.
    assert_eq!(
        vidcut_av::probe::probe_sequence(&pattern, first, 60, (FPS as i32, 1))
            .unwrap()
            .video
            .unwrap()
            .frames,
        Some(12)
    );
    assert_eq!(
        vidcut_av::probe::probe_sequence(&pattern, first, 4, (FPS as i32, 1))
            .unwrap()
            .video
            .unwrap()
            .frames,
        Some(4)
    );

    let mut reader = VideoReader::open_input(&Input::Sequence {
        pattern: pattern.clone(),
        first,
        rate: (FPS as i32, 1),
    })
    .unwrap();
    for k in 0..12 {
        let frame = reader.next_frame().unwrap().unwrap();
        assert_eq!(read_barcode(&frame).unwrap(), Some(k), "picture {k}");
    }
    assert!(reader.next_frame().unwrap().is_none());

    let image = sequence(&pattern, first, 12);
    let out = dir.join("out.mkv");
    render(
        &plan(vec![
            pictures(&files[0], image.clone(), 0, 6),
            pictures(&files[0], image, 3, 4),
        ]),
        &out,
    );
    let mut want: Vec<Option<u32>> = (0..6).map(Some).collect();
    want.extend((3..7).map(Some));
    assert_eq!(barcodes(&out), want);
}

#[test]
fn a_still_repeats_its_one_picture() {
    // Every frame is the one picture, including a piece whose source frame
    // is far past it. The file is not a sequence, so the end does not fail
    // the export.
    let png = fixture::ensure_images(&ImageSpec::still(41, W, H, "png")).unwrap();
    let dir = scratch("still");
    let out = dir.join("out.mkv");
    render(
        &plan(vec![
            pictures(&png[0], Image::Still, 0, 6),
            pictures(&png[0], Image::Still, 90_000, 4),
        ]),
        &out,
    );
    assert_eq!(barcodes(&out), vec![Some(41); 10]);
}

#[test]
fn a_sequence_slower_than_the_project_ends_on_its_last_picture() {
    // A project rate above the sequence's samples the last output frames
    // just past the last picture's time: that picture lasts its frame, as a
    // video's does. 30 pictures at 30 fps in 60, and 12 at 12 fps in 30:
    // each one second, each picture shown in turn, the last one at the end.
    for (rate, project) in [(30, 60), (12, 30)] {
        let spec = ImageSpec::sequence(1, rate, 4, W, H);
        let dir = scratch(&format!("slower-{rate}-{project}"));
        let (pattern, files) = place(&spec, &dir);
        let image = Image::Sequence {
            pattern,
            first: 1,
            count: u64::from(rate),
            rate: Rational::new(i64::from(rate), 1).unwrap(),
        };
        let mut p = plan(vec![pictures(&files[0], image, 0, i64::from(project))]);
        p.fps = Rational::new(i64::from(project), 1).unwrap();
        let out = dir.join("out.mkv");
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
