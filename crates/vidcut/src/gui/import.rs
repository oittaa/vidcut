//! Importing media (Ctrl+I, p. 654). Numbered images among the files are
//! asked about: one clip per numbered run (an image sequence, decision D39)
//! or separate stills (deviation X17). Either way the import is one
//! request.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::time::Rational;

use super::workspace::{Workspace, dim};

/// What the file dialog offers.
pub(super) const MEDIA_TYPES: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "mxf", "m4v", "ts", "mts", "m2ts", "mpg", "wav", "flac",
    "mp3", "m4a", "aac", "ogg", "opus",
];

/// Image files: stills, or the frames of a sequence.
pub(super) const IMAGE_TYPES: &[&str] = &[
    "png", "jpg", "jpeg", "tif", "tiff", "exr", "dpx", "tga", "bmp", "webp",
];

/// Image types that are frames of renders and scans: numbered ones are
/// offered as sequences first. (Numbered JPEGs are usually photos.)
const FRAME_TYPES: &[&str] = &["png", "tif", "tiff", "exr", "dpx", "tga", "bmp"];

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

#[derive(Default)]
pub(super) struct ImportUi {
    /// The UI's context, so the file dialog's thread can wake it.
    pub(super) ctx: Option<egui::Context>,
    /// The dialog's answer, filled in by its thread.
    picked: Arc<Mutex<Option<Picked>>>,
    choice: Option<Choice>,
}

/// The files picked, and the bin to put them in.
type Picked = (Vec<PathBuf>, Option<String>);

/// Numbered images among the files picked: how to bring them in.
struct Choice {
    runs: Vec<Run>,
    /// The other files picked.
    others: Vec<PathBuf>,
    /// All of them, for separate stills.
    files: Vec<PathBuf>,
    bin: Option<String>,
    sequences: bool,
    fps: String,
    error: Option<String>,
}

/// A run of two or more numbered images on disk with one of them picked.
#[derive(Debug, Clone, PartialEq)]
struct Run {
    /// The pool's name for it: `frame_[0001-0120].png`.
    name: String,
    count: u64,
    /// A file of it, picked.
    file: PathBuf,
}

/// The numbered runs among `files` (each counted on disk, picked or not),
/// and the files that aren't in one. Whether any run is of a frame type.
fn runs(files: &[PathBuf]) -> (Vec<Run>, Vec<PathBuf>, bool) {
    let (mut runs, mut others, mut frames) = (Vec::new(), Vec::new(), false);
    let mut taken: HashSet<&Path> = HashSet::new();
    for f in files {
        if taken.contains(f.as_path()) {
            continue;
        }
        let ext = extension(f);
        let run = IMAGE_TYPES
            .contains(&ext.as_str())
            .then(|| vidcut_core::sequence::find(f).ok())
            .flatten()
            .filter(|s| s.count >= 2);
        let Some(s) = run else {
            others.push(f.clone());
            continue;
        };
        let names: HashSet<String> = (0..s.count).map(|k| s.name(s.first + k)).collect();
        for g in files {
            if g.parent() == f.parent()
                && g.file_name()
                    .is_some_and(|n| names.contains(n.to_string_lossy().as_ref()))
            {
                taken.insert(g);
            }
        }
        frames |= FRAME_TYPES.contains(&ext.as_str());
        runs.push(Run {
            name: s.display(),
            count: s.count,
            file: f.clone(),
        });
    }
    (runs, others, frames)
}

/// The one request that imports `choice` as it stands.
fn request(choice: &Choice, fps: Rational) -> (&'static str, Value) {
    let bin = &choice.bin;
    if !choice.sequences {
        return ("media.add", json!({ "paths": choice.files, "bin": bin }));
    }
    let sequences: Vec<&PathBuf> = choice.runs.iter().map(|r| &r.file).collect();
    let add = json!({ "paths": sequences, "bin": bin, "sequence": true, "fps": fps.to_string() });
    if choice.others.is_empty() {
        return ("media.add", add);
    }
    (
        "apply",
        json!({ "commands": [
            { "method": "media.add", "params": add },
            { "method": "media.add", "params": { "paths": choice.others, "bin": bin } },
        ], "label": "import media" }),
    )
}

