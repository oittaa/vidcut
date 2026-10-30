use eframe::egui::{self, Key, RichText};
use serde_json::{Value, json};
use vidcut_core::model::{
    MAX_BORDER, MotionEase, PictureTransition, Project, PushPreset, SlidePreset, Transition,
    WipeDirection,
};
use vidcut_core::ops;
use vidcut_core::time::{Rational, format_timecode, parse_media_timecode};

use super::{EditSel, TrackRef, geometry};
use crate::gui::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct FadeDialog {
    pub track: TrackRef,
    pub at: i64,
    pub text: String,
}

const EDGES: [(WipeDirection, &str); 4] = [
    (WipeDirection::Left, "Left"),
    (WipeDirection::Right, "Right"),
    (WipeDirection::Up, "Top"),
    (WipeDirection::Down, "Bottom"),
];

fn push_label(p: PushPreset) -> &'static str {
    match p {
        PushPreset::Left => "Push Left",
        PushPreset::Right => "Push Right",
        PushPreset::Up => "Push Up",
        PushPreset::Down => "Push Down",
    }
}

fn slide_label(p: SlidePreset) -> &'static str {
    match p {
        SlidePreset::LeftRight => "Left-Right",
        SlidePreset::RightLeft => "Right-Left",
        SlidePreset::BottomUp => "Bottom-Up",
        SlidePreset::TopDown => "Top-Down",
        SlidePreset::TopLeft => "Top-Left",
        SlidePreset::BottomRight => "Bottom-Right",
    }
}

fn ease_label(e: MotionEase) -> &'static str {
    match e {
        MotionEase::None => "None",
        MotionEase::In => "In",
        MotionEase::Out => "Out",
        MotionEase::InOut => "In & Out",
        MotionEase::Custom => "Custom",
    }
}

const ALIGNMENTS: [(&str, &str); 3] = [
    ("start", "Start on Edit"),
    ("center", "Center on Edit"),
    ("end", "End on Edit"),
];

fn same_type(a: &PictureTransition, b: &PictureTransition) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

fn menu<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    now: T,
    choices: &[(T, &str)],
) -> Option<T> {
    let name = choices.iter().find(|(c, _)| *c == now).map(|c| c.1);
    let mut set = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(name.unwrap_or_default())
        .show_ui(ui, |ui| {
            for &(c, label) in choices {
                if ui.selectable_label(c == now, label).clicked() && c != now {
                    set = Some(c);
                }
            }
        });
    set
}

pub fn kind(x: &Transition) -> PictureTransition {
    vidcut_core::otio::transitions::kind_of(x).unwrap_or_default()
}

pub fn realigned(track: TrackRef, at: i64, (x, sound): (&Transition, bool), align: &str) -> Value {
    json!({
        "at": at,
        "track": track.id(),
        "length": format!("{}f", x.before + x.after),
        "align": align,
        "sound": sound,
        "transition": kind(x),
    })
}

pub fn resized(
    track: &str,
    at: i64,
    (before, after): (i64, i64),
    sound: bool,
    transition: &PictureTransition,
) -> Value {
    json!({
        "at": at,
        "track": track,
        "before": format!("{before}f"),
        "after": format!("{after}f"),
        "sound": sound,
        "transition": transition,
    })
}

pub fn alignment(x: &Transition) -> &'static str {
    match (x.before, x.after) {
        (0, _) => "start",
        (_, 0) => "end",
        _ => "center",
    }
}

pub fn changed(before: i64, after: i64, length: i64, room: (i64, i64)) -> (i64, i64) {
    let (b, a) = match (before, after) {
        (0, _) => (0, length),
        (_, 0) => (length, 0),
        _ => {
            let grow = length - before - after;
            let a = (after + grow.div_euclid(2)).max(1).min(length - 1);
            (length - a, a)
        }
    };
    (b.min(room.0), a.min(room.1))
}

pub fn shown(frames: i64, fps: Rational, in_frames: bool) -> String {
    if in_frames {
        frames.to_string()
    } else {
        format_timecode(frames, fps, 0, false)
    }
}

