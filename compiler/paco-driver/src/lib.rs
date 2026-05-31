//! Paco driver command-line interface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "paco")]
#[command(about = "Paco compiler driver")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Build {
        path: Option<PathBuf>,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        target: Option<String>,
    },
    Check {
        file: PathBuf,
    },
    Run {
        file: Option<PathBuf>,
    },
    Test {
        path: Option<PathBuf>,
    },
    Fmt {
        file: PathBuf,
        #[arg(long)]
        write: bool,
    },
    Doc,
    Clean,
}

pub fn run(cli: Cli) -> Result<(), String> {
    let name = match cli.command {
        Commands::Build { .. } => "build",
        Commands::Check { .. } => "check",
        Commands::Run { .. } => "run",
        Commands::Test { .. } => "test",
        Commands::Fmt { .. } => "fmt",
        Commands::Doc => "doc",
        Commands::Clean => "clean",
    };

    Err(format!("subcommand `{name}` is not implemented"))
}
