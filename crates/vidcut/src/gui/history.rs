use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, Modifiers, Rect, RichText, Sense, Stroke,
    StrokeKind, pos2, vec2,
};
use egui_phosphor::regular as icon;
use serde_json::json;
use vidcut_core::model::Project;
use vidcut_core::request::{Client, ClientKind};
use vidcut_core::steps::{Group, Span, Step, Steps, groups};

use super::theme;
use super::workspace::Workspace;
use crate::when;

const ROW: f32 = 24.0;
const WIDTH: f32 = 420.0;
pub(super) const MIN_WIDTH: f32 = 300.0;
const MENU: usize = 20;
const FLASH: f64 = 1.6;

#[derive(Default)]
pub(super) struct HistoryUi {
    pub open: bool,
    pub keys: bool,
    pub width: f32,
    hovered: Option<usize>,
    was: Option<usize>,
    cursor: Option<usize>,
    target: Option<usize>,
    sent: Option<(Option<usize>, u64)>,
    expanded: HashSet<usize>,
    parts: HashSet<usize>,
    flash: Option<(u64, f64)>,
    follow: bool,
    seen: Option<(usize, usize)>,
}

pub(super) struct Marks {
    pub clips: HashSet<String>,
    pub spans: Vec<Span>,
    pub strength: f32,
    pub focus: bool,
}

#[derive(Clone, Copy)]
enum Row {
    Original,
    Opened(u64),
    Group(usize, bool),
    Step(usize, bool),
    Part(usize, usize),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    width: f32,
}

fn kept_path() -> std::path::PathBuf {
    crate::paths::config_dir().join("history.json")
}

impl HistoryUi {
    pub(super) fn load() -> Self {
        let width = std::fs::read_to_string(kept_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Kept>(&t).ok())
            .map_or(WIDTH, |k| k.width.clamp(MIN_WIDTH, 900.0));
        Self {
            width,
            ..Self::default()
        }
    }

    fn save(&self) {
        if let Some(dir) = kept_path().parent()
            && std::fs::create_dir_all(dir).is_ok()
        {
            let _ = std::fs::write(kept_path(), json!(Kept { width: self.width }).to_string());
        }
    }

    pub(super) fn show(&mut self) {
        self.open = true;
        self.keys = true;
        self.follow = true;
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.keys = false;
        self.cursor = None;
    }
}

fn kind_color(kind: ClientKind) -> Color32 {
    match kind {
        ClientKind::Gui => theme::colors().accent,
        ClientKind::Cli => Color32::from_rgb(64, 160, 150),
        ClientKind::Mcp => Color32::from_rgb(150, 110, 225),
        ClientKind::Ipc => Color32::from_rgb(80, 135, 215),
        ClientKind::File => Color32::from_rgb(210, 135, 55),
    }
}

fn who(c: &Client) -> String {
    match (&c.name, c.kind) {
        (Some(n), ClientKind::Gui) => n.clone(),
        (Some(n), kind) => format!("{n} ({})", kind.name()),
        (None, ClientKind::File) => "another program, writing the project file".into(),
        (None, kind) => format!("the {}", kind.name()),
    }
}

fn place(p: &Project, spans: &[Span]) -> Option<String> {
    let s = spans.first()?;
    let at = p.settings.timecode(s.from);
    let more = if spans.len() > 1 {
        format!(" and {} more", spans.len() - 1)
    } else {
        String::new()
    };
    Some(match &s.track {
        Some(t) => format!("{t} at {at}{more}"),
        None => format!("markers at {at}{more}"),
    })
}

