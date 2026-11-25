use egui::{Color32, Painter, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use serde_json::{Value, json};
use vidcut_core::grade_keys::{Dissolve, LAST_PROFILE};
use vidcut_core::model::{Clip, Grade, GradeKeyKind, GradeNode, Project};
use vidcut_core::ops::grade::GradeOf;
use vidcut_core::request::{GradeLevel, Page};
use vidcut_core::shortcuts::Action;

use super::keyframe_editor::{KeyCmd, diamond, dot};
use super::theme;
use super::workspace::Workspace;
use crate::live::PlayerId;

const LABEL: f32 = 112.0;

const BUTTON: f32 = 15.0;

const ROW: f32 = 18.0;

const RULER: f32 = 18.0;

const NEAR: f32 = 6.0;

const INSET: f32 = 6.0;

const CURVE: egui::Vec2 = egui::Vec2::new(240.0, 140.0);

const ALL_BAR: Color32 = Color32::from_rgb(255, 140, 0);

const COLOR_BAR: Color32 = Color32::from_rgb(70, 190, 110);

const PICKED: Color32 = Color32::from_rgb(230, 50, 50);

const BAND: Color32 = Color32::from_rgba_premultiplied(60, 60, 70, 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum KeyMode {
    #[default]
    All,
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Picked {
    node: Option<u32>,
    frame: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Aim {
    Track(Option<u32>),
    Key(Picked),
}

#[derive(Debug, Clone)]
enum KeyAct {
    Add(Option<u32>, GradeKeyKind),
    Kind(Option<u32>, Vec<i64>, GradeKeyKind),
    Attributes(Picked),
    Delete(Option<u32>, Vec<i64>),
    Marks(Option<u32>, bool),
    Node(u32, Flag, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    On,
    Locked,
    AutoKey,
}

impl Flag {
    fn field(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Locked => "locked",
            Self::AutoKey => "auto_key",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Self::On => "Enable/Disable",
            Self::Locked => "Lock",
            Self::AutoKey => "Auto-Keyframe",
        }
    }
}

struct Dynamics {
    of: GradeOf,
    key: Picked,
    start: u8,
    end: u8,
}

#[derive(Default)]
pub(super) struct GradeKeys {
    mode: KeyMode,
    picked: Vec<Picked>,
    dragging: Option<(Picked, f32)>,
    band: Option<(Option<u32>, f32)>,
    aim: Option<Aim>,
    dynamics: Option<Dynamics>,
}

impl GradeKeys {
    pub(super) fn forget(&mut self) {
        self.picked.clear();
        self.dragging = None;
        self.band = None;
        self.aim = None;
    }

    fn frames_in(&self, track: Option<u32>) -> Vec<i64> {
        (self.picked.iter())
            .filter(|k| k.node == track)
            .map(|k| k.frame)
            .collect()
    }

    fn shows(&self, track: Option<u32>, frame: i64) -> bool {
        let has = |node| self.picked.contains(&Picked { node, frame });
        has(track) || (track.is_some() && has(None))
    }
}

struct Span {
    lo: f64,
    hi: f64,
    clip: Option<Clip>,
}

impl Span {
    fn of(p: &Project, of: &GradeOf) -> Option<Self> {
        match of {
            GradeOf::Clip(id) => {
                let c = p.clip(id)?;
                Some(Self {
                    lo: c.start as f64,
                    hi: c.end() as f64,
                    clip: Some(c.clone()),
                })
            }
            GradeOf::Timeline => Some(Self {
                lo: 0.0,
                hi: p.end().max(1) as f64,
                clip: None,
            }),
        }
    }

    fn time(&self, frame: i64) -> f64 {
        self.clip
            .as_ref()
            .map_or(frame as f64, |c| c.key_time(frame))
    }
}

#[derive(Clone, Copy)]
struct Map {
    lo: f64,
    hi: f64,
    left: f32,
    width: f32,
}

impl Map {
    fn x(&self, t: f64) -> f32 {
        self.left + ((t - self.lo) / (self.hi - self.lo).max(1.0)) as f32 * self.width
    }

    fn t(&self, x: f32) -> f64 {
        self.lo + f64::from((x - self.left) / self.width.max(1.0)) * (self.hi - self.lo)
    }
}

fn target(of: &GradeOf, more: Value) -> Value {
    let mut v = match of {
        GradeOf::Clip(id) => json!({ "clip": id }),
        GradeOf::Timeline => json!({ "timeline": true }),
    };
    if let (Value::Object(to), Value::Object(from)) = (&mut v, more) {
        to.extend(from);
    }
    v
}

fn clips_target(of: &GradeOf, more: Value) -> Value {
    let mut v = match of {
        GradeOf::Clip(id) => json!({ "clips": id }),
        GradeOf::Timeline => json!({ "timeline": true }),
    };
    if let (Value::Object(to), Value::Object(from)) = (&mut v, more) {
        to.extend(from);
    }
    v
}

fn with_node(node: Option<u32>, more: Value) -> Value {
    let mut more = more;
    if let Some(n) = node {
        more["node"] = json!(n);
    }
    more
}

fn keyed_nodes(g: &Grade) -> Vec<u32> {
    (g.nodes.iter().zip(1u32..))
        .filter(|(d, _)| d.takes_keyframes())
        .map(|(_, n)| n)
        .collect()
}

fn keys_of(g: &Grade, track: Option<u32>) -> Vec<(i64, bool)> {
    match track {
        Some(n) => g.node(n).map_or(Vec::new(), |d| {
            d.keys.iter().map(|k| (k.frame, !k.is_static())).collect()
        }),
        None => g
            .key_frames()
            .into_iter()
            .map(|f| {
                let moving = (g.nodes.iter())
                    .filter_map(|d| d.key_on(f))
                    .any(|k| !k.is_static());
                (f, moving)
            })
            .collect(),
    }
}

fn has_marks(g: &Grade, track: Option<u32>) -> bool {
    (g.nodes.iter().zip(1u32..))
        .filter(|(_, n)| track.is_none_or(|t| t == *n))
        .any(|(d, _)| d.keys.iter().any(|k| k.edl))
}

fn paint_glyph(painter: &Painter, at: Pos2, glyph: &str, tint: Color32) {
    let font = egui::FontId::proportional(11.0);
    painter.text(at, egui::Align2::CENTER_CENTER, glyph, font, tint);
}

fn track_name(track: Option<u32>) -> String {
    match track {
        Some(n) => format!("Node {n:02}"),
        None => "Master".into(),
    }
}

fn tick_frames(fps: f64, ppf: f32) -> i64 {
    let fps = fps.max(1.0);
    [
        1.0,
        5.0,
        10.0,
        fps,
        5.0 * fps,
        10.0 * fps,
        60.0 * fps,
        600.0 * fps,
    ]
    .into_iter()
    .find(|s| *s as f32 * ppf >= 8.0)
    .unwrap_or(3600.0 * fps)
    .round()
    .max(1.0) as i64
}

fn curve(ui: &mut Ui, start: u8, end: u8) {
    let (rect, _) = ui.allocate_exact_size(CURVE, Sense::hover());
    let painter = ui.painter_at(rect);
    let c = theme::colors();
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, c.line),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(8.0);
    let d = Dissolve::of(start, end);
    let points: Vec<Pos2> = (0..=100)
        .map(|i| {
            let u = f64::from(i) / 100.0;
            let w = d.at(u) as f32;
            pos2(
                inner.left() + inner.width() * u as f32,
                inner.bottom() - inner.height() * w,
            )
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.5, c.accent)));
}

impl Workspace {
    fn grade_key_node(&self, g: &Grade) -> Result<Option<u32>, &'static str> {
        match self.effects_page.grade_keys.mode {
            KeyMode::All if keyed_nodes(g).is_empty() => Err("The grade has no corrector"),
            KeyMode::All => Ok(None),
            KeyMode::Color => self
                .current_grade_node(g)
                .filter(|n| g.node(*n).is_some_and(GradeNode::takes_keyframes))
                .map(Some)
                .ok_or("Select a corrector or a key mixer"),
        }
    }

    fn no_grade(&self) -> &'static str {
        match self.effects_page.grading {
            GradeLevel::Clip => "Move the playhead over a clip",
            GradeLevel::Timeline => "Open a timeline",
        }
    }

    pub(super) fn grade_key_cmd_why(&self, cmd: KeyCmd) -> Option<&'static str> {
        let Some((of, _, g)) = self.grade_shown() else {
            return Some(self.no_grade());
        };
        let Some(span) = self.project().and_then(|p| Span::of(p, &of)) else {
            return Some(self.no_grade());
        };
        let play = self.snap.timeline.position as f64;
        match cmd {
            KeyCmd::Add => self.grade_key_node(&g).err(),
            KeyCmd::Delete if !g.key_frames().iter().any(|f| span.time(*f).round() == play) => {
                Some("No keyframe at the playhead")
            }
            KeyCmd::DeleteAll if !g.is_keyed() => Some("The grade has no keyframes"),
            _ => None,
        }
    }

    pub(super) fn grade_key_times(&self) -> Option<Vec<i64>> {
        if self.effects_page.page != Page::Color {
            return None;
        }
        let (of, _, g) = self.grade_shown()?;
        let span = Span::of(self.project()?, &of)?;
        let mut times: Vec<i64> = (g.key_frames().into_iter())
            .map(|f| span.time(f).round())
            .filter(|t| (span.lo..span.hi).contains(t))
            .map(|t| t as i64)
            .collect();
        times.sort_unstable();
        times.dedup();
        Some(times)
    }

    pub(super) fn grade_key_cmd(&mut self, cmd: KeyCmd) {
        if self.grade_key_cmd_why(cmd).is_some() {
            return;
        }
        let Some((of, _, _)) = self.grade_shown() else {
            return;
        };
        match cmd {
            KeyCmd::Add => self.add_grade_key(GradeKeyKind::Dynamic),
            KeyCmd::Delete => {
                let at = self.snap.timeline.position;
                self.send("grade.keys.remove", target(&of, json!({ "at": at })));
            }
            KeyCmd::DeleteAll => self.send("grade.keys.remove", target(&of, json!({}))),
        }
        self.effects_page.grade_keys.forget();
    }

    pub(super) fn add_grade_key(&mut self, kind: GradeKeyKind) {
        let Some((of, _, g)) = self.grade_shown() else {
            return;
        };
        self.settle_grade(&of, &g);
        let Ok(node) = self.grade_key_node(&g) else {
            return;
        };
        let at = self.snap.timeline.position;
        let more = with_node(node, json!({ "kind": kind, "at": at }));
        self.send("grade.keys.add", target(&of, more));
    }

    pub(super) fn static_key_item(&mut self, ui: &mut Ui) {
        if self.effects_page.page != Page::Color {
            return;
        }
        let why = self.grade_key_cmd_why(KeyCmd::Add);
        let mut button = egui::Button::new("Make Static Keyframe");
        if let Some(key) = self.snap.bindings.first(Action::AddStaticKeyframe) {
            button = button.shortcut_text(key);
        }
        let r = ui.add_enabled(why.is_none(), button);
        let r = match why {
            Some(why) => r.on_disabled_hover_text(why),
            None => r,
        };
        if r.clicked() {
            self.add_grade_key(GradeKeyKind::Static);
            ui.close();
        }
    }

    pub(super) fn grade_keys_panel(&mut self, ui: &mut Ui) {
        self.dynamics_window(ui.ctx());
        let Some((of, _, g)) = self.grade_shown() else {
            ui.strong("Keyframes");
            return;
        };
        self.settle_grade(&of, &g);
        let Some(span) = self.project().and_then(|p| Span::of(p, &of)) else {
            return;
        };
        let mut mode = self.effects_page.grade_keys.mode;
        ui.horizontal(|ui| {
            ui.strong("Keyframes");
            ui.separator();
            super::dialog::switch(ui, |ui| {
                for (m, text) in [(KeyMode::All, "All"), (KeyMode::Color, "Color")] {
                    if ui.selectable_label(mode == m, text).clicked() {
                        mode = m;
                    }
                }
            });
        });
        self.effects_page.grade_keys.mode = mode;
        let current = self
            .current_grade_node(&g)
            .filter(|n| g.node(*n).is_some_and(GradeNode::takes_keyframes));
        let tracks: Vec<Option<u32>> = std::iter::once(None)
            .chain(keyed_nodes(&g).into_iter().map(Some))
            .collect();
        let mut act = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                act = self.key_tracks(ui, (&of, &g, &span), &tracks, current);
            });
        if let Some(act) = act {
            self.grade_key_act(&of, &g, act);
        }
    }

    fn key_tracks(
        &mut self,
        ui: &mut Ui,
        (of, g, span): (&GradeOf, &Grade, &Span),
        tracks: &[Option<u32>],
        current: Option<u32>,
    ) -> Option<KeyAct> {
        let c = theme::colors();
        let height = RULER + ROW * tracks.len() as f32;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let left = rect.left() + LABEL;
        let map = Map {
            lo: span.lo,
            hi: span.hi,
            left: left + INSET,
            width: (rect.right() - left - 2.0 * INSET).max(1.0),
        };
        let painter = ui.painter_at(rect);
        let play = self.snap.timeline.position;
        let font = egui::FontId::proportional(11.0);
        let ruler = Rect::from_min_max(
            pos2(left, rect.top()),
            pos2(rect.right(), rect.top() + RULER),
        );
        painter.text(
            pos2(rect.left() + 2.0, ruler.center().y),
            egui::Align2::LEFT_CENTER,
            self.tc(play),
            font.clone(),
            c.text_dim,
        );
        let step = tick_frames(
            self.fps().as_f64(),
            map.width / (span.hi - span.lo).max(1.0) as f32,
        );
        let mut t = (span.lo as i64).div_euclid(step) * step;
        while (t as f64) <= span.hi {
            if (t as f64) >= span.lo {
                let x = map.x(t as f64);
                painter.line_segment(
                    [pos2(x, ruler.bottom() - 5.0), pos2(x, ruler.bottom())],
                    Stroke::new(1.0, c.line),
                );
            }
            t += step;
        }
        let r = ui.interact(
            ruler,
            ui.scope_id().with("grade-key-ruler"),
            Sense::click_and_drag(),
        );
        if (r.clicked() || r.dragged())
            && let Some(p) = r.interact_pointer_pos()
        {
            let at = map.t(p.x).round().clamp(span.lo, span.hi - 1.0) as i64;
            self.seek(PlayerId::Timeline, at, r.dragged());
        }
        let mut act = None;
        let timeline = *of == GradeOf::Timeline;
        for (i, &track) in tracks.iter().enumerate() {
            let top = rect.top() + RULER + ROW * i as f32;
            let row = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), top + ROW));
            let lane = Rect::from_min_max(pos2(left, top), row.max);
            painter.line_segment([row.left_top(), row.right_top()], Stroke::new(1.0, c.line));
            let bar = match (self.effects_page.grade_keys.mode, track) {
                (KeyMode::All, None) => Some(ALL_BAR),
                (KeyMode::Color, Some(n)) if Some(n) == current => Some(COLOR_BAR),
                _ => None,
            };
            if let Some(bar) = bar {
                let mark = Rect::from_min_size(row.left_top(), vec2(3.0, ROW));
                painter.rect_filled(mark, 0.0, bar);
            }
            painter.text(
                pos2(row.left() + 6.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                track_name(track),
                font.clone(),
                c.text,
            );
            if let Some((n, d)) = track.and_then(|n| Some((n, g.node(n)?))) {
                let flags = [
                    (Flag::On, d.on),
                    (Flag::Locked, d.locked),
                    (Flag::AutoKey, d.auto_key),
                ];
                for (j, (flag, lit)) in flags.into_iter().enumerate() {
                    let at = pos2(left - BUTTON * (3 - j) as f32, row.center().y);
                    let spot = Rect::from_center_size(at, vec2(BUTTON - 2.0, ROW - 2.0));
                    let id = ui.scope_id().with(("grade-key-flag", i, j));
                    let b = ui
                        .interact(spot, id, Sense::click())
                        .on_hover_text(flag.tip());
                    let tint = match (flag, lit) {
                        (Flag::AutoKey, true) => theme::KEY_ON,
                        (_, true) => c.text,
                        (_, false) => c.text_dim,
                    };
                    match (flag, lit) {
                        (Flag::On, true) => {
                            painter.circle_filled(at, 4.0, tint);
                        }
                        (Flag::On, false) => {
                            painter.circle_stroke(at, 4.0, Stroke::new(1.0, tint));
                        }
                        (Flag::Locked, true) => paint_glyph(&painter, at, icon::LOCK_SIMPLE, tint),
                        (Flag::Locked, false) => {
                            paint_glyph(&painter, at, icon::LOCK_SIMPLE_OPEN, tint)
                        }
                        (Flag::AutoKey, _) => paint_glyph(&painter, at, icon::DIAMOND, tint),
                    }
                    if b.clicked() {
                        act = Some(KeyAct::Node(n, flag, !lit));
                    }
                }
            }
            let keys = keys_of(g, track);
            let x_of = |f: i64| map.x(span.time(f));
            let hit = |p: Pos2| {
                keys.iter()
                    .map(|(f, _)| *f)
                    .filter(|f| (x_of(*f) - p.x).abs() <= NEAR)
                    .min_by(|a, b| (x_of(*a) - p.x).abs().total_cmp(&(x_of(*b) - p.x).abs()))
            };
            let state = &self.effects_page.grade_keys;
            for &(f, moving) in &keys {
                let at = pos2(x_of(f), row.center().y);
                if !lane.x_range().contains(at.x) {
                    continue;
                }
                let on = state.shows(track, f);
                let color = if on { PICKED } else { c.text };
                match moving {
                    true => diamond(&painter, at, if on { 6.0 } else { 4.5 }, color),
                    false => dot(&painter, at, if on { 5.0 } else { 3.5 }, color),
                }
            }
            let r = ui.interact(
                lane,
                ui.scope_id().with(("grade-key-lane", i)),
                Sense::click_and_drag(),
            );
            let pointer = r.interact_pointer_pos().or(r.hover_pos());
            let keys_state = &mut self.effects_page.grade_keys;
            let origin = match r.drag_started() {
                true => ui.input(|i| i.pointer.press_origin()).or(pointer),
                false => None,
            };
            if let Some(o) = origin {
                match hit(o) {
                    Some(f) => {
                        let picked = Picked {
                            node: track,
                            frame: f,
                        };
                        if !keys_state.picked.contains(&picked) {
                            keys_state.picked = vec![picked];
                        }
                        keys_state.dragging = Some((picked, o.x));
                    }
                    None => {
                        keys_state.picked.clear();
                        keys_state.band = Some((track, o.x));
                    }
                }
            }
            if let Some((_, from)) = keys_state.band.filter(|(t, _)| *t == track)
                && let Some(p) = pointer
            {
                let (a, b) = (from.min(p.x), from.max(p.x));
                let band = Rect::from_min_max(
                    pos2(a.max(lane.left()), row.top() + 1.0),
                    pos2(b.min(lane.right()), row.bottom() - 1.0),
                );
                painter.rect(
                    band,
                    0.0,
                    BAND,
                    Stroke::new(1.0, c.text_dim),
                    egui::StrokeKind::Inside,
                );
                if r.drag_stopped() {
                    keys_state.band = None;
                    keys_state.picked = (keys.iter())
                        .filter(|(f, _)| (a..=b).contains(&x_of(*f)))
                        .map(|(f, _)| Picked {
                            node: track,
                            frame: *f,
                        })
                        .collect();
                }
            }
            if let Some((d, from)) = keys_state.dragging.filter(|(d, _)| d.node == track)
                && let Some(p) = pointer
            {
                let moving = keys_state.frames_in(track);
                for &f in &moving {
                    let x = x_of(f) + p.x - from;
                    let ghost = pos2(x.clamp(lane.left(), lane.right()), row.center().y);
                    diamond(&painter, ghost, 4.0, PICKED.gamma_multiply(0.5));
                }
                if r.drag_stopped() {
                    keys_state.dragging = None;
                    let by = (map.t(p.x) - map.t(from)).round() as i64;
                    if by != 0 {
                        let more = with_node(d.node, json!({ "frames": moving, "by": by }));
                        self.send("grade.keys.move", target(of, more));
                    }
                }
            }
            let keys_state = &mut self.effects_page.grade_keys;
            if r.clicked() {
                keys_state.picked = (pointer.and_then(hit).into_iter())
                    .map(|f| Picked {
                        node: track,
                        frame: f,
                    })
                    .collect();
            }
            if r.secondary_clicked() {
                keys_state.aim = Some(match pointer.and_then(hit) {
                    Some(f) => {
                        let picked = Picked {
                            node: track,
                            frame: f,
                        };
                        if !keys_state.picked.contains(&picked) {
                            keys_state.picked = vec![picked];
                        }
                        Aim::Key(picked)
                    }
                    None => Aim::Track(track),
                });
            }
            let aim = keys_state.aim;
            let frames = keys_state.frames_in(track);
            let marks = timeline.then(|| has_marks(g, track));
            r.context_menu(|ui| {
                let chosen = match aim {
                    Some(Aim::Key(k)) if k.node == track => key_menu(ui, g, k, &frames),
                    Some(Aim::Track(t)) if t == track => track_menu(ui, t, marks),
                    _ => None,
                };
                if chosen.is_some() {
                    act = chosen;
                    ui.close();
                }
            });
        }
        let x = map.x(play as f64);
        if (left..=rect.right()).contains(&x) {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0, c.playhead),
            );
        }
        act
    }

    fn grade_key_act(&mut self, of: &GradeOf, g: &Grade, act: KeyAct) {
        let (method, more) = match act {
            KeyAct::Add(node, kind) => {
                let at = self.snap.timeline.position;
                (
                    "grade.keys.add",
                    with_node(node, json!({ "kind": kind, "at": at })),
                )
            }
            KeyAct::Kind(node, frames, kind) => (
                "grade.keys.set",
                with_node(node, json!({ "frames": frames, "kind": kind })),
            ),
            KeyAct::Delete(node, frames) => (
                "grade.keys.remove",
                with_node(node, json!({ "frames": frames })),
            ),
            KeyAct::Marks(node, true) => {
                ("grade.keys.add", with_node(node, json!({ "edl": true })))
            }
            KeyAct::Marks(node, false) => {
                ("grade.keys.remove", with_node(node, json!({ "edl": true })))
            }
            KeyAct::Node(n, flag, on) => {
                let more = json!({ "node": n, flag.field(): on });
                self.send("grade.set", clips_target(of, more));
                return;
            }
            KeyAct::Attributes(k) => {
                let key = (g.nodes.iter().zip(1u32..))
                    .filter(|(_, n)| k.node.is_none_or(|m| m == *n))
                    .find_map(|(d, _)| d.key_on(k.frame));
                if let Some(key) = key {
                    self.effects_page.grade_keys.dynamics = Some(Dynamics {
                        of: of.clone(),
                        key: k,
                        start: key.start,
                        end: key.end,
                    });
                }
                return;
            }
        };
        self.effects_page.grade_keys.forget();
        self.send(method, target(of, more));
    }

    fn dynamics_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.effects_page.grade_keys.dynamics.take() else {
            return;
        };
        let mut open = true;
        let (mut ok, mut cancel) = (false, false);
        super::dialog::window(ctx, "Dynamic Attributes", &mut open, |ui| {
            super::dialog::form(ui, "dynamics-form", |ui| {
                super::dialog::row(ui, "Start", |ui| {
                    ui.add(egui::Slider::new(&mut d.start, 0..=LAST_PROFILE));
                });
                super::dialog::row(ui, "End", |ui| {
                    ui.add(egui::Slider::new(&mut d.end, 0..=LAST_PROFILE));
                });
            });
            curve(ui, d.start, d.end);
            super::dialog::footer(ui, |ui| {
                ok = super::dialog::main_button(ui, "OK", true);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if ok {
            let more = json!({ "frames": [d.key.frame], "start": d.start, "end": d.end });
            self.send("grade.keys.set", target(&d.of, with_node(d.key.node, more)));
        } else if open && !cancel {
            self.effects_page.grade_keys.dynamics = Some(d);
        }
    }
}

fn key_menu(ui: &mut Ui, g: &Grade, k: Picked, frames: &[i64]) -> Option<KeyAct> {
    let kinds: Vec<bool> = (g.nodes.iter().zip(1u32..))
        .filter(|(_, n)| k.node.is_none_or(|m| m == *n))
        .flat_map(|(d, _)| frames.iter().filter_map(|f| d.key_on(*f)))
        .map(|key| key.is_static())
        .collect();
    let mut act = None;
    if kinds.iter().any(|s| *s) && ui.button("Change to Dynamic Keyframe").clicked() {
        act = Some(KeyAct::Kind(k.node, frames.to_vec(), GradeKeyKind::Dynamic));
    }
    if kinds.iter().any(|s| !*s) && ui.button("Change to Static Keyframe").clicked() {
        act = Some(KeyAct::Kind(k.node, frames.to_vec(), GradeKeyKind::Static));
    }
    if ui.button("Change Dynamic Attributes").clicked() {
        act = Some(KeyAct::Attributes(k));
    }
    ui.separator();
    if ui.button("Delete Selected Keyframe").clicked() {
        act = Some(KeyAct::Delete(k.node, frames.to_vec()));
    }
    act
}

fn track_menu(ui: &mut Ui, track: Option<u32>, marks: Option<bool>) -> Option<KeyAct> {
    let mut act = None;
    if ui.button("Add Static Keyframe").clicked() {
        act = Some(KeyAct::Add(track, GradeKeyKind::Static));
    }
    if ui.button("Add Dynamic Keyframe").clicked() {
        act = Some(KeyAct::Add(track, GradeKeyKind::Dynamic));
    }
    if let Some(has) = marks {
        ui.separator();
        if ui.button("Add EDL Marks on Tracks").clicked() {
            act = Some(KeyAct::Marks(track, true));
        }
        let delete = ui.add_enabled(has, egui::Button::new("Delete EDL Marks on Tracks"));
        if delete.clicked() {
            act = Some(KeyAct::Marks(track, false));
        }
    }
    act
}
