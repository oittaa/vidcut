use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui_phosphor::regular::CHECK;
use serde_json::json;
use vidcut_core::Error;
use vidcut_core::luts::{Lut, LutList};
use vidcut_core::model::ViewerLut;
use vidcut_core::request::Page;

use super::export::Ask;
use super::workspace::Workspace;

const NONE: &str = "No LUT selected";

const FOLLOW: &str = "Use video monitoring selection";

#[derive(Default)]
pub(super) struct Luts {
    pub(super) list: LutList,
    ask: Option<Ask>,
    asked: bool,
    folders: Arc<Vec<PathBuf>>,
}

impl Workspace {
    pub(super) fn luts_frame(&mut self, ctx: &egui::Context) {
        if !self.luts.asked || self.luts.folders != self.snap.lut_folders {
            self.luts.folders = self.snap.lut_folders.clone();
            self.update_luts(ctx);
        }
        let Some(answer) = self.luts.ask.as_ref().and_then(Ask::take) else {
            return;
        };
        self.luts.ask = None;
        let list = answer.and_then(|r| {
            serde_json::from_value(r.result).map_err(|e| Error::internal(e.to_string()))
        });
        match list {
            Ok(list) => self.luts.list = list,
            Err(e) => self.say(format!("LUTs: {}", e.message)),
        }
    }

    pub(super) fn update_luts(&mut self, ctx: &egui::Context) {
        self.luts.asked = true;
        self.luts.ask = Some(Ask::send(&self.live, ctx, "luts.list", json!({})));
    }

    pub(super) fn display_lut(&self) -> Option<&Path> {
        if self.effects_page.page != Page::Color {
            return None;
        }
        self.project()?.settings.viewer_lut()
    }

    pub(super) fn open_lut_folder(&mut self) {
        let folder = vidcut_core::luts::dir();
        match std::fs::create_dir_all(&folder) {
            Ok(()) => super::file_manager::show(&folder),
            Err(e) => self.say(format!("{}: {e}", folder.display())),
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn shown(list: &LutList, now: Option<&Path>) -> String {
    match now {
        None => NONE.to_string(),
        Some(path) => list
            .find(path)
            .map_or_else(|| file_name(path), |l| l.name.clone()),
    }
}

fn checked(on: bool, text: &str) -> String {
    format!("{}  {text}", if on { CHECK } else { "   " })
}

pub(super) fn menu_title(now: Option<&Path>) -> String {
    match now {
        Some(_) => format!("{CHECK}  LUT"),
        None => "LUT".to_string(),
    }
}

pub(super) fn menu(
    ui: &mut egui::Ui,
    list: &LutList,
    now: Option<&Path>,
) -> Option<Option<PathBuf>> {
    let mut pick = None;
    if ui.button(checked(now.is_none(), NONE)).clicked() {
        pick = Some(None);
        ui.close();
    }
    if let Some(path) = now.filter(|p| list.find(p).is_none())
        && ui.button(checked(true, &file_name(path))).clicked()
    {
        pick = Some(Some(path.to_path_buf()));
        ui.close();
    }
    level(ui, &list.luts, 0, now, &mut pick);
    pick
}

fn level(
    ui: &mut egui::Ui,
    luts: &[Lut],
    depth: usize,
    now: Option<&Path>,
    pick: &mut Option<Option<PathBuf>>,
) {
    let mut i = 0;
    while i < luts.len() {
        let lut = &luts[i];
        let Some(folder) = lut.menu.get(depth) else {
            if ui
                .button(checked(now == Some(lut.path.as_path()), &lut.name))
                .clicked()
            {
                *pick = Some(Some(lut.path.clone()));
                ui.close();
            }
            i += 1;
            continue;
        };
        let end = i + luts[i..]
            .iter()
            .take_while(|l| l.menu.get(depth) == Some(folder))
            .count();
        let inner = &luts[i..end];
        let on = now.is_some_and(|p| inner.iter().any(|l| l.path == p));
        ui.menu_button(checked(on, folder), |ui| {
            level(ui, inner, depth + 1, now, pick);
        });
        i = end;
    }
}

pub(super) fn combo(
    ui: &mut egui::Ui,
    id: &str,
    list: &LutList,
    now: Option<&Path>,
    width: f32,
) -> (egui::Response, Option<Option<PathBuf>>) {
    let mut pick = None;
    let r = egui::ComboBox::from_id_salt(id)
        .selected_text(shown(list, now))
        .width(width)
        .truncate()
        .show_ui(ui, |ui| entries(ui, list, (now, now.is_none()), &mut pick));
    (r.response, pick)
}

pub(super) fn viewer_combo(
    ui: &mut egui::Ui,
    id: &str,
    list: &LutList,
    now: &ViewerLut,
    width: f32,
) -> (egui::Response, Option<ViewerLut>) {
    let file = match now {
        ViewerLut::File(path) => Some(path.as_path()),
        ViewerLut::VideoMonitor | ViewerLut::None => None,
    };
    let text = match now {
        ViewerLut::VideoMonitor => FOLLOW.to_string(),
        _ => shown(list, file),
    };
    let (mut follow, mut pick) = (false, None);
    let r = egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .width(width)
        .truncate()
        .show_ui(ui, |ui| {
            follow = ui.selectable_label(now.follows(), FOLLOW).clicked();
            let none = matches!(now, ViewerLut::None);
            entries(ui, list, (file, none), &mut pick);
        });
    let picked = match (follow, pick) {
        (true, _) => Some(ViewerLut::VideoMonitor),
        (false, Some(Some(path))) => Some(ViewerLut::File(path)),
        (false, Some(None)) => Some(ViewerLut::None),
        (false, None) => None,
    };
    (r.response, picked)
}

fn entries(
    ui: &mut egui::Ui,
    list: &LutList,
    (now, none): (Option<&Path>, bool),
    pick: &mut Option<Option<PathBuf>>,
) {
    if ui.selectable_label(none, NONE).clicked() {
        *pick = Some(None);
    }
    if let Some(path) = now.filter(|p| list.find(p).is_none())
        && ui.selectable_label(true, file_name(path)).clicked()
    {
        *pick = Some(Some(path.to_path_buf()));
    }
    for lut in &list.luts {
        let on = now == Some(lut.path.as_path());
        if ui.selectable_label(on, &lut.name).clicked() {
            *pick = Some(Some(lut.path.clone()));
        }
    }
}
