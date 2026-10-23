use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut_core::model::Settings;
use vidcut_core::request::{Client, ClientKind};
use vidcut_core::store::Store;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Error, Project, Request, Result};

use crate::ipc::{self, RouteOptions};

const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const HIDDEN: &[&str] = &["schema"];

const PICTURES: &[&str] = &["frame.render", "gui.screenshot", "storyboard.render"];

const MAX_PICTURE: u64 = 8 << 20;

const INSTRUCTIONS: &str = "vidcut edits a video timeline: a media pool, a magnetic Track 1, B-roll \
tracks above it and audio tracks below; these tools are the same requests the vidcut CLI and GUI make. Choose the project with use_project \
(create: true makes a new one). Times are strings with a unit: \"450f\" (frames), \"15s\", \"1m30s\", \
\"00:01:00:00\"; a bare number is a frame. Ranges: \"10s..20s\" or \"1m..+5s\". Clips: their id \
(\"c17\") or \"V1@1m\" (the clip on V1 at 1 minute). Each change is one undo step (history_undo), \
under your name in the person's History window; history_get lists the steps with who made them, \
and history_go goes back to any of them. Changes take dry_run: true to preview. Look at your edit with frame_render or storyboard_render, \
which return images. If a vidcut GUI has the project open, calls run inside it and the person at it \
sees each edit; viewer_*, player_*, selection_*, gui_*, jobs_* and pool_set need that GUI.";

#[derive(Clone)]
struct Out(Arc<Mutex<std::io::Stdout>>);

impl Out {
    fn send(&self, message: &Value) {
        let mut out = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    }

    fn result(&self, id: &Value, result: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn error(&self, id: &Value, code: i64, message: &str) {
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message },
        }));
    }
}

pub fn serve(project: Option<PathBuf>, options: RouteOptions) -> Result<()> {
    let tools = tools()?;
    let methods: HashMap<String, String> = tools
        .iter()
        .filter_map(|t| {
            let name = t["name"].as_str()?;
            Some((name.to_string(), t["_method"].as_str()?.to_string()))
        })
        .collect();
    let listed: Vec<Value> = tools
        .into_iter()
        .map(|mut t| {
            if let Some(o) = t.as_object_mut() {
                o.remove("_method");
            }
            t
        })
        .collect();
    let out = Out(Arc::new(Mutex::new(std::io::stdout())));
    let state = Arc::new(State {
        project: Mutex::new(project.map(absolute)),
        options,
        running: Mutex::new(HashMap::new()),
        methods,
        client: Mutex::new(None),
    });
    let mut calls = Vec::new();
    let max = crate::ipc::server::Limits::default().line;
    let mut input = std::io::stdin().lock();
    loop {
        let line = match crate::ipc::read_capped_line(&mut input, max) {
            Ok(None) => break,
            Ok(Some(line)) => line,
            Err(e) => {
                out.error(&Value::Null, -32700, &format!("parse error: {e}"));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                out.error(&Value::Null, -32700, &format!("parse error: {e}"));
                continue;
            }
        };
        let method = message["method"].as_str().unwrap_or_default();
        let Some(id) = message.get("id").cloned() else {
            if method == "notifications/cancelled"
                && let Some(flag) = state
                    .lock_running()
                    .get(&message["params"]["requestId"].to_string())
            {
                flag.store(true, Ordering::Relaxed);
            }
            continue;
        };
        match method {
            "initialize" => {
                *state.client.lock().unwrap_or_else(PoisonError::into_inner) =
                    message["params"]["clientInfo"]["name"]
                        .as_str()
                        .map(String::from);
                let asked = message["params"]["protocolVersion"]
                    .as_str()
                    .unwrap_or_default();
                let version = VERSIONS
                    .iter()
                    .find(|v| **v == asked)
                    .unwrap_or(&VERSIONS[0]);
                out.result(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": {
                            "name": "vidcut",
                            "title": "vidcut video editor",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "instructions": INSTRUCTIONS,
                    }),
                );
            }
            "ping" => out.result(&id, json!({})),
            "tools/list" => out.result(&id, json!({ "tools": listed })),
            "tools/call" => {
                let (out, state) = (out.clone(), state.clone());
                let params = message["params"].clone();
                let cancel = Arc::new(AtomicBool::new(false));
                state.lock_running().insert(id.to_string(), cancel.clone());
                let call = crate::threads::spawn(
                    "vidcut-mcp-call",
                    crate::threads::Work::Interactive,
                    move || {
                        let result = state.call(&params, &cancel, &out);
                        state.lock_running().remove(&id.to_string());
                        if !cancel.load(Ordering::Relaxed) {
                            out.result(&id, result);
                        }
                    },
                );
                calls.push(call.expect("starting a thread"));
                calls.retain(|c| !c.is_finished());
            }
            _ => out.error(&id, -32601, &format!("method not found: {method}")),
        }
    }
    for c in calls {
        let _ = c.join();
    }
    Ok(())
}

