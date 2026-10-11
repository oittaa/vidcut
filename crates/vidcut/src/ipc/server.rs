//! The GUI side of the socket: one thread accepts connections, one thread per
//! connection reads requests line by line and answers them in order.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use vidcut_core::{Envelope, Error, Response, Result};

/// Answers one request. `project` is the project the client expects to be
/// open. `progress` sends a notification to the client and returns false once
/// the client is gone, so long requests can cancel.
pub type Handler = dyn Fn(Envelope, Option<PathBuf>, &mut dyn FnMut(Value) -> bool) -> Result<Response>
    + Send
    + Sync;

/// Binds `path` (replacing a stale socket file) and serves `handler` on it
/// from background threads until the process exits.
pub fn start(path: &Path, handler: Arc<Handler>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        super::private_dir(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::thread::Builder::new()
        .name("vidcut-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let handler = handler.clone();
                let _ = std::thread::Builder::new()
                    .name("vidcut-ipc-conn".into())
                    .spawn(move || serve(stream, &*handler));
            }
        })?;
    Ok(())
}

fn serve(stream: UnixStream, handler: &Handler) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let (id, outcome) = answer(&line, handler, &mut writer);
        let reply = match outcome {
            Ok(response) => json!({ "jsonrpc": "2.0", "id": id, "result": response }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": super::error_json(&e) }),
        };
        if send(&mut writer, &reply).is_err() {
            return;
        }
    }
}

fn send(writer: &mut UnixStream, v: &Value) -> std::io::Result<()> {
    let mut line = v.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

fn answer(line: &str, handler: &Handler, writer: &mut UnixStream) -> (Value, Result<Response>) {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return (Value::Null, Err(Error::usage(format!("not JSON: {e}")))),
    };
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let project = v.get("project").and_then(Value::as_str).map(PathBuf::from);
    let mut request = json!({ "method": v.get("method").cloned().unwrap_or(Value::Null) });
    if let Some(p) = v.get("params") {
        request["params"] = p.clone();
    }
    let envelope = match Envelope::from_json(request) {
        Ok(e) => e,
        Err(e) => return (id, Err(Error::usage(e))),
    };
    let mut progress = |params: Value| {
        send(
            writer,
            &json!({ "jsonrpc": "2.0", "method": "progress", "params": params }),
        )
        .is_ok()
    };
    (id, handler(envelope, project, &mut progress))
}
