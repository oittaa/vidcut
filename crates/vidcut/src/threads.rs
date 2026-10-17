//! Starting threads. Every thread vidcut starts says what its work is, and
//! that sets its CPU priority (docs/decisions/gui.md §9): media work runs at
//! the lowest, so the GUI, playback and other programs come first, and it
//! still takes every core they leave idle. This is the one way the app
//! starts threads; a test below fails on any other.

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
