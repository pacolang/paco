use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("paco_struct_return_{}_{}_{}.paco", name, std::process::id(), monotonic_suffix()));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

fn build_and_run(entry: &PathBuf) -> String {
    let build_cli = Cli::try_parse_from(["paco", "build", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));
    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let output = Command::new(&binary_path).output().expect("compiled binary should run");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(entry);
    stdout
}

#[test]
fn a_struct_returned_by_value_survives_an_intervening_call() {
    let entry = temp_paco(
        "two_field_intervening_call",
        r#"
struct Point {
    x: i64,
    y: i64,
}

fn make(x: i64, y: i64) -> Point {
    Point { x: x, y: y }
}

fn main() {
    let c = make(4, 6);
    print(c.x);
    print(c.y)
}
"#,
    );
    assert_eq!(build_and_run(&entry), "4\n6\n");
}

#[test]
fn a_nested_aggregate_returning_call_survives_an_intervening_call() {
    let entry = temp_paco(
        "nested_aggregate_return",
        r#"
struct Inner {
    v: i64,
}

struct Outer {
    inner: Inner,
}

fn make_inner() -> Inner {
    Inner { v: 9 }
}

fn make_outer() -> Outer {
    Outer { inner: make_inner() }
}

fn main() {
    let o = make_outer();
    print(o.inner.v);
    print(o.inner.v)
}
"#,
    );
    assert_eq!(build_and_run(&entry), "9\n9\n");
}

#[test]
fn a_generic_struct_returned_by_value_survives_an_intervening_call() {
    let entry = temp_paco(
        "generic_struct_return",
        r#"
struct Box<T> {
    value: T,
}

fn make_box(value: i64) -> Box<i64> {
    Box<i64> { value: value }
}

fn main() {
    let b = make_box(7);
    print(b.value);
    print(b.value)
}
"#,
    );
    assert_eq!(build_and_run(&entry), "7\n7\n");
}

#[test]
fn a_slice_returned_by_value_survives_an_intervening_call() {
    let entry = temp_paco(
        "slice_return",
        r#"
fn make() -> []i64 {
    slice_of_zeros<i64>(3)
}

fn main() {
    let s = make();
    print(s[0]);
    print(s[0])
}
"#,
    );
    assert_eq!(build_and_run(&entry), "0\n0\n");
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn run_and_release_build_output_agree_for_a_returned_struct() {
    let entry = temp_paco(
        "differential",
        r#"
struct Point {
    x: i64,
    y: i64,
}

fn make(x: i64, y: i64) -> Point {
    Point { x: x, y: y }
}

fn main() {
    let c = make(4, 6);
    print(c.x);
    print(c.y)
}
"#,
    );

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
