#![forbid(unsafe_code)]

mod diagnostic;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use reuben_core::Error;
use reuben_core::highlight;
use reuben_core::site::{BuildOptions, BuildReport, LoadOptions, Site};

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
            for w in &report.warnings {
                eprintln!("warning: {w}");
            }
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for e in &errors {
                let source = std::fs::read_to_string(&e.path).ok();
                eprint!("{}", diagnostic::render(e, source.as_deref()));
            }
            eprintln!("{}", error_summary(&errors));
            ExitCode::FAILURE
        }
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
