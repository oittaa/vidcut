use std::collections::{HashSet, VecDeque};
use std::ffi::{c_int, c_void};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::{Error, Result};
use crate::ffmpeg::{self, codec, ffi, format, frame};

const SPARE: usize = 64 << 20;

pub(crate) fn packed(f: &frame::Video, bytes: usize) -> Vec<u8> {
    let (row, h) = (f.width() as usize * bytes, f.height() as usize);
    let (stride, data) = (f.stride(0), f.data(0));
    let mut out = Vec::with_capacity(row * h);
    for y in 0..h {
        out.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    out
}

pub(crate) fn unpacked(
    pixel: format::Pixel,
    (w, h): (u32, u32),
    bytes: usize,
    rows: &[u8],
) -> frame::Video {
    let mut f = frame::Video::new(pixel, w, h);
    let (row, stride) = (w as usize * bytes, f.stride(0));
    let data = f.data_mut(0);
    for (y, line) in rows.chunks_exact(row).enumerate() {
        data[y * stride..y * stride + row].copy_from_slice(line);
    }
    f
}

static SPARES: Mutex<Spares> = Mutex::new(Spares::new());

fn spares() -> MutexGuard<'static, Spares> {
    SPARES.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn own(ctx: &mut codec::context::Context) {
    // SAFETY: the context isn't opened yet.
    unsafe { (*ctx.as_mut_ptr()).get_buffer2 = Some(get_buffer) };
}

unsafe extern "C" fn get_buffer(
    ctx: *mut ffi::AVCodecContext,
    frame: *mut ffi::AVFrame,
    flags: c_int,
) -> c_int {
    // SAFETY: FFmpeg's open context and empty frame of set size; the frame owns each buffer.
    unsafe {
        let dr1 = (*ctx)
            .codec
            .as_ref()
            .is_some_and(|c| c.capabilities & ffi::AV_CODEC_CAP_DR1 as c_int != 0);
        let video = (*ctx).codec_type == ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
        let ours = (video && dr1 && (*ctx).hw_frames_ctx.is_null())
            .then(|| layout(ctx, (*frame).width, (*frame).height))
            .flatten()
            .filter(|(_, size, _)| size[0] > 0);
        let Some((linesize, size, padding)) = ours else {
            return ffi::avcodec_default_get_buffer2(ctx, frame, flags);
        };
        let planes = size.iter().take_while(|&&s| s > 0).count();
        for i in 0..planes {
            let buf = plane(size[i] + padding);
            if buf.is_null() {
                ffi::av_frame_unref(frame);
                return ffi::AVERROR(ffi::ENOMEM);
            }
            (*frame).buf[i] = buf;
            (*frame).data[i] = (*buf).data;
            (*frame).linesize[i] = linesize[i];
        }
        for i in planes..ffi::AV_NUM_DATA_POINTERS as usize {
            (*frame).data[i] = std::ptr::null_mut();
            (*frame).linesize[i] = 0;
        }
        (*frame).extended_data = (*frame).data.as_mut_ptr();
        0
    }
}

pub(crate) fn pixel_format(n: c_int) -> Option<ffi::AVPixelFormat> {
    let min = ffi::AVPixelFormat::AV_PIX_FMT_NONE as c_int;
    let max = ffi::AVPixelFormat::AV_PIX_FMT_NB as c_int;
    if n < min || n > max {
        return None;
    }
    // SAFETY: the discriminants from NONE through NB are contiguous.
    Some(unsafe { std::mem::transmute::<c_int, ffi::AVPixelFormat>(n) })
}

pub(crate) fn owned(f: &frame::Video) -> Result<frame::Video> {
    if freed_with(f) {
        return crate::export::video::frame_ref(f);
    }
    let failed = |what: &str, ret: c_int| Error::Ffmpeg {
        context: what.into(),
        source: ffmpeg::Error::from(ret),
    };
    let mut out = frame::Video::empty();
    // SAFETY: `out` is fresh, given planes sized for `f`'s format and size; `f` is in memory.
    unsafe {
        let (src, dst) = (f.as_ptr(), out.as_mut_ptr());
        let format = pixel_format((*src).format)
            .ok_or_else(|| Error::Invalid("a kept picture's format is not known".into()))?;
        let mut linesize = [0; 4];
        let ret = ffi::av_image_fill_linesizes(linesize.as_mut_ptr(), format, (*src).width);
        if ret < 0 {
            return Err(failed("laying out a kept picture", ret));
        }
        let linesize = linesize.map(|l| (l + 63) & !63);
        let rows = linesize.map(|l| l as isize);
        let mut size = [0usize; 4];
        let ret =
            ffi::av_image_fill_plane_sizes(size.as_mut_ptr(), format, (*src).height, rows.as_ptr());
        if ret < 0 {
            return Err(failed("laying out a kept picture", ret));
        }
        (*dst).format = (*src).format;
        (*dst).width = (*src).width;
        (*dst).height = (*src).height;
        for i in 0..size.iter().take_while(|&&s| s > 0).count() {
            let buf = plane(size[i] + 64);
            if buf.is_null() {
                return Err(Error::Invalid("no memory for a kept picture".into()));
            }
            (*dst).buf[i] = buf;
            (*dst).data[i] = (*buf).data;
            (*dst).linesize[i] = linesize[i];
        }
        (*dst).extended_data = (*dst).data.as_mut_ptr();
        let ret = ffi::av_frame_copy(dst, src);
        if ret < 0 {
            return Err(failed("copying a kept picture", ret));
        }
        let ret = ffi::av_frame_copy_props(dst, src);
        if ret < 0 {
            return Err(failed("copying a kept picture's properties", ret));
        }
    }
    Ok(out)
}

pub(crate) fn give_back() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    trim::ask();
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod trim {
    use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
    use std::time::Duration;

    static ASKED: Mutex<bool> = Mutex::new(false);
    static WAKE: Condvar = Condvar::new();
    static THREAD: OnceLock<()> = OnceLock::new();

    pub(super) fn ask() {
        THREAD.get_or_init(|| {
            let _ = std::thread::Builder::new()
                .name("vidcut-give-back".into())
                .spawn(run);
        });
        *ASKED.lock().unwrap_or_else(PoisonError::into_inner) = true;
        WAKE.notify_one();
    }

    fn run() {
        loop {
            let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
            while !*asked {
                asked = WAKE.wait(asked).unwrap_or_else(PoisonError::into_inner);
            }
            *asked = false;
            drop(asked);
            // SAFETY: glibc's call, which takes its own locks.
            unsafe { libc::malloc_trim(0) };
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn freed_with(f: &frame::Video) -> bool {
    // SAFETY: reads the frame's own buffer references.
    unsafe {
        let raw = &*f.as_ptr();
        let mut spares = spares();
        raw.buf.iter().filter(|b| !b.is_null()).all(|&b| {
            ffi::av_buffer_get_opaque(b).is_null() || spares.live().contains(&((*b).data as usize))
        }) && raw.extended_buf.is_null()
    }
}

unsafe fn layout(
    ctx: *mut ffi::AVCodecContext,
    width: c_int,
    height: c_int,
) -> Option<([c_int; 4], [usize; 4], usize)> {
    let (mut w, mut h) = (width, height);
    let mut align = [0; ffi::AV_NUM_DATA_POINTERS as usize];
    // SAFETY: the caller's context; FFmpeg fills four alignments.
    unsafe { ffi::avcodec_align_dimensions2(ctx, &mut w, &mut h, align.as_mut_ptr()) };
    let pixel = unsafe { (*ctx).pix_fmt };
    if w <= 0 || align[..4].iter().any(|&a| a <= 0) {
        return None;
    }
    let mut linesize = [0; 4];
    loop {
        // SAFETY: room for four.
        if unsafe { ffi::av_image_fill_linesizes(linesize.as_mut_ptr(), pixel, w) } < 0 {
            return None;
        }
        if linesize.iter().zip(align).all(|(l, a)| l % a == 0) {
            break;
        }
        w = w.checked_add(w.isolate_lowest_one())?;
    }
    let rows = linesize.map(|l| l as isize);
    let mut size = [0; 4];
    // SAFETY: room for four, from four.
    if unsafe { ffi::av_image_fill_plane_sizes(size.as_mut_ptr(), pixel, h, rows.as_ptr()) } < 0 {
        return None;
    }
    let padding = 16 + align[0] as usize - 1;
    if size.iter().any(|&s| s > c_int::MAX as usize - padding) {
        return None;
    }
    Some((linesize, size, padding))
}

fn plane(size: usize) -> *mut ffi::AVBufferRef {
    let spare = spares().take(size);
    let data = spare.map_or_else(|| alloc(size), |p| p as *mut u8);
    if data.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the buffer owns `size` bytes at `data`, given back by `let_go` with `size` as opaque.
    let buf = unsafe { ffi::av_buffer_create(data, size, Some(let_go), size as *mut c_void, 0) };
    if buf.is_null() {
        // SAFETY: memory nothing refers to.
        unsafe { free(data, size) };
    } else {
        spares().live().insert(data as usize);
    }
    buf
}

unsafe extern "C" fn let_go(size: *mut c_void, data: *mut u8) {
    let size = size as usize;
    let gone = {
        let mut spares = spares();
        spares.live().remove(&(data as usize));
        spares.give(size, data as usize, SPARE)
    };
    for (size, p) in gone {
        // SAFETY: a plane's memory, its buffer gone and no spare any more.
        unsafe { free(p as *mut u8, size) };
    }
}

fn alloc(size: usize) -> *mut u8 {
    #[cfg(unix)]
    // SAFETY: a new private mapping.
    unsafe {
        use libc::{MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_READ, PROT_WRITE};
        let flags = MAP_PRIVATE | MAP_ANONYMOUS;
        let p = libc::mmap(
            std::ptr::null_mut(),
            size,
            PROT_READ | PROT_WRITE,
            flags,
            -1,
            0,
        );
        if p == MAP_FAILED {
            std::ptr::null_mut()
        } else {
            p.cast()
        }
    }
    #[cfg(not(unix))]
    // SAFETY: FFmpeg's allocation.
    unsafe {
        ffi::av_mallocz(size).cast()
    }
}

unsafe fn free(p: *mut u8, size: usize) {
    #[cfg(unix)]
    // SAFETY: the caller's mapping of `size` bytes.
    unsafe {
        libc::munmap(p.cast(), size);
    }
    #[cfg(not(unix))]
    // SAFETY: the caller's allocation.
    unsafe {
        let _ = size;
        ffi::av_free(p.cast());
    }
}

struct Spares {
    bytes: usize,
    planes: VecDeque<(usize, usize)>,
    live: Option<HashSet<usize>>,
}

impl Spares {
    const fn new() -> Self {
        Self {
            bytes: 0,
            planes: VecDeque::new(),
            live: None,
        }
    }

    fn live(&mut self) -> &mut HashSet<usize> {
        self.live.get_or_insert_with(HashSet::new)
    }

    fn take(&mut self, size: usize) -> Option<usize> {
        let i = self.planes.iter().rposition(|&(s, _)| s == size)?;
        let (_, p) = self.planes.remove(i)?;
        self.bytes -= size;
        Some(p)
    }

    fn give(&mut self, size: usize, p: usize, most: usize) -> Vec<(usize, usize)> {
        if size > most {
            return vec![(size, p)];
        }
        let mut gone = Vec::new();
        while self.bytes + size > most {
            let Some((s, q)) = self.planes.pop_front() else {
                break;
            };
            self.bytes -= s;
            gone.push((s, q));
        }
        self.planes.push_back((size, p));
        self.bytes += size;
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::{Spares, layout, owned, pixel_format};
    use crate::ffmpeg::{codec, decoder, ffi, format::Pixel, frame};

    #[test]
    fn a_pixel_format_past_the_list_is_refused() {
        let past = ffi::AVPixelFormat::AV_PIX_FMT_NB as i32 + 1;
        assert_eq!(pixel_format(past), None);
    }

    #[test]
    fn yuv420p_is_a_pixel_format() {
        assert_eq!(
            pixel_format(0),
            Some(ffi::AVPixelFormat::AV_PIX_FMT_YUV420P)
        );
    }

    fn pooled() -> frame::Video {
        let mut f = frame::Video::empty();
        // SAFETY: a fresh frame owns a pool buffer of its size; the pool goes with that buffer.
        unsafe {
            let raw = f.as_mut_ptr();
            let pix = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P;
            let size = ffi::av_image_get_buffer_size(pix, 4, 4, 1) as usize;
            let mut pool = ffi::av_buffer_pool_init(size, None);
            let buf = ffi::av_buffer_pool_get(pool);
            ffi::av_buffer_pool_uninit(&mut pool);
            (*raw).format = pix as i32;
            (*raw).width = 4;
            (*raw).height = 4;
            (*raw).buf[0] = buf;
            ffi::av_image_fill_arrays(
                (*raw).data.as_mut_ptr(),
                (*raw).linesize.as_mut_ptr(),
                (*buf).data,
                pix,
                4,
                4,
                1,
            );
            let data = std::slice::from_raw_parts_mut((*buf).data, size);
            for (i, b) in data.iter_mut().enumerate() {
                *b = if i < 16 { i as u8 + 1 } else { 128 };
            }
        }
        f
    }

    #[test]
    fn a_frame_in_plain_memory_is_kept_as_it_is() {
        let f = frame::Video::new(Pixel::YUV420P, 4, 4);
        assert_eq!(owned(&f).unwrap().data(0).as_ptr(), f.data(0).as_ptr());
    }

    #[test]
    fn a_frame_from_a_pool_is_copied_into_memory_of_its_own() {
        let f = pooled();
        let kept = owned(&f).unwrap();
        assert_ne!(kept.data(0).as_ptr(), f.data(0).as_ptr());
    }

    #[test]
    fn a_frame_copied_from_a_pool_has_its_samples() {
        let kept = owned(&pooled()).unwrap();
        let rows: Vec<&[u8]> = (0..4)
            .map(|y| &kept.data(0)[y * kept.stride(0)..][..4])
            .collect();
        assert_eq!(
            rows,
            [
                [1, 2, 3, 4],
                [5, 6, 7, 8],
                [9, 10, 11, 12],
                [13, 14, 15, 16]
            ]
        );
    }

    #[test]
    fn h264s_1080p_planes_take_the_rows_it_reads_past_the_picture() {
        let h264 = decoder::find(codec::Id::H264).unwrap();
        let mut ctx = codec::context::Context::new_with_codec(h264);
        // SAFETY: a context of its own, not opened.
        let planes = unsafe {
            (*ctx.as_mut_ptr()).pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P;
            layout(ctx.as_mut_ptr(), 1920, 1080)
        };
        let (linesize, size, _) = planes.unwrap();
        assert_eq!(linesize, [1920, 960, 960, 0]);
        assert_eq!(size, [2_092_800, 523_200, 523_200, 0]);
    }

    #[test]
    fn the_spare_of_a_size_let_go_last_is_taken() {
        let mut spares = Spares::new();
        for (size, p) in [(100, 1), (200, 2), (100, 3)] {
            spares.give(size, p, 1000);
        }
        assert_eq!(spares.take(100), Some(3));
    }

    #[test]
    fn past_the_most_the_oldest_spares_go() {
        let mut spares = Spares::new();
        for (size, p) in [(100, 1), (200, 2)] {
            spares.give(size, p, 400);
        }
        assert_eq!(spares.give(300, 3, 400), [(100, 1), (200, 2)]);
    }
}
