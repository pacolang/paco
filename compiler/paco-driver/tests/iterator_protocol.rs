use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

fn temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("paco_iterator_protocol_{}_{}_{}.paco", name, std::process::id(), monotonic_suffix()));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

const COUNTER_SOURCE: &str = r#"
enum Option<T> {
    Some(T),
    None,
}

trait Iter {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
}

struct Counter {
    current: i64,
    limit: i64,

    fn next(&mut self) -> Option<i64> {
        if self.current >= self.limit {
            return Option::None
        }
        let value = self.current;
        self.current = value + 1;
        Option::Some(value)
    }
}

fn main() {
    let c = Counter { current: 1, limit: 4 };
    for x in c {
        print(x)
    }
}
"#;

#[test]
fn for_iterates_a_user_type_implementing_iter() {
    let entry = temp_paco("counter", COUNTER_SOURCE);
    let cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));
    assert_eq!(output.stdout, "1\n2\n3\n");
    let _ = fs::remove_file(&entry);
}

#[test]
fn an_empty_iterator_runs_the_loop_body_zero_times() {
    let entry = temp_paco(
        "empty",
        r#"
enum Option<T> {
    Some(T),
    None,
}

trait Iter {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
}

struct Empty {
    fn next(&mut self) -> Option<i64> {
        Option::None
    }
}

fn main() {
    let e = Empty { };
    for x in e {
        print(x)
    }
    print(0 - 1)
}
"#,
    );
    let cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));
    assert_eq!(output.stdout, "-1\n");
    let _ = fs::remove_file(&entry);
}

#[test]
fn a_type_without_next_is_a_clear_error_naming_the_missing_method() {
    let entry = temp_paco(
        "no_next",
        r#"
struct Plain { value: i64 }

fn main() {
    let p = Plain { value: 1 };
    for x in p {
        print(x)
    }
}
"#,
    );
    let cli = Cli::try_parse_from(["paco", "check", entry.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();
    assert!(error.contains("PACO-E0314"), "{error}");
    assert!(error.contains("next"), "{error}");
    let _ = fs::remove_file(&entry);
}

#[test]
fn range_for_behavior_is_unchanged() {
    let entry = temp_paco("range_unchanged", "fn main() { for i in 0..3 { print(i) } }");
    let cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));
    assert_eq!(output.stdout, "0\n1\n2\n");
    let _ = fs::remove_file(&entry);
}

#[test]
fn continue_inside_a_for_iter_loop_re_evaluates_next_rather_than_skipping_it() {
    let entry = temp_paco(
        "continue",
        r#"
enum Option<T> {
    Some(T),
    None,
}

trait Iter {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
}

struct Counter {
    current: i64,
    limit: i64,

    fn next(&mut self) -> Option<i64> {
        if self.current >= self.limit {
            return Option::None
        }
        let value = self.current;
        self.current = value + 1;
        Option::Some(value)
    }
}

fn main() {
    let c = Counter { current: 1, limit: 5 };
    for x in c {
        if x == 2 {
            continue
        }
        print(x)
    }
}
"#,
    );
    let cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));
    assert_eq!(output.stdout, "1\n3\n4\n");
    let _ = fs::remove_file(&entry);
}
