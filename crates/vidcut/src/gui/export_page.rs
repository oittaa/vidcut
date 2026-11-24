use eframe::egui::{self, RichText};
use serde_json::json;
use vidcut_core::model::{RenderJob, RenderRange, RenderStatus};
use vidcut_core::request::ViewerMode;
use vidcut_core::time::{RangeEnd, TimeSpec};

use super::export::{
    self, Ask, Picked, SavePreset, Tab, audio_tab, file_tab, plan_and_command, subtitle_settings,
    video_tab,
};
use super::theme::{self, Deco};
use super::workspace::{BOTTOM_SHARE, Workspace, dim, escape_cancels};
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

    pub(super) fn export_mark(&self, is_in: bool) {
        let frame = self.snap.timeline.position;
        let (key, value) = match (is_in, self.clip_under(frame)) {
            (true, Some((a, _))) => ("in", a),
            (false, Some((_, b))) => ("out", b),
            (true, None) => ("in", frame),
            (false, None) => ("out", frame + 1),
        };
        self.mark_in_out(key, value);
    }

    fn clip_under(&self, frame: i64) -> Option<(i64, i64)> {
        if !self.export.render.as_ref().is_some_and(|d| d.per_clip) {
            return None;
        }
        let p = self.project()?;
        let mut under = p
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| !c.is_block() && c.start <= frame && frame < c.end());
        let c = under.next()?;
        under.next().is_none().then(|| (c.start, c.end()))
    }

    pub(super) fn mark_in_out(&self, key: &str, value: i64) {
        if !self.render_whole() {
            self.send("marks.set", json!({ key: value }));
            return;
        }
        let commands = json!([
            { "method": "marks.set", "params": { key: value } },
            { "method": "timeline.set", "params": { "render": RenderRange::InOut } },
        ]);
        self.send("apply", json!({ "commands": commands }));
    }

    pub(super) fn export_clear_marks(&self) {
        let Some(p) = self.project() else { return };
        if self.render_whole() && p.marks.mark_in.is_none() && p.marks.mark_out.is_none() {
            return;
        }
        self.send("timeline.set", json!({ "render": RenderRange::Entire }));
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
        if self.queue_rendering() {
            ui.disable();
        }
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
                    super::dialog::row(ui, "File Name", |ui| match (d.per_clip, d.source_names) {
                        (true, true) => {
                            ui.weak("Source Name");
                        }
                        (true, false) => {
                            let edit = egui::TextEdit::singleline(&mut d.template);
                            ui.add(edit.desired_width(220.0));
                        }
                        (false, _) => {
                            let edit = egui::TextEdit::singleline(&mut d.name);
                            ui.add(edit.desired_width(220.0));
                        }
                    });
                    if d.per_clip && !d.source_names {
                        super::dialog::row(ui, "", |ui| {
                            super::dialog::why(
                                ui,
                                "{source}, {index}, {index:03}, {name}, {clip}, {media}",
                            )
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

    pub(super) fn select_queue_jobs(&mut self, jobs: Vec<String>) -> Result<(), String> {
        let queue = self.project().map(|p| p.queue.clone()).unwrap_or_default();
        if let Some(j) = jobs.iter().find(|j| !queue.iter().any(|q| &q.id == *j)) {
            return Err(format!("no render job {j}"));
        }
        self.export.selected = jobs;
        Ok(())
    }

    pub(super) fn queue_rendering(&self) -> bool {
        self.snap
            .jobs
            .iter()
            .any(|j| j.render.is_some() && j.unfinished())
    }

    fn render_queue_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let queue = self.project().map(|p| p.queue.clone()).unwrap_or_default();
        self.export
            .selected
            .retain(|id| queue.iter().any(|j| &j.id == id));
        let rendering = self.queue_rendering();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Render Queue").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("⋯", |ui| {
                    ui.checkbox(&mut self.export.job_details, "Show Job Details");
                    ui.separator();
                    if ui.button("Clear Rendered").clicked() {
                        self.send("render.clear", json!({}));
                        ui.close();
                    }
                    if ui
                        .add_enabled(!rendering, egui::Button::new("Clear All"))
                        .clicked()
                    {
                        self.send("render.clear", json!({ "all": true }));
                        ui.close();
                    }
                });
            });
        });
        ui.separator();
        let mut render = false;
        let mut stop = false;
        egui::Panel::bottom("render-queue-footer")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if let Some(QueueStep::Error(e)) = &self.export.queue {
                    super::dialog::warn(ui, e.clone());
                }
                match self.queue_progress() {
                    Some((k, n, share)) => {
                        ui.label(dim("Rendering in Progress"));
                        ui.label(dim(format!("{k} of {n}")));
                        ui.add(egui::ProgressBar::new(share).show_percentage());
                        super::dialog::footer(ui, |ui| stop = ui.button("Stop").clicked());
                    }
                    None => super::dialog::footer(ui, |ui| {
                        let n = self.export.selected.len();
                        let label = match n == 0 || n == queue.len() {
                            true => "Render All".to_string(),
                            false => format!("Render {n}"),
                        };
                        let busy = matches!(self.export.queue, Some(QueueStep::Checking { .. }));
                        render = super::dialog::main_button(
                            ui,
                            &label,
                            !queue.is_empty() && !rendering && !busy,
                        );
                    }),
                }
            });
        if queue.is_empty() {
            ui.centered_and_justified(|ui| ui.label(dim("No jobs in queue")));
        } else {
            self.queue_rows(ui, &queue);
        }
        if stop {
            for j in self.snap.jobs.iter().filter(|j| j.render.is_some()) {
                if j.unfinished() {
                    self.send("jobs.cancel", json!({ "job": j.id }));
                }
            }
        }
        if render {
            let jobs = match self.export.selected.is_empty() {
                true => None,
                false => Some(self.export.selected.clone()),
            };
            let completed = queue.iter().any(|j| {
                jobs.as_ref().is_none_or(|ids| ids.contains(&j.id))
                    && matches!(j.status, Some(RenderStatus::Completed { .. }))
            });
            self.export.queue = match completed {
                true => Some(QueueStep::ReRender { jobs }),
                false => Some(self.check_queue(&ctx, jobs, None)),
            };
        }
        self.queue_prompts(&ctx);
    }

    fn queue_progress(&self) -> Option<(usize, usize, f32)> {
        let share = |j: &JobInfo| j.progress.clamp(0.0, 1.0) as f32;
        if let Some(QueueStep::Rendering { started }) = &self.export.queue {
            let jobs: Vec<&JobInfo> = (started.iter())
                .filter_map(|id| self.snap.jobs.iter().find(|j| &j.id == id))
                .collect();
            if let Some(at) = jobs.iter().position(|j| j.unfinished()) {
                return Some((at + 1, jobs.len(), share(jobs[at])));
            }
        }
        let left: Vec<&JobInfo> = (self.snap.jobs.iter())
            .filter(|j| j.render.is_some() && j.unfinished())
            .collect();
        let first = left.first()?;
        Some((1, left.len(), share(first)))
    }

    fn queue_rows(&mut self, ui: &mut egui::Ui, queue: &[RenderJob]) {
        let project = self.project_stem();
        let names: Vec<(String, String)> = self.project().map_or_else(Vec::new, |p| {
            (p.timelines.iter())
                .map(|t| (t.id.clone(), t.name.clone()))
                .collect()
        });
        let colors = theme::colors();
        let mut picked: Option<(usize, egui::Modifiers)> = None;
        let mut moved: Option<(String, usize)> = None;
        let mut menu: Option<(String, QueueAction)> = None;
        let mut renamed: Option<(String, String)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, j) in queue.iter().enumerate() {
                    let selected = self.export.selected.contains(&j.id);
                    let fill = match selected {
                        true => colors.selection,
                        false => egui::Color32::TRANSPARENT,
                    };
                    let live = (self.snap.jobs.iter())
                        .rev()
                        .find(|k| k.render.as_deref() == Some(j.id.as_str()) && k.unfinished())
                        .cloned();
                    let place = egui::UiBuilder::new()
                        .id_salt(("render-job", &j.id))
                        .sense(egui::Sense::click());
                    let row = ui.scope_builder(place, |ui| {
                        egui::Frame::group(ui.style()).fill(fill).show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let handle = ui.add(
                                    egui::Label::new(dim("⠿"))
                                        .sense(egui::Sense::drag())
                                        .selectable(false),
                                );
                                handle.dnd_set_drag_payload(j.id.clone());
                                let name =
                                    j.name.clone().unwrap_or_else(|| format!("Job {}", i + 1));
                                match &mut self.export.renaming {
                                    Some(edit) if edit.job == j.id => {
                                        let field = ui.add(
                                            egui::TextEdit::singleline(&mut edit.text)
                                                .desired_width(160.0),
                                        );
                                        if edit.fresh {
                                            field.request_focus();
                                            edit.fresh = false;
                                        }
                                        if field.lost_focus() {
                                            let keep =
                                                !ui.input(|x| x.key_pressed(egui::Key::Escape));
                                            let text = edit.text.trim().to_string();
                                            if keep && !text.is_empty() && text != name {
                                                renamed = Some((j.id.clone(), text));
                                            }
                                            self.export.renaming = None;
                                        }
                                    }
                                    _ => {
                                        let label = ui.add(
                                            egui::Label::new(RichText::new(&name).strong())
                                                .sense(egui::Sense::click())
                                                .selectable(false),
                                        );
                                        if label.double_clicked() {
                                            self.export.renaming = Some(Renaming {
                                                job: j.id.clone(),
                                                text: name.clone(),
                                                fresh: true,
                                            });
                                        } else if label.clicked() {
                                            picked = Some((i, ui.input(|x| x.modifiers)));
                                        }
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add_enabled(live.is_none(), egui::Button::new("✕"))
                                            .clicked()
                                        {
                                            menu = Some((j.id.clone(), QueueAction::Remove));
                                        }
                                    },
                                );
                            });
                            let timeline = (names.iter())
                                .find(|(id, _)| id == &j.timeline)
                                .map_or(j.timeline.as_str(), |(_, n)| n.as_str());
                            ui.add(
                                egui::Label::new(dim(format!("{project} | {timeline}"))).truncate(),
                            );
                            ui.add(
                                egui::Label::new(dim(j.export.output.display().to_string()))
                                    .truncate(),
                            );
                            if self.export.job_details {
                                for line in self.job_details(j) {
                                    ui.label(dim(line));
                                }
                            }
                            match (&live, &j.status) {
                                (Some(k), _) => export::job_view(ui, k, |method, params| {
                                    self.send(method, params)
                                }),
                                (None, Some(RenderStatus::Completed { seconds })) => {
                                    ui.label(
                                        RichText::new(format!("Completed in {}", hms(*seconds)))
                                            .color(colors.ok),
                                    );
                                }
                                (None, Some(RenderStatus::Cancelled)) => {
                                    ui.label(RichText::new("Job Cancelled").color(colors.error));
                                }
                                (None, Some(RenderStatus::Failed { error })) => {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("Failed: {error}"))
                                                .color(colors.error),
                                        )
                                        .wrap(),
                                    );
                                }
                                (None, None) => {}
                            }
                        });
                    });
                    let r = row.response;
                    if r.clicked() {
                        picked = Some((i, ui.input(|x| x.modifiers)));
                    }
                    if let Some(from) = r.dnd_release_payload::<String>() {
                        moved = Some(((*from).clone(), i + 1));
                    }
                    r.context_menu(|ui| {
                        if ui.button("Open File Location").clicked() {
                            menu = Some((j.id.clone(), QueueAction::Show));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Clear Render Status").clicked() {
                            menu = Some((j.id.clone(), QueueAction::ClearStatus));
                            ui.close();
                        }
                    });
                }
                let rest = ui.allocate_response(ui.available_size(), egui::Sense::click());
                if rest.clicked() {
                    self.export.selected.clear();
                }
            });
        if let Some((i, keys)) = picked {
            self.pick_queue_job(queue, i, keys);
        }
        if let Some((job, to)) = moved {
            self.send("render.move", json!({ "job": job, "to": to }));
        }
        if let Some((job, name)) = renamed {
            self.send("render.rename", json!({ "job": job, "name": name }));
        }
        match menu {
            Some((job, QueueAction::Remove)) => {
                self.send("render.remove", json!({ "jobs": [job] }))
            }
            Some((job, QueueAction::Show)) => {
                if let Some(j) = queue.iter().find(|j| j.id == job) {
                    super::file_manager::show(&j.export.output);
                }
            }
            Some((job, QueueAction::ClearStatus)) => {
                let jobs = match self.export.selected.contains(&job) {
                    true => self.export.selected.clone(),
                    false => vec![job],
                };
                self.send("render.clear_status", json!({ "jobs": jobs }));
            }
            None => {}
        }
    }

    fn pick_queue_job(&mut self, queue: &[RenderJob], i: usize, keys: egui::Modifiers) {
        let id = queue[i].id.clone();
        let sel = &mut self.export.selected;
        if keys.command {
            match sel.iter().position(|s| s == &id) {
                Some(at) => {
                    sel.remove(at);
                }
                None => sel.push(id),
            }
        } else if keys.shift
            && let Some(last) = sel.last()
            && let Some(from) = queue.iter().position(|j| &j.id == last)
        {
            let (a, b) = (from.min(i), from.max(i));
            for j in &queue[a..=b] {
                if !sel.contains(&j.id) {
                    sel.push(j.id.clone());
                }
            }
        } else {
            *sel = vec![id];
        }
    }

    fn job_details(&self, j: &RenderJob) -> Vec<String> {
        let Some(p) = self.project() else {
            return Vec::new();
        };
        let name = (j.export.preset.as_deref()).unwrap_or(crate::export::usual_preset(&j.export));
        let preset = (self.export.presets.iter())
            .find(|p| p.name == name)
            .map(|p| p.settings.clone());
        let Some(mut settings) = j.export.settings.clone().or(preset) else {
            return Vec::new();
        };
        if vidcut_core::export::apply_overrides(&mut settings, &j.export.set).is_err() {
            return Vec::new();
        }
        if j.export.audio_only {
            settings.video = None;
        }
        let t = p.timelines.iter().find(|t| t.id == j.timeline);
        let f = t
            .and_then(|t| t.own)
            .unwrap_or_else(|| vidcut_core::model::TimelineSettings::of(&p.settings));
        let video = match &settings.video {
            Some(v) => format!(
                "{}x{}, {}, {}",
                v.width.unwrap_or(f.width),
                v.height.unwrap_or(f.height),
                v.codec.title(),
                fps_text(f.fps.num(), f.fps.den())
            ),
            None => "No video".to_string(),
        };
        let length = match &j.export.range {
            Some(r) => match (&r.start, &r.end) {
                (TimeSpec::Frames(a), RangeEnd::At(TimeSpec::Frames(b))) => b - a,
                _ => 0,
            },
            None => 0,
        };
        let audio = match &settings.audio {
            Some(a) => format!(
                "Audio {}, {} {} Hz",
                a.channels.unwrap_or(p.settings.channels),
                export::name(&a.codec),
                a.sample_rate.unwrap_or(p.settings.sample_rate)
            ),
            None => "Audio 0".to_string(),
        };
        vec![
            video,
            format!(
                "{audio}, {}",
                vidcut_core::time::format_timecode(length, f.fps, 0, f.counts_drop_frame())
            ),
        ]
    }

    fn check_queue(
        &self,
        ctx: &egui::Context,
        jobs: Option<Vec<String>>,
        again: Option<bool>,
    ) -> QueueStep {
        let params = json!({ "jobs": jobs, "again": again });
        let ask = Ask::send_dry(&self.live, ctx, "render.start", params);
        QueueStep::Checking { jobs, again, ask }
    }

    fn start_queue(
        &self,
        ctx: &egui::Context,
        jobs: Option<Vec<String>>,
        again: Option<bool>,
        replace: bool,
    ) -> QueueStep {
        let params = json!({ "jobs": jobs, "again": again, "replace": replace });
        QueueStep::Starting(Ask::send(&self.live, ctx, "render.start", params))
    }

    fn queue_prompts(&mut self, ctx: &egui::Context) {
        let Some(step) = self.export.queue.take() else {
            return;
        };
        self.export.queue = match step {
            QueueStep::ReRender { jobs } => match ask_rerender(ctx) {
                Some(again) => Some(self.check_queue(ctx, jobs, Some(again))),
                None if escaped(ctx) => {
                    focus_reset(ctx, "render-again-focus");
                    None
                }
                None => Some(QueueStep::ReRender { jobs }),
            },
            QueueStep::Checking { jobs, again, ask } => match ask.take() {
                None => Some(QueueStep::Checking { jobs, again, ask }),
                Some(Err(e)) => Some(QueueStep::Error(e.message)),
                Some(Ok(r)) => {
                    let existing: Vec<String> = (r.result["existing"].as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    match existing.is_empty() {
                        true => Some(self.start_queue(ctx, jobs, again, false)),
                        false => Some(QueueStep::Replace {
                            jobs,
                            again,
                            existing,
                        }),
                    }
                }
            },
            QueueStep::Replace {
                jobs,
                again,
                existing,
            } => match ask_replace(ctx, &existing) {
                Some(true) => Some(self.start_queue(ctx, jobs, again, true)),
                Some(false) => None,
                None => Some(QueueStep::Replace {
                    jobs,
                    again,
                    existing,
                }),
            },
            QueueStep::Starting(ask) => match ask.take() {
                None => Some(QueueStep::Starting(ask)),
                Some(Err(e)) => Some(QueueStep::Error(e.message)),
                Some(Ok(r)) => Some(QueueStep::Rendering {
                    started: (r.result["started"].as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s["id"].as_str().map(String::from))
                        .collect(),
                }),
            },
            QueueStep::Rendering { started } => Some(QueueStep::Rendering { started }),
            QueueStep::Error(e) => Some(QueueStep::Error(e)),
        };
    }
}

