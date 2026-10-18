use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};

#[test]
fn a_file_being_read_can_be_renamed_and_deleted() {
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 90).no_audio();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("open.mp4");
    std::fs::copy(fixture::ensure(&spec).unwrap(), &path).unwrap();
    let mut reader = VideoReader::open(&path).unwrap();
    let mut last = reader.next_frame().unwrap().unwrap();
    let moved = dir.path().join("moved.mp4");
    std::fs::rename(&path, &moved).unwrap();
    std::fs::remove_file(&moved).unwrap();
    while let Some(frame) = reader.next_frame().unwrap() {
        last = frame;
    }
    assert_eq!(read_barcode(&last).unwrap(), Some(89));
}
