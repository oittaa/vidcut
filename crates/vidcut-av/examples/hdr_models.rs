use vidcut_av::hdr_check::{Clip, Model, Row, coding_name, pictures, run, speed};
use vidcut_core::model::{Hdr, Retime, Upscaler};

fn clips() -> Vec<Clip> {
    let media = vidcut_av::testdata::root().join("media");
    [
        ("hdr10plus-croatia-av1", "B_1.mp4", 16.0),
        (
            "dolby-vision-84-hevc",
            "Test Jellyfin 1080p DV P8.4.mp4",
            8.0,
        ),
        ("bbb-1080p30", "bbb_sunflower_1080p_30fps_normal.mp4", 60.0),
    ]
    .into_iter()
    .map(|(name, file, start)| Clip {
        name: name.into(),
        path: media.join(name).join(file),
        start,
    })
    .collect()
}

fn line(r: &Row) {
    let (s, b) = (&r.score, &r.base);
    println!(
        "{:<22} {:<11} {:<4} {:<32} {:>6.2} {:>6.2} {:>6.2} {:>5.3} {:>6.2} | {:<11} {:>6.2} {:>6.2} {:>6.2} {:>5.3} {:>6.2} | {:>3}",
        r.clip,
        r.test,
        r.coding,
        r.model,
        s.de,
        s.low,
        s.mean,
        s.light,
        s.psnr,
        r.baseline,
        b.de,
        b.low,
        b.mean,
        b.light,
        b.psnr,
        r.frames
    );
}

fn models() -> Vec<Model> {
    if cfg!(target_os = "macos") {
        vec![
            Model::Upscaler(Upscaler::Macos),
            Model::Retime(Retime::Macos),
        ]
    } else {
        vec![
            Model::Upscaler(Upscaler::AiFast),
            Model::Upscaler(Upscaler::AiBest),
            Model::Retime(Retime::AiFast),
            Model::Retime(Retime::AiBest),
            Model::Retime(Retime::FrameBlend),
        ]
    }
}

fn name(m: Model) -> &'static str {
    match m {
        Model::Upscaler(u) => vidcut_av::super_scale::name(u),
        Model::Retime(r) => vidcut_av::interpolate::name(r),
    }
}

const FORMS: [(bool, Option<Hdr>); 3] = [(false, None), (true, None), (true, Some(Hdr::Pq))];

fn upscalers() -> Vec<Upscaler> {
    models()
        .into_iter()
        .filter_map(|m| match m {
            Model::Upscaler(u) => Some(u),
            Model::Retime(_) => None,
        })
        .collect()
}

fn retimes() -> Vec<Retime> {
    models()
        .into_iter()
        .filter_map(|m| match m {
            Model::Retime(r) if r != Retime::FrameBlend => Some(r),
            _ => None,
        })
        .collect()
}

fn one_speed(k: usize, form: usize) {
    let clip = &clips()[0];
    let model = models()[k];
    let pipe = coding_name(FORMS[form]);
    let dir = vidcut_core::dirs::models();
    let mb = |b: Option<u64>| b.map_or("n/a".into(), |b| format!("{:.0}", b as f64 / 1048576.0));
    match speed(clip, 30, (model, FORMS[form]), &dir) {
        Ok(s) => println!(
            "{:<32} {:<5} {:>8.1} {:>9} {:>9} {:>3}",
            name(model),
            pipe,
            s.per_frame.as_secs_f64() * 1e3,
            mb(s.resident),
            mb(s.peak),
            s.frames
        ),
        Err(e) => println!("{:<32} {:<5} error: {e}", name(model), pipe),
    }
}

fn speeds() {
    println!();
    println!(
        "Speed and GPU memory: {} from 16 s, each run in its own process",
        clips()[0].name
    );
    println!("ms/frame: one timed pass after a warm-up, until the GPU is idle");
    println!("resident MB: GPU memory the model keeps after a pass, outputs freed");
    println!("peak MB: highest GPU memory during the timed pass, with all its outputs held");
    println!("both above the level before the model first ran; Vulkan and DX12 only");
    println!(
        "{:<32} {:<5} {:>8} {:>9} {:>9} {:>3}",
        "model", "pipe", "ms/frame", "resident", "peak", "n"
    );
    let me = std::env::current_exe().expect("this program");
    for k in 0..models().len() {
        for form in 0..FORMS.len() {
            let out = std::process::Command::new(&me)
                .args(["speed", &k.to_string(), &form.to_string()])
                .output()
                .expect("a run of this program");
            print!("{}", String::from_utf8_lossy(&out.stdout));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "speed" {
        return one_speed(
            args[2].parse().expect("a model"),
            args[3].parse().expect("a pipe"),
        );
    }
    if args.get(1).is_some_and(|a| a == "speeds") {
        return speeds();
    }
    if args.len() == 3 && args[1] == "pictures" {
        let out = std::path::Path::new(&args[2]);
        std::fs::create_dir_all(out).expect("a folder for the pictures");
        for clip in &clips()[..2] {
            let made = pictures(clip, 30, 15, &upscalers(), &vidcut_core::dirs::models())
                .expect("pictures");
            for (name, rgb) in made {
                let file = format!(
                    "{}-{}.rgb48",
                    clip.name,
                    name.split(' ').take(2).collect::<Vec<_>>().join("-")
                );
                std::fs::write(out.join(&file), rgb).expect("a picture file");
                println!("{file}: 1920x1080 RGB48LE");
            }
        }
        return;
    }
    let only = args.get(1).map(String::as_str);
    let upscalers = match only {
        Some("retime") => Vec::new(),
        _ => upscalers(),
    };
    let retimes = match only {
        Some("scale") => Vec::new(),
        _ => retimes(),
    };
    let dir = vidcut_core::dirs::models();
    println!("pipes: PQ or HLG and SDR at 16 bits, SDR8 at 8 bits; references drawn at 1920x1080");
    println!(
        "dE: mean dE ITP (BT.2124), lower is better; PSNR in dB on code values, higher is better"
    );
    println!("low: mean dE ITP of the 40x40 block means; mean: dE ITP of the frame means");
    println!("Y: mean luminance over the mean luminance of the real frame");
    println!(
        "{:<22} {:<11} {:<4} {:<32} {:>6} {:>6} {:>6} {:>5} {:>6} | {:<11} {:>6} {:>6} {:>6} {:>5} {:>6} | {:>3}",
        "clip",
        "test",
        "code",
        "model",
        "dE",
        "low",
        "mean",
        "Y",
        "PSNR",
        "baseline",
        "dE",
        "low",
        "mean",
        "Y",
        "PSNR",
        "n"
    );
    for clip in clips() {
        if !clip.path.exists() {
            eprintln!(
                "{}: missing; get it with scripts/fetch-test-media.sh {}",
                clip.path.display(),
                clip.name
            );
            continue;
        }
        match run(&clip, 30, (&upscalers, &retimes), &dir) {
            Ok(rows) => rows.iter().for_each(line),
            Err(e) => eprintln!("{}: {e}", clip.name),
        }
    }
}
