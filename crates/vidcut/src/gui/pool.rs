use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, Pos2, Rangef, Rect, RichText, Sense, Stroke, UiBuilder, Vec2,
    pos2, vec2,
};
use serde_json::json;
use vidcut_core::model::{Media, Project};
use vidcut_core::request::{PoolMode, PoolSort, ViewerMode};
use vidcut_core::shortcuts::Action;

use super::clip_info::scene_shot_take;
use super::theme;
use super::timeline::gesture::Gesture;
use super::title_browser::LeftTab;
use super::workspace::{Workspace, dim, fit};
use crate::live::PoolView;

const TILE: egui::Vec2 = egui::Vec2::new(176.0, 99.0);
const TILE_FULL: egui::Vec2 = egui::Vec2::new(TILE.x, TILE.y + 30.0);
const TILE_GAP: f32 = 8.0;
const POSTER_PART: i64 = 48;
const LIST_COLUMNS: [&str; 9] = [
    "Name",
    "Duration",
    "Date",
    "Resolution",
    "FPS",
    "Codec",
    "Audio",
    "Proxy",
    "Color",
];
const LIST_GAP: Vec2 = Vec2::new(12.0, 4.0);
const STRIP_HEIGHT: f32 = 54.0;
const HEADER: f32 = 20.0;
const EDIT_ICONS: f32 = 8.0;
const EDIT_ICON: f32 = 16.0;
const EDIT_ICON_GAP: f32 = 4.0;
const CARD_HEIGHT: f32 = 96.0;
const CARD_LEAST_WIDTH: f32 = 300.0;
const CARD_MARGIN: f32 = 6.0;
const CARD_PICTURE: egui::Vec2 = egui::Vec2::new(128.0, 72.0);
const CARD_LINES: [f32; 3] = [28.0, 50.0, 72.0];
const CLUSTER_HEADING: f32 = 26.0;
const NO_VALUE: &str = "---";

#[derive(Default)]
pub(super) struct BinsUi {
    pub(super) targets: Vec<(Rect, String)>,
    pub(super) area: Option<Rect>,
    order: Vec<String>,
    rename: Option<Rename>,
    list: Option<ListLayout>,
    places: Vec<Rect>,
    boxing: Option<Boxing>,
}

#[derive(Clone, Copy)]
struct Boxing {
    from: Vec2,
    toggle: bool,
}

#[derive(Clone, Copy)]
enum ListRow<'a> {
    Header,
    Bin(&'a str),
    Media(&'a Media),
}

impl<'a> ListRow<'a> {
    fn kind(self) -> usize {
        match self {
            Self::Header => 0,
            Self::Bin(_) => 1,
            Self::Media(_) => 2,
        }
    }

    fn id(self) -> (&'static str, &'a str) {
        match self {
            Self::Header => ("header", ""),
            Self::Bin(b) => ("bin", b),
            Self::Media(m) => ("media", &m.id),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ListSizes {
    columns: [f32; LIST_COLUMNS.len()],
    rows: [f32; 3],
}

impl ListSizes {
    fn least(ui: &egui::Ui) -> Self {
        let size = ui.spacing().interact_size;
        Self {
            columns: [size.x; LIST_COLUMNS.len()],
            rows: [size.y; 3],
        }
    }
}

struct ListLayout {
    project: Arc<Project>,
    bin: String,
    search: String,
    sort: (PoolSort, bool),
    scale: f32,
    sizes: ListSizes,
}

struct Rename {
    path: String,
    name: String,
    focus: bool,
    new: bool,
}

fn per_row(width: f32) -> usize {
    if width < TILE_FULL.x {
        1
    } else {
        ((width - TILE_FULL.x) / (TILE_FULL.x + TILE_GAP)) as usize + 1
    }
}

fn cards_per_row(width: f32) -> usize {
    (((width + TILE_GAP) / (CARD_LEAST_WIDTH + TILE_GAP)) as usize).max(1)
}

fn file_time(m: &Media) -> Option<crate::when::LocalTime> {
    u64::try_from(m.mtime_ns / 1_000_000)
        .ok()
        .filter(|&ms| ms > 0)
        .and_then(crate::when::local_time)
}

fn cluster(sort: PoolSort, m: &Media) -> Option<String> {
    match sort {
        PoolSort::Name => Some(crate::live::first_letter(&m.name)),
        PoolSort::Date => Some(file_time(m).map_or_else(|| "-".to_string(), |d| d.day())),
        PoolSort::Camera => m.camera.as_deref().map(super::clip_info::camera_label),
        PoolSort::DateTime | PoolSort::Timecode => m.created.clone(),
        PoolSort::Bin => Some(match m.bin.as_str() {
            "" => bin_name("").to_string(),
            path => path.to_string(),
        }),
        PoolSort::SceneShot => m.scene.as_ref().map(|s| format!("Scene {s}")),
        PoolSort::ClipColor => m.color.clone(),
        PoolSort::OnlineStatus => Some(
            match m.fresh_proxy() {
                Some(_) => "Proxy",
                None => "Original",
            }
            .to_string(),
        ),
        PoolSort::Added | PoolSort::Duration => None,
    }
}

fn main_description(sort: PoolSort, m: &Media, start: &str) -> String {
    let date = m.created.as_deref().unwrap_or(NO_VALUE);
    match sort {
        PoolSort::Camera => {
            let camera = m
                .camera
                .as_deref()
                .map_or_else(|| NO_VALUE.to_string(), super::clip_info::camera_label);
            format!("{camera}  {start}")
        }
        PoolSort::DateTime | PoolSort::Date | PoolSort::Added => {
            let file = m.path.file_name().unwrap_or_default().to_string_lossy();
            format!("{date}  {file}")
        }
        PoolSort::SceneShot => {
            let parts: Vec<String> = scene_shot_take(m).into_iter().flatten().collect();
            match parts.is_empty() {
                true => NO_VALUE.to_string(),
                false => parts.join(", "),
            }
        }
        PoolSort::Timecode | PoolSort::ClipColor => format!("{date}  {start}"),
        PoolSort::Name | PoolSort::Duration | PoolSort::Bin | PoolSort::OnlineStatus => {
            m.name.clone()
        }
    }
}

fn card_row(painter: &egui::Painter, x: Rangef, y: f32, cells: &[String], color: Color32) {
    let each = x.span() / cells.len() as f32;
    for (i, cell) in cells.iter().enumerate() {
        let galley = one_line(painter, cell.clone(), 12.0, color, each - CARD_MARGIN);
        let left = match i > 0 && i + 1 == cells.len() {
            true => x.max - galley.size().x,
            false => x.min + i as f32 * each,
        };
        painter.galley(Pos2::new(left, y), galley, color);
    }
}

fn one_line(
    painter: &egui::Painter,
    text: String,
    size: f32,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let font = egui::FontId::proportional(size);
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    painter.layout_job(job)
}

fn shown(view: Rangef, top: f32, step: f32, n: usize) -> Range<usize> {
    let row = |y: f32| ((y - top) / step).max(0.0);
    (row(view.min).floor() as usize).min(n)..(row(view.max).ceil() as usize).min(n)
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(a, _)| a)
}

fn join(bin: &str, name: &str) -> String {
    if bin.is_empty() {
        name.to_string()
    } else {
        format!("{bin}/{name}")
    }
}

fn within(bin: &str, path: &str) -> bool {
    path.is_empty() || bin == path || bin.starts_with(&format!("{path}/"))
}

fn bin_at(targets: &[(Rect, String)], pos: Pos2) -> Option<&str> {
    targets
        .iter()
        .find(|(rect, _)| rect.contains(pos))
        .map(|(_, bin)| bin.as_str())
}

fn bin_drop_to(from: &str, onto: &str) -> Option<String> {
    if from.is_empty() || within(onto, from) || onto == parent(from) {
        None
    } else {
        Some(join(onto, leaf(from)))
    }
}

fn new_bin(p: &Project, bin: &str) -> String {
    (1..)
        .map(|n| join(bin, &format!("Bin {n}")))
        .find(|b| !p.bins.contains(b))
        .unwrap_or_default()
}

fn bin_list(p: &Project) -> Vec<(String, usize)> {
    std::iter::once((String::new(), 0))
        .chain(
            p.bins
                .iter()
                .map(|b| (b.clone(), b.matches('/').count() + 1)),
        )
        .collect()
}

pub(super) fn shown_media<'a>(p: &'a Project, view: &PoolView, search: &str) -> Vec<&'a Media> {
    let needle = search.to_lowercase();
    let pool = p.pool();
    view.media(p)
        .iter()
        .filter_map(|id| pool.get(id.as_str()).copied())
        .filter(|m| {
            if needle.is_empty() {
                m.bin == view.bin
            } else {
                m.name.to_lowercase().contains(&needle)
            }
        })
        .collect()
}

fn chosen(clicked: Option<&str>, selected: &[String]) -> Option<String> {
    clicked
        .filter(|id| selected.iter().any(|s| s == id))
        .map(String::from)
        .or_else(|| selected.last().cloned())
}

fn boxed(selected: &[String], inside: Vec<String>, toggle: bool) -> Vec<String> {
    if !toggle {
        return inside;
    }
    let kept = selected.iter().filter(|id| !inside.contains(id)).cloned();
    let mut out: Vec<String> = kept.collect();
    out.extend(inside.into_iter().filter(|id| !selected.contains(id)));
    out
}

fn strip_path() -> std::path::PathBuf {
    crate::paths::config_dir().join("pool.json")
}

pub(super) fn strip_kept() -> bool {
    std::fs::read_to_string(strip_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["metadata"].as_bool())
        .unwrap_or(true)
}

fn header_sort(column: usize, sort: PoolSort, descending: bool) -> Option<(PoolSort, bool)> {
    let by = match column {
        0 => PoolSort::Name,
        1 => PoolSort::Duration,
        2 => PoolSort::Date,
        _ => return None,
    };
    Some((by, by == sort && !descending))
}

fn bin_name(path: &str) -> &str {
    if path.is_empty() {
        "Master"
    } else {
        leaf(path)
    }
}

pub(super) const FULL: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));

fn frame_at(rect: Rect, x: f32, frames: i64) -> i64 {
    let f = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    ((f64::from(f) * frames as f64) as i64).min((frames - 1).max(0))
}

