use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

#[test]
fn run_matches_enum_variant_payload() {
    let source = r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(41)
    let result: int = match value {
        Maybe::Some(x) => x + 1,
        Maybe::None => 0,
    }
    print(result)
}
"#;
    let file = write_temp_paco("enum_match", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "42\n");
}

#[test]
fn run_matches_guarded_at_binding_range() {
    let source = r#"
fn main() {
    let result: int = match 3 {
        n @ 1..=9 if n > 2 => n,
        _ => 0,
    }
    print(result)
}
"#;
    let file = write_temp_paco("guarded_range_match", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "3\n");
}

#[test]
fn run_executes_if_let_else() {
    let source = r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(7)
    if let Maybe::Some(x) = value {
        print(x)
    } else {
        print(0)
    }
}
"#;
    let file = write_temp_paco("if_let_else", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "7\n");
}

#[test]
fn run_executes_while_let_until_pattern_miss() {
    let source = r#"
enum Maybe { Some(int), None }

fn next(value: int) -> Maybe {
    if value > 0 {
        Maybe::Some(value)
    } else {
        Maybe::None
    }
}

fn main() {
    let mut current: int = 3
    while let Maybe::Some(n) = next(current) {
        print(n)
        current = current - 1
    }
}
"#;
    let file = write_temp_paco("while_let", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "3\n2\n1\n");
}

#[test]
fn run_executes_inclusive_for_range() {
    let source = r#"
fn main() {
    for n in 1..=3 {
        print(n)
    }
}
"#;
    let file = write_temp_paco("for_range", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "1\n2\n3\n");
}

#[test]
fn run_for_range_continue_advances_to_next_value() {
    let source = r#"
fn main() {
    for n in 1..=3 {
        if n == 2 {
            continue
        }
        print(n)
    }
}
"#;
    let file = write_temp_paco("for_range_continue", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "1\n3\n");
}

#[test]
fn run_rejects_assignment_to_for_range_binding() {
    let source = r#"
fn main() {
    for n in 1..=3 {
        n = 99
    }
}
"#;
    let file = write_temp_paco("for_range_immutable_binding", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("immutable binding"));
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_pattern_matching_{}_{}_{}.paco",
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
