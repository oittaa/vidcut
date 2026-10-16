//! The threads that end players once the window has closed
//! (docs/decisions/gui.md §1). `vidcut-viewer-close` and `vidcut-two-up-close`
//! are handed here and joined, so the process exits on its own.
//! `process::exit` skips `Drop`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;

/// Close threads started while the GUI runs, and whether the window has
/// closed. `vidcut-quit` exits the process only when the window has not.
pub(super) struct CloseThreads {
    window_closed: AtomicBool,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl CloseThreads {
    pub(super) fn new() -> Self {
        Self {
            window_closed: AtomicBool::new(false),
            handles: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn push(&self, handle: JoinHandle<()>) {
        self.lock().push(handle);
    }

    /// Close threads handed over and not yet joined.
    #[cfg(test)]
    pub(super) fn pending(&self) -> usize {
        self.lock().len()
    }

    /// The window has closed. `vidcut-quit` must not `process::exit`.
    pub(super) fn mark_window_closed(&self) {
        self.window_closed.store(true, Ordering::SeqCst);
    }

    /// `vidcut-quit`, after its wait: the window still has not closed.
    pub(super) fn window_never_closed(&self) -> bool {
        !self.window_closed.load(Ordering::SeqCst)
    }

    /// Joins every close thread handed over. Marks the window closed first,
    /// so a watchdog that wakes during the joins does not exit the process.
    pub(super) fn join(&self) {
        self.mark_window_closed();
        let handles = std::mem::take(&mut *self.lock());
        for handle in handles {
            let _ = handle.join();
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<JoinHandle<()>>> {
        self.handles.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for CloseThreads {
    fn drop(&mut self) {
        // A caller that drops the last handle without `join` would detach
        // the threads, and the process would exit under `Drop`.
        self.join();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::CloseThreads;

    #[test]
    fn the_quit_watchdog_exits_only_when_the_window_stays_open() {
        let closes = CloseThreads::new();
        assert!(closes.window_never_closed());
        closes.mark_window_closed();
        assert!(!closes.window_never_closed());
    }

    #[test]
    fn close_threads_are_joined_before_the_process_exits() {
        let closes = CloseThreads::new();
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let handle = crate::threads::spawn(
            "vidcut-viewer-close",
            crate::threads::Work::Interactive,
            move || {
                std::thread::sleep(Duration::from_millis(50));
                flag.store(true, Ordering::SeqCst);
            },
        )
        .unwrap();
        closes.push(handle);
        assert!(!done.load(Ordering::SeqCst));
        assert_eq!(closes.pending(), 1);
        closes.join();
        assert!(done.load(Ordering::SeqCst));
        assert_eq!(closes.pending(), 0);
    }
}