fn strip_frames(slots: usize, frames: i64) -> Vec<i64> {
    (0..slots)
        .map(|i| ((i as f64 + 0.5) / slots as f64 * frames as f64) as i64)
        .collect()
}

fn scrub_frame(frame: i64, frames: i64, px: f32) -> i64 {
    let step = super::tiles::tile_step(frames as f64 / f64::from(px.max(1.0)));
    frame - frame.rem_euclid(step)
}

fn tile_key(
    ctx: &egui::Context,
    m: &Media,
    frame: i64,
    px: f32,
) -> Option<(super::tiles::TileKey, u32)> {
    let aspect = super::tiles::media_aspect(m)?;
    let height = super::tiles::tile_height(px * ctx.pixels_per_point());
    let width = (height as f32 * aspect).round().max(1.0) as u32;
    let key = super::tiles::TileKey {
        media: m.id.clone(),
        frame,
        height,
    };
    Some((key, width))
}

fn poster_frame(frames: i64) -> i64 {
    frames / POSTER_PART
}

fn strip_slots(width: f32, picture: f32) -> usize {
    (width / picture).ceil().max(1.0) as usize
}

fn near_view(view: Rangef) -> Rangef {
    view.expand(view.span())
}

fn no_pictures(painter: &egui::Painter, rect: Rect, m: &Media) {
    let text = if m.info.video.is_none() {
        "♪ audio"
    } else {
        "…"
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(16.0),
        theme::colors().text_dim,
    );
}

fn subtitle_card(painter: &egui::Painter, rect: Rect, m: &Media) {
    let Some(s) = &m.subtitles else { return };
    let c = theme::colors();
    let text = painter.layout(
        elide(&s.first.replace('\n', " "), 90),
        egui::FontId::proportional(12.0),
        c.text,
        rect.width() - 16.0,
    );
    painter.galley(rect.center() - text.size() / 2.0, text, c.text);
    let icon = painter.layout_no_wrap(
        egui_phosphor::regular::SUBTITLES.to_string(),
        egui::FontId::proportional(14.0),
        c.text,
    );
    let r = Rect::from_min_size(
        Pos2::new(rect.left() + 3.0, rect.bottom() - icon.size().y - 5.0),
        icon.size() + vec2(6.0, 2.0),
    );
    painter.rect_filled(r, CornerRadius::same(2), c.panel_dark);
    painter.galley(r.min + vec2(3.0, 1.0), icon, c.text);
}

fn subtitle_frames(p: &Project, m: &Media) -> i64 {
    ((m.info.duration - m.info.start_time).max(0.0) * p.settings.fps.as_f64()).round() as i64
}

