use clap::CommandFactory;
use clap::Parser;
use paco_driver::{Cli, Commands};

#[test]
fn cli_exposes_phase_zero_subcommands() {
    let command = Cli::command();
    let names: Vec<_> = command
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_string())
        .collect();

    assert_eq!(
        names,
        vec!["build", "check", "run", "test", "fmt", "doc", "clean"]
    );
}

#[test]
fn run_subcommand_accepts_project_default_without_file_argument() {
    let cli = Cli::try_parse_from(["paco", "run"]).unwrap();

    assert!(matches!(cli.command, Commands::Run { file: None }));
}