struct State {
    project: Mutex<Option<PathBuf>>,
    options: RouteOptions,
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
    methods: HashMap<String, String>,
    client: Mutex<Option<String>>,
}

impl State {
    fn lock_running(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn project(&self) -> Option<PathBuf> {
        self.project
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn call(&self, params: &Value, cancel: &Arc<AtomicBool>, out: &Out) -> Value {
        let name = params["name"].as_str().unwrap_or_default();
        let args = match params.get("arguments") {
            Some(Value::Null) | None => json!({}),
            Some(a) => a.clone(),
        };
        if name == "use_project" {
            return match self.use_project(&args) {
                Ok(v) => success(&v, None),
                Err(e) => failure(&e),
            };
        }
        let Some(method) = self.methods.get(name) else {
            return failure(&Error::not_found(format!("no tool {name:?}")));
        };
        let token = params["_meta"]["progressToken"].clone();
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut progress = |v: &Value| {
            if token.is_null()
                || (last.elapsed() < Duration::from_millis(250) && v["frames"] != v["total"])
            {
                return;
            }
            last = Instant::now();
            let file = Path::new(v["path"].as_str().unwrap_or_default())
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.send(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {
                    "progressToken": token,
                    "progress": v["frames"],
                    "total": v["total"],
                    "message": match v["stage"].as_str() {
                        Some(stage) => format!("{file}: {stage}"),
                        None => format!(
                            "{file} ({}/{})",
                            v["output"].as_u64().unwrap_or(0) + 1,
                            v["outputs"].as_u64().unwrap_or(1)
                        ),
                    },
                },
            }));
        };
        let mut envelope = match Envelope::from_json(json!({
            "method": method,
            "params": args,
        })) {
            Ok(e) => e,
            Err(e) => return failure(&Error::usage(e)),
        };
        absolute_paths(&mut envelope.request);
        envelope.client = Some(Client::new(
            ClientKind::Mcp,
            self.client
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        ));
        let project = self.project();
        match ipc::run(
            project.as_deref(),
            &self.options,
            envelope,
            cancel,
            &mut progress,
        ) {
            Ok(r) => {
                let response = serde_json::to_value(&r).unwrap_or_default();
                let picture = PICTURES
                    .contains(&method.as_str())
                    .then(|| r.result["path"].as_str().map(PathBuf::from))
                    .flatten();
                success(&response, picture.as_deref())
            }
            Err(e) => failure(&e),
        }
    }

