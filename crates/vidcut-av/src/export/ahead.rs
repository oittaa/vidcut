//! Opening the next piece's source while the current piece is read.
//!
//! Starting a piece costs opening the file, seeking and decoding up to its
//! first frame: up to a whole GOP of decoding, and a few round trips on a
//! network share. Doing that on a helper thread while the piece before it
//! encodes takes it off the export's path.

use std::sync::mpsc;
use std::thread;

use crate::error::Result;

type Job<T> = Box<dyn FnOnce() -> Result<T> + Send>;

/// Opens sources one at a time on a helper thread of its own. One
/// long-lived thread, not one per piece: the allocator gives every new
/// thread an arena, and memory spread over many arenas stays resident.
pub(crate) struct Ahead<T> {
    jobs: Option<mpsc::Sender<(usize, Job<T>)>>,
    results: Option<mpsc::Receiver<(usize, Result<T>)>>,
    /// The piece being opened.
    pending: Option<usize>,
}

impl<T: Send + 'static> Ahead<T> {
    pub(crate) fn new() -> Self {
        Self {
            jobs: None,
            results: None,
            pending: None,
        }
    }

    /// Starts opening `piece`'s source.
    pub(crate) fn start(
        &mut self,
        piece: usize,
        open: impl FnOnce() -> Result<T> + Send + 'static,
    ) {
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<(usize, Job<T>)>();
            let (done, results) = mpsc::channel();
            let spawned = thread::Builder::new()
                .name("vidcut-open".into())
                .spawn(move || {
                    for (piece, job) in queue {
                        if done.send((piece, job())).is_err() {
                            break;
                        }
                    }
                });
            if spawned.is_err() {
                return; // the piece opens when it starts
            }
            self.jobs = Some(jobs);
            self.results = Some(results);
        }
        if let Some(jobs) = &self.jobs
            && jobs.send((piece, Box::new(open))).is_ok()
        {
            self.pending = Some(piece);
        }
    }

    /// `piece`'s source if it is being opened ahead, once it is open; `None`
    /// if it isn't (or the helper thread is gone), to open it now.
    pub(crate) fn take(&mut self, piece: usize) -> Option<Result<T>> {
        if self.pending.take() != Some(piece) {
            return None;
        }
        let results = self.results.as_ref()?;
        // Results of pieces that were skipped since are dropped.
        results
            .iter()
            .find_map(|(p, source)| (p == piece).then_some(source))
    }
}

#[cfg(test)]
mod tests {
    use super::Ahead;
    use crate::error::Error;

    #[test]
    fn opens_ahead_in_order_and_skips_the_unwanted() {
        let mut ahead = Ahead::new();
        assert!(ahead.take(0).is_none(), "nothing pending");
        ahead.start(1, || Ok(10));
        assert_eq!(ahead.take(1).unwrap().unwrap(), 10);
        assert!(ahead.take(1).is_none(), "taken once");
        // A piece that isn't the pending one opens itself; the pending result
        // is dropped when a later one is taken.
        ahead.start(2, || Ok(20));
        assert!(ahead.take(3).is_none());
        ahead.start(4, || Err(Error::Invalid("unreadable".into())));
        assert!(ahead.take(4).unwrap().is_err(), "errors come back");
        ahead.start(5, || Ok(50));
        assert_eq!(ahead.take(5).unwrap().unwrap(), 50);
    }
}
