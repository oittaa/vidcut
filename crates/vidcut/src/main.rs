use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::cli::{Action, Cli};
use vidcut::ipc::{self, Route, RouteOptions};
use vidcut::output;
use vidcut_core::{Envelope, Error, Response, Result};

fn main() -> ExitCode {
    if std::env::var_os("SVT_LOG").is_none() {
        // SAFETY: single-threaded at this point.
        unsafe { std::env::set_var("SVT_LOG", "2") };
    }
    if cfg!(target_os = "linux") && std::env::var_os("LIBVA_MESSAGING_LEVEL").is_none() {
        // SAFETY: single-threaded at this point.
        unsafe { std::env::set_var("LIBVA_MESSAGING_LEVEL", "1") };
    }
    let cli = Cli::parse();
    vidcut_av::cuda::keep_shaders_in(vidcut::paths::cache_dir().join("shaders"));
    vidcut_av::light::cache_in(vidcut::paths::cache_dir().join("analysis"));
    let ran = run(&cli);
    vidcut_av::ending::join();
    match ran {
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
        Action::Gui { project } => {
            leave_own_console();
            vidcut::gui::run(project)
        }
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
        Action::Run(envelope) => {
            let method = envelope.request.method();
            let response = execute(cli, *envelope)?;
            output::print(&method, &response, cli.json);
            Ok(())
        }
    }
}

fn execute(cli: &Cli, envelope: Envelope) -> Result<Response> {
    let options = RouteOptions {
        headless: cli.headless,
        instance: cli.instance.clone(),
    };
    let show = std::io::stderr().is_terminal() && !cli.json;
    let mut bar = ProgressBar::new(show);
    static CANCEL: LazyLock<Arc<AtomicBool>> = LazyLock::new(Arc::default);
    if matches!(
        envelope.request.method().as_str(),
        "export.start" | "proxy.generate"
    ) {
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
        &mut |v: &Value| bar.show(v),
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

    #[allow(clippy::too_many_arguments)]
    fn show(&mut self, v: &Value) {
        let i = v["output"].as_u64().unwrap_or(0) as usize;
        let n = v["outputs"].as_u64().unwrap_or(1) as usize;
        let path = Path::new(v["path"].as_str().unwrap_or_default());
        let frames = v["frames"].as_i64().unwrap_or(0);
        let total = v["total"].as_i64().unwrap_or(0);
        let pass = v["pass"].as_u64().unwrap_or(1) as u8;
        let passes = v["passes"].as_u64().unwrap_or(1) as u8;
        let elapsed = v["elapsed"].as_f64().unwrap_or(0.0);
        if !self.show || (frames < total && self.last.elapsed().as_millis() < 200) {
            return;
        }
        self.last = Instant::now();
        if v["queued"].as_bool() == Some(true) {
            let title = v["title"].as_str().unwrap_or_default();
            eprintln!("{title}: waiting for the jobs before it");
            return;
        }
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        if v["measuring"].as_bool() == Some(true) {
            eprint!(
                "\r{file}: measuring light for the HDR metadata, {:3}%   ",
                frames * 100 / total.max(1)
            );
            if frames == total {
                eprintln!();
            }
            return;
        }
        let passes = i64::from(passes.max(1));
        let done = i64::from(pass.max(1) - 1) * total + frames;
        let pct = done * 100 / (total * passes).max(1);
        let rate = done as f64 / elapsed.max(1e-3);
        let eta = (total * passes - done) as f64 / rate.max(1e-3);
        let pass = if passes > 1 {
            format!("pass {pass}/{passes}, ")
        } else {
            String::new()
        };
        let which = if n > 1 {
            format!("[{}/{n}] ", i + 1)
        } else {
            String::new()
        };
        eprint!(
            "\r{which}{file}: {pct:3}% ({pass}{frames}/{total} frames, {rate:.0} fps, ETA {}:{:02})   ",
            eta as u64 / 60,
            eta as u64 % 60
        );
        if frames == total {
            eprintln!();
        }
    }
}

fn leave_own_console() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
        let mut processes = [0u32; 2];
        // SAFETY: fills at most the two ids it is given room for.
        if unsafe { GetConsoleProcessList(processes.as_mut_ptr(), 2) } == 1 {
            // SAFETY: detaches this process from its console.
            unsafe { FreeConsole() };
        }
    }
}
