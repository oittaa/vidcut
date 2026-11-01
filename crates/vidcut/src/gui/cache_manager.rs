use std::collections::BTreeSet;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::clip_info::bytes;
use super::export::Ask;
use super::workspace::{Workspace, dim};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Column {
    Location,
    Library,
    #[default]
    Project,
    Size,
}

impl Column {
    const ALL: [Column; 4] = [
        Column::Location,
        Column::Library,
        Column::Project,
        Column::Size,
    ];

    fn title(self) -> &'static str {
        match self {
            Column::Location => "Location",
            Column::Library => "Project Library",
            Column::Project => "Project",
            Column::Size => "Render Cache",
        }
    }
}

const LOCATIONS: [(&str, &str); 2] = [("local", "Local"), ("network", "Network")];

struct Row {
    folder: PathBuf,
    projects: Vec<PathBuf>,
    locations: Vec<String>,
    libraries: Vec<String>,
    names: String,
    bytes: u64,
}

fn distinct(projects: &[Value], key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in projects {
        let v = p[key].as_str().unwrap_or("").to_string();
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

fn location_name(key: &str) -> &str {
    LOCATIONS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(key, |(_, name)| name)
}

impl Row {
    fn of(v: &Value) -> Self {
        let projects = v["projects"].as_array().cloned().unwrap_or_default();
        Self {
            folder: PathBuf::from(v["folder"].as_str().unwrap_or("")),
            projects: projects
                .iter()
                .filter_map(|p| p["path"].as_str().map(PathBuf::from))
                .collect(),
            locations: distinct(&projects, "location"),
            libraries: distinct(&projects, "library"),
            names: distinct(&projects, "name").join(", "),
            bytes: v["bytes"].as_u64().unwrap_or(0),
        }
    }

    fn text(&self, c: Column) -> String {
        match c {
            Column::Location => self
                .locations
                .iter()
                .map(|l| location_name(l))
                .collect::<Vec<_>>()
                .join(", "),
            Column::Library => self.libraries.join(", "),
            Column::Project => self.names.clone(),
            Column::Size => bytes(self.bytes),
        }
    }
}

#[derive(Default)]
pub(super) struct CacheManager {
    pub(super) open: bool,
    asked: Option<Ask>,
    clearing: Option<Ask>,
    rows: Vec<Row>,
    current: Option<PathBuf>,
    error: Option<String>,
    checked: BTreeSet<PathBuf>,
    location: Option<String>,
    library: Option<String>,
    sort: (Column, bool),
}

impl CacheManager {
    fn shown(&self) -> Vec<usize> {
        let mut shown: Vec<usize> = (0..self.rows.len())
            .filter(|&i| {
                let r = &self.rows[i];
                self.location
                    .as_ref()
                    .is_none_or(|l| r.locations.contains(l))
                    && self
                        .library
                        .as_ref()
                        .is_none_or(|l| r.libraries.contains(l))
            })
            .collect();
        let (column, descending) = self.sort;
        shown.sort_by(|&a, &b| {
            let (a, b) = (&self.rows[a], &self.rows[b]);
            let order = match column {
                Column::Size => a.bytes.cmp(&b.bytes),
                c => a.text(c).to_lowercase().cmp(&b.text(c).to_lowercase()),
            };
            if descending { order.reverse() } else { order }
        });
        shown
    }

    fn header(&mut self, ui: &mut egui::Ui, c: Column) {
        let text = match self.sort {
            (s, false) if s == c => format!("{} ⬆", c.title()),
            (s, true) if s == c => format!("{} ⬇", c.title()),
            _ => c.title().to_string(),
        };
        let label = egui::Label::new(dim(text)).sense(egui::Sense::click());
        if ui.add(label).clicked() {
            self.sort = (c, self.sort.0 == c && !self.sort.1);
        }
    }

    fn filters(&mut self, ui: &mut egui::Ui) {
        let mut libraries: Vec<String> = self
            .rows
            .iter()
            .flat_map(|r| r.libraries.iter().cloned())
            .collect();
        libraries.sort();
        libraries.dedup();
        ui.horizontal(|ui| {
            ui.label(dim("Location"));
            let shown = self.location.as_deref().map_or("All", location_name);
            egui::ComboBox::from_id_salt("render-cache-location")
                .selected_text(shown.to_string())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.location, None, "All");
                    for (key, name) in LOCATIONS {
                        ui.selectable_value(&mut self.location, Some(key.to_string()), name);
                    }
                });
            ui.add_space(12.0);
            ui.label(dim("Project Library"));
            egui::ComboBox::from_id_salt("render-cache-library")
                .selected_text(self.library.clone().unwrap_or_else(|| "All".into()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.library, None, "All");
                    for l in libraries {
                        ui.selectable_value(&mut self.library, Some(l.clone()), l);
                    }
                });
        });
    }

    fn table(&mut self, ui: &mut egui::Ui, shown: &[usize]) {
        egui::ScrollArea::both().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("render-cache-rows")
                .num_columns(4)
                .striped(true)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for c in Column::ALL {
                        self.header(ui, c);
                    }
                    ui.end_row();
                    for &i in shown {
                        let row = &self.rows[i];
                        for c in [Column::Location, Column::Library, Column::Project] {
                            ui.label(row.text(c));
                        }
                        let mut on = self.checked.contains(&row.folder);
                        let folder = row.folder.clone();
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut on, "");
                            ui.label(row.text(Column::Size));
                        });
                        match on {
                            true => self.checked.insert(folder),
                            false => self.checked.remove(&folder),
                        };
                        ui.end_row();
                    }
                });
        });
    }

    fn current_project(&self, ui: &mut egui::Ui) {
        let Some(row) = self
            .current
            .as_ref()
            .and_then(|c| self.rows.iter().find(|r| r.projects.contains(c)))
        else {
            return;
        };
        ui.label(RichText::new("Current Project").strong());
        egui::Grid::new("render-cache-current")
            .num_columns(4)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for c in Column::ALL {
                    ui.label(dim(c.title()));
                }
                ui.end_row();
                for c in Column::ALL {
                    ui.label(row.text(c));
                }
                ui.end_row();
            });
        ui.separator();
    }
}

