use std::collections::HashMap;

use eframe::egui::{
    self, Color32, ColorImage, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureId, TextureOptions, vec2,
};
use serde_json::json;
use vidcut_av::burn::Sprite;
use vidcut_core::model::{Project, Title, TitleAlign, TitleKind};

use super::export::Ask;
use super::theme;
use super::workspace::{Workspace, dim};

pub const KINDS: [(&str, TitleKind, Option<TitleAlign>); 5] = [
    ("Text", TitleKind::Text, None),
    ("L Lower 3rd", TitleKind::LowerThird, Some(TitleAlign::Left)),
    (
        "M Lower 3rd",
        TitleKind::LowerThird,
        Some(TitleAlign::Middle),
    ),
    (
        "R Lower 3rd",
        TitleKind::LowerThird,
        Some(TitleAlign::Right),
    ),
    ("Scroll", TitleKind::Scroll, None),
];

const CARD_WIDTH: f32 = 150.0;
const RENDER_WIDTH: u32 = 320;
const THUMB_HEIGHT: u32 = 90;
const BACKDROP: [u8; 3] = [30, 30, 32];
const STEPS: f64 = 60.0;
const KEPT: usize = 64;

pub fn sample(kind: TitleKind, align: Option<TitleAlign>) -> Title {
    let text = match kind {
        TitleKind::Text => "Title",
        TitleKind::LowerThird => "Name\nTitle",
        TitleKind::Scroll => "Directed by\nName\n\nMusic by\nName",
    };
    Title {
        kind,
        align,
        ..Title::new(text)
    }
}

