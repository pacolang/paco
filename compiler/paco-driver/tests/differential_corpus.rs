use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn assert_run_matches_release_llvm(name: &str, source: &str) -> String {
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
    compiled_stdout
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn factorial() {
    assert_run_matches_release_llvm(
        "factorial",
        "fn fact(n: i64) -> i64 { if n == 0 { 1 } else { n * fact(n - 1) } } fn main() { print(fact(10)) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn statement_return() {
    assert_run_matches_release_llvm(
        "statement_return",
        "fn value() -> i64 { return 1; } fn main() { print(value()) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn if_branch_return() {
    assert_run_matches_release_llvm(
        "if_branch_return",
        "fn choose(flag: bool) -> i64 { if flag { return 1; } return 2; } fn main() { print(choose(false)); print(choose(true)) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn struct_field() {
    assert_run_matches_release_llvm(
        "struct_field",
        "struct Point { x: i64, y: i64 } fn main() { let p = Point { x: 2, y: 3 }; print(p.x) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn struct_field_assignment() {
    assert_run_matches_release_llvm(
        "struct_field_assignment",
        "struct Point { x: i64, y: i64 } fn main() { let mut p = Point { x: 2, y: 3 }; p.x = 5; print(p.x) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn struct_method() {
    assert_run_matches_release_llvm(
        "struct_method",
        "struct Point { x: i64, y: i64, fn sum(&self) -> i64 { self.x + self.y } } fn main() { let p = Point { x: 4, y: 5 }; print(p.sum()) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn mutable_self_method() {
    assert_run_matches_release_llvm(
        "mutable_self_method",
        "struct Counter { value: i64, fn inc(&mut self) { self.value = self.value + 1 } } fn main() { let mut c = Counter { value: 1 }; c.inc(); print(c.value) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn associated_constructor() {
    assert_run_matches_release_llvm(
        "associated_constructor",
        "struct Point { x: i64, y: i64, fn origin() -> Point { Point { x: 0, y: 0 } } } fn main() { let p = Point::origin(); print(p.x) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn enum_match() {
    assert_run_matches_release_llvm(
        "enum_match",
        "enum Maybe { Some(i64), None } fn main() { let value: Maybe = Maybe::Some(41); let result: i64 = match value { Maybe::Some(x) => x + 1, Maybe::None => 0, }; print(result) }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn if_let_else() {
    assert_run_matches_release_llvm(
        "if_let_else",
        "enum Maybe { Some(i64), None } fn main() { let value: Maybe = Maybe::Some(7); if let Maybe::Some(x) = value { print(x) } else { print(0) } }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn while_let() {
    assert_run_matches_release_llvm(
        "while_let",
        "enum Maybe { Some(i64), None } fn next(value: i64) -> Maybe { if value > 0 { Maybe::Some(value) } else { Maybe::None } } fn main() { let mut current: i64 = 3; while let Maybe::Some(n) = next(current) { print(n); current = current - 1 } }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn for_range() {
    assert_run_matches_release_llvm(
        "for_range",
        "fn main() { for n in 1..=3 { print(n) } }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn for_range_continue() {
    assert_run_matches_release_llvm(
        "for_range_continue",
        "fn main() { for n in 1..=3 { if n == 2 { continue } print(n) } }",
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn mut_borrow_propagation_conformance_case() {
    let case = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/core/mut_borrow_propagation");
    let expected = fs::read_to_string(case.join("expected.stdout")).unwrap();
    let output = assert_run_matches_release_llvm(
        "mut_borrow_propagation",
        &fs::read_to_string(case.join("input.paco")).unwrap(),
    );
    assert_eq!(output, expected);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn core_conformance_cases_match_in_both_backends() {
    for name in ["deref_borrows", "method_through_borrow", "tuples"] {
        let case = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/core").join(name);
        let expected = fs::read_to_string(case.join("expected.stdout")).unwrap();
        let output = assert_run_matches_release_llvm(name, &fs::read_to_string(case.join("input.paco")).unwrap());
        assert_eq!(output, expected, "`{name}`");
    }
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_differential_corpus_{}_{}_{}.paco",
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
