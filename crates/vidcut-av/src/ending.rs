//! Threads left to end on their own. A reader's opener (`export::ahead`)
//! and a followed player's windows thread are told to stop as what started
//! them goes, which doesn't wait for them: a file being opened is out of
//! any cancel flag's reach. They close the files they hold as they end.
//! [`join`] waits for them, before the process ends: a GPU decoder closing
//! while the process exits hangs it in NVDEC's exit handler (gotchas.md).

use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;

/// The threads left, told to stop.
static LEFT: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

fn left() -> MutexGuard<'static, Vec<JoinHandle<()>>> {
    LEFT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Leaves `thread`, told to stop, to end on its own; [`join`] waits for it.
pub(crate) fn leave(thread: JoinHandle<()>) {
    let mut left = left();
    left.retain(|t| !t.is_finished());
    left.push(thread);
}

/// Waits for every thread left to end: before the process ends, once what
/// started them is gone.
pub fn join() {
    let threads = std::mem::take(&mut *left());
    for thread in threads {
        let _ = thread.join();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn join_waits_for_a_thread_left() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        leave(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        }));
        join();
        assert!(done.load(Ordering::SeqCst));
    }
}
