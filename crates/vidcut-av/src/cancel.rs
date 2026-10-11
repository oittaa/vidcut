//! The cancel flag of the long request running on this thread.
//!
//! A foreground export stops when its progress write fails: that sets the
//! call's flag, and the frame loop loads it. A frame, a storyboard, an
//! analysis and the loudness measure before an export report no progress, so
//! the same flag is published here and those decoders load it too. A worker
//! thread does not inherit the slot; it installs [`Guard::set_ptr`] with
//! [`current`] while the publishing guard is still held.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

thread_local! {
    static CANCEL: Cell<*const AtomicBool> = const { Cell::new(std::ptr::null()) };
}

/// A cancel flag a worker may install while the publishing [`Guard`] lives.
#[derive(Clone, Copy)]
pub struct Flag(*const AtomicBool);

// SAFETY: `Flag` only loads an `AtomicBool`. The guard that published it
// stays alive until every worker that received the `Flag` has joined.
unsafe impl Send for Flag {}
// SAFETY: same as `Send`: loads of the `AtomicBool` are synchronized.
unsafe impl Sync for Flag {}

/// Makes `cancel` visible to decoding on this thread until dropped.
pub struct Guard {
    prev: *const AtomicBool,
    /// `Rc` is not `Send`: the slot belongs to this thread.
    _unsend: PhantomData<Rc<()>>,
}

impl Guard {
    /// `cancel` must outlive the guard.
    pub fn set(cancel: &AtomicBool) -> Self {
        Self::set_ptr(Flag(std::ptr::from_ref(cancel)))
    }

    /// `flag` is one [`current`] returned while its guard is still held, or
    /// a null flag. It must stay valid until this guard drops.
    pub fn set_ptr(flag: Flag) -> Self {
        let prev = CANCEL.with(|slot| slot.replace(flag.0));
        Self {
            prev,
            _unsend: PhantomData,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let prev = self.prev;
        CANCEL.with(|slot| slot.set(prev));
    }
}

/// The flag installed on this thread, or a null flag.
pub fn current() -> Flag {
    Flag(CANCEL.with(|slot| slot.get()))
}

pub(crate) fn cancelled() -> bool {
    let ptr = current().0;
    if ptr.is_null() {
        return false;
    }
    // SAFETY: a non-null pointer was stored by `Guard::set` from a flag that
    // outlives that guard. `current` is passed to a worker only while the
    // publishing guard is held across the scope that joins the worker.
    unsafe { (*ptr).load(Ordering::Relaxed) }
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
        let flag = AtomicBool::new(true);
        {
            let _guard = Guard::set(&flag);
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
        let flag = AtomicBool::new(true);
        let _guard = Guard::set(&flag);
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
