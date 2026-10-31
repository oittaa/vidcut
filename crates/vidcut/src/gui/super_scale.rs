use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::model::{MediaUpscaler, ProjectUpscaler, Retime, Sharpness, SuperScale, Upscaler};

use super::workspace::Workspace;

const NAMES: [(Upscaler, &str); 4] = [
    (Upscaler::Macos, "AI"),
    (Upscaler::AiSlow, "AI Slow"),
    (Upscaler::AiFast, "AI Fast"),
    (Upscaler::Classic, "Classic"),
];

#[cfg(target_os = "macos")]
pub(super) const UPSCALERS: &[(MediaUpscaler, &str)] = &[
    (MediaUpscaler::Project, "Project Settings"),
    (MediaUpscaler::Macos, "AI"),
    (MediaUpscaler::Classic, "Classic"),
];

#[cfg(not(target_os = "macos"))]
pub(super) const UPSCALERS: &[(MediaUpscaler, &str)] = &[
    (MediaUpscaler::Project, "Project Settings"),
    (MediaUpscaler::AiSlow, "AI Slow"),
    (MediaUpscaler::AiFast, "AI Fast"),
    (MediaUpscaler::Classic, "Classic"),
];

#[cfg(target_os = "macos")]
pub(super) const PROJECT_UPSCALERS: &[(ProjectUpscaler, &str)] = &[
    (ProjectUpscaler::Auto, "Auto"),
    (ProjectUpscaler::Macos, "AI"),
    (ProjectUpscaler::Classic, "Classic"),
];

#[cfg(not(target_os = "macos"))]
pub(super) const PROJECT_UPSCALERS: &[(ProjectUpscaler, &str)] = &[
    (ProjectUpscaler::Auto, "Auto"),
    (ProjectUpscaler::AiSlow, "AI Slow"),
    (ProjectUpscaler::AiFast, "AI Fast"),
    (ProjectUpscaler::Classic, "Classic"),
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

pub(super) fn upscaler_text(u: MediaUpscaler, label: &str, project: ProjectUpscaler) -> String {
    match u {
        MediaUpscaler::Project => {
            format!(
                "Project Settings ({})",
                label_of(&NAMES, project.upscaler())
            )
        }
        _ => label.into(),
    }
}

pub(super) fn project_upscaler_text(u: ProjectUpscaler, label: &str) -> String {
    match u {
        ProjectUpscaler::Auto => format!("Auto ({})", label_of(&NAMES, u.upscaler())),
        _ => label.into(),
    }
}

pub(super) fn described(s: SuperScale) -> String {
    match s.upscaler {
        Upscaler::Classic => format!("Classic, Sharpness {}", label_of(&SHARPNESS, s.sharpness)),
        u => label_of(&NAMES, u).to_string(),
    }
}

pub(super) fn choices() -> Vec<(&'static str, &'static str)> {
    UPSCALERS
        .iter()
        .filter_map(|&(m, name)| m.upscaler().map(|u| (name, does(u))))
        .collect()
}

fn does(u: Upscaler) -> &'static str {
    match u {
        Upscaler::Macos => "The scaler of macOS, for pictures up to 1920×1080.",
        Upscaler::AiSlow => {
            "Slow. Invents more detail, and can look better on low-quality sources."
        }
        Upscaler::AiFast => "Fast. Adds detail to SDR and HDR pictures of any size.",
        Upscaler::Classic => {
            "Lanczos, or bicubic at Low Sharpness. Adds no detail. Runs everywhere."
        }
    }
}

pub(super) fn unfit<'a>(
    s: SuperScale,
    media: impl IntoIterator<Item = &'a vidcut_core::model::Media>,
    timeline: Option<vidcut_core::model::Hdr>,
) -> Option<String> {
    if !s.ai() {
        return None;
    }
    media
        .into_iter()
        .find_map(|m| vidcut_av::super_scale::unfit(m, s.upscaler, timeline))
        .map(|why| format!("{} can't take {why}", label_of(&NAMES, s.upscaler)))
}

pub(super) fn field<T: serde::de::DeserializeOwned>(params: &Value, key: &str) -> Option<T> {
    params
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub(super) fn needed(retimes: &[Retime], upscalers: &[Upscaler]) -> Vec<vidcut_av::models::Need> {
    let dir = crate::paths::models_dir();
    retimes
        .iter()
        .flat_map(|r| vidcut_av::interpolate::needed(&dir, *r))
        .chain(
            upscalers
                .iter()
                .flat_map(|u| vidcut_av::super_scale::needed(&dir, *u)),
        )
        .collect()
}

fn with_download(method: &str, params: &mut Value) {
    match params.get_mut("commands").and_then(Value::as_array_mut) {
        Some(commands) if method == "apply" => {
            for c in commands {
                c["params"]["download"] = json!(true);
            }
        }
        _ => params["download"] = json!(true),
    }
}

impl Workspace {
    fn models_of(&self, method: &str, params: &Value) -> vidcut_core::editor::Models {
        let Some(p) = self.project_shared() else {
            return Default::default();
        };
        if method == "media.set" {
            let media: Vec<String> = field(params, "media").unwrap_or_default();
            let set = vidcut_core::ops::SuperScaleSet {
                on: field(params, "super_scale"),
                sharpness: None,
                upscaler: field(params, "upscaler"),
            };
            return (Vec::new(), vidcut_core::ops::ai_upscalers(&p, &media, &set));
        }
        serde_json::from_value(json!({ "method": method, "params": params }))
            .ok()
            .and_then(|r| {
                let env = crate::env::AvEnv::new();
                vidcut_core::editor::models_after(&p, &r, &env, None).ok()
            })
            .unwrap_or_default()
    }

    pub(super) fn send_asking(&mut self, method: &'static str, params: Value) {
        let (retimes, upscalers) = self.models_of(method, &params);
        let needs = needed(&retimes, &upscalers);
        if needs.is_empty() {
            self.send(method, params);
        } else {
            self.models_asking = Some((method, params, needs));
        }
    }

    pub(super) fn models_prompt(&mut self, ctx: &egui::Context) {
        let Some((method, params, needs)) = &self.models_asking else {
            return;
        };
        match super::speech::ask_download(ctx, "models-download", needs, "Download") {
            Some(true) => {
                let (method, mut params) = (*method, params.clone());
                self.models_asking = None;
                with_download(method, &mut params);
                self.send(method, params);
            }
            Some(false) => self.models_asking = None,
            None => {}
        }
    }

    pub(super) fn set_super_scale(&mut self, media: &[String], mut params: Value) {
        params["media"] = json!(media);
        self.send_asking("media.set", params);
    }
}
