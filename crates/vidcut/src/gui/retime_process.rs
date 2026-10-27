use eframe::egui;
use serde_json::json;
use vidcut_core::model::Retime;

use super::super_scale::field;
use super::workspace::Workspace;

impl Workspace {
    pub(super) fn set_retime(&mut self, clips: &[String], retime: Retime) {
        let params = json!({ "clips": clips, "retime": retime });
        let dir = crate::paths::models_dir();
        if vidcut_av::interpolate::needed(&dir, retime).is_empty() {
            self.send("clip.set", params);
        } else {
            self.retime_asking = Some(params);
        }
    }

    pub(super) fn retime_prompt(&mut self, ctx: &egui::Context) {
        let Some(mut params) = self.retime_asking.clone() else {
            return;
        };
        let retime = field(&params, "retime").unwrap_or_default();
        let needs = vidcut_av::interpolate::needed(&crate::paths::models_dir(), retime);
        if needs.is_empty() {
            self.retime_asking = None;
            self.send("clip.set", params);
            return;
        }
        match super::speech::ask_download(ctx, "retime-download", &needs, "Download") {
            Some(true) => {
                self.retime_asking = None;
                params["download"] = json!(true);
                self.send("clip.set", params);
            }
            Some(false) => self.retime_asking = None,
            None => {}
        }
    }
}