fn marks(painter: &egui::Painter, rect: Rect, m: &Media, frames: i64, hovered: Option<i64>) {
    if frames <= 0 {
        return;
    }
    let x = |f: i64| rect.left() + rect.width() * (f as f32 / frames as f32);
    if !m.marks.is_empty() {
        let a = x(m.marks.mark_in.unwrap_or(0));
        let b = x(m.marks.mark_out.unwrap_or(frames));
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(a, rect.bottom() - 4.0),
                Pos2::new(b, rect.bottom()),
            ),
            CornerRadius::ZERO,
            Color32::WHITE,
        );
    }
    if let Some(f) = hovered {
        painter.line_segment(
            [Pos2::new(x(f), rect.top()), Pos2::new(x(f), rect.bottom())],
            Stroke::new(1.0, theme::colors().playhead),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinBar {
    Bins,
    Crumb(usize),
    SortLabel,
    Sort,
    Order,
}

fn sort_label(s: PoolSort) -> &'static str {
    match s {
        PoolSort::Added => "Added",
        PoolSort::Name => "Name",
        PoolSort::Duration => "Duration",
        PoolSort::Date => "Date",
        PoolSort::Timecode => "Timecode",
        PoolSort::Camera => "Camera",
        PoolSort::DateTime => "Date Time",
        PoolSort::Bin => "Bin",
        PoolSort::SceneShot => "Scene, Shot",
        PoolSort::ClipColor => "Clip Color",
        PoolSort::OnlineStatus => "Online Status",
    }
}

struct ProxyState {
    badge: &'static str,
    column: String,
    color: Color32,
}

fn proxy_state(m: &Media, lost: bool) -> Option<ProxyState> {
    let p = m.proxy.as_ref()?;
    Some(if lost {
        ProxyState {
            badge: "LOST PROXY",
            column: "file gone".into(),
            color: theme::colors().error,
        }
    } else if m.fresh_proxy().is_some() {
        ProxyState {
            badge: "PROXY",
            column: format!("{}x{}", p.width, p.height),
            color: theme::colors().ok,
        }
    } else {
        ProxyState {
            badge: "OLD PROXY",
            column: "out of date".into(),
            color: theme::colors().error,
        }
    })
}

impl Workspace {
    pub(super) fn pool_panel(&mut self, ui: &mut egui::Ui) {
        self.hover = None;
        self.bins.targets.clear();
        self.bins.area = Some(ui.max_rect());
        let press = ui.input(|i| i.pointer.press_origin().filter(|_| i.pointer.any_pressed()));
        let snap = self.snap.clone();
        let project = snap.project();
        let search_id = egui::Id::unique("pool-search");
        if !ui.ctx().memory(|m| m.has_focus(search_id)) && self.search != snap.pool.search {
            self.search = snap.pool.search.clone();
        }
        let live = self.live.clone();
        let search = |ui: &mut egui::Ui, text: &mut String| {
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .id(search_id)
                    .hint_text("Search")
                    .desired_width(f32::INFINITY),
            );
            if r.changed() {
                live.request("pool.set", json!({ "search": text }));
            }
        };
        let mut search_below = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Media Pool").strong());
            if let Some(p) = project {
                let mut on = p.settings.use_proxies;
                if ui.checkbox(&mut on, "Proxies").changed() {
                    self.send("project.set", json!({ "use_proxies": on }));
                }
            }
            let missing = snap.missing.len();
            if missing > 0
                && ui
                    .button(
                        RichText::new(format!("Relink Media ({missing})"))
                            .color(theme::colors().error),
                    )
                    .clicked()
            {
                self.relink_missing(ui.ctx());
            }
            let on = snap.pool.view == PoolMode::Metadata;
            let cards = egui::Button::selectable(
                on,
                RichText::new(egui_phosphor::regular::IDENTIFICATION_CARD).size(14.0),
            );
            let cards = ui.add(cards).on_hover_text("Metadata");
            cards.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Metadata"));
            if cards.clicked() && !on {
                self.send("pool.set", json!({ "view": PoolMode::Metadata }));
            }
            for (mode, name) in [
                (PoolMode::Thumbnail, "Grid"),
                (PoolMode::Filmstrip, "Filmstrip"),
                (PoolMode::List, "List"),
            ] {
                let on = snap.pool.view == mode;
                if ui.selectable_label(on, name).clicked() && !on {
                    self.send("pool.set", json!({ "view": mode }));
                }
            }
            let tag = egui::Button::selectable(
                self.metadata_strip,
                RichText::new(egui_phosphor::regular::TAG).size(14.0),
            );
            if ui.add(tag).clicked() {
                self.show_metadata_strip(!self.metadata_strip);
            }
            let sync = egui::Button::new(
                RichText::new(egui_phosphor::regular::ARROWS_CLOCKWISE).size(14.0),
            );
            if ui.add(sync).on_hover_text("Sync Clips").clicked() {
                self.open_sync_clips(ui.ctx());
            }
            if ui.available_width() >= 120.0 {
                search(ui, &mut self.search);
            } else {
                search_below = true;
            }
        });
        if search_below {
            search(ui, &mut self.search);
        }
        if let Some(p) = project {
            self.bin_bar(ui, p);
            if self.metadata_strip {
                self.metadata_strip(ui, p);
            }
        }
        ui.separator();
        let view = &snap.pool;
        let searching = !self.search.is_empty();
        let (folders, media): (Vec<String>, Vec<&Media>) = match project {
            Some(p) => (
                p.bins
                    .iter()
                    .filter(|b| !searching && parent(b) == view.bin)
                    .cloned()
                    .collect(),
                shown_media(p, view, &self.search),
            ),
            None => Default::default(),
        };
        self.bins.order = media.iter().map(|m| m.id.clone()).collect();
        let height = (ui.available_height() - self.pool_buttons).max(60.0);
        let scroll = match snap.pool.view {
            PoolMode::List => egui::ScrollArea::both(),
            _ => egui::ScrollArea::vertical().horizontal_scroll_offset(0.0),
        };
        let shown = scroll
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let background = ui.interact(
                    viewport.translate(ui.max_rect().min.to_vec2()),
                    ui.scope_id().with("background"),
                    Sense::click_and_drag(),
                );
                if let Some(p) = project {
                    background.context_menu(|ui| {
                        if ui.button("New Bin").clicked() {
                            self.send("bins.add", json!({ "path": new_bin(p, &view.bin) }));
                            ui.close();
                        }
                        if ui.button("Import Media…").clicked() {
                            self.import_media();
                            ui.close();
                        }
                        if ui.button("Import Media Folder…").clicked() {
                            self.import_folder();
                            ui.close();
                        }
                    });
                }
                if folders.is_empty() && media.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        if searching {
                            ui.label(dim("Nothing matches"));
                        } else if !view.bin.is_empty() {
                            ui.label(dim("Empty bin. Drag clips here, or use Move to Bin."));
                        } else {
                            ui.label(dim(match snap.bindings.first(Action::Import) {
                                Some(key) => format!(
                                    "No media yet. Import with {key}, or `vidcut media add FILE…`"
                                ),
                                None => "No media yet. Use `vidcut media add FILE…`".to_string(),
                            }));
                            if ui.button("Import Media…").clicked() {
                                self.import_media();
                            }
                        }
                    });
                    return;
                }
                let Some(p) = project else { return };
                match snap.pool.view {
                    PoolMode::Thumbnail => self.grid(ui, viewport, p, &folders, &media),
                    PoolMode::Filmstrip => self.strips(ui, viewport, p, &folders, &media),
                    PoolMode::List => self.list(ui, viewport, p, &folders, &media),
                    PoolMode::Metadata => self.cards(ui, viewport, p, &folders, &media),
                }
                self.pool_box(ui, &background, &media);
            });
        if let Some(pos) = press {
            self.pool_focus = shown.inner_rect.contains(pos);
        }
        let top = ui.cursor().top();
        ui.separator();
        self.edit_buttons(ui);
        let buttons = ui.cursor().top() - top;
        if (buttons - self.pool_buttons).abs() > 0.5 {
            self.pool_buttons = buttons;
            ui.ctx().request_repaint();
        }
        self.rename_bin_window(ui.ctx());
    }

    fn pool_box(&mut self, ui: &egui::Ui, background: &egui::Response, media: &[&Media]) {
        let content = ui.max_rect().min;
        if background.drag_started_by(egui::PointerButton::Primary)
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            self.bins.boxing = Some(Boxing {
                from: press - content,
                toggle: ui.input(|i| i.modifiers.command),
            });
        }
        let Some(Boxing { from, toggle }) = self.bins.boxing else {
            return;
        };
        let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let rect = Rect::from_two_pos(content + from, pos);
        if background.dragged() {
            super::timeline::paint::marquee(ui.painter(), rect);
            return;
        }
        self.bins.boxing = None;
        if background.drag_stopped() {
            let inside = media
                .iter()
                .zip(&self.bins.places)
                .filter(|(_, place)| place.intersects(rect))
                .map(|(m, _)| m.id.clone())
                .collect();
            let selection = boxed(&self.snap.selection.media, inside, toggle);
            self.send("selection.set", json!({ "media": selection }));
        }
    }

    pub(super) fn show_metadata_strip(&mut self, on: bool) {
        self.metadata_strip = on;
        if let Some(dir) = strip_path().parent()
            && std::fs::create_dir_all(dir).is_ok()
        {
            let text = json!({ "metadata": on }).to_string();
            let _ = vidcut_core::temp::write(&strip_path(), text.as_bytes());
        }
    }

    pub(super) fn chosen_media(&self) -> Option<String> {
        chosen(self.pool_clicked.as_deref(), &self.snap.selection.media)
    }

    fn metadata_strip(&mut self, ui: &mut egui::Ui, p: &Project) {
        let m = self.chosen_media().and_then(|id| p.media(&id).cloned());
        egui::Frame::new()
            .fill(theme::colors().panel_dark)
            .inner_margin(egui::Margin::symmetric(6, 4))
            .corner_radius(3)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let Some(m) = m else {
                    ui.label(dim("Select a clip to see its format"));
                    return;
                };
                ui.horizontal(|ui| {
                    if ui.small_button(egui_phosphor::regular::INFO).clicked() {
                        self.open_clip_info(Some(m.id.clone()));
                    }
                    ui.label(RichText::new(elide(&m.name, 48)).strong());
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (i, fact) in super::clip_info::glance(&m).iter().enumerate() {
                        if i > 0 {
                            ui.label(dim("·"));
                        }
                        ui.add(egui::Label::new(fact).extend());
                    }
                });
            });
    }

    fn bin_bar(&mut self, ui: &mut egui::Ui, p: &Project) {
        let view = self.snap.pool.clone();
        let mut levels = vec![("Master".to_string(), String::new())];
        for name in view.bin.split('/').filter(|s| !s.is_empty()) {
            let at = join(&levels[levels.len() - 1].1, name);
            levels.push((name.to_string(), at));
        }
        let last = levels.len() - 1;
        let left: Vec<BinBar> = std::iter::once(BinBar::Bins)
            .chain((0..last).map(BinBar::Crumb))
            .collect();
        let leaving: Vec<BinBar> = (0..last).map(BinBar::Crumb).collect();
        let row = super::row::Row {
            left: &left,
            fill: Some((BinBar::Crumb(last), egui::Align::Min)),
            right: &[BinBar::SortLabel, BinBar::Sort, BinBar::Order],
            leaving: &leaving,
            group: |_| 0,
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            self.fit_row(ui, row, |ws, ui, item| match item {
                BinBar::Bins => {
                    ui.menu_button("Bins", |ui| {
                        for (path, depth) in bin_list(p) {
                            let text = format!("{}{}", "    ".repeat(depth), bin_name(&path));
                            if ui.selectable_label(view.bin == path, text).clicked() {
                                ws.send("pool.set", json!({ "bin": path }));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button("New Bin").clicked() {
                            ws.send("bins.add", json!({ "path": new_bin(p, &view.bin) }));
                            ui.close();
                        }
                    })
                    .response
                }
                BinBar::Crumb(i) => {
                    let (name, at) = &levels[i];
                    ui.horizontal(|ui| {
                        if i > 0 && !egui::containers::menu::is_in_menu(ui) {
                            ui.label(dim("›"));
                        }
                        let here = *at == view.bin;
                        let text = if here {
                            RichText::new(name).strong()
                        } else {
                            RichText::new(name)
                        };
                        let r = ui.add(egui::Button::new(text).frame(false).truncate());
                        if !ui.is_sizing_pass() {
                            ws.drop_highlight(ui, r.rect, at);
                        }
                        if r.clicked() && !here {
                            ws.send("pool.set", json!({ "bin": at }));
                        }
                    })
                    .response
                }
                BinBar::SortLabel => ui.label(dim("Sort")),
                BinBar::Sort => {
                    let mut sort = view.sort;
                    let r = egui::ComboBox::from_id_salt("pool-sort")
                        .selected_text(sort_label(sort))
                        .width(100.0)
                        .show_ui(ui, |ui| {
                            for s in [
                                PoolSort::Added,
                                PoolSort::Name,
                                PoolSort::Duration,
                                PoolSort::Date,
                                PoolSort::Timecode,
                                PoolSort::Camera,
                                PoolSort::DateTime,
                                PoolSort::Bin,
                                PoolSort::SceneShot,
                                PoolSort::ClipColor,
                                PoolSort::OnlineStatus,
                            ] {
                                ui.selectable_value(&mut sort, s, sort_label(s));
                            }
                        })
                        .response;
                    if sort != view.sort {
                        ws.send("pool.set", json!({ "sort": sort }));
                    }
                    r
                }
                BinBar::Order => {
                    let arrow = if view.descending { "⬇" } else { "⬆" };
                    let r = ui.small_button(arrow);
                    if r.clicked() {
                        ws.send("pool.set", json!({ "descending": !view.descending }));
                    }
                    r
                }
            });
        });
    }

    fn dragging_media(&self) -> bool {
        self.timeline.drag.as_ref().is_some_and(|d| {
            matches!(
                d.drag.gesture,
                Gesture::Media { .. } | Gesture::Subtitles { .. }
            )
        })
    }

    fn dragged_bin(&self) -> Option<&str> {
        let Gesture::Bin { path } = &self.timeline.drag.as_ref()?.drag.gesture else {
            return None;
        };
        Some(path.as_str())
    }

    pub(super) fn bin_target_at(&self, pos: Pos2) -> Option<String> {
        bin_at(&self.bins.targets, pos).map(str::to_string)
    }

    pub(super) fn bin_rename_to(&self, from: &str, pos: Pos2) -> Option<String> {
        bin_drop_to(from, bin_at(&self.bins.targets, pos)?)
    }

    pub(super) fn bin_drag_label(&self) -> Option<String> {
        self.dragged_bin().map(|path| elide(leaf(path), 30))
    }

    fn drop_highlight(&mut self, ui: &egui::Ui, rect: Rect, bin: &str) {
        self.bins.targets.push((rect, bin.to_string()));
        let over = ui
            .input(|i| i.pointer.latest_pos())
            .is_some_and(|p| rect.contains(p));
        let show = if let Some(from) = self.dragged_bin() {
            bin_drop_to(from, bin).is_some()
        } else {
            self.dragging_media()
        };
        if over && show {
            ui.painter().rect_stroke(
                rect.expand(1.0),
                CornerRadius::same(3),
                Stroke::new(2.0, theme::colors().accent),
                egui::StrokeKind::Inside,
            );
        }
    }

    fn grid(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        p: &Project,
        folders: &[String],
        media: &[&Media],
    ) {
        let origin = ui.cursor().min;
        let per_row = per_row(ui.available_width());
        let count = folders.len() + media.len();
        let rows = count.div_ceil(per_row);
        let step = TILE_FULL + Vec2::splat(TILE_GAP);
        let top = origin.y - ui.max_rect().top();
        let place = |k: usize| {
            let at = vec2((k % per_row) as f32 * step.x, (k / per_row) as f32 * step.y);
            Rect::from_min_size(origin + at, TILE_FULL)
        };
        let visible = shown(viewport.y_range(), top, step.y, rows);
        for row in visible.clone() {
            for k in row * per_row..((row + 1) * per_row).min(count) {
                let rect = place(k);
                match k.checked_sub(folders.len()) {
                    None => self.folder_tile(ui, rect, p, &folders[k]),
                    Some(i) => self.tile(ui, rect, media[i]),
                }
            }
        }
        let cells = |row: usize| {
            let at = |k: usize| k.min(count).saturating_sub(folders.len());
            &media[at(row * per_row)..at((row + 1) * per_row)]
        };
        let seen: Vec<&Media> = visible.clone().flat_map(cells).copied().collect();
        let near: Vec<&Media> = shown(near_view(viewport.y_range()), top, step.y, rows)
            .filter(|row| !visible.contains(row))
            .flat_map(cells)
            .copied()
            .collect();
        self.preload(ui.ctx(), &seen, &near, TILE);
        self.bins.places = (folders.len()..count).map(place).collect();
        let size = vec2(per_row.min(count) as f32, rows as f32) * step - Vec2::splat(TILE_GAP);
        ui.expand_to_include_rect(Rect::from_min_size(origin, size));
    }

    fn strips(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        p: &Project,
        folders: &[String],
        media: &[&Media],
    ) {
        for b in folders {
            self.folder_row(ui, p, b);
        }
        let origin = ui.cursor().min;
        let size = vec2(ui.available_width(), STRIP_HEIGHT + HEADER);
        let gap = ui.spacing().item_spacing.y;
        let top = origin.y - ui.max_rect().top();
        let place =
            |i: usize| Rect::from_min_size(origin + vec2(0.0, i as f32 * (size.y + gap)), size);
        let visible = shown(viewport.y_range(), top, size.y + gap, media.len());
        for i in visible.clone() {
            self.filmstrip(ui, place(i), media[i]);
            self.pool_thumbs(media[i]);
        }
        for i in shown(
            near_view(viewport.y_range()),
            top,
            size.y + gap,
            media.len(),
        ) {
            if !visible.contains(&i) {
                self.preload_strip(ui.ctx(), media[i], size.x);
            }
        }
        self.bins.places = (0..media.len()).map(place).collect();
        if !media.is_empty() {
            let height = media.len() as f32 * (size.y + gap) - gap;
            ui.expand_to_include_rect(Rect::from_min_size(origin, vec2(size.x, height)));
        }
    }

    fn folder_tile(&mut self, ui: &mut egui::Ui, rect: Rect, p: &Project, path: &str) {
        let resp = ui.interact(
            rect,
            ui.scope_id().with(("bin", path)),
            Sense::click_and_drag(),
        );
        let picture = Rect::from_min_size(rect.min, TILE);
        let painter = ui.painter_at(rect);
        painter.rect_filled(picture, CornerRadius::same(3), theme::colors().track);
        let color = Color32::from_rgb(150, 132, 92);
        let body = Rect::from_center_size(picture.center() + vec2(0.0, 4.0), vec2(64.0, 42.0));
        let tab = Rect::from_min_size(body.min - vec2(0.0, 7.0), vec2(26.0, 9.0));
        painter.rect_filled(tab, CornerRadius::same(2), color);
        painter.rect_filled(body, CornerRadius::same(3), color);
        let count = p.media.iter().filter(|m| within(&m.bin, path)).count();
        for (y, text, color) in [
            (2.0, elide(leaf(path), 28), theme::colors().text),
            (16.0, format!("{count} clip(s)"), theme::colors().text_dim),
        ] {
            painter.text(
                Pos2::new(picture.left(), picture.bottom() + y),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(12.0),
                color,
            );
        }
        self.drop_highlight(ui, picture, path);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, leaf(path)));
        self.bin_interaction(&resp, path);
    }

    fn folder_row(&mut self, ui: &mut egui::Ui, p: &Project, path: &str) {
        let count = p.media.iter().filter(|m| within(&m.bin, path)).count();
        let r = ui
            .selectable_label(
                false,
                format!("📁 {}   {count} clip(s)", elide(leaf(path), 60)),
            )
            .interact(Sense::drag());
        self.drop_highlight(ui, r.rect, path);
        self.bin_interaction(&r, path);
    }

    fn bin_interaction(&mut self, resp: &egui::Response, path: &str) {
        if resp.drag_started()
            && let Some(press) = resp.ctx.input(|i| i.pointer.press_origin())
        {
            self.start_bin_drag(path, press);
            resp.ctx.request_repaint();
            return;
        }
        if resp.double_clicked() {
            self.send("pool.set", json!({ "bin": path }));
        }
        resp.context_menu(|ui| {
            if ui.button("Open").clicked() {
                self.send("pool.set", json!({ "bin": path }));
                ui.close();
            }
            if ui.button("Rename Bin…").clicked() {
                self.bins.rename = Some(Rename {
                    path: path.to_string(),
                    name: leaf(path).to_string(),
                    focus: true,
                    new: false,
                });
                ui.close();
            }
            if ui.button("Remove Bin").clicked() {
                self.send("bins.remove", json!({ "path": path }));
                ui.close();
            }
        });
    }

    fn rename_bin_window(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.bins.rename else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let title = if r.new { "New Bin" } else { "Rename Bin" };
        super::dialog::window(ctx, title, &mut open, |ui| {
            let mut enter = false;
            super::dialog::form(ui, "bin-form", |ui| {
                super::dialog::row(ui, "Name", |ui| {
                    let output = egui::TextEdit::singleline(&mut r.name)
                        .desired_width(260.0)
                        .show(ui);
                    let edit = output.response;
                    if r.focus {
                        edit.request_focus();
                        let mut state = output.state;
                        let all = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(r.name.chars().count()),
                        );
                        state.cursor.set_char_range(Some(all));
                        state.store(ui.ctx(), edit.id);
                        r.focus = false;
                    }
                    enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
            });
            let named = !r.name.trim().is_empty();
            super::dialog::footer(ui, |ui| {
                let label = if r.new { "Create" } else { "Rename" };
                apply = super::dialog::main_button(ui, label, named) || (enter && named);
                cancel |= ui.button("Cancel").clicked();
                if !named {
                    super::dialog::why(ui, "Name it first");
                }
            });
        });
        let name = r.name.trim();
        if apply && !name.is_empty() {
            let to = join(parent(&r.path), name);
            if r.new {
                self.send("bins.add", json!({ "path": to }));
            } else if to != r.path {
                let path = r.path.clone();
                self.send("bins.rename", json!({ "path": path, "to": to }));
            }
        }
        if apply || cancel || !open {
            self.bins.rename = None;
        }
    }

    pub(super) fn new_bin(&mut self) {
        if let Some(p) = self.project() {
            let path = new_bin(p, &self.snap.pool.bin);
            self.bins.rename = Some(Rename {
                name: leaf(&path).to_string(),
                path,
                focus: true,
                new: true,
            });
        }
    }

    fn tile_picture(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        picture: Rect,
        m: &Media,
        frames: i64,
        hovered_frame: Option<i64>,
    ) {
        painter.rect_filled(picture, CornerRadius::same(3), theme::colors().track);
        if let Some(f) = hovered_frame {
            self.hover = Some((m.id.clone(), f));
        }
        match super::tiles::media_aspect(m) {
            _ if m.subtitles.is_some() => subtitle_card(painter, picture, m),
            Some(aspect) => {
                let r = fit(picture, aspect);
                let poster = poster_frame(frames);
                let shown = hovered_frame
                    .and_then(|f| {
                        let f = scrub_frame(f, frames, picture.width());
                        self.pool_picture(ui.ctx(), m, f, r.height(), 0)
                    })
                    .or_else(|| self.pool_picture(ui.ctx(), m, poster, r.height(), 0));
                match shown {
                    Some(shown) => {
                        painter.image(shown, r, FULL, Color32::WHITE);
                    }
                    None => no_pictures(painter, picture, m),
                }
            }
            None if ui.is_rect_visible(picture) => self.sound(painter, picture, m),
            None => no_pictures(painter, picture, m),
        }
        marks(painter, picture, m, frames, hovered_frame);
        if let Some(color) = m.color.as_deref().and_then(theme::clip_color) {
            painter.circle_filled(picture.right_bottom() + vec2(-9.0, -11.0), 4.5, color);
        }
    }

    fn cues_text(&self, m: &Media) -> Option<String> {
        let (s, p) = (m.subtitles.as_ref()?, self.project()?);
        let length = self.src_tc(subtitle_frames(p, m));
        Some(format!("{} cues · {length}", s.cues))
    }

    fn cards(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        p: &Project,
        folders: &[String],
        media: &[&Media],
    ) {
        for b in folders {
            self.folder_row(ui, p, b);
        }
        let origin = ui.cursor().min;
        let top = origin.y - ui.max_rect().top();
        let width = ui.available_width();
        let per_row = cards_per_row(width);
        let size = vec2(
            (width - (per_row - 1) as f32 * TILE_GAP) / per_row as f32,
            CARD_HEIGHT,
        );
        let step = size + Vec2::splat(TILE_GAP);
        let sort = self.snap.pool.sort;
        let clusters: Vec<Option<String>> = media.iter().map(|m| cluster(sort, m)).collect();
        self.bins.places.clear();
        let mut y = 0.0;
        let mut first = 0;
        let (mut seen, mut near): (Vec<&Media>, Vec<&Media>) = (Vec::new(), Vec::new());
        for run in clusters.chunk_by(|a, b| a == b) {
            if let Some(name) = &run[0] {
                ui.painter().text(
                    origin + vec2(0.0, y + 4.0),
                    egui::Align2::LEFT_TOP,
                    name,
                    egui::FontId::proportional(14.0),
                    theme::colors().text,
                );
                y += CLUSTER_HEADING;
            }
            let rows = run.len().div_ceil(per_row);
            let place = |i: usize| {
                let at = vec2(
                    (i % per_row) as f32 * step.x,
                    y + (i / per_row) as f32 * step.y,
                );
                Rect::from_min_size(origin + at, size)
            };
            let visible = shown(viewport.y_range(), top + y, step.y, rows);
            for row in shown(near_view(viewport.y_range()), top + y, step.y, rows) {
                let cells = row * per_row..((row + 1) * per_row).min(run.len());
                if !visible.contains(&row) {
                    near.extend(cells.map(|i| media[first + i]));
                    continue;
                }
                for i in cells {
                    self.card(ui, place(i), media[first + i]);
                    seen.push(media[first + i]);
                }
            }
            self.bins.places.extend((0..run.len()).map(place));
            first += run.len();
            y += rows as f32 * step.y;
        }
        self.preload(ui.ctx(), &seen, &near, CARD_PICTURE);
        if y > 0.0 {
            ui.expand_to_include_rect(Rect::from_min_size(origin, vec2(width, y - TILE_GAP)));
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, rect: Rect, m: &Media) {
        let resp = ui.interact(
            rect,
            ui.scope_id().with(("media", &m.id)),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(3), ui.visuals().faint_bg_color);
        let picture = Rect::from_min_size(rect.min + Vec2::splat(CARD_MARGIN), CARD_PICTURE);
        let frames = self.project().map_or(0, |p| p.media_frames(m));
        let hovered_frame = resp
            .hover_pos()
            .filter(|pos| picture.contains(*pos))
            .map(|pos| frame_at(picture, pos.x, frames));
        self.tile_picture(ui, &painter, picture, m, frames, hovered_frame);
        self.outline(&painter, rect, m);
        self.badges(&painter, picture, m);
        let start = m
            .info
            .timecode
            .as_ref()
            .map_or_else(|| self.src_tc(0), super::clip_info::start_label);
        let title = main_description(self.snap.pool.sort, m, &start);
        let left = picture.right() + CARD_MARGIN;
        let room = rect.right() - CARD_MARGIN - left;
        let at = |y: f32| rect.top() + y;
        let (text, dim) = (theme::colors().text, theme::colors().text_dim);
        let line = Stroke::new(1.0, theme::colors().panel_dark);
        painter.galley(
            Pos2::new(left, at(CARD_MARGIN)),
            one_line(&painter, title, 14.0, text, room),
            text,
        );
        let x = Rangef::new(left, left + room);
        for y in CARD_LINES {
            painter.hline(x, at(y), line);
        }
        let row = |y: f32, cells: &[String]| card_row(&painter, x, at(y + 4.0), cells, dim);
        let name = m.name.clone();
        match self.cues_text(m) {
            Some(cues) => {
                row(CARD_LINES[0], &[cues]);
                row(CARD_LINES[2], &[name]);
            }
            None => {
                let note =
                    |value: &Option<String>| value.clone().unwrap_or_else(|| NO_VALUE.to_string());
                let camera = m.camera.as_ref().map(|name| format!("Cam {name}"));
                row(CARD_LINES[0], &[start, note(&m.created), note(&camera)]);
                let scene = scene_shot_take(m).map(|cell| note(&cell));
                row(CARD_LINES[1], &scene);
                row(CARD_LINES[2], &[name, note(&m.comment)]);
            }
        }
        self.media_interaction(&resp, m);
    }

    fn tile(&mut self, ui: &mut egui::Ui, rect: Rect, m: &Media) {
        let resp = ui.interact(
            rect,
            ui.scope_id().with(("media", &m.id)),
            Sense::click_and_drag(),
        );
        let picture = Rect::from_min_size(rect.min, TILE);
        let painter = ui.painter_at(rect);
        let frames = self.project().map_or(0, |p| p.media_frames(m));
        let hovered_frame = resp.hover_pos().map(|pos| frame_at(picture, pos.x, frames));
        self.tile_picture(ui, &painter, picture, m, frames, hovered_frame);
        self.outline(&painter, picture, m);
        self.badges(&painter, picture, m);
        let label = |pos: Pos2, text: String, color: Color32| {
            painter.text(
                pos,
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(12.0),
                color,
            )
        };
        label(
            Pos2::new(picture.left(), picture.bottom() + 2.0),
            elide(&m.name, 28),
            theme::colors().text,
        );
        let second = self
            .cues_text(m)
            .unwrap_or_else(|| self.src_tc(hovered_frame.unwrap_or(frames)));
        label(
            Pos2::new(picture.left(), picture.bottom() + 16.0),
            second,
            theme::colors().text_dim,
        );
        self.media_interaction(&resp, m);
        if resp.hovered() || ui.rect_contains_pointer(picture) {
            let at = Rect::from_min_size(
                picture.right_bottom() + vec2(-40.0, -22.0),
                vec2(20.0, 18.0),
            );
            if ui
                .put(at, egui::Button::new(egui_phosphor::regular::INFO).small())
                .clicked()
            {
                self.open_clip_info(Some(m.id.clone()));
            }
        }
    }

    fn filmstrip(&mut self, ui: &mut egui::Ui, rect: Rect, m: &Media) {
        let width = rect.width();
        let resp = ui.interact(
            rect,
            ui.scope_id().with(("media", &m.id)),
            Sense::click_and_drag(),
        );
        let header = Rect::from_min_size(rect.min, vec2(width, HEADER - 2.0));
        let film = Rect::from_min_max(Pos2::new(rect.left(), header.bottom() + 2.0), rect.max);
        let painter = ui.painter_at(rect);
        let frames = self.project().map_or(0, |p| p.media_frames(m));
        let hovered_frame = resp.hover_pos().map(|pos| frame_at(film, pos.x, frames));
        if let Some(f) = hovered_frame {
            self.hover = Some((m.id.clone(), f));
        }
        if let Some(color) = m.color.as_deref().and_then(theme::clip_color) {
            painter.rect_filled(header, CornerRadius::same(2), color.gamma_multiply(0.6));
        }
        let font = egui::FontId::proportional(12.0);
        let time = self
            .cues_text(m)
            .unwrap_or_else(|| self.src_tc(hovered_frame.unwrap_or(frames)));
        let time = painter.layout_no_wrap(time, font.clone(), theme::colors().text_dim);
        let room = ((width - time.size().x - 16.0) / 6.5).max(3.0) as usize;
        let name = painter.layout_no_wrap(elide(&m.name, room), font, theme::colors().text);
        let y = header.center().y;
        painter.galley(
            Pos2::new(header.left() + 4.0, y - name.size().y / 2.0),
            name,
            theme::colors().text,
        );
        painter.galley(
            Pos2::new(
                header.right() - time.size().x - 4.0,
                y - time.size().y / 2.0,
            ),
            time,
            theme::colors().text_dim,
        );
        painter.rect_filled(film, CornerRadius::same(3), theme::colors().track);
        match super::tiles::media_aspect(m) {
            _ if !ui.is_rect_visible(rect) => {}
            _ if m.subtitles.is_some() => subtitle_card(&painter, film, m),
            Some(aspect) => {
                let w = film.height() * aspect;
                let slots = strip_slots(film.width(), w);
                let mut at = strip_frames(slots, frames);
                let mut within = vec![frames / slots as i64 / 2; slots];
                if let (Some(f), Some(pos)) = (hovered_frame, resp.hover_pos()) {
                    let slot = (((pos.x - film.left()) / w) as usize).min(slots - 1);
                    at[slot] = scrub_frame(f, frames, film.width());
                    within[slot] = 0;
                }
                let mut drawn = false;
                for (i, f) in at.into_iter().enumerate() {
                    let r = Rect::from_min_size(
                        film.min + vec2(i as f32 * w, 0.0),
                        vec2(w, film.height()),
                    );
                    if let Some(shown) = self.pool_picture(ui.ctx(), m, f, r.height(), within[i]) {
                        painter.image(shown, r, FULL, Color32::WHITE);
                        drawn = true;
                    }
                }
                if !drawn {
                    no_pictures(&painter, film, m);
                }
            }
            None => self.sound(&painter, film, m),
        }
        marks(&painter, film, m, frames, hovered_frame);
        self.outline(&painter, film, m);
        self.badges(&painter, film, m);
        self.media_interaction(&resp, m);
    }

    pub(super) fn pool_picture(
        &self,
        ctx: &egui::Context,
        m: &Media,
        frame: i64,
        px: f32,
        within: i64,
    ) -> Option<egui::TextureId> {
        let (p, t) = (self.project()?, self.tiles.as_ref()?);
        let (key, width) = tile_key(ctx, m, frame, px)?;
        t.want(&key, || t.job(p, m, &key, width))
            .or_else(|| t.nearest(&m.id, key.height, frame, within))
            .or_else(|| t.nearest_any_height(&m.id, frame))
    }

    fn pool_ahead(&self, ctx: &egui::Context, m: &Media, frame: i64, px: f32) {
        let (Some(p), Some(t)) = (self.project(), self.tiles.as_ref()) else {
            return;
        };
        if let Some((key, width)) = tile_key(ctx, m, frame, px) {
            t.prefetch(&key, || t.job(p, m, &key, width));
        }
    }

    fn pool_thumbs(&self, m: &Media) {
        if let (Some(p), Some(t)) = (self.project(), self.tiles.as_ref())
            && self.snap.thumbs.contains(&m.id)
        {
            t.thumbs(p, m);
        }
    }

    fn preload(&self, ctx: &egui::Context, seen: &[&Media], near: &[&Media], cell: Vec2) {
        let Some(p) = self.project() else { return };
        let picture = Rect::from_min_size(Pos2::ZERO, cell);
        let px =
            |m: &Media| super::tiles::media_aspect(m).map_or(cell.y, |a| fit(picture, a).height());
        for m in seen.iter().chain(near) {
            self.pool_thumbs(m);
        }
        for m in near {
            let poster = poster_frame(p.media_frames(m));
            self.pool_ahead(ctx, m, poster, px(m));
        }
    }

    fn preload_strip(&self, ctx: &egui::Context, m: &Media, width: f32) {
        self.pool_thumbs(m);
        let (Some(p), Some(aspect)) = (self.project(), super::tiles::media_aspect(m)) else {
            return;
        };
        let slots = strip_slots(width, STRIP_HEIGHT * aspect);
        for f in strip_frames(slots, p.media_frames(m)) {
            self.pool_ahead(ctx, m, f, STRIP_HEIGHT);
        }
    }

    fn sound(&self, painter: &egui::Painter, rect: Rect, m: &Media) {
        let (Some(w), Some(p)) = (self.snap.waveforms.get(&m.id), self.project()) else {
            return no_pictures(painter, rect, m);
        };
        let secs = p.settings.fps.frames_to_secs(p.media_frames(m));
        let origin = m.origin();
        let per_px = secs / f64::from(rect.width().max(1.0));
        painter.rect_filled(rect, CornerRadius::same(3), theme::colors().audio);
        super::timeline::paint::wave_columns(painter, rect, |x| {
            let t = origin + f64::from(x - rect.left()) * per_px;
            w.range(t, t + per_px)
        });
    }

    fn outline(&self, painter: &egui::Painter, rect: Rect, m: &Media) {
        let selected = self.snap.selection.media.contains(&m.id);
        let open = self.snap.source_at().map(|(id, _)| id) == Some(m.id.as_str());
        let border = if selected {
            Stroke::new(2.0, theme::colors().accent)
        } else if open {
            Stroke::new(1.0, theme::colors().text_dim)
        } else {
            Stroke::NONE
        };
        painter.rect_stroke(
            rect,
            CornerRadius::same(3),
            border,
            egui::StrokeKind::Inside,
        );
    }

    fn badges(&self, painter: &egui::Painter, rect: Rect, m: &Media) {
        let badge = |text: &str, color: Color32, right: bool| {
            let galley =
                painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(10.0), color);
            let x = if right {
                rect.right() - galley.size().x - 8.0
            } else {
                rect.left() + 3.0
            };
            let r = Rect::from_min_size(
                Pos2::new(x, rect.top() + 3.0),
                galley.size() + vec2(6.0, 2.0),
            );
            painter.rect_filled(r, CornerRadius::same(2), theme::colors().panel_dark);
            painter.galley(r.min + vec2(3.0, 1.0), galley, color);
        };
        let missing = self.snap.missing.contains(&m.id);
        if missing {
            badge("MISSING", theme::colors().error, false);
        }
        let lost = self.snap.lost_proxies.contains_key(&m.id);
        let proxy = proxy_state(m, lost);
        if let Some(px) = &proxy {
            badge(px.badge, px.color, true);
        }
        if self.project().is_some_and(|p| p.group_of(&m.id).is_some()) {
            let top = if proxy.is_some() { 20.0 } else { 3.0 };
            let at = Pos2::new(rect.right() - 20.0, rect.top() + top);
            painter.text(
                at,
                egui::Align2::LEFT_TOP,
                egui_phosphor::regular::ARROWS_CLOCKWISE,
                egui::FontId::proportional(13.0),
                Color32::from_rgb(90, 170, 240),
            );
        }
        let transcribed = self.snap.transcribed.contains(&m.id);
        if transcribed {
            let galley = painter.layout_no_wrap(
                egui_phosphor::regular::CHAT_TEXT.to_string(),
                egui::FontId::proportional(12.0),
                theme::colors().text,
            );
            let r = Rect::from_min_size(
                Pos2::new(rect.left() + 3.0, rect.bottom() - galley.size().y - 5.0),
                galley.size() + vec2(6.0, 2.0),
            );
            painter.rect_filled(r, CornerRadius::same(2), theme::colors().panel_dark);
            painter.galley(r.min + vec2(3.0, 1.0), galley, theme::colors().text);
        }
    }

    fn list(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        p: &Project,
        folders: &[String],
        media: &[&Media],
    ) {
        let Some(project) = self.project_shared() else {
            return;
        };
        let (bin, search) = (self.snap.pool.bin.clone(), self.search.clone());
        let sort = (self.snap.pool.sort, self.snap.pool.descending);
        let scale = ui.ctx().pixels_per_point();
        let measured_for = |l: &&ListLayout| {
            Arc::ptr_eq(&l.project, &project)
                && l.bin == bin
                && l.search == search
                && l.sort == sort
                && l.scale == scale
        };
        let fresh = self
            .bins
            .list
            .as_ref()
            .filter(measured_for)
            .map(|l| l.sizes);
        let sizes = fresh
            .or_else(|| self.bins.list.as_ref().map(|l| l.sizes))
            .unwrap_or_else(|| ListSizes::least(ui));
        let origin = ui.cursor().min;
        let top = origin.y - ui.max_rect().top();
        let [header, bin_height, media_height] = sizes.rows;
        let bins_top = header + LIST_GAP.y;
        let media_top = bins_top + folders.len() as f32 * (bin_height + LIST_GAP.y);
        let (headers, bins, medias) = match fresh {
            Some(_) => {
                let view = viewport.y_range();
                (
                    shown(view, top, header + LIST_GAP.y, 1),
                    shown(view, top + bins_top, bin_height + LIST_GAP.y, folders.len()),
                    shown(
                        view,
                        top + media_top,
                        media_height + LIST_GAP.y,
                        media.len(),
                    ),
                )
            }
            None => (0..1, 0..folders.len(), 0..media.len()),
        };
        let rows = headers
            .map(|_| (0, 0.0, ListRow::Header))
            .chain(bins.map(|i| {
                let y = bins_top + i as f32 * (bin_height + LIST_GAP.y);
                (1 + i, y, ListRow::Bin(&folders[i]))
            }))
            .chain(medias.map(|i| {
                let y = media_top + i as f32 * (media_height + LIST_GAP.y);
                (1 + folders.len() + i, y, ListRow::Media(media[i]))
            }));
        let width =
            sizes.columns.iter().sum::<f32>() + (LIST_COLUMNS.len() - 1) as f32 * LIST_GAP.x;
        let mut measured = ListSizes::least(ui);
        for (r, y, row) in rows {
            let kind = row.kind();
            let height = sizes.rows[kind];
            let at = origin + vec2(0.0, y);
            if r % 2 == 1 {
                let stripe = Rect::from_min_size(at, vec2(width, height));
                let stripe = stripe.expand2(vec2(2.0, LIST_GAP.y / 2.0));
                ui.painter()
                    .rect_filled(stripe, 2.0, ui.visuals().faint_bg_color);
            }
            let mut x = at.x;
            for (c, column) in sizes.columns.iter().enumerate() {
                let cell = Rect::from_min_size(pos2(x, at.y), vec2(*column, height));
                let builder = UiBuilder::new()
                    .id_salt((row.id(), c))
                    .max_rect(cell)
                    .layout(egui::Layout::left_to_right(egui::Align::Center));
                let rect = ui
                    .scope_builder(builder, |ui| self.list_cell(ui, p, row, c))
                    .response
                    .rect;
                measured.columns[c] = measured.columns[c].max(rect.width());
                measured.rows[kind] = measured.rows[kind].max(rect.height());
                x += column + LIST_GAP.x;
            }
        }
        if fresh.is_none() {
            if measured != sizes {
                ui.ctx().request_discard("the List view's sizes changed");
            }
            self.bins.list = Some(ListLayout {
                project,
                bin,
                search,
                sort,
                scale,
                sizes: measured,
            });
        }
        self.bins.places = (0..media.len())
            .map(|i| {
                let y = media_top + i as f32 * (media_height + LIST_GAP.y);
                Rect::from_min_size(origin + vec2(0.0, y), vec2(width, media_height))
            })
            .collect();
        let height = match media.len() {
            0 => media_top - LIST_GAP.y,
            n => media_top + n as f32 * (media_height + LIST_GAP.y) - LIST_GAP.y,
        };
        ui.expand_to_include_rect(Rect::from_min_size(origin, vec2(width, height)));
    }

    fn list_cell(&mut self, ui: &mut egui::Ui, p: &Project, row: ListRow, c: usize) {
        match row {
            ListRow::Header => {
                let view = &self.snap.pool;
                let Some((sort, descending)) = header_sort(c, view.sort, view.descending) else {
                    ui.label(dim(LIST_COLUMNS[c]));
                    return;
                };
                let text = match (sort == view.sort, view.descending) {
                    (false, _) => LIST_COLUMNS[c].to_string(),
                    (true, false) => format!("{} ⬆", LIST_COLUMNS[c]),
                    (true, true) => format!("{} ⬇", LIST_COLUMNS[c]),
                };
                let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
                let (rect, r) = ui.allocate_exact_size(size, Sense::click());
                let builder = UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center));
                ui.scope_builder(builder, |ui| {
                    ui.add(egui::Label::new(dim(text)).selectable(false))
                });
                if r.clicked() {
                    self.send(
                        "pool.set",
                        json!({ "sort": sort, "descending": descending }),
                    );
                }
            }
            ListRow::Bin(b) => match c {
                0 => {
                    let r = ui
                        .selectable_label(false, format!("📁 {}", elide(leaf(b), 38)))
                        .interact(Sense::drag());
                    self.drop_highlight(ui, r.rect, b);
                    self.bin_interaction(&r, b);
                }
                1 => {
                    let count = p.media.iter().filter(|m| within(&m.bin, b)).count();
                    ui.label(dim(format!("{count} clip(s)")));
                }
                _ => {
                    ui.label("");
                }
            },
            ListRow::Media(m) => self.media_cell(ui, m, c),
        }
    }

    fn media_cell(&mut self, ui: &mut egui::Ui, m: &Media, c: usize) {
        let video = m.info.video.as_ref();
        match c {
            0 => {
                let selected = self.snap.selection.media.contains(&m.id);
                let mut name = RichText::new(elide(&m.name, 40));
                if self.snap.missing.contains(&m.id) {
                    name = name.color(theme::colors().error);
                }
                let resp = ui.selectable_label(selected, name).interact(Sense::drag());
                self.media_interaction(&resp, m);
            }
            1 => {
                let frames = self.project().map_or(0, |p| match m.subtitles {
                    Some(_) => subtitle_frames(p, m),
                    None => p.media_frames(m),
                });
                ui.label(self.src_tc(frames));
            }
            2 => {
                ui.label(file_time(m).map_or_else(|| "-".to_string(), |d| d.minutes()));
            }
            3 => {
                ui.label(video.map_or_else(
                    || "-".to_string(),
                    |v| {
                        let (w, h) = v.display_size();
                        format!("{w}x{h}")
                    },
                ));
            }
            4 => match video {
                Some(v) => {
                    ui.label(v.fps.decimal());
                }
                None => {
                    ui.label("-");
                }
            },
            5 => match &m.subtitles {
                Some(s) => {
                    let mut text = format!(
                        "{}, {} cues",
                        crate::gui::subtitles::format_name(s.format),
                        s.cues
                    );
                    if let Some(l) = &s.language {
                        text.push_str(&format!(", {l}"));
                    }
                    ui.label(text);
                }
                None => {
                    ui.label(video.map_or("-", |v| v.codec.as_str()));
                }
            },
            6 => {
                ui.label(match (m.info.audio.first(), &m.subtitles) {
                    (Some(a), _) => format!("{} {} ch", a.codec, a.channels),
                    (None, Some(_)) => "-".into(),
                    (None, None) => "none".into(),
                });
            }
            7 => match proxy_state(m, self.snap.lost_proxies.contains_key(&m.id)) {
                Some(px) => {
                    ui.label(RichText::new(px.column).color(px.color));
                }
                None => {
                    ui.label("-");
                }
            },
            _ => match m.color.as_deref().and_then(theme::clip_color) {
                Some(color) => {
                    let (patch, _) = ui.allocate_exact_size(vec2(14.0, 12.0), Sense::hover());
                    ui.painter()
                        .rect_filled(patch, CornerRadius::same(2), color);
                }
                None => {
                    ui.label("");
                }
            },
        }
    }

    fn media_interaction(&mut self, resp: &egui::Response, m: &Media) {
        if resp.drag_started()
            && m.subtitles.is_some()
            && let Some(press) = resp.ctx.input(|i| i.pointer.press_origin())
        {
            self.start_subtitle_drag(&m.id, press);
            return;
        }
        if resp.drag_started()
            && let Some(press) = resp.ctx.input(|i| i.pointer.press_origin())
        {
            let frames = self.project().map_or(0, |p| p.media_frames(m));
            let range = (
                m.marks.mark_in.unwrap_or(0),
                m.marks.mark_out.unwrap_or(frames),
            );
            self.start_media_drag(&m.id, range, press);
            return;
        }
        if resp.double_clicked() {
            if m.subtitles.is_none() {
                self.send("viewer.load", json!({ "media": m.id }));
            }
            return;
        }
        if resp.clicked() {
            self.pool_clicked = Some(m.id.clone());
            let mods = resp.ctx.input(|i| i.modifiers);
            let mut selection = self.snap.selection.media.clone();
            if mods.command {
                if let Some(i) = selection.iter().position(|x| x == &m.id) {
                    selection.remove(i);
                } else {
                    selection.push(m.id.clone());
                }
            } else if mods.shift
                && let Some(last) = selection.last()
            {
                let ids: Vec<&str> = self.bins.order.iter().map(String::as_str).collect();
                let (a, b) = (
                    ids.iter().position(|x| x == last).unwrap_or(0),
                    ids.iter().position(|x| *x == m.id).unwrap_or(0),
                );
                selection = ids[a.min(b)..=a.max(b)]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            } else {
                selection = vec![m.id.clone()];
            }
            self.send("selection.set", json!({ "media": selection }));
            if !mods.command && !mods.shift && m.subtitles.is_none() {
                match self.snap.viewer {
                    ViewerMode::SourceClip => self.send("viewer.load", json!({ "media": m.id })),
                    ViewerMode::SourceTape => {
                        if let Some(e) = self.snap.tape.iter().find(|e| e.media == m.id) {
                            self.send(
                                "player.seek",
                                json!({ "at": e.start, "viewer": "source_tape" }),
                            );
                        }
                    }
                    ViewerMode::MultiSource | ViewerMode::Timeline => {}
                }
            }
        }
        resp.context_menu(|ui| {
            if m.subtitles.is_some() {
                if ui
                    .button("Insert Selected Subtitles to Timeline Using Timecode")
                    .clicked()
                {
                    self.insert_subtitles(&m.id);
                    ui.close();
                }
                if ui.button("Relink…").clicked() {
                    self.relink_one(ui.ctx(), &m.id);
                    ui.close();
                }
                ui.separator();
                self.pool_item_menu(ui, m);
                return;
            }
            if ui.button("Open in Viewer").clicked() {
                self.send("viewer.load", json!({ "media": m.id }));
                ui.close();
            }
            let group = self
                .project()
                .and_then(|p| p.group_of(&m.id))
                .map(|g| g.id.clone());
            if let Some(group) = group
                && ui.button("Open Sync Group…").clicked()
            {
                self.open_sync_group(&group);
                ui.close();
            }
            if ui.button("Append to Timeline").clicked() {
                let frames = self.project().map_or(0, |p| p.media_frames(m));
                let (a, b) = (
                    m.marks.mark_in.unwrap_or(0),
                    m.marks.mark_out.unwrap_or(frames),
                );
                self.send(
                    "edit.append",
                    json!({ "media": m.id, "range": format!("{a}f..{b}f") }),
                );
                ui.close();
            }
            if ui
                .add_enabled(!m.marks.is_empty(), egui::Button::new("Clear In and Out"))
                .clicked()
            {
                self.send("media.marks.clear", json!({ "media": m.id }));
                ui.close();
            }
            if m.info.video.is_some() && !m.is_still() && ui.button("Generate Proxy").clicked() {
                self.send("proxy.generate", json!({ "media": m.id }));
                ui.close();
            }
            if m.proxy.is_some() && ui.button("Remove Proxy").clicked() {
                self.send("proxy.remove", json!({ "media": m.id }));
                ui.close();
            }
            if !m.info.audio.is_empty() && ui.button("Transcribe Audio…").clicked() {
                let ids = match self.snap.selection.media.contains(&m.id) {
                    true => self
                        .snap
                        .selection
                        .media
                        .iter()
                        .filter(|id| {
                            self.project()
                                .and_then(|p| p.media(id))
                                .is_some_and(|x| !x.info.audio.is_empty())
                        })
                        .cloned()
                        .collect(),
                    false => vec![m.id.clone()],
                };
                self.open_transcribe(ids);
                ui.close();
            }
            if ui.button("Relink…").clicked() {
                self.relink_one(ui.ctx(), &m.id);
                ui.close();
            }
            ui.separator();
            self.pool_item_menu(ui, m);
        });
    }

    pub(super) fn alpha_mode_menu(&mut self, ui: &mut egui::Ui, media: &[String]) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let alpha: Vec<&Media> = media
            .iter()
            .filter_map(|id| p.media(id))
            .filter(|m| m.info.video.as_ref().is_some_and(|v| v.alpha))
            .collect();
        let Some(first) = alpha.first() else {
            return;
        };
        let current = alpha
            .iter()
            .all(|m| m.alpha_mode == first.alpha_mode)
            .then_some(first.alpha_mode);
        let ids: Vec<&str> = alpha.iter().map(|m| m.id.as_str()).collect();
        ui.menu_button("Alpha Mode", |ui| {
            for mode in vidcut_core::model::AlphaMode::ALL {
                if ui.radio(current == Some(mode), mode.title()).clicked() {
                    self.send("media.set", json!({ "media": ids, "alpha_mode": mode }));
                    ui.close();
                }
            }
        });
    }

    fn pool_item_menu(&mut self, ui: &mut egui::Ui, m: &Media) {
        let ids = if self.snap.selection.media.contains(&m.id) {
            self.snap.selection.media.clone()
        } else {
            vec![m.id.clone()]
        };
        let current = self.project().and_then(|p| {
            let colors = ids
                .iter()
                .map(|id| p.media(id).and_then(|m| m.color.as_deref()));
            theme::same_color(colors).map(str::to_string)
        });
        if ui.button("Clip Info…").clicked() {
            self.open_clip_info(Some(m.id.clone()));
            ui.close();
        }
        if m.subclip.is_some() && ui.button("Edit Subclip…").clicked() {
            self.open_edit_subclip(&m.id);
            ui.close();
        }
        ui.menu_button("Clip Color", |ui| {
            if let Some(color) = theme::clip_color_menu(ui, current.as_deref()) {
                self.send("media.set", json!({ "media": ids, "color": color }));
                ui.close();
            }
        });
        if let Some(v) = &m.info.video {
            use vidcut_core::model::{ColorSpace, Hdr};
            self.alpha_mode_menu(ui, &ids);
            let tagged = match v.hdr {
                Some(Hdr::Pq) => "As Tagged (BT.2100 PQ)",
                Some(Hdr::Hlg) => "As Tagged (BT.2100 HLG)",
                None => "As Tagged (SDR)",
            };
            ui.menu_button("Input Color Space", |ui| {
                for (space, label) in [
                    (ColorSpace::Tagged, tagged),
                    (ColorSpace::Bt709, "BT.709 (SDR, HD)"),
                    (ColorSpace::Bt601, "BT.601 (SDR, SD)"),
                    (ColorSpace::Pq, "BT.2100 PQ (HDR10)"),
                    (ColorSpace::Hlg, "BT.2100 HLG"),
                ] {
                    if ui.radio(m.color_space == space, label).clicked() {
                        self.send("media.set", json!({ "media": ids, "color_space": space }));
                        ui.close();
                    }
                }
            });
            let mut put = None;
            ui.menu_button("Super Scale", |ui| {
                use super::super_scale::{SHARPNESS, UPSCALERS};
                use vidcut_core::model::Upscaler;
                let now = m.super_scale;
                let settings = self.project_shared().map(|p| p.settings.clone());
                let project = settings
                    .as_ref()
                    .map_or_else(Default::default, |s| s.upscaler);
                let shown = settings.as_ref().map_or_else(Default::default, |s| {
                    vidcut_core::render::super_scale_of(now, s)
                });
                for (on, label) in [(false, "Off"), (true, "On")] {
                    if ui.radio(now.on == on, label).clicked() {
                        put = Some(json!({ "super_scale": on }));
                    }
                }
                ui.separator();
                ui.add_enabled_ui(!now.is_off(), |ui| {
                    ui.menu_button("Upscaler", |ui| {
                        for &(upscaler, label) in UPSCALERS {
                            let text = super::super_scale::upscaler_text(upscaler, label, project);
                            if ui.radio(now.upscaler == upscaler, text).clicked() {
                                put = Some(json!({ "upscaler": upscaler }));
                            }
                        }
                    });
                    let timeline = self.timeline_colors();
                    if let Some(text) = super::super_scale::unfit(shown, [m], timeline) {
                        super::dialog::warn(ui, text);
                    }
                    ui.add_enabled_ui(shown.upscaler == Upscaler::Classic, |ui| {
                        ui.menu_button("Sharpness", |ui| {
                            for (sharpness, label) in SHARPNESS {
                                if ui.radio(now.sharpness == sharpness, label).clicked() {
                                    put = Some(json!({ "sharpness": sharpness }));
                                }
                            }
                        });
                    });
                });
                if put.is_some() {
                    ui.close();
                }
            });
            if let Some(params) = put {
                self.set_super_scale(&ids, params);
            }
        }
        if let Some(p) = self.project_shared() {
            ui.menu_button("Move to Bin", |ui| {
                for (path, depth) in bin_list(&p) {
                    let text = format!("{}{}", "    ".repeat(depth), bin_name(&path));
                    let here = ids.len() == 1 && m.bin == path;
                    if ui.add_enabled(!here, egui::Button::new(text)).clicked() {
                        self.send("media.move", json!({ "media": ids, "bin": path }));
                        ui.close();
                    }
                }
            });
            if ui.button("Create Bin With Selected Clips").clicked() {
                let bin = new_bin(&p, &self.snap.pool.bin);
                self.send(
                    "apply",
                    json!({
                        "label": "Create Bin With Selected Clips",
                        "commands": [
                            { "method": "bins.add", "params": { "path": bin } },
                            { "method": "media.move", "params": { "media": ids, "bin": bin } },
                        ],
                    }),
                );
                ui.close();
            }
            ui.separator();
        }
        let used = self.project().is_some_and(|p| {
            p.tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(|c| c.media == m.id)
        });
        let label = if used {
            "Remove from Pool (and its clips)"
        } else {
            "Remove from Pool"
        };
        if ui.button(label).clicked() {
            self.send("media.remove", json!({ "media": m.id, "force": used }));
            ui.close();
        }
    }

    fn insert_subtitles(&mut self, id: &str) {
        let ids: Vec<String> = match self.snap.selection.media.iter().any(|x| x == id) {
            true => self
                .snap
                .selection
                .media
                .iter()
                .filter(|x| {
                    self.project()
                        .and_then(|p| p.media(x))
                        .is_some_and(|m| m.subtitles.is_some())
                })
                .cloned()
                .collect(),
            false => vec![id.to_string()],
        };
        match ids.as_slice() {
            [one] => self.send("subtitles.import", json!({ "media": one })),
            _ => self.send(
                "apply",
                json!({
                    "label": "Insert Selected Subtitles to Timeline Using Timecode",
                    "commands": ids
                        .iter()
                        .map(|m| json!({ "method": "subtitles.import", "params": { "media": m } }))
                        .collect::<Vec<_>>(),
                }),
            ),
        }
    }

    pub(super) fn remove_media_window(&mut self, ctx: &egui::Context) {
        let Some(ids) = self.removing.clone() else {
            return;
        };
        let clips = self.project().map_or(0, |p| {
            p.tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter(|c| ids.contains(&c.media))
                .count()
        });
        let mut done = None;
        let modal = egui::Modal::new(egui::Id::unique("remove-media")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let items = match ids.len() {
                1 => "1 item".to_string(),
                n => format!("{n} items"),
            };
            ui.label(RichText::new(format!("Remove {items} from the Media Pool?")).strong());
            if clips > 0 {
                let it = if ids.len() == 1 { "it" } else { "them" };
                let clips = match clips {
                    1 => format!("The 1 clip on the timeline that uses {it} goes too."),
                    n => format!("The {n} clips on the timeline that use {it} go too."),
                };
                ui.label(clips);
            }
            ui.label(dim("The files stay on disk."));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Remove").clicked() {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
        if done.is_none() && super::workspace::escape_cancels(ctx, &modal) {
            done = Some(false);
        }
        if done == Some(true) {
            let remove = |id: &String| json!({ "media": id, "force": true });
            match ids.as_slice() {
                [id] => self.send("media.remove", remove(id)),
                _ => self.send(
                    "apply",
                    json!({
                        "label": format!("Remove {} items from the Media Pool", ids.len()),
                        "commands": ids
                            .iter()
                            .map(|id| json!({ "method": "media.remove", "params": remove(id) }))
                            .collect::<Vec<_>>(),
                    }),
                ),
            }
        }
        if done.is_some() {
            self.removing = None;
        }
    }

    pub(super) fn edit_buttons(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let spare = ui.available_width() - EDIT_ICONS * (EDIT_ICON + EDIT_ICON_GAP);
            let spacing = ui.spacing_mut();
            spacing.item_spacing.x = EDIT_ICON_GAP;
            spacing.button_padding.x = spacing
                .button_padding
                .x
                .min((spare / (2.0 * EDIT_ICONS)).max(0.0));
            let buttons = [
                (icon::SMART_INSERT, "Smart Insert", "edit.smart_insert"),
                (icon::APPEND, "Append", "edit.append"),
                (
                    icon::RIPPLE_OVERWRITE,
                    "Ripple Overwrite",
                    "edit.ripple_overwrite",
                ),
                (icon::PLACE_ON_TOP, "Place On Top", "edit.place_on_top"),
            ];
            for (glyph, name, method) in buttons {
                if edit_icon(ui, glyph, name) {
                    self.edit_function(method);
                }
            }
            if edit_icon(ui, icon::SOURCE_OVERWRITE, "Source Overwrite") {
                if self.left == LeftTab::SyncBin && self.snap.viewer == ViewerMode::SourceClip {
                    self.sync_overwrite(ui.ctx());
                } else {
                    self.edit_function("edit.source_overwrite");
                }
            }
            let at = self.snap.timeline.position;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if edit_icon(ui, icon::SMOOTH_CUT, "Smooth Cut") {
                    let smooth = json!({ "type": "smooth_cut" });
                    self.send_asking("edit.dissolve", json!({ "at": at, "transition": smooth }));
                }
                if edit_icon(ui, icon::DISSOLVE, "Dissolve") {
                    self.send("edit.dissolve", json!({ "at": at }));
                }
                if edit_icon(ui, icon::CUT, "Cut") {
                    self.send("edit.remove_transition", json!({ "at": at }));
                }
            });
        });
        let sources = self.edit_sources();
        let text = match sources.len() {
            0 if self.sync_grid_on() => "No source: select a camera".to_string(),
            0 => "No source: open a clip or select pool media".to_string(),
            1 => match self.snap.open_generator() {
                Some(g) => {
                    let (a, b) = g.range();
                    format!("Source: {} ({} long)", g.name(), self.src_tc(b - a))
                }
                None => {
                    let s = &sources[0];
                    let media = s["media"].as_str().unwrap_or_default();
                    let name = self
                        .project()
                        .and_then(|p| p.media(media))
                        .map_or(media.to_string(), |m| m.name.clone());
                    format!("Source: {} ({})", elide(&name, 30), range_text(self, s))
                }
            },
            n => format!("Source: {n} selected clips"),
        };
        ui.label(dim(text).small());
    }
}

