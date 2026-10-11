//! The cancel flag of the long request running on this thread.
//!
//! A foreground export stops when its progress write fails: that sets the
//! call's flag, and the frame loop loads it. A frame, a storyboard, an
//! analysis and the loudness measure before an export report no progress, so
//! the same flag is published here and those decoders load it too. A worker
//! thread does not inherit the slot; it installs a [`Guard`] with a clone of
//! [`current`].

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

thread_local! {
    static CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Makes a cancel flag visible to decoding on this thread until dropped.
pub struct Guard {
    prev: Option<Arc<AtomicBool>>,
    /// `Rc` is not `Send`: the slot belongs to this thread.
    _unsend: PhantomData<Rc<()>>,
}

impl Guard {
    pub fn set(cancel: Arc<AtomicBool>) -> Self {
        let prev = CANCEL.with(|slot| slot.replace(Some(cancel)));
        Self {
            prev,
            _unsend: PhantomData,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        CANCEL.with(|slot| *slot.borrow_mut() = prev);
    }
}

/// The flag installed on this thread, if any, for a worker to install too.
pub fn current() -> Option<Arc<AtomicBool>> {
    CANCEL.with(|slot| slot.borrow().clone())
}

pub(crate) fn cancelled() -> bool {
    CANCEL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    })
}

/// The error the export frame loop already returns when its flag is set.
pub(crate) fn stop() -> Result<()> {
    if cancelled() {
        Err(Error::Invalid(crate::export::CANCELLED.into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::analysis;
    use crate::export::is_cancelled;

    #[test]
    fn the_guard_is_visible_only_while_held() {
        assert!(!cancelled());
        let flag = Arc::new(AtomicBool::new(true));
        {
            let _guard = Guard::set(flag.clone());
            assert!(cancelled());
            flag.store(false, Ordering::Relaxed);
            assert!(!cancelled());
            flag.store(true, Ordering::Relaxed);
            assert!(cancelled());
        }
        assert!(!cancelled());
    }

    #[test]
    fn a_set_flag_stops_measurements_before_they_open_a_file() {
        let _guard = Guard::set(Arc::new(AtomicBool::new(true)));
        let err = analysis::audio_levels(Path::new("no-such-media"), 0, 10).unwrap_err();
        assert!(is_cancelled(&err), "{err}");
        let err =
            analysis::picture_stats(&crate::decode::Input::File("no-such-media".into()), None)
                .unwrap_err();
        assert!(is_cancelled(&err), "{err}");
        let err = analysis::mix_loudness(&[], 48_000, 2, 0, 48_000, None).unwrap_err();
        assert!(is_cancelled(&err), "{err}");
    }
}
