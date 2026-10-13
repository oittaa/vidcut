//! Live control of a running GUI: JSON-RPC 2.0 over a Unix socket (a named
//! pipe on Windows, [`transport`]).
//!
//! Each running GUI registers itself in [`registry`] and serves [`server`] on
//! `$XDG_RUNTIME_DIR/vidcut/instances/<pid>.sock`. The CLI's [`route`] sends a
//! command to the GUI that has its project open (so humans and agents edit
//! the same live session), and runs it headless otherwise.
//!
//! **Wire format:** one JSON object per line, in both directions.
//! - Request: `{"jsonrpc":"2.0","id":1,"method":"edit.split","params":{"at":"1m"}}`.
//!   `params` may carry `if_rev` and `dry_run` like everywhere else. An
//!   optional top-level `"project": "/abs/path.vidcut"` makes the GUI refuse
//!   the request unless it has that project open.
//! - Response: `{"jsonrpc":"2.0","id":1,"result":{...}}`, where `result` is the
//!   same [`Response`](vidcut_core::Response) object `vidcut --json` prints.
//! - Error: `{"jsonrpc":"2.0","id":1,"error":{"code":4,"message":"...","data":{"code":"INVALID_EDIT"}}}`.
//!   `code` is the CLI exit code; `data.code` the stable error name.
//! - Long requests (`export.start`) first send notifications:
//!   `{"jsonrpc":"2.0","method":"progress","params":{...}}`. Closing the
//!   connection cancels them, and also a frame, a storyboard, an analysis
//!   or the loudness measure before an export, which send no progress.

pub mod client;
pub mod registry;
pub mod server;
pub mod transport;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vidcut_core::{Envelope, Error, ErrorCode, Request, Result};

pub use registry::Instance;

/// Where a command runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Headless,
    Instance(Instance),
}

/// How the user asked to route (`--headless`, `--instance`, `VIDCUT_SOCKET`).
#[derive(Debug, Clone, Default)]
pub struct RouteOptions {
    pub headless: bool,
    /// A pid or a socket path.
    pub instance: Option<String>,
}

/// Runs `envelope` where [`route`] says: in the GUI that has `project` open,
/// or headless. `progress` gets export progress as
/// `{"output", "outputs", "path", "frames", "total", "elapsed"}`. `cancel`
/// stops a headless export, frame, storyboard or analysis. For a GUI it
/// closes the connection, which ends the wait and stops that work there
/// (a background export job keeps going; `jobs.cancel` stops it).
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
                        "elapsed": p.elapsed.as_secs_f64(),
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

/// Picks where `request` runs:
/// - project-less requests (`schema`, `probe`, ...) always run here;
/// - `--headless` runs here (live-only requests then fail with `NO_INSTANCE`);
/// - `--instance` / `VIDCUT_SOCKET` pick a GUI explicitly;
/// - otherwise a GUI that has `project` open, or with no `project`, the only
///   GUI that has a project open. Live-only requests need a GUI.
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
    // No project given: a lone GUI is the obvious target.
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
    // A socket path: the registry entry if there is one, else just the socket.
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

/// The JSON-RPC request object for `envelope`.
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
    if let Some(project) = project {
        v["project"] = json!(project);
    }
    Ok(v)
}

/// The JSON-RPC error object for `e`.
pub fn error_json(e: &Error) -> Value {
    json!({ "code": e.code.exit_code(), "message": e.message, "data": { "code": e.code } })
}

/// Turns a JSON-RPC error object back into an [`Error`].
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

/// `$XDG_RUNTIME_DIR/vidcut`, or a private directory under the temp dir
/// (on Windows the user's own temporary folder).
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("vidcut"),
        #[cfg(unix)]
        None => std::env::temp_dir().join(format!("vidcut-{}", unsafe { libc::getuid() })),
        #[cfg(not(unix))]
        None => std::env::temp_dir().join("vidcut"),
    }
}

/// Creates `dir` (and parents) readable only by this user. On Windows the
/// folders it is made in (the user's profile, its temporary folder) are the
/// user's own already, and a new folder inherits that.
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
