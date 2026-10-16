//! The player people watch comes first (player.md §1). While a player
//! plays, or decodes the picture its viewer waits for, decoding waits
//! between frames on threads of work nobody watches frame by frame: those
//! that [`yield_to_players`], and the threads vidcut-av starts for them.
//! They share the GPU's decoder with the player, and no thread priority
//! reaches that.

use std::cell::Cell;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::error::Result;

/// Players that come first now.
static CLAIMS: Mutex<usize> = Mutex::new(0);
static FREE: Condvar = Condvar::new();

/// How often a waiting thread looks whether its request was cancelled.
const LOOK: Duration = Duration::from_millis(100);

thread_local! {
    static YIELDS: Cell<bool> = const { Cell::new(false) };
}

fn claims() -> MutexGuard<'static, usize> {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Decoding on the calling thread waits while a player comes first.
pub fn yield_to_players() {
    YIELDS.with(|y| y.set(true));
}

/// The calling thread works for a job: at the lowest CPU priority, its
/// decoding waiting while a player comes first, and its slices on the
/// jobs' pool ([`crate::pool`]).
pub fn as_job() {
    lowest_priority();
    yield_to_players();
}

/// Gives the calling thread the lowest CPU priority. On Linux that is the
/// idle scheduling class, below nice 19, which the threads it starts later
/// inherit. On Windows it is the idle priority, which threads don't
/// inherit: the threads FFmpeg starts for a job run at the normal one. On
/// macOS it is the utility QoS class, which threads don't inherit either.
/// Not the background class below it: that keeps to the efficiency cores
/// and slows the thread's disk access, so a job wouldn't take the cores and
/// the disk left idle.
///
/// Not nice 19: NVIDIA's driver, opening NVENC, copies the calling thread's
/// nice value onto the process id, which is the main thread, and nothing
/// unprivileged raises it again. A job at nice 19 left the GUI there. In the
/// idle class the nice value stays the process's, so the copy changes
/// nothing.
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

/// Whether decoding on the calling thread waits for players, for a thread
/// it starts to do as it does.
pub(crate) fn yields() -> bool {
    YIELDS.with(Cell::get)
}

pub(crate) fn set_yields(on: bool) {
    YIELDS.with(|y| y.set(on));
}

/// On a thread that yields, waits while a player comes first. A cancelled
/// request stops waiting.
pub(crate) fn wait() -> Result<()> {
    if !yields() {
        return Ok(());
    }
    let mut n = claims();
    while *n > 0 {
        crate::cancel::stop()?;
        n = FREE
            .wait_timeout(n, LOOK)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    Ok(())
}

/// A player's claim to come first, while it is set.
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
