use std::path::PathBuf;

use eframe::egui;
use serde_json::{Value, json};

use super::export::{Ask, Picked};
use super::folders::Kind;
use super::import;
use super::workspace::{Workspace, dim};

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
        super::dialog::window(ctx, "Relink Media", &mut open, |ui| {
            if let Some(e) = &report.error {
                super::dialog::warn(ui, e.clone());
            }
            super::dialog::form(ui, "relink-form", |ui| {
                super::dialog::row(ui, "Looked In", |ui| {
                    ui.add(egui::Label::new(report.folder.display().to_string()).wrap())
                });
                super::dialog::row(ui, "Relinked", |ui| ui.label(report.relinked.to_string()));
                if !report.not_found.is_empty() {
                    super::dialog::row(ui, "Not Found", |ui| {
                        let mut names: Vec<String> =
                            report.not_found.iter().take(8).cloned().collect();
                        if report.not_found.len() > 8 {
                            names.push(format!("and {} more", report.not_found.len() - 8));
                        }
                        ui.add(egui::Label::new(dim(names.join(", "))).wrap())
                    });
                }
            });
            if !report.different.is_empty() {
                super::dialog::note(ui, "Found by name, but not the same file:");
                for (i, (_, name, path, why)) in report.different.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        ui.label(dim(why));
                        if ui.button("Relink Anyway").clicked() {
                            anyway = Some(i);
                        }
                    });
                    ui.add(egui::Label::new(dim(path.display().to_string())).wrap());
                }
            }
            super::dialog::footer(ui, |ui| {
                close = super::dialog::main_button(ui, "Close", true);
                if !self.snap.missing.is_empty() && ui.button("Choose Another Folder…").clicked()
                {
                    again = true;
                }
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
