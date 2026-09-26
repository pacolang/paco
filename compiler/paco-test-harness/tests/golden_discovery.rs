use std::fs;

use paco_test_harness::{GoldenStatus, TestKind, discover_golden_tests};

#[test]
fn discovers_and_skips_tests_above_current_feature_level() {
    let temp = tempfile::tempdir().unwrap();
    let test_dir = temp.path().join("core").join("future");
    fs::create_dir_all(&test_dir).unwrap();
    fs::write(test_dir.join("input.paco"), "fn main() {}\n").unwrap();
    fs::write(test_dir.join("expected.stdout"), "").unwrap();
    fs::write(
        test_dir.join("flags.toml"),
        "kind = \"run\"\nfeature_min = 2\n",
    )
    .unwrap();

    let tests = discover_golden_tests(temp.path(), 1).unwrap();

    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].kind, TestKind::Run);
    assert_eq!(tests[0].status, GoldenStatus::Skipped { feature_min: 2 });
}

#[test]
fn accepts_empty_conformance_tree() {
    let temp = tempfile::tempdir().unwrap();

    let tests = discover_golden_tests(temp.path(), 0).unwrap();

    assert!(tests.is_empty());
}

#[test]
fn reads_the_expected_exit_status_per_profile() {
    let temp = tempfile::tempdir().unwrap();
    let test_dir = temp.path().join("runtime").join("panics");
    fs::create_dir_all(&test_dir).unwrap();
    fs::write(test_dir.join("flags.toml"), "kind = \"run\"\nexit = 101\nrelease_exit = 0\n").unwrap();
    let plain_dir = temp.path().join("runtime").join("plain");
    fs::create_dir_all(&plain_dir).unwrap();
    fs::write(plain_dir.join("flags.toml"), "kind = \"run\"\nexit = 101\n").unwrap();

    let tests = discover_golden_tests(temp.path(), 1).unwrap();

    assert_eq!((tests[0].exit, tests[0].release_exit), (101, 0));
    assert_eq!((tests[1].exit, tests[1].release_exit), (101, 101));
}

#[test]
fn release_expectations_override_the_shared_ones() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("case");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("flags.toml"), "kind = \"run\"\nexit = 101\nrelease_exit = 0\n").unwrap();
    fs::write(dir.join("expected.stdout"), "").unwrap();
    fs::write(dir.join("expected.stderr"), "panic\n").unwrap();
    fs::write(dir.join("expected.release.stdout"), "wrapped\n").unwrap();
    let test = discover_golden_tests(temp.path(), 1).unwrap().remove(0);

    let debug = test.expected_run(false);
    assert_eq!((debug.stdout.as_str(), debug.stderr.as_deref(), debug.exit), ("", Some("panic\n"), 101));
    let release = test.expected_run(true);
    assert_eq!((release.stdout.as_str(), release.stderr.as_deref(), release.exit), ("wrapped\n", Some("panic\n"), 0));
}
