#![forbid(unsafe_code)]

mod diagnostic;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
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
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Build {
            root,
            out,
            drafts,
            env,
        } => build(root, out, drafts, env),
    }
}

fn build(root: PathBuf, out: Option<PathBuf>, drafts: bool, env: Option<String>) -> ExitCode {
    let started = Instant::now();
    let opts = BuildOptions {
        drafts,
        out: out.unwrap_or_else(|| root.join("dist")),
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
            for e in errors {
                let source = std::fs::read_to_string(&e.path).ok();
                eprint!("{}", diagnostic::render(&e, source.as_deref()));
            }
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

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
