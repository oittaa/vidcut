use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use vidcut_core::{Envelope, Error, Response, Result};

use super::transport::{Listener, Stream};

pub type Handler = dyn Fn(Envelope, Option<PathBuf>, &mut dyn FnMut(Option<Value>) -> bool) -> Result<Response>
    + Send
    + Sync;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub line: u64,
    pub connections: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            line: 64 << 20,
            connections: 64,
        }
    }
}

pub fn start(path: &Path, handler: Arc<Handler>) -> std::io::Result<()> {
    start_limited(path, handler, Limits::default())
}

pub fn start_limited(path: &Path, handler: Arc<Handler>, limits: Limits) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        super::private_dir(dir)?;
    }
    let listener = Listener::bind(path)?;
    let open = Arc::new(AtomicUsize::new(0));
    crate::threads::spawn("vidcut-ipc", crate::threads::Work::Interactive, move || {
        loop {
            let Ok(mut stream) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            };
            if open.load(Ordering::SeqCst) >= limits.connections {
                let e = Error::usage(format!(
                    "this vidcut GUI is serving {} connections already; try again later",
                    limits.connections
                ));
                let _ = send(&mut stream, &failure(Value::Null, &e));
                continue;
            }
            let slot = Slot::take(&open);
            let handler = handler.clone();
            let _ = crate::threads::spawn(
                "vidcut-ipc-conn",
                crate::threads::Work::Interactive,
                move || {
                    let _slot = slot;
                    serve(stream, &*handler, limits.line);
                },
            );
        }
    })?;
    Ok(())
}

struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(open: &Arc<AtomicUsize>) -> Self {
        open.fetch_add(1, Ordering::SeqCst);
        Self(open.clone())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn failure(id: Value, e: &Error) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": super::error_json(e) })
}

fn serve(stream: Stream, handler: &Handler, max_line: u64) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    let mut reader = BufReader::new(read_half);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match (&mut reader).take(max_line + 1).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let content = buf.strip_suffix(b"\n").unwrap_or(&buf);
        if content.len() as u64 > max_line {
            let e = Error::usage(format!("a request is limited to {max_line} bytes"));
            let _ = send(&mut writer, &failure(Value::Null, &e));
            return;
        }
        let Ok(line) = std::str::from_utf8(content) else {
            let e = Error::usage("a request must be UTF-8 JSON");
            if send(&mut writer, &failure(Value::Null, &e)).is_err() {
                return;
            }
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let (id, outcome) = answer(line, handler, &mut writer);
        let reply = match outcome {
            Ok(response) => json!({ "jsonrpc": "2.0", "id": id, "result": response }),
            Err(e) => failure(id, &e),
        };
        if send(&mut writer, &reply).is_err() {
            return;
        }
    }
}

fn send(writer: &mut Stream, v: &Value) -> std::io::Result<()> {
    let mut line = v.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

fn answer(line: &str, handler: &Handler, writer: &mut Stream) -> (Value, Result<Response>) {
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
    let mut progress = |params: Option<Value>| {
        if writer.peer_gone() {
            return false;
        }
        let Some(params) = params else {
            return true;
        };
        send(
            writer,
            &json!({ "jsonrpc": "2.0", "method": "progress", "params": params }),
        )
        .is_ok()
    };
    (id, handler(envelope, project, &mut progress))
}
