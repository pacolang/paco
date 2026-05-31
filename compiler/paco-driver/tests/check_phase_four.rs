use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{DriverOutput, run};

#[test]
fn check_reports_use_after_move_after_function_call() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    consume(box)
    print(box.value)
}
"#;
    let file = write_temp_paco("use_after_move_call", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_double_move() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    consume(box)
    consume(box)
}
"#;
    let file = write_temp_paco("double_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_inconsistent_conditional_move() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let flag: bool = true
    if flag {
        consume(box)
    } else {
    }
    print(0)
}
"#;
    let file = write_temp_paco("conditional_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("inconsistent move state"));
}

#[test]
fn check_allows_move_on_returning_branch() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let flag: bool = true
    if flag {
        consume(box)
        return
    } else {
        print(0)
    }
    print(box.value)
}
"#;
    let file = write_temp_paco("returning_branch_move", source);
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
fn check_reports_inconsistent_match_move() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let flag: bool = true
    match flag {
        true => consume(box),
        false => print(0),
    }
    print(0)
}
"#;
    let file = write_temp_paco("match_conditional_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("inconsistent move state"));
}

#[test]
fn check_allows_copy_int_after_by_value_call() {
    let source = r#"
fn consume(value: int) {}

fn main() {
    let n: int = 1
    consume(n)
    print(n)
}
"#;
    let file = write_temp_paco("copy_int", source);
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
fn check_allows_copy_int_after_generic_by_value_call() {
    let source = r#"
fn id<T>(value: T) -> T {
    value
}

fn main() {
    let n: int = 1
    let value = id(n)
    print(n)
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("copy_int_generic_call", source);
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
fn check_allows_copy_bool_and_float_after_by_value_call() {
    let source = r#"
fn consume_bool(value: bool) {}
fn consume_float(value: float) {}

fn main() {
    let flag: bool = true
    let ratio: float = 1.5
    consume_bool(flag)
    consume_float(ratio)
    print(flag)
    print(ratio)
}
"#;
    let file = write_temp_paco("copy_bool_float", source);
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
fn check_allows_read_only_print_of_non_copy_value() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    print(box.value)
    print(box.value)
}
"#;
    let file = write_temp_paco("print_reads_non_copy", source);
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
fn check_reports_assignment_move_of_non_copy_source() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    let moved: Box = box
    print(box.value)
    print(moved.value)
}
"#;
    let file = write_temp_paco("assignment_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_block_result_move_of_non_copy_source() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    let moved: Box = { box }
    print(box.value)
    print(moved.value)
}
"#;
    let file = write_temp_paco("block_result_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_discarded_non_copy_expression_move() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    box
    print(box.value)
}
"#;
    let file = write_temp_paco("discarded_non_copy_expression", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_if_result_move_of_non_copy_source() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    let flag: bool = true
    let moved: Box = if flag { box } else { Box { value: 2 } }
    print(box.value)
    print(moved.value)
}
"#;
    let file = write_temp_paco("if_result_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_match_result_move_of_non_copy_source() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    let flag: bool = true
    let moved: Box = match flag {
        true => box,
        false => Box { value: 2 },
    }
    print(box.value)
    print(moved.value)
}
"#;
    let file = write_temp_paco("match_result_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_string_move() {
    let source = r#"
fn consume(value: string) {}

fn main() {
    let text: string = "owned"
    consume(text)
    print(text)
}
"#;
    let file = write_temp_paco("string_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("text"));
}

#[test]
fn check_reports_enum_move() {
    let source = r#"
enum Token { Number(int) }

fn consume(value: Token) {}

fn main() {
    let token: Token = Token::Number(1)
    consume(token)
    consume(token)
}
"#;
    let file = write_temp_paco("enum_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("token"));
}

#[test]
fn check_allows_borrowed_method_receiver_after_call() {
    let source = r#"
struct Box {
    value: int,

    fn read(self&) -> int {
        self.value
    }
}

fn main() {
    let box: Box = Box { value: 1 }
    box.read()
    print(box.value)
}
"#;
    let file = write_temp_paco("borrowed_method_receiver", source);
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
fn check_allows_borrowed_method_receiver_on_field() {
    let source = r#"
struct Box {
    value: int,

    fn read(self&) -> int {
        self.value
    }
}

struct Holder { inner: Box }

fn main() {
    let holder: Holder = Holder { inner: Box { value: 1 } }
    holder.inner.read()
    print(holder.inner.value)
}
"#;
    let file = write_temp_paco("borrowed_field_method_receiver", source);
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
fn check_uses_method_return_type_for_inferred_copy_binding() {
    let source = r#"
struct Box {
    value: int,

    fn read(self&) -> int {
        self.value
    }
}

fn main() {
    let box: Box = Box { value: 1 }
    let value = box.read()
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("method_return_copy_binding", source);
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
fn check_uses_associated_return_type_for_inferred_copy_binding() {
    let source = r#"
struct Box {
    value: int,

    fn answer() -> int {
        42
    }
}

fn main() {
    let value = Box::answer()
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("associated_return_copy_binding", source);
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
fn check_uses_generic_method_return_type_for_inferred_copy_binding() {
    let source = r#"
struct Box {
    value: int,

    fn id<T>(self&, value: T) -> T {
        value
    }
}

fn main() {
    let box: Box = Box { value: 1 }
    let n: int = 1
    let value = box.id(n)
    print(n)
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("generic_method_return_copy_binding", source);
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
fn check_uses_generic_associated_return_type_for_inferred_copy_binding() {
    let source = r#"
struct Box {
    value: int,

    fn id<T>(value: T) -> T {
        value
    }
}

fn main() {
    let n: int = 1
    let value = Box::id(n)
    print(n)
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("generic_associated_return_copy_binding", source);
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
fn check_uses_generic_function_return_type_for_inferred_copy_binding() {
    let source = r#"
fn id<T>(value: T) -> T {
    value
}

fn main() {
    let value = id(1)
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("generic_function_return_copy_binding", source);
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
fn check_uses_generic_struct_field_type_for_inferred_copy_binding() {
    let source = r#"
struct Box<T> { value: T }

fn main() {
    let box: Box<int> = Box<int> { value: 1 }
    let value = box.value
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("generic_field_copy_binding", source);
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
fn check_uses_if_expression_result_type_for_inferred_copy_binding() {
    let source = r#"
fn main() {
    let flag: bool = true
    let value = if flag { 1 } else { 2 }
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("if_result_copy_binding", source);
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
fn check_uses_if_branch_local_result_type_for_inferred_copy_binding() {
    let source = r#"
fn main() {
    let flag: bool = true
    let value = if flag {
        let n: int = 1
        n
    } else {
        2
    }
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("if_branch_local_result_copy_binding", source);
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
fn check_uses_match_expression_result_type_for_inferred_copy_binding() {
    let source = r#"
fn main() {
    let flag: bool = true
    let value = match flag {
        true => 1,
        false => 2,
    }
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("match_result_copy_binding", source);
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
fn check_uses_match_pattern_result_type_for_inferred_copy_binding() {
    let source = r#"
enum Token { Number(int) }

fn main() {
    let token: Token = Token::Number(1)
    let value = match token {
        Token::Number(n) => n,
    }
    let alias = value
    print(value)
    print(alias)
}
"#;
    let file = write_temp_paco("match_pattern_result_copy_binding", source);
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
fn check_allows_reinitializing_moved_mutable_binding() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let mut box: Box = Box { value: 1 }
    consume(box)
    box = Box { value: 2 }
    print(box.value)
}
"#;
    let file = write_temp_paco("reinitialize_moved_mutable", source);
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
fn check_allows_reinitializing_moved_binding_in_all_branches() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let mut box: Box = Box { value: 1 }
    let flag: bool = true
    consume(box)
    if flag {
        box = Box { value: 2 }
    } else {
        box = Box { value: 3 }
    }
    print(box.value)
}
"#;
    let file = write_temp_paco("branch_reinitialize_moved", source);
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
fn check_reports_use_after_move_inside_method_body() {
    let source = r#"
struct Box {
    value: int,

    fn bad(self&) {
        let box: Box = Box { value: 1 }
        consume(box)
        print(box.value)
    }
}

fn consume(value: Box) {}
"#;
    let file = write_temp_paco("method_body_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_use_after_by_value_method_receiver() {
    let source = r#"
struct Box {
    value: int,

    fn consume(self) {}
}

fn main() {
    let box: Box = Box { value: 1 }
    box.consume()
    print(box.value)
}
"#;
    let file = write_temp_paco("method_receiver_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_reports_use_after_match_scrutinee_move() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    match box {
        _ => 0,
    }
    print(box.value)
}
"#;
    let file = write_temp_paco("match_scrutinee_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("box"));
}

#[test]
fn check_uses_enum_pattern_payload_type_for_copy_binding() {
    let source = r#"
enum Token { Number(int) }

fn main() {
    let token: Token = Token::Number(1)
    match token {
        Token::Number(value) => {
            let alias = value
            print(value)
            print(alias)
        },
    }
}
"#;
    let file = write_temp_paco("enum_pattern_payload_copy", source);
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
fn check_reports_move_inside_loop() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    while true {
        consume(box)
    }
}
"#;
    let file = write_temp_paco("loop_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("loop"));
}

#[test]
fn check_reports_move_inside_loop_even_when_reinitialized() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let mut box: Box = Box { value: 1 }
    while true {
        consume(box)
        box = Box { value: 2 }
    }
}
"#;
    let file = write_temp_paco("loop_move_reinitialized", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("loop"));
}

#[test]
fn check_rejects_non_copy_field_move() {
    let source = r#"
struct Inner { value: int }
struct Holder { inner: Inner }

fn consume(value: Inner) {}

fn main() {
    let holder: Holder = Holder { inner: Inner { value: 1 } }
    consume(holder.inner)
}
"#;
    let file = write_temp_paco("field_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("use-after-move"));
    assert!(error.contains("field moves"));
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_phase_four_{}_{}_{}.paco",
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
