use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::analysis::{AudioLevels, Loudness, PictureStats};
use crate::editor::GeneratedProxy;
use crate::editor::{Editor, Env, History, ProbedMedia};
use crate::error::{Error, Result};
use crate::folder::Folder;
use crate::model::{
    AudioInfo, ImageSequence, Media, MediaInfo, Project, Settings, VideoInfo, proxy_size,
};
use crate::render::{AudioPiece, RenderPlan};
use crate::request::{Envelope, Response};
use crate::time::Rational;

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
            rotation: 0,
            fields: None,
            hdr: None,
            still: false,
            mastering: None,
            light: None,
            hdr10plus: false,
            dolby_vision: None,
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
        timecode: None,
    }
}

pub fn still_info(width: u32, height: u32) -> MediaInfo {
    let mut info = media_info(0.04, Rational::new(25, 1).unwrap(), false);
    info.format = "png_pipe".into();
    let v = info.video.as_mut().expect("video");
    (v.codec, v.width, v.height, v.still) = ("png".into(), width, height, true);
    info
}

#[derive(Default)]
pub struct FakeEnv {
    pub files: HashMap<PathBuf, MediaInfo>,
    pub sequences: HashMap<PathBuf, (ImageSequence, u32, u32)>,
    pub levels: HashMap<PathBuf, AudioLevels>,
    pub pictures: HashMap<PathBuf, PictureStats>,
    pub proxy_rules: crate::preferences::Proxies,
    pub sizes: HashMap<PathBuf, u64>,
    pub fingerprints: HashMap<PathBuf, String>,
    pub lost_proxies: HashSet<PathBuf>,
    pub others: Vec<PathBuf>,
    pub looks: Option<PathBuf>,
    pub transcripts: HashMap<PathBuf, Arc<crate::transcript::Speech>>,
}

pub fn levels(secs: f64, loud: &[(f64, f64)]) -> AudioLevels {
    let peaks = (0..(secs * 100.0).round() as usize)
        .map(|i| {
            let t = i as f64 / 100.0;
            if loud.iter().any(|&(a, b)| t >= a && t < b) {
                0.5
            } else {
                0.001
            }
        })
        .collect();
    AudioLevels {
        per_second: 100,
        start: 0.0,
        peaks,
    }
}

