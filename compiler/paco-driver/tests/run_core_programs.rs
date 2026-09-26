use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, DriverOutput, run};

#[test]
fn run_executes_hello_world_program() {
    let file = write_temp_paco("hello", "fn main() { print(\"Hello, world!\") }");
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: "Hello, world!\n".to_string(),
            stderr: String::new(),
        }
    );
}

#[test]
fn run_executes_recursive_factorial_program() {
    let source = r#"
fn fact(n: i64) -> i64 {
    if n == 0 { 1 } else { n * fact(n - 1) }
}

fn main() {
    print(fact(10))
}
"#;
    let file = write_temp_paco("factorial", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "3628800\n");
    assert_eq!(output.stderr, "");
}

#[test]
fn run_reports_primitive_type_mismatch_before_evaluation() {
    let file = write_temp_paco("type_mismatch", "fn main() { print(1 + \"x\") }");
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0334"));
    assert!(error.contains("has no `add` method"));
}

#[test]
fn run_reports_undeclared_name_before_evaluation() {
    let file = write_temp_paco("undeclared", "fn main() { print(missing) }");
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0201"));
    assert!(error.contains("name not found"));
}

#[test]
fn run_reports_early_return_type_mismatch_before_evaluation() {
    let file = write_temp_paco("early_return_mismatch", "fn main() { return true; }");
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0302"));
    assert!(error.contains("return type mismatch"));
}

#[test]
fn run_reports_assignment_type_mismatch_before_evaluation() {
    let file = write_temp_paco(
        "assignment_mismatch",
        "fn main() { let mut value: i64 = 1; value = true; print(value) }",
    );
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0302"));
    assert!(error.contains("type mismatch"));
}

#[test]
fn run_accepts_return_statement_as_function_result() {
    let source = r#"
fn value() -> i64 {
    return 1;
}

fn main() {
    print(value())
}
"#;
    let file = write_temp_paco("statement_return", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "1\n");
}

#[test]
fn run_propagates_return_from_expression_context() {
    let source = r#"
fn value() -> i64 {
    let ignored = return 1;
    2
}

fn main() {
    print(value())
}
"#;
    let file = write_temp_paco("return_expression", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "1\n");
}

#[test]
fn run_reports_assignment_to_immutable_binding_before_evaluation() {
    let file = write_temp_paco(
        "immutable_assignment",
        "fn main() { let value: i64 = 1; value = 2; print(value) }",
    );
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0307"));
    assert!(error.contains("immutable binding"));
}

#[test]
fn run_reports_unsupported_ordering_before_evaluation() {
    let file = write_temp_paco("unsupported_ordering", "fn main() { print(\"a\" < \"b\") }");
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0301"));
    assert!(error.contains("type mismatch"));
}

#[test]
fn run_reports_break_outside_loop_before_evaluation() {
    let file = write_temp_paco("break_outside_loop", "fn main() { break; }");
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0308"));
    assert!(error.contains("outside of a loop"));
}

#[test]
fn run_reports_integer_overflow_as_a_located_panic() {
    let file = write_temp_paco(
        "integer_overflow",
        "fn main() { print(9223372036854775807 + 1) }",
    );
    let (output, status) = paco_driver::run_program(&file, &[]).unwrap();

    assert_eq!(status, Some(101));
    assert!(output.stderr.starts_with(&format!("panic at {}:1:19: attempt to add with overflow\n", file.display())), "{}", output.stderr);
    assert_eq!(output.stdout, "");
}

#[test]
fn run_accepts_return_statement_inside_if_branch() {
    let source = r#"
fn choose(flag: bool) -> i64 {
    if flag {
        return 1;
    }
    return 2;
}

fn main() {
    print(choose(false));
    print(choose(true))
}
"#;
    let file = write_temp_paco("if_branch_return", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "2\n1\n");
}

#[test]
fn run_print_err_writes_to_stderr_not_stdout() {
    let source = r#"
use stdlib::io;

fn main() {
    io::print_err("oops");
    print("stdout line")
}
"#;
    let file = write_temp_paco("print_err", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "stdout line\n");
    assert_eq!(output.stderr, "oops\n");
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_core_programs_{}_{}_{}.paco",
        name,
        std::process::id(),
        monotonic_suffix()
    ));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
