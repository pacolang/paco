use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{DriverOutput, run};

/// The ADR 0017 `sgemm` example (`extern "C" { fn cblas_sgemm(...); }` plus a
/// safe wrapper calling it through `unsafe { ... }`), trimmed to the numeric
/// and pointer types this compiler actually supports today (`i64`,
/// `*const i64`, `*mut i64`): the ADR's own example uses `i32`/`f32`, `as`
/// casts, `&[]f32` slices, and `.as_ptr()`/`.as_mut_ptr()` method calls, none
/// of which are implemented yet (this proposal's own Non-Goals section notes
/// a raw pointer has no in-language way to be *produced*, only received as a
/// parameter and passed through).
#[test]
fn check_accepts_the_adr_0017_sgemm_example() {
    let source = r#"
extern "C" {
    fn cblas_sgemm(a: *const i64, b: *const i64, c: *mut i64, m: i64, n: i64, k: i64);
}

pub fn sgemm(a: *const i64, b: *const i64, c: *mut i64, m: i64, n: i64, k: i64) {
    unsafe {
        cblas_sgemm(a, b, c, m, n, k)
    }
}
"#;
    let file = write_temp_paco("sgemm", source);
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
fn check_rejects_calling_the_extern_function_outside_unsafe() {
    let source = r#"
extern "C" {
    fn cblas_sgemm(a: *const i64, b: *const i64, c: *mut i64, m: i64, n: i64, k: i64);
}

pub fn sgemm(a: *const i64, b: *const i64, c: *mut i64, m: i64, n: i64, k: i64) {
    cblas_sgemm(a, b, c, m, n, k)
}
"#;
    let file = write_temp_paco("sgemm_missing_unsafe", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0325"));
}

#[test]
fn run_executes_a_real_extern_c_function_through_ffi() {
    let source = r#"
extern "C" {
    fn abs(n: i64) -> i64;
}

fn main() {
    unsafe {
        print(abs(0 - 9))
    }
}
"#;
    let file = write_temp_paco("run_extern_ffi_abs", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput { stdout: "9\n".to_string(), stderr: String::new() }
    );
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_ffi_extern_unsafe_{}_{}_{}.paco",
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
