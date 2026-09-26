use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    dir.push(format!("paco_test_command_{name}_{}_{suffix}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// unit-testing task 5.1: discovery finds every `_test.paco` file in the
/// project (both a `src/`-style colocated one and a `tests/`-located one),
/// and does not touch a `_test.paco` file that sits outside the project
/// directory entirely.
#[test]
fn discovery_finds_every_test_file_in_the_project_and_nothing_outside_it() {
    let dir = temp_dir("discovery");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/amount.paco"), "pub fn value() -> i64 { 1 }\n").unwrap();
    fs::write(
        dir.join("src/amount_test.paco"),
        "use stdlib::test;\n\n#[test]\nfn colocated_case() {\n    test::assert_eq(value(), 1, \"m\");\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("tests/integration_test.paco"),
        "use src::amount;\n\n#[test]\nfn integration_case() {\n    if amount::value() != 1 {\n        panic(\"bad\");\n    }\n}\n",
    )
    .unwrap();

    // A `_test.paco` file that sits entirely outside this project's own
    // directory: if discovery ever reached it, its distinct, impossible-to-
    // pass assertion would show up in the report below.
    let outside = temp_dir("discovery_outside");
    fs::write(outside.join("rogue_test.paco"), "#[test]\nfn rogue() {\n    panic(\"should never run\");\n}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "test", dir.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("expected every test to pass: {error}"));

    assert!(output.stdout.contains("colocated_case"), "{}", output.stdout);
    assert!(output.stdout.contains("integration_case"), "{}", output.stdout);
    assert!(!output.stdout.contains("rogue"), "{}", output.stdout);
    assert!(output.stdout.contains("2 passed; 0 failed"), "{}", output.stdout);

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&outside);
}

/// unit-testing task 5.2 / spec scenario "One panicking test does not stop
/// the others from running": three `#[test]` functions, one panics, all
/// three are still reported, the panicking one as a failure.
#[test]
fn a_panicking_test_does_not_stop_its_siblings_from_being_reported() {
    let dir = temp_dir("panic_siblings");
    fs::write(
        dir.join("three_test.paco"),
        r#"#[test]
fn first() {
}

#[test]
fn second() {
    panic("boom");
}

#[test]
fn third() {
}
"#,
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "test", dir.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();

    assert!(error.contains("first ... ok"), "{error}");
    assert!(error.contains("second ... FAILED"), "{error}");
    assert!(error.contains("third ... ok"), "{error}");
    assert!(error.contains("boom"), "{error}");
    assert!(error.contains("2 passed; 1 failed"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

/// unit-testing task 5.3: exact reported counts and a non-zero exit
/// (surfaced here as `run`'s `Err`, the same convention `paco run` already
/// uses for a nonzero program exit) for a fixture with a known pass/fail
/// mix, plus the failing test's panic message shown.
#[test]
fn reports_exact_counts_and_the_failing_panic_message_and_fails_the_command() {
    let dir = temp_dir("counts");
    fs::write(
        dir.join("mix_test.paco"),
        r#"use stdlib::test;

#[test]
fn passes() {
    test::assert_eq(1, 1, "ok");
}

#[test]
fn fails() {
    test::assert_eq(1, 2, "one is not two");
}
"#,
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "test", dir.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();

    assert!(error.contains("running 2 tests"), "{error}");
    assert!(error.contains("fails ... FAILED"), "{error}");
    assert!(error.contains("passes ... ok"), "{error}");
    assert!(error.contains("one is not two"), "{error}");
    assert!(error.contains("test result: FAILED. 1 passed; 1 failed"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

/// unit-testing task 5.3, the passing half: a project where every test
/// passes reports zero failures and succeeds (`run` returns `Ok`).
#[test]
fn a_project_where_every_test_passes_succeeds_with_zero_failures() {
    let dir = temp_dir("all_pass");
    fs::write(dir.join("ok_test.paco"), "#[test]\nfn works() {\n}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "test", dir.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("expected success: {error}"));

    assert!(output.stdout.contains("test result: ok. 1 passed; 0 failed"), "{}", output.stdout);

    let _ = fs::remove_dir_all(&dir);
}

/// unit-testing task 5.4 / spec scenario "Filtering by name": only test
/// functions whose name contains the given substring run.
#[test]
fn filtering_by_name_substring_only_runs_matching_test_functions() {
    let dir = temp_dir("filter");
    fs::write(
        dir.join("many_test.paco"),
        r#"#[test]
fn alpha_case() {
}

#[test]
fn beta_case() {
    panic("beta should never run when filtered out");
}
"#,
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "test", dir.to_str().unwrap(), "alpha"]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("expected only the matching test to run: {error}"));

    assert!(output.stdout.contains("running 1 test\n"), "{}", output.stdout);
    assert!(output.stdout.contains("alpha_case"), "{}", output.stdout);
    assert!(!output.stdout.contains("beta_case"), "{}", output.stdout);

    let _ = fs::remove_dir_all(&dir);
}

/// unit-testing task 5.5: `paco test` wired end-to-end through
/// `Commands::Test`, on a real fixture project, on both `paco build`'s
/// backends (skipped for LLVM in a build without the `llvm` feature).
#[test]
fn wired_end_to_end_on_a_real_fixture_project_on_both_backends() {
    let dir = temp_dir("wired");
    fs::write(
        dir.join("wired_test.paco"),
        "use stdlib::test;\n\n#[test]\nfn check() {\n    test::assert_eq(1 + 1, 2, \"m\");\n}\n",
    )
    .unwrap();

    let cranelift = Cli::try_parse_from(["paco", "test", dir.to_str().unwrap()]).unwrap();
    let cranelift_output = run(cranelift).unwrap_or_else(|error| panic!("cranelift `paco test` failed: {error}"));
    assert!(cranelift_output.stdout.contains("test result: ok. 1 passed; 0 failed"), "{}", cranelift_output.stdout);

    if cfg!(feature = "llvm") {
        let llvm = Cli::try_parse_from(["paco", "test", "--backend", "llvm", dir.to_str().unwrap()]).unwrap();
        let llvm_output = run(llvm).unwrap_or_else(|error| panic!("llvm `paco test` failed: {error}"));
        assert!(llvm_output.stdout.contains("test result: ok. 1 passed; 0 failed"), "{}", llvm_output.stdout);
    }

    let _ = fs::remove_dir_all(&dir);
}

/// unit-testing task 4.1's own visibility rule, exercised for the first
/// time through `paco test` itself (not just `check_program_at` directly):
/// a `tests/`-located test file calling a private project function gets
/// the same visibility error an external consumer would, not success.
#[test]
fn a_tests_directory_file_calling_a_private_function_is_a_visibility_error() {
    let dir = temp_dir("tests_visibility");
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("amount.paco"), "fn helper() -> i64 { 1 }\n").unwrap();
    fs::write(
        dir.join("tests/checkout_test.paco"),
        "use amount;\n\n#[test]\nfn calls_private() {\n    print(amount::helper());\n}\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "test", dir.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0333"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

/// Section 6 smoke-test fixture (`src/foo.paco` + `src/foo_test.paco` +
/// `tests/bar_test.paco`): a project that actually organizes its own code
/// under a `src/` directory (rather than flat at the project root, which
/// `a_tests_directory_file_calling_a_private_function_is_a_visibility_error`
/// above already covers), exercised together: a colocated test seeing a
/// private item, and an integration test limited to `pub` items reached
/// through an ordinary qualified path (`use src::foo;` — `tests/`-located
/// `use` resolution is rooted at the project directory, so a nested `src/`
/// subdirectory is just an ordinary Plain-path segment, matching
/// `discovery_finds_every_test_file_in_the_project_and_nothing_outside_it`'s
/// own `use src::amount;` above — no special-casing of the name `src`).
#[test]
fn a_project_using_a_src_directory_resolves_tests_use_paths_through_it() {
    let dir = temp_dir("src_dir");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(
        dir.join("src/foo.paco"),
        "pub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n\nfn secret() -> i64 {\n    99\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/foo_test.paco"),
        "use stdlib::test;\n\n#[test]\nfn sees_private_secret() {\n    test::assert_eq(secret(), 99, \"colocated sees private fn\");\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("tests/bar_test.paco"),
        "use src::foo;\n\n#[test]\nfn uses_only_pub_items() {\n    if foo::add(10, 20) != 30 {\n        panic(\"integration test failed\");\n    }\n}\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "test", dir.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("expected both tests to pass: {error}"));

    assert!(output.stdout.contains("sees_private_secret"), "{}", output.stdout);
    assert!(output.stdout.contains("uses_only_pub_items"), "{}", output.stdout);
    assert!(output.stdout.contains("2 passed; 0 failed"), "{}", output.stdout);

    let _ = fs::remove_dir_all(&dir);
}
