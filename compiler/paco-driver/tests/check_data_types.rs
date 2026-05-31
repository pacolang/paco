use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{DriverOutput, run};

#[test]
fn check_accepts_struct_program_without_running_it() {
    let source = r#"
struct Point { x: int, y: int }

fn make() -> Point {
    Point { x: 1, y: 2 }
}
"#;
    let file = write_temp_paco("check_struct", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: String::new(),
            stderr: String::new(),
        }
    );
}

#[test]
fn check_reports_struct_type_error() {
    let source = "struct Point { x: int } fn make() -> Point { Point { x: true } }";
    let file = write_temp_paco("check_struct_error", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0302"));
    assert!(error.contains("type mismatch"));
}

#[test]
fn check_reports_undeclared_name_inside_struct_literal_field() {
    let source = "struct Point { x: int } fn make() -> Point { Point { x: missing } }";
    let file = write_temp_paco("check_struct_literal_missing_name", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0201"));
    assert!(error.contains("name not found"));
}

#[test]
fn check_reports_undeclared_name_inside_struct_method() {
    let source = "struct Point { x: int, fn bad(self&) -> int { missing } }";
    let file = write_temp_paco("check_method_missing_name", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0201"));
    assert!(error.contains("name not found"));
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_data_types_{}_{}_{}.paco",
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
