use clap::Parser;
use paco_driver::{run, Cli};

/// `stdlib/test.paco` (task 3.1 of the `unit-testing` change) declares each
/// assertion function's trailing `message` parameter as an ordinary,
/// required `string` — the only form ordinary call-site type checking can
/// validate before `#[builtin]` call-site handling (tasks 3.2-3.4) exists.
/// This test proves that 2-argument (with-message) form type-checks for
/// every one of the nine functions.
///
/// The 1-argument (no-message) form — e.g. `assert_eq(a, b)` — is NOT
/// exercised here and cannot type-check yet under ordinary signature
/// checking, since `stdlib/test.paco`'s written signature requires `message`.
/// It becomes valid only once tasks 3.2-3.4 add `#[builtin]` call-site
/// handling that reads a call's raw argument list from the AST and accepts
/// either arity, bypassing this written signature (mirroring `grad`'s own
/// `infer_grad_call` special case). Proving the 1-argument form compiles is
/// deferred to task 3.5, which runs after that handling lands.
#[test]
fn every_assertion_functions_two_argument_form_type_checks() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.paco");
    std::fs::write(
        &input,
        r#"use stdlib::test;

fn main() {
    let n: i64 = 1;
    test::assert(true, "cond message");
    test::assert_eq(n, n, "eq message");
    test::assert_ne(n, n, "ne message");
    test::assert_true(true, "true message");
    test::assert_false(false, "false message");
    let some_opt: Option<i64> = Option::Some(n);
    test::assert_some(some_opt, "some message");
    let none_opt: Option<i64> = Option::None;
    test::assert_none(none_opt, "none message");
    let ok_result: Result<i64, string> = Result::Ok(n);
    test::assert_ok(ok_result, "ok message");
    let err_result: Result<i64, string> = Result::Err("boom");
    test::assert_err(err_result, "err message");
}
"#,
    )
    .unwrap();

    let output = run(Cli::try_parse_from(["paco", "check", input.to_str().unwrap()]).unwrap())
        .unwrap_or_else(|error| panic!("`paco check` failed: {error}"));
    assert!(output.stderr.is_empty(), "{}", output.stderr);
}
