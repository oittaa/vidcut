//! How many threads the FFmpeg decoders and filter graphs a thread opens
//! take (docs/decisions/gui.md §9). FFmpeg's automatic count is about one a
//! core for each decoder and each graph, so a player, the filmstrip's
//! workers and a job, each on it, put a hundred threads to work on the
//! cores at once, each with a picture of its own. The player and exports
//! keep the automatic count; the app's jobs take a quarter of the cores;
//! single frames take one, as frame threads hand a frame out only once a
//! frame for each thread has gone in. A thread opens on the automatic
//! count unless it says otherwise, and the helper opening sources for it
//! takes its count (`export::ahead`).

use std::cell::Cell;
use std::marker::PhantomData;

use crate::ffmpeg::filter;

/// How many threads FFmpeg takes for each decoder and filter graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threads {
    /// FFmpeg's automatic count: the player and exports.
    Auto,
    /// A quarter of the cores, at least one: jobs nobody watches frame by
    /// frame.
    Quarter,
    /// One: single frames (filmstrip tiles, storyboards, `frame render`).
    One,
}

thread_local! {
    static THREADS: Cell<Threads> = const { Cell::new(Threads::Auto) };
}

/// This thread's count from now on.
pub fn set(threads: Threads) {
    THREADS.with(|t| t.set(threads));
}

/// This thread's count.
pub fn current() -> Threads {
    THREADS.with(Cell::get)
}

/// This thread's count while it lives; the one before comes back after.
pub struct Guard {
    before: Threads,
    _this_thread: PhantomData<*const ()>,
}

impl Guard {
    pub fn set(threads: Threads) -> Self {
        let before = current();
        set(threads);
        Self {
            before,
            _this_thread: PhantomData,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        set(self.before);
    }
}

impl Threads {
    /// FFmpeg's thread count for it (`threads`, `nb_threads`): 0 for the
    /// automatic one.
    pub fn count(self) -> usize {
        self.count_of(std::thread::available_parallelism().map_or(1, |n| n.get()))
    }

    fn count_of(self, cores: usize) -> usize {
        match self {
            Self::Auto => 0,
            Self::Quarter => (cores / 4).max(1),
            Self::One => 1,
        }
    }
}

/// A filter graph whose filters take this thread's count.
pub(crate) fn graph() -> filter::Graph {
    let mut graph = filter::Graph::new();
    // SAFETY: a graph of no filters yet, when its count is set.
    unsafe { (*graph.as_mut_ptr()).nb_threads = current().count() as i32 };
    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarter_of_sixteen_cores_is_four() {
        assert_eq!(Threads::Quarter.count_of(16), 4);
    }

    #[test]
    fn a_quarter_of_two_cores_is_one() {
        assert_eq!(Threads::Quarter.count_of(2), 1);
    }

    #[test]
    fn a_guard_puts_the_count_before_it_back() {
        set(Threads::Quarter);
        drop(Guard::set(Threads::One));
        assert_eq!(current(), Threads::Quarter);
    }

    #[test]
    fn a_graph_takes_the_threads_count() {
        crate::init();
        let _one = Guard::set(Threads::One);
        let mut graph = graph();
        // SAFETY: reads the graph's own field.
        assert_eq!(unsafe { (*graph.as_mut_ptr()).nb_threads }, 1);
    }
}
