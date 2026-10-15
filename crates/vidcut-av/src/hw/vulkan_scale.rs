//! Vulkan's frames shrunk on the GPU before they come down to memory
//! ([`super::Shrinker`]): FFmpeg's `scale_vulkan` in halving steps, in the
//! frames' own layout. Its sampler is bilinear: it reads the four pixels
//! nearest each one it writes. Shrunk to less than half in one step, it
//! skips the pixels between and the picture aliases. Halved, it reads the
//! four pixels each one covers, their average. So frames are halved first,
//! as CUDA's are (export.md §5). A scaler is used once a test picture of
//! grays and colors comes back from it at the levels it went in at, as
//! Direct3D 11's video processor is (`d3d11::Scaler`).

use super::*;

/// The scaler of frames like `f` (of `frames`) to `size` (even): its passes
/// ([`passes`]), once they have given the test picture back at its levels.
pub(super) fn scaler(
    f: &frame::Video,
    frames: *mut ffi::AVBufferRef,
    size: (u32, u32),
) -> Result<GpuGraph> {
    let spec = passes((f.width(), f.height()), size)
        .iter()
        .map(|(w, h)| format!("scale_vulkan=w={w}:h={h}:scaler=bilinear"))
        .collect::<Vec<_>>()
        .join(",");
    let pixel = ffi::AVPixelFormat::AV_PIX_FMT_VULKAN;
    let mut graph = gpu_graph(f, frames, pixel, &spec)?;
    if !keeps_levels(&mut graph, f, frames, size)? {
        return Err(Error::Invalid("scale_vulkan changes the levels".into()));
    }
    Ok(graph)
}

/// The sizes of the passes that shrink frames of `from` to `to` (even):
/// halvings to even sides while both sides shrink at least twice, then one
/// to `to` where they don't reach it.
fn passes(from: (u32, u32), to: (u32, u32)) -> Vec<(u32, u32)> {
    let mut at = from;
    let mut sizes = Vec::new();
    while at.0 >= 2 * to.0 && at.1 >= 2 * to.1 && at.0 >= 4 && at.1 >= 4 {
        at = ((at.0 / 2) & !1, (at.1 / 2) & !1);
        sizes.push(at);
    }
    if at != to || sizes.is_empty() {
        sizes.push(to);
    }
    sizes
}

/// Whether `graph` gives back the test picture ([`test_picture`]) in the
/// layout of `f`'s frames, uploaded into a frame of `frames`, at the levels
/// it went in at ([`bands_kept`]). Too small to read at `size`, or in a
/// layout the test picture doesn't come in, the scaler isn't trusted.
fn keeps_levels(
    graph: &mut ffmpeg::filter::Graph,
    f: &frame::Video,
    frames: *mut ffi::AVBufferRef,
    size: (u32, u32),
) -> Result<bool> {
    if !readable(size) {
        return Ok(false);
    }
    let Some(picture) = test_picture(sw_format(f), (f.width(), f.height())) else {
        return Ok(false);
    };
    let mut uploaded = frame::Video::empty();
    // SAFETY: a frame of the frames' own pool, the picture's size, uploaded
    // into.
    unsafe {
        let ret = ffi::av_hwframe_get_buffer(frames, uploaded.as_mut_ptr(), 0);
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "getting a Vulkan frame to test with".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
        (*uploaded.as_mut_ptr()).width = f.width() as i32;
        (*uploaded.as_mut_ptr()).height = f.height() as i32;
        let ret = ffi::av_hwframe_transfer_data(uploaded.as_mut_ptr(), picture.as_ptr(), 0);
        if ret < 0 {
            return Err(Error::Ffmpeg {
                context: "uploading a test picture".into(),
                source: ffmpeg::Error::from(ret),
            });
        }
    }
    let scaled = run_graph(graph, &uploaded, "scaling a test picture on the GPU")?;
    Ok(bands_kept(&downloaded(&scaled)?))
}

/// The test picture in `layout` at `size`: [`BANDS`] side by side, each
/// component at its levels in its own bits. Luma (or red, green and blue)
/// and chroma take the bands' columns, alpha luma's. `None` for a layout of
/// samples under 8 bits, floats, a palette or a mosaic.
fn test_picture(layout: format::Pixel, size: (u32, u32)) -> Option<frame::Video> {
    let desc = described(layout)?;
    let mut picture = blank(layout, size);
    for c in 0..i32::from(desc.nb_components) {
        let (w, h) = component_size(desc, c, size);
        let line: Vec<u16> = (0..w)
            .map(|x| level(desc, c, BANDS[x * BANDS.len() / w]))
            .collect();
        for y in 0..h {
            write_line(&mut picture, c, y, &line);
        }
    }
    Some(picture)
}

