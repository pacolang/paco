//! `paco run` vs `paco build --release --backend llvm` output parity for generic struct/enum
//! codegen (`generic-aggregate-codegen` task 7.2) — the same differential
//! pattern `differential_corpus.rs`/`concurrency_codegen_differential.rs`
//! already establish.

use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn assert_run_matches_release_llvm(name: &str, source: &str) {
    let file = write_temp_paco(name, source);

    let run_cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();
    let run_output = run(run_cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));

    let build_cli = Cli::try_parse_from(["paco", "build", "--release", "--backend", "llvm", file.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    let compiled = Command::new(&binary_path)
        .output()
        .expect("compiled binary should run");
    let compiled_stdout = String::from_utf8_lossy(&compiled.stdout).to_string();

    assert_eq!(compiled_stdout, run_output.stdout, "stdout diverged for `{name}`");
    assert!(compiled.status.success(), "`{name}` binary exited with {:?}", compiled.status);

    let _ = fs::remove_file(&binary_path);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn a_generic_struct_round_trips_a_single_instantiation() {
    assert_run_matches_release_llvm(
        "generic_struct_single_instantiation",
        r#"
        struct Box<T> { value: T }

        fn main() {
            let b = Box<i64> { value: 42 };
            print(b.value)
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn a_generic_struct_gets_independent_layouts_per_instantiation() {
    assert_run_matches_release_llvm(
        "generic_struct_two_instantiations",
        r#"
        struct Box<T> { value: T }

        fn main() {
            let a = Box<i64> { value: 7 };
            let b = Box<bool> { value: true };
            print(a.value);
            print(b.value)
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn a_generic_enum_round_trips_construct_and_match() {
    assert_run_matches_release_llvm(
        "generic_enum_construct_and_match",
        r#"
        enum Choice<T> { Picked(T), None }

        fn make() -> Choice<i64> {
            Choice::Picked(9)
        }

        fn main() {
            match make() {
                Choice::Picked(value) => print(value),
                Choice::None => print(-1),
            }
        }
        "#,
    );
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_generic_aggregate_codegen_differential_{}_{}_{}.paco",
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
