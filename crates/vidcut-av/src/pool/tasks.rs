//! Tasks a thread hands its pool in order and takes back in that order: a
//! stream's frames that stand alone, each decoded on whichever thread is
//! free (docs/decisions/threads.md §4.2).
//!
//! A task the pool hasn't started when its result is wanted runs on the
//! thread that wants it, as a call's slices do, so taking never waits for
//! work queued behind other work.

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Work for one pool thread.
pub(crate) trait Task: Send + Sync {
    /// Runs it, unless it was started elsewhere or dropped.
    fn run(&self);
}

/// How often a thread waiting for a result looks whether its request was
/// cancelled.
const LOOK: Duration = Duration::from_millis(100);

enum State<J, R> {
    Waiting(J),
    Running,
    /// `None` when it panicked.
    Done(Option<R>),
    Dropped,
}

/// One task: its job until a thread starts it, then its result.
struct Slot<J, R> {
    state: Mutex<State<J, R>>,
    done: Condvar,
    run: fn(J) -> R,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<J, R> Slot<J, R> {
    /// Its job, if no thread has started it.
    fn claim(&self) -> Option<J> {
        let mut state = lock(&self.state);
        match std::mem::replace(&mut *state, State::Running) {
            State::Waiting(job) => Some(job),
            other => {
                *state = other;
                None
            }
        }
    }

    /// Runs `job` with `run`, keeping its result unless it was dropped
    /// meanwhile.
    fn finish(&self, job: J, run: impl FnOnce(J) -> R) {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| run(job))).ok();
        let mut state = lock(&self.state);
        if matches!(*state, State::Running) {
            *state = State::Done(result);
        }
        self.done.notify_all();
    }
}

impl<J: Send, R: Send> Task for Slot<J, R> {
    fn run(&self) {
        if let Some(job) = self.claim() {
            self.finish(job, self.run);
        }
    }
}

/// Jobs run by `run` on the pool, their results taken in the order given.
pub(crate) struct Ordered<J, R> {
    slots: VecDeque<Arc<Slot<J, R>>>,
    run: fn(J) -> R,
    /// Hands a task to the pool.
    give: fn(Arc<dyn Task>),
}

impl<J: Send + 'static, R: Send + 'static> Ordered<J, R> {
    /// Jobs run by `run` on the pool of the thread that gives them
    /// ([`super::give`]).
    pub(crate) fn new(run: fn(J) -> R) -> Self {
        Self::with(run, super::give)
    }

    fn with(run: fn(J) -> R, give: fn(Arc<dyn Task>)) -> Self {
        Self {
            slots: VecDeque::new(),
            run,
            give,
        }
    }

    /// Tasks given and not taken: waiting, running or done.
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(crate) fn push(&mut self, job: J) {
        let slot = Arc::new(Slot {
            state: Mutex::new(State::Waiting(job)),
            done: Condvar::new(),
            run: self.run,
        });
        (self.give)(slot.clone());
        self.slots.push_back(slot);
    }

    /// Drops every task: those not started never run, and the results of
    /// the others are let go as they come.
    pub(crate) fn clear(&mut self) {
        for slot in self.slots.drain(..) {
            *lock(&slot.state) = State::Dropped;
        }
    }

    /// The oldest task's result; `None` when none is left. Not started, it
    /// runs here with `inline`. Running elsewhere, it is waited for, and
    /// between looks `stop` may end the wait, the task left running. The
    /// tasks after it stay the pool's: the taker running one would give
    /// the pool no new ones meanwhile. A task that panicked panics here.
    pub(crate) fn take<E>(
        &mut self,
        mut inline: impl FnMut(J) -> R,
        stop: impl Fn() -> Result<(), E>,
    ) -> Result<Option<R>, E> {
        let Some(head) = self.slots.front().cloned() else {
            return Ok(None);
        };
        loop {
            if let Some(job) = head.claim() {
                head.finish(job, &mut inline);
            }
            let mut state = lock(&head.state);
            if matches!(*state, State::Done(_)) {
                let State::Done(result) = std::mem::replace(&mut *state, State::Dropped) else {
                    unreachable!("done")
                };
                drop(state);
                self.slots.pop_front();
                return Ok(Some(result.expect("a task of the pool's panicked")));
            }
            if matches!(*state, State::Running) {
                drop(head.done.wait_timeout(state, LOOK));
            }
            stop()?;
        }
    }
}

impl<J, R> Drop for Ordered<J, R> {
    fn drop(&mut self) {
        for slot in self.slots.drain(..) {
            *lock(&slot.state) = State::Dropped;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::convert::Infallible;

    use super::*;

    thread_local! {
        /// The tasks given to the test's pool, which runs them when the
        /// test says.
        static GIVEN: RefCell<Vec<Arc<dyn Task>>> = const { RefCell::new(Vec::new()) };
        /// Jobs [`double`] ran.
        static RAN: Cell<usize> = const { Cell::new(0) };
    }

    fn keep(task: Arc<dyn Task>) {
        GIVEN.with_borrow_mut(|g| g.push(task));
    }

    fn double(n: u32) -> u32 {
        RAN.set(RAN.get() + 1);
        2 * n
    }

    fn given() -> Vec<Arc<dyn Task>> {
        GIVEN.take()
    }

    fn never(_: u32) -> u32 {
        unreachable!("the pool ran it")
    }

    fn go_on() -> Result<(), Infallible> {
        Ok(())
    }

    #[test]
    fn results_come_back_in_the_order_given() {
        let mut tasks = Ordered::with(double, keep);
        for n in [1, 2, 3] {
            tasks.push(n);
        }
        given().iter().rev().for_each(|t| t.run());
        let taken: Vec<_> = (0..3).map(|_| tasks.take(never, go_on)).collect();
        assert_eq!(taken, [Ok(Some(2)), Ok(Some(4)), Ok(Some(6))]);
    }

    #[test]
    fn a_task_not_started_runs_on_the_taker() {
        let mut tasks = Ordered::with(double, keep);
        tasks.push(5);
        assert_eq!(tasks.take(|n| n + 100, go_on), Ok(Some(105)));
    }

    #[test]
    fn a_task_taken_back_does_nothing_on_the_pool() {
        let mut tasks = Ordered::with(double, keep);
        tasks.push(5);
        tasks.take(|n| n + 100, go_on).unwrap();
        given().iter().for_each(|t| t.run());
        assert_eq!(RAN.get(), 0);
    }

    #[test]
    fn a_seek_drops_the_tasks_not_started() {
        let mut tasks = Ordered::with(double, keep);
        tasks.push(1);
        tasks.push(2);
        tasks.clear();
        given().iter().for_each(|t| t.run());
        assert_eq!(RAN.get(), 0);
    }

    #[test]
    fn nothing_given_takes_nothing() {
        let mut tasks = Ordered::<u32, u32>::with(double, keep);
        assert_eq!(tasks.take(never, go_on), Ok(None));
    }
}
