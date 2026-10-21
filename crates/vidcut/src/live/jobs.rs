use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use serde::Serialize;
use serde_json::{Value, json};
use vidcut_av::export::{
    AudioNote, Note, Progress, SourceNote, SpeechNote, Stage, Tried, VideoNote,
};
use vidcut_core::Error;
use vidcut_core::editor::Copied;
use vidcut_core::export::color::ColorChoice;
use vidcut_core::export::metadata::MetadataChoice;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobOutput {
    pub path: PathBuf,
    pub frames: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub origin: &'static str,
    pub params: Value,
    pub state: JobState,
    pub outputs: Vec<JobOutput>,
    pub output: usize,
    pub frames: i64,
    pub total: i64,
    pub pass: u8,
    pub passes: u8,
    pub measuring: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
    pub progress: f64,
    pub elapsed: f64,
    pub eta: Option<f64>,
    pub started: f64,
    pub finished: Option<f64>,
    pub result: Option<Value>,
    pub error: Option<Error>,
    pub details: JobDetails,
    #[serde(skip)]
    pub(crate) clock: Instant,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JobDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoNote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioNote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataChoice>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tried: Vec<Tried>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceNote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech: Option<SpeechNote>,
    pub gpu_frames: i64,
    pub gpu_decoded: i64,
    pub copied: Copied,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(skip)]
    each: Vec<(i64, i64, Copied)>,
}

impl JobDetails {
    pub(crate) fn note(&mut self, note: Note) {
        match note {
            Note::Video(v) => self.video = Some(v),
            Note::Audio(a) => self.audio = Some(a),
            Note::Color { color, metadata } => (self.color, self.metadata) = (color, metadata),
            Note::Tried(t) => {
                if !self.tried.contains(&t) {
                    self.tried.push(t);
                }
            }
            Note::Source(s) => match self.sources.iter_mut().find(|o| o.path == s.path) {
                Some(o) => *o = s,
                None => self.sources.push(s),
            },
            Note::Downloading { name } => self.downloading = Some(name),
            Note::Speech(s) => self.speech = Some(s),
        }
    }

