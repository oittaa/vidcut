use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, CursorIcon, Event, EventFilter, ImeEvent, Key, Modifiers, Pos2, Rect, Sense, Stroke,
    pos2, vec2,
};
use egui::text_selection::text_cursor_state::is_word_char;
use vidcut_av::burn::Letters;
use vidcut_core::rich;

const UNDO_KEPT: usize = 100;
const MERGE: Duration = Duration::from_secs(1);
const CARET: f32 = 2.0;
const SHADE: f32 = 0.45;
const EMPTY_SHARE: f32 = 0.3;

#[derive(Debug, Clone, Copy)]
struct Spot {
    at: usize,
    x: [f32; 2],
    y: [f32; 2],
    rtl: bool,
}

impl Spot {
    fn lead(&self) -> f32 {
        if self.rtl { self.x[1] } else { self.x[0] }
    }

    fn trail(&self) -> f32 {
        if self.rtl { self.x[0] } else { self.x[1] }
    }
}

#[derive(Debug, Clone, Copy)]
struct Turn {
    pivot: Pos2,
    sin: f32,
    cos: f32,
}

impl Turn {
    fn on(self, x: f32, y: f32) -> Pos2 {
        let d = pos2(x, y) - self.pivot;
        self.pivot
            + vec2(
                d.x * self.cos + d.y * self.sin,
                -d.x * self.sin + d.y * self.cos,
            )
    }

    fn back(self, p: Pos2) -> Pos2 {
        let d = p - self.pivot;
        self.pivot
            + vec2(
                d.x * self.cos - d.y * self.sin,
                d.x * self.sin + d.y * self.cos,
            )
    }
}

#[derive(Debug)]
struct Line {
    turn: Turn,
    y: [f32; 2],
    spots: Vec<Spot>,
}

impl Line {
    fn first(&self) -> usize {
        self.spots.iter().map(|s| s.at).min().unwrap_or(0)
    }

    fn empty(&self) -> bool {
        matches!(&self.spots[..], [only] if only.x[0] == only.x[1])
    }

    fn stops(&self) -> Vec<(usize, f32)> {
        if self.empty() {
            return self.spots.iter().map(|s| (s.at, s.x[0])).collect();
        }
        self.spots
            .iter()
            .flat_map(|s| [(s.at, s.lead()), (s.at + 1, s.trail())])
            .collect()
    }

    fn nearest(&self, x: f32) -> usize {
        self.stops()
            .into_iter()
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .map_or(0, |s| s.0)
    }

    fn ends(&self) -> (usize, usize) {
        let stops = self.stops();
        let lo = stops.iter().map(|s| s.0).min().unwrap_or(0);
        let hi = stops.iter().map(|s| s.0).max().unwrap_or(0);
        (lo, hi)
    }

    fn gap(&self, p: Pos2) -> f32 {
        let q = self.turn.back(p);
        (self.y[0] - q.y).max(q.y - self.y[1]).max(0.0)
    }
}

#[derive(Debug)]
pub struct Laid {
    lines: Vec<Line>,
    home: [Pos2; 2],
}

impl Laid {
    pub fn new(
        all: &[Letters],
        picture: Rect,
        size: (f32, f32),
        drop: impl Fn(&Letters) -> f32,
        home: [Pos2; 2],
    ) -> Self {
        let scale = vec2(picture.width() / size.0, picture.height() / size.1);
        let mut lines: Vec<Line> = Vec::new();
        for b in all {
            let dy = drop(b);
            let (sin, cos) = (b.turn.to_radians() as f32).sin_cos();
            let turn = Turn {
                pivot: picture.min + vec2(b.pivot[0] * scale.x, b.pivot[1] * scale.y),
                sin,
                cos,
            };
            let mut open: Option<Line> = None;
            for l in &b.letters {
                let spot = Spot {
                    at: l.at,
                    x: l.x.map(|v| picture.min.x + v * scale.x),
                    y: l.y.map(|v| picture.min.y + (v + dy) * scale.y),
                    rtl: l.rtl,
                };
                match &mut open {
                    Some(line) if line.y == spot.y => line.spots.push(spot),
                    _ => {
                        lines.extend(open.take());
                        open = Some(Line {
                            turn,
                            y: spot.y,
                            spots: vec![spot],
                        });
                    }
                }
            }
            lines.extend(open);
        }
        lines.sort_by_key(Line::first);
        Self { lines, home }
    }

