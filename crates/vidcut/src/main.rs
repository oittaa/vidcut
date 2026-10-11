use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use clap::Parser;
use serde_json::{Value, json};
use vidcut::cli::{Action, Cli};
use vidcut::ipc::{self, Route, RouteOptions};
use vidcut::output;
use vidcut_core::store::Store;
use vidcut_core::{Envelope, Error, Project, Response, Result};

fn main() -> ExitCode {
    // SVT-AV1 logs at info level to stderr unless told otherwise. Set it here,
    // before any threads exist, where changing the environment is sound.
    if std::env::var_os("SVT_LOG").is_none() {
        // SAFETY: single-threaded at this point.
        unsafe { std::env::set_var("SVT_LOG", "2") };
    }
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if cli.json {
                println!("{}", json!({ "error": e }));
            } else {
                eprintln!("error: {}", e.message);
            }
            ExitCode::from(e.code.exit_code() as u8)
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    let action = match cli.action()? {
        Some(a) => a,
        None => Action::Gui {
            project: cli.project.clone(),
        },
    };
    match action {
        Action::Gui { project } => vidcut::gui::run(project),
        Action::Mcp => vidcut::mcp::serve(
            cli.project.clone(),
            RouteOptions {
                headless: cli.headless,
                instance: cli.instance.clone(),
            },
        ),
        Action::Instances => {
            let list = ipc::registry::list();
            if cli.json {
                println!("{}", json!({ "result": list }));
            } else if list.is_empty() {
                println!("no vidcut GUI is running");
            } else {
                for i in list {
                    println!(
                        "{:<8} {}  {}",
                        i.pid,
                        i.project
                            .as_deref()
                            .map_or("(no project)".into(), |p| p.display().to_string()),
                        i.socket.display()
                    );
                }
            }
            Ok(())
        }
        Action::New { file, settings } => {
            let store = Store::create(&file, &Project::new(settings.clone()))?;
            if cli.json {
                println!(
                    "{}",
                    json!({ "result": { "project": store.path(), "settings": settings } })
                );
            } else {
                println!("created {}", store.path().display());
            }
            Ok(())
        }
        Action::Run(envelope) => {
            let method = envelope.request.method();
            let response = execute(cli, envelope)?;
            output::print(&method, &response, cli.json);
            Ok(())
        }
        Action::ShowMarks(envelope) => {
            let r = execute(cli, envelope)?;
            let marks = &r.result["marks"];
            if cli.json {
                println!("{}", json!({ "rev": r.rev, "result": marks }));
            } else {
                println!(
                    "in {}  out {}",
                    marks.get("in").unwrap_or(&json!("-")),
                    marks.get("out").unwrap_or(&json!("-"))
                );
            }
            Ok(())
        }
        Action::ListTracks(envelope) => {
            let r = execute(cli, envelope)?;
            let tracks = &r.result["tracks"];
            if cli.json {
                println!("{}", json!({ "rev": r.rev, "result": tracks }));
            } else {
                for t in tracks.as_array().into_iter().flatten() {
                    let clips = t["items"]
                        .as_array()
                        .map_or(0, |i| i.iter().filter(|i| i["type"] == "clip").count());
                    let mut flags = Vec::new();
                    for (key, label) in [("locked", "locked"), ("muted", "muted"), ("solo", "solo")]
                    {
                        if t[key] == true {
                            flags.push(label.to_string());
                        }
                    }
                    if t["video_enabled"] == false {
                        flags.push("video off".into());
                    }
                    if let Some(v) = t["volume_db"].as_f64() {
                        flags.push(format!("{v:+} dB"));
                    }
                    println!(
                        "{:<4} {:>4} clip(s)  {}{}",
                        t["id"].as_str().unwrap_or_default(),
                        clips,
                        t["name"]
                            .as_str()
                            .map(|n| format!("{n:?}  "))
                            .unwrap_or_default(),
                        flags.join(", ")
                    );
                }
            }
            Ok(())
        }
        Action::ListBins(envelope) => {
            let r = execute(cli, envelope)?;
            let bins = &r.result["bins"];
            if cli.json {
                println!("{}", json!({ "rev": r.rev, "result": bins }));
            } else {
                for b in bins.as_array().into_iter().flatten() {
                    println!("{}", b.as_str().unwrap_or_default());
                }
            }
            Ok(())
        }
        Action::ListMarkers(envelope) => {
            let r = execute(cli, envelope)?;
            let markers = &r.result["markers"];
            if cli.json {
                println!("{}", json!({ "rev": r.rev, "result": markers }));
            } else {
                for k in markers.as_array().into_iter().flatten() {
                    println!(
                        "{} at {} {} {}",
                        k["id"].as_str().unwrap_or_default(),
                        k["at"],
                        k["color"].as_str().unwrap_or_default(),
                        k["name"].as_str().unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
    }
}

/// Runs one request in the GUI that has the project open, or headless.
fn execute(cli: &Cli, envelope: Envelope) -> Result<Response> {
    let options = RouteOptions {
        headless: cli.headless,
        instance: cli.instance.clone(),
    };
    let show = std::io::stderr().is_terminal() && !cli.json;
    let mut bar = ProgressBar::new(show);
    static CANCEL: LazyLock<Arc<AtomicBool>> = LazyLock::new(Arc::default);
    if envelope.request.method() == "export.start" {
        // First Ctrl-C cancels a headless export cleanly (no partial files);
        // a GUI's export stops when this process ends.
        let _ = ctrlc::set_handler(|| CANCEL.store(true, Ordering::Relaxed));
    }
    let live = matches!(
        ipc::route(cli.project.as_deref(), &options, &envelope.request),
        Ok(Route::Instance(_))
    );
    ipc::run(
        cli.project.as_deref(),
        &options,
        envelope,
        &CANCEL,
        &mut |v: &Value| {
            bar.show(
                v["output"].as_u64().unwrap_or(0) as usize,
                v["outputs"].as_u64().unwrap_or(1) as usize,
                Path::new(v["path"].as_str().unwrap_or_default()),
                v["frames"].as_i64().unwrap_or(0),
                v["total"].as_i64().unwrap_or(0),
                v["elapsed"].as_f64().unwrap_or(0.0),
            )
        },
    )
    .map_err(|e| {
        if live && e.code == vidcut_core::ErrorCode::NoInstance && !cli.json {
            Error::new(
                e.code,
                format!("{} (use --headless to run without the GUI)", e.message),
            )
        } else {
            e
        }
    })
}

/// Export progress on a terminal.
struct ProgressBar {
    show: bool,
    last: Instant,
}

impl ProgressBar {
    fn new(show: bool) -> Self {
        Self {
            show,
            last: Instant::now() - Duration::from_secs(1),
        }
    }

    fn show(&mut self, i: usize, n: usize, path: &Path, frames: i64, total: i64, elapsed: f64) {
        if !self.show || (frames < total && self.last.elapsed().as_millis() < 200) {
            return;
        }
        self.last = Instant::now();
        let pct = frames * 100 / total.max(1);
        let rate = frames as f64 / elapsed.max(1e-3);
        let eta = (total - frames) as f64 / rate.max(1e-3);
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let which = if n > 1 {
            format!("[{}/{n}] ", i + 1)
        } else {
            String::new()
        };
        eprint!(
            "\r{which}{file}: {pct:3}% ({frames}/{total} frames, {rate:.0} fps, ETA {}:{:02})   ",
            eta as u64 / 60,
            eta as u64 % 60
        );
        if frames == total {
            eprintln!();
        }
    }
}