pub fn typed(text: &str, fps: Rational, in_frames: bool) -> Option<i64> {
    let text = text.trim();
    let frames = if in_frames {
        text.parse().ok()?
    } else {
        parse_media_timecode(text, fps)?
    };
    (frames > 0).then_some(frames)
}

fn with_sound(p: &Project, track: TrackRef, at: i64) -> Option<(Transition, bool)> {
    let t = p.track(&track.id())?;
    let x = geometry::picture_transition(p, t, at)?;
    let sound = t
        .transitions
        .iter()
        .any(|y| y.sound && y.from == x.from && y.to == x.to);
    Some((x.clone(), sound))
}

impl Workspace {
    pub(crate) fn selected_transition(&self) -> Option<(EditSel, Transition)> {
        let e = self
            .timeline
            .edit
            .filter(|e| self.timeline.transition == Some((e.track, e.at)))?;
        let x = self.trim_transition(self.project()?, e)?;
        Some((e, x))
    }

    fn realign_transition(&self, track: TrackRef, at: i64, align: &str) {
        let Some((x, sound)) = self.project().and_then(|p| with_sound(p, track, at)) else {
            return;
        };
        self.send("edit.dissolve", realigned(track, at, (&x, sound), align));
    }

    fn resize_transition(&self, track: TrackRef, at: i64, length: i64) {
        let Some(p) = self.project() else {
            return;
        };
        let Some((x, sound)) = with_sound(p, track, at) else {
            return;
        };
        let room = ops::dissolve_room(p, &track.id(), at, false).unwrap_or((x.before, x.after));
        let (before, after) = changed(x.before, x.after, length, room);
        if (before, after) == (x.before, x.after) {
            return;
        }
        self.send(
            "edit.dissolve",
            resized(&track.id(), at, (before, after), sound, &kind(&x)),
        );
    }

    pub(crate) fn delete_transition(&self) -> bool {
        let Some((e, _)) = self.selected_transition() else {
            return false;
        };
        self.send(
            "edit.remove_transition",
            json!({ "at": e.at, "track": e.track.id() }),
        );
        true
    }

    pub(crate) fn open_fade_dialog(&mut self) {
        let Some((e, x)) = self.selected_transition() else {
            return;
        };
        let Some(fps) = self.project().map(|p| p.settings.fps) else {
            return;
        };
        self.timeline.fade_dialog = Some(FadeDialog {
            track: e.track,
            at: e.at,
            text: shown(x.before + x.after, fps, self.timeline.fade_in_frames),
        });
    }

    pub(crate) fn transition_menu(&mut self, ui: &mut egui::Ui) {
        let Some((e, x)) = self.selected_transition() else {
            ui.close();
            return;
        };
        if ui.button("Change Transition Duration…").clicked() {
            self.open_fade_dialog();
            ui.close();
        }
        ui.separator();
        let now = alignment(&x);
        for (align, label) in ALIGNMENTS {
            if ui
                .add_enabled(now != align, egui::Button::new(label))
                .clicked()
            {
                self.realign_transition(e.track, e.at, align);
                ui.close();
            }
        }
    }

    pub(crate) fn settle_transition_drafts(
        &mut self,
        showing: Option<(TrackRef, i64)>,
        ctx: &egui::Context,
    ) {
        if let Some((track, at, f)) = self.timeline.transition_frames
            && showing != Some((track, at))
        {
            self.timeline.transition_frames = None;
            self.resize_transition(track, at, f);
        }
        if let Some((track, at, color, id)) = self.timeline.transition_color
            && showing != Some((track, at))
        {
            self.timeline.transition_color = None;
            egui::Popup::close_id(ctx, id);
            self.recolor_transition(track, at, color);
        }
        if let Some((track, at, k)) = self.timeline.transition_values.take() {
            if showing == Some((track, at)) {
                self.timeline.transition_values = Some((track, at, k));
            } else {
                self.set_transition(track, at, &k);
            }
        }
    }

    fn transition_kind(&self, track: TrackRef, at: i64) -> Option<PictureTransition> {
        let p = self.project()?;
        p.track(&track.id())
            .and_then(|t| geometry::picture_transition(p, t, at))
            .and_then(vidcut_core::otio::transitions::kind_of)
    }