    fn caret(&self, c: usize) -> Option<(usize, f32)> {
        let mut before: Option<(usize, f32, usize)> = None;
        for (i, l) in self.lines.iter().enumerate() {
            for s in &l.spots {
                if s.at == c {
                    return Some((i, s.lead()));
                }
                if s.at < c && before.is_none_or(|b| s.at >= b.2) {
                    before = Some((i, s.trail(), s.at));
                }
            }
        }
        before
            .map(|(i, x, _)| (i, x))
            .or_else(|| Some((0, self.lines.first()?.spots.first()?.lead())))
    }

    fn caret_ends(&self, c: usize) -> [Pos2; 2] {
        match self.caret(c) {
            Some((i, x)) => {
                let l = &self.lines[i];
                [l.turn.on(x, l.y[0]), l.turn.on(x, l.y[1])]
            }
            None => self.home,
        }
    }

    fn hit(&self, p: Pos2) -> Option<usize> {
        let line = self
            .lines
            .iter()
            .min_by(|a, b| a.gap(p).total_cmp(&b.gap(p)))?;
        Some(line.nearest(line.turn.back(p).x))
    }

    fn column(&self, c: usize) -> Option<f32> {
        self.caret(c).map(|(_, x)| x)
    }

    fn vertical(&self, c: usize, down: bool, column: f32) -> Option<usize> {
        let (i, _) = self.caret(c)?;
        let to = if down { i + 1 } else { i.checked_sub(1)? };
        self.lines.get(to).map(|l| l.nearest(column))
    }

    fn line_ends(&self, c: usize) -> Option<(usize, usize)> {
        let (i, _) = self.caret(c)?;
        Some(self.lines[i].ends())
    }

