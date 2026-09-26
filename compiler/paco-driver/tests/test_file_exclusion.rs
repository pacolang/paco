use paco_driver::run_program;

/// A project with `main.paco` at the root and `src/amount.paco`/
/// `src/amount_test.paco` beside it; `main.paco` only `use`s `src::amount`,
/// never `src::amount_test`.
fn project(with_test_file: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/amount.paco"), "pub fn value() -> i64 { 42 }\n").unwrap();
    if with_test_file {
        // An unreferenced helper: if this file were ever discovered and
        // compiled, its distinct name would be visible to nothing, but a
        // compile error (e.g. a clashing definition) would still surface.
        std::fs::write(dir.path().join("src/amount_test.paco"), "fn value() -> i64 { 0 - 1 }\n").unwrap();
    }
    let entry = dir.path().join("main.paco");
    std::fs::write(&entry, "use src::amount;\n\nfn main() {\n    print(amount::value());\n}\n").unwrap();
    (dir, entry)
}

/// `discover_used_files`/`discover_one`'s existing `use`-reachability rule
/// only walks a file's `use` declarations; `main.paco` never `use`s
/// `src::amount_test`, so it is never parsed or compiled, exactly as if it
/// were absent. `run_program`'s captured stdout and exit code are the
/// observable proof (byte-identical binaries are not compared, since a
/// build can embed build-specific metadata such as paths or timestamps).
#[test]
fn an_unreferenced_test_paco_file_under_src_does_not_change_run_behavior() {
    let (_without_dir, without_entry) = project(false);
    let (without_output, without_status) = run_program(&without_entry, &[]).unwrap();

    let (_with_dir, with_entry) = project(true);
    let (with_output, with_status) = run_program(&with_entry, &[]).unwrap();

    assert_eq!(without_status, Some(0), "{}", without_output.stderr);
    assert_eq!(with_status, without_status);
    assert_eq!(with_output.stdout, without_output.stdout);
    assert_eq!(with_output.stdout, "42\n");
}
