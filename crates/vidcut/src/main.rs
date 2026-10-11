use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use serde_json::json;
use vidcut::cli::{Action, Cli};
use vidcut::{output, session};
use vidcut_core::store::Store;
use vidcut_core::{Error, Project, Result};

fn main() -> ExitCode {
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
            let response = session::run(cli.project.as_deref(), envelope)?;
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
