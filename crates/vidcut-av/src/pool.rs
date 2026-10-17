//! The shared pools that run work split into slices (docs/decisions/
//! threads.md §3): a filter graph's slices and the row loops of
//! [`crate::color`], instead of threads of their own. And tasks a thread
//! hands them in order ([`tasks`]): a stream's frames that stand alone,
//! each decoded on its own (§4.2).
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
use std::time::Instant;

use crate::decode::Reading;
use crate::ffmpeg::{ffi, filter};

pub(crate) mod tasks;

pub(crate) use tasks::Task;

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

/// What a pool thread takes from the queue, oldest first: a call's slices,
/// which stay at the front until none is left, or a task.
enum Work {
    Slices(Arc<Batch>),
    Task(Arc<dyn Task>),
}

struct Pool {
    queue: Mutex<VecDeque<Work>>,
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

/// The oldest work in `queue`: the slices of the oldest call that has some
/// left, or the oldest task, taken out.
fn next(queue: &mut VecDeque<Work>) -> Option<Work> {
    while let Some(Work::Slices(b)) = queue.front()
        && b.exhausted()
    {
        queue.pop_front();
    }
    match queue.front()? {
        Work::Slices(b) => Some(Work::Slices(b.clone())),
        Work::Task(_) => queue.pop_front(),
    }
}

/// A pool thread: the oldest work. The decoders it keeps for tasks close
/// once idle (`decode::standalone`), so it wakes for that too.
fn work(pool: &Pool) {
    let mut closes: Option<Instant> = None;
    loop {
        let work = {
            let mut queue = lock(&pool.queue);
            loop {
                if let Some(w) = next(&mut queue) {
                    break Some(w);
                }
                match closes {
                    None => queue = pool.ready.wait(queue).unwrap_or_else(|e| e.into_inner()),
                    Some(at) => {
                        let left = at.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            break None;
                        }
                        queue = pool
                            .ready
                            .wait_timeout(queue, left)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                }
            }
        };
        match work {
            Some(Work::Slices(batch)) => while batch.run_one() {},
            Some(Work::Task(task)) => task.run(),
            None => {}
        }
        closes = crate::decode::standalone::close_idle();
    }
}

/// Hands `task` to the calling thread's pool, which runs it once a thread
/// is free unless it is started elsewhere first. A pool of no threads
/// (one core) never takes it: whoever wants its result runs it.
pub(crate) fn give(task: Arc<dyn Task>) {
    if threads() == 1 {
        return;
    }
    let pool = pool();
    lock(&pool.queue).push_back(Work::Task(task));
    pool.ready.notify_one();
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
    lock(&pool.queue).push_back(Work::Slices(batch.clone()));
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

    /// A task that notes its name when it runs.
    struct Noted(&'static str, Arc<Mutex<Vec<&'static str>>>);

    impl Task for Noted {
        fn run(&self) {
            lock(&self.1).push(self.0);
        }
    }

    fn nothing(_: usize) {}

    #[test]
    fn the_oldest_work_runs_first() {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let job: &'static (dyn Fn(usize) + Sync) = &nothing;
        let mut queue = VecDeque::from([
            Work::Task(Arc::new(Noted("first task", ran.clone()))),
            Work::Slices(Arc::new(Batch {
                job,
                slices: 2,
                next: AtomicUsize::new(0),
                done: Mutex::new(0),
                finished: Condvar::new(),
                panicked: AtomicBool::new(false),
            })),
            Work::Task(Arc::new(Noted("last task", ran.clone()))),
        ]);
        while let Some(work) = next(&mut queue) {
            match work {
                Work::Slices(batch) => {
                    while batch.run_one() {}
                    lock(&ran).push("slices");
                }
                Work::Task(task) => task.run(),
            }
        }
        assert_eq!(*lock(&ran), ["first task", "slices", "last task"]);
    }

    #[test]
    fn a_graph_for_single_frames_runs_in_one_slice() {
        crate::init();
        let mut graph = graph(Reading::Frames);
        // SAFETY: reads the graph's own field.
        assert_eq!(unsafe { (*graph.as_mut_ptr()).nb_threads }, 1);
    }
}
