//! `paco run` vs `paco build --release --backend llvm` output parity for `spawn`/`channel`/`.join()`
//! (task 4's thunk-outlining lowering) — the exact differential pattern
//! `differential_corpus.rs` already established in `phase-7-dev-codegen`.

use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn assert_run_matches_release_llvm(name: &str, source: &str) {
    let file = write_temp_paco(name, source);

    let run_cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();
    let run_output = run(run_cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));

    let build_cli = Cli::try_parse_from(["paco", "build", "--release", "--backend", "llvm", file.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));

    let binary_path = file.with_extension(std::env::consts::EXE_EXTENSION);
    let compiled = Command::new(&binary_path)
        .output()
        .expect("compiled binary should run");
    let compiled_stdout = String::from_utf8_lossy(&compiled.stdout).to_string();

    assert_eq!(compiled_stdout, run_output.stdout, "stdout diverged for `{name}`");
    assert!(compiled.status.success(), "`{name}` binary exited with {:?}", compiled.status);

    let _ = fs::remove_file(&binary_path);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn spawn_join_and_print_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "spawn_join_print",
        r#"
        enum Result { Ok(i64), Err(TaskPanic) }

        fn main() {
            let handle = spawn { 40 + 2 };
            match handle.join() {
                Result::Ok(value) => print(value),
                Result::Err(e) => print(-1),
            }
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn spawn_blocking_join_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "spawn_blocking_join",
        r#"
        enum Result { Ok(i64), Err(TaskPanic) }

        fn main() {
            let base = 40;
            let handle = spawn_blocking(|| { base + 2 });
            match handle.join() {
                Result::Ok(value) => print(value),
                Result::Err(e) => print(-1),
            }
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn closure_values_match_run_and_release_build() {
    assert_run_matches_release_llvm(
        "closure_values",
        r#"
        fn main() {
            let base = 10;
            let add = |x: i64, y| x + y + base;
            let negate = |b: bool| !b;
            let say = || print(7);
            print(add(1, 2));
            print(add(3, 4));
            print(negate(false));
            say()
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn spawned_producer_over_a_channel_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "spawn_channel_producer",
        r#"
        enum Result { Ok(i64), Err(RecvError) }

        fn produce(tx: Sender<i64>) {
            let sent = tx.send(42);
        }

        fn main() {
            let (tx, rx) = channel<i64>(capacity: 1);
            let handle = spawn produce(tx);
            match rx.recv() {
                Result::Ok(value) => print(value),
                Result::Err(e) => print(-1),
            }
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn iter_fn_bounded_pull_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "iter_fn_bounded_pull",
        r#"
        enum Option { Some(i64), None }

        iter fn counts_up(start: i64) -> i64 {
            yield start;
            yield start + 1;
            yield start + 2
        }

        fn main() {
            let g = counts_up(10);
            let a = g.next();
            match a {
                Option::Some(value) => print(value),
                Option::None => print(-1),
            }
            let b = g.next();
            match b {
                Option::Some(value) => print(value),
                Option::None => print(-1),
            }
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn select_ready_arm_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "select_ready_arm",
        r#"
        enum Result { Ok(i64), Err(SendError) }

        fn main() {
            let (tx, rx) = channel<i64>(capacity: 1);
            let sent = tx.send(7);
            select {
                v = rx.recv() => print(v),
                default => print(0),
            }
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn select_default_arm_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "select_default_arm",
        r#"
        fn main() {
            let (tx, rx) = channel<i64>(capacity: 1);
            select {
                v = rx.recv() => print(v),
                default => print(99),
            }
        }
        "#,
    );
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn spawned_task_combines_with_struct_construction_and_method_dispatch_matches_run_and_release_build() {
    assert_run_matches_release_llvm(
        "spawn_struct_method",
        r#"
        struct Point {
            x: i64,
            y: i64,

            fn sum(&self) -> i64 {
                self.x + self.y
            }
        }

        enum Result { Ok(i64), Err(TaskPanic) }

        fn main() {
            let handle = spawn {
                let p = Point { x: 3, y: 4 };
                p.sum()
            };
            match handle.join() {
                Result::Ok(value) => print(value),
                Result::Err(e) => print(-1),
            }
        }
        "#,
    );
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_concurrency_codegen_differential_{}_{}_{}.paco",
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
