use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

#[test]
fn run_reads_struct_field() {
    let source = r#"
struct Point { x: i64, y: i64 }

fn main() {
    let p = Point { x: 2, y: 3 };
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
struct Point { x: i64, y: i64 }

fn main() {
    let mut p = Point { x: 2, y: 3 };
    p.x = 5;
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
    x: i64,
    y: i64,

    fn sum(&self) -> i64 {
        self.x + self.y
    }
}

fn main() {
    let p = Point { x: 4, y: 5 };
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
    value: i64,

    fn inc(&mut self) {
        self.value = self.value + 1
    }
}

fn main() {
    let mut c = Counter { value: 1 };
    c.inc();
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
    x: i64,
    y: i64,

    fn origin() -> Point {
        Point { x: 0, y: 0 }
    }
}

fn main() {
    let p = Point::origin();
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
enum Maybe { Some(i64), None }

fn main() {
    let value: Maybe = Maybe::Some(1);
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
    let b = Box<i64> { value: 7 };
    print(b.value)
}
"#;
    let file = write_temp_paco("generic_struct", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "7\n");
}

#[test]
fn run_calls_a_method_through_a_borrowed_receiver() {
    let source = r#"
fn count(v: &Vec<i64>) -> i64 {
    v.len()
}

fn main() {
    let mut v: Vec<i64> = Vec::new();
    v.push(1);
    print(count(&v))
}
"#;
    let file = write_temp_paco("borrowed_receiver_method", source);
    let output = run(Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap()).unwrap();
    assert_eq!(output.stdout, "1\n");
}

#[test]
fn check_rejects_a_mutating_method_through_a_shared_borrow() {
    let source = r#"
fn grow(v: &Vec<i64>) {
    v.push(1)
}

fn main() {
    let v: Vec<i64> = Vec::new();
    grow(&v)
}
"#;
    let file = write_temp_paco("shared_borrow_mutating_method", source);
    let error = run(Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap()).unwrap_err();
    assert!(error.contains("PACO-E0307"), "{error}");
}

#[test]
fn run_zero_fills_a_generic_slice_with_the_explicit_type_argument() {
    let source = r#"
struct Grid<T> {
    cells: []T,

    pub fn zeros(n: i64) -> Self {
        Grid { cells: slice_of_zeros<T>(n) }
    }
}

fn main() {
    let g = Grid<float>::zeros(2);
    print(g.cells[1] + 0.5)
}
"#;
    let file = write_temp_paco("generic_zero_fill", source);
    let output = run(Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap()).unwrap();
    assert_eq!(output.stdout, "0.5\n");
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_data_types_{}_{}_{}.paco",
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
