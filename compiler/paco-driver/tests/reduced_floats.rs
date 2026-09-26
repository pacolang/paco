//! Reduced-precision floats: `paco run` and `paco build` agree bit for bit.

use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("paco_floats_{name}_{}_{nanos}.paco", std::process::id()));
    fs::write(&path, source).unwrap();
    path
}

fn run_both(name: &str, source: &str) -> String {
    let file = write_temp_paco(name, source);
    let run_output = run(Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap())
        .unwrap_or_else(|error| panic!("`paco run` failed: {error}"));
    run(Cli::try_parse_from(["paco", "build", "--release", "--backend", "llvm", file.to_str().unwrap()]).unwrap())
        .unwrap_or_else(|error| panic!("`paco build` failed: {error}"));
    let binary = file.with_extension(std::env::consts::EXE_EXTENSION);
    let compiled = Command::new(&binary).output().expect("compiled binary should run");
    let _ = fs::remove_file(&binary);
    let _ = fs::remove_file(&file);
    assert!(compiled.status.success(), "{:?}", compiled.status);
    assert_eq!(String::from_utf8_lossy(&compiled.stdout), run_output.stdout, "stdout diverged for `{name}`");
    run_output.stdout
}

fn check(source: &str) -> Result<paco_driver::DriverOutput, String> {
    let file = write_temp_paco("check", source);
    let result = run(Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap());
    let _ = fs::remove_file(&file);
    result
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn narrowing_rounds_to_nearest_even_in_both_backends() {
    let output = run_both(
        "narrowing",
        "
fn scaled(x: float) -> i64 { (x * 1048576.0) as i64 }
fn main() {
    let x = 0.1;
    print(scaled(x as f32 as float));
    print(scaled(x as bf16 as float));
    print(scaled(x as f16 as float));
    print(scaled(x as f8e4m3 as float));
    print(scaled(x as f8e5m2 as float))
}
",
    );
    assert_eq!(output, "104857\n104960\n104832\n106496\n98304\n");
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn reduced_precision_arithmetic_rounds_each_operation() {
    let output = run_both(
        "arithmetic",
        "
fn main() {
    let one: bf16 = 1.0;
    let tiny: bf16 = 0.001;
    let mut acc: bf16 = 1.0;
    let mut i = 0;
    while i < 10 {
        acc = acc + tiny;
        i = i + 1
    }
    print(acc == one);
    let a: f32 = 16777216.0;
    let b: f32 = 1.0;
    print(((a + b) as float) as i64);
    let h: f16 = 3.0;
    print(((h * h - h / h) as float) as i64);
    print(-h < h)
}
",
    );
    assert_eq!(output, "true\n16777216\n8\ntrue\n");
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn overflow_becomes_infinity_or_nan() {
    let output = run_both(
        "overflow",
        "
fn main() {
    let big = 100000.0;
    let h = big as f16;
    print((h as float) > 1000000.0);
    let e = big as f8e4m3;
    print((e as float) == (e as float));
    let w: f8e4m3 = 2.0;
    print(w == (2.0 as f8e4m3))
}
",
    );
    assert_eq!(output, "true\nfalse\ntrue\n");
}

#[test]
fn fp8_has_no_arithmetic() {
    let error = check("fn main() { let a: f8e4m3 = 1.0;\n let b = a + a; }").unwrap_err();
    assert!(error.contains("PACO-E0339"), "{error}");
    let error = check("fn main() { let a: f8e5m2 = 1.0;\n let b = a < a; }").unwrap_err();
    assert!(error.contains("PACO-E0339"), "{error}");
}

#[test]
fn no_implicit_conversion_between_float_types() {
    let error = check("fn main() { let a: bf16 = 1.5;\n let b: f32 = a; }").unwrap_err();
    assert!(error.contains("expected f32, found bf16"), "{error}");
    let error = check("fn main() { let a: bf16 = 1.5;\n let b: f32 = 2.0;\n let c = a + b; }").unwrap_err();
    assert!(error.contains("PACO-E0301"), "{error}");
    let ok = check("fn main() { let a: bf16 = 1.5;\n let b: f32 = a as f32; }");
    assert!(ok.is_ok(), "{ok:?}");
}
