//! Background threads for slow media work (thumbnails, preview fillers,
//! remuxes). Jobs report back to the session as messages.

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

type Job = Box<dyn FnOnce() + Send>;

pub struct Workers {
    tx: Sender<Job>,
}

impl Workers {
    pub fn new(threads: usize) -> Self {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads.max(1) {
            let rx = rx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("vidcut-worker-{i}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock() {
                            Ok(r) => r.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                });
        }
        Self { tx }
    }

    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.tx.send(Box::new(job));
    }
}
