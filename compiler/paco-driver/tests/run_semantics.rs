use paco_driver::run_program;

fn run_status(source: &str) -> (String, String, Option<i32>) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.paco");
    std::fs::write(&file, source).unwrap();
    let (output, status) = run_program(&file, &[]).unwrap();
    (output.stdout, output.stderr, status)
}

fn run(source: &str) -> String {
    let (stdout, stderr, status) = run_status(source);
    assert_eq!(status, Some(0), "{stderr}");
    stdout
}

fn run_err(source: &str) -> String {
    let (_, stderr, status) = run_status(source);
    assert_ne!(status, Some(0), "the program should fail");
    stderr
}

#[test]
fn spawned_function_side_effects_are_observable_after_join() {
    let output = run(r#"
fn double(n: i64) -> i64 { n * 2 }

fn main() {
    let h = spawn double(21);
    match h.join() {
        Ok(value) => print(value),
        Err(e) => print(0),
    };
}
"#);
    assert_eq!(output, "42\n");
}

#[test]
fn join_on_a_panicking_task_returns_the_err_arm() {
    let (stdout, stderr, status) = run_status(r#"
fn boom() -> i64 { panic("kaboom"); }

fn main() {
    let h = spawn boom();
    match h.join() {
        Ok(value) => print(value),
        Err(reason) => print("caught"),
    };
}
"#);
    assert_eq!(stdout, "caught\n");
    assert!(stderr.contains("kaboom"), "{stderr}");
    assert_eq!(status, Some(0));
}

#[test]
fn select_runs_the_ready_channel_arm_over_default() {
    let output = run(r#"
fn main() {
    let (tx, rx) = channel<i64>(capacity: 1);
    tx.send(7);
    select {
        v = rx.recv() => print(v),
        default => print(0),
    };
}
"#);
    assert_eq!(output, "7\n");
}

#[test]
fn select_runs_default_when_no_channel_is_ready() {
    let output = run(r#"
fn main() {
    let (tx, rx) = channel<i64>(capacity: 1);
    select {
        v = rx.recv() => print(v),
        default => print(99),
    };
}
"#);
    assert_eq!(output, "99\n");
}

#[test]
fn iter_fn_generator_yields_the_fibonacci_sequence_pulled_ten_times() {
    let output = run(r#"
iter fn fibonacci() -> i64 {
    let mut a = 0;
    let mut b = 1;
    loop {
        yield a;
        let next = a + b;
        a = b;
        b = next;
    }
}

fn main() {
    let gen = fibonacci();
    let mut i = 0;
    while i < 10 {
        match gen.next() {
            Some(n) => print(n),
            None => {}
        };
        i = i + 1;
    }
}
"#);
    assert_eq!(output, "0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n");
}

#[test]
fn iter_fn_generator_reports_none_once_exhausted() {
    let output = run(r#"
iter fn count_to_two() -> i64 {
    yield 1;
    yield 2;
}

fn main() {
    let gen = count_to_two();
    let mut i = 0;
    while i < 4 {
        match gen.next() {
            Some(n) => print(n),
            None => print(-1),
        };
        i = i + 1;
    }
}
"#);
    assert_eq!(output, "1\n2\n-1\n-1\n");
}

#[test]
fn channel_producer_consumer_example() {
    let output = run(r#"
fn produce(tx: Sender<i64>) {
    let mut i = 0;
    while i < 5 {
        tx.send(i);
        i = i + 1;
    }
    tx.close();
}

fn main() {
    let (tx, rx) = channel<i64>(capacity: 8);
    let producer = spawn produce(tx);
    let mut sum = 0;
    loop {
        match rx.recv() {
            Ok(value) => { sum = sum + value; }
            Err(e) => break,
        };
    }
    producer.join();
    print(sum);
}
"#);
    assert_eq!(output, "10\n");
}

#[test]
fn module_level_const_resolves_at_runtime() {
    assert_eq!(run("const TILE: i64 = 64;\nfn main() { print(TILE); }\n"), "64\n");
}

#[test]
fn const_referencing_another_const_resolves_at_runtime() {
    let output = run("const TILE: i64 = 64;\nconst DOUBLE_TILE: i64 = TILE * 2;\nfn main() { print(DOUBLE_TILE); }\n");
    assert_eq!(output, "128\n");
}

#[test]
fn calls_a_real_libc_function_through_ffi() {
    let output = run(r#"
extern "C" {
    fn abs(n: i64) -> i64;
}

fn main() {
    unsafe {
        print(abs(0 - 7));
    }
}
"#);
    assert_eq!(output, "7\n");
}

#[test]
fn dereferences_a_pointer_received_as_a_parameter_and_marshals_raw_pointers_through_extern_calls() {
    let output = run(r#"
extern "C" {
    fn calloc(nmemb: i64, size: i64) -> *const i64;
    fn free(ptr: *const i64);
}

fn read_it(p: *const i64) -> i64 {
    unsafe { *p }
}

fn main() {
    unsafe {
        let p: *const i64 = calloc(1, 8);
        print(read_it(p));
        free(p);
    }
}
"#);
    assert_eq!(output, "0\n");
}

#[test]
fn question_mark_unwraps_ok_and_continues() {
    let output = run(r#"
fn safe_div(a: i64, b: i64) -> Result<i64, i64> { Ok(a) }
fn f() -> Result<i64, i64> {
    let n = safe_div(10, 2)?;
    Ok(n)
}
fn main() {
    match f() {
        Ok(n) => print(n),
        Err(e) => print(e),
    };
}
"#);
    assert_eq!(output, "10\n");
}

#[test]
fn question_mark_short_circuits_on_err() {
    let output = run(r#"
fn always_fails() -> Result<i64, i64> { Err(99) }
fn f() -> Result<i64, i64> {
    let n = always_fails()?;
    Ok(n)
}
fn main() {
    match f() {
        Ok(n) => print(n),
        Err(e) => print(e),
    };
}
"#);
    assert_eq!(output, "99\n");
}

#[test]
fn question_mark_converts_the_error_via_from_at_runtime() {
    let output = run(r#"
struct ParseError { code: i64 }
struct AppError {
    code: i64,

    fn from(e: ParseError) -> Self { AppError { code: e.code } }
}
fn parse() -> Result<i64, ParseError> { Err(ParseError { code: 7 }) }
fn f() -> Result<i64, AppError> {
    let n = parse()?;
    Ok(n)
}
fn main() {
    match f() {
        Ok(n) => print(n),
        Err(e) => print(e.code),
    };
}
"#);
    assert_eq!(output, "7\n");
}

#[test]
fn plus_dispatches_to_a_struct_add_method() {
    let output = run(r#"
struct Point {
    x: i64,
    y: i64,

    fn add(&self, other: Self) -> Self {
        Point { x: self.x + other.x, y: self.y + other.y }
    }
}

fn main() {
    let a = Point { x: 1, y: 2 };
    let b = Point { x: 3, y: 4 };
    let c = a + b;
    print(c.x);
    print(c.y);
}
"#);
    assert_eq!(output, "4\n6\n");
}

#[test]
fn unary_minus_dispatches_to_a_struct_neg_method() {
    let output = run(r#"
struct Point {
    x: i64,
    y: i64,

    fn neg(&self) -> Self {
        Point { x: 0 - self.x, y: 0 - self.y }
    }
}

fn main() {
    let a = Point { x: 1, y: 2 };
    let b = -a;
    print(b.x);
    print(b.y);
}
"#);
    assert_eq!(output, "-1\n-2\n");
}

#[test]
fn numeric_arithmetic_evaluation_is_unchanged() {
    assert_eq!(run("fn main() { print(1 + 2); print(0 - 3); }"), "3\n-3\n");
}

#[test]
fn a_generic_struct_implementing_add_dispatches_correctly() {
    let output = run(r#"
struct Vector2<T> {
    x: T,
    y: T,
}

methods<T: Numeric + Add> Vector2<T> {
    fn add(&self, other: Self) -> Self {
        Vector2<T> { x: self.x + other.x, y: self.y + other.y }
    }
}

fn main() {
    let a = Vector2<i64> { x: 1, y: 2 };
    let b = Vector2<i64> { x: 3, y: 4 };
    let c = a + b;
    print(c.x);
    print(c.y);
}
"#);
    assert_eq!(output, "4\n6\n");
}

#[test]
fn a_narrowing_cast_truncates_at_runtime() {
    assert_eq!(run("fn main() { print(300 as u8); }"), "44\n");
}

#[test]
fn char_equality_evaluates_at_runtime() {
    assert_eq!(run("fn main() { print('a' == 'b'); }"), "false\n");
}

#[test]
fn char_ordering_evaluates_at_runtime() {
    assert_eq!(run("fn main() { print('a' < 'b'); }"), "true\n");
}

#[test]
fn a_narrow_width_overflow_is_a_runtime_error() {
    let error = run_err("fn main() { let a: u8 = 200; let b: u8 = 100; print(a + b); }");
    assert!(error.contains("overflow"), "{error}");
}

#[test]
fn a_char_value_displays_as_the_character_itself() {
    assert_eq!(run("fn main() { print('z'); }"), "z\n");
}

#[test]
fn a_narrow_width_value_displays_as_its_decimal_value() {
    assert_eq!(run("fn main() { let x: u8 = 200; print(x); }"), "200\n");
}

#[test]
fn a_u64_value_with_the_sign_bit_set_displays_correctly() {
    assert_eq!(run("fn main() { print(-1 as u64); }"), "18446744073709551615\n");
}

#[test]
fn u64_ordering_is_correct_past_i64_max() {
    let output = run("fn main() { let big: u64 = -1 as u64; print(big > 5 as u64); print(big < 5 as u64); }");
    assert_eq!(output, "true\nfalse\n");
}

#[test]
fn u64_subtraction_is_correct_past_i64_max() {
    assert_eq!(run("fn main() { let big: u64 = -1 as u64; print(big - 1 as u64); }"), "18446744073709551614\n");
}

#[test]
fn u64_addition_still_detects_real_overflow_past_i64_max() {
    let error = run_err("fn main() { let big: u64 = -1 as u64; print(big + 1 as u64); }");
    assert!(error.contains("overflow"), "{error}");
}
