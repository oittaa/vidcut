//! A job's encoders on a thread of their own. The job's thread decodes the
//! frames and makes their pictures, and hands them over in order through a
//! channel that holds a few. The encoders' thread encodes them and writes
//! the file. Decoding and the GPU's picture making go on while an encoder
//! waits for the GPU.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use crate::error::{Error, Result};

/// The frames a job makes ahead of its encoders at most: enough to keep
/// the encoder fed, few enough that the memory and video memory they hold
/// stay small.
pub(crate) const AHEAD: usize = 3;

/// What the job sends its work.
enum Message<T> {
    Item(T),
    /// The last item went: the work finishes.
    End,
    /// The job failed: the work stops after the items sent before.
    Abort,
}

/// The items handed to a [`Worker`]'s work, in the order they were sent.
pub(crate) struct Inbox<T> {
    from: Receiver<Message<T>>,
    stop: Arc<AtomicBool>,
}

impl<T> Inbox<T> {
    /// The next item, once it comes; `None` after the last. Once the job
    /// stopped, an error: the work ends there, finishing nothing.
    pub(crate) fn recv(&mut self) -> Result<Option<T>> {
        let message = self.from.recv();
        // A cancelled job's items still waiting are dropped.
        if self.stop.load(Ordering::Acquire) {
            return Err(stopped());
        }
        match message {
            Ok(Message::Item(item)) => Ok(Some(item)),
            Ok(Message::End) => Ok(None),
            Ok(Message::Abort) | Err(_) => Err(stopped()),
        }
    }
}

/// The error a work stopped by its job ends with: a cancelled job's, which
/// the job's own error replaces ([`Worker::abandon`]).
fn stopped() -> Error {
    Error::Invalid(super::CANCELLED.into())
}

/// Work on a thread of its own, fed items in order through a channel that
/// holds a few: a job's encoders, fed its pictures.
pub(crate) struct Worker<T, R> {
    to: Option<SyncSender<Message<T>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<R>>>,
}

impl<T: Send + 'static, R: Send + 'static> Worker<T, R> {
    /// Starts `work` on a thread of its own, fed through a channel that
    /// holds `holds` items. The copies it makes count for this thread's job
    /// ([`crate::copies`]), and it yields to players as this thread does.
    pub(crate) fn spawn(
        holds: usize,
        work: impl FnOnce(Inbox<T>) -> Result<R> + Send + 'static,
    ) -> Result<Self> {
        let (to, from) = sync_channel(holds);
        let stop = Arc::new(AtomicBool::new(false));
        let inbox = Inbox {
            from,
            stop: stop.clone(),
        };
        let copies = crate::copies::current();
        // It works for whom this thread works ([`crate::first::as_job`]).
        let yields = crate::first::yields();
        let thread = std::thread::Builder::new()
            .name("vidcut-encode".into())
            .spawn(move || {
                crate::first::set_yields(yields);
                let _counting = crate::copies::Guard::maybe(copies);
                work(inbox)
            })
            .map_err(|e| Error::Invalid(format!("starting the encoder's thread: {e}")))?;
        Ok(Self {
            to: Some(to),
            stop,
            thread: Some(thread),
        })
    }

    /// Hands `item` to the work, waiting while the channel is full. Once
    /// the work failed, its error.
    pub(crate) fn send(&mut self, item: T) -> Result<()> {
        if let Some(to) = &self.to
            && to.send(Message::Item(item)).is_ok()
        {
            return Ok(());
        }
        // The work ends before the last item only when it fails.
        match self.join() {
            Some(Err(e)) => Err(e),
            _ => Err(gone()),
        }
    }

    /// The last item went: the work finishes, and its result.
    pub(crate) fn finish(mut self) -> Result<R> {
        if let Some(to) = self.to.take() {
            // A work that failed already says why when it is joined.
            let _ = to.send(Message::End);
        }
        self.join().unwrap_or_else(|| Err(gone()))
    }

    /// The job failed with `why`: the work stops, at once when the job was
    /// cancelled, otherwise after the items sent before. The error is the
    /// first in the order of the items: the work's with one of those, else
    /// `why`.
    pub(crate) fn abandon(mut self, why: Error) -> Error {
        if super::is_cancelled(&why) {
            self.stop.store(true, Ordering::Release);
        } else if let Some(to) = &self.to {
            let _ = to.send(Message::Abort);
        }
        match self.join() {
            Some(Err(e)) if !super::is_cancelled(&e) => e,
            _ => why,
        }
    }

