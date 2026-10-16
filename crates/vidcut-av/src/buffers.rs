//! Memory for the frames the CPU's decoders decode: each plane a buffer of
//! its own, laid out as FFmpeg's default lays them out, freed with the
//! frame. FFmpeg's default takes them from pools of the decoder's, which
//! never shrink and live as long as their last frame (gotchas.md): a
//! picture the player keeps would hold its decoder's whole pool, and a
//! decoder closed while its pictures are kept would keep its peak beside
//! the next one's.
//!
//! On Unix a plane is mapped of its own, so what is freed goes back to the
//! system: glibc's allocator would keep it (gotchas.md). Memory mapped anew
//! for each frame costs a fifth more CPU to decode, its pages faulted in,
//! so the planes let go last, up to [`SPARE`] bytes, are kept for the next
//! frames of their size, whichever decoder asks.
//!
//! Frames that come from pools all the same (libdav1d's, which takes no
//! buffers but its own; a filter graph's, the deinterlacer's and the
//! relayout's) are copied into such buffers when the player keeps them
//! ([`owned`]).

use std::collections::{HashSet, VecDeque};
use std::ffi::{c_int, c_void};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::{Error, Result};
use crate::ffmpeg::{self, codec, ffi, frame};

/// Bytes of planes let go kept for the next frames: a decoder lets its
/// frames go about as fast as it takes new ones. A 4K 10-bit frame's
/// planes take 25 MB; a plane larger than this is freed at once.
const SPARE: usize = 64 << 20;

static SPARES: Mutex<Spares> = Mutex::new(Spares::new());

fn spares() -> MutexGuard<'static, Spares> {
    SPARES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Frames `ctx` decodes in memory get buffers of their own
/// ([`get_buffer`]).
pub(crate) fn own(ctx: &mut codec::context::Context) {
    // SAFETY: the context isn't opened yet.
    unsafe { (*ctx.as_mut_ptr()).get_buffer2 = Some(get_buffer) };
}