pub fn pictures(secs: f64, fps: f64, shots: &[f64], black: &[(f64, f64)]) -> PictureStats {
    let n = (secs * fps).round() as usize;
    let pts: Vec<f64> = (0..n).map(|k| k as f64 / fps).collect();
    let score = pts
        .iter()
        .map(|&t| {
            if shots.iter().any(|&s| (t - s).abs() < 0.5 / fps) {
                60.0
            } else {
                0.5
            }
        })
        .collect();
    let black = pts
        .iter()
        .map(|&t| {
            if black.iter().any(|&(a, b)| t >= a && t < b) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    PictureStats { pts, score, black }
}

impl FakeEnv {
    pub fn with(mut self, path: &str, info: MediaInfo) -> Self {
        self.files.insert(PathBuf::from(path), info);
        self
    }
}

fn tree(dir: &Path, paths: &[PathBuf]) -> Folder {
    let mut files: Vec<PathBuf> = paths
        .iter()
        .filter(|p| p.parent() == Some(dir))
        .cloned()
        .collect();
    crate::folder::sort_by_name(&mut files);
    let mut dirs: Vec<PathBuf> = paths
        .iter()
        .filter_map(|p| {
            let mut parts = p.strip_prefix(dir).ok()?.components();
            let first = parts.next()?;
            parts.next()?;
            Some(dir.join(first))
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    crate::folder::sort_by_name(&mut dirs);
    Folder {
        path: dir.to_path_buf(),
        files,
        folders: dirs.iter().map(|d| tree(d, paths)).collect(),
        error: None,
    }
}

impl Env for FakeEnv {
    fn transcripts(&self, media: &Media, _stream: u32) -> Result<Vec<crate::transcripts::Kept>> {
        Ok(self
            .transcripts
            .get(&media.path)
            .map(|s| crate::transcripts::Kept {
                key: crate::transcripts::key(s),
                made: 0,
                speech: s.clone(),
            })
            .into_iter()
            .collect())
    }

    fn probe(&self, path: &Path) -> Result<ProbedMedia> {
        let info = self
            .files
            .get(path)
            .cloned()
            .ok_or_else(|| Error::media(format!("cannot open {}", path.display())))?;
        Ok(ProbedMedia {
            path: path.to_path_buf(),
            size: self.sizes.get(path).copied().unwrap_or(1),
            mtime_ns: 0,
            info,
            sequence: None,
            fingerprint: self.fingerprints.get(path).cloned(),
            subtitles: None,
        })
    }

    fn probe_sequence(&self, path: &Path, rate: Rational) -> Result<ProbedMedia> {
        let dir = path.parent().unwrap_or(Path::new(""));
        let (first, (s, w, h)) = self
            .sequences
            .iter()
            .find(|(_, (s, ..))| (0..s.count).any(|k| s.file(dir, k) == path))
            .ok_or_else(|| Error::media(format!("{}: not a numbered image", path.display())))?;
        let secs = rate.frames_to_secs(s.count as i64);
        let mut info = media_info(secs, rate, false);
        info.format = "image2".into();
        let v = info.video.as_mut().expect("video");
        (v.codec, v.width, v.height) = ("png".into(), *w, *h);
        Ok(ProbedMedia {
            path: first.clone(),
            size: self.sizes.get(first).copied().unwrap_or(1),
            mtime_ns: 0,
            info,
            sequence: Some(s.clone()),
            fingerprint: self.fingerprints.get(first).cloned(),
            subtitles: None,
        })
    }

    fn audio_levels(&self, media: &Media, _stream: u32) -> Result<Arc<AudioLevels>> {
        Ok(Arc::new(
            self.levels
                .get(&media.path)
                .cloned()
                .unwrap_or_else(|| levels(media.info.duration, &[(0.0, media.info.duration)])),
        ))
    }

    fn picture_stats(&self, media: &Media) -> Result<Arc<PictureStats>> {
        let fps = media.info.video.as_ref().map_or(25.0, |v| v.fps.as_f64());
        Ok(Arc::new(
            self.pictures
                .get(&media.path)
                .cloned()
                .unwrap_or_else(|| pictures(media.info.duration, fps, &[], &[])),
        ))
    }

    fn loudness(&self, plan: &RenderPlan) -> Result<Loudness> {
        let gains: Vec<f64> = plan
            .audio
            .iter()
            .flatten()
            .filter_map(|p| match p {
                AudioPiece::Source { gain_db, .. } => Some(*gain_db),
                AudioPiece::Silence { .. } => None,
            })
            .collect();
        let integrated = match gains.iter().cloned().reduce(f64::max) {
            Some(g) => -20.0 + g,
            None => f64::NEG_INFINITY,
        };
        Ok(Loudness {
            integrated,
            range: 0.0,
            true_peak: integrated + 6.0,
            sample_peak: integrated + 5.0,
        })
    }

    fn proxy_dir(&self) -> PathBuf {
        PathBuf::from("/proxies")
    }

    fn proxy_rules(&self) -> crate::preferences::Proxies {
        self.proxy_rules.clone()
    }

    fn generate_proxy(
        &self,
        media: &Media,
        longest: u32,
        codec: Option<&str>,
        dir: &Path,
        _dry_run: bool,
    ) -> Result<GeneratedProxy> {
        let v = media
            .info
            .video
            .as_ref()
            .ok_or_else(|| Error::invalid(format!("{} has no picture", media.id)))?;
        let (w, h) = v.display_size();
        let (w, h) = proxy_size(w, h, longest);
        Ok(GeneratedProxy {
            path: dir.join(format!("{}-{}x{h}.mkv", media.id, w)),
            width: w,
            height: h,
            codec: codec.unwrap_or("h264").into(),
            made_with: None,
            copies: None,
        })
    }

    fn find_files(
        &self,
        dirs: &[PathBuf],
        names: &std::collections::HashSet<std::ffi::OsString>,
    ) -> HashMap<std::ffi::OsString, Vec<PathBuf>> {
        let mut found: HashMap<std::ffi::OsString, Vec<PathBuf>> = HashMap::new();
        let mut paths: Vec<&PathBuf> = self.files.keys().chain(self.sequences.keys()).collect();
        paths.sort();
        for p in paths {
            if let Some(name) = p.file_name().filter(|n| names.contains(*n))
                && dirs.iter().any(|d| p.starts_with(d))
            {
                found
                    .entry(name.to_os_string())
                    .or_default()
                    .push(p.clone());
            }
        }
        found
    }

    fn read_folder(&self, dir: &Path) -> Result<Folder> {
        let frames = self.sequences.iter().flat_map(|(first, (s, ..))| {
            let at = first.parent().unwrap_or(Path::new("")).to_path_buf();
            (0..s.count).map(move |k| s.file(&at, k))
        });
        let mut paths: Vec<PathBuf> = self
            .files
            .keys()
            .cloned()
            .chain(frames)
            .chain(self.others.iter().cloned())
            .filter(|p| p.starts_with(dir) && p != dir)
            .collect();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return Err(Error::media(format!("{}: no such folder", dir.display())));
        }
        Ok(tree(dir, &paths))
    }

    fn present(&self, m: &Media) -> bool {
        self.files.contains_key(&m.path) || self.sequences.contains_key(&m.path)
    }

    fn proxy_present(&self, path: &Path) -> bool {
        !self.lost_proxies.contains(path)
    }

    fn looks(&self, kind: crate::looks::LookKind) -> crate::looks::Listing {
        self.looks
            .as_deref()
            .map(|dir| crate::looks::read(dir, kind))
            .unwrap_or_default()
    }
}

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

    pub fn append(&mut self, media: &str, a: i64, b: i64) -> String {
        let r = self.ok(json!({"method": "edit.append", "params": {"media": media, "range": format!("{a}f..{b}f")}}));
        r.result["clip"].as_str().unwrap().to_string()
    }
}

pub fn row(id: &str, start: i64, duration: i64, source_in: i64) -> (String, i64, i64, i64) {
    (id.to_string(), start, duration, source_in)
}
