use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::error::Result;

const LOOK: Duration = Duration::from_millis(100);

const CORES_EACH: usize = 4;

static CPU_ANALYSES: Line = Line::new();

pub(crate) struct Line {
    state: Mutex<State>,
    moved: Condvar,
}

struct State {
    running: usize,
    waiting: VecDeque<u64>,
    next: u64,
}

pub(crate) struct Turn<'a>(&'a Line);

impl Line {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(State {
                running: 0,
                waiting: VecDeque::new(),
                next: 0,
            }),
            moved: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn take(&self, slots: usize) -> Result<Turn<'_>> {
        let mut s = self.lock();
        let me = s.next;
        s.next += 1;
        s.waiting.push_back(me);
        loop {
            if s.waiting.front() == Some(&me) && s.running < slots.max(1) {
                s.waiting.pop_front();
                s.running += 1;
                self.moved.notify_all();
                return Ok(Turn(self));
            }
            if let Err(e) = crate::cancel::stop() {
                s.waiting.retain(|&w| w != me);
                self.moved.notify_all();
                return Err(e);
            }
            s = self
                .moved
                .wait_timeout(s, LOOK)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.0.lock().running -= 1;
        self.0.moved.notify_all();
    }
}

pub(crate) fn cpu_analysis() -> Result<Turn<'static>> {
    let slots = (crate::pool::threads() / CORES_EACH).max(1);
    let asked = Instant::now();
    let turn = CPU_ANALYSES.take(slots)?;
    let waited = asked.elapsed();
    if waited >= LOOK {
        crate::debug!(
            "decoding",
            "an analysis on the CPU waited {:.1} s for one of {slots} turns",
            waited.as_secs_f64()
        );
    }
    Ok(turn)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    use super::*;

    fn waiting(line: &Line) -> usize {
        line.lock().waiting.len()
    }

    fn until_waiting(line: &Line, n: usize) {
        let start = Instant::now();
        while waiting(line) < n {
            assert!(start.elapsed() < Duration::from_secs(5), "never queued");
            std::thread::yield_now();
        }
    }

    #[test]
    fn a_turn_waits_while_every_slot_is_taken() {
        let line = Line::new();
        let first = line.take(1).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _turn = line.take(1).unwrap();
                tx.send(()).unwrap();
            });
            until_waiting(&line, 1);
            let before = rx.try_recv().is_ok();
            drop(first);
            let after = rx.recv_timeout(Duration::from_secs(5)).is_ok();
            assert_eq!((before, after), (false, true));
        });
    }

    #[test]
    fn turns_come_in_the_order_they_were_asked_for() {
        let line = Line::new();
        let first = line.take(1).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            for name in ["a", "b", "c"] {
                let (line, tx) = (&line, tx.clone());
                let queued = waiting(line);
                scope.spawn(move || {
                    let _turn = line.take(1).unwrap();
                    tx.send(name).unwrap();
                });
                until_waiting(line, queued + 1);
            }
            drop(first);
        });
        drop(tx);
        assert_eq!(rx.iter().collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn a_cancelled_wait_leaves_the_line() {
        let line = Line::new();
        let _first = line.take(1).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let _guard = crate::cancel::Guard::set(cancel);
        let waited = line.take(1).is_err();
        assert_eq!((waited, waiting(&line)), (true, 0));
    }
}
