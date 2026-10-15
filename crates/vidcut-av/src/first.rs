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

/// Whether decoding on the calling thread waits for players, for a thread
/// it starts to do as it does.
pub(crate) fn yields() -> bool {
    YIELDS.with(Cell::get)
}

pub(crate) fn set_yields(on: bool) {
    YIELDS.with(|y| y.set(on));
}

/// Whether something comes first now: decoding on a thread that yields
/// would wait.
pub fn waits() -> bool {
    *claims() > 0
}

/// On a thread that yields, waits while something comes first ([`Claim`]).
/// A cancelled request stops waiting.
pub fn wait() -> Result<()> {
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

/// A claim to come first, while it is set: a player's.
#[derive(Default)]
pub struct Claim(bool);

impl Claim {
    pub fn set(&mut self, on: bool) {
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
    fn a_claim_makes_threads_that_yield_wait() {
        let mut claim = Claim::default();
        claim.set(true);
        assert!(waits());
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
