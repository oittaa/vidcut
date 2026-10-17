use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};

const BIN: &str = env!("CARGO_BIN_EXE_vidcut");

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, done: Presented) {
        done(None);
    }
    fn capture_viewer(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no renderer in tests")));
    }
    fn capture_window(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no window in tests")));
    }
    fn window(&self, _: WindowChange) {}
    fn quit(&self) {}
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Value>,
    next: u64,
}

impl Client {
    fn spawn(args: &[&str]) -> Self {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line).expect("one JSON message per line");
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut c = Self {
            child,
            stdin,
            lines,
            next: 1,
        };
        let init = c.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "vidcut-test", "version": "0" },
            }),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "{init}");
        c.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        c
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let v = self
                .lines
                .recv_timeout(Duration::from_secs(120))
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let r = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        r["result"].clone()
    }

    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let r = self.call(name, arguments);
        assert_eq!(r["isError"], false, "{name}: {r}");
        r["structuredContent"].clone()
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        assert!(status.success(), "vidcut mcp exited with {status}");
    }
}

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn has_png(result: &Value) -> bool {
    result["content"].as_array().unwrap().iter().any(|c| {
        c["type"] == "image"
            && c["mimeType"] == "image/png"
            && c["data"]
                .as_str()
                .is_some_and(|d| d.starts_with("iVBORw0KGgo"))
    })
}

#[test]
fn an_agent_edits_over_mcp_headless_and_live_in_the_gui() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-mcp-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
        std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
    }
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        300,
    ))
    .unwrap();

    let mut agent = Client::spawn(&["--headless"]);
    let scratch = dir.join("scratch.vidcut");
    agent.ok(
        "use_project",
        json!({ "path": scratch, "create": true, "fps": "30", "size": "320x180" }),
    );
    agent.ok("media_add", json!({ "paths": [media] }));
    agent.ok("edit_append", json!({ "media": "m1" }));
    let frame = agent.call(
        "frame_render",
        json!({ "at": "1s", "out": dir.join("f.png") }),
    );
    assert_eq!(frame["isError"], false, "{frame}");
    assert!(has_png(&frame), "the frame comes back as an image");
    let bad = agent.call("edit_split", json!({ "at": "99s" }));
    assert_eq!(bad["isError"], true);
    assert_eq!(
        bad["structuredContent"]["error"]["code"], "INVALID_EDIT",
        "{bad}"
    );
    agent.finish();

    let path = dir.join("talk.vidcut");
    Store::create(
        &path,
        &Project::new(Settings {
            fps: Rational::new(30, 1).unwrap(),
            width: 320,
            height: 180,
            auto_format: false,
            ..Settings::default()
        }),
    )
    .unwrap();
    let socket = dir.join("run").join("gui.sock");
    let (h, session) = live::start(live::Config {
        project: Some(path.clone()),
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: Some(socket.clone()),
    });
    let ipc = h.clone();
    vidcut::ipc::server::start(
        &socket,
        Arc::new(move |envelope, project, progress| {
            ipc.call(envelope, Origin::Ipc, project, progress)
        }),
    )
    .unwrap();
    let mut agent = Client::spawn(&[
        "-p",
        path.to_str().unwrap(),
        "--instance",
        socket.to_str().unwrap(),
    ]);
    let added = agent.ok("media_add", json!({ "paths": [media] }));
    assert!(
        added["via"].as_str().unwrap().starts_with("gui:"),
        "{added}"
    );
    agent.ok("edit_append", json!({ "media": "m1" }));
    let undo_before = call(&h, "history.get", json!({})).result["undo"]
        .as_array()
        .unwrap()
        .len();
    agent.ok(
        "edit_ripple_delete_silence",
        json!({ "clips": "c1", "min": "0.7s" }),
    );
    let undo = call(&h, "history.get", json!({})).result["undo"].clone();
    assert_eq!(
        undo.as_array().unwrap().len(),
        undo_before + 1,
        "one undo step"
    );
    agent.finish();

    h.shutdown();
    let _ = session.join();
    std::thread::sleep(Duration::from_millis(10));
}
