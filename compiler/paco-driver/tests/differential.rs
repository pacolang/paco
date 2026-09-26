use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

/// Runs `source` with `paco run` (Cranelift, debug) and as a `paco build
/// --release --backend llvm` binary, and asserts identical stdout: the two
/// code generators and profiles check each other. Panics with both outputs
/// on any mismatch, or if either path itself fails.
fn assert_run_matches_release_llvm(source: &str) {
    let file = write_temp_paco("differential", source);

    let run_cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();
    let run_output = run(run_cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));

    let build_cli = Cli::try_parse_from(["paco", "build", "--release", "--backend", "llvm", file.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    let compiled = Command::new(&binary_path)
        .output()
        .expect("compiled binary should run");
    let compiled_stdout = String::from_utf8_lossy(&compiled.stdout).to_string();

    assert_eq!(
        compiled_stdout, run_output.stdout,
        "`paco run` and the release LLVM build diverged for:\n{source}"
    );
    assert!(
        compiled.status.success(),
        "compiled binary exited with {:?} for:\n{source}",
        compiled.status
    );

    let _ = fs::remove_file(&binary_path);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn differential_helper_passes_on_a_trivial_program() {
    assert_run_matches_release_llvm("fn main() { print(1) }");
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_differential_{}_{}_{}.paco",
        name,
        std::process::id(),
        monotonic_suffix()
    ));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
