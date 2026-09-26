//! `paco run` vs `paco build --release --backend llvm` output parity for `[]T` construction
//! (via `slice_of_zeros`, arrays-and-slices' explicitly-placeholder
//! construction builtin) and bounds-checked indexed reads/writes
//! (`arrays-and-slices` task 5.2) — the same differential pattern
//! `differential_corpus.rs`/`concurrency_codegen_differential.rs` already
//! establish.

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
fn constructing_and_reading_several_slice_elements_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "slice_construct_and_read",
        r#"
        fn main() {
            let mut buf: []i64 = slice_of_zeros<i64>(4);
            buf[0] = 10;
            buf[1] = 20;
            buf[2] = 30;
            buf[3] = 40;
            print(buf[0]);
            print(buf[1]);
            print(buf[2]);
            print(buf[3])
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn a_structurally_dispatched_index_method_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "slice_structural_index_dispatch",
        r#"
        struct Bag {
            value: i64,

            fn index(&self, i: i64) -> &i64 {
                &self.value
            }
        }

        fn main() {
            let b = Bag { value: 7 };
            print(b[0])
        }
        "#,
    );
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_slices_differential_{}_{}_{}.paco",
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
