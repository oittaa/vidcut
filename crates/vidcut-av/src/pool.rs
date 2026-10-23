use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use crate::decode::Reading;
use crate::ffmpeg::{ffi, filter};

pub(crate) mod tasks;

pub(crate) use tasks::Task;

struct Batch {
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

enum Work {
    Slices(Arc<Batch>),
    Task(Arc<dyn Task>),
}

struct Pool {
    queue: Mutex<VecDeque<Work>>,
    ready: Condvar,
    threads: usize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

const MOST_FRAME_THREADS: usize = 16;

pub fn threads_for(reading: Reading) -> usize {
    budget(reading, crate::first::yields(), threads())
}

fn budget(reading: Reading, job: bool, cores: usize) -> usize {
    let n = match reading {
        Reading::Frames => return 1,
        Reading::Stream if job => cores / 2,
        Reading::Stream => cores,
    };
    n.clamp(1, MOST_FRAME_THREADS)
}

fn size(job: bool, cores: usize) -> usize {
    let workers = if job {
        budget(Reading::Stream, true, cores)
    } else {
        cores
    };
    workers.saturating_sub(1)
}

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
            threads: size(job, threads()),
        }
    });
    if new {
        for i in 0..pool.threads {
            let _ = std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || {
                    if job {
                        crate::first::as_job();
                    }
                    work(pool)
                });
        }
    }
    pool
}

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

pub(crate) fn give(task: Arc<dyn Task>) {
    let pool = pool();
    if pool.threads == 0 {
        return;
    }
    lock(&pool.queue).push_back(Work::Task(task));
    pool.ready.notify_one();
}

pub fn run(slices: usize, job: &(dyn Fn(usize) + Sync)) {
    match slices {
        0 => return,
        1 => return job(0),
        _ => {}
    }
    let pool = pool();
    if pool.threads == 0 {
        return (0..slices).for_each(job);
    }
    // SAFETY: only the lifetime goes; `job` runs only for slices not done, and `run` waits for all.
    let job: *const (dyn Fn(usize) + Sync + 'static) = unsafe { std::mem::transmute(job) };
    let batch = Arc::new(Batch {
        job,
        slices,
        next: AtomicUsize::new(0),
        done: Mutex::new(0),
        finished: Condvar::new(),
        panicked: AtomicBool::new(false),
    });
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

pub fn graph(reading: Reading) -> filter::Graph {
    let mut graph = filter::Graph::new();
    let slices = threads_for(reading);
    // SAFETY: a graph of no filters yet, which is when FFmpeg takes both.
    unsafe {
        let raw = graph.as_mut_ptr();
        (*raw).execute = Some(execute);
        (*raw).nb_threads = slices as libc::c_int;
    }
    graph
}

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
            // SAFETY: FFmpeg's call for slice `i` of `jobs`; `ret` is null or holds `jobs` results.
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
    fn the_normal_pool_has_a_thread_fewer_than_the_cores() {
        assert_eq!(size(false, 12), 11);
    }

    #[test]
    fn the_jobs_pool_has_a_thread_fewer_than_the_jobs_budget() {
        assert_eq!(size(true, 12), 5);
    }

    #[test]
    fn a_jobs_pool_on_two_cores_has_no_thread() {
        assert_eq!(size(true, 2), 0);
    }

    #[test]
    fn single_frames_take_one_thread() {
        assert_eq!(budget(Reading::Frames, false, 12), 1);
    }

    #[test]
    fn an_interactive_stream_takes_the_cores() {
        assert_eq!(budget(Reading::Stream, false, 12), 12);
    }

    #[test]
    fn a_jobs_stream_takes_half_the_cores() {
        assert_eq!(budget(Reading::Stream, true, 12), 6);
    }

    #[test]
    fn a_jobs_stream_on_one_core_takes_one() {
        assert_eq!(budget(Reading::Stream, true, 1), 1);
    }

    #[test]
    fn a_stream_takes_at_most_16() {
        assert_eq!(budget(Reading::Stream, false, 64), 16);
    }

    #[test]
    fn a_thread_that_yields_to_players_works_for_a_job() {
        let job = std::thread::spawn(|| {
            crate::first::yield_to_players();
            threads_for(Reading::Stream)
        });
        assert_eq!(
            job.join().unwrap(),
            budget(Reading::Stream, true, threads())
        );
    }

    #[test]
    fn a_graph_for_single_frames_runs_in_one_slice() {
        crate::init();
        let mut graph = graph(Reading::Frames);
        // SAFETY: reads the graph's own field.
        assert_eq!(unsafe { (*graph.as_mut_ptr()).nb_threads }, 1);
    }
}
