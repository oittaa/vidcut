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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn join_waits_for_a_thread_left() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        leave(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        }));
        join();
        assert!(done.load(Ordering::SeqCst));
    }
}
