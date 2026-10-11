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

/// Video and audio files.
const MEDIA_TYPES: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "mxf", "m4v", "ts", "mts", "m2ts", "mpg", "wav", "flac",
    "mp3", "m4a", "aac", "ogg", "opus",
];

/// Image files: stills, or the frames of a sequence.
const IMAGE_TYPES: &[&str] = &[
    "png", "jpg", "jpeg", "tif", "tiff", "exr", "dpx", "tga", "bmp", "webp",
];

/// Image types that are frames of renders and scans: numbered ones are
/// offered as sequences first. (Numbered JPEGs are usually photos.)
const FRAME_TYPES: &[&str] = &["png", "tif", "tiff", "exr", "dpx", "tga", "bmp"];

/// The file dialog's filters. The first one, which the dialog opens with,
/// shows everything vidcut imports: pictures hidden behind a second filter
/// made a folder of frames look empty.
fn filters() -> [(&'static str, Vec<&'static str>); 3] {
    [
        ("all media", [MEDIA_TYPES, IMAGE_TYPES].concat()),
        ("video and audio", MEDIA_TYPES.to_vec()),
        ("images", IMAGE_TYPES.to_vec()),
    ]
}

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
    /// The keyboard guard is registered once, on the first frame.
    guard: bool,
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
///
/// Separate stills ignore the rate, so a cleared or invalid field still
/// imports. The rate is parsed only when the numbered runs come in as
/// sequences.
fn request(choice: &Choice) -> Result<(&'static str, Value), String> {
    let bin = &choice.bin;
    if !choice.sequences {
        return Ok(("media.add", json!({ "paths": choice.files, "bin": bin })));
    }
    let fps = choice
        .fps
        .trim()
        .parse::<Rational>()
        .ok()
        .filter(|fps| fps.as_f64() > 0.0)
        .ok_or_else(|| format!("{:?} isn't a frame rate", choice.fps))?;
    let sequences: Vec<&PathBuf> = choice.runs.iter().map(|r| &r.file).collect();
    let add = json!({ "paths": sequences, "bin": bin, "sequence": true, "fps": fps.to_string() });
    if choice.others.is_empty() {
        return Ok(("media.add", add));
    }
    Ok((
        "apply",
        json!({ "commands": [
            { "method": "media.add", "params": add },
            { "method": "media.add", "params": { "paths": choice.others, "bin": bin } },
        ], "label": "import media" }),
    ))
}

/// Id of the flag the shortcut guard reads at the start of each pass.
fn question_id() -> egui::Id {
    egui::Id::new("vidcut-import-question")
}

fn question_open(ctx: &egui::Context, open: bool) {
    ctx.data_mut(|data| data.insert_temp(question_id(), open));
}

/// Shortcuts are handled before this modal is shown, so while the question
/// is up they are dropped at the start of the pass. Escape is left, and the
/// modal cancels on it. A focused rate field keeps its keys.
fn guard_import_shortcuts(ui: &mut egui::Ui) {
    let ctx = ui.ctx();
    let open = ctx.data(|data| data.get_temp::<bool>(question_id()).unwrap_or(false));
    if !open || ctx.text_edit_focused() {
        return;
    }
    ctx.input_mut(|input| {
        input.events.retain(|event| match event {
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                modifiers,
                ..
            } if !modifiers.any() => true,
            egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Paste(_) => false,
            _ => true,
        });
    });
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
            let filters = filters();
            let filters: Vec<(&str, &[&str])> = filters
                .iter()
                .map(|(name, types)| (*name, types.as_slice()))
                .collect();
            let files = dialogs.pick_files("Import Media", &filters);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((files, bin));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    }

    /// The import's answer, and the window asking about numbered images.
    pub(super) fn import_window(&mut self, ctx: &egui::Context) {
        self.import.ctx = Some(ctx.clone());
        if !self.import.guard {
            self.import.guard = true;
            ctx.on_begin_pass("vidcut-import-question", Arc::new(guard_import_shortcuts));
        }
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
            question_open(ctx, false);
            return;
        };
        question_open(ctx, true);
        let mut done = None;
        let modal = egui::Modal::new(egui::Id::new("import-numbered-images")).show(ctx, |ui| {
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
        // Escape cancels like Cancel: import nothing. It is consumed so the
        // timeline does not also see it.
        if done.is_none()
            && modal.is_top_modal
            && !modal.any_popup_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            done = Some(false);
        }
        match done {
            Some(true) => match request(choice) {
                Ok((method, params)) => {
                    self.send(method, params);
                    self.import.choice = None;
                    question_open(ctx, false);
                }
                Err(message) => choice.error = Some(message),
            },
            Some(false) => {
                self.import.choice = None;
                question_open(ctx, false);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_opens_showing_every_importable_file() {
        let [(_, first), ..] = filters();
        for ext in MEDIA_TYPES.iter().chain(IMAGE_TYPES) {
            assert!(first.contains(ext), "{ext}");
        }
    }

    #[test]
    fn numbered_images_make_runs_and_the_rest_stay() {
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-import-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
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
        let mut choice = Choice {
            runs: vec![Run {
                name: String::new(),
                count: 3,
                file: dir.join("f_002.png"),
            }],
            others: vec![dir.join("cam.mp4")],
            files: Vec::new(),
            bin: None,
            sequences: true,
            fps: "24".into(),
            error: None,
        };
        let (method, params) = request(&choice).unwrap();
        assert_eq!(method, "apply");
        assert_eq!(params["commands"][0]["params"]["sequence"], true);
        assert_eq!(params["commands"][0]["params"]["fps"], "24");
        // Separate stills ignore a cleared or invalid rate.
        choice.sequences = false;
        choice.fps.clear();
        choice.files = vec![dir.join("logo_1.png")];
        let (method, params) = request(&choice).unwrap();
        assert_eq!(method, "media.add");
        assert!(params.get("fps").is_none());
        choice.fps = "nope".into();
        assert!(request(&choice).is_ok());
        choice.sequences = true;
        choice.fps.clear();
        assert!(request(&choice).is_err());
        choice.fps = "0".into();
        assert!(request(&choice).is_err());
    }
}
