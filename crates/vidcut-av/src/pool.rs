//! The shared pools that run work split into slices (docs/decisions/
//! threads.md §3): a filter graph's slices and the row loops of
//! [`crate::color`], instead of threads of their own.
//!
//! Two pools, each started on first use with a thread fewer than the
//! cores, as the thread asking works too: one at normal priority
//! (`vidcut-cpu-N`) and one at the lowest (`vidcut-cpu-job-N`) for the
//! threads that work for a job ([`crate::first::as_job`]). A thread on
//! Linux would hand its priority down to the threads it starts; those on
//! Windows and macOS don't, so each of the jobs' pool sets its own.

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

use crate::decode::Reading;
use crate::ffmpeg::{ffi, filter};

/// One call's slices, while they run.
struct Batch {
    /// The caller's slice function. Valid until every slice is done: the
    /// caller waits for that.
    job: *const (dyn Fn(usize) + Sync),
    slices: usize,
    next: AtomicUsize,
    done: Mutex<usize>,
    finished: Condvar,
    panicked: AtomicBool,
}

// SAFETY: `job` is `Sync`, and only called while its caller waits.
unsafe impl Send for Batch {}
// SAFETY: as above; the rest is atomics and locks.
unsafe impl Sync for Batch {}

impl Batch {
    /// Runs the next slice; `false` once none is left.
    fn run_one(&self) -> bool {
        let i = self.next.fetch_add(1, Ordering::Relaxed);
        if i >= self.slices {
            return false;
        }
        // SAFETY: slice `i` isn't done, so the caller still waits.
        let job = unsafe { &*self.job };
        if std::panic::catch_unwind(AssertUnwindSafe(|| job(i))).is_err() {
            self.panicked.store(true, Ordering::Relaxed);
        }
        let mut done = lock(&self.done);
        *done += 1;
        if *done == self.slices {
            self.finished.notify_all();
        }
        true
    }

    fn exhausted(&self) -> bool {
        self.next.load(Ordering::Relaxed) >= self.slices
    }
}

struct Pool {
    queue: Mutex<VecDeque<Arc<Batch>>>,
    ready: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The cores: how many slices a call splits into, its own thread and the
/// pool's together.
pub fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// The pool of the calling thread: the jobs' for a thread working for a
/// job, the other's for the rest. Started on first use.
fn pool() -> &'static Pool {
    static NORMAL: OnceLock<Pool> = OnceLock::new();
    static JOBS: OnceLock<Pool> = OnceLock::new();
    let job = crate::first::yields();
    let (cell, name) = if job {
        (&JOBS, "vidcut-cpu-job")
    } else {
        (&NORMAL, "vidcut-cpu")
    };
    let mut new = false;
    let pool = cell.get_or_init(|| {
        new = true;
        Pool {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
        }
    });
    if new {
        for i in 0..threads() - 1 {
            let _ = std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || {
                    if job {
                        crate::first::lowest_priority();
                    }
                    work(pool)
                });
        }
    }
    pool
}

/// A pool thread: the slices of the oldest call that has some left.
fn work(pool: &Pool) {
    loop {
        let batch = {
            let mut queue = lock(&pool.queue);
            loop {
                while queue.front().is_some_and(|b| b.exhausted()) {
                    queue.pop_front();
                }
                if let Some(b) = queue.front() {
                    break b.clone();
                }
                queue = pool.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
            }
        };
        while batch.run_one() {}
    }
}

