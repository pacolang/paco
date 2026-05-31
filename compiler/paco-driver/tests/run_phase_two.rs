use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

#[test]
fn run_reads_struct_field() {
    let source = r#"
struct Point { x: int, y: int }

fn main() {
    let p = Point { x: 2, y: 3 }
    print(p.x)
}
"#;
    let file = write_temp_paco("struct_field", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "2\n");
}

#[test]
fn run_assigns_mutable_struct_field() {
    let source = r#"
struct Point { x: int, y: int }

fn main() {
    let mut p = Point { x: 2, y: 3 }
    p.x = 5
    print(p.x)
}
"#;
    let file = write_temp_paco("struct_field_assignment", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "5\n");
}

#[test]
fn run_calls_struct_method() {
    let source = r#"
struct Point {
    x: int,
    y: int,

    fn sum(self&) -> int {
        self.x + self.y
    }
}

fn main() {
    let p = Point { x: 4, y: 5 }
    print(p.sum())
}
"#;
    let file = write_temp_paco("struct_method", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "9\n");
}

#[test]
fn run_writes_back_mutable_self_method_changes() {
    let source = r#"
struct Counter {
    value: int,

    fn inc(self&mut) {
        self.value = self.value + 1
    }
}

fn main() {
    let mut c = Counter { value: 1 }
    c.inc()
    print(c.value)
}
"#;
    let file = write_temp_paco("mutable_self_method", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "2\n");
}

#[test]
fn run_calls_associated_constructor_function() {
    let source = r#"
struct Point {
    x: int,
    y: int,

    fn origin() -> Point {
        Point { x: 0, y: 0 }
    }
}

fn main() {
    let p = Point::origin()
    print(p.x)
}
"#;
    let file = write_temp_paco("associated_constructor", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "0\n");
}

#[test]
fn run_constructs_enum_value_without_matching() {
    let source = r#"
enum Maybe { Some(int), None }

fn main() {
    let value: Maybe = Maybe::Some(1)
}
"#;
    let file = write_temp_paco("enum_construction", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "");
}

#[test]
fn run_instantiates_generic_struct_field() {
    let source = r#"
struct Box<T> { value: T }

fn main() {
    let b = Box<int> { value: 7 }
    print(b.value)
}
"#;
    let file = write_temp_paco("generic_struct", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "7\n");
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_phase_two_{}_{}_{}.paco",
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