fn one_line(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font: FontId,
    color: Color32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

fn badge(ui: &egui::Ui, at: egui::Pos2, c: &Client) -> f32 {
    let font = FontId::proportional(11.0);
    let text = one_line(ui, &c.shown(), 110.0, font, Color32::WHITE);
    let rect = Rect::from_min_size(
        pos2(at.x, at.y - text.size().y / 2.0 - 2.0),
        vec2(text.size().x + 10.0, text.size().y + 4.0),
    );
    ui.painter()
        .rect_filled(rect, CornerRadius::same(4), kind_color(c.kind));
    ui.painter().galley(
        pos2(rect.left() + 5.0, rect.top() + 2.0),
        text,
        Color32::WHITE,
    );
    rect.width() + 6.0
}

impl Workspace {
    fn steps(&self) -> Option<Arc<Steps>> {
        self.snap.project.as_ref().map(|p| p.history.clone())
    }

    pub(super) fn history_go(&mut self, step: usize) {
        self.history.cursor = None;
        if self.steps().is_some_and(|s| s.done != step) {
            self.send("history.go", json!({ "step": step }));
        }
    }

    pub(super) fn history_keys(&mut self, ctx: &egui::Context) {
        let h = &mut self.history;
        if !h.open || !h.keys {
            return;
        }
        let Some(steps) = self.snap.project.as_ref().map(|p| p.history.clone()) else {
            return;
        };
        let consume = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        let at = h.cursor.unwrap_or(steps.done);
        if consume(Key::ArrowUp) {
            h.cursor = Some(at.saturating_sub(1));
            h.follow = true;
        }
        if consume(Key::ArrowDown) {
            h.cursor = Some((at + 1).min(steps.line.len()));
            h.follow = true;
        }
        if consume(Key::Enter) {
            if let Some(c) = h.cursor.take() {
                self.history_go(c);
            }
            return;
        }
        if consume(Key::Escape) && h.cursor.take().is_none() {
            h.close();
        }
    }

    pub(super) fn history_menu(&mut self, ui: &mut egui::Ui) {
        let Some(steps) = self.steps() else {
            return;
        };
        let p = self.snap.project.as_ref().map(|p| p.project.clone());
        let n = steps.line.len();
        let mut start = n.saturating_sub(MENU);
        if steps.done < start {
            start = steps.done.saturating_sub(MENU / 2);
        }
        let end = (start + MENU).min(n);
        let now = when::now_ms();
        let mut go = None;
        for i in (start..end).rev() {
            let s = &steps.line[i];
            let done = i < steps.done;
            let current = i + 1 == steps.done;
            let mut text = RichText::new(format!(
                "{}  {}",
                if current { icon::CHECK } else { "   " },
                truncate(&s.label, 64)
            ));
            if !done {
                text = text.color(theme::colors().text_dim);
            }
            let mut b = egui::Button::new(text).selected(current);
            if let Some(t) = s.time {
                b = b.shortcut_text(when::ago(t, now));
            }
            let r = ui.add(b);
            if r.hovered() {
                self.history.hovered = Some(i + 1);
            }
            let r = r.on_hover_text(step_tip(s, i + 1, done, p.as_deref()));
            if r.clicked() {
                go = Some(i + 1);
            }
        }
        if start == 0 {
            let current = steps.done == 0;
            let r = ui.add(
                egui::Button::new(format!(
                    "{}  Original",
                    if current { icon::CHECK } else { "   " }
                ))
                .selected(current),
            );
            if r.hovered() {
                self.history.hovered = Some(0);
            }
            if r.clicked() {
                go = Some(0);
            }
        }
        if n == 0 {
            ui.label(RichText::new("Nothing to undo yet").color(theme::colors().text_dim));
        }
        ui.separator();
        if ui.button("Open History Window").clicked() {
            self.history.show();
        }
        if let Some(step) = go {
            self.history_go(step);
        }
    }

    pub(super) fn history_frame(&mut self) {
        self.history.was = self.history.hovered.take();
        if !self.history.open {
            self.history.cursor = None;
        }
    }

    pub(super) fn history_panel(&mut self, ui: &mut egui::Ui) {
        let Some(steps) = self.steps() else {
            return;
        };
        let p = self.snap.project.as_ref().map(|p| p.project.clone());
        if self.history.seen != Some((steps.done, steps.line.len())) {
            self.history.seen = Some((steps.done, steps.line.len()));
            self.history.follow = true;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("History").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = ui
                    .add(egui::Button::new(icon::X).frame(false))
                    .on_hover_text("Close the history");
                close.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close the history")
                });
                if close.clicked() {
                    self.history.close();
                }
            });
        });
        self.history_list(ui, &steps, p.as_deref());
        let inside = ui.min_rect();
        let clicked = ui.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        if let Some(pos) = clicked {
            self.history.keys = inside.contains(pos);
        }
        let width = ui.min_rect().width();
        if (width - self.history.width).abs() > 1.0 && !ui.input(|i| i.pointer.any_down()) {
            self.history.width = width;
            self.history.save();
        }
    }

    fn history_list(&mut self, ui: &mut egui::Ui, steps: &Steps, p: Option<&Project>) {
        let n = steps.line.len();
        ui.horizontal(|ui| {
            let undo = ui
                .add_enabled(
                    steps.done > 0,
                    egui::Button::new(format!("{} Undo", icon::ARROW_COUNTER_CLOCKWISE)),
                )
                .on_hover_text(super::keys("Ctrl+Z"));
            if undo.clicked() {
                self.send("history.undo", json!({}));
            }
            let redo = ui
                .add_enabled(
                    steps.done < n,
                    egui::Button::new(format!("{} Redo", icon::ARROW_CLOCKWISE)),
                )
                .on_hover_text(super::keys("Ctrl+Shift+Z"));
            if redo.clicked() {
                self.send("history.redo", json!({}));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = match n {
                    0 => "No changes yet".to_string(),
                    _ if steps.done == n => format!("{n} steps"),
                    _ => format!("At step {} of {n}", steps.done),
                };
                ui.label(RichText::new(text).color(theme::colors().text_dim));
            });
        });
        ui.separator();
        let all = groups(&steps.line);
        let rows = self.history_rows(steps, &all);
        let mark = self.history.cursor.unwrap_or(steps.done);
        let mark_row = rows.iter().position(|r| match *r {
            Row::Original => mark == 0,
            Row::Step(i, _) => i + 1 == mark,
            Row::Group(g, false) => all[g].to == mark,
            _ => false,
        });
        let hint = 52.0;
        let view = (ui.available_height() - hint).max(ROW * 3.0);
        let spacing = ui.spacing().item_spacing.y;
        let mut area = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(view);
        if self.history.follow
            && let Some(at) = mark_row
        {
            self.history.follow = false;
            let y = at as f32 * (ROW + spacing) - view / 2.0 + ROW;
            area = area.vertical_scroll_offset(y.max(0.0));
        }
        let now = when::now_ms();
        let mut go = None;
        let mut toggle_group = None;
        let mut toggle_parts = None;
        area.show_rows(ui, ROW, rows.len(), |ui, range| {
            for k in range {
                let row = rows[k];
                let (rect, r) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                let state = match row {
                    Row::Original => Some(0),
                    Row::Step(i, _) => Some(i + 1),
                    Row::Group(g, _) => Some(all[g].to),
                    Row::Opened(_) | Row::Part(..) => None,
                };
                let current = state == Some(steps.done) && !matches!(row, Row::Group(_, true));
                let cursor = self.history.cursor.is_some() && state == self.history.cursor;
                let named = match row {
                    Row::Original => "Original".to_string(),
                    Row::Opened(t) => format!("Opened {}", when::stamp(t, now)),
                    Row::Group(g, _) => {
                        format!("{} steps by {}", all[g].to - all[g].from, all[g].by.shown())
                    }
                    Row::Step(i, _) => steps.line[i].label.clone(),
                    Row::Part(i, j) => steps.line[i].parts.get(j).cloned().unwrap_or_default(),
                };
                r.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::Button, true, current, &named)
                });
                let c = theme::colors();
                if current {
                    ui.painter()
                        .rect_filled(rect, 3.0, c.selection.gamma_multiply(0.55));
                } else if r.hovered() && state.is_some() {
                    ui.painter()
                        .rect_filled(rect, 3.0, c.text.gamma_multiply(0.07));
                }
                if cursor {
                    ui.painter().rect_stroke(
                        rect,
                        3.0,
                        Stroke::new(1.5, c.accent),
                        StrokeKind::Inside,
                    );
                }
                let mid = rect.center().y;
                let mut x = rect.left() + 4.0;
                let depth = match row {
                    Row::Step(_, true) | Row::Part(..) => 1.0,
                    _ => 0.0,
                };
                x += depth * 18.0;
                if current {
                    ui.painter()
                        .circle_filled(pos2(x + 5.0, mid), 3.5, c.accent);
                }
                x += 14.0;
                let font = FontId::proportional(13.0);
                let right = rect.right() - 6.0;
                match row {
                    Row::Original => {
                        let g = one_line(ui, "Original", right - x, font, c.text);
                        ui.painter()
                            .galley(pos2(x, mid - g.size().y / 2.0), g, c.text);
                    }
                    Row::Opened(t) => {
                        let text = format!("Opened {}", when::stamp(t, now));
                        let g =
                            one_line(ui, &text, right - x, FontId::proportional(11.0), c.text_dim);
                        let w = g.size().x;
                        ui.painter()
                            .galley(pos2(x, mid - g.size().y / 2.0), g, c.text_dim);
                        ui.painter().hline(
                            (x + w + 8.0)..=right,
                            mid,
                            Stroke::new(1.0, c.text_dim.gamma_multiply(0.35)),
                        );
                    }
                    Row::Part(i, j) => {
                        let text = steps.line[i].parts.get(j).cloned().unwrap_or_default();
                        let g = one_line(
                            ui,
                            &format!("· {text}"),
                            right - x,
                            FontId::proportional(12.0),
                            c.text_dim,
                        );
                        ui.painter()
                            .galley(pos2(x + 14.0, mid - g.size().y / 2.0), g, c.text_dim);
                    }
                    Row::Group(gi, open) => {
                        let g = &all[gi];
                        let caret = if open {
                            icon::CARET_DOWN
                        } else {
                            icon::CARET_RIGHT
                        };
                        let caret_rect =
                            Rect::from_center_size(pos2(x + 6.0, mid), vec2(18.0, ROW));
                        ui.painter().text(
                            pos2(x + 6.0, mid),
                            Align2::CENTER_CENTER,
                            caret,
                            FontId::proportional(13.0),
                            c.text_dim,
                        );
                        x += 18.0;
                        x += badge(ui, pos2(x, mid), &g.by);
                        let done = steps.done >= g.to;
                        let partly = steps.done > g.from && steps.done < g.to;
                        let color = if done || partly { c.text } else { c.text_dim };
                        let undone = g.to - steps.done.clamp(g.from, g.to);
                        let label = match undone {
                            0 => format!("{} steps", g.to - g.from),
                            u if u == g.to - g.from => format!("{} steps, undone", g.to - g.from),
                            u => format!("{} steps, {u} undone", g.to - g.from),
                        };
                        let undo_rect = Rect::from_min_max(
                            pos2(right - 92.0, rect.top() + 2.0),
                            pos2(right, rect.bottom() - 2.0),
                        );
                        let show_undo = r.hovered() && steps.done > g.from;
                        let time_w = if show_undo {
                            92.0
                        } else {
                            time_text(ui, steps.line[g.to - 1].time, now, right, mid)
                        };
                        let text = one_line(ui, &label, right - x - time_w - 8.0, font, color);
                        ui.painter()
                            .galley(pos2(x, mid - text.size().y / 2.0), text, color);
                        if show_undo {
                            let pointer = ui.input(|i| i.pointer.hover_pos());
                            let over = pointer.is_some_and(|p| undo_rect.contains(p));
                            ui.painter().rect_filled(
                                undo_rect,
                                3.0,
                                if over {
                                    c.accent.gamma_multiply(0.5)
                                } else {
                                    c.text.gamma_multiply(0.1)
                                },
                            );
                            ui.painter().text(
                                undo_rect.center(),
                                Align2::CENTER_CENTER,
                                format!("{} Undo all", icon::ARROW_COUNTER_CLOCKWISE),
                                FontId::proportional(12.0),
                                c.text,
                            );
                            if over {
                                self.history.hovered = Some(g.from);
                                if r.clicked() {
                                    go = Some(g.from);
                                }
                            }
                        }
                        let pointer = ui.input(|i| i.pointer.interact_pos());
                        if r.clicked() && go.is_none() {
                            if pointer.is_some_and(|p| caret_rect.contains(p)) {
                                toggle_group = Some(g.from);
                            } else {
                                go = Some(g.to);
                            }
                        }
                        if r.double_clicked() {
                            toggle_group = Some(g.from);
                        }
                        if r.hovered() && self.history.hovered.is_none() {
                            self.history.hovered = Some(g.to);
                        }
                        r.on_hover_text(format!(
                            "{} steps by {} in a row. Click to go to the last, ▸ to see them; \
                             Undo all goes back to before the first",
                            g.to - g.from,
                            who(&g.by)
                        ));
                        continue;
                    }
                    Row::Step(i, inner) => {
                        let s = &steps.line[i];
                        let done = i < steps.done;
                        let caret_rect =
                            Rect::from_center_size(pos2(x - 8.0, mid), vec2(16.0, ROW));
                        if !s.parts.is_empty() {
                            let caret = if self.history.parts.contains(&i) {
                                icon::CARET_DOWN
                            } else {
                                icon::CARET_RIGHT
                            };
                            ui.painter().text(
                                pos2(x - 8.0, mid),
                                Align2::CENTER_CENTER,
                                caret,
                                FontId::proportional(12.0),
                                c.text_dim,
                            );
                        }
                        if let Some(by) =
                            s.by.as_ref()
                                .filter(|b| b.kind != ClientKind::Gui && !inner)
                        {
                            x += badge(ui, pos2(x, mid), by);
                        }
                        let time_w = time_text(ui, s.time, now, right, mid);
                        let color = if done { c.text } else { c.text_dim };
                        let font = if current {
                            FontId::proportional(13.5)
                        } else {
                            font
                        };
                        let text = one_line(ui, &s.label, right - x - time_w - 8.0, font, color);
                        ui.painter()
                            .galley(pos2(x, mid - text.size().y / 2.0), text, color);
                        if r.clicked() {
                            let pointer = ui.input(|i| i.pointer.interact_pos());
                            if !s.parts.is_empty()
                                && pointer.is_some_and(|p| caret_rect.contains(p))
                            {
                                toggle_parts = Some(i);
                            } else {
                                go = Some(i + 1);
                            }
                        }
                        if r.hovered() {
                            self.history.hovered = Some(i + 1);
                        }
                        r.on_hover_text(step_tip(s, i + 1, done, p));
                        continue;
                    }
                }
                if r.hovered()
                    && let Some(s) = state
                {
                    self.history.hovered = Some(s);
                }
                if r.clicked()
                    && let Some(s) = state
                {
                    go = Some(s);
                }
            }
        });
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Hover a step to see the timeline as it was. Click it, or use \
                     \u{2191} \u{2193} and Enter, to go there; Esc comes back.",
                )
                .color(theme::colors().text_dim),
            )
            .wrap(),
        );
        if let Some(g) = toggle_group
            && !self.history.expanded.remove(&g)
        {
            self.history.expanded.insert(g);
        }
        if let Some(i) = toggle_parts
            && !self.history.parts.remove(&i)
        {
            self.history.parts.insert(i);
        }
        if let Some(step) = go {
            self.history.keys = true;
            self.history_go(step);
        }
    }

    fn history_rows(&self, steps: &Steps, all: &[Group]) -> Vec<Row> {
        let mut rows = vec![Row::Original];
        let mut opened = None;
        let mut gi = 0;
        let mut i = 0;
        let step = |rows: &mut Vec<Row>, i: usize, inner: bool| {
            rows.push(Row::Step(i, inner));
            if self.history.parts.contains(&i) {
                rows.extend((0..steps.line[i].parts.len()).map(|j| Row::Part(i, j)));
            }
        };
        while i < steps.line.len() {
            let s = &steps.line[i];
            if s.opened.is_some() && s.opened != opened {
                opened = s.opened;
                rows.push(Row::Opened(s.opened.unwrap_or_default()));
            }
            if let Some(g) = all.get(gi).filter(|g| g.from == i) {
                let inside = steps.done > g.from && steps.done < g.to;
                let cursor = self.history.cursor.is_some_and(|c| c > g.from && c < g.to);
                let open = self.history.expanded.contains(&g.from) || inside || cursor;
                rows.push(Row::Group(gi, open));
                if open {
                    for j in g.from..g.to {
                        step(&mut rows, j, true);
                    }
                }
                i = g.to;
                gi += 1;
                continue;
            }
            step(&mut rows, i, false);
            i += 1;
        }
        rows
    }

    pub(super) fn history_update(&mut self, ctx: &egui::Context) {
        let Some(steps) = self.steps() else {
            self.history.target = None;
            return;
        };
        let rev = self.project().map_or(0, |p| p.rev);
        self.history.target = self
            .history
            .hovered
            .or(self.history.was)
            .or(self.history.cursor)
            .filter(|&s| s <= steps.line.len());
        let wanted = self.history.target.filter(|&s| s != steps.done);
        if self.history.sent != Some((wanted, rev)) {
            self.history.sent = Some((wanted, rev));
            self.live.history_preview(wanted);
        }
        if let Some(f) = &self.snap.flash
            && self.history.flash.is_none_or(|(seq, _)| seq != f.seq)
        {
            self.history.flash = Some((f.seq, ctx.input(|i| i.time)));
        }
    }

    pub(super) fn history_shown(&self) -> Option<Arc<Project>> {
        let target = self.history.target?;
        let preview = self.snap.history_preview.as_ref()?;
        let rev = self.project()?.rev;
        (preview.step == target && preview.rev == rev).then(|| preview.project.clone())
    }

    pub(super) fn history_marks(&self, ctx: &egui::Context) -> Option<Marks> {
        let steps = self.steps()?;
        if let Some(t) = self.history.target {
            let s = steps.line.get(t.checked_sub(1)?)?;
            return Some(Marks {
                clips: s.clips.iter().cloned().collect(),
                spans: s.spans.clone(),
                strength: 1.0,
                focus: true,
            });
        }
        let f = self.snap.flash.as_ref()?;
        let (seq, start) = self.history.flash?;
        let age = ctx.input(|i| i.time) - start;
        if seq != f.seq || age > FLASH || f.rev != self.project()?.rev {
            return None;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        let t = (age / FLASH) as f32;
        Some(Marks {
            clips: f.clips.iter().cloned().collect(),
            spans: f.spans.clone(),
            strength: (1.0 - t * t).max(0.0),
            focus: false,
        })
    }
}

