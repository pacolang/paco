use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("paco_unsafe_ffi_codegen_{}_{}_{}.paco", name, std::process::id(), monotonic_suffix()));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

const ABS_SOURCE: &str = r#"
extern "C" {
    fn abs(n: i64) -> i64;
}

fn main() {
    unsafe {
        print(abs(0 - 5))
    }
}
"#;

#[test]
fn a_compiled_program_calls_a_declared_extern_function() {
    let entry = temp_paco("abs_build", ABS_SOURCE);

    let build_cli = Cli::try_parse_from(["paco", "build", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));

    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let output = Command::new(&binary_path).output().expect("compiled binary should run");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "5\n");

    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(&entry);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn run_and_release_build_extern_calls_agree() {
    let entry = temp_paco("abs_differential", ABS_SOURCE);

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

const RAW_POINTER_SOURCE: &str = r#"
extern "C" {
    fn calloc(nmemb: i64, size: i64) -> *const i64;
    fn free(ptr: *const i64);
}

fn read_it(p: *const i64) -> i64 {
    unsafe { *p }
}

fn main() {
    unsafe {
        let p: *const i64 = calloc(1, 8);
        print(read_it(p));
        free(p)
    }
}
"#;

#[test]
fn a_compiled_program_dereferences_a_raw_pointer_parameter() {
    let entry = temp_paco("deref_build", RAW_POINTER_SOURCE);

    let build_cli = Cli::try_parse_from(["paco", "build", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));

    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let output = Command::new(&binary_path).output().expect("compiled binary should run");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "0\n");

    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(&entry);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn run_and_release_build_raw_pointer_dereference_agree() {
    let entry = temp_paco("deref_differential", RAW_POINTER_SOURCE);

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
