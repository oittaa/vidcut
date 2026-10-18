use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

use crate::error::Result;

type Job<T> = Box<dyn FnOnce() -> Result<T> + Send>;

struct State<T> {
    queued: Option<(u64, Job<T>)>,
    running: Option<u64>,
    done: Option<(u64, Result<T>)>,
    wanted: Option<u64>,
    closed: bool,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
    stop: Arc<AtomicBool>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn want<T>(s: &mut State<T>, shared: &Shared<T>, id: Option<u64>) {
    s.wanted = id;
    if s.running.is_some() && s.running != id {
        shared.stop.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct Ahead<T, K = ()> {
    shared: Option<Arc<Shared<T>>>,
    helper: Option<thread::JoinHandle<()>>,
    pending: Option<(usize, u64, K)>,
    forgotten: Option<(u64, K)>,
    next: u64,
}

impl<T: Send + 'static, K: PartialEq> Ahead<T, K> {
    pub(crate) fn new() -> Self {
        Self {
            shared: None,
            helper: None,
            pending: None,
            forgotten: None,
            next: 0,
        }
    }

    pub(crate) fn forget(&mut self) {
        if let Some((_, id, key)) = self.pending.take() {
            self.forgotten = Some((id, key));
        }
    }

    pub(crate) fn start(
        &mut self,
        piece: usize,
        key: K,
        open: impl FnOnce() -> Result<T> + Send + 'static,
    ) {
        if let Some((id, k)) = self.forgotten.take()
            && k == key
            && self.alive(id)
        {
            self.pending = Some((piece, id, k));
            return;
        }
        let shared = match &self.shared {
            Some(s) => s.clone(),
            None => match spawn() {
                Some((s, helper)) => {
                    self.shared = Some(s.clone());
                    self.helper = Some(helper);
                    s
                }
                None => return,
            },
        };
        let id = self.next;
        self.next += 1;
        let replaced = {
            let mut s = lock(&shared.state);
            let replaced = s.queued.replace((id, Box::new(open)));
            want(&mut s, &shared, Some(id));
            replaced
        };
        drop(replaced);
        shared.changed.notify_all();
        self.pending = Some((piece, id, key));
    }

    fn alive(&self, id: u64) -> bool {
        let Some(shared) = &self.shared else {
            return false;
        };
        let s = lock(&shared.state);
        s.queued.as_ref().is_some_and(|(i, _)| *i == id)
            || s.running == Some(id)
            || s.done.as_ref().is_some_and(|(i, _)| *i == id)
    }

    pub(crate) fn take(&mut self, piece: usize) -> Option<Result<T>> {
        let (pending, id, _) = self.pending.take()?;
        if pending != piece {
            let shared = self.shared.as_ref()?;
            let mut s = lock(&shared.state);
            want(&mut s, shared, None);
            drop(s);
            shared.changed.notify_all();
            return None;
        }
        self.collect(id)
    }

    pub(crate) fn reclaim(&mut self, wanted: impl Fn(&K) -> bool) -> Option<Result<T>> {
        let id = match (&self.pending, &self.forgotten) {
            (Some((_, id, k)), _) if wanted(k) => {
                let id = *id;
                self.pending = None;
                id
            }
            (_, Some((id, k))) if wanted(k) => {
                let id = *id;
                self.forgotten = None;
                id
            }
            _ => return None,
        };
        self.collect(id)
    }

    fn collect(&mut self, id: u64) -> Option<Result<T>> {
        let shared = self.shared.as_ref()?;
        let mut s = lock(&shared.state);
        loop {
            if s.done.as_ref().is_some_and(|(i, _)| *i == id) {
                let (_, source) = s.done.take().expect("checked");
                return Some(source);
            }
            let coming = s.queued.as_ref().is_some_and(|(i, _)| *i == id) || s.running == Some(id);
            if !coming {
                return None;
            }
            s = shared.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl<T, K> Drop for Ahead<T, K> {
    fn drop(&mut self) {
        let Some(shared) = &self.shared else {
            return;
        };
        let queued = {
            let mut s = lock(&shared.state);
            s.closed = true;
            s.wanted = None;
            shared.stop.store(true, Ordering::Relaxed);
            s.queued.take()
        };
        drop(queued);
        shared.changed.notify_all();
        if let Some(helper) = self.helper.take() {
            crate::ending::leave(helper);
        }
    }
}

fn spawn<T: Send + 'static>() -> Option<(Arc<Shared<T>>, thread::JoinHandle<()>)> {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            queued: None,
            running: None,
            done: None,
            wanted: None,
            closed: false,
        }),
        changed: Condvar::new(),
        stop: Arc::new(AtomicBool::new(false)),
    });
    let yields = crate::first::yields();
    let copies = crate::copies::current();
    let worker = shared.clone();
    let helper = thread::Builder::new()
        .name("vidcut-open".into())
        .spawn(move || {
            crate::first::set_yields(yields);
            let _counting = crate::copies::Guard::maybe(copies);
            let _stop = crate::cancel::Guard::set(worker.stop.clone());
            work(&worker);
        })
        .ok()?;
    Some((shared, helper))
}