    fn set_transition(&self, track: TrackRef, at: i64, k: &PictureTransition) {
        if self.transition_kind(track, at).as_ref() == Some(k) {
            return;
        }
        self.send(
            "edit.transition",
            json!({ "at": at, "track": track.id(), "transition": k }),
        );
    }

    fn recolor_transition(&self, track: TrackRef, at: i64, color: [u8; 3]) {
        let Some(mut now) = self.transition_kind(track, at) else {
            return;
        };
        match &mut now {
            PictureTransition::DipToColor { color: c } => *c = color,
            k => match k.motion_mut() {
                Some(m) => m.color = color,
                None => return,
            },
        }
        self.set_transition(track, at, &now);
    }

    pub(crate) fn transition_form(&mut self, ui: &mut egui::Ui) -> bool {
        use super::super::dialog;
        use super::super::workspace::dim;
        let selected = self.selected_transition();
        self.settle_transition_drafts(selected.as_ref().map(|(e, _)| (e.track, e.at)), ui.ctx());
        let Some((e, x)) = selected else {
            return false;
        };
        let Some((fps, width)) = self
            .project()
            .map(|p| (p.settings.fps, f64::from(p.settings.width.max(1))))
        else {
            return false;
        };
        let kind = vidcut_core::otio::transitions::kind_of(&x);
        let mut values = match &self.timeline.transition_values {
            Some((track, at, k)) if (*track, *at) == (e.track, e.at) => Some(k.clone()),
            _ => kind.clone().filter(|k| k.motion().is_some()),
        };
        let (mut drafted_values, mut committed) = (false, false);
        let length = x.before + x.after;
        let mut frames = match self.timeline.transition_frames {
            Some((track, at, f)) if (track, at) == (e.track, e.at) => f,
            _ => length,
        };
        let (mut set, mut align, mut dragged, mut commit) = (None, None, false, false);
        let (mut picked, mut picker) = (None, None);
        let drafted = match self.timeline.transition_color {
            Some((track, at, c, _)) if (track, at) == (e.track, e.at) => Some(c),
            _ => None,
        };
        ui.label(RichText::new("Transition").strong());
        ui.label(dim(format!("{}   {}", e.track.id(), self.tc(e.at))));
        dialog::form(ui, "transition-form", |ui| {
            dialog::row(ui, "Transition Type", |ui| {
                let name = vidcut_core::otio::transitions::name(&x);
                egui::ComboBox::from_id_salt("transition-type")
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        let cards = crate::gui::transition_browser::GROUPS
                            .iter()
                            .flat_map(|(_, cards)| cards.iter());
                        for (label, k) in cards {
                            let same = kind.as_ref().is_some_and(|c| same_type(c, k));
                            if ui.selectable_label(same, *label).clicked() && !same {
                                set = Some(k.clone());
                            }
                        }
                    });
            });
            dialog::row(ui, "Duration", |ui| {
                let r = ui.add(
                    egui::DragValue::new(&mut frames)
                        .range(1..=i64::MAX / 4)
                        .suffix(" frames"),
                );
                dragged = r.changed();
                commit = r.drag_stopped() || r.lost_focus();
                ui.label(dim(shown(frames, fps, false)));
            });
            dialog::row(ui, "Alignment", |ui| {
                dialog::switch(ui, |ui| {
                    let now = alignment(&x);
                    for (a, label) in ALIGNMENTS {
                        if ui.selectable_label(now == a, label).clicked() && now != a {
                            align = Some(a);
                        }
                    }
                });
            });
            if let Some(PictureTransition::DipToColor { color }) = &kind {
                dialog::row(ui, "Color", |ui| {
                    let mut rgb = drafted.unwrap_or(*color);
                    picker = Some(ui.auto_id_with("popup"));
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        picked = Some(rgb);
                    }
                });
            }
            if let Some(PictureTransition::Wipe { direction }) = &kind {
                dialog::row(ui, "Edge", |ui| {
                    if let Some(d) = menu(ui, "transition-edge", *direction, &EDGES) {
                        set = Some(PictureTransition::Wipe { direction: d });
                    }
                });
            }
            if let Some(k) = &mut values {
                let color = |ui: &mut egui::Ui, now: [u8; 3]| {
                    let mut rgb = drafted.unwrap_or(now);
                    picker = Some(ui.auto_id_with("popup"));
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        picked = Some(rgb);
                    }
                };
                (drafted_values, committed) = motion_rows(ui, k, width, color);
            }
        });
        if let Some(k) = values.filter(|_| drafted_values || committed) {
            self.timeline.transition_values = Some((e.track, e.at, k));
        }
        if committed && let Some((track, at, k)) = self.timeline.transition_values.take() {
            self.set_transition(track, at, &k);
        }
        if dragged {
            self.timeline.transition_frames = Some((e.track, e.at, frames));
        }
        if commit && let Some((track, at, f)) = self.timeline.transition_frames.take() {
            self.resize_transition(track, at, f);
        }
        if let (Some(c), Some(id)) = (picked, picker) {
            self.timeline.transition_color = Some((e.track, e.at, c, id));
        }
        if let Some((track, at, color, id)) = self.timeline.transition_color
            && !egui::Popup::is_id_open(ui.ctx(), id)
        {
            self.timeline.transition_color = None;
            self.recolor_transition(track, at, color);
        }
        if let Some(k) = set {
            self.set_transition(e.track, e.at, &k);
        }
        if let Some(a) = align {
            self.realign_transition(e.track, e.at, a);
        }
        true
    }

    pub(crate) fn fade_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.fade_dialog.take() else {
            return;
        };
        let Some(fps) = self.project().map(|p| p.settings.fps) else {
            return;
        };
        let mut in_frames = self.timeline.fade_in_frames;
        let mut open = true;
        let mut apply = None;
        let mut done = false;
        use super::super::dialog;
        dialog::window(ctx, "Change Transition Duration", &mut open, |ui| {
            dialog::form(ui, "fade-form", |ui| {
                dialog::row(ui, "Format", |ui| {
                    dialog::switch(ui, |ui| {
                        for (frames, label) in [(false, "Time"), (true, "Frames")] {
                            if ui.selectable_label(in_frames == frames, label).clicked()
                                && in_frames != frames
                            {
                                if let Some(f) = typed(&d.text, fps, in_frames) {
                                    d.text = shown(f, fps, frames);
                                }
                                in_frames = frames;
                            }
                        }
                    });
                });
                dialog::row(ui, "Duration", |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut d.text).desired_width(110.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        apply = typed(&d.text, fps, in_frames);
                        done = apply.is_some();
                    }
                });
                dialog::row(ui, "Or", |ui| {
                    for secs in [1, 5, 15] {
                        let frames = fps.secs_to_frames(f64::from(secs));
                        let label = if in_frames {
                            frames.to_string()
                        } else {
                            format!("{secs}s")
                        };
                        if ui.button(label).clicked() {
                            apply = Some(frames);
                            done = true;
                        }
                    }
                });
            });
            let frames = typed(&d.text, fps, in_frames);
            dialog::footer(ui, |ui| {
                if dialog::main_button(ui, "Change", frames.is_some()) {
                    apply = frames;
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
                if frames.is_none() {
                    dialog::why(ui, "not a duration");
                }
            });
        });
        self.timeline.fade_in_frames = in_frames;
        if let Some(f) = apply {
            self.resize_transition(d.track, d.at, f);
        }
        if open && !done {
            self.timeline.fade_dialog = Some(d);
        }
    }
}

