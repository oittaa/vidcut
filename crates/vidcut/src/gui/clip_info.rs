use eframe::egui::{self, RichText};
use serde_json::json;
use vidcut_core::model::{
    ColorSpace, Hdr, Media, MediaDetails, Project, StartTimecode, StreamDetails, VideoInfo,
};
use vidcut_core::time::format_timecode;

use super::export::Ask;
use super::workspace::{Workspace, dim};

#[derive(Default)]
pub struct ClipInfo {
    pub open: bool,
    media: Option<String>,
    asked: Option<(String, Ask)>,
    read: Option<(String, Result<MediaDetails, String>)>,
}

type Section = (String, Vec<(String, String)>);

fn bytes(n: u64) -> String {
    match n {
        b if b >= 1_000_000_000 => format!("{:.2} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b => format!("{:.0} kB", b as f64 / 1e3),
    }
}

fn bit_rate(bits: u64) -> String {
    match bits {
        b if b >= 1_000_000 => format!("{:.1} Mb/s", b as f64 / 1e6),
        b => format!("{:.0} kb/s", b as f64 / 1e3),
    }
}

pub(super) fn start_label(tc: &StartTimecode) -> String {
    if tc.rate.as_f64() > 1000.0 {
        let secs = tc.count as f64 / tc.rate.as_f64();
        let whole = secs as i64;
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            whole / 3600,
            whole / 60 % 60,
            whole % 60,
            ((secs - whole as f64) * 1000.0) as i64
        )
    } else {
        format_timecode(tc.count, tc.rate, 0, tc.drop_frame)
    }
}

fn depth(v: &VideoInfo) -> String {
    let bits = vidcut_av::probe::pixel_depth(&v.pix_fmt);
    match vidcut_av::probe::pixel_chroma(&v.pix_fmt) {
        Some(c) => format!("{bits}-bit {c}"),
        None => format!("{bits}-bit"),
    }
}

fn dynamic_range(v: &VideoInfo) -> String {
    let mut s = match v.hdr {
        Some(Hdr::Pq) => "HDR (PQ)".to_string(),
        Some(Hdr::Hlg) => "HDR (HLG)".to_string(),
        None => "SDR".to_string(),
    };
    if let Some(d) = v.dolby_vision {
        s.push_str(&format!(", Dolby Vision {}.{}", d.profile, d.compatibility));
    }
    if v.hdr10plus {
        s.push_str(", HDR10+");
    }
    s
}

fn audio_line(a: &vidcut_core::model::AudioInfo) -> String {
    let channels = match a.channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n} channels"),
    };
    format!(
        "{} {} kHz {channels}",
        a.codec.to_uppercase(),
        f64::from(a.sample_rate) / 1000.0
    )
}

pub(super) fn glance(app: &Workspace, p: &Project, m: &Media) -> Vec<String> {
    if let Some(s) = &m.subtitles {
        return vec![
            format!("{} subtitles", super::subtitles::format_name(s.format)),
            format!("{} cues", s.cues),
            s.language.clone().unwrap_or_else(|| "no language".into()),
            s.encoding.clone(),
        ];
    }
    let mut out = Vec::new();
    if let Some(v) = &m.info.video {
        let (w, h) = v.display_size();
        out.push(format!("{w}×{h}"));
        if !v.still {
            out.push(match v.vfr {
                true => format!("{} fps (variable)", v.fps.decimal()),
                false => format!("{} fps", v.fps.decimal()),
            });
        }
        out.push(v.codec.to_uppercase());
        out.push(depth(v));
        out.push(dynamic_range(v));
        if v.fields.is_some() {
            out.push("interlaced".into());
        }
    }
    if m.info.video.as_ref().is_none_or(|v| !v.still) {
        out.push(app.src_tc(p.media_frames(m)));
    }
    if let Some(tc) = &m.info.timecode {
        out.push(format!("TC {}", start_label(tc)));
    }
    match m.info.audio.as_slice() {
        [] => {}
        [one] => out.push(audio_line(one)),
        [first, rest @ ..] => out.push(format!("{} (+{})", audio_line(first), rest.len())),
    }
    out.push(bytes(m.size));
    out
}