fn picture(sprite: Option<&Sprite>, (w, h): (u32, u32), dy: i32) -> ColorImage {
    let (w, h) = (w as usize, h as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for px in rgba.as_chunks_mut::<4>().0 {
        px[..3].copy_from_slice(&BACKDROP);
        px[3] = 255;
    }
    if let Some(s) = sprite {
        for sy in 0..s.height as i32 {
            let y = s.y + dy + sy;
            if y < 0 || y >= h as i32 {
                continue;
            }
            for sx in 0..s.width as i32 {
                let x = s.x + sx;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                let p = s.rgba[(sy as usize) * s.width as usize + sx as usize];
                let a = p[3].clamp(0.0, 1.0);
                if a <= 0.0 {
                    continue;
                }
                let at = (y as usize * w + x as usize) * 4;
                for c in 0..3 {
                    let under = f32::from(rgba[at + c]) / 255.0;
                    let v = p[c] * a + under * (1.0 - a);
                    rgba[at + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
    }
    ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

fn scrolled(sprite: Option<&Sprite>, h: u32, fraction: f64) -> i32 {
    sprite.map_or(0, |s| {
        let top = f64::from(h) - fraction * (f64::from(h) + f64::from(s.height));
        top.round() as i32 - s.y
    })
}

struct Card {
    size: (u32, u32),
    sprite: Option<Sprite>,
    texture: TextureHandle,
    fraction: f64,
}

pub struct Browser {
    pub open: bool,
    pub hover_scrub: bool,
    cards: Vec<Option<Card>>,
    hovered: Option<(usize, i64)>,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            open: false,
            hover_scrub: true,
            cards: (0..KINDS.len()).map(|_| None).collect(),
            hovered: None,
        }
    }
}

impl Browser {
    fn card(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        size: (u32, u32),
        fraction: f64,
    ) -> TextureId {
        let (_, kind, align) = KINDS[index];
        let stale = self.cards[index].as_ref().is_none_or(|c| c.size != size);
        if stale {
            let sprite = vidcut_av::burn::title(&sample(kind, align), size.0, size.1)
                .into_iter()
                .next();
            let dy = if kind == TitleKind::Scroll {
                scrolled(sprite.as_ref(), size.1, fraction)
            } else {
                0
            };
            let texture = ctx.load_texture(
                format!("title-card-{index}"),
                picture(sprite.as_ref(), size, dy),
                TextureOptions::LINEAR,
            );
            self.cards[index] = Some(Card {
                size,
                sprite,
                texture,
                fraction,
            });
        }
        let card = self.cards[index].as_mut().expect("made above");
        if kind == TitleKind::Scroll && card.fraction != fraction {
            let dy = scrolled(card.sprite.as_ref(), size.1, fraction);
            card.texture.set(
                picture(card.sprite.as_ref(), size, dy),
                TextureOptions::LINEAR,
            );
            card.fraction = fraction;
        }
        card.texture.id()
    }
}

#[derive(Default)]
pub struct Thumbs {
    kept: Vec<(String, TextureHandle)>,
    shown: HashMap<String, TextureId>,
}

impl Thumbs {
    pub fn prepare(&mut self, ctx: &egui::Context, p: &Project) -> &HashMap<String, TextureId> {
        self.shown.clear();
        let h = THUMB_HEIGHT;
        let w = ((f64::from(h) * f64::from(p.settings.width) / f64::from(p.settings.height.max(1)))
            .round() as u32)
            .max(1);
        for c in p.tracks.iter().flat_map(|t| &t.clips) {
            let Some(t) = &c.title else {
                continue;
            };
            let key = format!("{w}x{h} {}", serde_json::to_string(t).unwrap_or_default());
            let id = match self.kept.iter().position(|(k, _)| *k == key) {
                Some(i) => {
                    let kept = self.kept.remove(i);
                    let id = kept.1.id();
                    self.kept.push(kept);
                    id
                }
                None => {
                    let sprite = vidcut_av::burn::title(t, w, h).into_iter().next();
                    let dy = if t.moves() {
                        scrolled(sprite.as_ref(), h, 0.5)
                    } else {
                        0
                    };
                    let texture =
                        ctx.load_texture("title-thumb", picture(sprite.as_ref(), (w, h), dy), {
                            TextureOptions::LINEAR
                        });
                    let id = texture.id();
                    self.kept.push((key, texture));
                    id
                }
            };
            self.shown.insert(c.id.clone(), id);
        }
        while self.kept.len() > KEPT {
            drop(self.kept.remove(0));
        }
        &self.shown
    }
}

impl Workspace {
    pub(super) fn left_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(!self.titles.open, "Media Pool")
                .clicked()
            {
                self.titles.open = false;
            }
            if ui
                .selectable_label(self.titles.open, "Titles")
                .on_hover_text("Text, lower thirds and scrolls to drag onto the timeline")
                .clicked()
            {
                self.titles.open = true;
            }
        });
    }

    pub(crate) fn add_title_kind(
        &mut self,
        ctx: &egui::Context,
        kind: TitleKind,
        align: Option<TitleAlign>,
    ) {
        let t = sample(kind, align);
        let mut params = json!({ "text": t.text, "at": self.snap.timeline.position });
        if kind != TitleKind::Text {
            params["kind"] = json!(kind);
        }
        if let Some(a) = align {
            params["align"] = json!(a);
        }
        self.timeline.adding_title = Some(Ask::send(&self.live, ctx, "titles.add", params));
    }

    pub(super) fn end_title_hover(&mut self) {
        if self.titles.hovered.take().is_some() {
            self.live.preview_free_title(None);
        }
    }

    pub(super) fn titles_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project_shared() else {
            ui.label(dim("Open a project to add titles"));
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("Titles").strong());
            ui.checkbox(&mut self.titles.hover_scrub, "Hover Scrub Preview")
                .on_hover_text("Show a title in the viewer while the pointer is over it");
        });
        ui.label(dim(
            "Drag a title onto a timeline, or double-click it to add it at the playhead",
        ));
        ui.add_space(6.0);
        let (w, h) = (p.settings.width.max(1), p.settings.height.max(1));
        let size = (
            RENDER_WIDTH,
            ((f64::from(RENDER_WIDTH) * f64::from(h) / f64::from(w)).round() as u32).max(1),
        );
        let card_h = CARD_WIDTH * size.1 as f32 / size.0 as f32;
        let mut hovered = None;
        let mut grabbed = None;
        let mut added = None;
        ui.horizontal_wrapped(|ui| {
            for (i, (label, kind, align)) in KINDS.iter().enumerate() {
                ui.vertical(|ui| {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(CARD_WIDTH, card_h), Sense::click_and_drag());
                    let across = resp.hover_pos().map(|pos| {
                        f64::from(((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0))
                    });
                    let fraction = across
                        .filter(|_| *kind == TitleKind::Scroll)
                        .map_or(0.5, |f| (f * STEPS).round() / STEPS);
                    let tex = self.titles.card(ui.ctx(), i, size, fraction);
                    ui.painter().image(
                        tex,
                        rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    if resp.hovered() {
                        ui.painter().rect_stroke(
                            rect,
                            2.0,
                            Stroke::new(1.5, theme::colors().accent),
                            StrokeKind::Inside,
                        );
                        hovered = Some((i, (fraction * STEPS).round() as i64));
                    }
                    ui.label(*label);
                    if resp.drag_started()
                        && let Some(press) = ui.input(|i| i.pointer.press_origin())
                    {
                        grabbed = Some((*kind, *align, press));
                    }
                    if resp.double_clicked() {
                        added = Some((*kind, *align));
                    }
                });
            }
        });
        if let Some((kind, align, press)) = grabbed {
            self.start_title_drag(&p, kind, align, press);
        }
        if let Some((kind, align)) = added {
            self.add_title_kind(ui.ctx(), kind, align);
        }
        let want = hovered.filter(|_| self.titles.hover_scrub && self.timeline.drag.is_none());
        match want {
            Some(w) if Some(w) != self.titles.hovered => {
                let (_, kind, align) = KINDS[w.0];
                let at = self.snap.timeline.position;
                self.live
                    .preview_free_title(Some((sample(kind, align), at, w.1 as f64 / STEPS)));
                self.titles.hovered = Some(w);
            }
            Some(_) => {}
            None => self.end_title_hover(),
        }
    }
}
