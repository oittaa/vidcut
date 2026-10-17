use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

thread_local! {
    static CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub struct Guard {
    prev: Option<Arc<AtomicBool>>,
    _unsend: PhantomData<Rc<()>>,
}

impl Guard {
    pub fn set(cancel: Arc<AtomicBool>) -> Self {
        let prev = CANCEL.with(|slot| slot.replace(Some(cancel)));
        Self {
            prev,
            _unsend: PhantomData,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        CANCEL.with(|slot| *slot.borrow_mut() = prev);
    }
}

pub fn current() -> Option<Arc<AtomicBool>> {
    CANCEL.with(|slot| slot.borrow().clone())
}

pub(crate) fn cancelled() -> bool {
    CANCEL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    })
}

pub(crate) fn stop() -> Result<()> {
    if cancelled() {
        Err(Error::Invalid(crate::export::CANCELLED.into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn the_guard_is_visible_only_while_held() {
        assert!(!cancelled());
        let flag = Arc::new(AtomicBool::new(true));
        {
            let _guard = Guard::set(flag.clone());
            assert!(cancelled());
            flag.store(false, Ordering::Relaxed);
            assert!(!cancelled());
            flag.store(true, Ordering::Relaxed);
            assert!(cancelled());
        }
        assert!(!cancelled());
    }
}