fn stream_rows(s: &StreamDetails) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let codec = match (&s.codec_name, &s.profile) {
        (n, Some(p)) if !n.is_empty() => format!("{n}, {p}"),
        (n, None) if !n.is_empty() => n.clone(),
        (_, Some(p)) => format!("{}, {p}", s.codec),
        _ => s.codec.clone(),
    };
    rows.push(("Codec".into(), codec));
    if let Some(f) = &s.pix_fmt {
        let mut text = f.clone();
        if let Some(b) = s.bits {
            text.push_str(&format!(", {b}-bit"));
        }
        if let Some(c) = &s.chroma {
            text.push_str(&format!(" {c}"));
        }
        rows.push(("Pixels".into(), text));
    }
    let color: Vec<String> = [&s.primaries, &s.transfer, &s.matrix, &s.range]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    if !color.is_empty() {
        rows.push(("Color tags".into(), color.join(" / ")));
    }
    if let Some(r) = s.sample_rate {
        rows.push(("Sample rate".into(), format!("{r} Hz")));
    }
    match (&s.layout, s.channels) {
        (Some(l), Some(n)) => rows.push(("Channels".into(), format!("{n} ({l})"))),
        (None, Some(n)) => rows.push(("Channels".into(), n.to_string())),
        _ => {}
    }
    if let Some(f) = &s.sample_format {
        rows.push(("Samples".into(), f.clone()));
    }
    if let Some(b) = s.bit_rate {
        rows.push(("Bit rate".into(), bit_rate(b)));
    }
    for (k, v) in &s.tags {
        rows.push((k.clone(), v.clone()));
    }
    rows
}

fn sections(app: &Workspace, p: &Project, m: &Media, read: Option<&MediaDetails>) -> Vec<Section> {
    let mut file = vec![
        ("Name".to_string(), m.name.clone()),
        ("Path".into(), m.path.display().to_string()),
        (
            "Size".into(),
            format!("{} ({} bytes)", bytes(m.size), m.size),
        ),
    ];
    if let Some(t) = u64::try_from(m.mtime_ns / 1_000_000)
        .ok()
        .filter(|&ms| ms > 0)
        .and_then(crate::when::local_time)
    {
        file.push(("Modified".into(), t.minutes()));
    }
    if !m.bin.is_empty() {
        file.push(("Bin".into(), m.bin.clone()));
    }
    if let Some(c) = &m.color {
        file.push(("Clip Color".into(), c.clone()));
    }
    let mut out: Vec<Section> = vec![("File".into(), file)];
    let mut format = vec![(
        "Container".to_string(),
        read.map_or_else(|| m.info.format.clone(), |d| d.format.clone()),
    )];
    if m.info.video.as_ref().is_none_or(|v| !v.still) && m.subtitles.is_none() {
        format.push((
            "Duration".into(),
            format!(
                "{} ({:.3} s)",
                app.src_tc(p.media_frames(m)),
                m.info.duration
            ),
        ));
    }
    if let Some(tc) = &m.info.timecode {
        format.push(("Start timecode".into(), start_label(tc)));
    }
    if let Some(b) = read.and_then(|d| d.bit_rate) {
        format.push(("Bit rate".into(), bit_rate(b)));
    }
    for (k, v) in read.map(|d| &d.tags).into_iter().flatten() {
        format.push((k.clone(), v.clone()));
    }
    out.push(("Format".into(), format));
    if let Some(v) = &m.info.video {
        let (w, h) = v.display_size();
        let mut rows = vec![("Size".to_string(), format!("{}×{}", v.width, v.height))];
        if (w, h) != (v.width, v.height) {
            rows.push(("Shown as".into(), format!("{w}×{h}")));
        }
        if !v.still {
            rows.push((
                "Frame rate".into(),
                format!(
                    "{} fps ({}){}",
                    v.fps.decimal(),
                    v.fps,
                    if v.vfr { ", variable" } else { "" }
                ),
            ));
        }
        rows.push((
            "Pixel format".into(),
            format!("{}, {}", v.pix_fmt, depth(v)),
        ));
        rows.push(("Dynamic range".into(), dynamic_range(v)));
        rows.push((
            "Scan".into(),
            match v.fields {
                Some(vidcut_core::model::FieldOrder::Top) => "interlaced, top field first",
                Some(vidcut_core::model::FieldOrder::Bottom) => "interlaced, bottom field first",
                None => "progressive",
            }
            .into(),
        ));
        if v.rotation != 0 {
            rows.push(("Rotation".into(), format!("{}°", v.rotation)));
        }
        if let Some(d) = &v.mastering {
            rows.push((
                "Mastering display".into(),
                format!("{:.4}–{:.0} cd/m²", d.min_nits, d.max_nits),
            ));
        }
        if let Some(l) = &v.light {
            rows.push((
                "Light levels".into(),
                format!("MaxCLL {}, MaxFALL {} cd/m²", l.max_cll, l.max_fall),
            ));
        }
        if m.color_space != ColorSpace::Tagged {
            rows.push((
                "Input Color Space".into(),
                format!("{:?}, in place of its tags", m.color_space),
            ));
        }
        out.push(("Video".into(), rows));
    }
    for s in read.map(|d| d.streams.as_slice()).unwrap_or_default() {
        let title = format!("Stream {} ({})", s.index, s.kind);
        out.push((title, stream_rows(s)));
    }
    if read.is_none() {
        for (i, a) in m.info.audio.iter().enumerate() {
            out.push((
                format!("Audio {}", i + 1),
                vec![("Sound".into(), audio_line(a))],
            ));
        }
    }
    let mut vidcut = Vec::new();
    if let Some(px) = &m.proxy {
        vidcut.push(("Proxy".to_string(), format!("{}×{}", px.width, px.height)));
    }
    if app.snap.transcribed.contains(&m.id) {
        vidcut.push(("Transcript".into(), "its words are kept with it".into()));
    }
    if let Some(f) = &m.fingerprint {
        vidcut.push(("Fingerprint".into(), f.clone()));
    }
    if !vidcut.is_empty() {
        out.push(("In vidcut".into(), vidcut));
    }
    out
}

