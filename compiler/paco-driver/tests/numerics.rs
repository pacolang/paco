//! `tests/conformance/numerics` under `paco build`: every `run` case's
//! binary prints what `paco run` does, and every `fail` case is rejected
//! with its diagnostic and leaves no binary.

use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn cases() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/numerics");
    let mut cases: Vec<PathBuf> = fs::read_dir(root).unwrap().map(|entry| entry.unwrap().path()).collect();
    cases.sort();
    cases
}

fn copy_to_temp(case: &std::path::Path) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let name = case.file_name().unwrap().to_string_lossy();
    let path = std::env::temp_dir().join(format!("paco_numerics_{name}_{}_{nanos}.paco", std::process::id()));
    fs::copy(case.join("input.paco"), &path).unwrap();
    path
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn run_cases_build_and_print_their_expected_output() {
    for case in cases().into_iter().filter(|case| case.join("expected.stdout").exists() && !case.join("expected.stderr").exists()) {
        let file = copy_to_temp(&case);
        run(Cli::try_parse_from(["paco", "build", file.to_str().unwrap()]).unwrap())
            .unwrap_or_else(|error| panic!("`paco build` failed for {}: {error}", case.display()));
        let binary = file.with_extension(std::env::consts::EXE_EXTENSION);
        let output = Command::new(&binary).output().unwrap();
        let _ = fs::remove_file(&binary);
        let _ = fs::remove_file(&file);
        assert!(output.status.success(), "{} exited with {:?}", case.display(), output.status);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).replace(&file.display().to_string(), "input.paco"),
            fs::read_to_string(case.join("expected.stdout")).unwrap(),
            "compiled stdout diverged for {}",
            case.display()
        );
    }
}

#[test]
fn fail_cases_are_rejected_by_build_without_a_binary() {
    for case in cases().into_iter().filter(|case| case.join("expected.stderr").exists() && !case.join("expected.stdout").exists()) {
        let file = copy_to_temp(&case);
        let error = run(Cli::try_parse_from(["paco", "build", file.to_str().unwrap()]).unwrap())
            .expect_err(&format!("{} should not build", case.display()));
        let binary = file.with_extension(std::env::consts::EXE_EXTENSION);
        assert!(!binary.exists(), "{} produced a binary", case.display());
        let _ = fs::remove_file(&file);
        let expected = fs::read_to_string(case.join("expected.stderr")).unwrap();
        let code = expected.split(['[', ']']).nth(1).expect("expected.stderr names a code");
        assert!(error.contains(code), "{}: expected {code}, got {error}", case.display());
    }
}

#[test]
fn check_reports_a_static_shape_mismatch_at_the_call() {
    let case = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/numerics/shape_mismatch_static");
    let file = copy_to_temp(&case);
    let result = run(Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap());
    let _ = fs::remove_file(&file);
    let error = result.expect_err("a shape mismatch must fail `paco check`");
    assert!(error.contains("PACO-E0336"), "{error}");
    assert!(error.contains(":15:11: shape mismatch: dimension 0 expected `768`, found `512`"), "{error}");
}

#[test]
fn a_dim_parameter_is_instantiated_once_for_every_extent() {
    use object::{Object, ObjectSymbol};
    let case = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/numerics/dim_param_runtime_extents");
    let file = copy_to_temp(&case);
    run(Cli::try_parse_from(["paco", "build", file.to_str().unwrap()]).unwrap()).unwrap();
    let binary = file.with_extension(std::env::consts::EXE_EXTENSION);
    let data = fs::read(&binary).unwrap();
    let _ = fs::remove_file(&binary);
    let _ = fs::remove_file(&file);
    let object = object::File::parse(&*data).unwrap();
    let prefix = if object.format() == object::BinaryFormat::MachO { "_" } else { "" };
    let instances: Vec<String> = object
        .symbols()
        .filter_map(|symbol| symbol.name().ok()?.strip_prefix(prefix).map(str::to_string))
        .filter(|name| name.starts_with("rows::"))
        .collect();
    assert_eq!(instances, vec!["rows::dyn".to_string()], "extents 1, 7, 300 and a static 7 share one instance");
}
