use clap::Parser;
use paco_driver::{Cli, run as drive};

/// Checks and builds `source`, then runs the binary: the result is what the
/// compiler printed while evaluating `comptime` code followed by what the
/// program printed, or the compiler's diagnostics.
fn run(source: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.paco");
    std::fs::write(&file, source).unwrap();
    let path = file.to_str().unwrap();
    let checked = drive(Cli::try_parse_from(["paco", "check", path]).unwrap())?;
    if checked.stderr.contains("error") {
        return Err(checked.stderr);
    }
    let built = drive(Cli::try_parse_from(["paco", "build", path]).unwrap())?;
    let binary = std::process::Command::new(file.with_extension(std::env::consts::EXE_EXTENSION)).output().unwrap();
    assert!(binary.status.success(), "{}", String::from_utf8_lossy(&binary.stderr));
    Ok(built.stdout + &String::from_utf8_lossy(&binary.stdout))
}

#[test]
fn comptime_block_evaluates_its_inner_expression() {
    let output = run("fn main() { print(comptime { 1 + 2 }) }").expect("should evaluate");
    assert_eq!(output, "3\n");
}

#[test]
fn file_io_inside_comptime_is_rejected() {
    let error = run(r#"fn main() { comptime { fs_read_to_string(&"x") }; }"#).unwrap_err();
    assert!(error.contains("file I/O"), "{error}");
    assert!(error.contains("not allowed inside `comptime`"), "{error}");
}

#[test]
fn ffi_inside_comptime_is_rejected() {
    let error = run(
        r#"
extern "C" {
    fn abs(n: i64) -> i64;
}

fn main() {
    comptime { unsafe { abs(0 - 1) } };
}
"#,
    )
    .unwrap_err();
    assert!(error.contains("extern"), "{error}");
    assert!(error.contains("not allowed inside `comptime`"), "{error}");
}

#[test]
fn spawn_inside_comptime_is_rejected() {
    let error = run(
        r#"
fn worker() -> i64 { 1 }

fn main() {
    comptime { spawn worker() };
}
"#,
    )
    .unwrap_err();
    assert!(error.contains("spawning a task"), "{error}");
    assert!(error.contains("not allowed inside `comptime`"), "{error}");
}

#[test]
fn a_runaway_comptime_loop_is_stopped_rather_than_hanging() {
    let error = run(
        r#"
fn main() {
    comptime {
        let mut i: i64 = 0;
        while i >= 0 {
            i = i + 1
        }
        i
    };
}
"#,
    )
    .unwrap_err();
    assert!(error.contains("instruction budget"), "{error}");
}

#[test]
fn unbounded_recursion_inside_comptime_is_stopped_by_the_instruction_budget() {
    let error = run(
        r#"
fn count_up(n: i64) -> i64 {
    count_up(n + 1)
}

fn main() {
    comptime { count_up(0) };
}
"#,
    )
    .unwrap_err();
    assert!(error.contains("call-depth limit"), "{error}");
    assert!(error.contains("unbounded recursion"), "{error}");
}

#[test]
fn a_type_typed_parameter_binds_to_a_type_value() {
    let output = run(
        r#"
struct Foo { x: i64 }

fn f(t: type) {
    print(t)
}

fn main() {
    f(Foo)
}
"#,
    )
    .expect("should evaluate");
    assert_eq!(output, "Foo\n");
}

#[test]
fn fields_of_yields_fields_in_declaration_order_inside_comptime() {
    let output = run(
        r#"
struct User { name: string, age: i64 }

fn main() {
    comptime {
        for field in fields_of(User) {
            print(field.name)
        }
    }
}
"#,
    )
    .expect("should evaluate");
    assert_eq!(output, "name\nage\n");
}

#[test]
fn fields_of_outside_comptime_is_rejected() {
    let error = run(
        r#"
struct User { name: string, age: i64 }

fn main() {
    fields_of(User);
}
"#,
    )
    .unwrap_err();
    assert!(error.contains("fields_of"), "{error}");
    assert!(error.contains("comptime"), "{error}");
}

#[test]
fn type_name_returns_the_captured_types_own_name() {
    let output = run(
        r#"
struct User { name: string, age: i64 }

fn main() {
    comptime {
        print(type_name(User))
    }
}
"#,
    )
    .expect("should evaluate");
    assert_eq!(output, "User\n");
}


#[test]
fn fields_of_returns_a_fresh_independent_snapshot_each_call() {
    // `phase-9-comptime` task 4.7: introspection is read-only by
    // construction, not by an enforced runtime check — `FieldInfo` has no
    // methods at all (so no setter exists to call), and `fields_of`
    // builds a fresh `Vec<FieldInfo>` from the struct decl on every call
    // rather than handing out a live view into it. Calling it twice on
    // the same type yields two independent, identically-shaped results.
    let output = run(
        r#"
struct User { name: string, age: i64 }

fn main() {
    comptime {
        let a = fields_of(User);
        let b = fields_of(User);
        print(type_name(User))
    }
}
"#,
    )
    .expect("should evaluate");
    assert_eq!(output, "User\n");
}

#[test]
fn quote_substitutes_a_type_position_splice() {
    let output = run(
        r#"
struct User { name: string, age: i64 }

fn make(t: type) -> string {
    let c = quote { methods #(t) { fn hello(&self) -> i64 { 1 } } };
    code_to_string(c)
}

fn main() {
    print(comptime { make(User) })
}
"#,
    )
    .expect("should evaluate");
    assert!(output.contains("methods User {"), "{output}");
}

#[test]
fn quote_substitutes_an_expression_position_splice() {
    let output = run(
        r#"
fn main() {
    comptime {
        let c = quote { #(41) };
        print(code_to_string(c))
    }
}
"#,
    )
    .expect("should evaluate");
    assert_eq!(output, "41\n");
}

#[test]
fn quote_substitutes_an_identifier_position_splice() {
    let output = run(
        r#"
fn main() {
    comptime {
        let c = quote { self.#("age") };
        print(code_to_string(c))
    }
}
"#,
    )
    .expect("should evaluate");
    assert_eq!(output, "self.age\n");
}

#[test]
fn quote_substitutes_all_three_splice_positions_in_one_template() {
    let output = run(
        r#"
struct User { name: string, age: i64 }

fn make(t: type, field: string) -> string {
    let c = quote {
        methods #(t) {
            fn describe(&self) -> i64 {
                let bonus = #(41);
                self.#(field)
            }
        }
    };
    code_to_string(c)
}

fn main() {
    print(comptime { make(User, "age") })
}
"#,
    )
    .expect("should evaluate");
    assert!(output.contains("methods User {"), "{output}");
    assert!(output.contains("41"), "{output}");
    assert!(output.contains("self.age"), "{output}");
}

#[test]
fn code_join_concatenates_fragments_with_the_separator_as_raw_code() {
    let output = run(
        r#"
fn main() {
    comptime {
        let mut pieces: Vec<Code> = Vec::new();
        pieces.push(quote { "a" });
        pieces.push(quote { "b" });
        pieces.push(quote { "c" });
        let joined = Code::join(pieces, " + \", \" + ");
        print(code_to_string(joined))
    }
}
"#,
    )
    .expect("should evaluate");
    assert_eq!(output, "\"a\" + \", \" + \"b\" + \", \" + \"c\"\n");
}

#[test]
fn a_type_typed_parameter_can_be_passed_to_fields_of_or_type_name_on_itself() {
    // Bug found implementing `phase-9-comptime` task 7.1 (`derive_display`
    // calling `type_name(t)`/`fields_of(t)` on its own `t: type`
    // parameter): `eval_args_for_call`'s `type`-argument special-casing
    // always resolved a bare identifier as a literal struct/enum name,
    // even when it was already a bound `type`-typed parameter — treating
    // `t` as a struct literally named "t" instead of reusing its already-
    // bound `Value::Type`.
    let output = run(
        r#"
struct User { name: string, age: i64 }

fn inner(t: type) -> string {
    type_name(t)
}

fn main() {
    comptime {
        print(inner(User))
    }
}
"#,
    )
    .expect("should evaluate");
    assert_eq!(output, "User\n");
}
