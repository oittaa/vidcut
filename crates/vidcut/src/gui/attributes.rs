use eframe::egui;
use serde_json::json;
use vidcut_core::request::{ClipAttribute, KeyTiming};

use super::dialog;
use super::workspace::Workspace;

type A = ClipAttribute;

const ZOOM: [A; 2] = [A::ZoomX, A::ZoomY];
const CROP: [A; 5] = [
    A::CropLeft,
    A::CropRight,
    A::CropTop,
    A::CropBottom,
    A::CropSoftness,
];
const FLIP: [A; 2] = [A::FlipVertical, A::FlipHorizontal];
const VIDEO: [A; 20] = [
    A::CompositeMode,
    A::Opacity,
    A::Position,
    A::Rotation,
    A::Anchor,
    A::Pitch,
    A::Yaw,
    A::ZoomX,
    A::ZoomY,
    A::CropLeft,
    A::CropRight,
    A::CropTop,
    A::CropBottom,
    A::CropSoftness,
    A::FlipVertical,
    A::FlipHorizontal,
    A::RetimeProcess,
    A::VideoEffects,
    A::Grade,
    A::Composition,
];
const AUDIO: [A; 3] = [A::Volume, A::AudioEffects, A::Equalizer];
const AUDIO_REMOVED: [A; 5] = [
    A::Volume,
    A::AudioEffects,
    A::Equalizer,
    A::VoiceIsolation,
    A::DialogueLeveler,
];

#[derive(Debug, Clone)]
pub struct AttributesDialog {
    clips: Vec<String>,
    paste: bool,
    picked: Vec<A>,
    timing: KeyTiming,
    ripple: bool,
}

#[derive(Debug, Clone)]
pub struct PasteChoice {
    copy: u64,
    picked: Vec<A>,
    timing: KeyTiming,
    ripple: bool,
}

fn title(a: A) -> &'static str {
    match a {
        A::ClipColor => "Clip Color",
        A::CompositeMode => "Composite Mode",
        A::Opacity => "Opacity",
        A::Position => "Position",
        A::Rotation => "Rotation Angle",
        A::Anchor => "Anchor Point",
        A::Pitch => "Pitch",
        A::Yaw => "Yaw",
        A::ZoomX => "Scale X",
        A::ZoomY => "Scale Y",
        A::CropLeft => "Left",
        A::CropRight => "Right",
        A::CropTop => "Top",
        A::CropBottom => "Bottom",
        A::CropSoftness => "Softness",
        A::FlipVertical => "Vertical",
        A::FlipHorizontal => "Horizontal",
        A::RetimeProcess => "Retime Process",
        A::VideoEffects => "Effects",
        A::Grade => "Color Correction",
        A::Composition => "Effects Page",
        A::Volume => "Volume",
        A::AudioEffects => "Effects",
        A::Equalizer => "Equalizer",
        A::Speed => "Speed",
        A::VoiceIsolation => "Voice Isolation",
        A::DialogueLeveler => "Dialogue Leveler",
    }
}

fn set(picked: &mut Vec<A>, items: &[A], on: bool) {
    picked.retain(|a| !items.contains(a));
    if on {
        picked.extend_from_slice(items);
    }
}

fn group(ui: &mut egui::Ui, label: &str, items: &[A], picked: &mut Vec<A>) {
    let n = items.iter().filter(|a| picked.contains(a)).count();
    let mut all = n == items.len();
    let partly = n > 0 && n < items.len();
    let box_ = egui::Checkbox::new(&mut all, label).indeterminate(partly);
    if ui.add(box_).changed() {
        set(picked, items, all);
    }
}

fn item(ui: &mut egui::Ui, a: A, picked: &mut Vec<A>) {
    let mut on = picked.contains(&a);
    if ui.checkbox(&mut on, title(a)).changed() {
        set(picked, &[a], on);
    }
}

fn indented(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.indent(ui.next_auto_id(), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        add(ui)
    });
}

fn video(ui: &mut egui::Ui, picked: &mut Vec<A>) {
    group(ui, "All Video Attributes", &VIDEO, picked);
    indented(ui, |ui| {
        for a in &VIDEO[..7] {
            item(ui, *a, picked);
        }
        for (label, items) in [("Zoom", &ZOOM[..]), ("Crop", &CROP), ("Flip", &FLIP)] {
            group(ui, label, items, picked);
            indented(ui, |ui| items.iter().for_each(|a| item(ui, *a, picked)));
        }
        for a in &VIDEO[16..] {
            item(ui, *a, picked);
        }
    });
}

