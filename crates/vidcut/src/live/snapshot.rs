use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use vidcut_core::model::{Clip, TrackKind};
use vidcut_core::render::TapeEntry;
use vidcut_core::request::{TimecodeDisplay, ViewerFrame, ViewerMode};
use vidcut_core::steps::{Span, Steps};
use vidcut_core::time::{Rational, format_timecode, parse_counter, parse_frame_number};
use vidcut_core::{Error, Project};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerId {
    Source,
    Timeline,
}

impl PlayerId {
    pub const ALL: [PlayerId; 2] = [PlayerId::Source, PlayerId::Timeline];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn other(self) -> Self {
        match self {
            Self::Source => Self::Timeline,
            Self::Timeline => Self::Source,
        }
    }

    pub fn of(mode: ViewerMode) -> Self {
        if mode == ViewerMode::Timeline {
            Self::Timeline
        } else {
            Self::Source
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PlayerSnap {
    pub loaded: bool,
    pub muted: bool,
    pub frames: i64,
    pub position: i64,
    pub ready: bool,
    pub playing: bool,
    pub speed: f64,
    pub hwdec: String,
    pub error: Option<String>,
    pub lanes: Vec<String>,
    #[serde(skip)]
    pub glide: Option<vidcut_av::player::Glide>,
}

#[derive(Debug, Clone)]
pub struct ProjectSnap {
    pub path: PathBuf,
    pub project: Arc<Project>,
    pub history: Arc<Steps>,
    pub colors: Option<vidcut_core::model::Hdr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewAt {
    Step(usize),
    Version(u64, usize),
}

#[derive(Debug, Clone)]
pub struct HistoryPreview {
    pub at: PreviewAt,
    pub rev: u64,
    pub project: Arc<Project>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flash {
    pub seq: u64,
    pub rev: u64,
    pub clips: Vec<String>,
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PoolView {
    pub bin: String,
    pub sort: vidcut_core::request::PoolSort,
    pub descending: bool,
    pub view: vidcut_core::request::PoolMode,
    pub search: String,
}

impl PoolView {
    pub fn media(&self, p: &Project) -> Vec<String> {
        let inside = |b: &str| {
            self.bin.is_empty() || b == self.bin || b.starts_with(&format!("{}/", self.bin))
        };
        let mut media: Vec<&vidcut_core::model::Media> =
            p.media.iter().filter(|m| inside(&m.bin)).collect();
        use vidcut_core::request::PoolSort;
        match self.sort {
            PoolSort::Added => {}
            PoolSort::Name => media.sort_by_key(|m| m.name.to_lowercase()),
            PoolSort::Duration => media.sort_by_key(|m| p.media_frames(m)),
            PoolSort::Date => media.sort_by_key(|m| m.mtime_ns),
            PoolSort::Timecode => {
                let start = |m: &vidcut_core::model::Media| {
                    m.info.timecode.map_or(0.0, |t| t.label_seconds())
                };
                media.sort_by(|a, b| start(a).total_cmp(&start(b)));
            }
        }
        if self.descending {
            media.reverse();
        }
        media.into_iter().map(|m| m.id.clone()).collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Selection {
    pub clips: Vec<String>,
    pub cues: Vec<String>,
    pub media: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub seq: u64,
    pub time: f64,
    pub origin: String,
    pub method: String,
    pub params: Value,
    pub summary: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub rev: u64,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub seq: u64,
    pub text: String,
    pub error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Filmstrip {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Vec<u8>>,
    pub times: Vec<f64>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub seq: u64,
    pub pid: u32,
    pub socket: Option<PathBuf>,
    pub project: Option<ProjectSnap>,
    pub viewer: ViewerMode,
    pub viewer_frame: ViewerFrame,
    pub last_source: ViewerMode,
    pub source: PlayerSnap,
    pub timeline: PlayerSnap,
    pub source_media: Option<String>,
    pub tape: Arc<Vec<TapeEntry>>,
    pub selection: Selection,
    pub pool: PoolView,
    pub clipboard: usize,
    pub key_clipboard: usize,
    pub notices: Arc<VecDeque<Notice>>,
    pub thumbs: Arc<HashMap<String, Arc<Filmstrip>>>,
    pub missing: Arc<HashSet<String>>,
    pub transcribed: Arc<HashSet<String>>,
    pub fillers: Arc<std::collections::BTreeMap<String, Vec<String>>>,
    pub lost_proxies: Arc<HashMap<String, PathBuf>>,
    pub render_cache: Arc<Vec<vidcut_core::render_cache::Part>>,
    pub proxy_dir: Arc<Path>,
    pub waveforms: Arc<HashMap<String, Arc<vidcut_av::waveform::Waveform>>>,
    pub jobs: Arc<Vec<super::JobInfo>>,
    pub audio_scrubbing: bool,
    pub looping: bool,
    pub fast_review: bool,
    pub video_only: bool,
    pub audio_only: bool,
    pub edit_only_rev: u64,
    pub stats: bool,
    pub timecode_display: TimecodeDisplay,
    pub timer1_origin: Timer1Origin,
    pub guides: vidcut_core::preferences::Guides,
    pub show_super_scale: bool,
    pub preview_error: Option<String>,
    pub theme: Arc<crate::themes::Theme>,
    pub history_preview: Option<Arc<HistoryPreview>>,
    pub flash: Option<Arc<Flash>>,
}

impl Snapshot {
    pub fn player(&self, id: PlayerId) -> &PlayerSnap {
        match id {
            PlayerId::Source => &self.source,
            PlayerId::Timeline => &self.timeline,
        }
    }

    pub fn active(&self) -> &PlayerSnap {
        self.player(PlayerId::of(self.viewer))
    }

    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref().map(|p| &*p.project)
    }

    pub fn source_at(&self) -> Option<(&str, i64)> {
        match self.viewer {
            ViewerMode::SourceClip => self
                .source_media
                .as_deref()
                .map(|m| (m, self.source.position)),
            ViewerMode::SourceTape => tape_at(&self.tape, self.source.position)
                .map(|e| (e.media.as_str(), self.source.position - e.start)),
            ViewerMode::Timeline => None,
        }
    }

    pub fn timecode_view(&self) -> TimecodeView<'_> {
        let playhead = self.active().position;
        timecode_view(
            self.project(),
            self.viewer,
            playhead,
            &self.tape,
            self.timecode_display,
            self.timer1_origin.zero(PlayerId::of(self.viewer)),
            self.source_media.as_deref(),
        )
    }

    pub fn timecode_text(&self) -> String {
        format_timecode_field(&self.timecode_view()).unwrap_or_else(|| "-".into())
    }
}

pub fn tape_at(tape: &[TapeEntry], position: i64) -> Option<&TapeEntry> {
    tape.iter()
        .find(|e| position < e.start + e.frames)
        .or(tape.last())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Timer1Origin {
    pub source: Option<i64>,
    pub timeline: Option<i64>,
}

impl Timer1Origin {
    pub fn zero(self, player: PlayerId) -> i64 {
        match player {
            PlayerId::Source => self.source.unwrap_or(0),
            PlayerId::Timeline => self.timeline.unwrap_or(0),
        }
    }

    pub fn reset(&mut self, player: PlayerId, frame: i64) {
        match player {
            PlayerId::Source => self.source = Some(frame),
            PlayerId::Timeline => self.timeline = Some(frame),
        }
    }
}

pub fn source_frame_at(
    project: Option<&Project>,
    viewer: ViewerMode,
    playhead: i64,
    tape: &[TapeEntry],
) -> Option<i64> {
    match viewer {
        ViewerMode::SourceClip => Some(playhead),
        ViewerMode::SourceTape => tape_at(tape, playhead).map(|e| playhead - e.start),
        ViewerMode::Timeline => shown_clip(project?, playhead).map(|c| c.source_at(playhead)),
    }
}

fn shown_clip(project: &Project, playhead: i64) -> Option<&Clip> {
    let tracks = project.tracks_of(TrackKind::Video);
    for ti in tracks.into_iter().rev() {
        let track = &project.tracks[ti];
        if !track.video_enabled {
            continue;
        }
        let Some(ci) = track.clip_at(playhead) else {
            continue;
        };
        let clip = &track.clips[ci];
        if clip.enabled && clip.video && !clip.is_block() {
            return Some(clip);
        }
    }
    None
}

pub struct TimecodeView<'a> {
    pub display: TimecodeDisplay,
    pub viewer: ViewerMode,
    pub playhead: i64,
    pub fps: Rational,
    pub drop: bool,
    pub start_tc: i64,
    pub source_tc: i64,
    pub timer_zero: i64,
    pub source_frame: Option<i64>,
    pub tape_start: i64,
    pub tape_frames: Option<i64>,
    pub clip: Option<&'a Clip>,
}

pub fn timecode_view<'a>(
    project: Option<&'a Project>,
    viewer: ViewerMode,
    playhead: i64,
    tape: &'a [TapeEntry],
    display: TimecodeDisplay,
    timer_zero: i64,
    source_media: Option<&str>,
) -> TimecodeView<'a> {
    let entry = if viewer == ViewerMode::SourceTape {
        tape_at(tape, playhead)
    } else {
        None
    };
    let (fps, drop, start_tc) = match project {
        Some(p) => (
            p.settings.fps,
            p.settings.counts_drop_frame(),
            p.settings.start_tc,
        ),
        None => (Rational::new(30, 1).expect("valid"), false, 0),
    };
    let clip = match viewer {
        ViewerMode::Timeline => project.and_then(|p| shown_clip(p, playhead)),
        ViewerMode::SourceClip | ViewerMode::SourceTape => None,
    };
    let media = match viewer {
        ViewerMode::SourceClip => source_media,
        ViewerMode::SourceTape => entry.map(|e| e.media.as_str()),
        ViewerMode::Timeline => clip.map(|c| c.media.as_str()),
    };
    let source_tc = project
        .zip(media)
        .and_then(|(p, m)| p.media(m))
        .and_then(|m| m.info.timecode)
        .map_or(0, |tc| tc.frames_at(fps).round() as i64);
    TimecodeView {
        display,
        viewer,
        playhead,
        fps,
        drop,
        start_tc,
        source_tc,
        timer_zero,
        source_frame: source_frame_at(project, viewer, playhead, tape),
        tape_start: entry.map_or(0, |e| e.start),
        tape_frames: entry.map(|e| e.frames),
        clip,
    }
}

pub fn format_timecode_field(view: &TimecodeView<'_>) -> Option<String> {
    let source = counts_source(view.display, view.viewer);
    if source && view.source_frame.is_none() {
        return None;
    }
    let n = view.source_frame.unwrap_or(0);
    Some(match view.display {
        TimecodeDisplay::Auto if view.viewer.is_source() => {
            format_timecode(n, view.fps, view.source_tc, view.drop)
        }
        TimecodeDisplay::Auto | TimecodeDisplay::RecordTimecode => {
            format_timecode(view.playhead, view.fps, view.start_tc, view.drop)
        }
        TimecodeDisplay::SourceTimecode => format_timecode(n, view.fps, view.source_tc, view.drop),
        TimecodeDisplay::SourceFrame => n.to_string(),
        TimecodeDisplay::RecordFrame => view.playhead.to_string(),
        TimecodeDisplay::Timer2 => format_timecode(view.playhead, view.fps, 0, view.drop),
        TimecodeDisplay::Timer1 => {
            format_timecode(view.playhead - view.timer_zero, view.fps, 0, view.drop)
        }
    })
}

pub fn seek_frame(text: &str, view: &TimecodeView<'_>) -> Result<i64, Error> {
    let count = read_count(text, view)?;
    if counts_source(view.display, view.viewer) {
        return source_playhead(count, view);
    }
    match view.display {
        TimecodeDisplay::Timer1 => Ok(view.timer_zero + count),
        TimecodeDisplay::Auto
        | TimecodeDisplay::SourceTimecode
        | TimecodeDisplay::SourceFrame
        | TimecodeDisplay::RecordTimecode
        | TimecodeDisplay::RecordFrame
        | TimecodeDisplay::Timer2 => Ok(count),
    }
}

fn counts_source(display: TimecodeDisplay, viewer: ViewerMode) -> bool {
    match display {
        TimecodeDisplay::Auto => viewer.is_source(),
        TimecodeDisplay::SourceTimecode | TimecodeDisplay::SourceFrame => true,
        TimecodeDisplay::RecordTimecode
        | TimecodeDisplay::RecordFrame
        | TimecodeDisplay::Timer1
        | TimecodeDisplay::Timer2 => false,
    }
}

fn read_count(text: &str, view: &TimecodeView<'_>) -> Result<i64, Error> {
    let unread = || Error::usage("cannot read that count");
    if matches!(
        view.display,
        TimecodeDisplay::SourceFrame | TimecodeDisplay::RecordFrame
    ) {
        return parse_frame_number(text).ok_or_else(unread);
    }
    let start = match view.display {
        TimecodeDisplay::RecordTimecode => view.start_tc,
        TimecodeDisplay::Auto if !view.viewer.is_source() => view.start_tc,
        TimecodeDisplay::Auto | TimecodeDisplay::SourceTimecode => view.source_tc,
        TimecodeDisplay::SourceFrame
        | TimecodeDisplay::RecordFrame
        | TimecodeDisplay::Timer1
        | TimecodeDisplay::Timer2 => 0,
    };
    parse_counter(text, view.fps, view.drop, start).ok_or_else(unread)
}

fn source_playhead(n: i64, view: &TimecodeView<'_>) -> Result<i64, Error> {
    match view.viewer {
        ViewerMode::SourceClip => Ok(n),
        ViewerMode::SourceTape => {
            let frames = view
                .tape_frames
                .ok_or_else(|| Error::usage("no source frame here"))?;
            if !(0..frames).contains(&n) {
                return Err(Error::usage("that frame is outside the clip"));
            }
            Ok(view.tape_start + n)
        }
        ViewerMode::Timeline => {
            let clip = view
                .clip
                .ok_or_else(|| Error::usage("no source frame here"))?;
            (clip.start..clip.end())
                .find(|&t| clip.source_at(t) == n)
                .ok_or_else(|| Error::usage("that frame is outside the clip"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(num: i64, den: i64) -> Rational {
        Rational::new(num, den).unwrap()
    }

    fn view(display: TimecodeDisplay, viewer: ViewerMode) -> TimecodeView<'static> {
        TimecodeView {
            display,
            viewer,
            playhead: 0,
            fps: rate(24, 1),
            drop: false,
            start_tc: 0,
            source_tc: 0,
            timer_zero: 0,
            source_frame: None,
            tape_start: 0,
            tape_frames: None,
            clip: None,
        }
    }

    fn placed(start: i64, source_in: i64) -> Clip {
        Clip {
            id: "c1".into(),
            media: "m".into(),
            start,
            duration: 20,
            source_in,
            video: true,
            audio: None,
            enabled: true,
            name: None,
            effects: None,
            generator: None,
            title: None,
            picture: None,
            compound: None,
            speed: None,
            retime: vidcut_core::model::ClipRetime::Project,
            color: None,
            render_cache: vidcut_core::model::ClipCache::Auto,
        }
    }

    #[test]
    fn record_timecode_adds_the_timeline_start() {
        let mut view = view(TimecodeDisplay::RecordTimecode, ViewerMode::Timeline);
        view.start_tc = 86400;
        assert_eq!(format_timecode_field(&view).as_deref(), Some("01:00:00:00"));
    }

    #[test]
    fn auto_on_a_source_ignores_the_timeline_start() {
        let n = source_frame_at(None, ViewerMode::SourceClip, 24, &[]).unwrap();
        let mut view = view(TimecodeDisplay::Auto, ViewerMode::SourceClip);
        view.playhead = 24;
        view.start_tc = 86400;
        view.source_frame = Some(n);
        assert_eq!(format_timecode_field(&view).as_deref(), Some("00:00:01:00"));
    }

    #[test]
    fn drop_frame_record_timecode_uses_a_semicolon() {
        let mut view = view(TimecodeDisplay::RecordTimecode, ViewerMode::Timeline);
        view.fps = rate(30000, 1001);
        view.drop = true;
        assert_eq!(format_timecode_field(&view).as_deref(), Some("00:00:00;00"));
    }

    #[test]
    fn timer1_counts_from_its_zero() {
        let mut view = view(TimecodeDisplay::Timer1, ViewerMode::Timeline);
        view.playhead = 48;
        view.timer_zero = 24;
        assert_eq!(format_timecode_field(&view).as_deref(), Some("00:00:01:00"));
    }

    #[test]
    fn a_source_frame_is_a_whole_number() {
        let mut view = view(TimecodeDisplay::SourceFrame, ViewerMode::SourceClip);
        view.source_frame = Some(100);
        assert_eq!(format_timecode_field(&view).as_deref(), Some("100"));
    }

    #[test]
    fn a_tape_source_frame_is_inside_the_clip() {
        let tape = [TapeEntry {
            media: "m".into(),
            start: 900,
            frames: 500,
        }];
        let n = source_frame_at(None, ViewerMode::SourceTape, 1000, &tape).unwrap();
        let mut view = view(TimecodeDisplay::SourceFrame, ViewerMode::SourceTape);
        view.playhead = 1000;
        view.source_frame = Some(n);
        assert_eq!(format_timecode_field(&view).as_deref(), Some("100"));
    }

    #[test]
    fn a_tape_record_frame_is_the_playhead() {
        let mut view = view(TimecodeDisplay::RecordFrame, ViewerMode::SourceTape);
        view.playhead = 1000;
        assert_eq!(format_timecode_field(&view).as_deref(), Some("1000"));
    }

    #[test]
    fn a_source_count_without_a_frame_is_blank() {
        let view = view(TimecodeDisplay::SourceTimecode, ViewerMode::Timeline);
        assert_eq!(format_timecode_field(&view), None);
    }

    #[test]
    fn a_timeline_source_frame_seeks_that_clip() {
        let clip = placed(10, 5);
        let mut view = view(TimecodeDisplay::SourceFrame, ViewerMode::Timeline);
        view.clip = Some(&clip);
        assert_eq!(seek_frame("8", &view).unwrap(), 13);
    }
}