    fn counted(&mut self, output: usize, p: &Progress) {
        if self.each.len() <= output {
            self.each.resize(output + 1, (0, 0, Copied::default()));
        }
        self.each[output] = (p.gpu_frames, p.gpu_decoded, p.copied);
        self.gpu_frames = self.each.iter().map(|e| e.0).sum();
        self.gpu_decoded = self.each.iter().map(|e| e.1).sum();
        self.copied = self.each.iter().fold(Copied::default(), |a, e| Copied {
            down: a.down + e.2.down,
            down_bytes: a.down_bytes + e.2.down_bytes,
            up: a.up + e.2.up,
            up_bytes: a.up_bytes + e.2.up_bytes,
        });
        let secs = p.elapsed.as_secs_f64();
        let encodes = self.video.is_some() || self.audio.is_some();
        self.fps = (encodes && p.stage.is_none() && secs > 0.0 && p.frames > 0)
            .then(|| p.frames as f64 / secs);
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

impl JobInfo {
    pub(crate) fn new(id: String, origin: &'static str, params: Value) -> Self {
        let output = params["output"].as_str().unwrap_or_default();
        let path = std::path::Path::new(output);
        let name = path
            .file_name()
            .map_or(output.to_string(), |n| n.to_string_lossy().into_owned());
        let title = if params["per_clip"] == true {
            format!("{name}/ (one file per clip)")
        } else {
            name
        };
        Self {
            id,
            kind: "export",
            title,
            origin,
            params,
            state: JobState::Running,
            outputs: Vec::new(),
            output: 0,
            frames: 0,
            total: 0,
            pass: 1,
            passes: 1,
            measuring: false,
            stage: None,
            progress: 0.0,
            elapsed: 0.0,
            eta: None,
            started: now(),
            finished: None,
            result: None,
            error: None,
            details: JobDetails::default(),
            clock: Instant::now(),
        }
    }

    pub(crate) fn task(
        id: String,
        kind: &'static str,
        origin: &'static str,
        params: Value,
        title: String,
    ) -> Self {
        Self {
            kind,
            title,
            ..Self::new(id, origin, params)
        }
    }

    pub fn unfinished(&self) -> bool {
        matches!(self.state, JobState::Queued | JobState::Running)
    }

    pub(crate) fn begin(&mut self) {
        self.state = JobState::Running;
        self.started = now();
        self.clock = Instant::now();
    }

    pub(crate) fn planned(&mut self, outputs: Vec<(PathBuf, i64)>) {
        self.total = outputs.iter().map(|o| o.1).sum();
        self.outputs = outputs
            .into_iter()
            .map(|(path, frames)| JobOutput { path, frames })
            .collect();
    }

    pub(crate) fn progressed(&mut self, output: usize, p: Progress) {
        self.measuring = p.measuring;
        self.stage = p.stage;
        if p.measuring {
            self.elapsed = self.clock.elapsed().as_secs_f64();
            self.progress = p.frames as f64 / p.total.max(1) as f64;
            self.eta = None;
            return;
        }
        let before: i64 = self.outputs.iter().take(output).map(|o| o.frames).sum();
        if self.outputs.is_empty() {
            self.total = p.total;
        }
        self.output = output;
        (self.pass, self.passes) = (p.pass, p.passes);
        self.details.counted(output, &p);
        self.frames = (before + p.done()).min(self.total);
        self.elapsed = self.clock.elapsed().as_secs_f64();
        self.progress = if self.total > 0 {
            self.frames as f64 / self.total as f64
        } else {
            0.0
        };
        self.eta = match self.fraction() {
            Some(_) if self.frames > 0 => {
                Some(self.elapsed * (self.total - self.frames) as f64 / self.frames as f64)
            }
            _ => None,
        };
    }

    pub(crate) fn fraction(&self) -> Option<f64> {
        if self.measuring {
            return Some(self.progress.clamp(0.0, 1.0));
        }
        match self.stage {
            Some(Stage::Waiting | Stage::Loading) => None,
            Some(Stage::Speakers) if self.frames <= 0 => None,
            _ if self.total <= 0 => None,
            _ => Some(self.progress.clamp(0.0, 1.0)),
        }
    }

    pub(crate) fn ended(&mut self, result: &vidcut_core::Result<vidcut_core::Response>) {
        if self.state != JobState::Queued {
            self.elapsed = self.clock.elapsed().as_secs_f64();
        }
        self.finished = Some(now());
        self.eta = None;
        match result {
            Ok(r) => {
                self.state = JobState::Done;
                self.frames = self.total;
                self.progress = 1.0;
                self.result = Some(r.result.clone());
            }
            Err(e) => {
                self.state = if e.code == vidcut_core::ErrorCode::Cancelled {
                    JobState::Cancelled
                } else {
                    JobState::Failed
                };
                self.error = Some(e.clone());
            }
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({ "id": self.id }))
    }
}

#[derive(Debug, Default)]
pub(crate) struct Turns {
    current: Option<String>,
    waiting: Vec<(String, bool)>,
}

impl Turns {
    pub(crate) fn ask(&mut self, id: &str, export: bool) -> bool {
        if self.current.is_none() {
            self.current = Some(id.to_string());
            return true;
        }
        let at = match export {
            true => self.waiting.iter().position(|(_, e)| !e),
            false => None,
        };
        let at = at.unwrap_or(self.waiting.len());
        self.waiting.insert(at, (id.to_string(), export));
        false
    }

    pub(crate) fn ended(&mut self, id: &str) -> Option<String> {
        self.waiting.retain(|(w, _)| w != id);
        if self.current.as_deref() != Some(id) {
            return None;
        }
        self.current = (!self.waiting.is_empty()).then(|| self.waiting.remove(0).0);
        self.current.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tried(encoder: &str) -> Note {
        Note::Tried(Tried {
            encoder: Some(encoder.into()),
            error: "no free session".into(),
        })
    }

    fn source(path: &str, decoder: &str) -> Note {
        Note::Source(SourceNote {
            path: path.into(),
            decoding: vidcut_av::decode::Decoding {
                decoder: decoder.into(),
                ..Default::default()
            },
        })
    }

    fn made(gpu_frames: i64) -> Progress {
        Progress {
            gpu_frames,
            ..Progress::default()
        }
    }

    #[test]
    fn an_encoder_that_failed_twice_is_listed_once() {
        let mut d = JobDetails::default();
        d.note(tried("h264_nvenc"));
        d.note(tried("h264_nvenc"));
        assert_eq!(d.tried.len(), 1);
    }

    #[test]
    fn a_file_read_again_keeps_one_entry_with_its_latest_decoder() {
        let mut d = JobDetails::default();
        d.note(source("a.mp4", "h264"));
        d.note(source("a.mp4", "h264_cuvid"));
        assert_eq!(
            d.sources
                .iter()
                .map(|s| s.decoding.decoder.as_str())
                .collect::<Vec<_>>(),
            ["h264_cuvid"]
        );
    }

    #[test]
    fn gpu_frames_add_up_across_the_files_of_a_job() {
        let mut d = JobDetails::default();
        d.counted(0, &made(10));
        d.counted(1, &made(5));
        d.counted(0, &made(12));
        assert_eq!(d.gpu_frames, 17);
    }

    #[test]
    fn a_job_that_encodes_nothing_has_no_frame_rate() {
        let mut d = JobDetails::default();
        d.counted(
            0,
            &Progress::single(500, 1000, std::time::Duration::from_secs(1)),
        );
        assert_eq!(d.fps, None);
    }

    #[test]
    fn a_second_export_waits_for_the_first() {
        let mut t = Turns::default();
        assert_eq!((t.ask("j1", true), t.ask("j2", true)), (true, false));
        assert_eq!(t.ended("j1").as_deref(), Some("j2"));
    }

    #[test]
    fn an_export_goes_before_a_waiting_proxy() {
        let mut t = Turns::default();
        t.ask("j1", false);
        t.ask("j2", false);
        t.ask("j3", true);
        assert_eq!(t.ended("j1").as_deref(), Some("j3"));
    }

    #[test]
    fn a_download_fills_its_share() {
        let mut j = JobInfo::new("j".into(), "gui", json!({}));
        j.progressed(
            0,
            Progress::stage(
                Stage::Downloading,
                25,
                100,
                std::time::Duration::from_secs(1),
            ),
        );
        assert_eq!(j.fraction(), Some(0.25));
    }

    #[test]
    fn encoded_frames_fill_their_share() {
        let mut j = JobInfo::new("j".into(), "gui", json!({}));
        j.progressed(
            0,
            Progress::single(40, 100, std::time::Duration::from_secs(2)),
        );
        assert_eq!(j.fraction(), Some(0.4));
    }

    #[test]
    fn measuring_light_fills_its_share() {
        let mut j = JobInfo::new("j".into(), "gui", json!({}));
        j.progressed(
            0,
            Progress::measuring(0.4, std::time::Duration::from_secs(1)),
        );
        assert_eq!(j.fraction(), Some(0.4));
    }

    #[test]
    fn a_job_before_its_total_has_no_share() {
        let j = JobInfo::new("j".into(), "gui", json!({}));
        assert_eq!(j.fraction(), None);
    }

    #[test]
    fn loading_a_model_has_no_share() {
        let mut j = JobInfo::new("j".into(), "gui", json!({}));
        j.progressed(
            0,
            Progress::stage(Stage::Loading, 0, 0, std::time::Duration::from_secs(1)),
        );
        assert_eq!(j.fraction(), None);
        assert_eq!(j.eta, None);
    }

    #[test]
    fn a_speaker_pass_with_no_count_has_no_share() {
        let mut j = JobInfo::new("j".into(), "gui", json!({}));
        j.progressed(
            0,
            Progress::stage(
                Stage::Speakers,
                0,
                12_000,
                std::time::Duration::from_secs(4),
            ),
        );
        assert_eq!(j.fraction(), None);
        assert_eq!(j.eta, None);
    }

    #[test]
    fn a_speaker_pass_that_counts_fills_its_share() {
        let mut j = JobInfo::new("j".into(), "gui", json!({}));
        j.progressed(
            0,
            Progress::stage(
                Stage::Speakers,
                3_000,
                12_000,
                std::time::Duration::from_secs(4),
            ),
        );
        assert_eq!(j.fraction(), Some(0.25));
    }

    #[test]
    fn waiting_for_a_reply_keeps_the_upload_and_has_no_share() {
        let mut j = JobInfo::new("j".into(), "gui", json!({}));
        j.progressed(
            0,
            Progress::stage(Stage::Waiting, 80, 80, std::time::Duration::from_secs(5)),
        );
        assert_eq!(j.progress, 1.0);
        assert_eq!(j.fraction(), None);
        assert_eq!(j.eta, None);
    }

    #[test]
    fn a_job_cancelled_while_waiting_never_gets_a_turn() {
        let mut t = Turns::default();
        t.ask("j1", true);
        t.ask("j2", true);
        t.ask("j3", true);
        t.ended("j2");
        assert_eq!(t.ended("j1").as_deref(), Some("j3"));
    }
}
