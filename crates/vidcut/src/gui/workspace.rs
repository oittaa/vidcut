use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Key, RichText};
use serde_json::{Value, json};
use vidcut_core::model::{PictureTool, Project};
use vidcut_core::request::ViewerMode;
use vidcut_core::time::{Rational, format_timecode};

pub use super::timeline::tools::{DynamicMode, DynamicTarget, heard};
pub use super::timeline::{EditSel, Side, TrackRef};
pub use super::viewer::{TwoUp, TwoUpCell, TwoUpMode};

use super::shuttle::Jkl;
use super::theme::{self, Deco};
use super::timeline::TimelineView;
use super::title_browser::LeftTab;
use crate::live::{self, Filmstrip, PlayerId, Snapshot};
use crate::when::{LocalTime, local_time};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TimecodeEntry {
    pub sign: Option<i64>,
    pub text: String,
    pub focus: bool,
}

pub(super) struct Thumbs {
    pub strip: Arc<Filmstrip>,
    pub textures: Vec<egui::TextureHandle>,
}

impl Thumbs {
    pub(super) fn nearest(&self, secs: f64) -> Option<egui::TextureId> {
        let i = self
            .strip
            .times
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - secs).abs().total_cmp(&(b.1 - secs).abs()))?
            .0;
        self.textures.get(i).map(egui::TextureHandle::id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Drag {
    Scrub(PlayerId),
    SourceMark {
        media: String,
        is_in: bool,
        frame: i64,
    },
    TapeMark {
        is_in: bool,
        frame: i64,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Picture {
    pub texture: Option<egui::TextureId>,
    pub has_frame: bool,
}

pub struct Workspace {
    pub(super) live: live::Handle,
    pub(super) snap: Arc<Snapshot>,
    pub(super) thumbs: HashMap<String, Thumbs>,
    pub picture: Picture,
    pub want: [i32; 2],
    pub stats: Option<live::stats::PlayerStats>,
    pub display: Option<String>,
    pub levels: Option<vidcut_av::player::Levels>,
    pub(super) search: String,
    pub(super) bins: super::pool::BinsUi,
    pub(super) timecode: Option<TimecodeEntry>,
    pub(super) duration_text: Option<String>,
    pub(super) show_log: bool,
    pub(super) log_edits_only: bool,
    pub(super) hover: Option<(String, i64)>,
    pub(super) pool_clicked: Option<String>,
    pub(super) left: LeftTab,
    pub(super) titles: super::title_browser::Browser,
    pub(super) inspector: super::inspector::Inspector,
    pub(super) title_thumbs: super::title_browser::Thumbs,
    pub(super) looks: super::looks::Looks,
    pub(super) scrub: Option<(PlayerId, i64)>,
    pub(super) recent: Vec<PathBuf>,
    pub(super) flash: Option<(String, std::time::Instant)>,
    notice_shown: Option<(u64, f64)>,
    notices_done: u64,
    title: String,
    theme: Option<Arc<crate::themes::Theme>>,
    pub(super) pool_buttons: f32,
    pub(super) pool_focus: bool,
    pub(super) removing: Option<Vec<String>>,
    pub(super) video_only: bool,
    pub(super) audio_only: bool,
    pub(super) target: Option<String>,
    pub(super) drag: Option<Drag>,
    pub(super) timeline: TimelineView,
    pub(super) trim_anchor: Option<super::trim_strips::TrimAnchor>,
    pub(super) cells: Option<super::cells::Cells>,
    pub(super) screens: Option<super::cells::Screens>,
    pub(super) two_up: Option<TwoUp>,
    pub(super) export: super::export::ExportUi,
    pub(super) show_mixer: bool,
    pub(super) mixer: super::mixer::Mixer,
    pub(super) preferences: super::preferences::PreferencesUi,
    pub(super) help: super::help::HelpUi,
    pub(super) history: super::history::HistoryUi,
    pub(super) project_ui: super::project::ProjectUi,
    pub(super) import: super::import::ImportUi,
    pub(super) relink: super::relink::RelinkUi,
    pub(super) dialogs: Arc<dyn super::export::Dialogs>,
    pub(super) folders: super::folders::Folders,
    pub(super) tiles: Option<super::tiles::Tiles>,
    pub(super) shuttle: super::shuttle::Shuttle,
    pub(super) speech: super::speech::SpeechUi,
    pub(super) isolation: super::isolation::IsolationUi,
    pub(super) ducking: super::ducking::DuckingUi,
    pub(super) transcription: super::transcription::TranscriptionUi,
    pub(super) clip_info: super::clip_info::ClipInfo,
    pub(super) metadata_strip: bool,
}

fn edited_text(edited: &LocalTime, today: &LocalTime) -> String {
    if (edited.year, edited.month, edited.day) == (today.year, today.month, today.day) {
        format!("edited {:02}:{:02}", edited.hour, edited.minute)
    } else {
        format!("edited {}", edited.minutes())
    }
}

fn decoding(hwdec: &str) -> Option<String> {
    Some(match hwdec {
        "" => return None,
        "no" => "decoding on the CPU".into(),
        other => format!(
            "decoding on the GPU ({})",
            vidcut_av::hw::Kind::named(other).map_or(other, |k| k.label())
        ),
    })
}

impl Workspace {
    pub fn new(live: live::Handle) -> Self {
        Self {
            snap: live.snapshot(),
            live,
            thumbs: HashMap::new(),
            picture: Picture::default(),
            stats: None,
            want: [640, 360],
            display: None,
            levels: None,
            search: String::new(),
            bins: Default::default(),
            timecode: None,
            duration_text: None,
            show_log: false,
            log_edits_only: true,
            hover: None,
            pool_clicked: None,
            left: Default::default(),
            titles: Default::default(),
            inspector: Default::default(),
            title_thumbs: Default::default(),
            looks: Default::default(),
            scrub: None,
            recent: recent_projects(),
            flash: None,
            notice_shown: None,
            notices_done: 0,
            title: String::new(),
            theme: None,
            pool_buttons: 84.0,
            pool_focus: false,
            removing: None,
            video_only: false,
            audio_only: false,
            target: None,
            drag: None,
            timeline: TimelineView {
                options: super::timeline::Options::load(),
                ..TimelineView::default()
            },
            trim_anchor: None,
            cells: None,
            screens: None,
            two_up: None,
            export: Default::default(),
            show_mixer: false,
            mixer: Default::default(),
            preferences: Default::default(),
            help: Default::default(),
            history: super::history::HistoryUi::load(),
            project_ui: Default::default(),
            import: Default::default(),
            relink: Default::default(),
            dialogs: Arc::new(super::export::RfdDialogs),
            folders: super::folders::Folders::load(),
            tiles: Some(super::tiles::Tiles::new(
                &crate::preferences::load().unwrap_or_default().thumbnails,
            )),
            shuttle: Default::default(),
            speech: Default::default(),
            isolation: Default::default(),
            ducking: Default::default(),
            transcription: Default::default(),
            clip_info: Default::default(),
            metadata_strip: super::pool::strip_kept(),
        }
    }

    pub(super) fn project_folder(&self) -> Option<PathBuf> {
        self.snap
            .project
            .as_ref()
            .and_then(|p| p.path.parent().map(Path::to_path_buf))
            .filter(|d| !d.as_os_str().is_empty())
    }

    pub fn show_panel(&mut self, panel: vidcut_core::request::GuiPanel, ctx: &egui::Context) {
        use vidcut_core::request::GuiPanel;
        if self.project().is_none()
            && matches!(
                panel,
                GuiPanel::QuickExport | GuiPanel::Export | GuiPanel::Mixer
            )
        {
            return;
        }
        match panel {
            GuiPanel::QuickExport => self.open_quick_export(ctx),
            GuiPanel::Export => self.open_export(ctx),
            GuiPanel::Jobs => self.export.show_jobs = true,
            GuiPanel::Log => self.show_log = true,
            GuiPanel::Mixer => self.show_mixer = true,
            GuiPanel::Preferences => self.open_preferences(ctx),
            GuiPanel::Help => self.help.open = true,
            GuiPanel::History => self.history.show(),
            GuiPanel::Transcription => self.transcription.open = true,
            GuiPanel::Titles => self.left = LeftTab::Titles,
            GuiPanel::Inspector => self.left = LeftTab::Inspector,
            GuiPanel::Metadata => self.show_metadata_strip(true),
            GuiPanel::ClipInfo => self.open_clip_info(None),
            GuiPanel::Transform => self.show_picture_tool(PictureTool::Transform),
            GuiPanel::Crop => self.show_picture_tool(PictureTool::Crop),
            GuiPanel::DynamicZoom => self.show_picture_tool(PictureTool::DynamicZoom),
            GuiPanel::Tools => self.timeline.picture_grab.row = true,
        }
    }

    pub fn gui_change(
        &mut self,
        changes: Vec<live::GuiChange>,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        use vidcut_core::request::GuiPanel;
        for change in changes {
            match change {
                live::GuiChange::SelectEdit { near, track, side } => {
                    self.select_edit(near, &track, side.into())?
                }
                live::GuiChange::EditSide(side) => self.set_edit_side(side.into())?,
                live::GuiChange::Deselect => self.timeline.edit = None,
                live::GuiChange::Target(t) => {
                    self.target = (t != vidcut_core::ops::MAIN).then_some(t)
                }
                live::GuiChange::Timeline(o) => self.set_timeline_options(&o)?,
                live::GuiChange::Show(
                    GuiPanel::QuickExport | GuiPanel::Export | GuiPanel::Mixer,
                ) if self.project().is_none() => {
                    return Err("Open a project first".into());
                }
                live::GuiChange::Show(panel) => self.show_panel(panel, ctx),
                live::GuiChange::Close(panel) => self.close_panel(panel),
            }
        }
        Ok(())
    }

    pub fn close_panel(&mut self, panel: vidcut_core::request::GuiPanel) {
        use vidcut_core::request::GuiPanel;
        match panel {
            GuiPanel::QuickExport => self.export.close_quick(),
            GuiPanel::Export => self.export.close_dialog(),
            GuiPanel::Jobs => self.export.show_jobs = false,
            GuiPanel::Log => self.show_log = false,
            GuiPanel::Mixer => self.show_mixer = false,
            GuiPanel::Preferences => self.preferences.open = false,
            GuiPanel::Help => self.help.open = false,
            GuiPanel::History => self.history.close(),
            GuiPanel::Transcription => self.transcription.open = false,
            GuiPanel::Titles | GuiPanel::Inspector => {
                let tab = match panel {
                    GuiPanel::Titles => LeftTab::Titles,
                    _ => LeftTab::Inspector,
                };
                if self.left == tab {
                    self.left = LeftTab::Pool;
                }
            }
            GuiPanel::Metadata => self.show_metadata_strip(false),
            GuiPanel::ClipInfo => self.clip_info.open = false,
            GuiPanel::Transform | GuiPanel::Crop | GuiPanel::DynamicZoom => {
                let tool = match panel {
                    GuiPanel::Crop => PictureTool::Crop,
                    GuiPanel::DynamicZoom => PictureTool::DynamicZoom,
                    _ => PictureTool::Transform,
                };
                let g = &mut self.timeline.picture_grab;
                if g.tool == Some(tool) {
                    g.tool = None;
                }
            }
            GuiPanel::Tools => self.timeline.picture_grab.row = false,
        }
    }

    pub fn gui_state(&self) -> live::GuiState {
        use vidcut_core::request::GuiPanel;
        let (quick, dialog) = self.export.windows();
        let tool = self.timeline.picture_grab.tool;
        let open = [
            (GuiPanel::QuickExport, quick),
            (GuiPanel::Export, dialog),
            (GuiPanel::Jobs, self.export.show_jobs),
            (GuiPanel::Log, self.show_log),
            (GuiPanel::Mixer, self.show_mixer),
            (GuiPanel::Preferences, self.preferences.open),
            (GuiPanel::Help, self.help.open),
            (GuiPanel::History, self.history.open),
            (GuiPanel::Transcription, self.transcription.open),
            (GuiPanel::Titles, self.left == LeftTab::Titles),
            (GuiPanel::Inspector, self.left == LeftTab::Inspector),
            (GuiPanel::Metadata, self.metadata_strip),
            (GuiPanel::ClipInfo, self.clip_info.open),
            (GuiPanel::Transform, tool == Some(PictureTool::Transform)),
            (GuiPanel::Crop, tool == Some(PictureTool::Crop)),
            (
                GuiPanel::DynamicZoom,
                tool == Some(PictureTool::DynamicZoom),
            ),
            (GuiPanel::Tools, self.timeline.picture_grab.row),
        ];
        live::GuiState {
            edit: self.edit_point(),
            target: self
                .target
                .clone()
                .unwrap_or_else(|| vidcut_core::ops::MAIN.into()),
            timeline: self.timeline_state(),
            windows: open
                .into_iter()
                .filter_map(|(panel, open)| open.then_some(panel))
                .collect(),
        }
    }

    pub fn set_dialogs(&mut self, dialogs: Arc<dyn super::export::Dialogs>) {
        self.dialogs = dialogs;
    }

    pub fn timeline_layout(&self) -> Option<super::timeline::Layout> {
        self.timeline.layout.clone()
    }

    pub fn timeline_tiles(&self) -> (usize, usize) {
        self.tiles
            .as_ref()
            .map_or((0, 0), super::tiles::Tiles::usage)
    }

    pub fn timeline_tiles_pending(&self) -> usize {
        self.tiles.as_ref().map_or(0, super::tiles::Tiles::pending)
    }

    pub fn selected_edit(&self) -> Option<i64> {
        self.timeline.edit.map(|e| e.at)
    }

    pub fn timeline_options_mut(&mut self) -> &mut super::timeline::Options {
        &mut self.timeline.options
    }

    pub fn two_up(&self) -> Option<&TwoUp> {
        self.two_up.as_ref()
    }

    pub fn two_up_stats(&self) -> crate::live::stats::TwoUpStats {
        use crate::live::stats::{TwoUpCell, TwoUpShown, TwoUpStats};
        let counts = self.cells.as_ref().map(|s| s.counts()).unwrap_or_default();
        TwoUpStats {
            shown: self.two_up.as_ref().map(|t| TwoUpShown {
                mode: t.mode.to_string(),
                cells: t
                    .cells
                    .iter()
                    .map(|c| {
                        c.as_ref().map(|c| TwoUpCell {
                            frame: c.frame,
                            source_frame: c.source_frame,
                            exact: c.exact,
                        })
                    })
                    .collect(),
            }),
            asked: counts.asked,
            passed_over: counts.passed_over,
        }
    }

    pub(super) fn send(&self, method: &str, params: Value) {
        self.live.request(method, params);
    }

    pub(super) fn show_timeline(&self) {
        if self.snap.viewer != vidcut_core::request::ViewerMode::Timeline {
            self.send("viewer.set", json!({ "mode": "timeline" }));
        }
    }

    pub(super) fn send_drag(&self, method: &str, params: Value) {
        self.live.request_drag(method, params);
    }

    pub(super) fn project(&self) -> Option<&Project> {
        self.snap.project()
    }

    pub(super) fn project_shared(&self) -> Option<Arc<Project>> {
        self.snap.project.as_ref().map(|s| s.project.clone())
    }

    pub(super) fn fps(&self) -> Rational {
        self.project()
            .map_or(Rational::new(30, 1).expect("valid"), |p| p.settings.fps)
    }

    pub(super) fn dropped(&self) -> i64 {
        match self.project() {
            Some(p) if p.settings.counts_drop_frame() => p.settings.fps.dropped_per_minute(),
            _ => 0,
        }
    }

    pub(super) fn tc(&self, frame: i64) -> String {
        match self.project() {
            Some(p) => p.settings.timecode(frame),
            None => format_timecode(frame, self.fps(), 0, false),
        }
    }

    pub(super) fn src_tc(&self, frame: i64) -> String {
        match self.project() {
            Some(p) => p.settings.length_timecode(frame),
            None => format_timecode(frame, self.fps(), 0, false),
        }
    }

    pub(super) fn media_frames(&self, media: &str) -> i64 {
        self.project()
            .and_then(|p| p.media(media).map(|m| p.media_frames(m)))
            .unwrap_or(0)
    }

    pub(super) fn tape_order(&self) -> Vec<String> {
        self.snap.tape.iter().map(|e| e.media.clone()).collect()
    }

    pub(super) fn crossing_span(&self) -> Option<(i64, i64)> {
        let p = self.project()?;
        if p.tape_marks.order != self.tape_order() {
            return None;
        }
        vidcut_core::render::crossing_span(
            &self.snap.tape,
            p.tape_marks.mark_in,
            p.tape_marks.mark_out,
        )
    }

    pub(super) fn edit_sources(&self) -> Vec<Value> {
        if self.snap.viewer == ViewerMode::SourceTape && self.crossing_span().is_some() {
            return vec![json!({ "tape": true })];
        }
        let Some(p) = self.project() else {
            return Vec::new();
        };
        let range = |media: &str| -> Option<Value> {
            let m = p.media(media)?;
            let frames = p.media_frames(m);
            let a = m.marks.mark_in.unwrap_or(0);
            let b = m.marks.mark_out.unwrap_or(frames);
            (b > a).then(|| json!({ "media": media, "range": format!("{a}f..{b}f") }))
        };
        match self.snap.source_at() {
            Some((media, _)) => range(media).into_iter().collect(),
            None => self
                .snap
                .selection
                .media
                .iter()
                .filter_map(|m| range(m))
                .collect(),
        }
    }

    pub(super) fn forget_view_state(&mut self) -> ViewState {
        ViewState {
            folders: self.folders.forget(),
            options: std::mem::take(&mut self.timeline.options),
            recent: std::mem::take(&mut self.recent),
        }
    }

    pub(super) fn restore_view_state(&mut self, old: ViewState) {
        self.folders.restore(old.folders);
        self.timeline.options = old.options;
        self.recent = old.recent;
    }

    pub(super) fn after_reset(&mut self) {
        self.folders = super::folders::Folders::load();
        self.timeline.options = super::timeline::Options::load();
        self.recent = recent_projects();
    }

    pub(super) fn say(&mut self, text: impl Into<String>) {
        self.flash = Some((text.into(), std::time::Instant::now()));
    }

    pub(super) fn edit_function(&mut self, method: &str) {
        let sources = self.edit_sources();
        let playhead = self.snap.timeline.position;
        let with_position = |mut s: Value| {
            match method {
                "edit.smart_insert" => s["playhead"] = json!(playhead),
                "edit.overwrite" | "edit.ripple_overwrite" | "edit.place_on_top" => {
                    s["at"] = json!(playhead)
                }
                _ => {}
            }
            s
        };
        let (video_only, audio_only) = (self.video_only, self.audio_only);
        let target = self
            .target
            .clone()
            .filter(|_| !matches!(method, "edit.append" | "edit.place_on_top"))
            .filter(|t| self.project().is_some_and(|p| p.track(t).is_some()));
        let sources: Vec<Value> = sources
            .into_iter()
            .map(|mut s| {
                if video_only {
                    s["video_only"] = json!(true);
                }
                if audio_only {
                    s["audio_only"] = json!(true);
                }
                if let Some(t) = &target {
                    s["track"] = json!(t);
                }
                s
            })
            .collect();
        match sources.len() {
            0 => self.say("Open a clip or select pool media first"),
            1 => self.send(
                method,
                with_position(sources.into_iter().next().expect("one")),
            ),
            _ if method == "edit.append" => {
                let commands: Vec<Value> = sources
                    .into_iter()
                    .map(|s| json!({ "method": "edit.append", "params": s }))
                    .collect();
                let label = format!("append {} clips", commands.len());
                self.send("apply", json!({ "commands": commands, "label": label }));
            }
            _ => self.send(
                method,
                with_position(sources.into_iter().next().expect("one")),
            ),
        }
    }

    pub(super) fn seek(&self, player: PlayerId, frame: i64, fast: bool) {
        let viewer = match player {
            PlayerId::Timeline => ViewerMode::Timeline,
            PlayerId::Source => self.snap.last_source,
        };
        self.send(
            "player.seek",
            json!({ "at": frame.max(0), "viewer": viewer, "fast": fast }),
        );
    }

    fn sync_textures(&mut self, ctx: &egui::Context) {
        let thumbs = self.snap.thumbs.clone();
        self.thumbs.retain(|m, _| thumbs.contains_key(m));
        for (media, strip) in thumbs.iter() {
            if self
                .thumbs
                .get(media)
                .is_some_and(|t| Arc::ptr_eq(&t.strip, strip))
            {
                continue;
            }
            let size = [strip.width as usize, strip.height as usize];
            let textures = strip
                .frames
                .iter()
                .enumerate()
                .map(|(i, rgba)| {
                    ctx.load_texture(
                        format!("thumb-{media}-{i}"),
                        egui::ColorImage::from_rgba_unmultiplied(size, rgba),
                        egui::TextureOptions::LINEAR,
                    )
                })
                .collect();
            self.thumbs.insert(
                media.clone(),
                Thumbs {
                    strip: strip.clone(),
                    textures,
                },
            );
        }
    }

    fn notices(&mut self, ui: &mut egui::Ui) {
        if let Some((text, at)) = &self.flash {
            if at.elapsed().as_secs() < 6 {
                ui.label(RichText::new(text).color(theme::colors().accent));
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(1));
                return;
            }
            self.flash = None;
        }
        let Some(n) = self
            .snap
            .notices
            .back()
            .filter(|n| n.seq > self.notices_done)
        else {
            return;
        };
        let now = ui.input(|i| i.time);
        let shown = match self.notice_shown {
            Some((seq, at)) if seq == n.seq => at,
            _ => now,
        };
        let hold = if n.error { 20.0 } else { 6.0 };
        let age = now - shown;
        if age >= hold + NOTICE_FADE {
            self.notices_done = n.seq;
            return;
        }
        let opacity = (1.0 - (age - hold) / NOTICE_FADE).clamp(0.0, 1.0) as f32;
        let color = if n.error {
            theme::colors().error
        } else {
            theme::colors().text_dim
        };
        let text = ui
            .scope(|ui| {
                ui.set_max_width((ui.available_width() - 30.0).max(0.0));
                ui.add(
                    egui::Label::new(RichText::new(&n.text).color(color.gamma_multiply(opacity)))
                        .truncate(),
                )
            })
            .inner;
        let close = ui.add(
            egui::Button::new(
                RichText::new(egui_phosphor::regular::X)
                    .color(theme::colors().text_dim.gamma_multiply(opacity)),
            )
            .frame(false)
            .small(),
        );
        close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Dismiss"));
        if close.clicked() {
            self.notices_done = n.seq;
            return;
        }
        let shown = if text.hovered() || close.hovered() {
            now - age.min(hold)
        } else {
            shown
        };
        self.notice_shown = Some((n.seq, shown));
        let wait = if age < hold { hold - age } else { 1.0 / 30.0 };
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(wait));
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Project…").clicked() {
                    self.open_new_project();
                }
                if ui.button("Open Project…").clicked() {
                    self.pick_project(ui.ctx(), None);
                }
                ui.menu_button("Open Recent", |ui| {
                    for p in self.recent.clone() {
                        if ui.button(p.display().to_string()).clicked() {
                            self.send("project.open", json!({ "path": p }));
                        }
                    }
                });
                ui.separator();
                let open = self.project().is_some();
                if !open {
                    super::dialog::why(ui, "open a project for these");
                }
                if ui
                    .add_enabled(open, egui::Button::new("Project Settings…"))
                    .clicked()
                {
                    self.open_project_settings();
                }
                ui.separator();
                if ui
                    .add_enabled(open, egui::Button::new("Import Media…"))
                    .clicked()
                {
                    self.import_media();
                }
                if ui
                    .add_enabled(open, egui::Button::new("Import Media Folder…"))
                    .clicked()
                {
                    self.import_folder();
                }
                if ui
                    .add_enabled(open, egui::Button::new("Import Subtitle…"))
                    .clicked()
                {
                    self.import_subtitle(ui.ctx());
                }
                if ui.add_enabled(open, egui::Button::new("New Bin")).clicked() {
                    self.new_bin();
                }
                if ui
                    .add_enabled(open, egui::Button::new("Import Timeline (OTIO)…"))
                    .clicked()
                {
                    self.import_timeline(ui.ctx(), false);
                }
                if ui
                    .add_enabled(
                        open,
                        egui::Button::new("Import Timeline, Decomposing Compound Clips…"),
                    )
                    .clicked()
                {
                    self.import_timeline(ui.ctx(), true);
                }
                if ui
                    .add_enabled(open, egui::Button::new("Export Timeline (OTIO)…"))
                    .clicked()
                {
                    self.export_timeline(ui.ctx(), false);
                }
                if ui
                    .add_enabled(
                        open,
                        egui::Button::new("Export Timeline with Media (OTIOZ)…"),
                    )
                    .clicked()
                {
                    self.export_timeline(ui.ctx(), true);
                }
                let subtitles = self.project().is_some_and(|p| !p.subtitles.is_empty());
                if ui
                    .add_enabled(subtitles, egui::Button::new("Export Subtitle…"))
                    .clicked()
                {
                    self.export_subtitle(ui.ctx(), None);
                }
                if open && !subtitles {
                    super::dialog::why(ui, "no subtitle track yet");
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("Preferences…").shortcut_text(super::keys("Ctrl+,")))
                    .clicked()
                {
                    self.open_preferences(ui.ctx());
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                let (undo, redo) = self
                    .snap
                    .project
                    .as_ref()
                    .map(|p| {
                        let label = |s: &vidcut_core::steps::Step| s.label.clone();
                        (p.history.undo().map(label), p.history.redo().map(label))
                    })
                    .unwrap_or_default();
                let named = |verb: &str, label: &Option<String>| match label {
                    Some(l) => format!("{verb} {l}"),
                    None => verb.to_string(),
                };
                let stripes = self
                    .timeline
                    .silence
                    .as_ref()
                    .map(|d| (d.can_undo(), d.can_redo()));
                let (undo, redo) = match stripes {
                    Some((u, r)) => (
                        u.then(|| "Stripe Change".to_string()),
                        r.then(|| "Stripe Change".to_string()),
                    ),
                    None => (undo, redo),
                };
                if ui
                    .add_enabled(
                        undo.is_some(),
                        egui::Button::new(named("Undo", &undo))
                            .shortcut_text(super::keys("Ctrl+Z")),
                    )
                    .clicked()
                {
                    match &mut self.timeline.silence {
                        Some(d) => d.undo(),
                        None => self.send("history.undo", json!({})),
                    }
                }
                if ui
                    .add_enabled(
                        redo.is_some(),
                        egui::Button::new(named("Redo", &redo))
                            .shortcut_text(super::keys("Ctrl+Shift+Z")),
                    )
                    .clicked()
                {
                    match &mut self.timeline.silence {
                        Some(d) => d.redo(),
                        None => self.send("history.redo", json!({})),
                    }
                }
                ui.add_enabled_ui(self.snap.project.is_some(), |ui| {
                    ui.menu_button("History", |ui| self.history_menu(ui));
                });
            });
            ui.menu_button("Timeline", |ui| {
                let mut on = self.snap.audio_scrubbing;
                if ui.checkbox(&mut on, "Audio Scrubbing").clicked() {
                    self.send("player.audio_scrubbing", json!({ "on": on }));
                }
                match self.project_shared() {
                    Some(p) => {
                        ui.separator();
                        self.timeline_actions_menu(ui, &p);
                    }
                    None => super::dialog::why(ui, "open a project for the rest"),
                }
            });
            ui.separator();
            self.bar_buttons(ui);
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        use Status as S;
        let decoding = decoding(&self.snap.active().hwdec);
        let open = self.project().is_some();
        let right: Vec<Status> = [
            open.then_some(S::Format),
            open.then_some(S::Edited),
            decoding.is_some().then_some(S::Decoding),
            self.snap.socket.is_some().then_some(S::Pid),
            Some(S::Version),
        ]
        .into_iter()
        .flatten()
        .collect();
        let row = super::row::Row {
            left: &[],
            fill: Some((S::Notices, egui::Align::Min)),
            right: &right,
            leaving: &[S::Pid, S::Decoding, S::Format, S::Edited, S::Version],
            group: |s| s as u8,
        };
        let dim = |text: String| RichText::new(text).color(theme::colors().text_dim);
        ui.horizontal(|ui| {
            self.fit_row(ui, row, |ws, ui, s| match s {
                S::Notices => ui.scope(|ui| ws.notices(ui)).response,
                S::Format => {
                    let p = ws.project().expect("open");
                    ui.label(dim(format!(
                        "{}×{}, {} fps",
                        p.settings.width,
                        p.settings.height,
                        p.settings.fps.decimal()
                    )))
                }
                S::Edited => {
                    let p = ws.project().expect("open");
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as u64);
                    match (p.edited.and_then(local_time), local_time(now)) {
                        (Some(edited), Some(today)) => ui.label(dim(edited_text(&edited, &today))),
                        _ => ui.label(dim(format!("revision {}", p.rev))),
                    }
                }
                S::Decoding => ui.label(dim(decoding.clone().expect("decoding"))),
                S::Pid => ui.label(dim(format!("pid {}", ws.snap.pid))),
                S::Version => ui.label(dim(ws.version())),
            });
        });
    }

    fn version(&self) -> String {
        format!(
            "vidcut {} · FFmpeg {}",
            env!("CARGO_PKG_VERSION"),
            vidcut_av::ffmpeg_release()
        )
    }

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.label(RichText::new("vidcut").size(36.0).strong());
            ui.label(RichText::new("a scriptable video editor").color(theme::colors().text_dim));
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                if ui.button("New Project…").clicked() {
                    self.open_new_project();
                }
                if ui.button("Open Project…").clicked() {
                    self.pick_project(ui.ctx(), None);
                }
            });
            if !self.recent.is_empty() {
                ui.add_space(24.0);
                ui.label(RichText::new("Recent").color(theme::colors().text_dim));
                for p in self.recent.clone() {
                    if ui.link(p.display().to_string()).clicked() {
                        self.send("project.open", json!({ "path": p }));
                    }
                }
            }
        });
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Command log");
            ui.checkbox(&mut self.log_edits_only, "edits only");
        });
        ui.label(
            RichText::new("Click an entry to copy it as a command.")
                .color(theme::colors().text_dim)
                .small(),
        );
        ui.separator();
        let project = self.snap.project.as_ref().map(|p| p.path.clone());
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for e in self.snap.log.iter() {
                    if self.log_edits_only && !e.changed && e.ok {
                        continue;
                    }
                    let color = if e.ok {
                        theme::colors().text
                    } else {
                        theme::colors().error
                    };
                    let who = match e.origin.as_str() {
                        "ipc" => "script",
                        "session" => "automatic",
                        _ => "",
                    };
                    let r = ui
                        .horizontal_wrapped(|ui| {
                            if !who.is_empty() {
                                ui.label(
                                    RichText::new(who).small().color(theme::colors().text_dim),
                                );
                            }
                            let r = ui.add(
                                egui::Label::new(RichText::new(&e.summary).small().color(color))
                                    .sense(egui::Sense::click()),
                            );
                            if let Some(err) = &e.error {
                                ui.label(RichText::new(err).small().color(color));
                            }
                            r
                        })
                        .inner;
                    if r.clicked() {
                        ui.ctx().copy_text(vidcut_command(
                            project.as_deref(),
                            &e.method,
                            &e.params,
                        ));
                    }
                }
            });
    }

    pub(super) fn pick_project(&self, ctx: &egui::Context, new: Option<Value>) {
        use super::folders::Kind;
        let (live, dialogs, folders) = (
            self.live.clone(),
            self.dialogs.clone(),
            self.folders.clone(),
        );
        let (found, ctx) = (self.project_ui.found.clone(), ctx.clone());
        let folder = folders
            .get(Kind::Projects)
            .or_else(|| self.project_folder())
            .or_else(|| {
                self.recent
                    .first()
                    .and_then(|p| p.parent().map(Path::to_path_buf))
            })
            .unwrap_or_default();
        let _ = crate::threads::spawn(
            "vidcut-project-dialog",
            crate::threads::Work::Interactive,
            move || {
                let path = if new.is_some() {
                    dialogs.save_file("New Project", &folder, "untitled.vidcut", &["vidcut"])
                } else {
                    dialogs.pick_file("Open Project", &folder, &["vidcut", "otio", "otioz"])
                };
                if let Some(path) = path
                    .as_ref()
                    .filter(|p| new.is_none() && vidcut_core::editor::is_timeline_file(p))
                {
                    folders.remember(Kind::Projects, path);
                    let file = super::project::OtioFile::read(path.clone());
                    *found.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
                    ctx.request_repaint();
                } else if let Some(mut path) = path {
                    let mut params = json!({ "path": path });
                    if let Some(format) = new {
                        let force = path.extension().is_some();
                        if path.extension().is_none() {
                            path.set_extension("vidcut");
                        }
                        params = json!({ "path": path, "create": true, "force": force });
                        if let (Some(p), Some(f)) = (params.as_object_mut(), format.as_object()) {
                            p.extend(f.clone());
                        }
                    }
                    folders.remember(Kind::Projects, &path);
                    live.request("project.open", params);
                }
            },
        );
    }

    pub fn asking(&self) -> bool {
        self.import.asking() || self.export.asking(&self.snap.jobs) || self.removing.is_some()
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if vidcut_av::debug() {
            ctx.input(|i| {
                for e in &i.events {
                    if let egui::Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        modifiers,
                        ..
                    } = e
                    {
                        eprintln!("[keys] {key:?} (physical {physical_key:?}) {modifiers:?}");
                    }
                }
            });
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if self.transcription.over {
            self.transcription.take_keys(ctx);
        }
        if self.asking() {
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Key {
                        key: Key::Escape,
                        pressed: true,
                        modifiers,
                        ..
                    } => !modifiers.any(),
                    egui::Event::Key { .. }
                    | egui::Event::Text(_)
                    | egui::Event::Copy
                    | egui::Event::Cut
                    | egui::Event::Paste(_) => false,
                    _ => true,
                });
            });
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::F1)) {
            self.help.open = !self.help.open;
        }
        if self.help.open && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            self.help.open = false;
        }
        self.history_keys(ctx);
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::Comma)) {
            self.open_preferences(ctx);
        }
        if self.snap.project.is_none() {
            return;
        }
        let input = ctx.input(|i| i.clone());
        let m = input.modifiers;
        let pressed = |k: Key| input.key_pressed(k);
        let viewer = self.snap.viewer;
        let id = PlayerId::of(viewer);
        let player = self.snap.player(id).clone();
        let fps = self.fps().as_f64().round().max(1.0) as i64;
        self.timeline_keys(&input);

        for e in &input.events {
            match e {
                egui::Event::Copy | egui::Event::Cut if !self.snap.selection.clips.is_empty() => {
                    let clips = self.snap.selection.clips.clone();
                    if let Some(p) = self.project()
                        && let Ok(data) = vidcut_core::ops::copy_clips(p, &clips)
                    {
                        ctx.copy_text(json!(data).to_string());
                    }
                    let method = if matches!(e, egui::Event::Cut) {
                        "edit.cut"
                    } else {
                        "edit.copy"
                    };
                    self.send(method, json!({ "clips": clips }));
                }
                egui::Event::Paste(_) => self.send("edit.paste", json!({})),
                _ => {}
            }
        }

        let dynamic = self.timeline.dynamic.is_some();
        let trimming = dynamic && self.dynamic_target().is_some();
        if dynamic {
            self.dynamic_keys(&input);
        }
        if pressed(Key::Space) && !dynamic {
            self.send("player.toggle", json!({}));
        }
        if pressed(Key::S) && m.shift && !m.command && !m.alt {
            self.send("player.audio_scrubbing", json!({}));
        }
        if pressed(Key::Slash) && m.command && !m.alt {
            self.send("player.loop", json!({}));
        }
        if super::shuttle::first_press(&input, Key::K) && !trimming {
            self.send("player.pause", json!({}));
        }
        for jkl in self.shuttle.input(&input) {
            if trimming {
                self.dynamic_jkl(jkl, input.time);
                continue;
            }
            match jkl {
                Jkl::Play(dir) => {
                    let speed = crate::gui::timeline::tools::faster(player.speed, dir);
                    self.send("player.play", json!({ "speed": speed }));
                }
                Jkl::Step(dir) => self.send("player.step", json!({ "frames": dir as i64 })),
                Jkl::Slow(speed) => self.send("player.play", json!({ "speed": speed })),
                Jkl::Stop => self.send("player.pause", json!({})),
            }
        }
        if let Some(wait) = self.shuttle.due(input.time) {
            super::timeline::repaint_in(ctx, std::time::Duration::from_secs_f64(wait));
        }
        if pressed(Key::ArrowLeft) {
            let n = if m.shift { fps } else { 1 };
            self.send("player.step", json!({ "frames": -n }));
        }
        if pressed(Key::ArrowRight) {
            let n = if m.shift { fps } else { 1 };
            self.send("player.step", json!({ "frames": n }));
        }
        if pressed(Key::Home) {
            self.seek(id, 0, false);
        }
        if pressed(Key::End) {
            self.seek(id, player.frames, false);
        }
        if pressed(Key::ArrowUp) || pressed(Key::ArrowDown) {
            let up = pressed(Key::ArrowUp);
            let dynamic_track = self.timeline.edit.filter(|_| dynamic).map(|e| e.track);
            let (id, player) = match dynamic_track {
                Some(_) => (
                    PlayerId::Timeline,
                    self.snap.player(PlayerId::Timeline).clone(),
                ),
                None => (id, player.clone()),
            };
            let points = match (m.shift, dynamic_track) {
                (true, _) => self.marker_points(),
                (false, Some(t)) => self.track_edit_points(&t.id()),
                (false, None) => self.edit_points(id),
            };
            let at = match id {
                PlayerId::Timeline => self.drawn_playhead(),
                PlayerId::Source => player.position,
            };
            let target = if up {
                points.iter().rev().find(|&&f| f < at)
            } else {
                points.iter().find(|&&f| f > at)
            };
            if let Some(&f) = target {
                self.seek(id, f, false);
                if dynamic && !m.shift {
                    self.dynamic_commit();
                    let (side, track) = self.timeline.edit.map_or(
                        (
                            crate::gui::timeline::Side::Both,
                            crate::gui::timeline::TrackRef::MAIN,
                        ),
                        |e| (e.side, e.track),
                    );
                    let at = if side == crate::gui::timeline::Side::Audio {
                        self.project()
                            .and_then(|p| p.track(&track.id()))
                            .map(|t| crate::gui::timeline::audio_at_cut(t, f))
                            .unwrap_or(f)
                    } else {
                        f
                    };
                    self.timeline.edit = Some(crate::gui::timeline::EditSel { at, side, track });
                }
            }
        }
        if pressed(Key::Q) && !m.command {
            let mode = if viewer == ViewerMode::Timeline {
                self.snap.last_source
            } else {
                ViewerMode::Timeline
            };
            self.send("viewer.set", json!({ "mode": mode }));
        }

        if (pressed(Key::I) || pressed(Key::O)) && !m.command {
            let is_in = pressed(Key::I);
            if m.shift {
                self.go_to_mark(is_in);
            } else {
                self.mark(is_in);
            }
        }
        if pressed(Key::X) && m.alt {
            self.clear_marks();
        }

        if pressed(Key::F9) {
            self.edit_function("edit.smart_insert");
        }
        if pressed(Key::F10) {
            self.edit_function(if m.shift {
                "edit.ripple_overwrite"
            } else {
                "edit.overwrite"
            });
        }
        if pressed(Key::F12) {
            self.edit_function(if m.shift {
                "edit.append"
            } else {
                "edit.place_on_top"
            });
        }
        if pressed(Key::T) && m.command && !m.shift && !m.alt {
            use super::timeline::Side;
            let params = match self.timeline.edit {
                Some(e) => {
                    let align = match e.side {
                        Side::Out => "end",
                        Side::In => "start",
                        Side::Both | Side::Audio => "center",
                    };
                    json!({
                        "at": e.at,
                        "track": e.track.id(),
                        "align": align,
                        "picture": e.side != Side::Audio,
                    })
                }
                None => json!({
                    "at": self.snap.timeline.position,
                    "clips": self.snap.selection.clips,
                }),
            };
            self.send("edit.dissolve", params);
        }
        if pressed(Key::T) && !m.command && !(m.alt && m.shift) {
            self.picture_tool(match (m.alt, m.shift) {
                (true, _) => PictureTool::DynamicZoom,
                (_, true) => PictureTool::Crop,
                _ => PictureTool::Transform,
            });
        }
        if (pressed(Key::Backslash) || pressed(Key::B)) && m.command {
            self.split_at_playhead();
        }
        if pressed(Key::Enter)
            && !m.any()
            && let [cue] = self.snap.selection.cues.as_slice()
        {
            let cue = cue.clone();
            self.open_cue(&cue);
        }
        if (pressed(Key::Delete) || pressed(Key::Backspace)) && !m.command {
            if self.pool_focus {
                let selected = self.snap.selection.media.clone();
                self.removing = (!selected.is_empty()).then_some(selected);
            } else {
                self.delete_key(m.alt);
            }
        }
        if pressed(Key::Z) && m.command {
            match (&mut self.timeline.silence, m.shift) {
                (Some(d), true) => d.redo(),
                (Some(d), false) => d.undo(),
                (None, true) => self.send("history.redo", json!({})),
                (None, false) => self.send("history.undo", json!({})),
            }
        }
        if pressed(Key::I) && m.command {
            self.import_media();
        }
        if pressed(Key::N) && m.command && m.shift {
            self.new_bin();
        }
        if pressed(Key::M) && !m.command {
            if m.alt {
                let at = self.snap.timeline.position;
                if let Some(k) = self
                    .project()
                    .and_then(|p| p.markers.iter().find(|k| k.at == at))
                {
                    self.send("markers.remove", json!({ "marker": k.id }));
                }
            } else {
                let at = self.snap.timeline.position;
                let existing = self
                    .project()
                    .and_then(|p| p.markers.iter().find(|k| k.at == at))
                    .map(|k| k.id.clone());
                match (existing, self.project_shared()) {
                    (Some(id), Some(p)) => self.open_marker(id, &p),
                    _ => self.send("markers.add", json!({ "at": at })),
                }
            }
        }
        if pressed(Key::D)
            && m.alt
            && !m.command
            && !m.shift
            && let Some(track) = self.project().and_then(|p| {
                let id = self.snap.selection.clips.first()?;
                p.find_clip(id).map(|(t, _)| p.tracks[t].id.clone())
            })
        {
            self.toggle_duck(&track);
        }
        if pressed(Key::D) && !m.command && !m.alt && !self.snap.selection.clips.is_empty() {
            let enabled = self
                .project()
                .and_then(|p| p.clip(&self.snap.selection.clips[0]))
                .is_none_or(|c| c.enabled);
            self.send(
                "clip.set",
                json!({ "clips": self.snap.selection.clips, "enabled": !enabled }),
            );
        }
        if pressed(Key::V) && m.alt && !m.shift && !m.command {
            let clips = self.snap.selection.clips.clone();
            let current = self
                .project()
                .and_then(|p| clips.iter().find_map(|id| super::isolation::heard(p, id)));
            if let Some(current) = current {
                let amount = match current {
                    Some(_) => 0.0,
                    None => super::isolation::FIRST_AMOUNT,
                };
                self.isolate(super::isolation::Isolate::Clips(clips), amount);
            }
        }
        if pressed(Key::A) && m.command && self.pool_focus {
            let media: Vec<String> = match self.project() {
                Some(p) if !m.shift => super::pool::shown_media(p, &self.snap.pool, &self.search)
                    .iter()
                    .map(|m| m.id.clone())
                    .collect(),
                _ => Vec::new(),
            };
            self.send("selection.set", json!({ "media": media }));
        } else if pressed(Key::A) && m.command {
            let (clips, cues): (Vec<String>, Vec<String>) = match self.project() {
                Some(p) if !m.shift => (
                    p.tracks
                        .iter()
                        .flat_map(|t| &t.clips)
                        .map(|c| c.id.clone())
                        .collect(),
                    p.subtitles
                        .iter()
                        .flat_map(|t| &t.cues)
                        .map(|c| c.id.clone())
                        .collect(),
                ),
                _ => (Vec::new(), Vec::new()),
            };
            self.send("selection.set", json!({ "clips": clips, "cues": cues }));
        }
        if pressed(Key::V) && m.alt && m.shift {
            let at = self.snap.timeline.position;
            let clips: Vec<String> = self
                .project()
                .map(|p| {
                    p.tracks
                        .iter()
                        .filter_map(|t| t.clip_at(at).map(|i| t.clips[i].id.clone()))
                        .collect()
                })
                .unwrap_or_default();
            self.send("selection.set", json!({ "clips": clips }));
        }

        let typed: Option<char> = input.events.iter().find_map(|e| match e {
            egui::Event::Text(t) if t == "=" || t == "+" || t == "-" => t.chars().next(),
            _ => None,
        });
        if let Some(c) = typed {
            self.timecode = Some(TimecodeEntry {
                sign: match c {
                    '+' => Some(1),
                    '-' => Some(-1),
                    _ => None,
                },
                text: String::new(),
                focus: true,
            });
        }
    }

    fn track_edit_points(&mut self, track: &str) -> Vec<i64> {
        self.shown_project()
            .and_then(|p| {
                p.track(track).map(|t| {
                    let mut v = t.edit_points();
                    v.push(t.end());
                    v.dedup();
                    v
                })
            })
            .unwrap_or_default()
    }

    fn edit_points(&mut self, id: PlayerId) -> Vec<i64> {
        match id {
            PlayerId::Timeline => self.track_edit_points("V1"),
            PlayerId::Source => match self.snap.viewer {
                ViewerMode::SourceTape => {
                    let mut v: Vec<i64> = self.snap.tape.iter().map(|e| e.start).collect();
                    if let Some((a, b)) = self.crossing_span() {
                        v.extend([a, b]);
                    }
                    v.sort_unstable();
                    v.dedup();
                    v
                }
                _ => {
                    let mut v = vec![0, self.snap.source.frames];
                    if let Some((m, _)) = self.snap.source_at()
                        && let Some(media) = self.project().and_then(|p| p.media(m))
                    {
                        v.extend(media.marks.mark_in);
                        v.extend(media.marks.mark_out);
                    }
                    v.sort_unstable();
                    v
                }
            },
        }
    }

    fn marker_points(&self) -> Vec<i64> {
        self.project()
            .map(|p| p.markers.iter().map(|k| k.at).collect())
            .unwrap_or_default()
    }

    pub(super) fn mark(&self, is_in: bool) {
        if self.hover.is_none() && self.snap.viewer == ViewerMode::SourceTape {
            let order = self.tape_order();
            if order.is_empty() {
                return;
            }
            let frame = self.snap.source.position;
            let value = if is_in { frame } else { frame + 1 };
            let key = if is_in { "in" } else { "out" };
            self.send("tape.marks.set", json!({ key: value, "order": order }));
            return;
        }
        let (media, frame) = match (&self.hover, self.snap.source_at()) {
            (Some((m, f)), _) => (Some(m.clone()), *f),
            (None, Some((m, f))) => (Some(m.to_string()), f),
            (None, None) => (None, self.snap.timeline.position),
        };
        let key = if is_in { "in" } else { "out" };
        let value = if is_in { frame } else { frame + 1 };
        match media {
            Some(m) => {
                let frames = self.media_frames(&m);
                self.send(
                    "media.marks.set",
                    json!({ "media": m, key: value.clamp(0, frames) }),
                );
            }
            None => self.send("marks.set", json!({ key: value })),
        }
    }

    fn go_to_mark(&self, is_in: bool) {
        if self.snap.viewer == ViewerMode::SourceTape
            && let Some((a, b)) = self.crossing_span()
        {
            self.seek(PlayerId::Source, if is_in { a } else { b - 1 }, false);
            return;
        }
        match self.snap.source_at() {
            Some((media, _)) => {
                let Some(m) = self.project().and_then(|p| p.media(media)) else {
                    return;
                };
                let base = match self.snap.viewer {
                    ViewerMode::SourceTape => self
                        .snap
                        .tape
                        .iter()
                        .find(|e| e.media == media)
                        .map_or(0, |e| e.start),
                    _ => 0,
                };
                let mark = if is_in {
                    m.marks.mark_in
                } else {
                    m.marks.mark_out.map(|o| o - 1)
                };
                if let Some(f) = mark {
                    self.seek(PlayerId::Source, base + f, false);
                }
            }
            None => {
                let marks = self.project().map(|p| p.marks.clone()).unwrap_or_default();
                let mark = if is_in {
                    marks.mark_in
                } else {
                    marks.mark_out.map(|o| o - 1)
                };
                if let Some(f) = mark {
                    self.seek(PlayerId::Timeline, f, false);
                }
            }
        }
    }

    pub(super) fn clear_marks(&self) {
        if self.snap.viewer == ViewerMode::SourceTape {
            let media = self.snap.source_at().map(|(m, _)| m.to_string());
            self.send("tape.marks.clear", json!({ "media": media }));
            return;
        }
        match self.snap.source_at() {
            Some((media, _)) => self.send("media.marks.clear", json!({ "media": media })),
            None => self.send("marks.clear", json!({})),
        }
    }

    pub(super) fn delete_key(&self, lift: bool) {
        if self.delete_transition() {
            return;
        }
        let Some(p) = self.project() else { return };
        if let (Some(a), Some(b)) = (p.marks.mark_in, p.marks.mark_out)
            && a < p.end()
        {
            self.send(
                "edit.delete_range",
                json!({ "range": format!("{a}f..{b}f"), "lift": lift }),
            );
        } else {
            self.delete_selected(lift);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.snap = self.live.snapshot();
        if let Some(t) = &mut self.tiles {
            t.proxies = self.snap.proxy_dir.clone();
        }
        if !self
            .theme
            .as_ref()
            .is_some_and(|t| Arc::ptr_eq(t, &self.snap.theme))
        {
            theme::apply(&ctx, &self.snap.theme);
            self.theme = Some(self.snap.theme.clone());
        }
        let tracks = self.project().map_or(0, |p| p.tracks.len());
        if tracks < self.timeline.tracks {
            self.timeline.edit = None;
            self.timeline.dynamic = None;
        }
        self.timeline.tracks = tracks;
        let rev = self.project().map_or(0, |p| p.rev);
        if rev != self.timeline.rev {
            self.timeline.rev = rev;
            if let (Some(e), Some(p)) = (self.timeline.edit, self.shown_project())
                && !crate::gui::timeline::edit_exists(&p, &e)
            {
                self.timeline.edit = None;
                self.timeline.dynamic = None;
            }
        }
        let title = match &self.snap.project {
            Some(p) => format!(
                "{} — vidcut",
                p.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            None => "vidcut".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.sync_textures(&ctx);
        if let Some(tiles) = &mut self.tiles {
            tiles.begin_frame(&ctx, self.snap.project());
        }
        self.keys(&ctx);
        self.export_windows(&ctx);
        self.preferences_window(&ctx);
        self.help_window(&ctx);
        self.history_frame(&ctx);
        self.project_windows(&ctx);
        self.import_window(&ctx);
        self.relink_window(&ctx);
        self.remove_media_window(&ctx);
        self.speech_window(&ctx);
        self.isolation_prompt(&ctx);
        self.ducking_windows(&ctx);
        self.transcription_window(&ctx);
        self.clip_info_window(&ctx);
        let panel = theme::colors().panel;
        egui::Panel::top("top").frame(theme::bar()).show(ui, |ui| {
            theme::decorated(ui, Deco::Bar, |ui| self.top_bar(ui))
        });
        egui::Panel::bottom("status")
            .frame(theme::bar())
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Status, |ui| self.status_bar(ui))
            });
        if self.show_log {
            egui::Panel::right("log")
                .resizable(true)
                .default_size(380.0)
                .frame(theme::card(panel))
                .show(ui, |ui| {
                    theme::decorated(ui, Deco::Card, |ui| self.log_panel(ui))
                });
        }
        if self.snap.project.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.start_screen(ui));
            return;
        }
        let frame = theme::card(panel);
        let spacing = ctx.global_style().spacing.clone();
        let wanted = self.timeline_wanted().unwrap_or(300.0)
            + frame.total_margin().sum().y
            + spacing.interact_size.y
            + spacing.item_spacing.y;
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(wanted.min(ctx.content_rect().height() * 0.6).max(140.0))
            .min_size(140.0)
            .frame(frame)
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| self.timeline_panel(ui))
            });
        if self.show_mixer {
            egui::Panel::right("mixer")
                .resizable(true)
                .default_size(320.0)
                .frame(theme::card(panel))
                .show(ui, |ui| {
                    theme::decorated(ui, Deco::Card, |ui| self.mixer_panel(ui))
                });
        }
        let pool = egui::Panel::left("pool")
            .resizable(true)
            .default_size(430.0)
            .min_size(260.0)
            .frame(theme::card(panel))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| {
                    self.left_tabs(ui);
                    if self.left != LeftTab::Titles {
                        self.end_title_hover();
                    }
                    if self.left != LeftTab::Inspector {
                        self.leave_inspector();
                    }
                    match self.left {
                        LeftTab::Pool => self.pool_panel(ui),
                        LeftTab::Titles => {
                            self.pool_focus = false;
                            self.titles_panel(ui);
                        }
                        LeftTab::Inspector => {
                            self.pool_focus = false;
                            self.inspector_panel(ui);
                        }
                    }
                })
            })
            .response
            .rect;
        if self.history.open {
            egui::Panel::left("history")
                .resizable(true)
                .default_size(self.history.width)
                .min_size(super::history::MIN_WIDTH)
                .frame(theme::card(panel))
                .show(ui, |ui| {
                    theme::decorated(ui, Deco::Card, |ui| self.history_panel(ui))
                });
        }
        egui::CentralPanel::default()
            .frame(theme::card(theme::colors().panel_dark))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Viewer, |ui| self.viewer_panel(ui))
            });
        self.settle_picture_wheel(&ctx);
        if let Some(tiles) = &mut self.tiles {
            tiles.end_frame();
        }
        self.media_drag_overlay(&ctx);
        self.drop_hint(&ctx, pool);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Notices,
    Format,
    Edited,
    Decoding,
    Pid,
    Version,
}

