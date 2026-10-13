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
    fn requests_round_trip_through_json_rpc() {
        let mut e =
            Envelope::from_json(json!({"method": "edit.split", "params": {"at": "1m"}})).unwrap();
        e.if_rev = Some(7);
        e.dry_run = true;
        let v = request_json(3, &e, Some(Path::new("/p.vidcut"))).unwrap();
        assert_eq!(v["method"], "edit.split");
        assert_eq!(v["params"]["if_rev"], 7);
        assert_eq!(v["project"], "/p.vidcut");
        let back =
            Envelope::from_json(json!({"method": v["method"], "params": v["params"]})).unwrap();
        assert_eq!(back, e);
        let err = Error::invalid("nope");
        assert_eq!(error_from_json(&error_json(&err)), err);
    }

    #[test]
    fn server_and_client_talk_json_rpc() {
        use std::sync::Arc;
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-ipc-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let socket = dir.join("test.sock");
        server::start(
            &socket,
            Arc::new(|envelope: Envelope, project, progress| {
                match envelope.request.method().as_str() {
                    "export.start" => {
                        for i in 0..3 {
                            assert!(progress(Some(json!({ "frames": i, "total": 3 }))));
                        }
                        Ok(vidcut_core::Response {
                            rev: 7,
                            changes: Default::default(),
                            result: json!({ "project": project }),
                            dry_run: false,
                            via: "gui:1".into(),
                            warnings: Vec::new(),
                        })
                    }
                    _ => Err(Error::invalid("nope")),
                }
            }),
        )
        .unwrap();
        let export =
            Envelope::from_json(json!({"method": "export.start", "params": {"output": "/o.mp4"}}))
                .unwrap();
        let mut seen = Vec::new();
        let r = client::call(
            &socket,
            &export,
            Some(Path::new("/p.vidcut")),
            &mut |p| seen.push(p["frames"].as_i64().unwrap()),
            None,
        )
        .unwrap();
        assert_eq!(seen, vec![0, 1, 2]);
        assert_eq!((r.rev, r.via.as_str()), (7, "gui:1"));
        assert_eq!(r.result["project"], "/p.vidcut");
        let split =
            Envelope::from_json(json!({"method": "edit.split", "params": {"at": "1s"}})).unwrap();
        let e = client::call(&socket, &split, None, &mut |_| {}, None).unwrap_err();
        assert_eq!(e, Error::invalid("nope"));
        let gone =
            client::call(&dir.join("missing.sock"), &split, None, &mut |_| {}, None).unwrap_err();
        assert_eq!(gone.code, ErrorCode::NoInstance);
    }

    #[test]
    fn the_server_limits_lines_and_connections() {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::Arc;
        use transport::Stream;
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-ipc-limits-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let socket = dir.join("test.sock");
        let limits = server::Limits {
            line: 100,
            connections: 1,
        };
        let handler = Arc::new(
            |_: Envelope, _: Option<PathBuf>, _: &mut dyn FnMut(Option<Value>) -> bool| {
                Ok(vidcut_core::Response {
                    rev: 1,
                    changes: Default::default(),
                    result: json!("ok"),
                    dry_run: false,
                    via: "gui:1".into(),
                    warnings: Vec::new(),
                })
            },
        );
        server::start_limited(&socket, handler, limits).unwrap();
        // Sends a line on a new connection and reads one line back.
        let ask = |line: &[u8]| {
            let mut s = Stream::connect(&socket).unwrap();
            // A server at its limit turns the connection away without
            // reading it and may close first: the write then fails, and the
            // refusal is still there to read.
            let _ = s.write_all(line);
            let mut reader = BufReader::new(s.try_clone().unwrap());
            let mut reply = String::new();
            reader.read_line(&mut reply).unwrap();
            (s, reader, reply)
        };
        let request = b"{\"method\": \"project.get\"}\n";
        // Served one at a time: a second connection is turned away while
        // the first is open, and served once it closes.
        let (first, _, reply) = ask(request);
        assert!(reply.contains("\"ok\""), "{reply}");
        let (_, _, reply) = ask(request);
        assert!(reply.contains("1 connections already"), "{reply}");
        drop(first);
        let mut served = false;
        for _ in 0..200 {
            let (_, _, reply) = ask(request);
            if reply.contains("\"ok\"") {
                served = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(served, "the freed place serves again");
        // A line past the limit: an error, then the connection closes.
        let mut long = vec![b' '; 200];
        long.push(b'\n');
        let mut done = false;
        for _ in 0..200 {
            let (_, mut reader, reply) = ask(&long);
            if reply.contains("connections already") {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            assert!(reply.contains("limited to 100 bytes"), "{reply}");
            let mut rest = String::new();
            assert_eq!(reader.read_line(&mut rest).unwrap(), 0, "closed");
            done = true;
            break;
        }
        assert!(done);
        // The client reports both refusals as what they are.
        let held = loop {
            let (s, _, reply) = ask(request);
            if reply.contains("\"ok\"") {
                break s;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let call = |e: &Envelope| client::call(&socket, e, None, &mut |_| {}, None);
        let get = Envelope::from_json(json!({"method": "project.get"})).unwrap();
        let e = call(&get).unwrap_err();
        assert_eq!(e.code, ErrorCode::Usage, "{e}");
        assert!(e.message.contains("connections already"), "{e}");
        drop(held);
        let long = Envelope::from_json(json!({"method": "media.add", "params": {"paths": ["/a/very/long/path/to/some/media/file/that/makes/the/line/too/long.mp4"]}})).unwrap();
        let mut refused = None;
        for _ in 0..200 {
            match call(&long) {
                Err(e) if e.message.contains("connections already") => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                other => {
                    refused = Some(other);
                    break;
                }
            }
        }
        let e = refused.expect("served").unwrap_err();
        assert_eq!(e.code, ErrorCode::Usage, "{e}");
        assert!(e.message.contains("limited to 100 bytes"), "{e}");
    }

    #[test]
    fn a_refusal_is_the_answer_even_when_the_request_is_cut_short() {
        // A GUI at its connection limit turns a connection away without
        // reading it. A request bigger than the socket buffer is still being
        // written when the GUI closes, so the write fails: the refusal is
        // the answer all the same, not a lost GUI.
        use std::io::{BufRead, BufReader, Write};
        use std::sync::Arc;
        use transport::Stream;
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-ipc-refused-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let socket = dir.join("test.sock");
        let handler = Arc::new(
            |_: Envelope, _: Option<PathBuf>, _: &mut dyn FnMut(Option<Value>) -> bool| {
                Ok(vidcut_core::Response {
                    rev: 1,
                    changes: Default::default(),
                    result: json!("ok"),
                    dry_run: false,
                    via: "gui:1".into(),
                    warnings: Vec::new(),
                })
            },
        );
        let limits = server::Limits {
            line: 64 << 20,
            connections: 1,
        };
        server::start_limited(&socket, handler, limits).unwrap();
        // Hold the one place: served, and still open.
        let mut held = Stream::connect(&socket).unwrap();
        held.write_all(b"{\"method\": \"project.get\"}\n").unwrap();
        let mut reply = String::new();
        BufReader::new(held.try_clone().unwrap())
            .read_line(&mut reply)
            .unwrap();
        assert!(reply.contains("\"ok\""), "{reply}");
        let path = format!("/{}.mp4", "x".repeat(4 << 20));
        let add = Envelope::from_json(json!({"method": "media.add", "params": {"paths": [path]}}))
            .unwrap();
        let e = client::call(&socket, &add, None, &mut |_| {}, None).unwrap_err();
        assert_eq!(e.code, ErrorCode::Usage, "{e}");
        assert!(e.message.contains("connections already"), "{e}");
        drop(held);
    }

    #[test]
    fn a_cancel_already_set_sends_nothing() {
        // The GUI never sees the request, so nothing ran there: CANCELLED is
        // the truth. (Sending it and hanging up could still apply an edit.)
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-ipc-cancel-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let socket = dir.join("test.sock");
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        server::start(
            &socket,
            Arc::new(
                move |_: Envelope, _: Option<PathBuf>, _: &mut dyn FnMut(Option<Value>) -> bool| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    Ok(vidcut_core::Response {
                        rev: 1,
                        changes: Default::default(),
                        result: json!("ok"),
                        dry_run: false,
                        via: "gui:1".into(),
                        warnings: Vec::new(),
                    })
                },
            ),
        )
        .unwrap();
        let split =
            Envelope::from_json(json!({"method": "edit.split", "params": {"at": "1s"}})).unwrap();
        let cancel = AtomicBool::new(true);
        let e = client::call(&socket, &split, None, &mut |_| {}, Some(&cancel)).unwrap_err();
        assert_eq!(e.code, ErrorCode::Cancelled, "{e}");
        // A clear flag is served, and only that request reached the GUI.
        let waiting = AtomicBool::new(false);
        let r = client::call(&socket, &split, None, &mut |_| {}, Some(&waiting)).unwrap();
        assert_eq!(r.result, json!("ok"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the cancelled call sent nothing"
        );
    }

    #[test]
    fn projectless_requests_never_route() {
        let r = Request::from_parts("schema", None).unwrap();
        let opts = RouteOptions {
            instance: Some("999999999".into()),
            ..Default::default()
        };
        assert_eq!(route(None, &opts, &r).unwrap(), Route::Headless);
    }

    #[test]
    fn a_closed_connection_fails_a_progress_probe() {
        use std::io::Write;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{Duration, Instant};

        use transport::Stream;

        let tmp = tempfile::Builder::new()
            .prefix("vidcut-ipc-probe-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let socket = dir.join("t.sock");
        let probes = Arc::new(AtomicU64::new(0));
        let stopped = Arc::new(AtomicU64::new(u64::MAX));
        let (probes_h, stopped_h) = (probes.clone(), stopped.clone());
        server::start(
            &socket,
            Arc::new(move |_envelope, _project, progress| {
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(2) {
                    if !progress(None) {
                        stopped_h.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
                        return Err(Error::new(ErrorCode::Cancelled, "cancelled"));
                    }
                    probes_h.fetch_add(1, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(10));
                }
                stopped_h.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
                Err(Error::internal("the probe never saw the client go"))
            }),
        )
        .unwrap();
        let mut stream = None;
        for _ in 0..50 {
            if let Ok(s) = Stream::connect(&socket) {
                stream = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut stream = stream.expect("connect");
        stream.write_all(b"{\"method\":\"project.get\"}\n").unwrap();
        stream.flush().unwrap();
        let mut live = false;
        for _ in 0..100 {
            if probes.load(Ordering::Relaxed) > 0 {
                live = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(live, "the handler never probed a connected client");
        assert_eq!(
            stopped.load(Ordering::Relaxed),
            u64::MAX,
            "stopped while the client was still connected"
        );
        // Shut down both ways (a socket), and closed (a pipe on Windows is
        // gone once its last handle is).
        stream.shutdown().unwrap();
        drop(stream);
        let mut ms = u64::MAX;
        for _ in 0..100 {
            ms = stopped.load(Ordering::Relaxed);
            if ms != u64::MAX {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ms < 1000, "still running after the client left: {ms} ms");
    }

    /// A pipe (Windows) can't be shut down one way only.
    #[cfg(unix)]
    #[test]
    fn a_client_that_only_stops_writing_still_gets_its_answer() {
        // `printf '...' | socat - UNIX:...` shuts down its writing once the
        // request is sent, and reads on. That is not a client that left.
        use std::io::{BufRead, BufReader, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream as Stream;
        use std::sync::Arc;
        use std::time::Duration;
        let tmp = tempfile::Builder::new()
            .prefix("vidcut-ipc-half-")
            .tempdir()
            .unwrap();
        let dir = tmp.path();
        let socket = dir.join("t.sock");
        server::start(
            &socket,
            Arc::new(
                |_: Envelope,
                 _: Option<PathBuf>,
                 progress: &mut dyn FnMut(Option<Value>) -> bool| {
                    for _ in 0..10 {
                        if !progress(None) {
                            return Err(Error::new(ErrorCode::Cancelled, "cancelled"));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(vidcut_core::Response {
                        rev: 1,
                        changes: Default::default(),
                        result: json!("ok"),
                        dry_run: false,
                        via: "gui:1".into(),
                        warnings: Vec::new(),
                    })
                },
            ),
        )
        .unwrap();
        let mut stream = Stream::connect(&socket).unwrap();
        stream.write_all(b"{\"method\":\"project.get\"}\n").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).unwrap();
        assert!(reply.contains("\"ok\""), "{reply}");
    }
}