    /// The work's result once its thread ended; `None` if it was waited for
    /// before. A panic there goes on here.
    fn join(&mut self) -> Option<Result<R>> {
        self.to = None;
        let thread = self.thread.take()?;
        Some(
            thread
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        )
    }

    /// Hands `item` to the work if the channel has room.
    #[cfg(test)]
    fn try_send(&mut self, item: T) -> bool {
        self.to
            .as_ref()
            .is_some_and(|to| to.try_send(Message::Item(item)).is_ok())
    }
}

/// What a work that ended without saying why gives.
fn gone() -> Error {
    Error::Invalid("the encoder's thread ended before the last picture".into())
}

impl<T, R> Drop for Worker<T, R> {
    /// The work stops at once, and its thread is waited for: the file it
    /// wrote is closed by then, for the job to remove.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.to = None;
        if let Some(thread) = self.thread.take()
            && let Err(panic) = thread.join()
            && !std::thread::panicking()
        {
            std::panic::resume_unwind(panic);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    use super::*;

    /// Work that keeps every item it takes.
    fn keeping() -> Worker<u32, Vec<u32>> {
        Worker::spawn(AHEAD, |mut items| {
            let mut kept = Vec::new();
            while let Some(item) = items.recv()? {
                kept.push(item);
            }
            Ok(kept)
        })
        .unwrap()
    }

    /// Work that fails on item `bad`.
    fn failing_on(bad: u32) -> Worker<u32, ()> {
        Worker::spawn(AHEAD, move |mut items| {
            while let Some(item) = items.recv()? {
                if item == bad {
                    return Err(Error::Invalid(format!("the encoder failed on item {bad}")));
                }
            }
            Ok(())
        })
        .unwrap()
    }

    #[test]
    fn items_reach_the_work_in_order() {
        let mut work = keeping();
        for item in 0..10 {
            work.send(item).unwrap();
        }
        assert_eq!(work.finish().unwrap(), (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn the_channel_holds_three_items() {
        let (open, gate) = mpsc::channel::<()>();
        let mut work = Worker::spawn(AHEAD, move |mut items: Inbox<u32>| {
            let _ = gate.recv();
            while items.recv()?.is_some() {}
            Ok(())
        })
        .unwrap();
        let held = (0..10).take_while(|&item| work.try_send(item)).count();
        drop(open);
        work.finish().unwrap();
        assert_eq!(held, 3);
    }

    #[test]
    fn an_error_of_the_work_reaches_the_job() {
        let mut work = failing_on(2);
        let err = match (0..10).try_for_each(|item| work.send(item)) {
            Ok(()) => work.finish().unwrap_err(),
            Err(e) => e,
        };
        assert_eq!(err.to_string(), "the encoder failed on item 2");
    }

    #[test]
    fn the_works_error_on_an_item_sent_before_the_jobs_error_comes_first() {
        let mut work = failing_on(1);
        for item in 0..2 {
            work.send(item).unwrap();
        }
        let err = work.abandon(Error::Invalid("the decoder failed on item 2".into()));
        assert_eq!(err.to_string(), "the encoder failed on item 1");
    }

    #[test]
    fn a_cancelled_job_leaves_the_items_still_waiting_alone() {
        let (open, gate) = mpsc::channel::<()>();
        let taken = Arc::new(AtomicUsize::new(0));
        let counted = taken.clone();
        let mut work = Worker::spawn(AHEAD, move |mut items: Inbox<u32>| {
            let _ = gate.recv();
            while items.recv()?.is_some() {
                counted.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        })
        .unwrap();
        for item in 0..3 {
            work.send(item).unwrap();
        }
        let stop = work.stop.clone();
        let cancelled = Error::Invalid(super::super::CANCELLED.into());
        let job = std::thread::spawn(move || work.abandon(cancelled));
        // The work goes on once the job has stopped it.
        while !stop.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        drop(open);
        job.join().unwrap();
        assert_eq!(taken.load(Ordering::Relaxed), 0);
    }
}