fn time_text(ui: &egui::Ui, time: Option<u64>, now: u64, right: f32, mid: f32) -> f32 {
    let Some(t) = time else {
        return 0.0;
    };
    let c = theme::colors().text_dim;
    let g = one_line(ui, &when::ago(t, now), 120.0, FontId::proportional(11.0), c);
    let w = g.size().x;
    ui.painter()
        .galley(pos2(right - w, mid - g.size().y / 2.0), g, c);
    w
}

fn truncate(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        return text.to_string();
    }
    let mut s: String = text.chars().take(n - 1).collect();
    s.push('…');
    s
}

fn step_tip(s: &Step, n: usize, done: bool, p: Option<&Project>) -> String {
    let mut lines = vec![s.label.clone()];
    let mut facts = vec![format!("Step {n}")];
    if let Some(by) = &s.by {
        facts.push(format!("by {}", who(by)));
    }
    if let Some(t) = s.time.and_then(when::local_time) {
        facts.push(t.full());
    }
    lines.push(facts.join(" · "));
    if let Some(at) = p.and_then(|p| place(p, &s.spans)) {
        lines.push(format!("Changed {at}"));
    }
    if !s.parts.is_empty() {
        lines.push(format!("{} commands as one step", s.parts.len()));
    }
    if !done {
        lines.push("Undone: a click redoes up to here".into());
    }
    lines.join("\n")
}
