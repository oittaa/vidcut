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
pub(crate) struct Ahead<T, K = ()> {
    jobs: Option<mpsc::Sender<(u64, Job<T>)>>,
    results: Option<mpsc::Receiver<(u64, Result<T>)>>,
    /// The piece being opened, its job's number and what it opens.
    pending: Option<(usize, u64, K)>,
    /// A job forgotten while it was pending (the pieces were numbered
    /// anew): a new list asking for the same source takes it up again.
    forgotten: Option<(u64, K)>,
    /// The next job's number. Results are matched to jobs by it, so a piece
    /// numbered alike in a new list of pieces never takes an older job's.
    next: u64,
}

impl<T: Send + 'static, K: PartialEq> Ahead<T, K> {
    pub(crate) fn new() -> Self {
        Self {
            jobs: None,
            results: None,
            pending: None,
            forgotten: None,
            next: 0,
        }
    }

    /// Forgets the piece being opened: the pieces are numbered anew. Its
    /// source is dropped when it arrives, unless a piece of the new list
    /// asks for the same one first.
    pub(crate) fn forget(&mut self) {
        if let Some((_, id, key)) = self.pending.take() {
            self.forgotten = Some((id, key));
        }
    }

    /// Starts opening `piece`'s source, which `key` names.
    pub(crate) fn start(
        &mut self,
        piece: usize,
        key: K,
        open: impl FnOnce() -> Result<T> + Send + 'static,
    ) {
        // A forgotten job for another source stays forgotten.
        if let Some((id, k)) = self.forgotten.take()
            && k == key
        {
            self.pending = Some((piece, id, k));
            return;
        }
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<(u64, Job<T>)>();
            let (done, results) = mpsc::channel();
            let spawned = thread::Builder::new()
                .name("vidcut-open".into())
                .spawn(move || {
                    for (id, job) in queue {
                        if done.send((id, job())).is_err() {
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
        let id = self.next;
        self.next += 1;
        if let Some(jobs) = &self.jobs
            && jobs.send((id, Box::new(open))).is_ok()
        {
            self.pending = Some((piece, id, key));
        }
    }

    /// `piece`'s source if it is being opened ahead, once it is open; `None`
    /// if it isn't (or the helper thread is gone), to open it now.
    pub(crate) fn take(&mut self, piece: usize) -> Option<Result<T>> {
        let (pending, id, _) = self.pending.take()?;
        if pending != piece {
            return None;
        }
        let results = self.results.as_ref()?;
        // Results of jobs that were skipped or forgotten since are dropped.
        results
            .iter()
            .find_map(|(i, source)| (i == id).then_some(source))
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
        ahead.start(1, (), || Ok(10));
        assert_eq!(ahead.take(1).unwrap().unwrap(), 10);
        assert!(ahead.take(1).is_none(), "taken once");
        // A piece that isn't the pending one opens itself; the pending result
        // is dropped when a later one is taken.
        ahead.start(2, (), || Ok(20));
        assert!(ahead.take(3).is_none());
        ahead.start(4, (), || Err(Error::Invalid("unreadable".into())));
        assert!(ahead.take(4).unwrap().is_err(), "errors come back");
        ahead.start(5, (), || Ok(50));
        assert_eq!(ahead.take(5).unwrap().unwrap(), 50);
    }

    #[test]
    fn a_forgotten_job_is_no_new_pieces_source_unless_it_is_the_same() {
        // Pieces numbered anew: piece 1 of the new list must not take the
        // source opened for piece 1 of the old one.
        let mut ahead = Ahead::new();
        ahead.start(1, "a", || Ok(11));
        ahead.forget();
        assert!(ahead.take(1).is_none(), "forgotten");
        ahead.start(1, "b", || Ok(12));
        assert_eq!(ahead.take(1).unwrap().unwrap(), 12);
        // The same source asked for again is taken up, not opened twice.
        ahead.start(3, "c", || Ok(13));
        ahead.forget();
        ahead.start(2, "c", || panic!("opened twice"));
        assert_eq!(ahead.take(2).unwrap().unwrap(), 13);
    }
}
