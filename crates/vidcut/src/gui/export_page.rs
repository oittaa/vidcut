use std::path::PathBuf;

use eframe::egui::{self, RichText};
use serde_json::json;
use vidcut_core::model::RenderRange;
use vidcut_core::request::ViewerMode;

use super::export::{
    self, Picked, SavePreset, Tab, audio_tab, file_tab, plan_and_command, subtitle_settings,
    video_tab,
};
use super::theme::{self, Deco};
use super::workspace::{BOTTOM_SHARE, Workspace, dim};
use crate::live::JobInfo;

impl Workspace {
    pub(super) fn enter_export_page(&mut self) {
        if self.snap.viewer != ViewerMode::Timeline {
            self.export.viewer_before = Some(self.snap.viewer);
            self.send("viewer.set", json!({ "mode": ViewerMode::Timeline }));
        }
    }

    pub(super) fn leave_export_page(&mut self) {
        if let Some(mode) = self.export.viewer_before.take() {
            self.send("viewer.set", json!({ "mode": mode }));
        }
    }

    pub(super) fn export_page_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.export.render.is_none() {
            self.want_render_settings(&ctx);
        }
        let panel = theme::colors().panel;
        egui::Panel::left("render-settings")
            .resizable(true)
            .default_size(420.0)
            .min_size(320.0)
            .frame(theme::card(panel))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| self.render_settings_panel(ui))
            });
        let frame = theme::card(panel);
        let spacing = ctx.global_style().spacing.clone();
        let chrome =
            frame.total_margin().sum().y + spacing.interact_size.y + spacing.item_spacing.y;
        let wanted = self.timeline_wanted().unwrap_or(300.0) + chrome;
        let least = self.timeline_least(&spacing).map_or(140.0, |h| h + chrome);
        let room = ui.available_rect_before_wrap().height();
        let bottom = (ctx.content_rect().height() * BOTTOM_SHARE).min(room);
        egui::Panel::bottom("export-timeline")
            .resizable(true)
            .default_size(wanted.min(bottom).max(least))
            .min_size(least)
            .frame(frame)
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| self.timeline_panel(ui))
            });
        egui::Panel::right("render-queue")
            .resizable(true)
            .default_size(320.0)
            .min_size(240.0)
            .frame(theme::card(panel))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| self.render_queue_panel(ui))
            });
        egui::CentralPanel::default()
            .frame(theme::card(theme::colors().panel_dark))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Viewer, |ui| self.viewer_panel(ui))
            });
        self.settle_picture_wheel(&ctx);
    }

    pub(super) fn render_whole(&self) -> bool {
        self.project()
            .and_then(|p| p.timeline())
            .is_none_or(|t| t.render == RenderRange::Entire)
    }

    pub(super) fn render_span(&self) -> (i64, i64) {
        let Some(p) = self.project() else {
            return (0, 0);
        };
        let end = p.end();
        if self.render_whole() {
            return (0, end);
        }
        (
            p.marks.mark_in.unwrap_or(0),
            p.marks.mark_out.unwrap_or(end),
        )
    }

    fn render_range_spec(&self) -> String {
        let marks = self.project().map(|p| p.marks.clone()).unwrap_or_default();
        let whole = self.render_whole();
        let from = match marks.mark_in {
            Some(_) if !whole => "in",
            _ => "start",
        };
        let to = match marks.mark_out {
            Some(_) if !whole => "out",
            _ => "end",
        };
        format!("{from}..{to}")
    }

    pub(super) fn render_range_menu(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let whole = self.render_whole();
        let text = |r: RenderRange| match r {
            RenderRange::Entire => "Entire Timeline",
            RenderRange::InOut => "In/Out Range",
        };
        let now = match whole {
            true => RenderRange::Entire,
            false => RenderRange::InOut,
        };
        ui.horizontal(|ui| {
            ui.label(dim("Render"));
            egui::ComboBox::from_id_salt("export-render-range")
                .selected_text(text(now))
                .show_ui(ui, |ui| {
                    for r in [RenderRange::Entire, RenderRange::InOut] {
                        if ui.selectable_label(now == r, text(r)).clicked() && now != r {
                            self.send("timeline.set", json!({ "render": r }));
                        }
                    }
                });
        })
        .response
    }

    pub(super) fn render_range_row(&self, ui: &mut egui::Ui) {
        let (a, b) = self.render_span();
        ui.horizontal(|ui| {
            ui.label(dim("IN"));
            ui.monospace(self.tc(a));
            ui.add_space(12.0);
            ui.label(dim("OUT"));
            ui.monospace(self.tc((b - 1).max(a)));
            ui.add_space(12.0);
            ui.label(dim("DURATION"));
            ui.monospace(self.src_tc((b - a).max(0)));
        });
    }

    fn render_settings_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(mut d) = self.export.render.take() else {
            ui.label(RichText::new("Render Settings").strong());
            ui.separator();
            super::dialog::note(ui, "Loading presets…");
            return;
        };
        d.range = self.render_range_spec();
        if let Some(p) = &d.picking
            && let Some(choice) = p.take()
        {
            d.picking = None;
            if let Some(path) = choice {
                d.folder = path.display().to_string();
            }
        }
        let presets = self.export.presets.clone();
        let title = presets
            .iter()
            .find(|p| p.name == d.preset)
            .map_or(d.preset.clone(), |p| p.title.clone());
        let user = presets.iter().any(|p| p.name == d.preset && p.user);
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(RichText::new(format!("Render Settings - {title}")).strong())
                    .truncate(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("⋯", |ui| {
                    if ui.button("Save As New Preset…").clicked() {
                        d.save = Some(SavePreset::new());
                        ui.close();
                    }
                    if ui
                        .add_enabled(user, egui::Button::new("Delete Preset"))
                        .clicked()
                    {
                        self.send("presets.delete", json!({ "name": d.preset }));
                        self.refresh_presets(&ctx);
                        ui.close();
                    }
                });
            });
        });
        ui.separator();
        let ready = self.plan_ready(&d);
        let busy = self.export.busy();
        let mut add = false;
        egui::Panel::bottom("render-settings-footer")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if let Some(e) = &d.error {
                    super::dialog::warn(ui, e.clone());
                }
                super::dialog::footer(ui, |ui| {
                    add = super::dialog::main_button(ui, "Add to Render Queue", ready);
                    match &d.plan_result {
                        _ if ready || busy => {}
                        Some(Err(_)) => super::dialog::why(ui, "These settings can't render"),
                        _ => super::dialog::why(ui, "Checking the settings…"),
                    }
                });
            });
        let project = self.snap.project.as_ref().map(|p| p.path.clone());
        let size = self
            .project()
            .map_or((1920, 1080), |p| (p.width(), p.height()));
        let audio = self.project().map_or((48_000, 2), |p| {
            (p.settings.sample_rate, p.settings.channels)
        });
        let tracks: Vec<(String, String, bool)> = self.project().map_or_else(Vec::new, |p| {
            p.subtitles
                .iter()
                .map(|t| (t.id.clone(), t.display_name(), t.shown))
                .collect()
        });
        let can = self.export.encodable.clone();
        let mut browse = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.save_preset_form(ui, &ctx, &mut d);
                super::dialog::form(ui, "render-settings-form", |ui| {
                    super::dialog::row(ui, "Preset", |ui| {
                        let shown = match d.changed() {
                            true => format!("{title} (changed)"),
                            false => title.clone(),
                        };
                        egui::ComboBox::from_id_salt("export-preset")
                            .selected_text(shown)
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for p in &presets {
                                    let mark = match (p.unavailable.is_some(), p.user) {
                                        (true, _) => " (not available here)",
                                        (false, true) => " (yours)",
                                        (false, false) => "",
                                    };
                                    let r = ui
                                        .add_enabled_ui(p.unavailable.is_none(), |ui| {
                                            ui.selectable_label(
                                                p.name == d.preset,
                                                format!("{}{mark}", p.title),
                                            )
                                        })
                                        .inner;
                                    if r.clicked() {
                                        d.use_preset(p);
                                    }
                                }
                            });
                    });
                    super::dialog::row(ui, "File Name", |ui| {
                        let field = match d.per_clip {
                            true => &mut d.template,
                            false => &mut d.name,
                        };
                        ui.add(egui::TextEdit::singleline(field).desired_width(220.0));
                    });
                    if d.per_clip {
                        super::dialog::row(ui, "", |ui| {
                            super::dialog::why(ui, "{index}, {index:03}, {name}, {clip}, {media}")
                        });
                    }
                    super::dialog::row(ui, "Location", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.folder).desired_width(220.0));
                        if ui
                            .add_enabled(d.picking.is_none(), egui::Button::new("Browse"))
                            .clicked()
                        {
                            browse = true;
                        }
                    });
                    super::dialog::row(ui, "Render", |ui| {
                        super::dialog::switch(ui, |ui| {
                            ui.selectable_value(&mut d.per_clip, false, "Single Clip");
                            ui.selectable_value(&mut d.per_clip, true, "Individual Clips");
                        })
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    super::dialog::switch(ui, |ui| {
                        ui.selectable_value(&mut d.tab, Tab::Video, "Video");
                        ui.selectable_value(&mut d.tab, Tab::Audio, "Audio");
                        ui.selectable_value(&mut d.tab, Tab::File, "File");
                    })
                });
                ui.add_space(6.0);
                match d.tab {
                    Tab::Video => {
                        video_tab(ui, &mut d, &can, size);
                        subtitle_settings(ui, &mut d, &tracks);
                    }
                    Tab::Audio => audio_tab(ui, &mut d, &can, audio),
                    Tab::File => file_tab(ui, &mut d),
                }
                ui.separator();
                plan_and_command(ui, &mut d, project.as_deref());
            });
        if browse {
            let (dialogs, folder) = (self.dialogs.clone(), self.export_folder());
            d.picking = Some(Picked::ask(&ctx, move || {
                dialogs.pick_folder("Render Location", &folder)
            }));
        }
        self.refresh_plan(&ctx, &mut d);
        if add {
            self.start_render(&ctx, &mut d);
        }
        self.export.render = Some(d);
    }

    fn render_queue_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Render Queue").strong());
        ui.separator();
        let jobs: Vec<JobInfo> = self
            .snap
            .jobs
            .iter()
            .filter(|j| is_render(j))
            .cloned()
            .collect();
        if jobs.is_empty() {
            ui.centered_and_justified(|ui| ui.label(dim("No jobs in queue")));
            return;
        }
        let project = self.project_stem();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, j) in jobs.iter().enumerate() {
                    ui.group(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(RichText::new(format!("Job {}", i + 1)).strong());
                        ui.label(dim(&project));
                        if let Some(path) = j.params["output"].as_str() {
                            ui.add(egui::Label::new(dim(path)).truncate());
                        }
                        export::job_view(ui, j, |method, params| self.send(method, params));
                    });
                }
            });
    }
}

fn is_render(j: &JobInfo) -> bool {
    j.kind == "export"
        && j.params["output"]
            .as_str()
            .is_some_and(|o| crate::export::timeline_format(&PathBuf::from(o)).is_none())
}
