//! Borrow-checking for `[]T`/`&[]T`/`&mut []T` (`arrays-and-slices` task
//! 3.1) — confirms the existing generic-type ownership/borrow machinery
//! `paco-borrow` already has for `Ty::Slice` is correctly mechanical, same
//! `paco check` CLI-driven pattern `check_borrowing.rs` already uses.

use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{DriverOutput, run};

#[test]
fn check_accepts_a_moved_slice_and_a_shared_slice_borrow() {
    let source = r#"
fn sum(values: &[]i64) -> i64 {
    0
}

fn consume(values: []i64) {}

fn main() {
    let buf: []i64 = slice_of_zeros<i64>(4);
    let total: i64 = sum(&buf);
    print(total);
    consume(buf)
}
"#;
    let file = write_temp_paco("slice_move_and_shared_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output, DriverOutput { stdout: String::new(), stderr: String::new() });
}

#[test]
fn check_rejects_use_of_a_slice_after_it_was_moved() {
    let source = r#"
fn consume(values: []i64) {}

fn main() {
    let buf: []i64 = slice_of_zeros<i64>(4);
    consume(buf);
    consume(buf)
}
"#;
    let file = write_temp_paco("slice_use_after_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("moved"));
    assert!(error.contains("buf"));
}

#[test]
fn check_rejects_a_mutable_slice_borrow_while_a_shared_slice_borrow_is_live() {
    let source = r#"
fn main() {
    let mut buf: []i64 = slice_of_zeros<i64>(4);
    let shared: &[]i64 = &buf;
    let unique: &mut []i64 = &mut buf;
    print(shared[0]);
    print(unique[0])
}
"#;
    let file = write_temp_paco("slice_mutable_while_shared_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("mutable"));
    assert!(error.contains("buf"));
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_check_slices_{}_{}_{}.paco",
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
