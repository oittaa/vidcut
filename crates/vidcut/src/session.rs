//! Runs requests: headless against a project file (M1), live in the GUI (M3).

use std::path::Path;

use serde_json::{Value, json};
use vidcut_core::store::{Store, state_dir};
use vidcut_core::{Envelope, Error, Request, Response, Result};

use crate::env::AvEnv;

/// Runs one request headless. Project-less requests (`schema`, `doctor`,
/// `probe`) ignore `project`.
pub fn run(project: Option<&Path>, envelope: Envelope) -> Result<Response> {
    let mut response = match &envelope.request {
        Request::Schema { method } => answer(schema(method.as_deref())?),
        Request::Doctor {} => answer(doctor()),
        Request::Probe { .. } => {
            // Probing needs no project; an empty editor answers it.
            let mut editor = vidcut_core::Editor::new(
                vidcut_core::Project::new(Default::default()),
                Default::default(),
            );
            editor.apply(envelope, &AvEnv, None)?
        }
        _ => {
            let path = project
                .ok_or_else(|| Error::usage("no project: pass -p FILE or set VIDCUT_PROJECT"))?;
            let store = Store::open(path)?;
            if envelope.request.is_mutation() {
                store.transact(|editor| editor.apply(envelope, &AvEnv, None))?
            } else {
                store.load()?.apply(envelope, &AvEnv, None)?
            }
        }
    };
    response.via = "headless".into();
    Ok(response)
}

fn answer(result: Value) -> Response {
    Response {
        rev: 0,
        changes: Default::default(),
        result,
        dry_run: false,
        via: String::new(),
    }
}

/// JSON Schema of the whole protocol, or of one method's params.
pub fn schema(method: Option<&str>) -> Result<Value> {
    let root = serde_json::to_value(schemars::schema_for!(Request))
        .map_err(|e| Error::internal(e.to_string()))?;
    let Some(method) = method else {
        return Ok(root);
    };
    let variants = root
        .get("oneOf")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let names = |v: &Value| -> Vec<String> {
        let m = &v["properties"]["method"];
        m.get("const")
            .and_then(Value::as_str)
            .map(|s| vec![s.to_string()])
            .or_else(|| {
                m.get("enum").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default()
    };
    let variant = variants
        .iter()
        .find(|v| names(v).iter().any(|n| n == method))
        .ok_or_else(|| Error::not_found(format!("unknown method {method:?}")))?;
    Ok(json!({
        "method": method,
        "description": variant.get("description"),
        "params": variant["properties"].get("params"),
        "$defs": root.get("$defs"),
    }))
}

/// Dependencies and environment, for humans and agents debugging setups.
pub fn doctor() -> Value {
    let gpus: Vec<Value> = std::fs::read_dir("/sys/class/drm")
        .map(|dir| {
            let mut nodes: Vec<Value> = dir
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
                .map(|e| {
                    let driver = std::fs::read_link(e.path().join("device/driver"))
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
                    json!({ "node": format!("/dev/dri/{}", e.file_name().to_string_lossy()), "driver": driver })
                })
                .collect();
            nodes.sort_by_key(|v| v["node"].as_str().unwrap_or_default().to_string());
            nodes
        })
        .unwrap_or_default();
    json!({
        "vidcut": env!("CARGO_PKG_VERSION"),
        "ffmpeg": vidcut_av::library_versions(),
        "libmpv": match vidcut_mpv::available() { Ok(()) => json!("ok"), Err(e) => json!(e) },
        "gpus": gpus,
        "state_dir": state_dir(),
        "test_data": vidcut_av::testdata::root(),
    })
}
