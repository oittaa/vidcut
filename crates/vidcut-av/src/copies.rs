//! Frames copied between GPU memory and memory, counted for the job this
//! thread works for (an export, a proxy): how many went down and up, and
//! their bytes. Every copy down goes through `hw::download`, every copy up
//! through the encoders' uploads, and those count here. A worker thread does
//! not inherit the slot; it installs a [`Guard`] with a clone of
//! [`current`].

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use vidcut_core::editor::Copied;

use crate::ffmpeg::{ffi, frame};

thread_local! {
    static COPIES: RefCell<Option<Arc<Copies>>> = const { RefCell::new(None) };
}

/// One job's count.
#[derive(Debug, Default)]
pub struct Copies {
    down: AtomicU64,
    down_bytes: AtomicU64,
    up: AtomicU64,
    up_bytes: AtomicU64,
}

impl Copies {
    /// The count so far.
    pub fn counted(&self) -> Copied {
        Copied {
            down: self.down.load(Ordering::Relaxed),
            down_bytes: self.down_bytes.load(Ordering::Relaxed),
            up: self.up.load(Ordering::Relaxed),
            up_bytes: self.up_bytes.load(Ordering::Relaxed),
        }
    }
}

/// Counts the copies made on this thread for `copies` until dropped.
pub struct Guard {
    prev: Option<Arc<Copies>>,
    /// `Rc` is not `Send`: the slot belongs to this thread.
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

    /// Counts for `copies` when there are some to count for.
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

/// The count installed on this thread, if any, for a worker to install too.
pub fn current() -> Option<Arc<Copies>> {
    COPIES.with(|slot| slot.borrow().clone())
}

/// `f`, just copied down from GPU memory.
pub(crate) fn down(f: &frame::Video) {
    count(f, |c| (&c.down, &c.down_bytes));
}

/// `f`, just copied up into GPU memory.
pub(crate) fn up(f: &frame::Video) {
    count(f, |c| (&c.up, &c.up_bytes));
}

/// `f`, about to go to an encoder of `kind`: a GPU's encoder copies a
/// frame from memory up itself (AMF, QSV, VideoToolbox, NVENC given one).
pub(crate) fn to_encoder(f: &frame::Video, kind: vidcut_core::export::EncoderChoice) {
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

/// The bytes of `f`'s picture in memory: its layout at its size.
fn size(f: &frame::Video) -> u64 {
    let (w, h) = (f.width() as i32, f.height() as i32);
    // SAFETY: computes a size from a format and sides.
    let bytes = unsafe { ffi::av_image_get_buffer_size(f.format().into(), w, h, 1) };
    u64::try_from(bytes).unwrap_or(0)
}
