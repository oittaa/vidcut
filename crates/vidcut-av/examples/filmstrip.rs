//! Times filmstrip extraction, says the frames copied down from the GPU,
//! and writes the pictures as PNGs, decoded on the GPU the viewer decodes
//! on (`VIDCUT_HWDEC` picks another, `none` the CPU):
//! `cargo run -p vidcut-av --example filmstrip -- FILE OUTDIR [COUNT [WxH]]`
//! (the pictures fit in 192x108 unless WxH says another box).

fn main() {
    let mut args = std::env::args().skip(1);
    let file = std::path::PathBuf::from(args.next().expect("FILE"));
    let out = std::path::PathBuf::from(args.next().expect("OUTDIR"));
    let count = args.next().map_or(24, |c| c.parse().expect("COUNT"));
    let (max_w, max_h) = args.next().map_or((192, 108), |b| {
        let (w, h) = b.split_once('x').expect("WxH");
        (w.parse().expect("W"), h.parse().expect("H"))
    });
    let gpu = vidcut_av::hw::GpuDecoder::best();
    let copies = std::sync::Arc::new(vidcut_av::copies::Copies::default());
    let counting = vidcut_av::copies::Guard::set(copies.clone());
    let started = std::time::Instant::now();
    let input = vidcut_av::decode::Input::File(file.clone());
    let (images, times) =
        vidcut_av::thumbs::filmstrip(&input, count, max_w, max_h, gpu.as_ref()).unwrap();
    let elapsed = started.elapsed();
    drop(counting);
    let copied = copies.counted();
    println!(
        "{} pictures ({}x{}) in {:.0} ms, decoding on {}; copied down {} frames ({:.1} MB)",
        images.len(),
        images.first().map_or(0, |i| i.width),
        images.first().map_or(0, |i| i.height),
        elapsed.as_secs_f64() * 1e3,
        gpu.as_ref().map_or("the CPU", |d| d.kind().name()),
        copied.down,
        copied.down_bytes as f64 / 1e6
    );
    std::fs::create_dir_all(&out).unwrap();
    for (i, (img, t)) in images.iter().zip(&times).enumerate() {
        let png = vidcut_av::export::still::png_rgba(img.width, img.height, &img.rgba).unwrap();
        std::fs::write(out.join(format!("{i:02}-{t:.1}s.png")), png).unwrap();
    }
}