fn motion_rows(
    ui: &mut egui::Ui,
    k: &mut PictureTransition,
    width: f64,
    color: impl FnOnce(&mut egui::Ui, [u8; 3]),
) -> (bool, bool) {
    use super::super::dialog;
    let (mut changed, mut done) = (false, false);
    dialog::row(ui, "Preset", |ui| match k {
        PictureTransition::Push { preset, .. } => {
            let pushes = PushPreset::ALL.map(|p| (p, push_label(p)));
            if let Some(p) = menu(ui, "transition-preset", *preset, &pushes) {
                (*preset, done) = (p, true);
            }
        }
        PictureTransition::Slide { preset, .. } => {
            let slides = SlidePreset::ALL.map(|p| (p, slide_label(p)));
            if let Some(p) = menu(ui, "transition-preset", *preset, &slides) {
                (*preset, done) = (p, true);
            }
        }
        _ => {}
    });
    if let PictureTransition::Slide { out, .. } = k {
        dialog::row(ui, "Direction", |ui| {
            dialog::switch(ui, |ui| {
                for (o, label) in [(false, "In"), (true, "Out")] {
                    if ui.selectable_label(*out == o, label).clicked() && *out != o {
                        (*out, done) = (o, true);
                    }
                }
            });
        });
    }
    let Some(m) = k.motion_mut() else {
        return (changed, done);
    };
    dialog::row(ui, "Border", |ui| {
        let mut shown = m.border * width;
        let r = ui.add(
            egui::DragValue::new(&mut shown)
                .range(0.0..=MAX_BORDER * width)
                .max_decimals(1),
        );
        if r.changed() {
            m.border = shown / width;
            changed = true;
        }
        done |= r.drag_stopped() || r.lost_focus();
    });
    dialog::row(ui, "Feather", |ui| {
        done |= ui.checkbox(&mut m.feather, "").changed();
    });
    dialog::row(ui, "Color", |ui| color(ui, m.color));
    dialog::row(ui, "Motion Blur", |ui| {
        let r = ui.add(
            egui::DragValue::new(&mut m.motion_blur)
                .range(0.0..=1.0)
                .speed(0.01)
                .max_decimals(3),
        );
        changed |= r.changed();
        done |= r.drag_stopped() || r.lost_focus();
    });
    dialog::row(ui, "Ease", |ui| {
        let eases = MotionEase::ALL.map(|e| (e, ease_label(e)));
        if let Some(e) = menu(ui, "transition-ease", m.ease, &eases) {
            m.ease = e;
            m.curve.clear();
            done = true;
        }
    });
    (changed, done)
}

