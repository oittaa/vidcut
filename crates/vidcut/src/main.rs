use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{CommandFactory, Parser};
use serde_json::json;
use vidcut::cli::{Action, Cli};
use vidcut::session::Hooks;
use vidcut::{output, session};
use vidcut_core::store::Store;
use vidcut_core::{Error, Project, Result};

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
    let Some(action) = cli.action()? else {
        // The GUI arrives in M3; until then, show what the CLI can do.
        Cli::command()
            .print_help()
            .map_err(|e| Error::internal(e.to_string()))?;
        println!(
            "\n(The GUI is not built yet; every feature is available through these commands.)"
        );
        return Ok(());
    };
    match action {
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
        }
        Action::Run(envelope) => {
            let method = envelope.request.method();
            static CANCEL: AtomicBool = AtomicBool::new(false);
            if method == "export.start" {
                // First Ctrl-C cancels the export cleanly (no partial files).
                let _ = ctrlc::set_handler(|| CANCEL.store(true, Ordering::Relaxed));
            }
            let show = std::io::stderr().is_terminal() && !cli.json;
            let mut last = std::time::Instant::now();
            let mut progress =
                |i: usize, n: usize, path: &std::path::Path, p: vidcut_av::export::Progress| {
                    if !show || (p.frames < p.total && last.elapsed().as_millis() < 200) {
                        return;
                    }
                    last = std::time::Instant::now();
                    let pct = p.frames * 100 / p.total.max(1);
                    let rate = p.frames as f64 / p.elapsed.as_secs_f64().max(1e-3);
                    let eta = (p.total - p.frames) as f64 / rate.max(1e-3);
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
                        "\r{which}{file}: {pct:3}% ({}/{} frames, {rate:.0} fps, ETA {}:{:02})   ",
                        p.frames,
                        p.total,
                        eta as u64 / 60,
                        eta as u64 % 60
                    );
                    if p.frames == p.total {
                        eprintln!();
                    }
                };
            let response = session::run_with(
                cli.project.as_deref(),
                envelope,
                &mut Hooks {
                    cancel: &CANCEL,
                    progress: &mut progress,
                },
            )?;
            output::print(&method, &response, cli.json);
        }
        Action::ShowMarks(envelope) => {
            let r = session::run(cli.project.as_deref(), envelope)?;
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
        }
        Action::ListMarkers(envelope) => {
            let r = session::run(cli.project.as_deref(), envelope)?;
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
        }
    }
    Ok(())
}
