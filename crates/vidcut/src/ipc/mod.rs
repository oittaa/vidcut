pub mod client;
pub mod registry;
pub mod server;
pub mod transport;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vidcut_core::{Envelope, Error, ErrorCode, Request, Result};

pub use registry::Instance;

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Headless,
    Instance(Instance),
}

#[derive(Debug, Clone, Default)]
pub struct RouteOptions {
    pub headless: bool,
    pub instance: Option<String>,
}

pub fn run(
    project: Option<&Path>,
    options: &RouteOptions,
    envelope: Envelope,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: &mut dyn FnMut(&Value),
) -> Result<vidcut_core::Response> {
    match route(project, options, &envelope.request)? {
        Route::Headless => crate::session::run_with(
            project,
            envelope,
            &mut crate::session::Hooks {
                cancel,
                progress: &mut |i, n, path, p| {
                    progress(&json!({
                        "output": i, "outputs": n, "path": path,
                        "frames": p.frames, "total": p.total,
                        "pass": p.pass, "passes": p.passes, "measuring": p.measuring,
                        "stage": p.stage, "elapsed": p.elapsed.as_secs_f64(),
                    }))
                },
            },
        ),
        Route::Instance(instance) => {
            let project = project.and_then(|p| vidcut_core::dirs::canonical(p).ok());
            client::call(
                &instance.socket,
                &envelope,
                project.as_deref(),
                progress,
                Some(&**cancel),
            )
        }
    }
}

pub fn route(project: Option<&Path>, options: &RouteOptions, request: &Request) -> Result<Route> {
    if request.is_projectless() {
        return Ok(Route::Headless);
    }
    if options.headless {
        return Ok(Route::Headless);
    }
    if let Some(which) = &options.instance {
        return find_explicit(which).map(Route::Instance);
    }
    let live = request.is_live();
    let instances = registry::list();
    if let Some(project) = project {
        let want = vidcut_core::dirs::canonical(project).ok();
        if let Some(i) = instances
            .iter()
            .find(|i| i.project.is_some() && i.project.as_deref() == want.as_deref())
        {
            return Ok(Route::Instance(i.clone()));
        }
        if live {
            return Err(Error::no_instance(format!(
                "no running vidcut GUI has {} open; start one with `vidcut -p {}`",
                project.display(),
                project.display()
            )));
        }
        return Ok(Route::Headless);
    }
    let candidates: Vec<&Instance> = if live {
        instances.iter().collect()
    } else {
        instances.iter().filter(|i| i.project.is_some()).collect()
    };
    match candidates.as_slice() {
        [one] => Ok(Route::Instance((*one).clone())),
        [] if live => Err(Error::no_instance(format!(
            "{} needs a running vidcut GUI; start one with `vidcut` or `vidcut -p FILE`",
            request.method()
        ))),
        [] => Ok(Route::Headless),
        many => Err(Error::usage(format!(
            "{} vidcut GUIs are running ({}); pick one with -p FILE or --instance PID",
            many.len(),
            many.iter()
                .map(|i| i.pid.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn find_explicit(which: &str) -> Result<Instance> {
    if let Ok(pid) = which.parse::<u32>() {
        return registry::list()
            .into_iter()
            .find(|i| i.pid == pid)
            .ok_or_else(|| Error::no_instance(format!("no running vidcut GUI with pid {pid}")));
    }
    let socket = PathBuf::from(which);
    Ok(registry::list()
        .into_iter()
        .find(|i| i.socket == socket)
        .unwrap_or(Instance {
            pid: 0,
            socket,
            project: None,
            version: String::new(),
        }))
}

pub fn request_json(id: u64, envelope: &Envelope, project: Option<&Path>) -> Result<Value> {
    let Value::Object(mut m) =
        serde_json::to_value(&envelope.request).map_err(|e| Error::internal(e.to_string()))?
    else {
        return Err(Error::internal("request did not serialize to an object"));
    };
    let method = m.remove("method").unwrap_or(Value::Null);
    let mut params = m.remove("params").unwrap_or_else(|| json!({}));
    if let Some(p) = params.as_object_mut() {
        if let Some(rev) = envelope.if_rev {
            p.insert("if_rev".into(), json!(rev));
        }
        if envelope.dry_run {
            p.insert("dry_run".into(), json!(true));
        }
    }
    let mut v = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    if let Some(client) = &envelope.client {
        v["client"] = json!(client);
    }
    if let Some(project) = project {
        v["project"] = json!(project);
    }
    Ok(v)
}

pub fn error_json(e: &Error) -> Value {
    json!({ "code": e.code.exit_code(), "message": e.message, "data": { "code": e.code } })
}

pub fn error_from_json(v: &Value) -> Error {
    let code = v
        .get("data")
        .and_then(|d| d.get("code"))
        .and_then(|c| serde_json::from_value::<ErrorCode>(c.clone()).ok())
        .unwrap_or(ErrorCode::Internal);
    let message = v
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error from the vidcut GUI")
        .to_string();
    Error::new(code, message)
}

pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("vidcut"),
        #[cfg(unix)]
        None => std::env::temp_dir().join(format!("vidcut-{}", unsafe { libc::getuid() })),
        #[cfg(not(unix))]
        None => std::env::temp_dir().join("vidcut"),
    }
}

pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projectless_requests_never_route() {
        let r = Request::from_parts("schema", None).unwrap();
        let opts = RouteOptions {
            instance: Some("999999999".into()),
            ..Default::default()
        };
        assert_eq!(route(None, &opts, &r).unwrap(), Route::Headless);
    }
}
