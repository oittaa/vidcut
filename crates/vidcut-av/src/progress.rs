//! Progress of the long request running on this thread, for work that
//! doesn't take a callback of its own: proxy encodes, reached through the
//! editor. Like [`crate::cancel`], the caller installs a [`Guard`] and the
//! work reports to it.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;

use crate::export::Progress;

/// Receives `(file being written, progress)`.
pub type Report = Box<dyn FnMut(&Path, Progress)>;

thread_local! {
    static REPORT: RefCell<Option<Report>> = const { RefCell::new(None) };
}

/// Sends this thread's progress to `report` until dropped.
pub struct Guard {
    prev: Option<Report>,
    /// `Rc` is not `Send`: the slot belongs to this thread.
    _unsend: PhantomData<Rc<()>>,
}

impl Guard {
    pub fn set(report: Report) -> Self {
        let prev = REPORT.with(|slot| slot.replace(Some(report)));
        Self {
            prev,
            _unsend: PhantomData,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        REPORT.with(|slot| *slot.borrow_mut() = prev);
    }
}

/// Reports `progress` on `path` to this thread's [`Guard`], if any.
pub(crate) fn report(path: &Path, progress: Progress) {
    REPORT.with(|slot| {
        if let Some(report) = slot.borrow_mut().as_mut() {
            report(path, progress);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    #[test]
    fn reports_reach_the_guard_only_while_held() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let p = |frames| Progress {
            frames,
            total: 10,
            elapsed: Duration::ZERO,
        };
        report(Path::new("a"), p(1));
        {
            let seen = seen.clone();
            let _guard = Guard::set(Box::new(move |_, pr| seen.lock().unwrap().push(pr.frames)));
            report(Path::new("a"), p(2));
            report(Path::new("a"), p(3));
        }
        report(Path::new("a"), p(4));
        assert_eq!(*seen.lock().unwrap(), vec![2, 3]);
    }
}
