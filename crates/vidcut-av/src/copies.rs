use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use vidcut_core::editor::Copied;
use vidcut_core::export::EncoderChoice;

use crate::ffmpeg::{ffi, frame};

thread_local! {
    static COPIES: RefCell<Option<Arc<Copies>>> = const { RefCell::new(None) };
}

#[derive(Debug, Default)]
pub struct Copies {
    down: AtomicU64,
    down_bytes: AtomicU64,
    up: AtomicU64,
    up_bytes: AtomicU64,
}

impl Copies {
    pub fn counted(&self) -> Copied {
        Copied {
            down: self.down.load(Ordering::Relaxed),
            down_bytes: self.down_bytes.load(Ordering::Relaxed),
            up: self.up.load(Ordering::Relaxed),
            up_bytes: self.up_bytes.load(Ordering::Relaxed),
        }
    }
}

pub struct Guard {
    prev: Option<Arc<Copies>>,
    _unsend: PhantomData<Rc<()>>,
}

impl Guard {
    pub fn set(copies: Arc<Copies>) -> Self {
        let prev = COPIES.with(|slot| slot.replace(Some(copies)));
        Self {
            prev,
            _unsend: PhantomData,
        }
    }

    pub fn maybe(copies: Option<Arc<Copies>>) -> Option<Self> {
        copies.map(Self::set)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        COPIES.with(|slot| *slot.borrow_mut() = prev);
    }
}

pub fn current() -> Option<Arc<Copies>> {
    COPIES.with(|slot| slot.borrow().clone())
}

pub(crate) fn down(f: &frame::Video) {
    count(f, |c| (&c.down, &c.down_bytes));
}

pub fn up(f: &frame::Video) {
    count(f, |c| (&c.up, &c.up_bytes));
}

pub(crate) fn to_encoder(f: &frame::Video, kind: EncoderChoice) {
    if kind.gpu() && !crate::hw::on_gpu(f) {
        up(f);
    }
}

fn count(f: &frame::Video, which: impl Fn(&Copies) -> (&AtomicU64, &AtomicU64)) {
    COPIES.with(|slot| {
        if let Some(c) = slot.borrow().as_ref() {
            let (frames, bytes) = which(c);
            frames.fetch_add(1, Ordering::Relaxed);
            bytes.fetch_add(size(f), Ordering::Relaxed);
        }
    });
}

fn size(f: &frame::Video) -> u64 {
    let (w, h) = (f.width() as i32, f.height() as i32);
    // SAFETY: computes a size from a format and sides.
    let bytes = unsafe { ffi::av_image_get_buffer_size(f.format().into(), w, h, 1) };
    u64::try_from(bytes).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::format::Pixel;

    #[test]
    fn a_frame_copied_down_counts_its_bytes_for_the_job() {
        crate::init();
        let copies = Arc::new(Copies::default());
        let _counting = Guard::set(copies.clone());
        down(&frame::Video::new(Pixel::NV12, 1920, 1080));
        let want = Copied {
            down: 1,
            down_bytes: 3_110_400,
            up: 0,
            up_bytes: 0,
        };
        assert_eq!(copies.counted(), want);
    }

    #[test]
    fn a_frame_in_memory_for_a_gpu_encoder_counts_as_copied_up() {
        crate::init();
        let copies = Arc::new(Copies::default());
        let _counting = Guard::set(copies.clone());
        to_encoder(
            &frame::Video::new(Pixel::NV12, 960, 540),
            EncoderChoice::Amf,
        );
        let want = Copied {
            down: 0,
            down_bytes: 0,
            up: 1,
            up_bytes: 777_600,
        };
        assert_eq!(copies.counted(), want);
    }

    #[test]
    fn a_frame_in_memory_for_the_cpus_encoder_counts_nothing() {
        crate::init();
        let copies = Arc::new(Copies::default());
        let _counting = Guard::set(copies.clone());
        to_encoder(
            &frame::Video::new(Pixel::NV12, 960, 540),
            EncoderChoice::Cpu,
        );
        assert_eq!(copies.counted(), Copied::default());
    }
}
