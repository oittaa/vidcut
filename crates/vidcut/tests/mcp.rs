//! An agent drives vidcut over MCP (`vidcut mcp`). Headless
//! it makes a project and looks at a frame; against a live session (the
//! GUI's core, serving IPC) it imports, ripple-deletes the silences longer
//! than 0.7 s, exports and checks a storyboard, while the session shows the
//! edit and undoes it as one step.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};

const BIN: &str = env!("CARGO_BIN_EXE_vidcut");

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, _: PlayerId, done: Presented) {
        done();
    }
    fn capture_viewer(&self, _: PlayerId, done: Capture) {
        done(Err(vidcut_core::Error::usage("no renderer in tests")));
    }
    fn capture_window(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no window in tests")));
    }
    fn window(&self, _: WindowChange) {}
    fn quit(&self) {}
}

/// An MCP client talking to a `vidcut mcp` child process.
struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Value>,
    next: u64,
    /// Notifications that arrived while waiting for answers.
    notes: Vec<Value>,
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
            notes: Vec::new(),
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
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("use_project")
        );
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
            self.notes.push(v);
        }
    }

    /// Calls a tool; returns its result (`content`, `structuredContent`, `isError`).
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.call_with(name, arguments, json!({}))
    }

    fn call_with(&mut self, name: &str, arguments: Value, meta: Value) -> Value {
        let r = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments, "_meta": meta }),
        );
        r["result"].clone()
    }

    /// Calls a tool that must succeed; returns the vidcut response.
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

/// The left channel of a 16-bit stereo WAV.
fn wav_left(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
    bytes[data..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[lo, hi, ..]| f32::from(i16::from_le_bytes([lo, hi])) / 32768.0)
        .collect()
}

/// The image in a tool result, decoded far enough to check it is a PNG.
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
fn an_agent_cuts_silence_over_mcp_live_in_the_gui() {
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
    // Ten seconds of 30 fps with a 10 ms beep at the start of each second.
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        300,
    ))
    .unwrap();

    // 1. Headless: a new project, media, and a look at a frame.
    let mut agent = Client::spawn(&["--headless"]);
    let tools = agent.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "use_project",
        "edit_ripple_delete_silence",
        "storyboard_render",
        "history_undo",
    ] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
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

    // 2. Live: the GUI's session serves IPC; the agent's calls run in it.
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
        players: None,
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
    let r = agent.ok(
        "edit_ripple_delete_silence",
        json!({ "clips": "c1", "min": "0.7s" }),
    );
    // Each second keeps its beep with 0.15 s around it: 12 frames of 30.
    assert_eq!(r["result"]["removed"].as_array().unwrap().len(), 10, "{r}");
    let tl = call(&h, "timeline.get", json!({})).result;
    let clips: Vec<&Value> = tl["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "clip")
        .collect();
    assert_eq!(clips.len(), 10, "the session shows the edit: {tl}");
    assert_eq!(
        tl["duration"],
        300 - r["result"]["frames"].as_i64().unwrap()
    );
    let undo = call(&h, "history.get", json!({})).result["undo"].clone();
    assert_eq!(
        undo.as_array().unwrap().len(),
        undo_before + 1,
        "one undo step"
    );

    // Export with progress, then hear where the beeps went.
    let out = dir.join("cut.wav");
    let exported = agent.call_with(
        "export_start",
        json!({ "output": out, "set": ["audio.codec=pcm_s16"] }),
        json!({ "progressToken": "export-1" }),
    );
    assert_eq!(exported["isError"], false, "{exported}");
    assert!(
        agent
            .notes
            .iter()
            .any(|n| n["method"] == "notifications/progress"
                && n["params"]["progressToken"] == "export-1"),
        "progress was reported"
    );
    let left = wav_left(&out);
    let onsets = vidcut_av::beep::onsets(&left, 0.1, 4800);
    let pieces: Vec<(i64, i64, i64)> = clips
        .iter()
        .map(|c| {
            (
                c["start"].as_i64().unwrap(),
                c["duration"].as_i64().unwrap(),
                c["source_in"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(onsets.len(), 10, "{onsets:?}");
    for (k, &at) in onsets.iter().enumerate() {
        // Second k's beep (source frame 30k) where its piece put it, to 1 ms.
        let (start, _, source_in) = *pieces
            .iter()
            .find(|(_, d, s)| (*s..s + d).contains(&(30 * k as i64)))
            .unwrap_or_else(|| panic!("beep {k} was cut: {pieces:?}"));
        let want = (start + 30 * k as i64 - source_in) * 1600;
        assert!(
            (at as i64 - want).abs() <= 48,
            "beep {k} at {at}, not {want}"
        );
    }

    // The storyboard: one picture per piece, returned as an image.
    let board = agent.call(
        "storyboard_render",
        json!({ "out": dir.join("board.png"), "columns": 5, "width": 160 }),
    );
    assert_eq!(board["isError"], false, "{board}");
    assert!(has_png(&board));
    assert_eq!(
        board["structuredContent"]["result"]["cells"]
            .as_array()
            .unwrap()
            .len(),
        10
    );

    // One undo brings the whole clip back.
    agent.ok("history_undo", json!({}));
    let tl = call(&h, "timeline.get", json!({})).result;
    assert_eq!(tl["duration"], 300);

    // Cancelling a call that runs in the GUI stops it there, and the
    // cancelled request gets no answer (MCP's cancellation). A slow export:
    // 4K with x264's slowest preset.
    let slow = dir.join("slow.mp4");
    let id = agent.next;
    agent.next += 1;
    agent.send(&json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {
            "name": "export_start",
            "arguments": {
                "output": slow,
                "set": ["video.width=3840", "video.height=2160", "video.speed=veryslow",
                        "video.encoder=cpu"],
            },
            "_meta": { "progressToken": "slow" },
        },
    }));
    loop {
        let v = agent
            .lines
            .recv_timeout(Duration::from_secs(30))
            .expect("the slow export reports progress");
        assert_ne!(v["id"], json!(id), "finished before the cancel: {v}");
        if v["params"]["progressToken"] == "slow" {
            break;
        }
    }
    agent.send(&json!({
        "jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": { "requestId": id, "reason": "test" },
    }));
    let started = std::time::Instant::now();
    loop {
        let jobs = call(&h, "jobs.list", json!({})).result;
        let state = jobs.as_array().unwrap().last().unwrap()["state"].clone();
        if state == "cancelled" {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the GUI's export goes on: {jobs:#}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!slow.exists(), "no output left");
    while let Ok(v) = agent.lines.recv_timeout(Duration::from_millis(500)) {
        assert_ne!(v["id"], json!(id), "an answer to a cancelled call: {v}");
    }
    agent.finish();

    h.shutdown();
    let _ = session.join();
    std::thread::sleep(Duration::from_millis(10));
}