impl Workspace {
    pub(super) fn open_cache_manager(&mut self, ctx: &egui::Context) {
        self.cache_manager.open = true;
        self.cache_manager.asked = Some(Ask::send(&self.live, ctx, "cache.list", json!({})));
    }

    pub(super) fn cache_manager_window(&mut self, ctx: &egui::Context) {
        if !self.cache_manager.open {
            return;
        }
        if let Some(answer) = self.cache_manager.clearing.as_ref().and_then(Ask::take) {
            self.cache_manager.clearing = None;
            if let Err(e) = answer {
                self.cache_manager.error = Some(e.message);
            }
            self.open_cache_manager(ctx);
        }
        let open_project = self.snap.project.as_ref().map(|p| p.path.clone());
        let m = &mut self.cache_manager;
        if let Some(answer) = m.asked.as_ref().and_then(Ask::take) {
            m.asked = None;
            match answer {
                Ok(r) => {
                    m.rows = r.result["rows"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(Row::of)
                        .collect();
                    let folders: BTreeSet<PathBuf> =
                        m.rows.iter().map(|r| r.folder.clone()).collect();
                    m.checked.retain(|f| folders.contains(f));
                    m.current = open_project.map(|p| vidcut_core::dirs::canonical(&p).unwrap_or(p));
                    m.error = None;
                }
                Err(e) => m.error = Some(e.message),
            }
        }
        let (mut open, mut close, mut clear) = (true, false, false);
        let shown = m.shown();
        super::dialog::tool_window(
            ctx,
            "Manage Render Cache",
            &mut open,
            [720.0, 480.0],
            |ui| {
                if let Some(e) = &m.error {
                    super::dialog::warn(ui, e.clone());
                }
                m.current_project(ui);
                m.filters(ui);
                m.table(ui, &shown);
                let any = shown.iter().any(|&i| m.checked.contains(&m.rows[i].folder));
                super::dialog::footer(ui, |ui| {
                    close = super::dialog::main_button(ui, "Close", true);
                    let button = egui::Button::new("Clear Selected Cache");
                    clear = ui
                        .add_enabled(any && m.clearing.is_none(), button)
                        .clicked();
                });
            },
        );
        if clear {
            let projects: Vec<PathBuf> = shown
                .iter()
                .map(|&i| &m.rows[i])
                .filter(|r| m.checked.contains(&r.folder))
                .flat_map(|r| r.projects.iter().cloned())
                .collect();
            m.checked.clear();
            let params = json!({ "projects": projects });
            m.clearing = Some(Ask::send(&self.live, ctx, "cache.clear", params));
        }
        if !open || close {
            self.cache_manager.open = false;
        }
    }
}