fn as_text(sections: &[Section]) -> String {
    let mut s = String::new();
    for (title, rows) in sections {
        s.push_str(title);
        s.push('\n');
        for (k, v) in rows {
            s.push_str(&format!("  {k}: {v}\n"));
        }
    }
    s
}

impl Workspace {
    pub(crate) fn open_clip_info(&mut self, media: Option<String>) {
        self.clip_info.media = media;
        self.clip_info.open = true;
    }

    pub(super) fn clip_info_window(&mut self, ctx: &egui::Context) {
        if !self.clip_info.open {
            return;
        }
        let Some(p) = self.project_shared() else {
            return;
        };
        let id = self
            .clip_info
            .media
            .clone()
            .or_else(|| self.snap.selection.media.last().cloned());
        let m = id.as_deref().and_then(|id| p.media(id)).cloned();
        if let Some(m) = &m {
            let fresh = self
                .clip_info
                .read
                .as_ref()
                .is_some_and(|(r, _)| *r == m.id);
            let asking = self
                .clip_info
                .asked
                .as_ref()
                .is_some_and(|(a, _)| *a == m.id);
            if !fresh && !asking && m.subtitles.is_none() {
                let ask = Ask::send(&self.live, ctx, "media.details", json!({ "media": m.id }));
                self.clip_info.asked = Some((m.id.clone(), ask));
            }
        }
        if let Some((id, ask)) = &self.clip_info.asked
            && let Some(answer) = ask.take()
        {
            let read = answer
                .map_err(|e| e.message)
                .and_then(|r| serde_json::from_value(r.result).map_err(|e| e.to_string()));
            self.clip_info.read = Some((id.clone(), read));
            self.clip_info.asked = None;
        }
        let mut open = true;
        egui::Window::new("Clip Info")
            .collapsible(false)
            .resizable(true)
            .default_pos(super::export::near_center(ctx, 520.0, 620.0))
            .default_size([520.0, 620.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let Some(m) = &m else {
                    ui.label(dim("Select a clip in the Media Pool"));
                    return;
                };
                let read = match &self.clip_info.read {
                    Some((id, r)) if *id == m.id => Some(r),
                    _ => None,
                };
                let details = read.and_then(|r| r.as_ref().ok());
                let all = sections(self, &p, m, details);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&m.name).strong());
                    if ui
                        .button("Copy")
                        .on_hover_text("Copy all of it as text")
                        .clicked()
                    {
                        ui.ctx().copy_text(as_text(&all));
                    }
                });
                match read {
                    None if m.subtitles.is_none() => {
                        ui.label(dim("Reading the file…"));
                    }
                    Some(Err(e)) => {
                        ui.label(dim(format!("The file couldn't be read again: {e}")));
                    }
                    _ => {}
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("clip-info")
                            .num_columns(2)
                            .spacing([12.0, 3.0])
                            .show(ui, |ui| {
                                for (title, rows) in &all {
                                    ui.label(RichText::new(title).strong());
                                    ui.end_row();
                                    for (k, v) in rows {
                                        ui.label(dim(k));
                                        ui.add(egui::Label::new(v).wrap());
                                        ui.end_row();
                                    }
                                }
                            });
                    });
            });
        if !open {
            self.clip_info.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use vidcut_core::model::StartTimecode;
    use vidcut_core::time::Rational;

    use super::start_label;

    #[test]
    fn a_broadcast_wavs_time_reference_reads_as_a_time_of_day() {
        let tc = StartTimecode {
            count: 48_000 * 3661 + 24_000,
            rate: Rational::new(48_000, 1).unwrap(),
            drop_frame: false,
        };
        assert_eq!(start_label(&tc), "01:01:01.500");
    }

    #[test]
    fn a_recorded_timecode_reads_as_frames() {
        let tc = StartTimecode {
            count: 25 * 3600 + 12,
            rate: Rational::new(25, 1).unwrap(),
            drop_frame: false,
        };
        assert_eq!(start_label(&tc), "01:00:00:12");
    }
}