    fn paint(&self, painter: &egui::Painter, (a, b): (usize, usize), fill: egui::Color32) {
        for l in &self.lines {
            for s in l.spots.iter().filter(|s| a <= s.at && s.at < b) {
                let x = if s.x[0] == s.x[1] {
                    [s.x[0], s.x[0] + (l.y[1] - l.y[0]) * EMPTY_SHARE]
                } else {
                    s.x
                };
                let points = vec![
                    l.turn.on(x[0], l.y[0]),
                    l.turn.on(x[1], l.y[0]),
                    l.turn.on(x[1], l.y[1]),
                    l.turn.on(x[0], l.y[1]),
                ];
                painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Step {
    text: String,
    caret: usize,
    anchor: usize,
}

#[derive(Debug)]
pub struct Typing {
    pub clip: String,
    caret: usize,
    anchor: usize,
    press: Option<Pos2>,
    column: Option<f32>,
    preedit: Option<(usize, usize)>,
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    typed_at: Option<Instant>,
    fresh: bool,
    held: bool,
    since: Instant,
}

fn next_word(chars: &[char], mut c: usize) -> usize {
    while c < chars.len() && !is_word_char(chars[c]) {
        c += 1;
    }
    while c < chars.len() && is_word_char(chars[c]) {
        c += 1;
    }
    c
}

fn previous_word(chars: &[char], mut c: usize) -> usize {
    while c > 0 && !is_word_char(chars[c - 1]) {
        c -= 1;
    }
    while c > 0 && is_word_char(chars[c - 1]) {
        c -= 1;
    }
    c
}

fn word_at(chars: &[char], c: usize) -> (usize, usize) {
    let c = c.min(chars.len());
    let word = |i: usize| chars.get(i).is_some_and(|ch| is_word_char(*ch));
    if !word(c) && !(c > 0 && word(c - 1)) {
        return (c, (c + 1).min(chars.len()));
    }
    let mut a = c;
    while a > 0 && word(a - 1) {
        a -= 1;
    }
    let mut b = c;
    while word(b) {
        b += 1;
    }
    (a, b)
}

fn printable(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(char::is_control)
}

impl Typing {
    pub fn new(clip: &str, press: Option<Pos2>) -> Self {
        Self {
            clip: clip.to_string(),
            caret: usize::MAX,
            anchor: usize::MAX,
            press,
            column: None,
            preedit: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
            typed_at: None,
            fresh: true,
            held: false,
            since: Instant::now(),
        }
    }

    fn range(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    fn to(&mut self, c: usize, extend: bool) {
        self.caret = c;
        if !extend {
            self.anchor = c;
        }
        self.since = Instant::now();
    }

    fn remember(&mut self, text: &str, merge: bool) {
        let now = Instant::now();
        let joined = merge && self.typed_at.is_some_and(|t| now - t < MERGE);
        self.typed_at = merge.then_some(now);
        if joined {
            return;
        }
        self.undo.push_back(Step {
            text: text.to_string(),
            caret: self.caret,
            anchor: self.anchor,
        });
        while self.undo.len() > UNDO_KEPT {
            self.undo.pop_front();
        }
        self.redo.clear();
    }

    fn put(&mut self, text: &mut String, (a, b): (usize, usize), with: &str, merge: bool) {
        if a == b && with.is_empty() {
            return;
        }
        self.remember(text, merge && a == b && !with.contains(char::is_whitespace));
        *text = rich::edited(text, (a, b), with);
        self.to(a + with.chars().count(), false);
    }

    fn back_to(&mut self, text: &mut String, step: Step, redo: bool) {
        let now = Step {
            text: std::mem::replace(text, step.text),
            caret: self.caret,
            anchor: self.anchor,
        };
        if redo {
            self.undo.push_back(now);
        } else {
            self.redo.push(now);
        }
        self.caret = step.caret;
        self.anchor = step.anchor;
        self.typed_at = None;
        self.since = Instant::now();
    }

    fn key(&mut self, text: &mut String, laid: &Laid, key: Key, m: Modifiers) -> bool {
        let chars: Vec<char> = rich::plain(text).chars().collect();
        let n = chars.len();
        let (a, b) = self.range();
        let word = m.alt || m.ctrl;
        match key {
            Key::ArrowLeft | Key::ArrowRight => {
                let right = key == Key::ArrowRight;
                let to = if m.mac_cmd {
                    let (lo, hi) = laid.line_ends(self.caret).unwrap_or((0, n));
                    if right { hi } else { lo }
                } else if word {
                    if right {
                        next_word(&chars, self.caret)
                    } else {
                        previous_word(&chars, self.caret)
                    }
                } else if a != b && !m.shift {
                    if right { b } else { a }
                } else if right {
                    (self.caret + 1).min(n)
                } else {
                    self.caret.saturating_sub(1)
                };
                self.to(to, m.shift);
                self.column = None;
            }
            Key::ArrowUp | Key::ArrowDown => {
                let down = key == Key::ArrowDown;
                let column = self.column.or_else(|| laid.column(self.caret));
                let to = match column.filter(|_| !m.command) {
                    Some(x) => laid.vertical(self.caret, down, x),
                    None => None,
                };
                self.to(to.unwrap_or(if down { n } else { 0 }), m.shift);
                self.column = column;
            }
            Key::Home | Key::End => {
                let end = key == Key::End;
                let (lo, hi) = match laid.line_ends(self.caret) {
                    Some(ends) if !m.command => ends,
                    _ => (0, n),
                };
                self.to(if end { hi } else { lo }, m.shift);
                self.column = None;
            }
            Key::Backspace | Key::Delete => {
                let range = if a != b {
                    (a, b)
                } else if key == Key::Backspace {
                    let from = if word {
                        previous_word(&chars, a)
                    } else {
                        a.saturating_sub(1)
                    };
                    (from, a)
                } else {
                    let to = if word { next_word(&chars, a) } else { a + 1 };
                    (a, to.min(n))
                };
                self.put(text, range, "", false);
            }
            Key::Enter => self.put(text, (a, b), "\n", false),
            Key::A if m.command => {
                self.anchor = 0;
                self.to(n, true);
            }
            Key::Z if m.command && !m.shift => {
                if let Some(step) = self.undo.pop_back() {
                    self.back_to(text, step, false);
                }
            }
            Key::Z | Key::Y if m.command => {
                if let Some(step) = self.redo.pop() {
                    self.back_to(text, step, true);
                }
            }
            Key::Escape => return true,
            _ => {}
        }
        false
    }

    fn ime(&mut self, text: &mut String, e: &ImeEvent) {
        let range = self.preedit.unwrap_or_else(|| self.range());
        match e {
            ImeEvent::Preedit { text: s, .. } => {
                if s.is_empty() && self.preedit.is_none() {
                    return;
                }
                if self.preedit.is_none() {
                    self.remember(text, false);
                }
                *text = rich::edited(text, range, s);
                let end = range.0 + s.chars().count();
                self.preedit = (!s.is_empty()).then_some((range.0, end));
                self.to(end, false);
            }
            ImeEvent::Commit(s) => {
                if s.is_empty() && self.preedit.is_none() {
                    return;
                }
                if self.preedit.is_none() {
                    self.remember(text, false);
                }
                *text = rich::edited(text, range, s);
                self.preedit = None;
                self.to(range.0 + s.chars().count(), false);
            }
            ImeEvent::DeleteSurrounding {
                before_chars,
                after_chars,
            } if self.caret == self.anchor => {
                let n = rich::plain(text).chars().count();
                let from = self.caret.saturating_sub(*before_chars);
                let to = (self.caret + after_chars).min(n);
                self.put(text, (from, to), "", false);
            }
            _ => {}
        }
    }

    fn events(&mut self, ui: &egui::Ui, text: &mut String, laid: &Laid) -> bool {
        let events = ui.input_mut(|i| {
            let (mine, rest): (Vec<Event>, Vec<Event>) =
                std::mem::take(&mut i.events).into_iter().partition(|e| {
                    matches!(
                        e,
                        Event::Text(_)
                            | Event::Key { .. }
                            | Event::Copy
                            | Event::Cut
                            | Event::Paste(_)
                            | Event::Ime(_)
                    )
                });
            i.events = rest;
            mine
        });
        let mut done = false;
        for e in events {
            let (a, b) = self.range();
            match e {
                Event::Text(s) if printable(&s) => self.put(text, (a, b), &s, true),
                Event::Paste(s) => {
                    let s = s.replace("\r\n", "\n").replace('\r', "\n");
                    self.put(text, (a, b), &s, false);
                }
                Event::Copy | Event::Cut if a != b => {
                    let chosen: String = rich::plain(text).chars().skip(a).take(b - a).collect();
                    ui.ctx().copy_text(chosen);
                    if matches!(e, Event::Cut) {
                        self.put(text, (a, b), "", false);
                    }
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => done |= self.key(text, laid, key, modifiers),
                Event::Ime(ime) => self.ime(text, &ime),
                _ => {}
            }
        }
        done
    }

    fn mouse(&mut self, ui: &egui::Ui, resp: &egui::Response, text: &str, laid: &Laid) {
        if let Some(p) = self.press.take()
            && let Some(c) = laid.hit(p)
        {
            self.to(c, false);
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let shift = ui.input(|i| i.modifiers.shift);
        if resp.is_pointer_button_down_on()
            && ui.input(|i| i.pointer.primary_pressed())
            && let Some(c) = pointer.and_then(|p| laid.hit(p))
        {
            self.to(c, shift);
            self.held = true;
            self.column = None;
        }
        if self.held
            && resp.dragged()
            && let Some(c) = pointer.and_then(|p| laid.hit(p))
        {
            self.to(c, true);
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            self.held = false;
        }
        let chars: Vec<char> = rich::plain(text).chars().collect();
        if resp.triple_clicked() {
            self.anchor = 0;
            self.to(chars.len(), true);
        } else if resp.double_clicked() {
            let (a, b) = word_at(&chars, self.caret);
            self.anchor = a;
            self.to(b, true);
        }
    }

    pub fn frame(
        &mut self,
        ui: &egui::Ui,
        text: &str,
        laid: &Laid,
        outline: [Pos2; 4],
    ) -> (Option<String>, bool) {
        let id = ui.scope_id().with(("title-text", &self.clip));
        let rect = Rect::from_points(&outline).expand(CARET * 2.0);
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        let n = rich::plain(text).chars().count();
        self.caret = self.caret.min(n);
        self.anchor = self.anchor.min(n);
        if self.fresh {
            resp.request_focus();
        }
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
        let outside = ui.input(|i| i.pointer.any_pressed()) && !resp.hovered();
        if !self.fresh && (!resp.has_focus() || outside) {
            ui.memory_mut(|m| m.surrender_focus(id));
            return (None, true);
        }
        self.fresh = false;
        self.mouse(ui, &resp, text, laid);
        let mut edited = text.to_string();
        let done = self.events(ui, &mut edited, laid);
        let changed = (edited != text).then_some(edited);
        if done {
            ui.memory_mut(|m| m.surrender_focus(id));
            return (changed, true);
        }
        (changed, false)
    }

    pub fn paint(&self, ui: &egui::Ui, laid: &Laid, outline: [Pos2; 4], clip: Rect) {
        let painter = ui.painter_at(clip);
        let visuals = ui.visuals();
        let fill = visuals.selection.bg_fill.gamma_multiply(SHADE);
        laid.paint(&painter, self.range(), fill);
        let style = &visuals.text_cursor;
        let ends = laid.caret_ends(self.caret);
        let on = if style.blink {
            let period = style.on_duration + style.off_duration;
            let t = self.since.elapsed().as_secs_f32() % period;
            let wait = if t < style.on_duration {
                style.on_duration - t
            } else {
                period - t
            };
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f32(wait.max(0.01)));
            t < style.on_duration
        } else {
            true
        };
        if on {
            painter.line_segment(ends, Stroke::new(CARET, style.stroke.color));
        }
        let rect = Rect::from_points(&outline);
        let cursor_rect = Rect::from_points(&ends).expand(CARET / 2.0);
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Normal,
                rect,
                cursor_rect,
                should_interrupt_composition: false,
            });
        });
    }
}
