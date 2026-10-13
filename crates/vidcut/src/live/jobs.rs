//! Jobs of the running GUI, exports and proxy encodes: what `jobs.*` report
//! and the jobs panel shows (docs/decisions/gui.md §9).

use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use serde::Serialize;
use serde_json::{Value, json};
use vidcut_core::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Done,
    Failed,
    Cancelled,
}

/// A file a job writes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobOutput {
    pub path: PathBuf,
    /// Length in frames.
    pub frames: i64,
}

/// One export or proxy encode.
#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    /// `j<N>`, counted per GUI session.
    pub id: String,
    /// `export` or `proxy`.
    pub kind: &'static str,
    /// What it writes: a file name, or a folder for per-clip exports; for
    /// proxies, the media they are made for.
    pub title: String,
    /// Who started it: `gui` or `ipc`.
    pub origin: &'static str,
    /// The `export.start` or `proxy.generate` params.
    pub params: Value,
    pub state: JobState,
    /// The files, once planned.
    pub outputs: Vec<JobOutput>,
    /// Index of the file being written.
    pub output: usize,
    /// Frames done over all files, and their total. A file encoded in two
    /// runs (decision D45) counts each frame half in each.
    pub frames: i64,
    pub total: i64,
    /// The run over the current file's range, of `passes`: 2 of 2 is the
    /// second pass of a two-pass export.
    pub pass: u8,
    pub passes: u8,
    /// Measuring the light the HDR metadata needs, before encoding
    /// (decision D46): `progress` is the share measured.
    pub measuring: bool,
    /// 0..1 over all files.
    pub progress: f64,
    /// Seconds since the job started.
    pub elapsed: f64,
    /// Estimated seconds left.
    pub eta: Option<f64>,
    /// Unix times in seconds.
    pub started: f64,
    pub finished: Option<f64>,
    /// The request's result, when done.
    pub result: Option<Value>,
    pub error: Option<Error>,
    #[serde(skip)]
    pub(crate) clock: Instant,
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
            progress: 0.0,
            elapsed: 0.0,
            eta: None,
            started: now(),
            finished: None,
            result: None,
            error: None,
            clock: Instant::now(),
        }
    }

    /// A `proxy.generate` encode. Its files are known only as it writes
    /// them, so progress counts the file being written.
    pub(crate) fn proxies(id: String, origin: &'static str, params: Value, title: String) -> Self {
        Self {
            kind: "proxy",
            title,
            ..Self::new(id, origin, params)
        }
    }

    pub fn running(&self) -> bool {
        self.state == JobState::Running
    }

    /// The files are planned.
    pub(crate) fn planned(&mut self, outputs: Vec<(PathBuf, i64)>) {
        self.total = outputs.iter().map(|o| o.1).sum();
        self.outputs = outputs
            .into_iter()
            .map(|(path, frames)| JobOutput { path, frames })
            .collect();
    }

    /// Output `output` has `frames` of `total` frames done in run `pass`
    /// of `passes`; or, `measuring`, that share of the light measured.
    pub(crate) fn progressed(
        &mut self,
        output: usize,
        frames: i64,
        total: i64,
        (pass, passes): (u8, u8),
        measuring: bool,
    ) {
        self.measuring = measuring;
        if measuring {
            self.elapsed = self.clock.elapsed().as_secs_f64();
            self.progress = frames as f64 / total.max(1) as f64;
            self.eta = None;
            return;
        }
        let before: i64 = self.outputs.iter().take(output).map(|o| o.frames).sum();
        if self.outputs.is_empty() {
            self.total = total;
        }
        self.output = output;
        (self.pass, self.passes) = (pass, passes);
        let done = vidcut_av::export::Progress {
            frames,
            total,
            elapsed: Default::default(),
            pass,
            passes,
            measuring,
        }
        .done();
        self.frames = (before + done).min(self.total);
        self.elapsed = self.clock.elapsed().as_secs_f64();
        self.progress = if self.total > 0 {
            self.frames as f64 / self.total as f64
        } else {
            0.0
        };
        self.eta = (self.frames > 0)
            .then(|| self.elapsed * (self.total - self.frames) as f64 / self.frames as f64);
    }

    pub(crate) fn ended(&mut self, result: &vidcut_core::Result<vidcut_core::Response>) {
        self.elapsed = self.clock.elapsed().as_secs_f64();
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
