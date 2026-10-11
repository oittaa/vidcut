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
    /// preview's helper files. The lowest priority (nice 19); the threads
    /// it starts, FFmpeg's decoders and encoders among them, inherit it.
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
            }
            f()
        })
}

/// Gives the calling thread the lowest CPU priority (nice 19). Threads it
/// starts later inherit it. Linux only.
fn lowest_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain system calls on this thread's own id.
    unsafe {
        let tid = libc::gettid();
        libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, 19);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn a_job_and_the_threads_it_starts_run_at_low_priority() {
        fn nice() -> i32 {
            // SAFETY: plain system calls on this thread's own id.
            unsafe { libc::getpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t) }
        }
        let before = nice();
        let job = spawn("test-job", Work::Job, || {
            (
                nice(),
                std::thread::scope(|s| s.spawn(nice).join().unwrap()),
            )
        })
        .unwrap()
        .join()
        .unwrap();
        assert_eq!(job, (19, 19));
        let interactive = spawn("test-interactive", Work::Interactive, nice)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(interactive, before);
        assert_eq!(nice(), before, "the caller's priority changed");
    }

    /// Every thread the app starts goes through [`spawn`], which makes each
    /// one say what its work is: a thread started otherwise could run media
    /// work at the GUI's priority, as Detect Scene Cuts once did.
    #[test]
    fn threads_start_only_through_spawn() {
        fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    sources(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&src, &mut files);
        let mut offenders = Vec::new();
        for f in files.iter().filter(|f| !f.ends_with("threads.rs")) {
            let text = std::fs::read_to_string(f).unwrap();
            // Unit tests may start threads of their own.
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for (n, line) in code.lines().enumerate() {
                if line.contains("thread::spawn(") || line.contains("thread::Builder::new(") {
                    offenders.push(format!("{}:{}", f.display(), n + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "start threads with threads::spawn, saying what the work is: {offenders:#?}"
        );
    }
}
