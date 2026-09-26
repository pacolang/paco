use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use paco_driver::{COMPILER_EXE, exec_cached};

/// `paco run [FILE] [-- ARGS...]`, the only shape answered without the
/// compiler.
fn run_arguments(args: &[OsString]) -> Option<(PathBuf, Vec<String>)> {
    let [command, rest @ ..] = args else { return None };
    if command != "run" {
        return None;
    }
    let (file, rest) = match rest {
        [file, rest @ ..] if file != "--" && !file.to_string_lossy().starts_with('-') => (PathBuf::from(file), rest),
        _ => (PathBuf::from("main.paco"), rest),
    };
    let program_args = match rest {
        [] => &[][..],
        [separator, program_args @ ..] if separator == "--" => program_args,
        _ => return None,
    };
    let program_args = program_args.iter().map(|arg| arg.clone().into_string().ok()).collect::<Option<_>>()?;
    Some((file, program_args))
}

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if let Some((file, program_args)) = run_arguments(&args)
        && let Some(code) = exec_cached(&file, &program_args)
    {
        std::process::exit(code);
    }
    let compiler = std::env::current_exe().map(|exe| exe.with_file_name(COMPILER_EXE)).unwrap_or_else(|_| COMPILER_EXE.into());
    let mut command = Command::new(&compiler);
    command.args(&args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("failed to run `{}`: {error}", compiler.display());
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("failed to run `{}`: {error}", compiler.display());
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<(PathBuf, Vec<String>)> {
        run_arguments(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn only_plain_run_invocations_are_answered_from_the_cache() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(parse(&["run"]), Some(("main.paco".into(), vec![])));
        assert_eq!(parse(&["run", "a.paco"]), Some(("a.paco".into(), vec![])));
        assert_eq!(parse(&["run", "--", "x", "--y"]), Some(("main.paco".into(), args(&["x", "--y"]))));
        assert_eq!(parse(&["run", "a.paco", "--", "x"]), Some(("a.paco".into(), args(&["x"]))));
        assert_eq!(parse(&["run", "--help"]), None);
        assert_eq!(parse(&["run", "a.paco", "x"]), None);
        assert_eq!(parse(&["build", "a.paco"]), None);
        assert_eq!(parse(&[]), None);
    }
}