mod icon {
    pub use egui_phosphor::regular::{
        ARROW_LINE_DOWN as SMART_INSERT, ARROW_LINE_RIGHT as APPEND,
        ARROWS_LEFT_RIGHT as RIPPLE_OVERWRITE, CLOCK as SOURCE_OVERWRITE, INTERSECT as DISSOLVE,
        LINE_VERTICAL as CUT, STACK as PLACE_ON_TOP, WAVE_SINE as SMOOTH_CUT,
    };
}

fn edit_icon(ui: &mut egui::Ui, glyph: &str, name: &str) -> bool {
    let r = ui
        .add(egui::Button::new(RichText::new(glyph).size(EDIT_ICON)))
        .on_hover_text(name);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    r.clicked()
}

fn range_text(app: &Workspace, source: &serde_json::Value) -> String {
    let r = source["range"].as_str().unwrap_or_default();
    let mut parts = r
        .split("..")
        .map(|x| x.trim_end_matches('f').parse::<i64>().unwrap_or(0));
    let (a, b) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    format!("{} long", app.src_tc(b - a))
}

pub(super) fn elide(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use vidcut_core::model::Settings;

    fn media(id: &str, name: &str, bin: &str) -> Media {
        serde_json::from_value(json!({
            "id": id,
            "path": format!("/{id}.mp4"),
            "name": name,
            "size": 0,
            "mtime_ns": 0,
            "info": { "format": "mov", "duration": 1.0, "start_time": 0.0, "video": null, "audio": [] },
            "bin": bin,
        }))
        .unwrap()
    }

    fn pool() -> Project {
        let mut p = Project::new(Settings::default());
        p.bins = vec!["Day 1".into(), "Day 1/Noon".into()];
        p.media = vec![
            media("m1", "Beach", ""),
            media("m2", "Beach walk", "Day 1/Noon"),
            media("m3", "Forest", "Day 1"),
        ];
        p
    }

    fn day_1() -> PoolView {
        PoolView {
            bin: "Day 1".into(),
            ..PoolView::default()
        }
    }

    #[test]
    fn the_strip_shows_the_clip_clicked_last_while_it_is_selected() {
        let selected = ["m1".to_string(), "m2".into(), "m3".into()];
        assert_eq!(super::chosen(Some("m1"), &selected).as_deref(), Some("m1"));
    }

    #[test]
    fn select_all_takes_what_a_search_finds_in_the_bin_and_the_bins_inside_it() {
        let p = pool();
        let ids: Vec<&str> = shown_media(&p, &day_1(), "beach")
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["m2"]);
    }

    #[test]
    fn select_all_without_a_search_takes_the_open_bins_own_media() {
        let p = pool();
        let ids: Vec<&str> = shown_media(&p, &day_1(), "")
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["m3"]);
    }

    #[test]
    fn a_click_on_the_sorted_columns_header_reverses_the_order() {
        assert_eq!(
            header_sort(0, PoolSort::Name, false),
            Some((PoolSort::Name, true))
        );
    }

    #[test]
    fn a_click_on_another_columns_header_sorts_by_it_ascending() {
        assert_eq!(
            header_sort(1, PoolSort::Name, true),
            Some((PoolSort::Duration, false))
        );
    }

    #[test]
    fn a_click_on_the_date_header_sorts_by_the_files_time() {
        assert_eq!(
            header_sort(2, PoolSort::Name, false),
            Some((PoolSort::Date, false))
        );
    }

    #[test]
    fn a_click_on_a_header_without_a_sort_does_nothing() {
        assert_eq!(header_sort(3, PoolSort::Name, false), None);
    }

    #[test]
    fn each_place_on_a_filmstrip_shows_the_frame_at_its_middle() {
        assert_eq!(strip_frames(4, 100), vec![12, 37, 62, 87]);
    }

    #[test]
    fn the_pools_usual_width_fits_two_tiles_a_row() {
        assert_eq!(per_row(418.0), 2);
    }

    #[test]
    fn a_pool_narrower_than_a_tile_still_shows_one_a_row() {
        assert_eq!(per_row(100.0), 1);
    }

    #[test]
    fn only_the_rows_in_view_are_laid_out() {
        assert_eq!(shown(Rangef::new(300.0, 700.0), 0.0, 137.0, 100), 2..6);
    }

    #[test]
    fn the_rows_laid_out_end_at_the_last() {
        assert_eq!(shown(Rangef::new(300.0, 700.0), 0.0, 137.0, 4), 2..4);
    }

    #[test]
    fn scrubbing_a_long_clip_steps_by_whole_frames_per_point() {
        assert_eq!(scrub_frame(1000, 18_000, 400.0), 992);
    }

    #[test]
    fn bin_drop_to_names_the_moved_bin() {
        assert_eq!(
            bin_drop_to("Day 1", "Interviews").as_deref(),
            Some("Interviews/Day 1")
        );
        assert_eq!(bin_drop_to("A/B", "C").as_deref(), Some("C/B"));
        assert_eq!(bin_drop_to("Hold/Inner", "").as_deref(), Some("Inner"));
        assert_eq!(
            bin_drop_to("Bin 1", "Bin 2").as_deref(),
            Some("Bin 2/Bin 1")
        );
    }

    #[test]
    fn bin_drop_to_rejects_self_descendant_and_parent() {
        assert_eq!(bin_drop_to("A", "A"), None, "onto itself");
        assert_eq!(bin_drop_to("A", "A/Child"), None, "into a descendant");
        assert_eq!(
            bin_drop_to("A/B", "A/B/C"),
            None,
            "into a nested descendant"
        );
        assert_eq!(bin_drop_to("A/B", "A"), None, "onto its parent");
        assert_eq!(bin_drop_to("Top", ""), None, "a top-level bin onto Master");
        assert_eq!(bin_drop_to("", "A"), None, "Master is not a bin to drag");
    }
}
