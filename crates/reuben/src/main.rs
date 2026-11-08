#![forbid(unsafe_code)]

mod diagnostic;

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use reuben_core::Error;
use reuben_core::config::Config;
use reuben_core::highlight;
use reuben_core::output::Output;
use reuben_core::serve::DevServer;
use reuben_core::serve::watch::{Watcher, debounce};
use reuben_core::site::{BuildOptions, BuildReport, LoadOptions, RenderOptions, Site};

/// The tastiest SSG.
#[derive(Parser)]
#[command(name = "reuben", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build the site into the output directory.
    Build {
        /// Site root, containing `content/` and optionally `static/`.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Output directory. Defaults to `<root>/dist`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Include pages marked `draft: true`.
        #[arg(long)]
        drafts: bool,
        /// Merge `reuben.<name>.toml` over `reuben.toml` if it exists.
        #[arg(long)]
        env: Option<String>,
        /// Render with this many threads. Defaults to one per core.
        #[arg(long, value_name = "N")]
        jobs: Option<usize>,
    },
    /// Serve the site from memory, rebuild on change and reload the browser.
    Serve {
        /// Site root, containing `content/` and optionally `static/`.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Port to listen on, on 127.0.0.1 only. `0` picks a free one.
        #[arg(long, default_value_t = 1111)]
        port: u16,
        /// Include pages marked `draft: true`.
        #[arg(long)]
        drafts: bool,
        /// Merge `reuben.<name>.toml` over `reuben.toml` if it exists.
        #[arg(long)]
        env: Option<String>,
        /// Render with this many threads. Defaults to one per core.
        #[arg(long, value_name = "N")]
        jobs: Option<usize>,
    },
    /// Print stylesheets that go with build output.
    #[command(subcommand)]
    Css(CssCommand),
}

#[derive(Subcommand)]
enum CssCommand {
    /// Print CSS for the `hl-` classes used by highlighted code blocks.
    Highlight {
        /// Built-in theme name or path to a `.tmTheme` file.
        #[arg(long, default_value = "base16-ocean.dark")]
        theme: String,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Build {
            root,
            out,
            drafts,
            env,
            jobs,
        } => build(root, out, drafts, env, jobs),
        Command::Serve {
            root,
            port,
            drafts,
            env,
            jobs,
        } => serve(&root, port, drafts, env, jobs),
        Command::Css(CssCommand::Highlight { theme }) => match highlight::theme_css(&theme) {
            Ok(css) => {
                print!("{css}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn build(
    root: PathBuf,
    out: Option<PathBuf>,
    drafts: bool,
    env: Option<String>,
    jobs: Option<usize>,
) -> ExitCode {
    let started = Instant::now();
    let opts = BuildOptions {
        drafts,
        out: out.unwrap_or_else(|| root.join("dist")),
        jobs,
    };
    match Site::load_with(&root, &LoadOptions { env }).and_then(|site| site.build(&opts)) {
        Ok(report) => {
            println!(
                "{} in {}ms",
                summary(&report),
                started.elapsed().as_millis()
            );
            print_warnings(&report);
            ExitCode::SUCCESS
        }
        Err(errors) => {
            print_errors(&errors);
            ExitCode::FAILURE
        }
    }
}

fn print_errors(errors: &[Error]) {
    for e in errors {
        let source = std::fs::read_to_string(&e.path).ok();
        eprint!("{}", diagnostic::render(e, source.as_deref()));
    }
    eprintln!("{}", error_summary(errors));
}

type Rendered = Result<(Output, BuildReport), Vec<Error>>;

/// One full load and render. The config comes back even when the render fails, so the watches can follow it.
fn rebuild(
    root: &Path,
    env: &Option<String>,
    drafts: bool,
    jobs: Option<usize>,
) -> (Option<Config>, Rendered) {
    match Site::load_with(root, &LoadOptions { env: env.clone() }) {
        Ok(site) => {
            let rendered = site.render(&RenderOptions { drafts, jobs });
            (Some(site.config), rendered)
        }
        Err(errors) => (None, Err(errors)),
    }
}

fn serve(
    root: &Path,
    port: u16,
    drafts: bool,
    env: Option<String>,
    jobs: Option<usize>,
) -> ExitCode {
    let (config, first) = rebuild(root, &env, drafts, jobs);
    let output = match first {
        Ok((output, report)) => {
            print_warnings(&report);
            output
        }
        Err(errors) => {
            print_errors(&errors);
            return ExitCode::FAILURE;
        }
    };
    let server = match DevServer::start(&format!("127.0.0.1:{port}"), output) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("error: cannot listen on 127.0.0.1:{port}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let (mut watcher, events) = match Watcher::new(&canonical) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: cannot watch {}: {e}", root.display());
            return ExitCode::FAILURE;
        }
    };
    for problem in watcher.sync(config.as_ref(), env.as_deref()).problems {
        eprintln!("warning: {problem}");
    }
    println!("Serving at http://{}", server.local_addr());

    for () in debounce(events, Duration::from_millis(150), Duration::from_secs(1)) {
        // Adding a watch can drop events for a moment (macOS restarts its event stream), so when the watches
        // change, one more rebuild picks up anything saved meanwhile.
        let mut again = true;
        loop {
            let watches_changed =
                rebuild_and_serve(root, &env, drafts, jobs, &server, &mut watcher);
            if !(watches_changed && again) {
                break;
            }
            again = false;
        }
    }
    ExitCode::SUCCESS
}

/// One rebuild in the serve loop: the watches follow the new config before its output goes live, so an edit
/// made right after a page appears is never missed. Returns whether the watches changed.
fn rebuild_and_serve(
    root: &Path,
    env: &Option<String>,
    drafts: bool,
    jobs: Option<usize>,
    server: &DevServer,
    watcher: &mut Watcher,
) -> bool {
    let started = Instant::now();
    // A panic in a rebuild is a failed rebuild, not the end of the server.
    let (config, result) =
        std::panic::catch_unwind(AssertUnwindSafe(|| rebuild(root, env, drafts, jobs)))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                eprintln!("error: rebuild panicked: {message}");
                (None, Err(Vec::new()))
            });
    let synced = watcher.sync(config.as_ref(), env.as_deref());
    for problem in &synced.problems {
        eprintln!("warning: {problem}");
    }
    match result {
        Ok((output, report)) => {
            server.replace(output);
            print_warnings(&report);
            println!(
                "Rebuilt {} in {} ms",
                plural(report.pages_written, "page", "pages"),
                started.elapsed().as_millis()
            );
        }
        Err(errors) => {
            // Empty only after a panic, which has already been reported.
            if !errors.is_empty() {
                print_errors(&errors);
            }
            server.notify_error();
        }
    }
    synced.changed
}

fn print_warnings(report: &BuildReport) {
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
}

fn summary(r: &BuildReport) -> String {
    let mut s = format!(
        "Built {}, copied {}",
        plural(r.pages_written, "page", "pages"),
        plural(r.static_copied, "static file", "static files"),
    );
    if r.drafts_skipped > 0 {
        s += &format!(" ({} skipped)", plural(r.drafts_skipped, "draft", "drafts"));
    }
    s
}

fn error_summary(errors: &[Error]) -> String {
    let files: HashSet<&Path> = errors.iter().map(|e| e.path.as_path()).collect();
    format!(
        "Found {} in {}",
        plural(errors.len(), "error", "errors"),
        plural(files.len(), "file", "files"),
    )
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
