//! Paco driver command-line interface.

use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand};
use paco_diag::{Diagnostic, Reporter};
use paco_span::SourceMap;
use paco_syntax::{ast::Item, lex::lex, parse::parse_module};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverOutput {
    pub stdout: String,
    pub stderr: String,
}

pub fn run(cli: Cli) -> Result<DriverOutput, String> {
    match cli.command {
        Commands::Run { file } => run_file(file.unwrap_or_else(|| PathBuf::from("main.paco"))),
        Commands::Build { .. } => not_implemented("build"),
        Commands::Check { file } => check_file(file),
        Commands::Test { .. } => not_implemented("test"),
        Commands::Fmt { file, write } => format_file(file, write),
        Commands::Doc => not_implemented("doc"),
        Commands::Clean => not_implemented("clean"),
    }
}

fn run_file(file: PathBuf) -> Result<DriverOutput, String> {
    let CheckedProgram {
        sources,
        module,
        mut reporter,
    } = check_program(file)?;
    if !module
        .items
        .iter()
        .any(|item| matches!(item, Item::Fn(function) if function.name == "main"))
    {
        reporter.push(Diagnostic::error(
            "PACO-E0001",
            module.span,
            "main function was not found",
        ));
    }
    if reporter.has_errors() {
        return Err(reporter.emit_to_string(&sources));
    }

    let stdout =
        paco_eval::evaluate_module(&module).map_err(|error| format!("runtime error: {error}"))?;
    Ok(DriverOutput {
        stdout,
        stderr: String::new(),
    })
}

fn check_file(file: PathBuf) -> Result<DriverOutput, String> {
    let _ = check_program(file)?;
    Ok(DriverOutput {
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn format_file(file: PathBuf, write: bool) -> Result<DriverOutput, String> {
    let source = fs::read_to_string(&file)
        .map_err(|error| format!("failed to read `{}`: {error}", file.display()))?;
    let mut sources = SourceMap::new();
    let file_id = sources.add_file(file.display().to_string(), source);
    let source_ref = sources.source(file_id).unwrap_or("");
    let mut reporter = Reporter::new();

    let tokens = lex(source_ref, file_id, &mut reporter);
    if reporter.has_errors() {
        return Err(reporter.emit_to_string(&sources));
    }

    let module =
        parse_module(&tokens, &mut reporter).map_err(|_| reporter.emit_to_string(&sources))?;
    if reporter.has_errors() {
        return Err(reporter.emit_to_string(&sources));
    }

    let formatted = paco_syntax::fmt::format_module(&module, Some(source_ref));

    if write {
        fs::write(&file, &formatted)
            .map_err(|error| format!("failed to write `{}`: {error}", file.display()))?;
        Ok(DriverOutput {
            stdout: String::new(),
            stderr: String::new(),
        })
    } else {
        Ok(DriverOutput {
            stdout: formatted,
            stderr: String::new(),
        })
    }
}

struct CheckedProgram {
    sources: SourceMap,
    module: paco_syntax::ast::Module,
    reporter: Reporter,
}

fn check_program(file: PathBuf) -> Result<CheckedProgram, String> {
    let source = fs::read_to_string(&file)
        .map_err(|error| format!("failed to read `{}`: {error}", file.display()))?;
    let mut sources = SourceMap::new();
    let file_id = sources.add_file(file.display().to_string(), source);
    let source = sources.source(file_id).unwrap_or("");
    let mut reporter = Reporter::new();

    let tokens = lex(source, file_id, &mut reporter);
    if reporter.has_errors() {
        return Err(reporter.emit_to_string(&sources));
    }

    let module =
        parse_module(&tokens, &mut reporter).map_err(|_| reporter.emit_to_string(&sources))?;
    if reporter.has_errors() {
        return Err(reporter.emit_to_string(&sources));
    }

    paco_resolve::resolve_module(&module, &mut reporter)
        .map_err(|_| reporter.emit_to_string(&sources))?;
    paco_types::check_module(&module, &mut reporter)
        .map_err(|_| reporter.emit_to_string(&sources))?;
    paco_borrow::check_module(&module, &mut reporter)
        .map_err(|_| reporter.emit_to_string(&sources))?;

    Ok(CheckedProgram {
        sources,
        module,
        reporter,
    })
}

fn not_implemented(name: &str) -> Result<DriverOutput, String> {
    Err(format!("subcommand `{name}` is not implemented"))
}
