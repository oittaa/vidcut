use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;

static LEFT: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

fn left() -> MutexGuard<'static, Vec<JoinHandle<()>>> {
    LEFT.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn leave(thread: JoinHandle<()>) {
    let mut left = left();
    left.retain(|t| !t.is_finished());
    left.push(thread);
}

pub fn join() {
    let threads = std::mem::take(&mut *left());
    for thread in threads {
        let _ = thread.join();
    }
}
