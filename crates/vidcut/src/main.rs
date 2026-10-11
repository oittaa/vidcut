use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use serde_json::{Value, json};
use vidcut::cli::{Action, Cli};
use vidcut::ipc::{self, Route, RouteOptions};
use vidcut::session::Hooks;
use vidcut::{output, session};
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
    let route = ipc::route(cli.project.as_deref(), &options, &envelope.request)?;
    let show = std::io::stderr().is_terminal() && !cli.json;
    let mut bar = ProgressBar::new(show);
    match route {
        Route::Headless => {
            static CANCEL: AtomicBool = AtomicBool::new(false);
            if envelope.request.method() == "export.start" {
                // First Ctrl-C cancels the export cleanly (no partial files).
                let _ = ctrlc::set_handler(|| CANCEL.store(true, Ordering::Relaxed));
            }
            session::run_with(
                cli.project.as_deref(),
                envelope,
                &mut Hooks {
                    cancel: &CANCEL,
                    progress: &mut |i, n, path, p| {
                        bar.show(i, n, path, p.frames, p.total, p.elapsed.as_secs_f64())
                    },
                },
            )
        }
        Route::Instance(instance) => {
            // Ctrl-C ends this process; the GUI notices the closed connection
            // and cancels the export.
            let project = cli.project.as_deref().and_then(|p| p.canonicalize().ok());
            let mut progress = |v: &Value| {
                bar.show(
                    v["output"].as_u64().unwrap_or(0) as usize,
                    v["outputs"].as_u64().unwrap_or(1) as usize,
                    Path::new(v["path"].as_str().unwrap_or_default()),
                    v["frames"].as_i64().unwrap_or(0),
                    v["total"].as_i64().unwrap_or(0),
                    v["elapsed"].as_f64().unwrap_or(0.0),
                )
            };
            ipc::client::call(
                &instance.socket,
                &envelope,
                project.as_deref(),
                &mut progress,
            )
            .map_err(|e| {
                if e.code == vidcut_core::ErrorCode::NoInstance && !cli.json {
                    Error::new(
                        e.code,
                        format!("{} (use --headless to run without the GUI)", e.message),
                    )
                } else {
                    e
                }
            })
        }
    }
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