impl Workspace {
    /// Ctrl+I: pick media files to import (p. 654), into the bin the pool
    /// shows (p. 655).
    pub(super) fn import_media(&self) {
        if self.project().is_none() {
            return;
        }
        let bin = Some(self.snap.pool.bin.clone()).filter(|b| !b.is_empty());
        let (dialogs, slot, ctx) = (
            self.dialogs.clone(),
            self.import.picked.clone(),
            self.import.ctx.clone(),
        );
        std::thread::spawn(move || {
            let files = dialogs.pick_files(
                "Import Media",
                &[("media", MEDIA_TYPES), ("images", IMAGE_TYPES)],
            );
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((files, bin));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    }

    /// The import's answer, and the window asking about numbered images.
    pub(super) fn import_window(&mut self, ctx: &egui::Context) {
        self.import.ctx = Some(ctx.clone());
        let picked = self
            .import
            .picked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some((files, bin)) = picked.filter(|(files, _)| !files.is_empty()) {
            let (runs, others, frames) = runs(&files);
            if runs.is_empty() {
                self.send("media.add", json!({ "paths": files, "bin": bin }));
            } else {
                self.import.choice = Some(Choice {
                    runs,
                    others,
                    files,
                    bin,
                    sequences: frames,
                    fps: self.fps().to_string(),
                    error: None,
                });
            }
        }
        let Some(choice) = self.import.choice.as_mut() else {
            return;
        };
        let mut done = None;
        egui::Modal::new(egui::Id::new("import-numbered-images")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.label(egui::RichText::new("Numbered images").strong());
            for run in choice.runs.iter().take(6) {
                ui.label(format!("{}: {} files", run.name, run.count));
            }
            if choice.runs.len() > 6 {
                ui.label(dim(format!("and {} more", choice.runs.len() - 6)));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.radio_value(&mut choice.sequences, true, "One clip each, at");
                ui.add_enabled(
                    choice.sequences,
                    egui::TextEdit::singleline(&mut choice.fps).desired_width(64.0),
                );
                ui.label("fps");
            });
            ui.radio_value(&mut choice.sequences, false, "Separate stills");
            if let Some(e) = &choice.error {
                ui.colored_label(super::theme::ERROR, e);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Import").clicked() {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => match choice.fps.trim().parse::<Rational>() {
                Ok(fps) if fps.as_f64() > 0.0 => {
                    let (method, params) = request(choice, fps);
                    self.send(method, params);
                    self.import.choice = None;
                }
                _ => choice.error = Some(format!("{:?} isn't a frame rate", choice.fps)),
            },
            Some(false) => self.import.choice = None,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_images_make_runs_and_the_rest_stay() {
        let dir = std::env::temp_dir().join(format!("vidcut-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in [
            "f_001.png",
            "f_002.png",
            "f_003.png",
            "IMG_0005.JPG",
            "IMG_0006.JPG",
            "logo_1.png",
            "cam.mp4",
        ] {
            std::fs::write(dir.join(n), b"").unwrap();
        }
        let files: Vec<PathBuf> = ["f_002.png", "f_003.png", "logo_1.png", "cam.mp4"]
            .iter()
            .map(|n| dir.join(n))
            .collect();
        let (runs, others, frames) = super::runs(&files);
        assert_eq!(
            runs,
            vec![Run {
                name: "f_[001-003].png".into(),
                count: 3,
                file: dir.join("f_002.png"),
            }]
        );
        assert_eq!(others, vec![dir.join("logo_1.png"), dir.join("cam.mp4")]);
        assert!(frames);
        // Numbered JPEGs: a run, offered as stills first.
        let (runs, _, frames) = super::runs(&[dir.join("IMG_0005.JPG")]);
        assert_eq!(runs.len(), 1);
        assert!(!frames);
        let choice = Choice {
            runs: vec![Run {
                name: String::new(),
                count: 3,
                file: dir.join("f_002.png"),
            }],
            others: vec![dir.join("cam.mp4")],
            files: Vec::new(),
            bin: None,
            sequences: true,
            fps: String::new(),
            error: None,
        };
        let (method, params) = request(&choice, Rational::new(24, 1).unwrap());
        assert_eq!(method, "apply");
        assert_eq!(params["commands"][0]["params"]["sequence"], true);
        assert_eq!(params["commands"][0]["params"]["fps"], "24");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
