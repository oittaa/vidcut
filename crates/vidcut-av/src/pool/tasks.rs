use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

pub(crate) trait Task: Send + Sync {
    fn run(&self);
}

const LOOK: Duration = Duration::from_millis(100);

enum State<J, R> {
    Waiting(J),
    Running,
    Done(Option<R>),
    Dropped,
}

struct Slot<J, R> {
    state: Mutex<State<J, R>>,
    done: Condvar,
    run: fn(J) -> R,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<J, R> Slot<J, R> {
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

pub(crate) struct Ordered<J, R> {
    slots: VecDeque<Arc<Slot<J, R>>>,
    run: fn(J) -> R,
    give: fn(Arc<dyn Task>),
}

impl<J: Send + 'static, R: Send + 'static> Ordered<J, R> {
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

    pub(crate) fn clear(&mut self) {
        for slot in self.slots.drain(..) {
            *lock(&slot.state) = State::Dropped;
        }
    }

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
        static GIVEN: RefCell<Vec<Arc<dyn Task>>> = const { RefCell::new(Vec::new()) };
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
