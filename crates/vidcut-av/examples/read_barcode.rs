//! Prints the barcode frame index in each image or video frame file:
//! `cargo run -p vidcut-av --example read_barcode -- FILE...`
//! (e.g. to compare `vidcut gui screenshot` with `vidcut frame render`).

fn main() {
    for path in std::env::args().skip(1) {
        let index = vidcut_av::decode::VideoReader::open(std::path::Path::new(&path))
            .and_then(|mut r| r.next_frame())
            .and_then(|f| match f {
                Some(f) => vidcut_av::decode::read_barcode(&f),
                None => Ok(None),
            });
        match index {
            Ok(Some(i)) => println!("{path}: {i}"),
            Ok(None) => println!("{path}: no barcode"),
            Err(e) => println!("{path}: error: {e}"),
        }
    }
}