#[cfg(test)]
mod tests {
    use vidcut_core::time::Rational;

    #[test]
    fn realigning_a_wipe_keeps_it_a_wipe() {
        use vidcut_core::model::{Transition, WipeDirection};
        let x = Transition {
            id: "t1".into(),
            sound: false,
            from: Some("c1".into()),
            to: Some("c2".into()),
            before: 10,
            after: 10,
            otio: vidcut_core::otio::transitions::wipe_otio(WipeDirection::Left).into(),
        };
        let track = super::TrackRef::MAIN;
        assert_eq!(
            super::realigned(track, 100, (&x, false), "end")["transition"],
            serde_json::json!({"type": "wipe", "direction": "left"})
        );
    }

    #[test]
    fn a_one_second_dissolve_reads_as_a_timecode_in_time_format() {
        let fps = Rational::new(30, 1).unwrap();
        assert_eq!(super::shown(30, fps, false), "00:00:01:00");
    }

    #[test]
    fn changing_an_uneven_dissolve_to_its_own_length_keeps_its_sides() {
        assert_eq!(super::changed(10, 15, 25, (10, 300)), (10, 15));
    }

    #[test]
    fn a_longer_duration_lengthens_both_sides_evenly_from_their_split() {
        assert_eq!(super::changed(10, 15, 31, (40, 300)), (13, 18));
    }

    #[test]
    fn a_duration_past_both_sides_media_fills_both() {
        assert_eq!(super::changed(10, 15, 999, (10, 300)), (10, 300));
    }

    #[test]
    fn a_shorter_centered_dissolve_keeps_its_odd_frame_before_the_edit() {
        assert_eq!(super::changed(15, 15, 25, (60, 60)), (13, 12));
    }

    #[test]
    fn a_typed_timecode_of_two_seconds_at_25_fps_is_50_frames() {
        let fps = Rational::new(25, 1).unwrap();
        assert_eq!(super::typed("00:00:02:00", fps, false), Some(50));
    }
}
