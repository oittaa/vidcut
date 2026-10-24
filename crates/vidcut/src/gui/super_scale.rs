use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::model::{NoiseReduction, ScaleFactor, Sharpness, SuperScale, Upscaler};

use super::workspace::Workspace;

pub(super) const FACTORS: [(ScaleFactor, &str); 5] = [
    (ScaleFactor::None, "Off"),
    (ScaleFactor::Auto, "Auto"),
    (ScaleFactor::X2, "2x"),
    (ScaleFactor::X3, "3x"),
    (ScaleFactor::X4, "4x"),
];

pub(super) const UPSCALERS: [(Upscaler, &str); 2] =
    [(Upscaler::Ai, "AI"), (Upscaler::Classic, "Classic")];

pub(super) const NOISE: [(NoiseReduction, &str); 3] = [
    (NoiseReduction::Low, "Low"),
    (NoiseReduction::Medium, "Medium"),
    (NoiseReduction::High, "High"),
];

pub(super) const SHARPNESS: [(Sharpness, &str); 3] = [
    (Sharpness::Low, "Low"),
    (Sharpness::Medium, "Medium"),
    (Sharpness::High, "High"),
];

pub(super) fn label_of<T: PartialEq + Copy>(list: &[(T, &'static str)], value: T) -> &'static str {
    list.iter()
        .find(|(v, _)| *v == value)
        .map_or("", |(_, l)| *l)
}

pub(super) fn described(s: SuperScale) -> String {
    let factor = label_of(&FACTORS, s.factor);
    match s.upscaler {
        Upscaler::Ai => format!(
            "{factor}, AI, Noise Reduction {}",
            label_of(&NOISE, s.noise_reduction)
        ),
        Upscaler::Classic => format!(
            "{factor}, Classic, Sharpness {}",
            label_of(&SHARPNESS, s.sharpness)
        ),
    }
}

pub(super) fn unfit<'a>(
    s: SuperScale,
    media: impl IntoIterator<Item = &'a vidcut_core::model::Media>,
) -> Option<String> {
    if !s.ai() {
        return None;
    }
    media
        .into_iter()
        .find_map(vidcut_av::upscale::unfit)
        .map(|why| format!("AI can't take {why}"))
}

fn field<T: serde::de::DeserializeOwned>(params: &Value, key: &str) -> Option<T> {
    params
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

impl Workspace {
    pub(super) fn set_super_scale(&mut self, media: &[String], mut params: Value) {
        params["media"] = json!(media);
        let ai_on = self.project_shared().is_some_and(|p| {
            media.iter().filter_map(|id| p.media(id)).any(|m| {
                let mut s = m.super_scale;
                s.factor = field(&params, "super_scale").unwrap_or(s.factor);
                s.upscaler = field(&params, "upscaler").unwrap_or(s.upscaler);
                s.ai()
            })
        });
        if ai_on && !vidcut_av::upscale::needed(&crate::paths::models_dir()).is_empty() {
            self.super_scale_asking = Some(params);
        } else {
            self.send("media.set", params);
        }
    }

    pub(super) fn super_scale_prompt(&mut self, ctx: &egui::Context) {
        let Some(mut params) = self.super_scale_asking.clone() else {
            return;
        };
        let needs = vidcut_av::upscale::needed(&crate::paths::models_dir());
        if needs.is_empty() {
            self.super_scale_asking = None;
            self.send("media.set", params);
            return;
        }
        match super::speech::ask_download(ctx, "super-scale-download", &needs, "Download") {
            Some(true) => {
                self.super_scale_asking = None;
                params["download"] = json!(true);
                self.send("media.set", params);
            }
            Some(false) => self.super_scale_asking = None,
            None => {}
        }
    }
}
