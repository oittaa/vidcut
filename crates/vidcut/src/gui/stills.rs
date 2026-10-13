//! Exact single frames for the GUI (the Trim Editor's and Dynamic Trim's
//! two-up, on a copy of the project), rendered by the export engine so they
//! match export and never block the UI. The viewer itself shows the
//! player's pictures (docs/decisions/preview.md §4).
//!
//! Each cell showing frames has a thread of its own, so the cells render at
//! once, and a renderer that decodes each frame once whichever way the
//! position moves (docs/decisions/preview.md §9). Going back, the renderer
//! plans the window before the one it is in, and a second thread of the
//! cell's renders it meanwhile. The position moves faster than frames
//! render, so a cell renders only the newest frame it asked for, skipping
//! the rest, and shows its last picture until the next one is ready.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use vidcut_av::export::renderer::{Prefetch, StillRenderer};

use eframe::egui;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

/// A one-frame render: a piece (`frames == 1`) at a size.
#[derive(Debug, Clone, PartialEq)]
pub struct Still {
    pub piece: VideoPiece,
    /// Where the shown stretch of the file starts: frames behind it aren't
    /// worth rendering ahead (0: the whole file).
    pub floor: i64,
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
    Rendered(Result<Arc<vidcut_av::thumbs::Image>, String>),
    /// Its cell asked for a newer frame first.
    Skipped,
}

/// How long a cell keeps its file and decoder open without being asked for
/// a frame: a decoder holds frame buffers and a thread per core.
const IDLE: Duration = Duration::from_secs(10);

/// Memory for a cell's kept pictures.
const BUDGET: usize = 256 << 20;

/// Textures kept for frames shown before: the renderer keeps the pictures,
/// so one shown again is only uploaded again.
const TEXTURES: usize = 24;

/// What a cell shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shown {
    /// The frame asked for.
    Exact(egui::TextureId),
    /// The cell's last picture, while the frame asked for renders.
    Previous(egui::TextureId),
}

impl Shown {
    pub fn texture(self) -> egui::TextureId {
        match self {
            Shown::Exact(t) | Shown::Previous(t) => t,
        }
    }
}

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
            // The window before the one going back renders here.
            let (prefetches, ahead) = channel::<Prefetch>();
            let _ = crate::threads::spawn(
                format!("vidcut-stills-{n}-ahead"),
                crate::threads::Work::Interactive,
                move || {
                    while let Ok(p) = ahead.recv() {
                        let started = std::time::Instant::now();
                        let r = p.run();
                        if crate::live::debug() {
                            eprintln!(
                                "[stills] cell {n}: window behind in {:.1} ms{}",
                                started.elapsed().as_secs_f64() * 1000.0,
                                r.err().map_or(String::new(), |e| format!(": {e}"))
                            );
                        }
                    }
                },
            );
            // Frames someone is looking at: they wait for them.
            let _ = crate::threads::spawn(
                format!("vidcut-stills-{n}"),
                crate::threads::Work::Interactive,
                move || {
                    let mut on_gpu = crate::preferences::decoding_gpu();
                    let mut renderer = cell_renderer(on_gpu.is_some());
                    loop {
                        let mut job = match rx.recv_timeout(IDLE) {
                            Ok(job) => job,
                            Err(RecvTimeoutError::Timeout) => {
                                renderer = cell_renderer(on_gpu.is_some());
                                match rx.recv() {
                                    Ok(job) => job,
                                    Err(_) => return,
                                }
                            }
                            Err(RecvTimeoutError::Disconnected) => return,
                        };
                        // The preferences as each frame is asked for: a
                        // change reaches the two-up at once.
                        let gpu = crate::preferences::decoding_gpu();
                        if gpu != on_gpu {
                            renderer = cell_renderer(gpu.is_some());
                            on_gpu = gpu;
                        }
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
                            .rgba(&s.piece, s.floor, s.fps, s.width, s.height)
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
                        if let Some(p) = renderer.take_prefetch() {
                            let _ = prefetches.send(p);
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
    /// the cell (the position moves on before each frame is ready).
    pub fn get(&mut self, ctx: &egui::Context, cell: usize, still: &Still) -> Option<Shown> {
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
                return Some(Shown::Exact(id));
            }
            Some(State::Failed) => return None,
            Some(State::Pending) => {}
            None => {
                if self.cache.len() > TEXTURES {
                    self.cache.retain(|_, s| matches!(s, State::Pending));
                }
                self.cache.insert(key.clone(), State::Pending);
                let _ = self.cell(cell).jobs.send((key, still.clone()));
            }
        }
        self.cell(cell)
            .shown
            .as_ref()
            .map(|t| Shown::Previous(t.id()))
    }
}

/// A renderer for a cell, decoding on the GPU the viewer decodes on while
/// `on_gpu` (`gpu.decoding`).
fn cell_renderer(on_gpu: bool) -> StillRenderer {
    let renderer = StillRenderer::with_budget(BUDGET);
    match crate::preferences::timeline_decoder(on_gpu) {
        Some(gpu) => renderer.decoding_on(gpu),
        None => renderer,
    }
}