/// Whether `got`, the test picture scaled, has each band's levels in the
/// middle of each band in each component, two samples across in its middle
/// rows, within a level of 8 bits.
fn bands_kept(got: &frame::Video) -> bool {
    let Some(desc) = described(got.format()) else {
        return false;
    };
    (0..i32::from(desc.nb_components)).all(|c| {
        let (w, h) = component_size(desc, c, (got.width(), got.height()));
        let near = level(desc, c, [1; 3]);
        let mut line = vec![0u16; w];
        (h / 4..h * 3 / 4).all(|y| {
            // SAFETY: the frame's own planes, `w` samples of component `c`
            // on row `y`, into a line of `w` 16-bit values.
            unsafe {
                let raw = got.as_ptr();
                ffi::av_read_image_line2(
                    line.as_mut_ptr().cast(),
                    (*raw).data.as_ptr().cast_mut().cast(),
                    (*raw).linesize.as_ptr(),
                    desc,
                    0,
                    y as i32,
                    c,
                    w as i32,
                    0,
                    2,
                );
            }
            BANDS.iter().enumerate().all(|(k, &band)| {
                let middle = (2 * k + 1) * w / (2 * BANDS.len());
                line[middle.saturating_sub(1)..=middle]
                    .iter()
                    .all(|v| v.abs_diff(level(desc, c, band)) <= near)
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shrink_to_a_quarter_is_two_halvings() {
        assert_eq!(passes((3840, 2160), (960, 540)), [(1920, 1080), (960, 540)]);
    }

    #[test]
    fn a_shrink_past_the_last_halving_ends_in_a_pass_to_the_size() {
        assert_eq!(
            passes((3840, 2160), (1280, 720)),
            [(1920, 1080), (1280, 720)]
        );
    }

    #[test]
    fn a_shrink_to_more_than_half_is_one_pass() {
        assert_eq!(passes((1920, 1080), (1280, 720)), [(1280, 720)]);
    }

    #[test]
    fn a_side_that_shrinks_less_than_twice_holds_the_halvings_back() {
        assert_eq!(passes((3840, 1080), (960, 720)), [(960, 720)]);
    }

    #[test]
    fn halvings_keep_even_sides() {
        assert_eq!(
            passes((1920, 1080), (192, 108)),
            [(960, 540), (480, 270), (240, 134), (192, 108)]
        );
    }

    #[test]
    fn the_test_picture_as_it_is_keeps_its_levels() {
        let picture = test_picture(format::Pixel::YUV422P10LE, (640, 360)).unwrap();
        assert!(bands_kept(&picture));
    }

    #[test]
    fn a_band_a_level_off_is_kept() {
        let mut picture = test_picture(format::Pixel::YUV420P, (640, 360)).unwrap();
        // Luma in the middle of the first band, black (16), a level up.
        let stride = picture.stride(0);
        picture.data_mut(0)[180 * stride + 31] = 17;
        assert!(bands_kept(&picture));
    }

    #[test]
    fn a_band_two_levels_off_is_changed() {
        let mut picture = test_picture(format::Pixel::YUV420P, (640, 360)).unwrap();
        let stride = picture.stride(0);
        picture.data_mut(0)[180 * stride + 31] = 18;
        assert!(!bands_kept(&picture));
    }

    #[test]
    fn chroma_two_levels_off_is_changed() {
        let mut picture = test_picture(format::Pixel::YUV422P10LE, (640, 360)).unwrap();
        // Cb in the middle of the last band, 128 at 10 bits (512), two
        // 8-bit levels up.
        let stride = picture.stride(1);
        let at = 180 * stride + 2 * 303;
        picture.data_mut(1)[at..at + 2].copy_from_slice(&520u16.to_le_bytes());
        assert!(!bands_kept(&picture));
    }

    #[test]
    fn floats_get_no_test_picture() {
        assert!(test_picture(format::Pixel::GRAYF32LE, (640, 360)).is_none());
    }
}
