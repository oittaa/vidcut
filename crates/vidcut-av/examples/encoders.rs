//! Prints which GPU encoders work on this machine (test encodes).
fn main() {
    let started = std::time::Instant::now();
    let a = vidcut_av::export::encode::detect();
    for e in &a.encoders {
        println!(
            "{:<11} {:<6} {:<8} {:<19} {}",
            e.encoder,
            format!("{:?}", e.kind).to_lowercase(),
            e.pixel,
            e.device.as_deref().unwrap_or("-"),
            if e.ok {
                "ok".to_string()
            } else {
                format!("no: {}", e.error.as_deref().unwrap_or(""))
            }
        );
    }
    eprintln!("detected in {:.2} s", started.elapsed().as_secs_f64());
}
