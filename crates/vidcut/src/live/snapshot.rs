//! What the live session publishes for the GUI: an immutable snapshot the UI
//! thread reads without ever waiting on the session.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use vidcut_core::Project;
use vidcut_core::render::TapeEntry;
use vidcut_core::request::ViewerMode;

/// The two players behind the single viewer (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerId {
    /// Source Clip and Source Tape.
    Source,
    Timeline,
}

impl PlayerId {
    pub const ALL: [PlayerId; 2] = [PlayerId::Source, PlayerId::Timeline];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn of(mode: ViewerMode) -> Self {
        if mode == ViewerMode::Timeline {
            Self::Timeline
        } else {
            Self::Source
        }
    }
}

/// A player's state in frames of its content.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PlayerSnap {
    /// Something is loaded (otherwise the viewer shows black).
    pub loaded: bool,
    /// The monitoring mute is on.
    pub muted: bool,
    /// Length of the content in frames.
    pub frames: i64,
    /// The playhead frame.
    pub position: i64,
    pub playing: bool,
    /// Shuttle speed (negative: backwards); 0 when stopped.
    pub speed: f64,
    /// mpv's hardware decoder in use (`nvdec`, `vaapi`, `no`).
    pub hwdec: String,
    /// Display size of the current video frame.
    pub video_size: Option<(u32, u32)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectSnap {
    pub path: PathBuf,
    pub project: Arc<Project>,
    /// Undo and redo labels, newest first.
    pub undo: Vec<String>,
    pub redo: Vec<String>,
}

/// The Media Pool view: the bin shown and the order of its media. Source
/// Tape strings together that bin's media, and its bins', in that order
///.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PoolView {
    /// A bin path; empty: the top level (all media).
    pub bin: String,
    pub sort: vidcut_core::request::PoolSort,
    pub descending: bool,
}

impl PoolView {
    /// The media shown, in order: those in the bin or bins inside it.
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
    /// Selected timeline clips.
    pub clips: Vec<String>,
    /// Selected pool media.
    pub media: Vec<String>,
}

/// One request in the command log.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub seq: u64,
    /// Unix time in seconds.
    pub time: f64,
    /// `gui`, `ipc` or `session`.
    pub origin: String,
    pub method: String,
    pub params: Value,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Project revision after the request.
    pub rev: u64,
    /// Whether the request changed the project.
    pub changed: bool,
}

/// A message for the status bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub seq: u64,
    pub text: String,
    pub error: bool,
}

/// Pool thumbnails of one media item.
#[derive(Debug, Clone, PartialEq)]
pub struct Filmstrip {
    pub width: u32,
    pub height: u32,
    /// RGBA pictures, evenly spread over the media.
    pub frames: Vec<Vec<u8>>,
    /// Source time (s) of each picture.
    pub times: Vec<f64>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Increases with every publication.
    pub seq: u64,
    pub pid: u32,
    pub socket: Option<PathBuf>,
    pub project: Option<ProjectSnap>,
    pub viewer: ViewerMode,
    /// The source mode Q returns to.
    pub last_source: ViewerMode,
    pub source: PlayerSnap,
    pub timeline: PlayerSnap,
    /// Media shown in Source Clip mode.
    pub source_media: Option<String>,
    /// Media layout of the Source Tape.
    pub tape: Arc<Vec<TapeEntry>>,
    pub selection: Selection,
    pub pool: PoolView,
    /// Clips on the clipboard.
    pub clipboard: usize,
    pub log: Arc<VecDeque<LogEntry>>,
    pub notices: Arc<VecDeque<Notice>>,
    pub thumbs: Arc<HashMap<String, Arc<Filmstrip>>>,
    /// Audio peaks of media (their first audio stream).
    pub waveforms: Arc<HashMap<String, Arc<vidcut_av::waveform::Waveform>>>,
    /// Export jobs, oldest first.
    pub jobs: Arc<Vec<super::JobInfo>>,
    /// Why there is no preview (libmpv missing), if so.
    pub preview_error: Option<String>,
}

impl Snapshot {
    pub fn player(&self, id: PlayerId) -> &PlayerSnap {
        match id {
            PlayerId::Source => &self.source,
            PlayerId::Timeline => &self.timeline,
        }
    }

    /// The player the viewer shows.
    pub fn active(&self) -> &PlayerSnap {
        self.player(PlayerId::of(self.viewer))
    }

    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref().map(|p| &*p.project)
    }

    /// The media whose source the viewer shows: the Source Clip, or the clip
    /// under the Source Tape playhead, with the source frame there.
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

/// The tape entry under `position` (the last one at or past the end).
pub fn tape_at(tape: &[TapeEntry], position: i64) -> Option<&TapeEntry> {
    tape.iter()
        .find(|e| position < e.start + e.frames)
        .or(tape.last())
}
