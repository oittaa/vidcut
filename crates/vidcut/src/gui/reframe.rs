use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::model::{FindModel, Project, Subject};
use vidcut_core::reframe::{SMOOTH, USUAL_SMOOTH};
use vidcut_core::request::ReframeMode;

use super::dialog;
use super::inspector::{label, switched_group};
use super::workspace::Workspace;

const NAME: &str = "Smart Reframe";
const SUBJECTS: [(Subject, &str); 4] = [
    (Subject::Person, "Person"),
    (Subject::Face, "Face"),
    (Subject::Vehicle, "Vehicle"),
    (Subject::Text, "Text"),
];

#[derive(Debug, Clone, Copy)]
pub struct Reframing {
    mode: ReframeMode,
    subject: Subject,
    smooth: f64,
}

impl Default for Reframing {
    fn default() -> Self {
        Self {
            mode: ReframeMode::Auto,
            subject: Subject::Person,
            smooth: USUAL_SMOOTH,
        }
    }
}

impl Reframing {
    fn params(&self, clips: &[String]) -> Value {
        let mut params = json!({ "clips": clips });
        if self.mode != ReframeMode::Auto {
            params["mode"] = json!(self.mode);
        }
        if self.subject != Subject::Person {
            params["subject"] = json!(self.subject);
        }
        if self.smooth != USUAL_SMOOTH {
            params["smooth"] = json!(self.smooth);
        }
        params
    }
}

fn takes(p: &Project, id: &str) -> bool {
    vidcut_core::ops::picture::reframable(p, id).is_ok()
}

fn rows(ui: &mut egui::Ui, r: &mut Reframing) {
    label(ui, "Mode", r.mode != ReframeMode::Auto);
    dialog::switch(ui, |ui| {
        for m in ReframeMode::ALL {
            if ui
                .add(egui::Button::selectable(m == r.mode, m.label()))
                .clicked()
            {
                r.mode = m;
            }
        }
    });
    ui.end_row();
    label(ui, "Subject", r.subject != Subject::Person);
    let shown = SUBJECTS.iter().find(|(s, _)| *s == r.subject);
    egui::ComboBox::from_id_salt("reframe-subject")
        .selected_text(shown.map_or("", |(_, l)| *l))
        .show_ui(ui, |ui| {
            for (s, l) in SUBJECTS {
                ui.selectable_value(&mut r.subject, s, l);
            }
        });
    ui.end_row();
    label(ui, "Smooth", r.smooth != USUAL_SMOOTH);
    ui.add(
        egui::DragValue::new(&mut r.smooth)
            .range(SMOOTH.0..=SMOOTH.1)
            .speed(0.01)
            .fixed_decimals(2)
            .suffix(" s"),
    );
    ui.end_row();
}

impl Workspace {
    pub(super) fn reframe_section(&mut self, ui: &mut egui::Ui, p: &Project, ids: &[String]) {
        let clips: Vec<String> = ids.iter().filter(|id| takes(p, id)).cloned().collect();
        if clips.is_empty() {
            return;
        }
        let running = (self.snap.jobs.iter()).any(|j| j.unfinished() && j.kind == "reframe");
        let mut r = self.inspector.reframing;
        let mut go = false;
        switched_group(
            ui,
            NAME,
            None,
            |_| (),
            |ui| {
                rows(ui, &mut r);
                label(ui, "", false);
                go = ui
                    .add_enabled(!running, egui::Button::new("Reframe"))
                    .clicked();
                ui.end_row();
            },
        );
        self.inspector.reframing = r;
        if go {
            let ask = ("picture.reframe", r.params(&clips));
            self.ask(vec![ask], Some((r.subject, FindModel::Fast)));
        }
    }
}
