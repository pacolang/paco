//! `stdlib::math::Matrix` checks shapes at compile time; only `Dyn` extents
//! are checked at run time, and a mismatch there is an `Err`.

use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("paco_matrix_{name}_{}_{nanos}.paco", std::process::id()));
    fs::write(&path, source).unwrap();
    path
}

fn check(name: &str, source: &str) -> Result<(), String> {
    let file = write_temp_paco(name, source);
    let result = run(Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap());
    let _ = fs::remove_file(&file);
    result.map(|_| ())
}

#[test]
fn adding_matrices_of_different_static_shapes_is_a_compile_error() {
    let error = check(
        "add",
        "use stdlib::math;\nfn main() {\n    let a = math::Matrix<float, 2, 3>::zeros();\n    let b = math::Matrix<float, 3, 2>::zeros();\n    let c = a + &b;\n}\n",
    )
    .unwrap_err();
    assert!(error.contains("PACO-E0301") || error.contains("PACO-E0336"), "{error}");
    assert!(error.contains("Matrix<float, 2, 3>"), "{error}");
}

#[test]
fn multiplying_matrices_with_disagreeing_inner_dimensions_is_a_compile_error() {
    let error = check(
        "mul",
        "use stdlib::math;\nfn main() {\n    let a = math::Matrix<float, 2, 3>::zeros();\n    let b = math::Matrix<float, 2, 2>::zeros();\n    let c = a * &b;\n}\n",
    )
    .unwrap_err();
    assert!(error.contains("PACO-E0301") || error.contains("PACO-E0336"), "{error}");
    let error = check(
        "blas",
        "use stdlib::blas;\nuse stdlib::math;\nfn main() {\n    let a = math::Matrix<float, 2, 3>::zeros();\n    let b = math::Matrix<float, 2, 2>::zeros();\n    let c = blas::matmul(&a, &b);\n}\n",
    )
    .unwrap_err();
    assert!(error.contains("PACO-E0336"), "{error}");
    assert!(error.contains("expected `3`, found `2`"), "{error}");
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn a_dyn_mismatch_is_an_err_in_both_backends() {
    let source = "
use stdlib::math;

fn main() {
    match math::Matrix<float, Dyn, 2>::with_shape(3, 2) {
        Result::Ok(a) => match math::Matrix<float, Dyn, 2>::with_shape(4, 2) {
            Result::Ok(b) => match a.checked_add(&b) {
                Result::Ok(sum) => print(sum.rows()),
                Result::Err(e) => {
                    print(e.axis);
                    print(e.expected);
                    print(e.found)
                }
            },
            Result::Err(e) => print(-1),
        },
        Result::Err(e) => print(-2),
    }
    match math::Matrix<float, 2, 2>::with_shape(2, 5) {
        Result::Ok(m) => print(m.cols()),
        Result::Err(e) => print(e.axis),
    }
}
";
    let file = write_temp_paco("dyn", source);
    let run_output = run(Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap()).unwrap();
    assert_eq!(run_output.stdout, "0\n3\n4\n1\n");
    run(Cli::try_parse_from(["paco", "build", "--release", "--backend", "llvm", file.to_str().unwrap()]).unwrap()).unwrap();
    let binary = file.with_extension(std::env::consts::EXE_EXTENSION);
    let compiled = Command::new(&binary).output().unwrap();
    let _ = fs::remove_file(&binary);
    let _ = fs::remove_file(&file);
    assert_eq!(String::from_utf8_lossy(&compiled.stdout), run_output.stdout);
}
