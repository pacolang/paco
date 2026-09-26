use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{DriverOutput, run};

#[test]
fn check_rejects_yield_outside_an_iter_fn() {
    let source = r#"
fn counter() {
    yield 1
}

fn main() {
    counter()
}
"#;
    let file = write_temp_paco("yield_outside_iter_fn", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0326"));
    assert!(error.contains("iter fn"));
}

#[test]
fn check_accepts_yield_inside_an_iter_fn() {
    let source = r#"
iter fn counter() -> i64 {
    yield 1;
    yield 2
}

fn main() {
    let gen = counter();
}
"#;
    let file = write_temp_paco("yield_inside_iter_fn", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: String::new(),
            stderr: String::new(),
        }
    );
}

#[test]
fn check_rejects_use_of_a_variable_after_it_was_captured_by_spawn() {
    let source = r#"
struct Box { value: i64 }

fn consume(b: Box) {}

fn main() {
    let b: Box = Box { value: 1 };
    spawn consume(b);
    print(b.value)
}
"#;
    let file = write_temp_paco("spawn_capture_use_after_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("moved value `b`"));
}

#[test]
fn check_rejects_use_after_spawn_captures_a_bare_variable() {
    let source = r#"
struct Box { value: i64 }

fn main() {
    let b: Box = Box { value: 1 };
    spawn b;
    print(b.value)
}
"#;
    let file = write_temp_paco("spawn_capture_bare_ident", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("moved value `b`"));
}

#[test]
fn check_rejects_use_of_an_owner_after_spawn_reads_its_field() {
    let source = r#"
struct Box { value: i64 }

fn main() {
    let b: Box = Box { value: 1 };
    spawn { print(b.value) };
    print(b.value)
}
"#;
    let file = write_temp_paco("spawn_field_read_moves_owner", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("moved value `b`"));
}

#[test]
fn check_allows_spawn_to_capture_a_copy_value_without_moving_it() {
    let source = r#"
fn main() {
    let n: i64 = 1;
    spawn { print(n) };
    print(n)
}
"#;
    let file = write_temp_paco("spawn_copy_capture", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    assert!(run(cli).is_ok());
}

#[test]
fn check_rejects_use_of_a_value_after_it_was_sent_over_a_channel() {
    let source = r#"
struct Box { value: i64 }

fn main() {
    let (tx, rx) = channel<Box>(capacity: 1);
    let b: Box = Box { value: 1 };
    tx.send(b);
    print(b.value)
}
"#;
    let file = write_temp_paco("channel_send_use_after_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("moved value `b`"));
}

#[test]
fn check_accepts_the_spec_channel_of_int_example() {
    let source = r#"
fn main() {
    let (tx, rx) = channel<i64>(capacity: 8);
    tx.send(1);
    tx.close();
    let v = rx.recv();
    print(v)
}
"#;
    let file = write_temp_paco("channel_of_int_example", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: String::new(),
            stderr: String::new(),
        }
    );
}

#[test]
fn check_rejects_sending_a_bare_reference_over_a_channel() {
    let source = r#"
struct Box { value: i64 }

fn main() {
    let (tx, rx) = channel<&Box>(capacity: 1);
    let b: Box = Box { value: 1 };
    tx.send(&b);
}
"#;
    let file = write_temp_paco("channel_send_shared_without_sync", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("shared-without-sync"));
}

#[test]
fn check_accepts_sending_an_arc_wrapped_value_over_a_channel() {
    let source = r#"
struct Box { value: i64 }

fn main() {
    let (tx, rx) = channel<Arc<Box>>(capacity: 1);
    let b: Box = Box { value: 1 };
    tx.send(Arc::new(b));
}
"#;
    let file = write_temp_paco("channel_send_arc_ok", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: String::new(),
            stderr: String::new(),
        }
    );
}

#[test]
fn check_rejects_spawn_capturing_a_bare_reference() {
    let source = r#"
struct Box { value: i64 }

fn look(b: &Box) {}

fn main() {
    let b: Box = Box { value: 1 };
    let r: &Box = &b;
    spawn look(r);
}
"#;
    let file = write_temp_paco("spawn_capture_shared_without_sync", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("shared-without-sync"));
}

#[test]
fn check_accepts_spawn_capturing_an_arc_wrapped_value() {
    let source = r#"
struct Box { value: i64 }

fn look(b: Arc<Box>) {}

fn main() {
    let b: Box = Box { value: 1 };
    let shared: Arc<Box> = Arc::new(b);
    spawn look(shared);
}
"#;
    let file = write_temp_paco("spawn_capture_arc_ok", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: String::new(),
            stderr: String::new(),
        }
    );
}

#[test]
fn check_warns_but_still_succeeds_on_a_direct_extern_call_inside_spawn() {
    let source = r#"
extern "C" {
    fn slow_work();
}

fn main() {
    spawn { unsafe { slow_work() } };
}
"#;
    let file = write_temp_paco("blocking_call_on_worker", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("blocking-call-on-worker"));
    assert!(output.stderr.starts_with("warning"));
}

#[test]
fn check_does_not_warn_on_an_extern_call_outside_spawn() {
    let source = r#"
extern "C" {
    fn slow_work();
}

fn main() {
    unsafe { slow_work() }
}
"#;
    let file = write_temp_paco("no_blocking_call_warning_outside_spawn", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: String::new(),
            stderr: String::new(),
        }
    );
}