pub(super) struct Renaming {
    job: String,
    text: String,
    fresh: bool,
}

pub(super) enum QueueStep {
    ReRender {
        jobs: Option<Vec<String>>,
    },
    Checking {
        jobs: Option<Vec<String>>,
        again: Option<bool>,
        ask: Ask,
    },
    Replace {
        jobs: Option<Vec<String>>,
        again: Option<bool>,
        existing: Vec<String>,
    },
    Starting(Ask),
    Rendering {
        started: Vec<String>,
    },
    Error(String),
}

enum QueueAction {
    Remove,
    Show,
    ClearStatus,
}

fn fps_text(num: i64, den: i64) -> String {
    match den {
        1 => num.to_string(),
        _ => {
            let text = format!("{:.3}", num as f64 / den as f64);
            text.trim_end_matches('0').trim_end_matches('.').to_string()
        }
    }
}

fn hms(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn escaped(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

fn ask_rerender(ctx: &egui::Context) -> Option<bool> {
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("render-again")).show(ctx, |ui| {
        ui.set_width(super::dialog::WIDTH);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.heading("Re-Render Jobs?");
        ui.label("These jobs were rendered already. Render them again?");
        super::dialog::footer(ui, |ui| {
            let render = super::dialog::main_button_ui(ui, "Render", true);
            focus_once(ui.ctx(), "render-again-focus", &render);
            if render.clicked() {
                answer = Some(true);
            }
            if ui.button("Don't Render").clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_some() {
        focus_reset(ctx, "render-again-focus");
    }
    answer
}

pub(super) enum AddChoice {
    Replace,
    Rename,
    Cancel,
}

pub(super) fn ask_add_replace(
    ctx: &egui::Context,
    existing: &[String],
    rename: bool,
) -> Option<AddChoice> {
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique("render-add-replace")).show(ctx, |ui| {
        ui.set_width(super::dialog::WIDTH);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.heading("Replace Existing File?");
        ui.label("This render replaces a file with the same name.");
        for f in existing.iter().take(3) {
            ui.add(egui::Label::new(dim(f)).truncate());
        }
        super::dialog::footer(ui, |ui| {
            if super::dialog::main_button(ui, "Replace", true) {
                answer = Some(AddChoice::Replace);
            }
            if rename && ui.button("Rename").clicked() {
                answer = Some(AddChoice::Rename);
            }
            let cancel = ui.button("Cancel");
            focus_once(ui.ctx(), "render-add-focus", &cancel);
            if cancel.clicked() {
                answer = Some(AddChoice::Cancel);
            }
        });
    });
    if escape_cancels(ctx, &modal) {
        answer = Some(AddChoice::Cancel);
    }
    if answer.is_some() {
        focus_reset(ctx, "render-add-focus");
    }
    answer
}

fn focus_once(ctx: &egui::Context, key: &str, button: &egui::Response) {
    let id = egui::Id::unique(key);
    if !ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        button.request_focus();
        ctx.data_mut(|d| d.insert_temp(id, true));
    }
}

fn focus_reset(ctx: &egui::Context, key: &str) {
    ctx.data_mut(|d| d.remove::<bool>(egui::Id::unique(key)));
}

fn ask_replace(ctx: &egui::Context, existing: &[String]) -> Option<bool> {
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique("render-replace")).show(ctx, |ui| {
        ui.set_width(super::dialog::WIDTH);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.heading("Replace Existing File?");
        ui.label("This render replaces a file with the same name.");
        for f in existing.iter().take(3) {
            ui.add(egui::Label::new(dim(f)).truncate());
        }
        super::dialog::footer(ui, |ui| {
            if super::dialog::main_button(ui, "Replace", true) {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    if escape_cancels(ctx, &modal) {
        answer = Some(false);
    }
    answer
}
