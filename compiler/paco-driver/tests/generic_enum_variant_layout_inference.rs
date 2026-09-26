use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_generic_enum_variant_layout_inference_{}_{}_{}.paco",
        name,
        std::process::id(),
        monotonic_suffix()
    ));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

const SOURCE: &str = r#"
enum Option<T> {
    Some(T),
    None,
}

fn maybe(flag: bool) -> Option<i64> {
    if flag {
        return Option::None
    }
    Option::Some(5)
}

fn main() {
    match maybe(true) {
        Option::Some(v) => print(v),
        Option::None => print(0 - 1),
    }
    match maybe(false) {
        Option::Some(v) => print(v),
        Option::None => print(0 - 1),
    }
}
"#;

#[test]
fn a_compiled_program_constructs_a_returned_generic_none_variant() {
    let entry = temp_paco("compiled", SOURCE);

    let build_cli = Cli::try_parse_from(["paco", "build", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));
    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let output = Command::new(&binary_path).output().expect("compiled binary should run");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "-1\n5\n");

    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(&entry);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn run_and_release_build_output_agree_for_a_generic_none_variant() {
    let entry = temp_paco("differential", SOURCE);

    let run_cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let run_output = run(run_cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));

    let build_cli = Cli::try_parse_from(["paco", "build", "--release", "--backend", "llvm", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));
    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let compiled = Command::new(&binary_path).output().expect("compiled binary should run");

    assert_eq!(String::from_utf8_lossy(&compiled.stdout), run_output.stdout);
    assert!(compiled.status.success());

    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(&entry);
}

#[test]
fn a_let_annotated_generic_none_compiles() {
    let entry = temp_paco(
        "let_annotated",
        r#"
enum Option<T> {
    Some(T),
    None,
}

fn main() {
    let done: Option<i64> = Option::None;
    match done {
        Option::Some(v) => print(v),
        Option::None => print(0 - 1),
    }
}
"#,
    );

    let build_cli = Cli::try_parse_from(["paco", "build", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));
    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let output = Command::new(&binary_path).output().expect("compiled binary should run");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "-1\n");

    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(&entry);
}
