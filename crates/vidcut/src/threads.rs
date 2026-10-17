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

/// The stage the GUI's ending is at: the session, the window, the
/// filmstrip's workers, the closing threads, the process's exit.
static ENDING: Mutex<&'static str> = Mutex::new("");

/// The GUI's ending reached `stage`: said under `VIDCUT_DEBUG`, and named
/// by [`end_within`]'s watchdog.
pub fn ending(stage: &'static str) {
    *ENDING.lock().unwrap_or_else(|e| e.into_inner()) = stage;
    vidcut_av::debug!("gui", "ending: {stage}");
}

/// The stage the GUI's ending is at.
pub fn ending_at() -> &'static str {
    *ENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ends the process if the GUI's ending takes longer than `patience`, saying
/// the stage it hangs at. Started as the window closes; a process that ends
/// in time takes the watchdog's thread with it.
pub fn end_within(patience: std::time::Duration) {
    let _ = spawn("vidcut-exit-watch", Work::Interactive, move || {
        std::thread::sleep(patience);
        eprintln!(
            "vidcut: still ending {} after {} s; quitting",
            ending_at(),
            patience.as_secs()
        );
        for (name, waits_in) in threads_waiting() {
            eprintln!("  {name:<16} {waits_in}");
        }
        // Under VIDCUT_DEBUG it aborts instead: a debugger it runs under
        // (`gdb -ex run`) then shows every thread's stack at the hang.
        if vidcut_av::debug() {
            std::process::abort();
        }
        #[cfg(unix)]
        // SAFETY: ends the process at once, without the exit handlers a
        // hang may be in.
        unsafe {
            libc::_exit(1)
        }
        #[cfg(not(unix))]
        std::process::exit(1)
    });
}

/// Ends the process now with `code`, its output flushed and no library's
/// exit handler run: libnvcuvid's joins a thread of the driver's that
/// never ends once a window closed while NVDEC decoded (gotchas.md). For
/// the GUI once its own threads are joined ([`join_closing`]).
pub fn exit_now(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    // SAFETY: ends the process at once; nothing of ours runs after.
    unsafe {
        libc::_exit(code)
    }
    #[cfg(not(unix))]
    std::process::exit(code)
}

/// Every thread of the process with what the kernel has it waiting on
/// (Linux, from `/proc`; nothing elsewhere), for the watchdog's report.
fn threads_waiting() -> Vec<(String, String)> {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    let mut threads: Vec<(String, String)> = tasks
        .filter_map(|t| {
            let t = t.ok()?.path();
            let read = |f: &str| std::fs::read_to_string(t.join(f)).ok();
            Some((
                read("comm")?.trim().to_string(),
                read("wchan")?.trim().to_string(),
            ))
        })
        .collect();
    threads.sort();
    threads
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_ending_stage_is_the_last_one_named() {
        ending("the session");
        ending("the window");
        assert_eq!(ending_at(), "the window");
    }

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
