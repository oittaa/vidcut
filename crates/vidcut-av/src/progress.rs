use std::cell::RefCell;
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use crate::export::{Note, Progress};

pub type Report = Box<dyn FnMut(&Path, Progress)>;

thread_local! {
    static REPORT: RefCell<Option<Report>> = const { RefCell::new(None) };
}

pub struct Guard {
    prev: Option<Report>,
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

pub(crate) fn report(path: &Path, progress: Progress) {
    REPORT.with(|slot| {
        if let Some(report) = slot.borrow_mut().as_mut() {
            report(path, progress);
        }
    });
}

pub type Notes = Arc<dyn Fn(Note) + Send + Sync>;

thread_local! {
    static NOTES: RefCell<Option<Notes>> = const { RefCell::new(None) };
}

pub struct Noting {
    prev: Option<Notes>,
    _unsend: PhantomData<Rc<()>>,
}

impl Noting {
    pub fn set(notes: Notes) -> Self {
        let prev = NOTES.with(|slot| slot.replace(Some(notes)));
        Self {
            prev,
            _unsend: PhantomData,
        }
    }
}

impl Drop for Noting {
    fn drop(&mut self) {
        let prev = self.prev.take();
        NOTES.with(|slot| *slot.borrow_mut() = prev);
    }
}

pub fn notes() -> Option<Notes> {
    NOTES.with(|slot| slot.borrow().clone())
}

pub(crate) fn note(note: Note) {
    if let Some(notes) = notes() {
        notes(note);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    #[test]
    fn reports_reach_the_guard_only_while_held() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let p = |frames| Progress::single(frames, 10, Duration::ZERO);
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
