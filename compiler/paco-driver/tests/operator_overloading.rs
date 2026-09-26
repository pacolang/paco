use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("paco_operator_overloading_{}_{}_{}.paco", name, std::process::id(), monotonic_suffix()));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

const SOURCE: &str = r#"
struct Point {
    x: i64,
    y: i64,

    fn add(&self, other: Self) -> Self {
        Point { x: self.x + other.x, y: self.y + other.y }
    }

    fn neg(&self) -> Self {
        Point { x: 0 - self.x, y: 0 - self.y }
    }
}

fn main() {
    let a = Point { x: 1, y: 2 };
    let b = Point { x: 3, y: 4 };
    let c = a + b;
    print(c.x);
    print(c.y);
    let d = -c;
    print(d.x);
    print(d.y)
}
"#;

#[test]
fn a_compiled_program_dispatches_operator_overloads() {
    let entry = temp_paco("compiled", SOURCE);

    let build_cli = Cli::try_parse_from(["paco", "build", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));
    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let output = Command::new(&binary_path).output().expect("compiled binary should run");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "4\n6\n-4\n-6\n");

    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(&entry);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn run_and_release_build_operator_overloading_agree() {
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
