use std::cell::Cell;
use std::sync::Mutex;
use std::thread::{JoinHandle, Scope, ScopedJoinHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    Job,
    Interactive,
}

thread_local! {
    static WORK: Cell<Work> = const { Cell::new(Work::Interactive) };
}

impl Work {
    pub fn current() -> Self {
        WORK.get()
    }
}

pub fn spawn<T: Send + 'static>(
    name: impl Into<String>,
    work: Work,
    f: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<JoinHandle<T>> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            begin(work);
            f()
        })
}

pub fn scoped<'scope, T: Send + 'scope>(
    scope: &'scope Scope<'scope, '_>,
    name: impl Into<String>,
    work: Work,
    f: impl FnOnce() -> T + Send + 'scope,
) -> std::io::Result<ScopedJoinHandle<'scope, T>> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn_scoped(scope, move || {
            begin(work);
            f()
        })
}

fn begin(work: Work) {
    WORK.set(work);
    if work == Work::Job {
        vidcut_av::first::as_job();
    }
}

static CLOSING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

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

pub fn join_closing() {
    let handles = std::mem::take(&mut *CLOSING.lock().unwrap_or_else(|e| e.into_inner()));
    for handle in handles {
        let _ = handle.join();
    }
    vidcut_av::ending::join();
}

static ENDING: Mutex<&'static str> = Mutex::new("");

pub fn ending(stage: &'static str) {
    *ENDING.lock().unwrap_or_else(|e| e.into_inner()) = stage;
    vidcut_av::debug!("gui", "ending: {stage}");
}

pub fn ending_at() -> &'static str {
    *ENDING.lock().unwrap_or_else(|e| e.into_inner())
}

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
        if vidcut_av::debug() {
            std::process::abort();
        }
        #[cfg(unix)]
        // SAFETY: ends the process at once, without the exit handlers a hang may be in.
        unsafe {
            libc::_exit(1)
        }
        #[cfg(not(unix))]
        std::process::exit(1)
    });
}

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

    #[test]
    fn a_scoped_thread_can_work_as_its_starter_does() {
        let work = spawn("vidcut-test-job", Work::Job, || {
            std::thread::scope(|s| {
                scoped(s, "vidcut-test-part", Work::current(), Work::current)
                    .unwrap()
                    .join()
                    .unwrap()
            })
        })
        .unwrap()
        .join()
        .unwrap();
        assert_eq!(work, Work::Job);
    }
}
