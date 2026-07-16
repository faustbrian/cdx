//! Binary entrypoint for the `cdx` conversation picker.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(author, version, about = "Global Codex conversation picker")]
struct Cli {
    #[arg(long)]
    db_path: Option<PathBuf>,
    #[arg(long)]
    session_index_path: Option<PathBuf>,
    #[arg(long, default_value = "codex")]
    codex_bin: String,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, help = "Include spawned subagent conversations")]
    include_subagents: bool,
}

fn main() {
    if let Err(error) = run() {
        let _ignored = writeln!(io::stderr(), "cdx: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    cdx::run_default(cdx::RunConfig {
        db_path: cli.db_path,
        session_index_path: cli.session_index_path,
        codex_bin: cli.codex_bin,
        dry_run: cli.dry_run,
        include_subagents: cli.include_subagents,
    })
}
