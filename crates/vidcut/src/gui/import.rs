use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::folder::{FRAME_TYPES, IMAGE_TYPES, MEDIA_TYPES, SUBTITLE_TYPES, extension};
use vidcut_core::model::ImageSequence;
use vidcut_core::time::Rational;

use super::workspace::{Workspace, dim, escape_cancels};

fn filters() -> [(&'static str, Vec<&'static str>); 4] {
    [
        ("all media", media_types()),
        ("video and audio", MEDIA_TYPES.to_vec()),
        ("images", IMAGE_TYPES.to_vec()),
        ("subtitles", SUBTITLE_TYPES.to_vec()),
    ]
}

pub(super) fn media_types() -> Vec<&'static str> {
    [MEDIA_TYPES, IMAGE_TYPES, SUBTITLE_TYPES].concat()
}

#[derive(Default)]
pub(super) struct ImportUi {
    pub(super) ctx: Option<egui::Context>,
    picks: Picks,
    dropped: Vec<PathBuf>,
    choice: Option<Choice>,
}

impl ImportUi {
    pub(super) fn asking(&self) -> bool {
        self.choice.is_some()
    }
}

type Picks = Arc<Mutex<VecDeque<Pick>>>;

enum Pick {
    Files(Vec<PathBuf>, Option<String>),
    Folder(PickedFolder),
}

struct PickedFolder {
    path: PathBuf,
    bin: Option<String>,
    runs: Vec<(PathBuf, ImageSequence)>,
}

impl PickedFolder {
    fn read(path: PathBuf, bin: Option<String>) -> Self {
        let runs = vidcut_core::folder::read(&path)
            .map(|f| f.runs())
            .unwrap_or_default();
        Self { path, bin, runs }
    }
}

