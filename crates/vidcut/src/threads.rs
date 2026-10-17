//! Starting threads. Every thread vidcut starts says what its work is, and
//! that sets its CPU priority (docs/decisions/gui.md §9): media work runs at
//! the lowest, so the GUI, playback and other programs come first, and it
//! still takes every core they leave idle. This is the one way the app
//! starts threads; a test below fails on any other.

use std::sync::Mutex;
use std::thread::JoinHandle;

/// What a thread's work is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    /// Media work nobody watches frame by frame: exports, proxies,
    /// measurements and analyses, storyboards, thumbnails, waveforms, the
    /// preview's helper files. The lowest priority (Linux's idle class,
    /// Windows's idle priority, macOS's utility class); on Linux the threads
    /// it starts, FFmpeg's decoders and encoders among them, inherit it.
    /// Its decoding waits while a player comes first (`vidcut_av::first`).
    Job,
    /// Work someone waits on now: the session itself, requests and their
    /// answers, file dialogs, a frame or a plan.
    Interactive,
}

/// Starts `f` on a thread named `name`, at the priority its `work` calls for.
pub fn spawn<T: Send + 'static>(
    name: impl Into<String>,
    work: Work,
    f: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<JoinHandle<T>> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            if work == Work::Job {
                vidcut_av::first::as_job();
            }
            f()
        })
}

/// Threads that close something and end: a player when the window closes,
/// the two-up's players, Vulkan imports. Joined before the process ends
/// ([`join_closing`]): CUDA and the GPU drivers tear down as it exits, and
/// a close after that crashes (gotchas.md).
static CLOSING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Starts `f` on a thread named `name` that closes something and ends,
/// which [`join_closing`] waits for.
pub fn closing(
    name: impl Into<String>,
    work: Work,
    f: impl FnOnce() + Send + 'static,
) -> std::io::Result<()> {
    let handle = spawn(name, work, f)?;
    let mut closing = CLOSING.lock().unwrap_or_else(|e| e.into_inner());
    closing.retain(|h| !h.is_finished());
    closing.push(handle);
    Ok(())
}

/// Waits for every closing thread still running: before the process ends.
pub fn join_closing() {
    let handles = std::mem::take(&mut *CLOSING.lock().unwrap_or_else(|e| e.into_inner()));
    for handle in handles {
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn join_closing_waits_for_a_closing_thread() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        closing("vidcut-test-close", Work::Interactive, move || {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        })
        .unwrap();
        join_closing();
        assert!(done.load(Ordering::SeqCst));
    }
}
