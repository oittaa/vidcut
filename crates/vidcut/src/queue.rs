use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vidcut_core::model::{Project, RenderStatus};
use vidcut_core::ops::render;
use vidcut_core::request::{ExportParams, Request};
use vidcut_core::{Error, Result};

pub struct Picked {
    pub job: String,
    pub project: Project,
    pub params: ExportParams,
}

#[derive(Default)]
pub struct Plan {
    pub render: Vec<Picked>,
    pub skipped: Vec<String>,
    pub completed: Vec<String>,
    pub existing: Vec<PathBuf>,
}

pub fn plan(
    p: &Project,
    proxies: &Path,
    jobs: Option<&[String]>,
    again: Option<bool>,
    replace: bool,
) -> Result<Plan> {
    let mut out = Plan::default();
    for id in render::chosen(p, jobs)? {
        let job = &p.queue[render::find(p, &id)?];
        let done = matches!(job.status, Some(RenderStatus::Completed { .. }));
        if done {
            out.completed.push(id.clone());
            if again == Some(false) {
                out.skipped.push(id);
                continue;
            }
        }
        let project = render::source(p, job)?;
        let mut params = job.export.clone();
        params.background = false;
        params.overwrite |= replace || (done && again == Some(true));
        if !params.overwrite {
            out.existing.extend(taken(&project, &params, proxies)?);
        }
        out.render.push(Picked {
            job: id,
            project,
            params,
        });
    }
    Ok(out)
}

impl Plan {
    pub fn refuse(&self, again: Option<bool>) -> Result<()> {
        if again.is_none() && !self.completed.is_empty() {
            return Err(Error::invalid(format!(
                "{} rendered already; pass again true to render it again, or false to skip it",
                self.completed.join(", ")
            )));
        }
        if let Some(f) = self.existing.first() {
            return Err(Error::invalid(format!(
                "{} exists; pass replace to write over it",
                f.display()
            )));
        }
        Ok(())
    }

    pub fn json(&self) -> Value {
        json!({
            "render": self.render.iter().map(|r| &r.job).collect::<Vec<_>>(),
            "skipped": self.skipped,
            "completed": self.completed,
            "existing": self.existing,
        })
    }
}

pub fn updated(request: &Request) -> Option<&str> {
    match request {
        Request::RenderUpdate { job, .. } => Some(job),
        _ => None,
    }
}

pub fn existing(
    p: &Project,
    job: Option<&str>,
    params: &ExportParams,
    proxies: &Path,
) -> Result<Vec<PathBuf>> {
    let mut changed = p.clone();
    match job {
        Some(job) => render::update(&mut changed, job, params.clone())?,
        None => {
            render::add(&mut changed, params.clone())?;
        }
    }
    let files = taken(&render::top(p), params, proxies)?;
    Ok(if params.overwrite { Vec::new() } else { files })
}

pub fn refuse_existing(files: &[PathBuf]) -> Result<()> {
    match files.first() {
        Some(f) => Err(Error::invalid(format!(
            "{} exists; pass overwrite to replace it",
            f.display()
        ))),
        None => Ok(()),
    }
}

fn taken(p: &Project, params: &ExportParams, proxies: &Path) -> Result<Vec<PathBuf>> {
    if crate::export::timeline_format(&params.output).is_some() {
        return Ok(params
            .output
            .exists()
            .then(|| params.output.clone())
            .into_iter()
            .collect());
    }
    let probe = ExportParams {
        overwrite: true,
        ..params.clone()
    };
    let prepared = crate::export::prepare(p, &probe, None, proxies)?;
    let mut files: Vec<PathBuf> = prepared.outputs.iter().filter_map(|o| o.taken()).collect();
    let beside = crate::export::subtitles_beside(p, &prepared, &probe)?;
    files.extend(
        beside
            .into_iter()
            .filter(|f| f.path.exists())
            .map(|f| f.path),
    );
    Ok(files)
}

pub fn status(result: &Result<vidcut_core::Response>, seconds: f64) -> RenderStatus {
    match result {
        Ok(_) => RenderStatus::Completed { seconds },
        Err(e) if e.code == vidcut_core::ErrorCode::Cancelled => RenderStatus::Cancelled,
        Err(e) => RenderStatus::Failed {
            error: e.message.clone(),
        },
    }
}
