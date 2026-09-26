use clap::Parser;
use paco_driver::{Cli, Commands, exec_program, run, set_format};

fn main() {
    let cli = Cli::parse();
    if let Commands::Run { file, args, format } = &cli.command {
        set_format(format.unwrap_or_default());
        let file = file.clone().unwrap_or_else(|| "main.paco".into());
        std::process::exit(exec_program(&file, args));
    }

    match run(cli) {
        Ok(output) => {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
