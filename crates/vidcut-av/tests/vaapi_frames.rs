use vidcut_av::decode::{Input, VideoReader};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};

fn first_frame(gpu: &GpuDecoder, frames: u32) -> vidcut_av::ffmpeg::frame::Video {
    let spec = FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        320,
        180,
        (30, 1),
        frames,
    )
    .no_audio();
    let path = fixture::ensure(&spec).unwrap();
    let mut reader = VideoReader::open_on(&Input::File(path), gpu).unwrap();
    reader.next_frame().unwrap().unwrap()
}

fn frames_of(f: &vidcut_av::ffmpeg::frame::Video) -> usize {
    // SAFETY: reads the frame's own field.
    unsafe { (*f.as_ptr()).hw_frames_ctx as usize }
}

#[test]
fn decoders_of_two_files_alike_decode_into_the_same_frames() {
    let Some(gpu) = GpuDecoder::best().filter(|g| g.kind() == Kind::Vaapi) else {
        eprintln!("skipping: the GPU decoder in use isn't VA-API");
        return;
    };
    let first = first_frame(&gpu, 10);
    let second = first_frame(&gpu, 11);
    // SAFETY: both frames' frames contexts.
    let same = unsafe {
        (*(*first.as_ptr()).hw_frames_ctx).data == (*(*second.as_ptr()).hw_frames_ctx).data
    };
    assert!(same, "{:x} and {:x}", frames_of(&first), frames_of(&second));
}