fn push(picks: &Picks, ctx: Option<&egui::Context>, pick: Pick) {
    picks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(pick);
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

struct Choice {
    runs: Vec<Run>,
    others: Vec<PathBuf>,
    files: Vec<PathBuf>,
    folder: Option<PathBuf>,
    bin: Option<String>,
    sequences: bool,
    fps: String,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Run {
    name: String,
    count: u64,
    file: PathBuf,
}

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

fn request(choice: &Choice) -> Result<(&'static str, Value), String> {
    let bin = &choice.bin;
    let fps = match choice.sequences {
        false => None,
        true => Some(
            choice
                .fps
                .trim()
                .parse::<Rational>()
                .ok()
                .filter(|fps| fps.as_f64() > 0.0)
                .ok_or_else(|| format!("{:?} isn't a frame rate", choice.fps))?,
        ),
    };
    if let Some(folder) = &choice.folder {
        let mut params = json!({ "path": folder, "bin": bin, "sequence": choice.sequences });
        if let Some(fps) = fps {
            params["fps"] = json!(fps.to_string());
        }
        return Ok(("media.add_folder", params));
    }
    let Some(fps) = fps else {
        return Ok(("media.add", json!({ "paths": choice.files, "bin": bin })));
    };
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

impl Workspace {
    pub(super) fn import_from(&self) -> (Option<String>, PathBuf) {
        let bin = Some(self.snap.pool.bin.clone()).filter(|b| !b.is_empty());
        let folder = self
            .folders
            .get(super::folders::Kind::Media)
            .or_else(|| self.project_folder())
            .unwrap_or_default();
        (bin, folder)
    }

    pub(super) fn import_folder(&self) {
        if self.project().is_none() {
            return;
        }
        let (bin, folder) = self.import_from();
        let (dialogs, picks, ctx) = (
            self.dialogs.clone(),
            self.import.picks.clone(),
            self.import.ctx.clone(),
        );
        let _ = crate::threads::spawn(
            "vidcut-import-dialog",
            crate::threads::Work::Interactive,
            move || {
                let Some(path) = dialogs.pick_folder("Import Media Folder", &folder) else {
                    return;
                };
                let folder = PickedFolder::read(path, bin);
                push(&picks, ctx.as_ref(), Pick::Folder(folder));
            },
        );
    }

    pub(super) fn import_media(&self) {
        if self.project().is_none() {
            return;
        }
        let (bin, folder) = self.import_from();
        let (dialogs, picks, ctx) = (
            self.dialogs.clone(),
            self.import.picks.clone(),
            self.import.ctx.clone(),
        );
        let _ = crate::threads::spawn(
            "vidcut-import-dialog",
            crate::threads::Work::Interactive,
            move || {
                let filters = filters();
                let filters: Vec<(&str, &[&str])> = filters
                    .iter()
                    .map(|(name, types)| (*name, types.as_slice()))
                    .collect();
                let files = dialogs.pick_files("Import Media", &folder, &filters);
                if !files.is_empty() {
                    push(&picks, ctx.as_ref(), Pick::Files(files, bin));
                }
            },
        );
    }

    fn import_dropped(&self, paths: Vec<PathBuf>) {
        if self.project().is_none() {
            return;
        }
        let (bin, _) = self.import_from();
        let (picks, ctx) = (self.import.picks.clone(), self.import.ctx.clone());
        let _ = crate::threads::spawn(
            "vidcut-import-drop",
            crate::threads::Work::Interactive,
            move || {
                let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) =
                    paths.into_iter().partition(|p| p.is_dir());
                if !files.is_empty() {
                    push(&picks, ctx.as_ref(), Pick::Files(files, bin.clone()));
                }
                for path in folders {
                    let folder = PickedFolder::read(path, bin.clone());
                    push(&picks, ctx.as_ref(), Pick::Folder(folder));
                }
            },
        );
    }

    fn take_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.import.dropped.extend(dropped);
            ctx.request_repaint();
        } else if !self.import.dropped.is_empty() {
            let paths = std::mem::take(&mut self.import.dropped);
            self.import_dropped(paths);
        }
    }

    pub(super) fn drop_hint(&self, ctx: &egui::Context, pool: egui::Rect) {
        if self.project().is_none() || ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("import-drop"),
        ));
        let colors = super::theme::colors();
        let card = super::theme::card(colors.panel);
        let rect = pool - card.outer_margin;
        painter.rect_filled(rect, card.corner_radius, colors.panel.gamma_multiply(0.9));
        painter.rect_stroke(
            rect,
            card.corner_radius,
            egui::Stroke::new(2.0, colors.accent),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop to Import",
            egui::TextStyle::Heading.resolve(&ctx.global_style()),
            colors.text,
        );
    }

    fn picked_folder(&mut self, f: PickedFolder) {
        let parent = f.path.parent().unwrap_or(&f.path);
        self.folders.remember(super::folders::Kind::Media, parent);
        if f.runs.is_empty() {
            self.send("media.add_folder", json!({ "path": f.path, "bin": f.bin }));
            return;
        }
        let frames = f
            .runs
            .iter()
            .any(|(file, _)| FRAME_TYPES.contains(&extension(file).as_str()));
        let runs = f
            .runs
            .into_iter()
            .map(|(file, s)| Run {
                name: s.display(),
                count: s.count,
                file,
            })
            .collect();
        self.import.choice = Some(Choice {
            runs,
            others: Vec::new(),
            files: Vec::new(),
            folder: Some(f.path),
            bin: f.bin,
            sequences: frames,
            fps: self.fps().decimal(),
            error: None,
        });
    }

    fn picked_files(&mut self, files: Vec<PathBuf>, bin: Option<String>) {
        self.folders
            .remember(super::folders::Kind::Media, &files[0]);
        let (runs, others, frames) = runs(&files);
        if runs.is_empty() {
            self.send("media.add", json!({ "paths": files, "bin": bin }));
            return;
        }
        self.import.choice = Some(Choice {
            runs,
            others,
            files,
            folder: None,
            bin,
            sequences: frames,
            fps: self.fps().decimal(),
            error: None,
        });
    }

    pub(super) fn import_window(&mut self, ctx: &egui::Context) {
        self.import.ctx = Some(ctx.clone());
        self.take_dropped(ctx);
        while self.import.choice.is_none() {
            let pick = self
                .import
                .picks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front();
            match pick {
                Some(Pick::Folder(f)) => self.picked_folder(f),
                Some(Pick::Files(files, bin)) => self.picked_files(files, bin),
                None => break,
            }
        }
        let Some(choice) = self.import.choice.as_mut() else {
            return;
        };
        let mut done = None;
        let modal = egui::Modal::new(egui::Id::unique("import-numbered-images")).show(ctx, |ui| {
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
                ui.colored_label(super::theme::colors().error, e);
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
        if done.is_none() && escape_cancels(ctx, &modal) {
            done = Some(false);
        }
        match done {
            Some(true) => match request(choice) {
                Ok((method, params)) => {
                    self.send(method, params);
                    self.import.choice = None;
                }
                Err(message) => choice.error = Some(message),
            },
            Some(false) => self.import.choice = None,
            None => {}
        }
        if self.import.choice.is_none() {
            ctx.request_repaint();
        }
    }
}
