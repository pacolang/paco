//! `arrays-and-slices` task 5.3: an out-of-bounds slice index is a defined
//! runtime error under both `paco run` and `paco build` — not
//! unrelated-memory reads, per the spec delta's own memory-safety
//! requirement.

use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

const SOURCE: &str = r#"
fn main() {
    let buf: []i64 = slice_of_zeros<i64>(3);
    print(buf[5])
}
"#;

#[test]
fn an_out_of_bounds_index_is_a_located_panic_under_paco_run() {
    let file = write_temp_paco("run", SOURCE);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains(":4:11: index 5 out of bounds for length 3"), "{error}");
    assert!(error.contains("status 101"), "{error}");
}

#[test]
fn an_out_of_bounds_index_panics_instead_of_reading_unrelated_memory_when_compiled() {
    let file = write_temp_paco("compiled", SOURCE);
    let build_cli = Cli::try_parse_from(["paco", "build", file.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    let output = Command::new(&binary_path).output().expect("compiled binary should run");

    assert_eq!(output.status.code(), Some(101), "expected a panic exit, got {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(":4:11: index 5 out of bounds for length 3"), "{stderr}");
    assert!(output.stdout.is_empty());

    let _ = fs::remove_file(&binary_path);
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_slice_bounds_check_{}_{}_{}.paco",
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
