//! Exact single frames for the GUI (the Trim Editor), rendered by the export
//! engine on a background thread so they match export and never block the UI.

use std::collections::HashMap;
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

pub struct Stills {
    jobs: Sender<(Key, Still)>,
    done: Receiver<(Key, Result<vidcut_av::thumbs::Image, String>)>,
    cache: HashMap<Key, State>,
}

impl Stills {
    pub fn new(ctx: egui::Context) -> Self {
        let (jobs, rx) = channel::<(Key, Still)>();
        let (tx, done) = channel();
        let _ = std::thread::Builder::new()
            .name("vidcut-stills".into())
            .spawn(move || {
                while let Ok((key, s)) = rx.recv() {
                    let r = vidcut_av::export::still::rgba(&s.piece, s.fps, s.width, s.height)
                        .map_err(|e| e.to_string());
                    if tx.send((key, r)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        Self {
            jobs,
            done,
            cache: HashMap::new(),
        }
    }

    /// The still's texture, rendering it in the background if needed.
    pub fn get(&mut self, ctx: &egui::Context, still: &Still) -> Option<egui::TextureId> {
        while let Ok((key, result)) = self.done.try_recv() {
            let state = match result {
                Ok(img) => State::Ready(ctx.load_texture(
                    format!("still-{}", self.cache.len()),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [img.width as usize, img.height as usize],
                        &img.rgba,
                    ),
                    egui::TextureOptions::LINEAR,
                )),
                Err(_) => State::Failed,
            };
            self.cache.insert(key, state);
        }
        let key = still.key();
        match self.cache.get(&key) {
            Some(State::Ready(t)) => Some(t.id()),
            Some(State::Pending | State::Failed) => None,
            None => {
                if self.cache.len() > 64 {
                    self.cache.retain(|_, s| matches!(s, State::Pending));
                }
                self.cache.insert(key.clone(), State::Pending);
                let _ = self.jobs.send((key, still.clone()));
                None
            }
        }
    }
}
