use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::error::Result;

static CLAIMS: Mutex<usize> = Mutex::new(0);
static FREE: Condvar = Condvar::new();
static BESIDE: AtomicBool = AtomicBool::new(false);

const LOOK: Duration = Duration::from_millis(100);

thread_local! {
    static YIELDS: Cell<bool> = const { Cell::new(false) };
    static ASKS: Cell<bool> = const { Cell::new(false) };
    static HOLDS: Cell<bool> = const { Cell::new(false) };
}

fn claims() -> MutexGuard<'static, usize> {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn yield_to_players() {
    YIELDS.with(|y| y.set(true));
}

pub fn as_job() {
    lowest_priority();
    yield_to_players();
}

pub(crate) fn lowest_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain system calls on this thread's own id.
    unsafe {
        let param = libc::sched_param { sched_priority: 0 };
        libc::sched_setscheduler(libc::gettid(), libc::SCHED_IDLE, &param);
    }
    #[cfg(windows)]
    // SAFETY: sets this thread's own priority.
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_IDLE,
        };
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_IDLE);
    }
    #[cfg(target_os = "macos")]
    // SAFETY: sets this thread's own class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

pub(crate) fn yields() -> bool {
    YIELDS.with(Cell::get)
}

pub(crate) fn set_yields(on: bool) {
    YIELDS.with(|y| y.set(on));
}

pub(crate) fn wait() -> Result<()> {
    if !yields() {
        return Ok(());
    }
    let mut n = claims();
    while *n > 0 && !beside() {
        crate::cancel::stop()?;
        n = FREE
            .wait_timeout(n, LOOK)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    Ok(())
}

fn beside() -> bool {
    if !ASKS.get() {
        return false;
    }
    if !HOLDS.get() {
        let won = BESIDE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        HOLDS.set(won);
    }
    HOLDS.get()
}

pub struct Beside(PhantomData<*const ()>);

impl Beside {
    pub fn ask() -> Self {
        ASKS.set(true);
        Self(PhantomData)
    }
}

impl Drop for Beside {
    fn drop(&mut self) {
        ASKS.set(false);
        if HOLDS.replace(false) {
            let _n = claims();
            BESIDE.store(false, Ordering::Release);
            FREE.notify_all();
        }
    }
}

#[derive(Default)]
pub(crate) struct Claim(bool);

impl Claim {
    pub(crate) fn set(&mut self, on: bool) {
        if on == self.0 {
            return;
        }
        self.0 = on;
        let mut n = claims();
        if on {
            *n += 1;
        } else {
            *n -= 1;
            if *n == 0 {
                FREE.notify_all();
            }
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.set(false);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn as_job_marks_the_thread_to_yield() {
        assert!(!yields());
        as_job();
        assert!(yields());
        set_yields(false);
    }

    #[test]
    fn threads_that_do_not_yield_never_wait() {
        let mut claim = Claim::default();
        claim.set(true);
        assert!(wait().is_ok());
    }

    #[test]
    fn a_cancelled_request_stops_waiting() {
        let mut claim = Claim::default();
        claim.set(true);
        yield_to_players();
        let _cancel = crate::cancel::Guard::set(Arc::new(AtomicBool::new(true)));
        assert!(wait().is_err());
    }
}