const NOTICE_FADE: f64 = 1.0;

fn recent_projects() -> Vec<PathBuf> {
    std::fs::read_to_string(crate::paths::state_dir().join("recent.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<PathBuf>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.exists())
        .collect()
}

pub(super) fn vidcut_command(
    project: Option<&std::path::Path>,
    method: &str,
    params: &Value,
) -> String {
    let params = shell_quote(&params.to_string());
    match project {
        Some(p) => format!(
            "vidcut -p {} rpc {method} {params}",
            shell_quote(&p.display().to_string())
        ),
        None => format!("vidcut rpc {method} {params}"),
    }
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub(super) fn fit(avail: egui::Rect, aspect: f32) -> egui::Rect {
    let (w, h) = (avail.width(), avail.height());
    let size = if w / h.max(1.0) > aspect {
        egui::vec2(h * aspect, h)
    } else {
        egui::vec2(w, w / aspect.max(0.01))
    };
    egui::Rect::from_center_size(avail.center(), size)
}

pub(super) fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(theme::colors().text_dim)
}

pub(super) struct ViewState {
    folders: super::folders::Folders,
    options: super::timeline::Options,
    recent: Vec<PathBuf>,
}

pub(super) fn escape_cancels<T>(ctx: &egui::Context, modal: &egui::ModalResponse<T>) -> bool {
    modal.is_top_modal
        && !modal.any_popup_open
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
}

#[cfg(test)]
mod tests {
    use super::{decoding, vidcut_command};

    #[test]
    fn a_command_for_a_project_with_a_space_quotes_both() {
        assert_eq!(
            vidcut_command(
                Some(std::path::Path::new("my talk.vidcut")),
                "proxy.generate",
                &serde_json::json!({ "media": "m1" })
            ),
            r#"vidcut -p 'my talk.vidcut' rpc proxy.generate '{"media":"m1"}'"#
        );
    }

    #[test]
    fn the_status_bar_says_where_the_viewer_decodes() {
        assert_eq!(decoding(""), None);
        assert_eq!(decoding("no").as_deref(), Some("decoding on the CPU"));
        assert_eq!(
            decoding("nvdec").as_deref(),
            Some("decoding on the GPU (NVDEC)")
        );
        assert_eq!(
            decoding("d3d11va").as_deref(),
            Some("decoding on the GPU (Direct3D 11)")
        );
    }
}
