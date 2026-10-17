use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use vidcut_core::Project;
use vidcut_core::render::TapeEntry;
use vidcut_core::request::ViewerMode;

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
}

#[derive(Debug, Clone)]
pub struct ProjectSnap {
    pub path: PathBuf,
    pub project: Arc<Project>,
    pub undo: Vec<String>,
    pub redo: Vec<String>,
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
    pub last_source: ViewerMode,
    pub source: PlayerSnap,
    pub timeline: PlayerSnap,
    pub source_media: Option<String>,
    pub tape: Arc<Vec<TapeEntry>>,
    pub selection: Selection,
    pub pool: PoolView,
    pub clipboard: usize,
    pub log: Arc<VecDeque<LogEntry>>,
    pub notices: Arc<VecDeque<Notice>>,
    pub thumbs: Arc<HashMap<String, Arc<Filmstrip>>>,
    pub missing: Arc<HashSet<String>>,
    pub lost_proxies: Arc<HashMap<String, PathBuf>>,
    pub waveforms: Arc<HashMap<String, Arc<vidcut_av::waveform::Waveform>>>,
    pub jobs: Arc<Vec<super::JobInfo>>,
    pub audio_scrubbing: bool,
    pub stats: bool,
    pub preview_error: Option<String>,
    pub theme: Arc<crate::themes::Theme>,
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
}

pub fn tape_at(tape: &[TapeEntry], position: i64) -> Option<&TapeEntry> {
    tape.iter()
        .find(|e| position < e.start + e.frames)
        .or(tape.last())
}
