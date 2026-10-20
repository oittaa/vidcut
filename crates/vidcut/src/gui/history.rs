use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, Modifiers, Rect, RichText, Sense, Stroke,
    StrokeKind, pos2, vec2,
};
use egui_phosphor::regular as icon;
use serde_json::json;
use vidcut_core::model::Project;
use vidcut_core::request::{Client, ClientKind};
use vidcut_core::steps::{
    Branch, Group, KINDS, Span, Step, Steps, category, every_step, groups, since_step,
    since_version,
};

use super::theme;
use super::workspace::Workspace;
use crate::live::{PlayerId, PreviewAt};
use crate::when;

const ROW: f32 = 24.0;
const WIDTH: f32 = 440.0;
pub(super) const MIN_WIDTH: f32 = 320.0;
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
    twig: Option<(u64, usize)>,
    was_twig: Option<(u64, usize)>,
    version: Option<(u64, usize)>,
    sent: Option<(Option<PreviewAt>, u64)>,
    expanded: HashSet<usize>,
    parts: HashSet<usize>,
    twigs: HashSet<u64>,
    flash: Option<(u64, f64)>,
    follow: bool,
    seen: Option<(usize, usize)>,
    find: String,
    kind: Option<&'static str>,
    source: Option<String>,
    naming: Option<Naming>,
    since: Option<((PreviewAt, u64), String)>,
    generation: u64,
    laid: Option<Laid>,
    opening: bool,
    drawn: bool,
    swallow: bool,
}

#[derive(Clone)]
struct Layout {
    groups: Arc<Vec<Group>>,
    rows: Arc<Vec<Row>>,
    paths: Arc<Vec<Vec<usize>>>,
    kinds: Arc<Vec<&'static str>>,
    sources: Arc<Vec<String>>,
}

struct Laid {
    steps: Arc<Steps>,
    cursor: Option<usize>,
    generation: u64,
    filter: (String, Option<&'static str>, Option<String>),
    layout: Layout,
}

struct Naming {
    step: usize,
    name: String,
    fresh: bool,
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
    Branch(usize, bool),
    Twig(usize, usize),
    Flag(usize),
}

enum Act {
    Go(usize),
    Group(usize),
    Parts(usize),
    Twigs(u64),
    Restore(u64),
    Revert(usize),
    Name(usize),
    Unname(usize),
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
        self.opening = true;
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.leave();
    }

    fn leave(&mut self) {
        self.keys = false;
        self.cursor = None;
        self.hovered = None;
        self.was = None;
        self.twig = None;
        self.was_twig = None;
    }

    fn filtered(&self) -> bool {
        !self.find.trim().is_empty() || self.kind.is_some() || self.source.is_some()
    }

    fn matches(&self, s: &Step, name: Option<&str>) -> bool {
        let find = self.find.trim().to_lowercase();
        let text = find.is_empty()
            || s.label.to_lowercase().contains(&find)
            || name.is_some_and(|n| n.to_lowercase().contains(&find))
            || s.parts.iter().any(|p| p.to_lowercase().contains(&find));
        let kind = self
            .kind
            .is_none_or(|k| s.kind.as_deref().map(category) == Some(k));
        let source = self
            .source
            .as_ref()
            .is_none_or(|w| s.by.as_ref().map(Client::shown).as_ref() == Some(w));
        text && kind && source
    }
}

fn kind_color(kind: ClientKind) -> Color32 {
    match kind {
        ClientKind::Gui => theme::colors().accent,
        ClientKind::Cli => Color32::from_rgb(64, 160, 150),
        ClientKind::Mcp => Color32::from_rgb(150, 110, 225),
        ClientKind::Ipc => Color32::from_rgb(80, 135, 215),
        ClientKind::File => Color32::from_rgb(210, 135, 55),
        ClientKind::Other => Color32::from_rgb(120, 120, 130),
    }
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

fn chip(ui: &egui::Ui, at: egui::Pos2, text: &str, fill: Color32) -> f32 {
    let font = FontId::proportional(11.0);
    let text = one_line(ui, text, 140.0, font, Color32::WHITE);
    let rect = Rect::from_min_size(
        pos2(at.x, at.y - text.size().y / 2.0 - 2.0),
        vec2(text.size().x + 10.0, text.size().y + 4.0),
    );
    ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    ui.painter().galley(
        pos2(rect.left() + 5.0, rect.top() + 2.0),
        text,
        Color32::WHITE,
    );
    rect.width() + 6.0
}

fn badge(ui: &egui::Ui, at: egui::Pos2, c: &Client) -> f32 {
    chip(ui, at, &c.shown(), kind_color(c.kind))
}

fn flag(ui: &egui::Ui, at: egui::Pos2, name: &str) -> f32 {
    chip(
        ui,
        at,
        &format!("{} {name}", icon::FLAG),
        theme::colors().accent.gamma_multiply(0.85),
    )
}

fn caret(ui: &egui::Ui, at: egui::Pos2, open: bool) -> Rect {
    let c = if open {
        icon::CARET_DOWN
    } else {
        icon::CARET_RIGHT
    };
    ui.painter().text(
        at,
        Align2::CENTER_CENTER,
        c,
        FontId::proportional(12.0),
        theme::colors().text_dim,
    );
    Rect::from_center_size(at, vec2(18.0, ROW))
}

fn steps_word(n: usize) -> String {
    if n == 1 {
        "1 step".into()
    } else {
        format!("{n} steps")
    }
}

fn kinds_in(steps: &Steps) -> Vec<&'static str> {
    let present: HashSet<&'static str> = every_step(steps)
        .into_iter()
        .filter_map(|s| s.kind.as_deref().map(category))
        .collect();
    KINDS.into_iter().filter(|k| present.contains(k)).collect()
}