/// `get_buffer2`: each plane of a frame decoded in memory gets a buffer of
/// its own ([`plane`]), laid out as FFmpeg's default lays them out
/// ([`layout`]). Frames in GPU memory, decoders that take no buffers but
/// FFmpeg's (without `AV_CODEC_CAP_DR1`, such as libdav1d), and layouts
/// FFmpeg can't tell get FFmpeg's default.
unsafe extern "C" fn get_buffer(
    ctx: *mut ffi::AVCodecContext,
    frame: *mut ffi::AVFrame,
    flags: c_int,
) -> c_int {
    // SAFETY: FFmpeg's open context, and the frame it asks buffers for, its
    // format and size set and its planes empty. Each buffer is the frame's
    // own reference; FFmpeg frees them with the frame, or with
    // `av_frame_unref` when one can't be had.
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

/// `f`, a frame in memory, in buffers that are freed with it: itself, by
/// reference, where they are ([`plane`]'s, or plain memory of FFmpeg's,
/// which keeps no pool behind them), else a copy into buffers of its own,
/// with its properties. A buffer from a pool (a decoder's of its own, a
/// filter graph's) would keep that pool whole.
pub(crate) fn owned(f: &frame::Video) -> Result<frame::Video> {
    if freed_with(f) {
        return crate::export::video::frame_ref(f);
    }
    let failed = |what: &str, ret: c_int| Error::Ffmpeg {
        context: what.into(),
        source: ffmpeg::Error::from(ret),
    };
    let mut out = frame::Video::empty();
    // SAFETY: a fresh frame given planes of its own, laid out as
    // `av_frame_get_buffer` lays them out (rows 64-byte aligned), then the
    // picture and properties of `f`, a frame in memory of that layout.
    unsafe {
        let (src, dst) = (f.as_ptr(), out.as_mut_ptr());
        let format = std::mem::transmute::<c_int, ffi::AVPixelFormat>((*src).format);
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

/// Gives what glibc keeps of the memory freed back to the system, on a
/// thread of its own, at most once a second. A closed decoder's own pools
/// (libdav1d's) and its threads' memory come from glibc's heaps, which
/// keep what is freed (gotchas.md): two AV1 clips swapped under a 500 MB
/// budget grew 300 MB in 8 swaps without it, 110 MB with it.
pub(crate) fn give_back() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    trim::ask();
}

/// glibc's `malloc_trim`, on a thread of its own.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod trim {
    use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
    use std::time::Duration;

    /// A trim is asked for.
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

/// Whether every buffer of `f` is freed with its last frame: [`plane`]'s,
/// or plain memory of FFmpeg's (`av_buffer_alloc`, with no opaque). A
/// pool's buffer has the pool's entry for its opaque.
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

/// The rows (bytes) and sizes of the planes of a `width`×`height` frame
/// `ctx` decodes, and the padding after each, as FFmpeg's default lays them
/// out (8.0, get_buffer.c `update_frame_pool`): the picture grown to the
/// codec's alignment, rows past it that decoders read (H.264's two), each
/// plane's rows a multiple of the CPU's alignment, and FFmpeg's padding.
/// `None` where FFmpeg knows no such layout, or a plane doesn't fit its
/// `int`.
///
/// # Safety
///
/// `ctx` is a live codec context with its pixel format set.
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
    // Wider by its lowest set bit until every plane's rows are aligned: the
    // planes widen together, as some decoders take one plane's rows to be
    // twice another's.
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

/// A buffer of `size` bytes for a plane: a spare one let go last, or new
/// memory. Null when there is none.
fn plane(size: usize) -> *mut ffi::AVBufferRef {
    let spare = spares().take(size);
    let data = spare.map_or_else(|| alloc(size), |p| p as *mut u8);
    if data.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `size` bytes of memory of the buffer's own, given back by
    // `let_go` with its size.
    let buf = unsafe { ffi::av_buffer_create(data, size, Some(let_go), size as *mut c_void, 0) };
    if buf.is_null() {
        // SAFETY: memory nothing refers to.
        unsafe { free(data, size) };
    } else {
        spares().live().insert(data as usize);
    }
    buf
}

/// A plane's buffer of `size` bytes freed: its memory becomes a spare, and
/// the oldest spares past [`SPARE`] bytes are freed.
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

/// `size` bytes of new memory, zeroed, aligned for the CPU; null when there
/// is none. Mapped on Unix.
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

/// Frees memory of `size` bytes from [`alloc`].
///
/// # Safety
///
/// `p` is [`alloc`]'s, and nothing refers to it.
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

/// Planes' memory let go, kept for frames to come: each one's size and
/// address, oldest first.
struct Spares {
    bytes: usize,
    planes: VecDeque<(usize, usize)>,
    /// The planes given out and not let go, by address.
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

    /// The spare of `size` bytes let go last.
    fn take(&mut self, size: usize) -> Option<usize> {
        let i = self.planes.iter().rposition(|&(s, _)| s == size)?;
        let (_, p) = self.planes.remove(i)?;
        self.bytes -= size;
        Some(p)
    }

    /// Keeps the plane at `p`, `size` bytes, as a spare, and lets the
    /// oldest go past `most` bytes: those to free (`p` itself when it is
    /// larger).
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
    use super::{Spares, layout, owned};
    use crate::ffmpeg::{codec, decoder, ffi, format::Pixel, frame};

    /// A 4×4 8-bit 4:2:0 frame whose planes are in one buffer of a pool,
    /// luma 1 to 16 row by row, chroma 128.
    fn pooled() -> frame::Video {
        let mut f = frame::Video::empty();
        // SAFETY: a fresh frame given one buffer of a pool of its size; the
        // pool goes once that buffer does.
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
        // 1088 rows and 2 more, and half of that for U and V.
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
