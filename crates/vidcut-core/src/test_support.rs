//! Test helpers: fake media and an editor driven by JSON requests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::editor::{Editor, Env, History, ProbedMedia};
use crate::error::{Error, Result};
use crate::model::{AudioInfo, MediaInfo, Project, Settings, VideoInfo};
use crate::request::{Envelope, Response};
use crate::time::Rational;

/// Media facts for a fake file: `secs` long at `fps`, optionally with stereo audio.
pub fn media_info(secs: f64, fps: Rational, audio: bool) -> MediaInfo {
    MediaInfo {
        format: "mov,mp4,m4a,3gp,3g2,mj2".into(),
        duration: secs,
        start_time: 0.0,
        video: Some(VideoInfo {
            codec: "h264".into(),
            width: 1920,
            height: 1080,
            sar: [1, 1],
            pix_fmt: "yuv420p".into(),
            fps,
            vfr: false,
            first_pts: 0.0,
            duration: secs,
        }),
        audio: if audio {
            vec![AudioInfo {
                index: 0,
                codec: "aac".into(),
                sample_rate: 48_000,
                channels: 2,
                first_pts: 0.0,
                duration: secs,
            }]
        } else {
            vec![]
        },
    }
}

/// Media facts for a fake audio-only file (music, narration).
pub fn audio_info(secs: f64) -> MediaInfo {
    let mut info = media_info(secs, Rational::new(25, 1).unwrap(), true);
    info.video = None;
    info
}

/// An Env whose files are fake media keyed by path.
#[derive(Default)]
pub struct FakeEnv {
    pub files: HashMap<PathBuf, MediaInfo>,
}

impl FakeEnv {
    pub fn with(mut self, path: &str, info: MediaInfo) -> Self {
        self.files.insert(PathBuf::from(path), info);
        self
    }
}

impl Env for FakeEnv {
    fn probe(&self, path: &Path) -> Result<ProbedMedia> {
        let info = self
            .files
            .get(path)
            .cloned()
            .ok_or_else(|| Error::media(format!("cannot open {}", path.display())))?;
        Ok(ProbedMedia {
            path: path.to_path_buf(),
            size: 1,
            mtime_ns: 0,
            info,
        })
    }

    fn find_file(&self, dirs: &[PathBuf], name: &std::ffi::OsStr) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|p| p.file_name() == Some(name) && dirs.iter().any(|d| p.starts_with(d)))
            .cloned()
            .collect();
        found.sort();
        found
    }
}

/// An editor at 25 fps with media `m1`..: each `(secs, audio)` becomes `/media/<n>.mp4`.
pub struct Fixture {
    pub editor: Editor,
    pub env: FakeEnv,
}

impl Fixture {
    pub fn new(media: &[(f64, bool)]) -> Self {
        let fps = Rational::new(25, 1).unwrap();
        let mut env = FakeEnv::default();
        let mut paths = Vec::new();
        for (i, (secs, audio)) in media.iter().enumerate() {
            let path = format!("/media/{}.mp4", i + 1);
            env = env.with(&path, media_info(*secs, fps, *audio));
            paths.push(path);
        }
        let settings = Settings {
            fps,
            auto_format: false,
            ..Settings::default()
        };
        let mut f = Self {
            editor: Editor::new(Project::new(settings), History::default()),
            env,
        };
        if !paths.is_empty() {
            f.ok(json!({"method": "media.add", "params": {"paths": paths}}));
        }
        f
    }

    pub fn try_req(&mut self, req: Value) -> Result<Response> {
        let env = Envelope::from_json(req).map_err(Error::usage)?;
        self.editor.apply(env, &self.env, None)
    }

    pub fn ok(&mut self, req: Value) -> Response {
        let desc = req.to_string();
        self.try_req(req).unwrap_or_else(|e| panic!("{desc}: {e}"))
    }

    pub fn err(&mut self, req: Value) -> Error {
        let desc = req.to_string();
        let before = self.editor.project().clone();
        let e = self
            .try_req(req)
            .err()
            .unwrap_or_else(|| panic!("{desc} should fail"));
        assert_eq!(
            self.editor.project(),
            &before,
            "a failed request must not change the project"
        );
        e
    }

    /// V1 as `(id, start, duration, source_in)`.
    pub fn v1(&self) -> Vec<(String, i64, i64, i64)> {
        self.editor
            .project()
            .track("V1")
            .unwrap()
            .clips
            .iter()
            .map(|c| (c.id.clone(), c.start, c.duration, c.source_in))
            .collect()
    }

    /// Adds an audio-only file `secs` long to the pool; returns its media id.
    pub fn add_music(&mut self, name: &str, secs: f64) -> String {
        let path = format!("/media/{name}");
        self.env
            .files
            .insert(PathBuf::from(&path), audio_info(secs));
        let r = self.ok(json!({"method": "media.add", "params": {"paths": [path]}}));
        r.result["media"][0].as_str().unwrap().to_string()
    }

    /// A track's clips as `(id, start, duration, source_in)`.
    pub fn track(&self, id: &str) -> Vec<(String, i64, i64, i64)> {
        self.editor
            .project()
            .track(id)
            .map(|t| {
                t.clips
                    .iter()
                    .map(|c| (c.id.clone(), c.start, c.duration, c.source_in))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Track ids, bottom video track first.
    pub fn track_ids(&self) -> Vec<String> {
        self.editor
            .project()
            .tracks
            .iter()
            .map(|t| t.id.clone())
            .collect()
    }

    /// Shorthand: append `media` frames `a..b`.
    pub fn append(&mut self, media: &str, a: i64, b: i64) -> String {
        let r = self.ok(json!({"method": "edit.append", "params": {"media": media, "range": format!("{a}f..{b}f")}}));
        r.result["clip"].as_str().unwrap().to_string()
    }
}

pub fn row(id: &str, start: i64, duration: i64, source_in: i64) -> (String, i64, i64, i64) {
    (id.to_string(), start, duration, source_in)
}
