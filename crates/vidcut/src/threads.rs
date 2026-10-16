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
    /// Its decoding waits while a player comes first (`vidcut_av::first`),
    /// and its decoders and filter graphs take a quarter of the cores
    /// (`vidcut_av::threads`; an export takes FFmpeg's automatic count).
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
                lowest_priority();
                vidcut_av::first::yield_to_players();
                vidcut_av::threads::set(vidcut_av::threads::Threads::Quarter);
            }
            f()
        })
}

/// Gives the calling thread the lowest CPU priority. On Linux that is the
/// idle scheduling class, below nice 19, which the threads it starts later
/// inherit. On Windows it is the idle priority, which threads don't
/// inherit: the threads FFmpeg starts for a job run at the normal one. On
/// macOS it is the utility QoS class, which threads don't inherit either.
/// Not the background class below it: that keeps to the efficiency cores
/// and slows the thread's disk access, so a job wouldn't take the cores and
/// the disk left idle.
///
/// Not nice 19: NVIDIA's driver, opening NVENC, copies the calling thread's
/// nice value onto the process id, which is the main thread, and nothing
/// unprivileged raises it again. A job at nice 19 left the GUI there. In the
/// idle class the nice value stays the process's, so the copy changes
/// nothing.
fn lowest_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain system calls on this thread's own id.
    unsafe {
        let param = libc::sched_param { sched_priority: 0 };
        libc::sched_setscheduler(libc::gettid(), libc::SCHED_IDLE, &param);
    }
    #[cfg(windows)]
    // SAFETY: sets this thread's own priority.
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_IDLE,
        };
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_IDLE);
    }
    #[cfg(target_os = "macos")]
    // SAFETY: sets this thread's own class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}
