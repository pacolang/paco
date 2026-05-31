use std::fs;

use paco_test_harness::{GoldenStatus, TestKind, discover_golden_tests};

#[test]
fn discovers_and_skips_tests_above_current_phase() {
    let temp = tempfile::tempdir().unwrap();
    let test_dir = temp.path().join("phase_01").join("future");
    fs::create_dir_all(&test_dir).unwrap();
    fs::write(test_dir.join("input.paco"), "fn main() {}\n").unwrap();
    fs::write(test_dir.join("expected.stdout"), "").unwrap();
    fs::write(
        test_dir.join("flags.toml"),
        "kind = \"run\"\nphase_min = 2\n",
    )
    .unwrap();

    let tests = discover_golden_tests(temp.path(), 1).unwrap();

    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].kind, TestKind::Run);
    assert_eq!(tests[0].status, GoldenStatus::Skipped { phase_min: 2 });
}

#[test]
fn accepts_empty_conformance_tree() {
    let temp = tempfile::tempdir().unwrap();

    let tests = discover_golden_tests(temp.path(), 0).unwrap();

    assert!(tests.is_empty());
}
