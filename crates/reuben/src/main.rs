#![forbid(unsafe_code)]

use clap::Parser;

/// The tastiest SSG.
#[derive(Parser)]
#[command(name = "reuben", version, about)]
struct Cli {}

fn main() {
    Cli::parse();
}
