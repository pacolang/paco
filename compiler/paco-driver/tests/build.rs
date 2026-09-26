use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::run;

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_build_{}_{}_{}.paco",
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

#[test]
fn build_produces_a_binary_that_runs_and_exits_with_the_program_result() {
    let file = write_temp_paco("valid", "fn main() -> i64 { 7 }");
    let cli = paco_driver::Cli::try_parse_from(["paco", "build", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    assert!(binary_path.exists(), "expected a binary at {binary_path:?}");

    let status = std::process::Command::new(&binary_path)
        .status()
        .expect("compiled binary should run");
    assert_eq!(status.code(), Some(7));

    let _ = fs::remove_file(&binary_path);
}

#[test]
fn build_fails_on_a_program_that_fails_checking_and_produces_no_binary() {
    let file = write_temp_paco("ill_typed", "fn main() -> i64 { true }");
    let cli = paco_driver::Cli::try_parse_from(["paco", "build", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0302"));
    assert!(error.contains("type mismatch"));
    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    assert!(
        !binary_path.exists(),
        "expected no binary to be produced for a program that fails checking"
    );
}

/// Regression test for a codegen bug found while bringing up `stdlib::collections::Vec<T>`'s
/// compiled path (`generic-function-codegen`): a generic struct's own `[]T` field is a
/// 16-byte `[data_ptr, len]` descriptor, like a nested struct/enum field, not an 8-byte
/// scalar — `store_field`/`read_place` previously fell through to a scalar store/load for
/// `Type::Slice`, writing/reading only the descriptor's own address and leaving its `len`
/// half as uninitialized stack garbage. That corrupted `len` then failed (or, by luck,
/// passed) the next slice-element bounds check nondeterministically. Constructing the
/// slice-holding struct inside a *generic* associated function (as `Vec::new()` does) is
/// exactly the shape that exposed it — a plain, non-generic struct with a `[]T` field could
/// still get lucky with stack contents and not trip the bug.
#[test]
fn build_indexes_a_slice_field_of_a_struct_built_by_a_generic_constructor() {
    let source = r#"
struct Boxed<T> {
    data: []T,
    len: i64,

    fn new() -> Self {
        Boxed { data: slice_of_zeros<T>(4), len: 0 }
    }
}

fn main() {
    let b: Boxed<i64> = Boxed::new();
    print(b.len);
    print(b.data[1])
}
"#;
    let file = write_temp_paco("slice_field_of_generic_struct", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "build", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    let result = std::process::Command::new(&binary_path)
        .output()
        .expect("compiled binary should run");
    assert!(
        result.status.success(),
        "compiled binary should exit successfully, got status {:?}",
        result.status
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "0\n0\n");

    let _ = fs::remove_file(&binary_path);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn build_release_produces_an_optimized_binary_whose_main_value_is_the_exit_code() {
    let file = write_temp_paco("release", "fn main() -> i64 {\n    print(7);\n    3\n}");
    let cli =
        paco_driver::Cli::try_parse_from(["paco", "build", "--release", file.to_str().unwrap()])
            .unwrap();

    run(cli).unwrap();

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    let result = std::process::Command::new(&binary_path).output().expect("compiled binary should run");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "7\n");
    assert_eq!(result.status.code(), Some(3));
    let _ = fs::remove_file(&binary_path);
}

/// `None` when the host has no system BLAS to link against (`PACO-E0803`):
/// Linux CI installs it, macOS finds it in the SDK, Windows has none.
fn build_and_run_against_system_blas(name: &str, conformance_case: &str) -> Option<String> {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/conformance/stdlib")
            .join(conformance_case)
            .join("input.paco"),
    )
    .unwrap();
    let file = write_temp_paco(name, &source);

    let build = std::process::Command::new(env!("CARGO_BIN_EXE_paco"))
        .arg("build")
        .arg(&file)
        .output()
        .unwrap();
    if !build.status.success() {
        let stderr = String::from_utf8_lossy(&build.stderr);
        if stderr.contains("PACO-E0803") {
            eprintln!("skipping {name}: a linked library is missing on this host: {stderr}");
            let _ = fs::remove_file(&file);
            return None;
        }
        panic!("{stderr}");
    }

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    let result = std::process::Command::new(&binary_path).output().unwrap();
    assert!(result.status.success());

    let _ = fs::remove_file(&binary_path);
    Some(String::from_utf8_lossy(&result.stdout).to_string())
}

#[test]
fn build_links_std_blas_dgemm_against_the_system_blas() {
    let Some(output) = build_and_run_against_system_blas("blas_dgemm", "blas_dgemm_2x2") else { return };
    assert_eq!(output, "19\n22\n43\n50\n");
}

#[test]
fn build_blas_matmul_of_math_matrices_matches_the_pure_paco_product() {
    let Some(output) = build_and_run_against_system_blas("blas_matmul", "blas_matrix_matmul") else { return };
    assert_eq!(output, "58\n58\n64\n64\n139\n139\n154\n154\n");
}

#[test]
fn build_compiles_panic_to_a_located_report() {
    let file = write_temp_paco("panic_abort", "fn main() -> i64 {\n    if 1 > 0 {\n        panic(\"boom\")\n    }\n    0\n}\n");
    run(paco_driver::Cli::try_parse_from(["paco", "build", file.to_str().unwrap()]).unwrap()).unwrap();

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    let result = std::process::Command::new(&binary_path).output().unwrap();
    assert_eq!(result.status.code(), Some(101));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.starts_with(&format!("panic at {}:3:9: boom\n", file.display())), "{stderr}");

    let _ = fs::remove_file(&binary_path);
}
