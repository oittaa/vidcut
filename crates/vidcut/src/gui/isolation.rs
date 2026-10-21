use eframe::egui;
use serde_json::json;
use vidcut_core::Project;

use super::workspace::Workspace;

pub(super) const FIRST_AMOUNT: f64 = 75.0;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Isolate {
    Clips(Vec<String>),
    Track(String),
}

pub(super) fn heard(p: &Project, clip: &str) -> Option<Option<f64>> {
    let t = p
        .tracks
        .iter()
        .find(|t| t.clips.iter().any(|c| c.id == clip))?;
    let audio = t.clips.iter().find(|c| c.id == clip)?.audio.as_ref()?;
    Some(
        audio
            .voice_isolation
            .or(t.voice_isolation)
            .filter(|a| *a > 0.0),
    )
}

#[derive(Default)]
pub(super) struct IsolationUi {
    asking: Option<(Isolate, f64)>,
    dragging: Option<(Isolate, f64)>,
}

impl Workspace {
    pub(super) fn isolate(&mut self, on: Isolate, amount: f64) {
        let on = match on {
            Isolate::Clips(clips) => {
                let heard: Vec<String> = self
                    .project()
                    .map(|p| {
                        clips
                            .into_iter()
                            .filter(|id| p.clip(id).is_some_and(|c| c.audio.is_some()))
                            .collect()
                    })
                    .unwrap_or_default();
                if heard.is_empty() {
                    return;
                }
                Isolate::Clips(heard)
            }
            track => track,
        };
        let dir = crate::paths::models_dir();
        if amount > 0.0 && !vidcut_av::isolate::needed(&dir).is_empty() {
            self.isolation.asking = Some((on, amount));
            return;
        }
        self.send_isolation(&on, amount, false);
    }

    fn send_isolation(&self, on: &Isolate, amount: f64, download: bool) {
        match on {
            Isolate::Clips(clips) => self.send(
                "clip.set",
                json!({ "clips": clips, "voice_isolation": amount, "download": download }),
            ),
            Isolate::Track(track) => self.send(
                "track.set",
                json!({ "track": track, "voice_isolation": amount, "download": download }),
            ),
        }
    }

    pub(super) fn isolation_controls(
        &mut self,
        ui: &mut egui::Ui,
        on: Isolate,
        amount: Option<f64>,
        label: &str,
    ) -> bool {
        let mut checked = amount.is_some();
        let toggled = ui.checkbox(&mut checked, label).clicked();
        if toggled {
            self.isolate(on.clone(), if checked { FIRST_AMOUNT } else { 0.0 });
        }
        if let Some(stored) = amount {
            let mut value = match &self.isolation.dragging {
                Some((o, v)) if *o == on => *v,
                _ => stored,
            };
            let r = ui.add(
                egui::DragValue::new(&mut value)
                    .range(1.0..=100.0)
                    .speed(0.5)
                    .fixed_decimals(0)
                    .prefix("Amount "),
            );
            if r.changed() {
                self.isolation.dragging = Some((on.clone(), value));
            }
            if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged()))
                && let Some((o, v)) = self.isolation.dragging.take()
                && v != stored
            {
                self.isolate(o, v);
            }
        }
        toggled
    }

    pub(super) fn isolation_prompt(&mut self, ctx: &egui::Context) {
        let Some((on, amount)) = self.isolation.asking.clone() else {
            return;
        };
        let needs = vidcut_av::isolate::needed(&crate::paths::models_dir());
        if needs.is_empty() {
            self.isolation.asking = None;
            self.send_isolation(&on, amount, false);
            return;
        }
        match super::speech::ask_download(ctx, "voice-isolation-download", &needs, "Download") {
            Some(true) => {
                self.isolation.asking = None;
                self.send_isolation(&on, amount, true);
            }
            Some(false) => self.isolation.asking = None,
            None => {}
        }
    }
}