/// Runs `job(0)` to `job(slices - 1)` on the calling thread's pool and on
/// the calling thread, returning once all are done. A slice that panicked
/// panics here.
pub fn run(slices: usize, job: &(dyn Fn(usize) + Sync)) {
    match slices {
        0 => return,
        1 => return job(0),
        _ => {}
    }
    if threads() == 1 {
        return (0..slices).for_each(job);
    }
    // SAFETY: only the lifetime goes: the batch calls `job` only for slices
    // not yet done, and this call returns once all are.
    let job: *const (dyn Fn(usize) + Sync + 'static) = unsafe { std::mem::transmute(job) };
    let batch = Arc::new(Batch {
        job,
        slices,
        next: AtomicUsize::new(0),
        done: Mutex::new(0),
        finished: Condvar::new(),
        panicked: AtomicBool::new(false),
    });
    let pool = pool();
    lock(&pool.queue).push_back(batch.clone());
    pool.ready.notify_all();
    while batch.run_one() {}
    let mut done = lock(&batch.done);
    while *done < slices {
        done = batch.finished.wait(done).unwrap_or_else(|e| e.into_inner());
    }
    drop(done);
    assert!(
        !batch.panicked.load(Ordering::Relaxed),
        "a slice of the pool's panicked"
    );
}

/// A filter graph whose slices run on the pool, for a stream: in as many
/// slices as [`threads`]. For single frames, in one, on the thread that
/// runs the graph. Either way the graph starts no thread of its own.
pub fn graph(reading: Reading) -> filter::Graph {
    let mut graph = filter::Graph::new();
    let slices = match reading {
        Reading::Stream => threads(),
        Reading::Frames => 1,
    };
    // SAFETY: a graph of no filters yet, which is when FFmpeg takes both.
    unsafe {
        let raw = graph.as_mut_ptr();
        (*raw).execute = Some(execute);
        (*raw).nb_threads = slices as libc::c_int;
    }
    graph
}

/// FFmpeg's `AVFilterGraph.execute`: a filter's `nb_jobs` slices of
/// `func`, their results into `ret`.
unsafe extern "C" fn execute(
    ctx: *mut ffi::AVFilterContext,
    func: ffi::avfilter_action_func,
    arg: *mut libc::c_void,
    ret: *mut libc::c_int,
    nb_jobs: libc::c_int,
) -> libc::c_int {
    let Some(func) = func else {
        return 0;
    };
    /// What the slices share: FFmpeg's pointers, each slice writing only
    /// its own result.
    struct Call {
        func: unsafe extern "C" fn(
            *mut ffi::AVFilterContext,
            *mut libc::c_void,
            libc::c_int,
            libc::c_int,
        ) -> libc::c_int,
        ctx: *mut ffi::AVFilterContext,
        arg: *mut libc::c_void,
        ret: *mut libc::c_int,
        jobs: libc::c_int,
    }
    // SAFETY: FFmpeg runs a filter's slices in parallel by its design.
    unsafe impl Sync for Call {}
    impl Call {
        fn slice(&self, i: usize) {
            // SAFETY: FFmpeg's slice function on its own context and
            // argument, slice `i` of `jobs`, its result in `ret[i]`.
            unsafe {
                let r = (self.func)(self.ctx, self.arg, i as libc::c_int, self.jobs);
                if !self.ret.is_null() {
                    *self.ret.add(i) = r;
                }
            }
        }
    }
    let call = Call {
        func,
        ctx,
        arg,
        ret,
        jobs: nb_jobs,
    };
    let slices = usize::try_from(nb_jobs).unwrap_or(0);
    let ran = std::panic::catch_unwind(AssertUnwindSafe(|| run(slices, &|i| call.slice(i))));
    match ran {
        Ok(()) => 0,
        Err(_) => ffi::AVERROR_BUG,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slice_runs_once() {
        let ran: Vec<AtomicUsize> = (0..100).map(|_| AtomicUsize::new(0)).collect();
        run(100, &|i| {
            ran[i].fetch_add(1, Ordering::Relaxed);
        });
        assert!(ran.iter().all(|r| r.load(Ordering::Relaxed) == 1));
    }

    #[test]
    fn a_graph_for_single_frames_runs_in_one_slice() {
        crate::init();
        let mut graph = graph(Reading::Frames);
        // SAFETY: reads the graph's own field.
        assert_eq!(unsafe { (*graph.as_mut_ptr()).nb_threads }, 1);
    }
}