fn audio(ui: &mut egui::Ui, items: &[A], picked: &mut Vec<A>) {
    group(ui, "All Audio Attributes", items, picked);
    indented(ui, |ui| items.iter().for_each(|a| item(ui, *a, picked)));
}

impl Workspace {
    pub(super) fn open_paste_attributes(&mut self, clips: Vec<String>) {
        let Some(copied) = self.snap.copied_clip.clone() else {
            return;
        };
        let last = self.timeline.paste_last.clone();
        let d = match last.filter(|l| l.copy == copied.copy) {
            Some(l) => AttributesDialog {
                clips,
                paste: true,
                picked: l.picked,
                timing: l.timing,
                ripple: l.ripple,
            },
            None => AttributesDialog {
                clips,
                paste: true,
                picked: copied.picked.clone(),
                timing: KeyTiming::Maintain,
                ripple: false,
            },
        };
        self.timeline.attributes = Some(d);
    }

    pub(super) fn open_remove_attributes(&mut self, clips: Vec<String>) {
        self.timeline.attributes = Some(AttributesDialog {
            clips,
            paste: false,
            picked: Vec::new(),
            timing: KeyTiming::Maintain,
            ripple: false,
        });
    }

    fn clips_text(&self, clips: &[String]) -> String {
        let Some(p) = self.project() else {
            return String::new();
        };
        let shown = clips.iter().filter_map(|id| p.clip(id));
        let (video, audio) = shown.fold((0, 0), |(v, a), c| {
            (v + usize::from(c.video), a + usize::from(c.audio.is_some()))
        });
        format!("{video} Video and {audio} Audio Clips")
    }

    pub(super) fn attributes_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.attributes.take() else {
            return;
        };
        let from = (self.snap.copied_clip.as_ref()).map(|c| (c.name.clone(), c.copy));
        if d.paste && from.is_none() {
            return;
        }
        let to = self.clips_text(&d.clips);
        let name = if d.paste {
            "Paste Attributes"
        } else {
            "Remove Attributes"
        };
        let (mut open, mut done, mut apply) = (true, false, false);
        dialog::window_of(ctx, name, &mut open, 520.0, |ui| {
            if d.paste {
                dialog::form(ui, "attributes-form", |ui| {
                    dialog::row(ui, "From", |ui| {
                        ui.label(from.as_ref().map_or("", |f| f.0.as_str()))
                    });
                    dialog::row(ui, "To", |ui| ui.label(to.as_str()));
                    dialog::row(ui, "Keyframes", |ui| {
                        dialog::switch(ui, |ui| {
                            ui.selectable_value(
                                &mut d.timing,
                                KeyTiming::Maintain,
                                "Maintain Timing",
                            );
                            ui.selectable_value(
                                &mut d.timing,
                                KeyTiming::Stretch,
                                "Stretch to Fit",
                            );
                        })
                    });
                });
            }
            ui.columns(2, |cols| {
                video(&mut cols[0], &mut d.picked);
                let ui = &mut cols[1];
                group(ui, "Clip Attributes", &[A::ClipColor], &mut d.picked);
                indented(ui, |ui| item(ui, A::ClipColor, &mut d.picked));
                ui.add_space(8.0);
                let sound: &[A] = if d.paste { &AUDIO } else { &AUDIO_REMOVED };
                audio(ui, sound, &mut d.picked);
                ui.add_space(8.0);
                item(ui, A::Speed, &mut d.picked);
                indented(ui, |ui| {
                    let speed = d.picked.contains(&A::Speed);
                    ui.add_enabled(speed, egui::Checkbox::new(&mut d.ripple, "Ripple Timeline"));
                });
            });
            dialog::footer(ui, |ui| {
                apply = dialog::main_button(ui, "Apply", !d.picked.is_empty());
                done = ui.button("Cancel").clicked();
            });
        });
        if apply {
            let ripple = d.ripple && d.picked.contains(&A::Speed);
            match d.paste {
                true => {
                    self.send(
                        "clip.paste_attributes",
                        json!({
                            "clips": d.clips,
                            "attributes": d.picked,
                            "keyframes": d.timing,
                            "ripple": ripple,
                        }),
                    );
                    self.timeline.paste_last = from.map(|(_, copy)| PasteChoice {
                        copy,
                        picked: d.picked.clone(),
                        timing: d.timing,
                        ripple: d.ripple,
                    });
                }
                false => self.send(
                    "clip.remove_attributes",
                    json!({ "clips": d.clips, "attributes": d.picked, "ripple": ripple }),
                ),
            }
        }
        if open && !done && !apply {
            self.timeline.attributes = Some(d);
        }
    }
}