#[test]
fn run_executes_the_spec_channel_producer_consumer_example_end_to_end() {
    let source = r#"
enum Result<T, E> { Ok(T), Err(E) }

fn produce(tx: Sender<i64>) {
    let mut i = 0;
    while i < 5 {
        tx.send(i);
        i = i + 1
    }
    tx.close()
}

fn main() {
    let (tx, rx) = channel<i64>(capacity: 8);
    let producer = spawn produce(tx);
    let mut sum = 0;
    loop {
        match rx.recv() {
            Result::Ok(value) => { sum = sum + value }
            Result::Err(e) => break
        }
    }
    producer.join();
    print(sum)
}
"#;
    let file = write_temp_paco("run_channel_producer_consumer", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: "10\n".to_string(),
            stderr: String::new(),
        }
    );
}

#[test]
fn run_select_prefers_a_ready_channel_over_default() {
    let source = r#"
fn main() {
    let (tx, rx) = channel<i64>(capacity: 1);
    tx.send(7);
    select {
        v = rx.recv() => print(v),
        default => print(0),
    }
}
"#;
    let file = write_temp_paco("run_select_ready_channel", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput { stdout: "7\n".to_string(), stderr: String::new() }
    );
}

#[test]
fn run_select_runs_default_when_nothing_is_ready() {
    let source = r#"
fn main() {
    let (tx, rx) = channel<i64>(capacity: 1);
    select {
        v = rx.recv() => print(v),
        default => print(99),
    }
}
"#;
    let file = write_temp_paco("run_select_default", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput { stdout: "99\n".to_string(), stderr: String::new() }
    );
}

#[test]
fn run_executes_the_spec_fibonacci_iter_fn_example_end_to_end() {
    let source = r#"
enum Option<T> { Some(T), None }

iter fn fibonacci() -> i64 {
    let mut a = 0;
    let mut b = 1;
    loop {
        yield a;
        let next = a + b;
        a = b;
        b = next
    }
}

fn main() {
    let gen = fibonacci();
    let mut i = 0;
    while i < 10 {
        match gen.next() {
            Option::Some(n) => print(n),
            Option::None => {}
        }
        i = i + 1
    }
}
"#;
    let file = write_temp_paco("run_iter_fn_fibonacci", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(
        output,
        DriverOutput {
            stdout: "0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n".to_string(),
            stderr: String::new(),
        }
    );
}

fn check_source(name: &str, source: &str) -> Result<DriverOutput, String> {
    let file = write_temp_paco(name, source);
    run(paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap())
}

fn run_source(name: &str, source: &str) -> Result<DriverOutput, String> {
    let file = write_temp_paco(name, source);
    run(paco_driver::Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap())
}

#[test]
fn check_types_spawn_blocking_as_a_join_handle_of_the_closure_result() {
    let source = r#"
fn main() {
    let handle = spawn_blocking(|| { 40 + 2 });
    let n: i64 = handle;
}
"#;
    let error = check_source("spawn_blocking_handle_type", source).unwrap_err();
    assert!(error.contains("PACO-E0302"), "{error}");
    assert!(error.contains("JoinHandle<i64>"), "{error}");
}

#[test]
fn check_rejects_spawn_blocking_with_a_non_closure_argument() {
    let error = check_source("spawn_blocking_non_closure", "fn main() { spawn_blocking(1) }").unwrap_err();
    assert!(error.contains("spawn_blocking expects a closure taking no arguments"), "{error}");
}

#[test]
fn check_does_not_warn_on_an_extern_call_inside_spawn_blocking() {
    let source = r#"
extern "C" {
    fn slow_work();
}

fn main() {
    spawn { spawn_blocking(|| unsafe { slow_work() }) };
}
"#;
    let output = check_source("spawn_blocking_extern_no_warning", source).unwrap();
    assert_eq!(output.stderr, "");
}

#[test]
fn check_rejects_use_after_a_closure_captures_a_value() {
    let source = r#"
struct Box { value: i64 }

fn main() {
    let b = Box { value: 1 };
    let h = spawn_blocking(|| b.value);
    let again = b;
}
"#;
    let error = check_source("closure_capture_moves", source).unwrap_err();
    assert!(error.contains("use of moved value"), "{error}");
}

#[test]
fn check_rejects_a_closure_call_with_a_mistyped_argument() {
    let source = r#"
fn main() {
    let add = |x: i64, y: i64| x + y;
    add(1, true)
}
"#;
    let error = check_source("closure_call_arg_type", source).unwrap_err();
    assert!(error.contains("PACO-E0302"), "{error}");
}

#[test]
fn run_calls_a_closure_with_captures_and_parameters() {
    let source = r#"
fn main() {
    let base = 10;
    let add = |x: i64, y: i64| x + y + base;
    print(add(1, 2))
}
"#;
    assert_eq!(run_source("closure_call", source).unwrap().stdout, "13\n");
}

#[test]
fn run_spawn_blocking_returns_the_closure_result_through_join() {
    let source = r#"
enum Result<T, E> { Ok(T), Err(E) }

fn main() {
    let base = 40;
    let handle = spawn_blocking(|| { base + 2 });
    match handle.join() {
        Result::Ok(value) => print(value),
        Result::Err(e) => print(0),
    }
}
"#;
    assert_eq!(run_source("spawn_blocking_join", source).unwrap().stdout, "42\n");
}

#[test]
fn check_infers_an_unannotated_closure_parameter_from_its_first_call() {
    let source = r#"
fn main() {
    let inc = |x| x + 1;
    let s: string = inc(1);
}
"#;
    let error = check_source("closure_param_inference", source).unwrap_err();
    assert!(error.contains("expected string, found i64"), "{error}");
}

#[test]
fn check_rejects_an_unannotated_closure_parameter_that_is_never_called() {
    let error = check_source("closure_param_uninferred", "fn main() { let f = |x| x; }").unwrap_err();
    assert!(error.contains("PACO-E0335"), "{error}");
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_concurrency_front_end_{}_{}_{}.paco",
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