fn work<T>(shared: &Shared<T>) {
    loop {
        let (id, job) = {
            let mut s = lock(&shared.state);
            loop {
                if s.closed {
                    let left = s.done.take();
                    drop(s);
                    drop(left);
                    return;
                }
                let wanted = s.wanted;
                if let Some(unwanted) = s.done.take_if(|(i, _)| Some(*i) != wanted) {
                    drop(s);
                    drop(unwanted);
                    s = lock(&shared.state);
                    continue;
                }
                if let Some(job) = s.queued.take() {
                    s.running = Some(job.0);
                    shared.stop.store(false, Ordering::Relaxed);
                    break job;
                }
                s = shared.changed.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        let result = job();
        let unwanted = {
            let mut s = lock(&shared.state);
            s.running = None;
            if s.wanted == Some(id) && !s.closed {
                s.done.replace((id, result))
            } else {
                Some((id, result))
            }
        };
        shared.changed.notify_all();
        drop(unwanted);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    struct Noted(Arc<Mutex<Option<String>>>);

    impl Drop for Noted {
        fn drop(&mut self) {
            let name = thread::current().name().unwrap_or("?").to_string();
            *lock(&self.0) = Some(name);
        }
    }

    fn noted() -> (Arc<Mutex<Option<String>>>, Noted) {
        let on = Arc::new(Mutex::new(None));
        (on.clone(), Noted(on))
    }

    fn soon(what: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if what() {
                return true;
            }
            thread::sleep(Duration::from_millis(1));
        }
        what()
    }

    #[test]
    fn a_forgotten_job_is_no_new_pieces_source_unless_it_is_the_same() {
        let mut ahead = Ahead::new();
        ahead.start(1, "a", || Ok(11));
        ahead.forget();
        assert!(ahead.take(1).is_none(), "forgotten");
        ahead.start(1, "b", || Ok(12));
        assert_eq!(ahead.take(1).unwrap().unwrap(), 12);
        ahead.start(3, "c", || Ok(13));
        ahead.forget();
        ahead.start(2, "c", || panic!("opened twice"));
        assert_eq!(ahead.take(2).unwrap().unwrap(), 13);
    }

    #[test]
    fn a_forgotten_job_of_the_same_file_is_reclaimed() {
        let mut ahead = Ahead::new();
        ahead.start(1, ("a", 5), || Ok(11));
        ahead.forget();
        assert!(ahead.reclaim(|k| k.0 == "b").is_none());
        assert_eq!(ahead.reclaim(|k| k.0 == "a").unwrap().unwrap(), 11);
        assert!(ahead.reclaim(|k| k.0 == "a").is_none(), "taken once");
    }

    #[test]
    fn a_source_nobody_wants_closes_on_the_helper_thread() {
        let (dropped, source) = noted();
        let (began, begun) = mpsc::channel();
        let mut ahead = Ahead::new();
        ahead.start(1, "a", move || {
            let _ = began.send(());
            Ok(source)
        });
        begun.recv().unwrap();
        ahead.forget();
        ahead.start(2, "b", || Ok(noted().1));
        assert!(soon(|| lock(&dropped).is_some()));
        assert_eq!(lock(&dropped).as_deref(), Some("vidcut-open"));
        assert!(ahead.take(2).unwrap().is_ok());
    }

    #[test]
    fn taken_for_another_piece_the_source_is_dropped() {
        let (dropped, source) = noted();
        let mut ahead = Ahead::new();
        ahead.start(1, "a", move || Ok(source));
        assert!(ahead.take(2).is_none());
        assert!(soon(|| lock(&dropped).is_some()));
        assert_eq!(lock(&dropped).as_deref(), Some("vidcut-open"));
    }

    #[test]
    fn a_start_before_the_one_before_began_replaces_it() {
        let (began, begun) = mpsc::channel();
        let (go, gate) = mpsc::channel::<()>();
        let mut ahead = Ahead::new();
        ahead.start(0, "gate", move || {
            let _ = began.send(());
            let _ = gate.recv();
            Ok(0)
        });
        begun.recv().unwrap();
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        ahead.forget();
        ahead.start(1, "a", move || {
            flag.store(true, Ordering::Relaxed);
            Ok(1)
        });
        ahead.start(2, "b", || Ok(2));
        go.send(()).unwrap();
        assert_eq!(ahead.take(2).unwrap().unwrap(), 2);
        assert!(!ran.load(Ordering::Relaxed), "replaced before it began");
    }

    #[test]
    fn a_running_job_nobody_wants_stops() {
        let (began, begun) = mpsc::channel();
        let mut ahead = Ahead::new();
        ahead.start(1, "a", move || -> Result<i32> {
            let _ = began.send(());
            loop {
                crate::cancel::stop()?;
                thread::sleep(Duration::from_millis(1));
            }
        });
        begun.recv().unwrap();
        ahead.forget();
        ahead.start(2, "b", || Ok(2));
        assert_eq!(ahead.take(2).unwrap().unwrap(), 2);
    }
}
