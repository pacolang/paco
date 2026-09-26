//! Diagnostics for dereferencing borrows, tuple destructuring, and names
//! of modules reached only through another module's imports.

use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

fn check(name: &str, source: &str) -> Result<paco_driver::DriverOutput, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let file: PathBuf = std::env::temp_dir().join(format!("paco_deref_{name}_{}_{nanos}.paco", std::process::id()));
    fs::write(&file, source).unwrap();
    let result = run(Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap());
    let _ = fs::remove_file(&file);
    result
}

#[test]
fn writing_through_a_shared_borrow_is_rejected() {
    let error = check("shared", "fn set(n: &i64) {\n    *n = 1\n}\nfn main() {}\n").unwrap_err();
    assert!(error.contains("PACO-E0307"), "{error}");
    assert!(error.contains("cannot assign through &i64"), "{error}");
}

#[test]
fn dereferencing_a_non_pointer_is_rejected() {
    let error = check("value", "fn main() {\n    let x = 1;\n    print(*x)\n}\n").unwrap_err();
    assert!(error.contains("PACO-E0301"), "{error}");
    assert!(error.contains("expected a borrow or raw pointer, found i64"), "{error}");
}

#[test]
fn writing_through_a_raw_pointer_needs_unsafe() {
    let error = check("raw", "fn set(p: *mut i64) {\n    *p = 1\n}\nfn main() {}\n").unwrap_err();
    assert!(error.contains("PACO-E0325"), "{error}");
}

#[test]
fn a_tuple_pattern_must_match_the_tuple_arity() {
    let error = check("arity", "fn main() {\n    let (a, b, c) = (1, 2);\n}\n").unwrap_err();
    assert!(error.contains("tuple pattern expects 2 elements, found 3"), "{error}");
}

#[test]
fn a_module_needs_its_own_use_even_when_a_dependency_imports_it() {
    let error = check("transitive", "use stdlib::blas;\n\nfn main() {\n    print(math::Matrix<float, 2, 2>::zeros().rows())\n}\n")
        .unwrap_err();
    assert!(error.contains("PACO-E1001"), "{error}");
    assert!(error.contains(":4:11: module `math` is not imported in this file"), "{error}");
    assert!(check("direct", "use stdlib::blas;\nuse stdlib::math;\n\nfn main() {\n    print(math::Matrix<float, 2, 2>::zeros().rows())\n}\n").is_ok());
}
