//! Exact single frames for the GUI (the Trim Editor and Dynamic Trim's
//! two-up), rendered by the export engine so they match export and never
//! block the UI.
//!
//! Each cell showing frames has a thread of its own, so the cells render at
//! once, and a renderer that keeps its file open: a running Dynamic Trim
//! asks for frame after frame, and each one decodes on from the one before
//! instead of from a keyframe. The trim moves on faster than frames render,
//! so a cell renders only the newest frame it asked for, skipping the rest,
//! and shows its last picture until the next one is ready.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

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

enum Outcome {
    Rendered(Result<vidcut_av::thumbs::Image, String>),
    /// Its cell asked for a newer frame first.
    Skipped,
}

/// How long a cell keeps its file and decoder open without being asked for
/// a frame: a decoder holds frame buffers and a thread per core.
const IDLE: Duration = Duration::from_secs(10);

/// A cell's renderer thread, and the picture it showed last.
struct Cell {
    jobs: Sender<(Key, Still)>,
    shown: Option<egui::TextureHandle>,
}

pub struct Stills {
    ctx: egui::Context,
    cells: Vec<Cell>,
    finished: Sender<(Key, usize, Outcome)>,
    done: Receiver<(Key, usize, Outcome)>,
    cache: HashMap<Key, State>,
}

impl Stills {
    pub fn new(ctx: egui::Context) -> Self {
        let (finished, done) = channel();
        Self {
            ctx,
            cells: Vec::new(),
            finished,
            done,
            cache: HashMap::new(),
        }
    }

    /// Cell `i`, starting its thread the first time.
    fn cell(&mut self, i: usize) -> &mut Cell {
        while self.cells.len() <= i {
            let n = self.cells.len();
            let (jobs, rx) = channel::<(Key, Still)>();
            let tx = self.finished.clone();
            let ctx = self.ctx.clone();
            // The Trim Editor's frames: someone waits for them.
            let _ = crate::threads::spawn(
                format!("vidcut-stills-{n}"),
                crate::threads::Work::Interactive,
                move || {
                    let mut renderer = vidcut_av::export::still::StillRenderer::default();
                    loop {
                        let mut job = match rx.recv_timeout(IDLE) {
                            Ok(job) => job,
                            Err(RecvTimeoutError::Timeout) => {
                                renderer = Default::default();
                                match rx.recv() {
                                    Ok(job) => job,
                                    Err(_) => return,
                                }
                            }
                            Err(RecvTimeoutError::Disconnected) => return,
                        };
                        let mut skipped = 0;
                        for newer in rx.try_iter() {
                            if tx.send((job.0, n, Outcome::Skipped)).is_err() {
                                return;
                            }
                            job = newer;
                            skipped += 1;
                        }
                        let (key, s) = job;
                        let started = std::time::Instant::now();
                        let r = renderer
                            .rgba(&s.piece, s.fps, s.width, s.height)
                            .map_err(|e| e.to_string());
                        if crate::live::debug() {
                            let at = match &s.piece {
                                VideoPiece::Source { source_frame, .. } => *source_frame,
                                VideoPiece::Black { .. } => -1,
                            };
                            eprintln!(
                                "[stills] cell {n}: source frame {at} in {:.1} ms ({skipped} skipped){}",
                                started.elapsed().as_secs_f64() * 1000.0,
                                r.as_ref().err().map_or(String::new(), |e| format!(": {e}"))
                            );
                        }
                        if tx.send((key, n, Outcome::Rendered(r))).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                },
            );
            self.cells.push(Cell { jobs, shown: None });
        }
        &mut self.cells[i]
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
                    // A cell renders frames in the order it asked for them.
                    self.cell(done_cell).shown = Some(texture.clone());
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
                let t = t.clone();
                let id = t.id();
                self.cell(cell).shown = Some(t);
                return Some(id);
            }
            Some(State::Failed) => return None,
            Some(State::Pending) => {}
            None => {
                if self.cache.len() > 64 {
                    self.cache.retain(|_, s| matches!(s, State::Pending));
                }
                self.cache.insert(key.clone(), State::Pending);
                let _ = self.cell(cell).jobs.send((key, still.clone()));
            }
        }
        self.cell(cell).shown.as_ref().map(|t| t.id())
    }
}
