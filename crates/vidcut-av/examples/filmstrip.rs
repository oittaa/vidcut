fn main() {
    let mut args = std::env::args().skip(1);
    let file = std::path::PathBuf::from(args.next().expect("FILE"));
    let out = std::path::PathBuf::from(args.next().expect("OUTDIR"));
    let count = args.next().map_or(24, |c| c.parse().expect("COUNT"));
    let gpu = vidcut_av::hw::GpuDecoder::best();
    let started = std::time::Instant::now();
    let input = vidcut_av::decode::Input::File(file.clone());
    let (images, times) =
        vidcut_av::thumbs::filmstrip(&input, count, 192, 108, gpu.as_ref(), Default::default())
            .unwrap();
    println!(
        "{} pictures ({}x{}) in {:.0} ms, decoding on {}",
        images.len(),
        images.first().map_or(0, |i| i.width),
        images.first().map_or(0, |i| i.height),
        started.elapsed().as_secs_f64() * 1e3,
        gpu.as_ref().map_or("the CPU", |d| d.kind().name())
    );
    std::fs::create_dir_all(&out).unwrap();
    for (i, (img, t)) in images.iter().zip(&times).enumerate() {
        let png = vidcut_av::export::still::png_rgba(img.width, img.height, &img.rgba).unwrap();
        std::fs::write(out.join(format!("{i:02}-{t:.1}s.png")), png).unwrap();
    }
}
