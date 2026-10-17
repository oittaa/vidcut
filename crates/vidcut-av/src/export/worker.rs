use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use crate::error::{Error, Result};

pub(crate) const AHEAD: usize = 3;

enum Message<T> {
    Item(T),
    End,
    Abort,
}

pub(crate) struct Inbox<T> {
    from: Receiver<Message<T>>,
    stop: Arc<AtomicBool>,
}

impl<T> Inbox<T> {
    pub(crate) fn recv(&mut self) -> Result<Option<T>> {
        let message = self.from.recv();
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

fn stopped() -> Error {
    Error::Invalid(super::CANCELLED.into())
}

pub(crate) struct Worker<T, R> {
    to: Option<SyncSender<Message<T>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<R>>>,
}

impl<T: Send + 'static, R: Send + 'static> Worker<T, R> {
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

    pub(crate) fn send(&mut self, item: T) -> Result<()> {
        if let Some(to) = &self.to
            && to.send(Message::Item(item)).is_ok()
        {
            return Ok(());
        }
        match self.join() {
            Some(Err(e)) => Err(e),
            _ => Err(gone()),
        }
    }

    pub(crate) fn finish(mut self) -> Result<R> {
        if let Some(to) = self.to.take() {
            let _ = to.send(Message::End);
        }
        self.join().unwrap_or_else(|| Err(gone()))
    }

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

    fn join(&mut self) -> Option<Result<R>> {
        self.to = None;
        let thread = self.thread.take()?;
        Some(
            thread
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        )
    }

    #[cfg(test)]
    fn try_send(&mut self, item: T) -> bool {
        self.to
            .as_ref()
            .is_some_and(|to| to.try_send(Message::Item(item)).is_ok())
    }
}

fn gone() -> Error {
    Error::Invalid("the encoder's thread ended before the last picture".into())
}

impl<T, R> Drop for Worker<T, R> {
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
        while !stop.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        drop(open);
        job.join().unwrap();
        assert_eq!(taken.load(Ordering::Relaxed), 0);
    }
}
