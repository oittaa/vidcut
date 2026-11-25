use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Project, RenderJob, RenderStatus};
use crate::request::{ExportParams, Request};
use crate::time::{RangeEnd, RangeSpec, TimeSpec};

pub fn top(p: &Project) -> Project {
    let mut top = p.clone();
    if !top.compound_path.is_empty() {
        top.check_in();
        top.compound_path.clear();
        top.check_out();
    }
    top
}

fn fixed(p: &Project, mut export: ExportParams) -> Result<(String, ExportParams)> {
    let timeline =
        (p.open_id().map(str::to_string)).ok_or_else(|| Error::not_found("no timeline is open"))?;
    let top = top(p);
    let end = top.end();
    let (a, b) = match &export.range {
        Some(r) => top.time_context(None).range(r)?,
        None => match (top.marks.mark_in, top.marks.mark_out) {
            (Some(i), Some(o)) => (i, o),
            _ => (0, end),
        },
    };
    let (a, b) = (a.max(0), b.min(end));
    if b <= a {
        return Err(Error::invalid(format!("nothing to render in {a}..{b}")));
    }
    export.range = Some(RangeSpec {
        start: TimeSpec::Frames(a),
        end: RangeEnd::At(TimeSpec::Frames(b)),
    });
    export.background = false;
    Ok((timeline, export))
}

pub fn add(p: &mut Project, export: ExportParams) -> Result<String> {
    let (timeline, export) = fixed(p, export)?;
    let n = (p.queue.iter())
        .filter_map(|j| j.id.strip_prefix('r')?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let id = format!("r{n}");
    p.queue.push(RenderJob {
        id: id.clone(),
        name: None,
        timeline,
        export,
        status: None,
    });
    Ok(id)
}

pub fn update(p: &mut Project, job: &str, export: ExportParams) -> Result<()> {
    let i = find(p, job)?;
    let (timeline, export) = fixed(p, export)?;
    let j = &mut p.queue[i];
    j.timeline = timeline;
    j.export = export;
    j.status = None;
    Ok(())
}

pub fn find(p: &Project, job: &str) -> Result<usize> {
    (p.queue.iter().position(|j| j.id == job))
        .ok_or_else(|| Error::not_found(format!("no render job {job:?}")))
}

pub fn chosen(p: &Project, jobs: Option<&[String]>) -> Result<Vec<String>> {
    match jobs {
        None => Ok(p.queue.iter().map(|j| j.id.clone()).collect()),
        Some(ids) => {
            for id in ids {
                find(p, id)?;
            }
            Ok((p.queue.iter())
                .filter(|j| ids.contains(&j.id))
                .map(|j| j.id.clone())
                .collect())
        }
    }
}

pub fn remove(p: &mut Project, jobs: &[String]) -> Result<Vec<String>> {
    let gone = chosen(p, Some(jobs))?;
    p.queue.retain(|j| !gone.contains(&j.id));
    Ok(gone)
}

pub fn move_to(p: &mut Project, job: &str, to: usize) -> Result<()> {
    let from = find(p, job)?;
    if to == 0 || to > p.queue.len() {
        return Err(Error::invalid(format!(
            "a place in the queue is from 1 to {}",
            p.queue.len()
        )));
    }
    let j = p.queue.remove(from);
    p.queue.insert(to - 1, j);
    Ok(())
}

pub fn rename(p: &mut Project, job: &str, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::usage("name: the new name of the job"));
    }
    let i = find(p, job)?;
    p.queue[i].name = Some(name.to_string());
    Ok(())
}

pub fn clear(p: &mut Project, all: bool) -> Vec<String> {
    let gone: Vec<String> = (p.queue.iter())
        .filter(|j| all || matches!(j.status, Some(RenderStatus::Completed { .. })))
        .map(|j| j.id.clone())
        .collect();
    p.queue.retain(|j| !gone.contains(&j.id));
    gone
}

pub fn clear_status(p: &mut Project, jobs: Option<&[String]>) -> Result<()> {
    let ids = chosen(p, jobs)?;
    for j in p.queue.iter_mut().filter(|j| ids.contains(&j.id)) {
        j.status = None;
    }
    Ok(())
}

pub fn ended(p: &mut Project, job: &str, status: RenderStatus) -> Result<()> {
    let i = find(p, job)?;
    p.queue[i].status = Some(status);
    Ok(())
}

pub fn change(p: &mut Project, request: &Request) -> Result<Value> {
    Ok(match request {
        Request::RenderAdd(export) => json!({ "id": add(p, export.clone())? }),
        Request::RenderUpdate { job, export } => {
            update(p, job, export.clone())?;
            Value::Null
        }
        Request::RenderRemove { jobs } => json!({ "removed": remove(p, &jobs.to_vec())? }),
        Request::RenderMove { job, to } => {
            move_to(p, job, *to)?;
            Value::Null
        }
        Request::RenderRename { job, name } => {
            rename(p, job, name)?;
            Value::Null
        }
        Request::RenderClear { all } => json!({ "removed": clear(p, *all) }),
        Request::RenderClearStatus { jobs } => {
            clear_status(p, jobs.as_ref().map(|j| j.to_vec()).as_deref())?;
            Value::Null
        }
        Request::RenderEnded { job, status } => {
            ended(p, job, status.clone())?;
            Value::Null
        }
        other => {
            return Err(Error::internal(format!(
                "{} is not a change to the render queue",
                other.method()
            )));
        }
    })
}

pub fn list(p: &Project) -> Value {
    json!({ "jobs": p.queue })
}

pub fn source(p: &Project, job: &RenderJob) -> Result<Project> {
    let mut q = top(p);
    q.open_timeline(&job.timeline)
        .map_err(|_| Error::not_found(format!("no timeline {} for {}", job.timeline, job.id)))?;
    Ok(q)
}
