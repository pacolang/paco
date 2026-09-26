//! Gradients end to end: `grad` sites expanded before code generation, one
//! backend per build, the example and the diagnostics of every
//! non-differentiable construct under `paco check`, `paco run` and
//! `paco build`.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

fn conformance(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/autodiff").join(case)
}

fn scratch(case: &Path) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(case).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|ext| ext == "paco") {
            fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
        }
    }
    dir
}

#[test]
fn a_grad_site_calls_the_generated_primal_and_pullback_not_the_stub() {
    use object::{Object, ObjectSymbol};
    let dir = scratch(&conformance("scalar_gradient"));
    let input = dir.path().join("input.paco");
    run(Cli::try_parse_from(["paco", "build", input.to_str().unwrap()]).unwrap()).unwrap();
    assert_eq!(paco_driver::last_object_count(), 1, "a dev build links only the Cranelift object");
    let data = fs::read(input.with_extension(std::env::consts::EXE_EXTENSION)).unwrap();
    let binary = object::File::parse(&*data).unwrap();
    let prefix = if binary.format() == object::BinaryFormat::MachO { "_" } else { "" };
    let names: Vec<String> =
        binary.symbols().filter_map(|symbol| symbol.name().ok()?.strip_prefix(prefix).map(str::to_string)).collect();
    assert!(names.iter().any(|name| name == "cube__primal_1"), "{names:?}");
    assert!(names.iter().any(|name| name == "cube__pullback_1"), "{names:?}");
    assert!(!names.iter().any(|name| name.contains("autodiff::grad")), "the `grad` stub is never called: {names:?}");
}

/// Each case, the code it reports and the source text its primary span
/// starts with.
const REJECTED: [(&str, &str, &str); 7] = [
    ("extern_without_derivative", "PACO-E0810", "cbrt(x)"),
    ("task_boundary", "PACO-E0811", "spawn square(x)"),
    ("indirect_call", "PACO-E0812", "scale(x)"),
    ("struct_valued_result", "PACO-E0813", "scaled"),
    ("ambiguous_in_out", "PACO-E0813", "fn swap"),
    ("wrong_derivative_signature", "PACO-E0814", "#[derivative(of = twice)]"),
    ("tangent_not_self_without_derivative", "PACO-E0815", "line.w * x + line.b"),
];

fn primary_text(error: &str, source: &str) -> String {
    let location = error.split_whitespace().nth(1).unwrap();
    let mut parts = location.trim_end_matches(':').rsplit(':');
    let column: usize = parts.next().unwrap().parse().unwrap();
    let line: usize = parts.next().unwrap().parse().unwrap();
    source.lines().nth(line - 1).unwrap()[column - 1..].to_string()
}

#[test]
fn every_non_differentiable_construct_is_rejected_by_check_run_and_build() {
    for (case, code, text) in REJECTED {
        let dir = scratch(&conformance(case));
        let input = dir.path().join("input.paco");
        let source = fs::read_to_string(&input).unwrap();
        let expected = fs::read_to_string(conformance(case).join("expected.stderr")).unwrap().replace("\r\n", "\n");
        for command in ["check", "run", "build"] {
            let result = run(Cli::try_parse_from(["paco", command, input.to_str().unwrap()]).unwrap());
            let error = match (command, result) {
                ("check", Ok(output)) => output.stderr,
                (_, Err(error)) => error,
                (_, Ok(_)) => panic!("`paco {command}` accepted {case}"),
            };
            let error = paco_test_harness::strip_dir(&error, dir.path());
            assert!(error.starts_with(&format!("error[{code}]")), "{case} under `paco {command}`: {error}");
            assert!(primary_text(&error, &source).starts_with(text), "{case} under `paco {command}`: {error}");
            assert_eq!(error.trim_end(), expected.trim_end(), "{case} under `paco {command}`");
            assert!(!input.with_extension(std::env::consts::EXE_EXTENSION).exists(), "{case} under `paco {command}` produced a binary");
        }
    }
}

#[test]
fn every_diagnostic_names_the_calls_from_the_grad_site() {
    let expected = fs::read_to_string(conformance("extern_without_derivative").join("expected.stderr")).unwrap();
    let notes: Vec<&str> = expected.lines().filter(|line| line.contains("reached through this call")).collect();
    assert_eq!(notes.len(), 2, "{expected}");
    assert!(notes[0].contains("in `main`") && notes[1].contains("in `f`"), "{expected}");
}
