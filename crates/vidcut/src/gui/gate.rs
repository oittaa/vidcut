use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const AWAY: Duration = Duration::from_millis(500);

pub struct Gate {
    state: Mutex<State>,
    changed: Condvar,
    wake: Box<dyn Fn() + Send + Sync>,
}

struct State {
    closed: Option<Instant>,
    passing: usize,
}

pub struct Open<'a>(&'a Gate);

impl Drop for Open<'_> {
    fn drop(&mut self) {
        let mut s = self.0.lock();
        while s.passing > 0 {
            s = self.0.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
        s.closed = Some(Instant::now());
    }
}

struct Passing<'a>(&'a Gate);

impl Drop for Passing<'_> {
    fn drop(&mut self) {
        self.0.lock().passing -= 1;
        self.0.changed.notify_all();
    }
}

impl Gate {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::closed_at(Instant::now(), wake)
    }

    fn closed_at(at: Instant, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            state: Mutex::new(State {
                closed: Some(at),
                passing: 0,
            }),
            changed: Condvar::new(),
            wake: Box::new(wake),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn opened(&self) -> Open<'_> {
        self.open();
        Open(self)
    }

    pub fn open(&self) {
        self.lock().closed = None;
        self.changed.notify_all();
    }

    pub fn pass<T>(&self, f: impl FnOnce() -> T) -> T {
        let mut s = self.lock();
        s.passing += 1;
        let mut woke = false;
        while let Some(at) = s.closed {
            let left = AWAY.saturating_sub(at.elapsed());
            if left.is_zero() {
                break;
            }
            if !woke {
                (self.wake)();
                woke = true;
            }
            s = self
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(s);
        let _passing = Passing(self);
        f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    #[test]
    fn the_ui_closes_the_gate_after_the_submission_it_woke_for() {
        let (woken, wakes) = mpsc::channel();
        let woken = Mutex::new(woken);
        let gate = Arc::new(Gate::new(move || {
            let _ = woken.lock().unwrap().send(());
        }));
        let submitted = Arc::new(AtomicBool::new(false));
        let thread = {
            let (gate, submitted) = (gate.clone(), submitted.clone());
            std::thread::spawn(move || gate.pass(|| submitted.store(true, Ordering::SeqCst)))
        };
        wakes.recv().unwrap();
        drop(gate.opened());
        assert!(submitted.load(Ordering::SeqCst));
        thread.join().unwrap();
    }

    #[test]
    fn a_ui_away_lets_submissions_through_without_waking_it() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = wakes.clone();
        let away = Instant::now().checked_sub(AWAY).unwrap();
        let gate = Gate::closed_at(away, move || {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(gate.pass(|| 7), 7);
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
    }
}