fn sources_in(steps: &Steps) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in every_step(steps) {
        if let Some(name) = s.by.as_ref().map(Client::shown)
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

impl Workspace {
    fn steps(&self) -> Option<Arc<Steps>> {
        self.snap.project.as_ref().map(|p| p.history.clone())
    }

    pub(super) fn history_go(&mut self, step: usize) {
        self.history.cursor = None;
        let Some(steps) = self.steps().filter(|s| s.done != step) else {
            return;
        };
        self.send("history.go", json!({ "step": step }));
        let spans = step
            .checked_sub(1)
            .and_then(|i| steps.line.get(i))
            .map(|s| s.spans.as_slice())
            .unwrap_or_default();
        if let (Some(from), Some(to)) = (
            spans.iter().map(|s| s.from).min(),
            spans.iter().map(|s| s.to).max(),
        ) && !self.lower_shows(from, to)
        {
            self.seek(PlayerId::Timeline, from, false);
        }
    }

    pub(super) fn history_swallows(&mut self, ui: &egui::Ui, rect: Rect, drawn: bool) -> bool {
        let (pressed, down, released, pos) = ui.input(|i| {
            (
                i.pointer.any_pressed(),
                i.pointer.any_down(),
                i.pointer.any_released(),
                i.pointer.interact_pos(),
            )
        });
        let h = &mut self.history;
        if h.swallow && !pressed && !down && !released {
            h.swallow = false;
        }
        if h.drawn && pressed && pos.is_some_and(|p| rect.contains(p)) {
            h.swallow = true;
            h.leave();
        }
        h.drawn = drawn;
        h.swallow
    }

    pub(super) fn history_name(&mut self, step: Option<usize>) {
        let Some(steps) = self.steps() else {
            return;
        };
        let step = step.unwrap_or(steps.done);
        let id = match step {
            0 => Some(0),
            n => steps.line.get(n - 1).and_then(|s| s.id),
        };
        let name = id
            .and_then(|id| steps.named(id))
            .map(String::from)
            .unwrap_or_else(|| format!("Version {}", steps.marks.len() + 1));
        self.history.naming = Some(Naming {
            step,
            name,
            fresh: true,
        });
    }

    pub(super) fn history_keys(&mut self, ctx: &egui::Context) {
        let h = &mut self.history;
        if !h.open || !h.keys || h.naming.is_some() {
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
            let name = s.id.and_then(|id| steps.named(id));
            let label = match name {
                Some(n) => format!("{} {n} · {}", icon::FLAG, s.label),
                None => s.label.clone(),
            };
            let mut text = RichText::new(format!(
                "{}  {}",
                if current { icon::CHECK } else { "   " },
                truncate(&label, 64)
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
            let r = r;
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
        if ui.button("Mark This Version…").clicked() {
            self.history_name(None);
            ui.close();
        }
        if ui.button("Open History Window").clicked() {
            self.history.show();
        }
        if let Some(step) = go {
            self.history_go(step);
        }
    }

    pub(super) fn history_frame(&mut self, ctx: &egui::Context) {
        self.history.was = self.history.hovered.take();
        self.history.was_twig = self.history.twig.take();
        self.naming_window(ctx);
        if !self.history.open {
            self.history.cursor = None;
        }
    }

    pub(super) fn history_panel(&mut self, ui: &mut egui::Ui) {
        let Some(steps) = self.steps() else {
            return;
        };
        if self.history.seen != Some((steps.done, steps.line.len())) {
            self.history.seen = Some((steps.done, steps.line.len()));
            self.history.follow = true;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("History").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = ui.add(egui::Button::new(icon::X).frame(false));
                close.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, "Close the history")
                });
                if close.clicked() {
                    self.history.close();
                }
            });
        });
        self.history_list(ui, &steps);
        let inside = ui.min_rect();
        let clicked = ui.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        let opening = std::mem::take(&mut self.history.opening);
        if let Some(pos) = clicked
            && !opening
        {
            self.history.keys = inside.contains(pos);
            if !self.history.keys {
                self.history.leave();
            }
        }
        let width = ui.min_rect().width();
        if (width - self.history.width).abs() > 1.0 && !ui.input(|i| i.pointer.any_down()) {
            self.history.width = width;
            self.history.save();
        }
    }

    fn history_layout(&mut self, steps: &Arc<Steps>) -> Layout {
        let h = &self.history;
        let filter = (h.find.clone(), h.kind, h.source.clone());
        if let Some(l) = &h.laid
            && Arc::ptr_eq(&l.steps, steps)
            && l.cursor == h.cursor
            && l.generation == h.generation
            && l.filter == filter
        {
            return l.layout.clone();
        }
        let all = groups(&steps.line);
        let paths = version_paths(steps);
        let layout = Layout {
            rows: Arc::new(self.history_rows(steps, &all, &paths)),
            groups: Arc::new(all),
            paths: Arc::new(paths),
            kinds: Arc::new(kinds_in(steps)),
            sources: Arc::new(sources_in(steps)),
        };
        self.history.laid = Some(Laid {
            steps: steps.clone(),
            cursor: self.history.cursor,
            generation: self.history.generation,
            filter,
            layout: layout.clone(),
        });
        layout
    }

    fn naming_window(&mut self, ctx: &egui::Context) {
        let Some(naming) = &mut self.history.naming else {
            return;
        };
        let mut done = None;
        let mut open = true;
        egui::Window::new("Mark This Version")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(ctx.content_rect().center() - vec2(160.0, 60.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(if naming.step == 0 {
                        "A name for the project as it was before every step".to_string()
                    } else {
                        format!("A name for the project as it is after step {}", naming.step)
                    })
                    .color(theme::colors().text_dim),
                );
                ui.horizontal(|ui| {
                    ui.label("Name");
                    let r =
                        ui.add(egui::TextEdit::singleline(&mut naming.name).desired_width(240.0));
                    if naming.fresh {
                        r.request_focus();
                        naming.fresh = false;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        done = Some(true);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                    let ok = !naming.name.trim().is_empty();
                    if ui.add_enabled(ok, egui::Button::new("Mark")).clicked() {
                        done = Some(true);
                    }
                });
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    done = Some(false);
                }
            });
        if !open {
            done = Some(false);
        }
        match done {
            Some(true) if !naming.name.trim().is_empty() => {
                let (step, name) = (naming.step, naming.name.trim().to_string());
                self.history.naming = None;
                self.send("history.mark", json!({ "step": step, "name": name }));
            }
            Some(_) => self.history.naming = None,
            None => {}
        }
    }

    fn history_header(&mut self, ui: &mut egui::Ui, steps: &Steps, shown: usize, layout: &Layout) {
        let (kinds, sources) = (&layout.kinds, &layout.sources);
        let n = steps.line.len();
        ui.horizontal(|ui| {
            let undo = ui.add_enabled(
                steps.done > 0,
                egui::Button::new(format!("{} Undo", icon::ARROW_COUNTER_CLOCKWISE)),
            );
            if undo.clicked() {
                self.send("history.undo", json!({}));
            }
            let redo = ui.add_enabled(
                steps.done < n,
                egui::Button::new(format!("{} Redo", icon::ARROW_CLOCKWISE)),
            );
            if redo.clicked() {
                self.send("history.redo", json!({}));
            }
            if ui.button(format!("{} Mark…", icon::FLAG)).clicked() {
                self.history_name(None);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = match n {
                    0 => "No changes yet".to_string(),
                    _ if self.history.filtered() => format!("{shown} of {n} steps"),
                    _ if steps.done == n => steps_word(n),
                    _ => format!("At step {} of {n}", steps.done),
                };
                ui.add(
                    egui::Label::new(RichText::new(text).color(theme::colors().text_dim))
                        .truncate(),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(theme::colors().text_dim));
            ui.add(
                egui::TextEdit::singleline(&mut self.history.find)
                    .hint_text("Find a step or a name")
                    .desired_width(170.0),
            );
            egui::ComboBox::from_id_salt("history-kind")
                .selected_text(self.history.kind.unwrap_or("Every kind"))
                .width(118.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.history.kind, None, "Every kind");
                    for &k in kinds.iter() {
                        ui.selectable_value(&mut self.history.kind, Some(k), k);
                    }
                });
            egui::ComboBox::from_id_salt("history-source")
                .selected_text(self.history.source.as_deref().unwrap_or("Anyone"))
                .width(96.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.history.source, None, "Anyone");
                    for s in sources.iter() {
                        ui.selectable_value(&mut self.history.source, Some(s.clone()), s);
                    }
                });
        });
    }

    fn history_list(&mut self, ui: &mut egui::Ui, steps: &Arc<Steps>) {
        let layout = self.history_layout(steps);
        let (all, rows, paths) = (
            layout.groups.clone(),
            layout.rows.clone(),
            layout.paths.clone(),
        );
        let shown = rows.iter().filter(|r| matches!(r, Row::Step(..))).count();
        self.history_header(ui, steps, shown, &layout);
        ui.separator();
        let mark = self.history.cursor.unwrap_or(steps.done);
        let mark_row = rows.iter().position(|r| match *r {
            Row::Original => mark == 0,
            Row::Step(i, _) => i + 1 == mark,
            Row::Group(g, false) => all[g].to == mark,
            _ => false,
        });
        let footer = 52.0;
        let view = (ui.available_height() - footer).max(ROW * 3.0);
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
        let mut acts = Vec::new();
        area.show_rows(ui, ROW, rows.len(), |ui, range| {
            for k in range {
                acts.extend(self.history_row(ui, rows[k], steps, &all, &paths, now));
            }
        });
        let footer = self.history_footer(steps);
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(footer).color(theme::colors().text_dim)).wrap());
        for act in acts {
            match act {
                Act::Go(step) => {
                    self.history.keys = true;
                    self.history_go(step);
                }
                Act::Group(g) => {
                    self.history.generation += 1;
                    if !self.history.expanded.remove(&g) {
                        self.history.expanded.insert(g);
                    }
                }
                Act::Parts(i) => {
                    self.history.generation += 1;
                    if !self.history.parts.remove(&i) {
                        self.history.parts.insert(i);
                    }
                }
                Act::Twigs(id) => {
                    self.history.generation += 1;
                    if !self.history.twigs.remove(&id) {
                        self.history.twigs.insert(id);
                    }
                }
                Act::Restore(id) => self.send("history.restore", json!({ "branch": id })),
                Act::Revert(step) => self.send("history.revert", json!({ "step": step })),
                Act::Name(step) => self.history_name(Some(step)),
                Act::Unname(step) => self.send("history.unmark", json!({ "step": step })),
            }
        }
    }

    fn history_footer(&mut self, steps: &Steps) -> String {
        let hint = "Hover a step to see the timeline as it was. Click it, or use \u{2191} \
                    \u{2193} and Enter, to go there; Esc comes back.";
        let at = match (self.history.version, self.history.target) {
            (Some((id, k)), _) => PreviewAt::Version(id, k),
            (None, Some(t)) if t == steps.done => return "This is where the project is".into(),
            (None, Some(t)) => PreviewAt::Step(t),
            (None, None) => return hint.into(),
        };
        let (Some(then), Some(now)) = (
            self.history_shown(),
            self.snap.project.as_ref().map(|p| p.project.clone()),
        ) else {
            return hint.into();
        };
        let key = (at, now.rev);
        if self.history.since.as_ref().is_none_or(|(k, _)| *k != key) {
            let text = match at {
                PreviewAt::Version(id, _) => since_version(&then, &now, id).text(),
                PreviewAt::Step(t) => since_step(&then, &now, t, steps.done).text(),
            };
            self.history.since = Some((key, text));
        }
        self.history
            .since
            .as_ref()
            .map(|(_, t)| t.clone())
            .unwrap_or_default()
    }

    #[allow(clippy::too_many_arguments)]
    fn history_row(
        &mut self,
        ui: &mut egui::Ui,
        row: Row,
        steps: &Steps,
        all: &[Group],
        paths: &[Vec<usize>],
        now: u64,
    ) -> Vec<Act> {
        let mut acts = Vec::new();
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        let state = match row {
            Row::Original => Some(0),
            Row::Step(i, _) => Some(i + 1),
            Row::Group(g, _) => Some(all[g].to),
            Row::Flag(i) => Some(i + 1),
            Row::Opened(_) | Row::Part(..) | Row::Branch(..) | Row::Twig(..) => None,
        };
        let current =
            state == Some(steps.done) && !matches!(row, Row::Group(_, true) | Row::Flag(_));
        let cursor = self.history.cursor.is_some() && state == self.history.cursor;
        let branch = |bi: usize| -> &Branch<Step> { version(steps, &paths[bi]) };
        let named = match row {
            Row::Original => "Original".to_string(),
            Row::Opened(t) => format!("Opened {}", when::stamp(t, now)),
            Row::Group(g, _) => {
                format!(
                    "{} by {}",
                    steps_word(all[g].to - all[g].from),
                    all[g].by.shown()
                )
            }
            Row::Step(i, _) => steps.line[i].label.clone(),
            Row::Part(i, j) => steps.line[i].parts.get(j).cloned().unwrap_or_default(),
            Row::Branch(bi, _) => branch_label(branch(bi), now),
            Row::Twig(bi, k) => branch(bi).steps[k].label.clone(),
            Row::Flag(i) => {
                let s = &steps.line[i];
                let name = s.id.and_then(|id| steps.named(id)).unwrap_or_default();
                format!("{name}: {}", s.label)
            }
        };
        r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, current, &named));
        let c = theme::colors();
        if current {
            ui.painter()
                .rect_filled(rect, 3.0, c.selection.gamma_multiply(0.55));
        } else if r.hovered() && !matches!(row, Row::Opened(_) | Row::Part(..)) {
            ui.painter()
                .rect_filled(rect, 3.0, c.text.gamma_multiply(0.07));
        }
        if cursor {
            ui.painter()
                .rect_stroke(rect, 3.0, Stroke::new(1.5, c.accent), StrokeKind::Inside);
        }
        let mid = rect.center().y;
        let depth = match row {
            Row::Step(_, true) | Row::Part(..) | Row::Flag(_) => 1.0,
            Row::Branch(bi, _) => paths[bi].len() as f32 - 1.0,
            Row::Twig(bi, _) => paths[bi].len() as f32,
            _ => 0.0,
        };
        let mut x = rect.left() + 4.0 + depth * 18.0;
        if current {
            ui.painter()
                .circle_filled(pos2(x + 5.0, mid), 3.5, c.accent);
        }
        x += 14.0;
        let font = FontId::proportional(13.0);
        let small = FontId::proportional(12.0);
        let right = rect.right() - 6.0;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let on = |rect: Rect| pointer.is_some_and(|p| rect.contains(p));
        match row {
            Row::Original => {
                if let Some(name) = steps.named(0) {
                    x += flag(ui, pos2(x, mid), name);
                }
                let g = one_line(ui, "Original", right - x, font, c.text);
                ui.painter()
                    .galley(pos2(x, mid - g.size().y / 2.0), g, c.text);
                if r.hovered() {
                    self.history.hovered = Some(0);
                }
                if r.clicked() {
                    acts.push(Act::Go(0));
                }
                r.context_menu(|ui| {
                    if ui.button("Go Here").clicked() {
                        acts.push(Act::Go(0));
                    }
                    names_menu(ui, steps, 0, &mut acts);
                });
            }
            Row::Opened(t) => {
                let text = format!("Opened {}", when::stamp(t, now));
                let g = one_line(ui, &text, right - x, FontId::proportional(11.0), c.text_dim);
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
                let g = one_line(ui, &format!("· {text}"), right - x, small, c.text_dim);
                ui.painter()
                    .galley(pos2(x + 14.0, mid - g.size().y / 2.0), g, c.text_dim);
            }
            Row::Group(gi, open) => {
                let g = &all[gi];
                let caret_rect = caret(ui, pos2(x + 6.0, mid), open);
                x += 18.0;
                x += badge(ui, pos2(x, mid), &g.by);
                let reached = steps.done > g.from;
                let color = if reached { c.text } else { c.text_dim };
                let undone = g.to - steps.done.clamp(g.from, g.to);
                let label = match undone {
                    0 => steps_word(g.to - g.from),
                    u if u == g.to - g.from => format!("{}, undone", steps_word(u)),
                    u => format!("{}, {u} undone", steps_word(g.to - g.from)),
                };
                let undo_rect = Rect::from_min_max(
                    pos2(right - 92.0, rect.top() + 2.0),
                    pos2(right, rect.bottom() - 2.0),
                );
                let show_undo = r.hovered() && reached;
                let time_w = if show_undo {
                    92.0
                } else {
                    time_text(ui, steps.line[g.to - 1].time, now, right, mid)
                };
                let text = one_line(ui, &label, right - x - time_w - 8.0, font, color);
                ui.painter()
                    .galley(pos2(x, mid - text.size().y / 2.0), text, color);
                let over_undo = show_undo
                    && ui
                        .input(|i| i.pointer.hover_pos())
                        .is_some_and(|p| undo_rect.contains(p));
                if show_undo {
                    button(
                        ui,
                        undo_rect,
                        &format!("{} Undo all", icon::ARROW_COUNTER_CLOCKWISE),
                        over_undo,
                    );
                }
                if over_undo {
                    self.history.hovered = Some(g.from);
                } else if r.hovered() {
                    self.history.hovered = Some(g.to);
                }
                if r.clicked() {
                    acts.push(if over_undo {
                        Act::Go(g.from)
                    } else if on(caret_rect) {
                        Act::Group(g.from)
                    } else {
                        Act::Go(g.to)
                    });
                }
                if r.double_clicked() {
                    acts.push(Act::Group(g.from));
                }
            }
            Row::Step(i, inner) => {
                let s = &steps.line[i];
                let done = i < steps.done;
                let caret_rect = (!s.parts.is_empty())
                    .then(|| caret(ui, pos2(x - 8.0, mid), self.history.parts.contains(&i)));
                if let Some(by) =
                    s.by.as_ref()
                        .filter(|b| b.kind != ClientKind::Gui && !inner)
                {
                    x += badge(ui, pos2(x, mid), by);
                }
                if let Some(name) = s.id.and_then(|id| steps.named(id)) {
                    x += flag(ui, pos2(x, mid), name);
                }
                let time_w = time_text(ui, s.time, now, right, mid);
                let color = if done { c.text } else { c.text_dim };
                let text = one_line(ui, &s.label, right - x - time_w - 8.0, font, color);
                ui.painter()
                    .galley(pos2(x, mid - text.size().y / 2.0), text, color);
                if r.clicked() {
                    acts.push(match caret_rect {
                        Some(cr) if on(cr) => Act::Parts(i),
                        _ => Act::Go(i + 1),
                    });
                }
                if r.hovered() {
                    self.history.hovered = Some(i + 1);
                }
                let r = r;
                r.context_menu(|ui| {
                    if ui.button("Go Here").clicked() {
                        acts.push(Act::Go(i + 1));
                    }
                    let revert = ui.add_enabled(done, egui::Button::new("Revert This Step"));
                    if revert.clicked() {
                        acts.push(Act::Revert(i + 1));
                    }
                    names_menu(ui, steps, i + 1, &mut acts);
                    ui.separator();
                    if ui.button("Copy Label").clicked() {
                        ui.ctx().copy_text(s.label.clone());
                    }
                });
            }
            Row::Branch(bi, open) => {
                let b = branch(bi);
                let id = b.id();
                let caret_rect = caret(ui, pos2(x + 6.0, mid), open);
                x += 18.0;
                ui.painter().text(
                    pos2(x + 6.0, mid),
                    Align2::CENTER_CENTER,
                    icon::GIT_BRANCH,
                    FontId::proportional(13.0),
                    c.accent,
                );
                x += 16.0;
                let back_rect = Rect::from_min_max(
                    pos2(right - 128.0, rect.top() + 2.0),
                    pos2(right, rect.bottom() - 2.0),
                );
                let show = r.hovered() && id.is_some();
                let over = show
                    && ui
                        .input(|i| i.pointer.hover_pos())
                        .is_some_and(|p| back_rect.contains(p));
                let label = branch_label(b, now);
                let w = if show { 132.0 } else { 0.0 };
                let text = one_line(ui, &label, right - x - w - 4.0, small.clone(), c.text_dim);
                ui.painter()
                    .galley(pos2(x, mid - text.size().y / 2.0), text, c.text_dim);
                if show {
                    button(ui, back_rect, "Restore this version", over);
                }
                if r.clicked() {
                    match (over, id) {
                        (true, Some(id)) => acts.push(Act::Restore(id)),
                        (_, Some(id)) if on(caret_rect) || !over => acts.push(Act::Twigs(id)),
                        _ => {}
                    }
                }
            }
            Row::Twig(bi, k) => {
                let b = branch(bi);
                let s = &b.steps[k];
                if r.hovered() {
                    self.history.twig = b.id().map(|id| (id, k));
                }
                if let Some(name) = s.id.and_then(|id| steps.named(id)) {
                    x += flag(ui, pos2(x, mid), name);
                }
                let time_w = time_text(ui, s.time, now, right, mid);
                let text = one_line(
                    ui,
                    &s.label,
                    right - x - time_w - 8.0,
                    FontId::new(12.5, egui::FontFamily::Proportional),
                    c.text_dim.gamma_multiply(0.8),
                );
                ui.painter()
                    .galley(pos2(x, mid - text.size().y / 2.0), text, c.text_dim);
            }
            Row::Flag(i) => {
                let s = &steps.line[i];
                if let Some(name) = s.id.and_then(|id| steps.named(id)) {
                    x += flag(ui, pos2(x, mid), name);
                }
                let color = if i < steps.done { c.text } else { c.text_dim };
                let text = one_line(ui, &s.label, right - x - 8.0, small, color);
                ui.painter()
                    .galley(pos2(x, mid - text.size().y / 2.0), text, color);
                if r.hovered() {
                    self.history.hovered = Some(i + 1);
                }
                if r.clicked() {
                    acts.push(Act::Go(i + 1));
                }
                let r = r;
                r.context_menu(|ui| {
                    if ui.button("Go Here").clicked() {
                        acts.push(Act::Go(i + 1));
                    }
                    names_menu(ui, steps, i + 1, &mut acts);
                });
            }
        }
        acts
    }

    fn history_rows(&self, steps: &Steps, all: &[Group], paths: &[Vec<usize>]) -> Vec<Row> {
        let mut rows = vec![Row::Original];
        if self.history.filtered() {
            rows.extend(
                steps
                    .line
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| self.history.matches(s, s.id.and_then(|id| steps.named(id))))
                    .map(|(i, _)| Row::Step(i, false)),
            );
            for (bi, path) in paths.iter().enumerate() {
                let b = version(steps, path);
                let found: Vec<usize> = (0..b.steps.len())
                    .filter(|&k| {
                        let s = &b.steps[k];
                        self.history.matches(s, s.id.and_then(|id| steps.named(id)))
                    })
                    .collect();
                if !found.is_empty() {
                    rows.push(Row::Branch(bi, true));
                    rows.extend(found.into_iter().map(|k| Row::Twig(bi, k)));
                }
            }
            return rows;
        }
        let at: HashMap<&[usize], usize> = paths
            .iter()
            .enumerate()
            .map(|(bi, p)| (p.as_slice(), bi))
            .collect();
        let mut hanging: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut inside: HashMap<usize, Vec<usize>> = HashMap::new();
        for (bi, path) in paths.iter().enumerate() {
            match path.split_last() {
                Some((_, [])) => hanging
                    .entry(version(steps, path).after)
                    .or_default()
                    .push(bi),
                Some((_, outer)) => inside.entry(at[outer]).or_default().push(bi),
                None => {}
            }
        }
        let twigs = &self.history.twigs;
        let branches = |rows: &mut Vec<Row>, after: usize| {
            for &bi in hanging.get(&after).into_iter().flatten() {
                version_rows(rows, bi, steps, paths, &inside, twigs);
            }
        };
        let step = |rows: &mut Vec<Row>, i: usize, inner: bool| {
            rows.push(Row::Step(i, inner));
            if self.history.parts.contains(&i) {
                rows.extend((0..steps.line[i].parts.len()).map(|j| Row::Part(i, j)));
            }
            branches(rows, i + 1);
        };
        branches(&mut rows, 0);
        let mut opened = None;
        let mut gi = 0;
        let mut i = 0;
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
                } else {
                    rows.extend(
                        (g.from..g.to)
                            .filter(|&j| steps.line[j].id.and_then(|id| steps.named(id)).is_some())
                            .map(Row::Flag),
                    );
                    for j in g.from..g.to {
                        branches(&mut rows, j + 1);
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
        let version = self.history.twig.or(self.history.was_twig);
        self.history.version = version;
        self.history.target = match version {
            Some(_) => None,
            None => self
                .history
                .hovered
                .or(self.history.was)
                .or(self.history.cursor.filter(|_| self.history.keys))
                .filter(|&s| s <= steps.line.len()),
        };
        let wanted = match version {
            Some((id, k)) => Some(PreviewAt::Version(id, k)),
            None => self
                .history
                .target
                .filter(|&s| s != steps.done)
                .map(PreviewAt::Step),
        };
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
        let at = match self.history.version {
            Some((id, k)) => PreviewAt::Version(id, k),
            None => PreviewAt::Step(self.history.target?),
        };
        let preview = self.snap.history_preview.as_ref()?;
        let rev = self.project()?.rev;
        (preview.at == at && preview.rev == rev).then(|| preview.project.clone())
    }

    pub(super) fn history_marks(&self, ctx: &egui::Context) -> Option<Marks> {
        let steps = self.steps()?;
        if let Some((id, k)) = self.history.version {
            let s = steps.chain(id)?.last()?.steps.get(k)?;
            return Some(Marks {
                clips: s.clips.iter().cloned().collect(),
                spans: s.spans.clone(),
                strength: 1.0,
                focus: true,
            });
        }
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

fn version_paths(steps: &Steps) -> Vec<Vec<usize>> {
    fn walk(branches: &[Branch<Step>], path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        for (i, b) in branches.iter().enumerate() {
            path.push(i);
            out.push(path.clone());
            walk(&b.branches, path, out);
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(&steps.branches, &mut Vec::new(), &mut out);
    out
}

fn version<'a>(steps: &'a Steps, path: &[usize]) -> &'a Branch<Step> {
    let mut b = &steps.branches[path[0]];
    for &i in &path[1..] {
        b = &b.branches[i];
    }
    b
}

fn version_rows(
    rows: &mut Vec<Row>,
    bi: usize,
    steps: &Steps,
    paths: &[Vec<usize>],
    inside: &HashMap<usize, Vec<usize>>,
    twigs: &HashSet<u64>,
) {
    let b = version(steps, &paths[bi]);
    let open = b.id().is_some_and(|id| twigs.contains(&id));
    rows.push(Row::Branch(bi, open));
    if !open {
        return;
    }
    let inner = inside.get(&bi).map(Vec::as_slice).unwrap_or_default();
    for k in 0..=b.steps.len() {
        for &c in inner {
            if version(steps, &paths[c]).after == k {
                version_rows(rows, c, steps, paths, inside, twigs);
            }
        }
        if k < b.steps.len() {
            rows.push(Row::Twig(bi, k));
        }
    }
}

fn names_menu(ui: &mut egui::Ui, steps: &Steps, step: usize, acts: &mut Vec<Act>) {
    let id = match step {
        0 => Some(0),
        n => steps.line.get(n - 1).and_then(|s| s.id),
    };
    let named = id.and_then(|id| steps.named(id)).is_some();
    let mark = ui.add_enabled(
        id.is_some(),
        egui::Button::new(if named {
            "Rename This Version…"
        } else {
            "Mark This Version…"
        }),
    );
    if mark.clicked() {
        acts.push(Act::Name(step));
    }
    if named && ui.button("Remove the Name").clicked() {
        acts.push(Act::Unname(step));
    }
}

fn button(ui: &egui::Ui, rect: Rect, text: &str, over: bool) {
    let c = theme::colors();
    ui.painter().rect_filled(
        rect,
        3.0,
        if over {
            c.accent.gamma_multiply(0.5)
        } else {
            c.text.gamma_multiply(0.1)
        },
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.0),
        c.text,
    );
}

fn branch_label(b: &Branch<Step>, now: u64) -> String {
    let n = b.steps.len();
    let undone = if n == 1 {
        "1 undone step".to_string()
    } else {
        format!("{n} undone steps")
    };
    let more = match b.branches.len() {
        0 => String::new(),
        1 => " (and a version of them)".into(),
        k => format!(" (and {k} versions of them)"),
    };
    match b.replaced {
        Some(t) => format!("{undone}{more}, replaced {}", when::ago(t, now)),
        None => format!("{undone}{more}, replaced"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn by(kind: &str, client: Client) -> Step {
        Step {
            kind: Some(kind.into()),
            by: Some(client),
            ..Step::labeled("x")
        }
    }

    fn nested(step: Step) -> Steps {
        let mut steps = Steps::default();
        steps
            .line
            .push(by("edit.append", Client::new(ClientKind::Gui, None)));
        steps.branches.push(Branch {
            after: 1,
            replaced: None,
            steps: vec![by("bins.add", Client::new(ClientKind::Gui, None))],
            branches: vec![Branch {
                after: 0,
                replaced: None,
                steps: vec![step],
                branches: Vec::new(),
            }],
        });
        steps
    }

    #[test]
    fn the_kind_filter_offers_a_kind_found_only_in_a_version() {
        let steps = nested(by("markers.add", Client::new(ClientKind::Gui, None)));
        assert_eq!(
            kinds_in(&steps),
            ["Edits in", "Markers and marks", "Media and bins"]
        );
    }

    #[test]
    fn the_who_filter_offers_a_client_found_only_in_a_version() {
        let agent = Client::new(ClientKind::Mcp, Some("agent".into()));
        let steps = nested(by("markers.add", agent));
        assert_eq!(sources_in(&steps), ["GUI", "agent"]);
    }
}
