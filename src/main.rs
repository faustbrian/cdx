//! Binary entrypoint for the `cdx` conversation picker.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Global Claude Code and Codex conversation picker"
)]
struct Cli {
    #[arg(long)]
    db_path: Option<PathBuf>,
    #[arg(long)]
    session_index_path: Option<PathBuf>,
    #[arg(
        long,
        help = "Claude Code projects directory [default: ~/.claude/projects]"
    )]
    claude_projects_path: Option<PathBuf>,
    #[arg(long, default_value = "codex")]
    codex_bin: String,
    #[arg(long, default_value = "claude")]
    claude_bin: String,
    #[arg(long, value_enum, default_value_t = AgentArg::Claude, help = "Agent shown on start")]
    agent: AgentArg,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, help = "Allow Codex to use its shared background daemon")]
    allow_daemon: bool,
    #[arg(long, help = "Include spawned subagent conversations")]
    include_subagents: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentArg {
    Claude,
    Codex,
    All,
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
        claude_projects_path: cli.claude_projects_path,
        codex_bin: cli.codex_bin,
        claude_bin: cli.claude_bin,
        dry_run: cli.dry_run,
        allow_daemon: cli.allow_daemon,
        include_subagents: cli.include_subagents,
        agent_mode: match cli.agent {
            AgentArg::Claude => cdx::AgentMode::Claude,
            AgentArg::Codex => cdx::AgentMode::Codex,
            AgentArg::All => cdx::AgentMode::All,
        },
    })
}
