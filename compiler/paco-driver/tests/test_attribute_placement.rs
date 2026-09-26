use clap::Parser;
use paco_driver::{run, Cli};

/// unit-testing tasks 2.1/2.2: `#[test]` is only meaningful inside a
/// `_test.paco` file (`is_test_file`); anywhere else it is `PACO-E1002`,
/// naming the function and its file.
#[test]
fn test_attribute_outside_a_test_file_is_paco_e1002() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("amount.paco");
    std::fs::write(
        &input,
        r#"#[test]
fn check_it() {
    print(1);
}
"#,
    )
    .unwrap();

    let error = run(Cli::try_parse_from(["paco", "check", input.to_str().unwrap()]).unwrap()).unwrap_err();

    assert!(error.contains("PACO-E1002"), "{error}");
    assert!(error.contains("check_it"), "{error}");
    assert!(error.contains("amount.paco"), "{error}");
}

/// 2.1's positive case, and 2.2(a)/(b): inside a `_test.paco` file, a
/// `#[test]`-tagged function is not flagged, and a plain helper function
/// (no `#[test]`) compiles and is callable from it with no error — it needs
/// no `#[test]` of its own to be usable.
///
/// Deferred to Section 5 (once `paco test`'s discovery/reporting exists,
/// task 3.1 already deferred its own not-yet-buildable half the same way):
/// asserting that the helper does NOT appear in a test run's report. No
/// test-execution mechanism exists yet to observe that against.
#[test]
fn test_attribute_inside_a_test_file_is_accepted_and_helper_needs_no_attribute() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("amount_test.paco");
    std::fs::write(
        &input,
        r#"fn helper() -> i64 {
    41 + 1
}

#[test]
fn check_it() {
    print(helper());
}
"#,
    )
    .unwrap();

    let output = run(Cli::try_parse_from(["paco", "check", input.to_str().unwrap()]).unwrap())
        .unwrap_or_else(|error| panic!("expected no PACO-E1002 (or any other) error, got: {error}"));

    assert!(output.stderr.is_empty(), "{}", output.stderr);
}