    fn use_project(&self, args: &Value) -> Result<Value> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| Error::usage("use_project needs `path`"))?;
        let path = absolute(PathBuf::from(path));
        let create = args["create"].as_bool().unwrap_or(false);
        if create {
            let mut settings = Settings::default();
            if let Some(fps) = args["fps"].as_str() {
                settings.fps = fps
                    .parse::<Rational>()
                    .map_err(|e| Error::usage(e.to_string()))?;
                settings.auto_format = false;
            }
            if let Some(size) = args["size"].as_str() {
                let (w, h) = size
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .ok_or_else(|| Error::usage(format!("size {size:?}: expected WIDTHxHEIGHT")))?;
                (settings.width, settings.height) = (w, h);
                settings.auto_format = false;
            }
            Store::create(&path, &Project::new(settings))?;
        } else {
            Store::open(&path)?;
        }
        *self.project.lock().unwrap_or_else(PoisonError::into_inner) = Some(path.clone());
        Ok(json!({ "result": { "project": path, "created": create } }))
    }
}

fn success(response: &Value, picture: Option<&Path>) -> Value {
    let mut content = vec![json!({
        "type": "text",
        "text": serde_json::to_string_pretty(response).unwrap_or_default(),
    })];
    if let Some(path) = picture
        && std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_PICTURE)
        && let Ok(bytes) = std::fs::read(path)
    {
        content.push(json!({ "type": "image", "data": base64(&bytes), "mimeType": "image/png" }));
    }
    json!({ "content": content, "structuredContent": response, "isError": false })
}

fn failure(e: &Error) -> Value {
    let error = serde_json::to_value(e).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": format!("{}: {}", error["code"].as_str().unwrap_or("ERROR"), e.message) }],
        "structuredContent": { "error": error },
        "isError": true,
    })
}

pub fn tools() -> Result<Vec<Value>> {
    let root = crate::session::schema(None)?;
    let defs = root["$defs"].as_object().cloned().unwrap_or_default();
    let mut tools = vec![json!({
        "name": "use_project",
        "_method": "use_project",
        "title": "Use a project",
        "description": "Work on this vidcut project file from now on; create: true makes a new one \
            (fps like \"25\" or \"30000/1001\" and size like \"1920x1080\" fix its format; by \
            default the first video added sets them).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The .vidcut file." },
                "create": { "type": "boolean", "default": false },
                "fps": { "type": "string" },
                "size": { "type": "string" },
            },
            "required": ["path"],
        },
    })];
    for variant in root["oneOf"].as_array().into_iter().flatten() {
        let Some(&method) = vidcut_core::request::method_names(variant).first() else {
            continue;
        };
        if HIDDEN.contains(&method) {
            continue;
        }
        let mut schema = variant["properties"]
            .get("params")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object" }));
        if let Some(name) = schema["$ref"]
            .as_str()
            .and_then(|r| r.strip_prefix("#/$defs/"))
            && let Some(def) = defs.get(name)
        {
            schema = def.clone();
        }
        let mut used = Vec::new();
        refs(&schema, &defs, &mut used);
        if !used.is_empty() {
            schema["$defs"] = used
                .into_iter()
                .filter_map(|n| defs.get(&n).map(|d| (n, d.clone())))
                .collect::<serde_json::Map<_, _>>()
                .into();
        }
        if schema.get("type").is_none() {
            schema["type"] = json!("object");
        }
        tools.push(json!({
            "name": method.replace('.', "_"),
            "_method": method,
            "description": variant["description"].as_str().unwrap_or(method),
            "inputSchema": schema,
        }));
    }
    Ok(tools)
}

fn refs(v: &Value, defs: &serde_json::Map<String, Value>, used: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            if let Some(name) = o
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/$defs/"))
                && !used.iter().any(|u| u == name)
            {
                used.push(name.to_string());
                if let Some(def) = defs.get(name) {
                    refs(def, defs, used);
                }
            }
            for x in o.values() {
                refs(x, defs, used);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| refs(x, defs, used)),
        _ => {}
    }
}

fn absolute_paths(request: &mut Request) {
    for p in request.paths_mut() {
        *p = absolute(std::mem::take(p));
    }
}

fn absolute(p: PathBuf) -> PathBuf {
    std::path::absolute(&p).unwrap_or(p)
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
