use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("paco_iterator_protocol_compiled_{}_{}_{}.paco", name, std::process::id(), monotonic_suffix()));
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
fn a_compiled_program_iterates_a_user_type_implementing_iter() {
    let entry = temp_paco("compiled", COUNTER_SOURCE);

    let build_cli = Cli::try_parse_from(["paco", "build", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));
    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let output = Command::new(&binary_path).output().expect("compiled binary should run");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n2\n3\n");

    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(&entry);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn run_and_release_build_for_over_iter_agree() {
    let entry = temp_paco("differential", COUNTER_SOURCE);

    let run_cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let run_output = run(run_cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));

    let build_cli = Cli::try_parse_from(["paco", "build", "--release", "--backend", "llvm", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));
    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let compiled = Command::new(&binary_path).output().expect("compiled binary should run");

    assert_eq!(String::from_utf8_lossy(&compiled.stdout), run_output.stdout);
    assert!(compiled.status.success());

    let _ = fs::remove_file(&binary_path);
    let _ = fs::remove_file(&entry);
}
