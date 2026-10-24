use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::model::{ScaleFactor, Sharpness, SuperScale, Upscaler};

use super::workspace::Workspace;

pub(super) const FACTORS: [(ScaleFactor, &str); 5] = [
    (ScaleFactor::None, "Off"),
    (ScaleFactor::Auto, "Auto"),
    (ScaleFactor::X2, "2x"),
    (ScaleFactor::X3, "3x"),
    (ScaleFactor::X4, "4x"),
];

pub(super) const UPSCALERS: [(Upscaler, &str); 3] = [
    (Upscaler::AiBest, "AI Best"),
    (Upscaler::AiFast, "AI Fast"),
    (Upscaler::Classic, "Classic"),
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
        Upscaler::AiFast => format!("{factor}, AI Fast"),
        Upscaler::AiBest => format!("{factor}, AI Best"),
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
        .find_map(|m| vidcut_av::ai::unfit(m, s.upscaler))
        .map(|why| format!("{} can't take {why}", label_of(&UPSCALERS, s.upscaler)))
}

fn field<T: serde::de::DeserializeOwned>(params: &Value, key: &str) -> Option<T> {
    params
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub(super) fn needed(upscalers: &[Upscaler]) -> Vec<vidcut_av::models::Need> {
    let dir = crate::paths::models_dir();
    upscalers
        .iter()
        .flat_map(|u| vidcut_av::ai::needed(&dir, *u))
        .collect()
}

impl Workspace {
    fn ai_after(&self, params: &Value) -> Vec<Upscaler> {
        let media: Vec<String> = field(params, "media").unwrap_or_default();
        let set = vidcut_core::ops::SuperScaleSet {
            factor: field(params, "super_scale"),
            sharpness: None,
            upscaler: field(params, "upscaler"),
        };
        self.project_shared()
            .map(|p| vidcut_core::ops::ai_upscalers(&p, &media, &set))
            .unwrap_or_default()
    }

    pub(super) fn set_super_scale(&mut self, media: &[String], mut params: Value) {
        params["media"] = json!(media);
        if needed(&self.ai_after(&params)).is_empty() {
            self.send("media.set", params);
        } else {
            self.super_scale_asking = Some(params);
        }
    }

    pub(super) fn super_scale_prompt(&mut self, ctx: &egui::Context) {
        let Some(mut params) = self.super_scale_asking.clone() else {
            return;
        };
        let needs = needed(&self.ai_after(&params));
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
