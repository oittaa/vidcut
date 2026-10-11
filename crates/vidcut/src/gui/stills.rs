//! Exact single frames for the GUI (the Trim Editor and Dynamic Trim's
//! two-up), rendered by the export engine on a background thread so they
//! match export and never block the UI.
//!
//! A running Dynamic Trim asks for new frames on every redraw, far faster
//! than they render. So the thread renders only the newest frame each cell
//! asked for and skips the rest, and a cell keeps its last picture until
//! the next one is ready.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

/// A one-frame render: a piece (`frames == 1`) at a size.
#[derive(Debug, Clone, PartialEq)]
pub struct Still {
    pub piece: VideoPiece,
    pub fps: Rational,
    pub width: u32,
    pub height: u32,
}

/// Hashable identity of a [`Still`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key(String);

impl Still {
    fn key(&self) -> Key {
        Key(format!(
            "{}|{}|{}x{}",
            serde_json::to_string(&self.piece).unwrap_or_default(),
            self.fps,
            self.width,
            self.height
        ))
    }
}

enum State {
    Pending,
    Ready(egui::TextureHandle),
    Failed,
}

/// A frame to render, for one of the cells showing them.
struct Job {
    key: Key,
    still: Still,
    cell: usize,
}

enum Outcome {
    Rendered(Result<vidcut_av::thumbs::Image, String>),
    /// Its cell asked for a newer frame first.
    Skipped,
}

pub struct Stills {
    jobs: Sender<Job>,
    done: Receiver<(Key, usize, Outcome)>,
    cache: HashMap<Key, State>,
    /// The picture each cell showed last, kept while its next one renders.
    shown: Vec<Option<egui::TextureHandle>>,
}

/// Splits `jobs` (oldest first) into the newest one of each cell and the
/// outdated rest, both oldest first.
fn newest_per_cell(jobs: Vec<Job>) -> (Vec<Job>, Vec<Job>) {
    let mut seen = HashSet::new();
    let (mut newest, mut outdated) = (Vec::new(), Vec::new());
    for job in jobs.into_iter().rev() {
        if seen.insert(job.cell) {
            newest.push(job);
        } else {
            outdated.push(job);
        }
    }
    newest.reverse();
    outdated.reverse();
    (newest, outdated)
}

impl Stills {
    pub fn new(ctx: egui::Context) -> Self {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        // The Trim Editor's frames: someone waits for them.
        let _ = crate::threads::spawn(
            "vidcut-stills",
            crate::threads::Work::Interactive,
            move || {
                let mut queue: Vec<Job> = Vec::new();
                loop {
                    if queue.is_empty() {
                        match rx.recv() {
                            Ok(job) => queue.push(job),
                            Err(_) => return,
                        }
                    }
                    queue.extend(rx.try_iter());
                    let (mut newest, outdated) = newest_per_cell(std::mem::take(&mut queue));
                    for job in outdated {
                        if tx.send((job.key, job.cell, Outcome::Skipped)).is_err() {
                            return;
                        }
                    }
                    let job = newest.remove(0);
                    queue = newest;
                    let s = &job.still;
                    let r = vidcut_av::export::still::rgba(&s.piece, s.fps, s.width, s.height)
                        .map_err(|e| e.to_string());
                    if tx.send((job.key, job.cell, Outcome::Rendered(r))).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            },
        );
        Self {
            jobs,
            done,
            cache: HashMap::new(),
            shown: Vec::new(),
        }
    }

    /// The picture for `cell`: the still's texture, rendering it in the
    /// background if needed, and until then the newest picture rendered for
    /// the cell (a running trim moves on before each frame is ready).
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        cell: usize,
        still: &Still,
    ) -> Option<egui::TextureId> {
        if self.shown.len() <= cell {
            self.shown.resize(cell + 1, None);
        }
        while let Ok((key, done_cell, outcome)) = self.done.try_recv() {
            let state = match outcome {
                Outcome::Rendered(Ok(img)) => {
                    let texture = ctx.load_texture(
                        format!("still-{}", self.cache.len()),
                        egui::ColorImage::from_rgba_unmultiplied(
                            [img.width as usize, img.height as usize],
                            &img.rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    );
                    // Frames render in the order each cell asked for them.
                    if done_cell >= self.shown.len() {
                        self.shown.resize(done_cell + 1, None);
                    }
                    self.shown[done_cell] = Some(texture.clone());
                    State::Ready(texture)
                }
                Outcome::Rendered(Err(_)) => State::Failed,
                // Asked for again, it renders then.
                Outcome::Skipped => {
                    self.cache.remove(&key);
                    continue;
                }
            };
            self.cache.insert(key, state);
        }
        let key = still.key();
        match self.cache.get(&key) {
            Some(State::Ready(t)) => {
                self.shown[cell] = Some(t.clone());
                return Some(t.id());
            }
            Some(State::Failed) => return None,
            Some(State::Pending) => {}
            None => {
                if self.cache.len() > 64 {
                    self.cache.retain(|_, s| matches!(s, State::Pending));
                }
                self.cache.insert(key.clone(), State::Pending);
                let _ = self.jobs.send(Job {
                    key,
                    still: still.clone(),
                    cell,
                });
            }
        }
        self.shown[cell].as_ref().map(|t| t.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(n: i64, cell: usize) -> Job {
        Job {
            key: Key(n.to_string()),
            still: Still {
                piece: VideoPiece::Black { frames: 1 },
                fps: Rational::new(30, 1).unwrap(),
                width: 2,
                height: 2,
            },
            cell,
        }
    }

    #[test]
    fn only_the_newest_frame_of_each_cell_renders() {
        // A running trim: both cells ask for a new frame on every redraw.
        let jobs = vec![job(1, 0), job(2, 1), job(3, 0), job(4, 1), job(5, 0)];
        let (newest, outdated) = newest_per_cell(jobs);
        let keys = |v: &[Job]| v.iter().map(|j| j.key.0.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&newest), ["4", "5"]);
        assert_eq!(keys(&outdated), ["1", "2", "3"]);
    }
}
