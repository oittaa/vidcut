use std::path::PathBuf;

use eframe::egui;
use serde_json::{Value, json};

use super::export::{Ask, Picked};
use super::folders::Kind;
use super::workspace::{Workspace, dim};
use super::{import, theme};

#[derive(Default)]
pub(super) struct RelinkUi {
    state: Option<State>,
}

enum State {
    Folder(Picked),
    Searching(Ask, PathBuf),
    Report(Report),
    File(Picked, String),
}

struct Report {
    folder: PathBuf,
    relinked: usize,
    different: Vec<(String, String, PathBuf, String)>,
    not_found: Vec<String>,
    error: Option<String>,
}

impl Workspace {
    pub(super) fn relink_missing(&mut self, ctx: &egui::Context) {
        let (dialogs, folder) = (self.dialogs.clone(), self.media_folder());
        self.relink.state = Some(State::Folder(Picked::ask(ctx, move || {
            dialogs.pick_folder("Where Is the Media?", &folder)
        })));
    }

    pub(super) fn relink_one(&mut self, ctx: &egui::Context, media: &str) {
        let (dialogs, folder) = (self.dialogs.clone(), self.media_folder());
        let types = import::media_types();
        let picked = Picked::ask(ctx, move || dialogs.pick_file("Relink", &folder, &types));
        self.relink.state = Some(State::File(picked, media.to_string()));
    }

    fn media_folder(&self) -> PathBuf {
        self.folders
            .get(Kind::Media)
            .or_else(|| self.project_folder())
            .unwrap_or_default()
    }

    pub(super) fn relink_window(&mut self, ctx: &egui::Context) {
        self.relink.state = match self.relink.state.take() {
            Some(State::Folder(picked)) => match picked.take() {
                None => Some(State::Folder(picked)),
                Some(None) => None,
                Some(Some(folder)) => {
                    self.folders.remember(Kind::Media, &folder);
                    let params = json!({ "search": [folder] });
                    let ask = Ask::send(&self.live, ctx, "media.relink", params);
                    Some(State::Searching(ask, folder))
                }
            },
            Some(State::File(picked, media)) => match picked.take() {
                None => Some(State::File(picked, media)),
                Some(None) => None,
                Some(Some(path)) => {
                    self.folders.remember(Kind::Media, &path);
                    let params = json!({ "media": media, "path": path });
                    let ask = Ask::send(&self.live, ctx, "media.relink", params);
                    Some(State::Searching(ask, PathBuf::new()))
                }
            },
            Some(State::Searching(ask, folder)) => match ask.take() {
                None => Some(State::Searching(ask, folder)),
                Some(Ok(r)) if folder.as_os_str().is_empty() => {
                    self.say(self.relinked_one(&r.result));
                    None
                }
                Some(Err(e)) if folder.as_os_str().is_empty() => {
                    self.say(format!("Relink: {}", e.message));
                    None
                }
                Some(r) => Some(State::Report(self.report(folder, r))),
            },
            Some(State::Report(report)) => self.report_window(ctx, report),
            None => None,
        };
    }

    fn relinked_one(&self, result: &Value) -> String {
        let r = &result["relinked"][0];
        let name = self.media_name(r["media"].as_str().unwrap_or(""));
        match r["why"].as_str() {
            Some(why) => format!("Relinked {name} to a different file: {why}"),
            None => format!("Relinked {name}"),
        }
    }

    fn media_name(&self, id: &str) -> String {
        self.project()
            .and_then(|p| p.media(id))
            .map_or_else(|| id.to_string(), |m| m.name.clone())
    }

    fn report(
        &self,
        folder: PathBuf,
        result: vidcut_core::Result<vidcut_core::Response>,
    ) -> Report {
        let mut report = Report {
            folder,
            relinked: 0,
            different: Vec::new(),
            not_found: Vec::new(),
            error: None,
        };
        match result {
            Ok(r) => {
                let list = |key: &str| r.result[key].as_array().cloned().unwrap_or_default();
                report.relinked = list("relinked").len();
                for d in list("different") {
                    let id = d["media"].as_str().unwrap_or("").to_string();
                    report.different.push((
                        id.clone(),
                        self.media_name(&id),
                        PathBuf::from(d["path"].as_str().unwrap_or("")),
                        d["why"].as_str().unwrap_or("").to_string(),
                    ));
                }
                report.not_found = list("not_found")
                    .iter()
                    .map(|n| n["name"].as_str().unwrap_or("").to_string())
                    .collect();
            }
            Err(e) => report.error = Some(e.message),
        }
        report
    }

    fn report_window(&mut self, ctx: &egui::Context, mut report: Report) -> Option<State> {
        let (mut open, mut again, mut close) = (true, false, false);
        let mut anyway = None;
        egui::Window::new("Relink Media")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_max_width(560.0);
                ui.label(dim(format!(
                    "Looked in {} and its subfolders.",
                    report.folder.display()
                )));
                if let Some(e) = &report.error {
                    ui.colored_label(theme::colors().error, e);
                }
                ui.label(format!("{} relinked", report.relinked));
                if !report.not_found.is_empty() {
                    ui.label(format!("{} not found:", report.not_found.len()));
                    let shown = report.not_found.iter().take(8).cloned();
                    ui.label(dim(shown.collect::<Vec<_>>().join(", ")));
                    if report.not_found.len() > 8 {
                        ui.label(dim(format!("and {} more", report.not_found.len() - 8)));
                    }
                }
                if !report.different.is_empty() {
                    ui.label(format!(
                        "{} found by name, but not the same file:",
                        report.different.len()
                    ));
                    for (i, (_, name, path, why)) in report.different.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(name).on_hover_text(path.display().to_string());
                            ui.label(dim(why));
                            if ui
                                .button("Relink Anyway")
                                .on_hover_text(format!("Use {}", path.display()))
                                .clicked()
                            {
                                anyway = Some(i);
                            }
                        });
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if !self.snap.missing.is_empty()
                        && ui.button("Choose Another Folder…").clicked()
                    {
                        again = true;
                    }
                    close = ui.button("Close").clicked();
                });
            });
        if let Some(i) = anyway {
            let (id, _, path, _) = report.different.remove(i);
            self.send("media.relink", json!({ "media": id, "path": path }));
        }
        if again {
            self.relink_missing(ctx);
            return self.relink.state.take();
        }
        (open && !close).then_some(State::Report(report))
    }
}
