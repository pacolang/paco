//! `stdlib::test`'s nine `#[builtin(name)]` assertion functions, end to end
//! (`unit-testing`'s tasks 3.2-3.5): a passing call is silent, a failing
//! call panics naming the source expression(s) and value(s)/variant, and an
//! optional trailing message is included in the panic text.

use paco_driver::run_program;

fn run_status(source: &str) -> (String, String, Option<i32>) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.paco");
    std::fs::write(&file, source).unwrap();
    let (output, status) = run_program(&file, &[]).unwrap();
    (output.stdout, output.stderr, status)
}

/// Runs `body` inside `fn main()` (with `stdlib::test` already `use`d) and
/// asserts it produced no panic: `print("ok")` after `body` is the
/// observable proof nothing stopped execution early.
fn assert_passes(body: &str) {
    let source = format!("use stdlib::test;\n\nfn main() {{\n{body}\n    print(\"ok\");\n}}\n");
    let (stdout, stderr, status) = run_status(&source);
    assert_eq!(status, Some(0), "{stderr}");
    assert_eq!(stdout, "ok\n");
}

/// Runs `body` inside `fn main()` and asserts it panicked, returning stderr.
fn assert_fails(body: &str) -> String {
    let source = format!("use stdlib::test;\n\nfn main() {{\n{body}\n}}\n");
    let (_, stderr, status) = run_status(&source);
    assert_ne!(status, Some(0), "expected the program to panic; stderr:\n{stderr}");
    stderr
}

#[test]
fn assert_does_nothing_observable_when_its_condition_holds() {
    assert_passes("    test::assert(1 == 1);");
}

#[test]
fn assert_names_the_failing_expression_and_both_values() {
    let stderr = assert_fails(
        "    let x = 1;\n    let y = 2;\n    test::assert(x == y);",
    );
    assert!(stderr.contains("x == y"), "{stderr}");
    assert!(stderr.contains('1'), "{stderr}");
    assert!(stderr.contains('2'), "{stderr}");
}

#[test]
fn assert_true_names_the_failing_expression() {
    let stderr = assert_fails("    let flag = false;\n    test::assert_true(flag);");
    assert!(stderr.contains("flag"), "{stderr}");
}

#[test]
fn assert_true_does_nothing_observable_when_its_condition_holds() {
    assert_passes("    test::assert_true(true);");
}

#[test]
fn assert_false_names_the_failing_expression() {
    let stderr = assert_fails("    let flag = true;\n    test::assert_false(flag);");
    assert!(stderr.contains("flag"), "{stderr}");
}

#[test]
fn assert_false_does_nothing_observable_when_its_condition_holds() {
    assert_passes("    test::assert_false(false);");
}

#[test]
fn assert_eq_reports_both_expressions_and_values_on_failure() {
    let stderr = assert_fails("    let a = 3;\n    let b = 4;\n    test::assert_eq(a, b);");
    assert!(stderr.contains('a'), "{stderr}");
    assert!(stderr.contains('b'), "{stderr}");
    assert!(stderr.contains('3'), "{stderr}");
    assert!(stderr.contains('4'), "{stderr}");
}

#[test]
fn assert_eq_does_nothing_observable_when_equal() {
    assert_passes("    test::assert_eq(1, 1);");
}

#[test]
fn assert_ne_fails_when_the_values_are_equal() {
    let stderr = assert_fails("    test::assert_ne(5, 5);");
    assert!(stderr.contains('5'), "{stderr}");
}

#[test]
fn assert_ne_does_nothing_observable_when_different() {
    assert_passes("    test::assert_ne(1, 2);");
}

#[test]
fn assert_some_names_the_expression_and_that_it_held_none() {
    let stderr = assert_fails("    let opt: Option<i64> = Option::None;\n    test::assert_some(opt);");
    assert!(stderr.contains("opt"), "{stderr}");
    assert!(stderr.contains("None"), "{stderr}");
}

#[test]
fn assert_some_does_nothing_observable_when_some() {
    assert_passes("    let opt: Option<i64> = Option::Some(1);\n    test::assert_some(opt);");
}

#[test]
fn assert_none_names_the_expression_and_that_it_held_some() {
    let stderr = assert_fails("    let opt: Option<i64> = Option::Some(7);\n    test::assert_none(opt);");
    assert!(stderr.contains("opt"), "{stderr}");
    assert!(stderr.contains("Some"), "{stderr}");
}

#[test]
fn assert_none_does_nothing_observable_when_none() {
    assert_passes("    let opt: Option<i64> = Option::None;\n    test::assert_none(opt);");
}

#[test]
fn assert_ok_names_the_expression_and_the_err_value_it_held() {
    let stderr =
        assert_fails("    let result: Result<i64, string> = Result::Err(\"bad input\");\n    test::assert_ok(result);");
    assert!(stderr.contains("result"), "{stderr}");
    assert!(stderr.contains("Err"), "{stderr}");
    assert!(stderr.contains("bad input"), "{stderr}");
}

#[test]
fn assert_ok_does_nothing_observable_when_ok() {
    assert_passes("    let result: Result<i64, string> = Result::Ok(1);\n    test::assert_ok(result);");
}

#[test]
fn assert_err_names_the_expression_and_the_ok_value_it_held() {
    let stderr = assert_fails("    let result: Result<i64, string> = Result::Ok(42);\n    test::assert_err(result);");
    assert!(stderr.contains("result"), "{stderr}");
    assert!(stderr.contains("Ok"), "{stderr}");
    assert!(stderr.contains("42"), "{stderr}");
}

#[test]
fn assert_err_does_nothing_observable_when_err() {
    assert_passes("    let result: Result<i64, string> = Result::Err(\"bad\");\n    test::assert_err(result);");
}

/// Task 3.5: the optional trailing message argument is included in the
/// panic text for every one of the nine functions.
#[test]
fn optional_message_is_included_in_the_panic_text_for_every_function() {
    let cases: &[&str] = &[
        r#"test::assert(1 == 2, "assert message");"#,
        r#"test::assert_eq(1, 2, "assert_eq message");"#,
        r#"test::assert_ne(1, 1, "assert_ne message");"#,
        r#"test::assert_true(false, "assert_true message");"#,
        r#"test::assert_false(true, "assert_false message");"#,
        r#"let opt: Option<i64> = Option::None; test::assert_some(opt, "assert_some message");"#,
        r#"let opt: Option<i64> = Option::Some(1); test::assert_none(opt, "assert_none message");"#,
        r#"let result: Result<i64, string> = Result::Err("e"); test::assert_ok(result, "assert_ok message");"#,
        r#"let result: Result<i64, string> = Result::Ok(1); test::assert_err(result, "assert_err message");"#,
    ];
    for case in cases {
        let stderr = assert_fails(&format!("    {case}"));
        let expected = case.split('"').nth(1).unwrap();
        assert!(stderr.contains(expected), "case `{case}`: {stderr}");
    }
}
