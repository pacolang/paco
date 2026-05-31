use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{DriverOutput, run};

#[test]
fn check_accepts_shared_borrow_parameter() {
    let source = r#"
struct Box { value: int }

fn read(value: &Box) -> int {
    value.value
}

fn main() {
    let box: Box = Box { value: 1 }
    let value: int = read(&box)
    print(value)
}
"#;
    let file = write_temp_paco("shared_borrow_parameter", source);
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
fn check_allows_multiple_shared_borrows() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    let first: &Box = &box
    let second: &Box = &box
    print(first.value)
    print(second.value)
}
"#;
    let file = write_temp_paco("multiple_shared_borrows", source);
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
fn check_rejects_mutable_borrow_while_shared_borrow_live() {
    let source = r#"
struct Box { value: int }

fn main() {
    let mut box: Box = Box { value: 1 }
    let shared: &Box = &box
    let unique: &mut Box = &mut box
    print(shared.value)
    print(unique.value)
}
"#;
    let file = write_temp_paco("mutable_while_shared_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("mutable"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_mutable_borrow_argument_while_shared_borrow_live() {
    let source = r#"
struct Box { value: int }

fn mutate(value: &mut Box) {}

fn main() {
    let mut box: Box = Box { value: 1 }
    let shared: &Box = &box
    mutate(&mut box)
    print(shared.value)
}
"#;
    let file = write_temp_paco("mutable_argument_while_shared_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("mutable"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_sibling_mutable_borrow_arguments() {
    let source = r#"
struct Box { value: int }

fn both(left: &mut Box, right: &mut Box) {}

fn main() {
    let mut box: Box = Box { value: 1 }
    both(&mut box, &mut box)
}
"#;
    let file = write_temp_paco("sibling_mutable_borrow_arguments", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_sibling_shared_and_mutable_borrow_arguments() {
    let source = r#"
struct Box { value: int }

fn both(left: &mut Box, right: &Box) {}

fn main() {
    let mut box: Box = Box { value: 1 }
    both(&mut box, &box)
}
"#;
    let file = write_temp_paco("sibling_shared_and_mutable_borrow_arguments", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_call_temporary_borrow_with_by_value_argument_move() {
    let source = r#"
struct Box { value: int }

fn read_then_consume(read: &Box, moved: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    read_then_consume(&box, box)
}
"#;
    let file = write_temp_paco("call_temporary_borrow_with_by_value_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_call_temporary_borrow_with_block_argument_move() {
    let source = r#"
struct Box { value: int }

fn read_then_consume(read: &Box, moved: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    read_then_consume(&box, { box })
}
"#;
    let file = write_temp_paco("call_temporary_borrow_with_block_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_call_temporary_borrow_with_nested_call_argument_move() {
    let source = r#"
struct Box { value: int }

fn read_then_consume(read: &Box, moved: Box) {}

fn take(value: Box) -> Box {
    value
}

fn main() {
    let box: Box = Box { value: 1 }
    read_then_consume(&box, take(box))
}
"#;
    let file = write_temp_paco("call_temporary_borrow_with_nested_call_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_call_temporary_block_borrow_with_by_value_move() {
    let source = r#"
struct Box { value: int }

fn read_then_consume(read: &Box, moved: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    read_then_consume({ &box }, box)
}
"#;
    let file = write_temp_paco("call_temporary_block_borrow_with_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_call_temporary_if_borrow_with_by_value_move() {
    let source = r#"
struct Box { value: int }

fn read_then_consume(read: &Box, moved: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    read_then_consume(if true { &box } else { &box }, box)
}
"#;
    let file = write_temp_paco("call_temporary_if_borrow_with_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_call_temporary_if_borrow_with_distinct_branch_move() {
    let source = r#"
struct Box { value: int }

fn read_then_consume(read: &Box, moved: Box) {}

fn main(flag: bool) {
    let left: Box = Box { value: 1 }
    let right: Box = Box { value: 2 }
    read_then_consume(if flag { &left } else { &right }, left)
}
"#;
    let file = write_temp_paco("call_temporary_if_borrow_distinct_branch_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("left"));
}

#[test]
fn check_rejects_call_temporary_match_borrow_with_distinct_branch_move() {
    let source = r#"
struct Box { value: int }

fn read_then_consume(read: &Box, moved: Box) {}

fn main(flag: bool) {
    let left: Box = Box { value: 1 }
    let right: Box = Box { value: 2 }
    read_then_consume(match flag {
        true => &left,
        false => &right,
    }, left)
}
"#;
    let file = write_temp_paco("call_temporary_match_borrow_distinct_branch_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("left"));
}

#[test]
fn check_rejects_call_temporary_returned_borrow_with_by_value_move() {
    let source = r#"
struct Box { value: int }

fn identity(value: &Box) -> &Box {
    value
}

fn read_then_consume(read: &Box, moved: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    read_then_consume(identity(&box), box)
}
"#;
    let file = write_temp_paco("call_temporary_returned_borrow_with_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_call_temporary_borrow_with_nested_copy_argument_move() {
    let source = r#"
struct Box { value: int }

fn read_then_int(read: &Box, value: int) {}

fn take_to_int(value: Box) -> int {
    1
}

fn main() {
    let box: Box = Box { value: 1 }
    read_then_int(&box, take_to_int(box))
}
"#;
    let file = write_temp_paco("call_temporary_borrow_with_nested_copy_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_accepts_single_mutable_borrow_argument() {
    let source = r#"
struct Box { value: int }

fn mutate(value: &mut Box) {}

fn main() {
    let mut box: Box = Box { value: 1 }
    mutate(&mut box)
    print(box.value)
}
"#;
    let file = write_temp_paco("single_mutable_borrow_argument", source);
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
fn check_rejects_method_shared_receiver_with_mutable_borrow_argument() {
    let source = r#"
struct Box {
    value: int

    fn read_with(self&, other: &mut Box) -> int {
        self.value
    }
}

fn main() {
    let mut box: Box = Box { value: 1 }
    let value: int = box.read_with(&mut box)
    print(value)
}
"#;
    let file = write_temp_paco("method_shared_receiver_mutable_argument", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_method_mutable_receiver_with_shared_borrow_argument() {
    let source = r#"
struct Box {
    value: int

    fn update_with(self&mut, other: &Box) {}
}

fn main() {
    let mut box: Box = Box { value: 1 }
    box.update_with(&box)
}
"#;
    let file = write_temp_paco("method_mutable_receiver_shared_argument", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_method_mutable_receiver_with_mutable_borrow_argument() {
    let source = r#"
struct Box {
    value: int

    fn update_with(self&mut, other: &mut Box) {}
}

fn main() {
    let mut box: Box = Box { value: 1 }
    box.update_with(&mut box)
}
"#;
    let file = write_temp_paco("method_mutable_receiver_mutable_argument", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_method_shared_receiver_with_by_value_argument_move() {
    let source = r#"
struct Box {
    value: int

    fn read_consume(self&, other: Box) -> int {
        self.value
    }
}

fn main() {
    let box: Box = Box { value: 1 }
    let value: int = box.read_consume(box)
    print(value)
}
"#;
    let file = write_temp_paco("method_shared_receiver_by_value_argument", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_method_temporary_borrow_with_nested_copy_argument_move() {
    let source = r#"
struct Box {
    value: int

    fn read_int(self&, value: int) -> int {
        self.value
    }
}

fn take_to_int(value: Box) -> int {
    1
}

fn main() {
    let box: Box = Box { value: 1 }
    let value: int = box.read_int(take_to_int(box))
    print(value)
}
"#;
    let file = write_temp_paco("method_temporary_borrow_with_nested_copy_move", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_method_mutable_receiver_with_by_value_argument_move() {
    let source = r#"
struct Box {
    value: int

    fn update_consume(self&mut, other: Box) {}
}

fn main() {
    let mut box: Box = Box { value: 1 }
    box.update_consume(box)
}
"#;
    let file = write_temp_paco("method_mutable_receiver_by_value_argument", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_second_mutable_borrow_while_first_live() {
    let source = r#"
struct Box { value: int }

fn main() {
    let mut box: Box = Box { value: 1 }
    let first: &mut Box = &mut box
    let second: &mut Box = &mut box
    print(first.value)
    print(second.value)
}
"#;
    let file = write_temp_paco("second_mutable_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("mutable"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_while_borrow_live() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let shared: &Box = &box
    consume(box)
    print(shared.value)
}
"#;
    let file = write_temp_paco("move_while_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("move"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_while_field_borrow_live() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let field: &int = &box.value
    consume(box)
    print(field)
}
"#;
    let file = write_temp_paco("move_while_field_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_while_returned_call_borrow_live() {
    let source = r#"
struct Box { value: int }

fn identity(value: &Box) -> &Box {
    value
}

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let borrowed: &Box = identity(&box)
    consume(box)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("move_while_returned_call_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_while_returned_method_receiver_borrow_live() {
    let source = r#"
struct Box {
    value: int

    fn identity(self&) -> &Box {
        &self
    }
}

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let borrowed: &Box = box.identity()
    consume(box)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("move_while_returned_method_receiver_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_temporary_method_receiver_borrow() {
    let source = r#"
struct Box {
    value: int

    fn identity(self&) -> &Box {
        &self
    }
}

fn make_box() -> Box {
    Box { value: 1 }
}

fn bad() -> &Box {
    make_box().identity()
}
"#;
    let file = write_temp_paco("return_temporary_method_receiver_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("temporary"));
}

#[test]
fn check_rejects_binding_temporary_method_receiver_borrow() {
    let source = r#"
struct Box {
    value: int

    fn identity(self&) -> &Box {
        &self
    }
}

fn make_box() -> Box {
    Box { value: 1 }
}

fn main() {
    let borrowed: &Box = make_box().identity()
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("bind_temporary_method_receiver_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("temporary"));
}

#[test]
fn check_rejects_move_while_returned_method_argument_borrow_live() {
    let source = r#"
struct Box { value: int }

struct Helper {
    value: int

    fn choose<'a>(self&, other: &'a Box) -> &'a Box {
        other
    }
}

fn consume(value: Box) {}

fn main() {
    let helper: Helper = Helper { value: 1 }
    let box: Box = Box { value: 1 }
    let borrowed: &Box = helper.choose(&box)
    consume(box)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("move_while_returned_method_argument_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_while_shared_reference_field_borrow_live() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let shared: &Box = &box
    let field: &int = &shared.value
    consume(box)
    print(field)
}
"#;
    let file = write_temp_paco("move_while_shared_reference_field_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_while_mutable_reference_field_borrow_live() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let mut box: Box = Box { value: 1 }
    let unique: &mut Box = &mut box
    let field: &int = &unique.value
    consume(box)
    print(field)
}
"#;
    let file = write_temp_paco("move_while_mutable_reference_field_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_allows_move_after_borrow_last_use() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let shared: &Box = &box
    print(shared.value)
    consume(box)
}
"#;
    let file = write_temp_paco("move_after_borrow_last_use", source);
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
fn check_allows_move_in_nested_block_after_borrow_last_use() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let shared: &Box = &box
    print(shared.value)
    {
        consume(box)
    }
}
"#;
    let file = write_temp_paco("nested_move_after_borrow_last_use", source);
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
fn check_rejects_move_in_nested_block_before_later_borrow_use() {
    let source = r#"
struct Box { value: int }

fn consume(value: Box) {}

fn main() {
    let box: Box = Box { value: 1 }
    let shared: &Box = &box
    if true {
        print(1)
        consume(box)
    }
    print(shared.value)
}
"#;
    let file = write_temp_paco("nested_move_before_later_borrow_use", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_assignment_while_borrow_live() {
    let source = r#"
struct Box { value: int }

fn main() {
    let mut box: Box = Box { value: 1 }
    let shared: &Box = &box
    box = Box { value: 2 }
    print(shared.value)
}
"#;
    let file = write_temp_paco("assignment_while_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_field_assignment_while_borrow_live() {
    let source = r#"
struct Box { value: int }

fn main() {
    let mut box: Box = Box { value: 1 }
    let shared: &Box = &box
    box.value = 2
    print(shared.value)
}
"#;
    let file = write_temp_paco("field_assignment_while_borrow_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_mutable_borrow_of_immutable_binding() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    let unique: &mut Box = &mut box
    print(unique.value)
}
"#;
    let file = write_temp_paco("mutable_borrow_immutable_binding", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("mutable"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_mutable_field_borrow_while_shared_root_borrow_live() {
    let source = r#"
struct Box { value: int }

fn main() {
    let mut box: Box = Box { value: 1 }
    let shared: &Box = &box
    let field: &mut int = &mut box.value
    print(shared.value)
    print(field)
}
"#;
    let file = write_temp_paco("mutable_field_borrow_while_shared_root_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_field_assignment_through_shared_borrow() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    let mut shared: &Box = &box
    shared.value = 2
}
"#;
    let file = write_temp_paco("field_assignment_through_shared_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("mutable") || error.contains("immutable"));
    assert!(error.contains("shared"));
}

#[test]
fn check_rejects_mutable_field_borrow_through_shared_borrow() {
    let source = r#"
struct Box { value: int }

fn main() {
    let box: Box = Box { value: 1 }
    let mut shared: &Box = &box
    let field: &mut int = &mut shared.value
    print(field)
}
"#;
    let file = write_temp_paco("mutable_field_borrow_through_shared_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("mutable") || error.contains("shared"));
    assert!(error.contains("shared"));
}

#[test]
fn check_allows_field_assignment_through_mutable_borrow() {
    let source = r#"
struct Box { value: int }

fn main() {
    let mut box: Box = Box { value: 1 }
    let unique: &mut Box = &mut box
    unique.value = 2
}
"#;
    let file = write_temp_paco("field_assignment_through_mutable_borrow", source);
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
fn check_rejects_return_reference_to_local_owner() {
    let source = r#"
struct Box { value: int }

fn bad() -> &Box {
    let box: Box = Box { value: 1 }
    &box
}
"#;
    let file = write_temp_paco("return_local_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_return_reference_to_temporary_owner() {
    let source = r#"
struct Box { value: int }

fn make_box() -> Box {
    Box { value: 1 }
}

fn bad() -> &Box {
    &make_box()
}
"#;
    let file = write_temp_paco("return_temporary_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("temporary"));
}

#[test]
fn check_rejects_return_statement_reference_to_temporary_owner() {
    let source = r#"
struct Box { value: int }

fn make_box() -> Box {
    Box { value: 1 }
}

fn bad() -> &Box {
    return &make_box()
}
"#;
    let file = write_temp_paco("return_statement_temporary_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("temporary"));
}

#[test]
fn check_rejects_return_statement_reference_to_block_local_owner() {
    let source = r#"
struct Box { value: int }

fn bad() -> &Box {
    return {
        let box: Box = Box { value: 1 }
        &box
    }
}
"#;
    let file = write_temp_paco("return_statement_block_local_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_return_statement_if_temporary_borrow() {
    let source = r#"
struct Box { value: int }

fn make_box() -> Box {
    Box { value: 1 }
}

fn bad(flag: bool) -> &Box {
    return if flag { &make_box() } else { &make_box() }
}
"#;
    let file = write_temp_paco("return_statement_if_temporary_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("temporary"));
}

#[test]
fn check_rejects_return_statement_if_local_borrow_from_distinct_owners() {
    let source = r#"
struct Box { value: int }

fn bad(flag: bool) -> &Box {
    let left: Box = Box { value: 1 }
    let right: Box = Box { value: 2 }
    return if flag { &left } else { &right }
}
"#;
    let file = write_temp_paco("return_statement_if_local_borrow_distinct_owners", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("left") || error.contains("right"));
}

#[test]
fn check_rejects_return_statement_match_local_borrow_from_distinct_owners() {
    let source = r#"
struct Box { value: int }

fn bad(flag: bool) -> &Box {
    let left: Box = Box { value: 1 }
    let right: Box = Box { value: 2 }
    return match flag {
        true => &left,
        false => &right,
    }
}
"#;
    let file = write_temp_paco(
        "return_statement_match_local_borrow_distinct_owners",
        source,
    );
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("left") || error.contains("right"));
}

#[test]
fn check_rejects_return_reference_to_by_value_parameter() {
    let source = r#"
struct Box { value: int }

fn bad(box: Box) -> &Box {
    &box
}
"#;
    let file = write_temp_paco("return_parameter_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_parameter_borrow_alias() {
    let source = r#"
struct Box { value: int }

fn bad(box: Box) -> &Box {
    let borrowed: &Box = &box
    borrowed
}
"#;
    let file = write_temp_paco("return_parameter_borrow_alias", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_parameter_borrow_through_call() {
    let source = r#"
struct Box { value: int }

fn identity(value: &Box) -> &Box {
    value
}

fn bad(box: Box) -> &Box {
    identity(&box)
}
"#;
    let file = write_temp_paco("return_parameter_borrow_call", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_parameter_borrow_through_method_argument() {
    let source = r#"
struct Box { value: int }

struct Helper {
    value: int

    fn choose<'a>(self&, other: &'a Box) -> &'a Box {
        other
    }
}

fn bad(helper: Helper, box: Box) -> &Box {
    helper.choose(&box)
}
"#;
    let file = write_temp_paco("return_parameter_borrow_method_argument", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_local_borrow_alias() {
    let source = r#"
struct Box { value: int }

fn bad() -> &Box {
    let box: Box = Box { value: 1 }
    let borrowed: &Box = &box
    borrowed
}
"#;
    let file = write_temp_paco("return_local_borrow_alias", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_local_borrow_through_call() {
    let source = r#"
struct Box { value: int }

fn identity(value: &Box) -> &Box {
    value
}

fn bad() -> &Box {
    let box: Box = Box { value: 1 }
    identity(&box)
}
"#;
    let file = write_temp_paco("return_local_borrow_call", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_temporary_borrow_through_call() {
    let source = r#"
struct Box { value: int }

fn make_box() -> Box {
    Box { value: 1 }
}

fn identity(value: &Box) -> &Box {
    value
}

fn bad() -> &Box {
    identity(&make_box())
}
"#;
    let file = write_temp_paco("return_temporary_borrow_call", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("temporary"));
}

#[test]
fn check_rejects_return_statement_temporary_borrow_through_call() {
    let source = r#"
struct Box { value: int }

fn make_box() -> Box {
    Box { value: 1 }
}

fn identity(value: &Box) -> &Box {
    value
}

fn bad() -> &Box {
    return identity(&make_box())
}
"#;
    let file = write_temp_paco("return_statement_temporary_borrow_call", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("temporary"));
}

#[test]
fn check_rejects_returning_local_borrow_through_method_call() {
    let source = r#"
struct Box {
    value: int

    fn identity(self&) -> &Box {
        &self
    }
}

fn bad() -> &Box {
    let box: Box = Box { value: 1 }
    box.identity()
}
"#;
    let file = write_temp_paco("return_local_borrow_method_call", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_local_borrow_through_method_argument() {
    let source = r#"
struct Box { value: int }

struct Helper {
    value: int

    fn choose<'a>(self&, other: &'a Box) -> &'a Box {
        other
    }
}

fn bad(helper: Helper) -> &Box {
    let box: Box = Box { value: 1 }
    helper.choose(&box)
}
"#;
    let file = write_temp_paco("return_local_borrow_method_argument", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_accepts_method_return_lifetime_resolved_by_receiver_type() {
    let source = r#"
struct Box { value: int }

struct Keeper {
    value: int

    fn choose<'a>(self&, input: &'a Box, local: &Box) -> &'a Box {
        input
    }
}

struct Leaker {
    value: int

    fn choose<'b>(self&, input: &Box, local: &'b Box) -> &'b Box {
        local
    }
}

fn choose_input(keeper: Keeper, input: &Box) -> &Box {
    let local: Box = Box { value: 1 }
    keeper.choose(input, &local)
}
"#;
    let file = write_temp_paco("method_receiver_lifetime_resolution", source);
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
fn check_rejects_method_return_lifetime_resolved_by_receiver_type() {
    let source = r#"
struct Box { value: int }

struct Keeper {
    value: int

    fn choose<'a>(self&, input: &'a Box, local: &Box) -> &'a Box {
        input
    }
}

struct Leaker {
    value: int

    fn choose<'b>(self&, input: &Box, local: &'b Box) -> &'b Box {
        local
    }
}

fn leak_local(leaker: Leaker, input: &Box) -> &Box {
    let local: Box = Box { value: 1 }
    leaker.choose(input, &local)
}
"#;
    let file = write_temp_paco("method_receiver_lifetime_rejection", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("local"));
}

#[test]
fn check_rejects_method_return_lifetime_from_factory_receiver() {
    let source = r#"
struct Box { value: int }

struct Leaker {
    value: int

    fn choose<'b>(self&, input: &Box, local: &'b Box) -> &'b Box {
        local
    }
}

fn make_leaker() -> Leaker {
    Leaker { value: 1 }
}

fn leak_local(input: &Box) -> &Box {
    let local: Box = Box { value: 1 }
    make_leaker().choose(input, &local)
}
"#;
    let file = write_temp_paco("method_factory_receiver_lifetime_rejection", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("local"));
}

#[test]
fn check_rejects_method_return_lifetime_from_inferred_receiver_binding() {
    let source = r#"
struct Box { value: int }

struct Leaker {
    value: int

    fn choose<'b>(self&, input: &Box, local: &'b Box) -> &'b Box {
        local
    }
}

fn make_leaker() -> Leaker {
    Leaker { value: 1 }
}

fn leak_local(input: &Box) -> &Box {
    let leaker = make_leaker()
    let local: Box = Box { value: 1 }
    leaker.choose(input, &local)
}
"#;
    let file = write_temp_paco("method_inferred_receiver_lifetime_rejection", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("local"));
}

#[test]
fn check_allows_returned_method_borrow_origin_resolved_by_receiver_type() {
    let source = r#"
struct Box { value: int }

struct Keeper {
    value: int

    fn choose<'a>(self&, input: &'a Box, local: &Box) -> &'a Box {
        input
    }
}

struct Leaker {
    value: int

    fn choose<'b>(self&, input: &Box, local: &'b Box) -> &'b Box {
        local
    }
}

fn consume(value: Box) {}

fn main() {
    let keeper: Keeper = Keeper { value: 1 }
    let input: Box = Box { value: 1 }
    let local: Box = Box { value: 2 }
    let borrowed: &Box = keeper.choose(&input, &local)
    consume(local)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("method_receiver_returned_borrow_origin", source);
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
fn check_rejects_returned_method_borrow_origin_resolved_by_receiver_type() {
    let source = r#"
struct Box { value: int }

struct Keeper {
    value: int

    fn choose<'a>(self&, input: &'a Box, local: &Box) -> &'a Box {
        input
    }
}

struct Leaker {
    value: int

    fn choose<'b>(self&, input: &Box, local: &'b Box) -> &'b Box {
        local
    }
}

fn consume(value: Box) {}

fn main() {
    let leaker: Leaker = Leaker { value: 1 }
    let input: Box = Box { value: 1 }
    let local: Box = Box { value: 2 }
    let borrowed: &Box = leaker.choose(&input, &local)
    consume(local)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("method_receiver_returned_borrow_origin_rejection", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow"));
    assert!(error.contains("local"));
}

#[test]
fn check_rejects_explicit_lifetime_reference_to_local_owner() {
    let source = r#"
struct Box { value: int }

fn bad<'a>() -> &'a Box {
    let box: Box = Box { value: 1 }
    &box
}
"#;
    let file = write_temp_paco("explicit_lifetime_local_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_returning_local_borrow_when_input_borrow_exists() {
    let source = r#"
struct Box { value: int }

fn bad(input: &Box) -> &Box {
    let box: Box = Box { value: 1 }
    &box
}
"#;
    let file = write_temp_paco("local_borrow_with_input_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_infers_return_reference_from_single_input() {
    let source = r#"
struct Box { value: int }

fn identity(value: &Box) -> &Box {
    value
}

fn main() {
    let box: Box = Box { value: 1 }
    let borrowed: &Box = identity(&box)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("single_input_return_borrow", source);
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
fn check_accepts_explicit_lifetime_annotation() {
    let source = r#"
struct Box { value: int }

fn identity<'a>(value: &'a Box) -> &'a Box {
    value
}

fn main() {
    let box: Box = Box { value: 1 }
    let borrowed: &Box = identity(&box)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("explicit_lifetime_annotation", source);
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
fn check_allows_explicit_lifetime_return_from_input_when_local_borrow_argument_exists() {
    let source = r#"
struct Box { value: int }

fn first<'a>(input: &'a Box, local: &Box) -> &'a Box {
    input
}

fn choose(input: &Box) -> &Box {
    let box: Box = Box { value: 1 }
    first(input, &box)
}
"#;
    let file = write_temp_paco("explicit_lifetime_input_with_local_borrow_argument", source);
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
fn check_rejects_ambiguous_return_reference_from_multiple_inputs() {
    let source = r#"
struct Box { value: int }

fn choose(left: &Box, right: &Box, flag: bool) -> &Box {
    if flag { left } else { right }
}
"#;
    let file = write_temp_paco("ambiguous_return_borrow", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("ambiguous"));
    assert!(error.contains("'a"));
}

#[test]
fn check_rejects_borrow_escaping_block() {
    let source = r#"
struct Box { value: int }

fn main() {
    let leaked: &Box = {
        let box: Box = Box { value: 1 }
        &box
    }
    print(leaked.value)
}
"#;
    let file = write_temp_paco("borrow_escaping_block", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("lifetime") || error.contains("outlive"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_while_struct_reference_field_is_live() {
    let source = r#"
struct Box { value: int }
struct Holder { item: &Box }

fn consume(box: Box) {
    print(box.value)
}

fn main() {
    let box: Box = Box { value: 1 }
    let holder: Holder = Holder { item: &box }
    consume(box)
    print(holder.item.value)
}
"#;
    let file = write_temp_paco("struct_reference_field_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow") || error.contains("move"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_while_reference_extracted_from_field_is_live() {
    let source = r#"
struct Box { value: int }
struct Holder { item: &Box }

fn consume(box: Box) {
    print(box.value)
}

fn main() {
    let box: Box = Box { value: 1 }
    let holder: Holder = Holder { item: &box }
    let borrowed: &Box = holder.item
    consume(box)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("reference_extracted_from_field_live", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow") || error.contains("move"));
    assert!(error.contains("box"));
}

#[test]
fn check_rejects_move_after_reference_assignment_to_target() {
    let source = r#"
struct Box { value: int }

fn consume(box: Box) {
    print(box.value)
}

fn main() {
    let box: Box = Box { value: 1 }
    let other: Box = Box { value: 2 }
    let mut borrowed: &Box = &other
    borrowed = &box
    consume(box)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("reference_assignment_to_target", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow") || error.contains("move"));
    assert!(error.contains("box"));
}

#[test]
fn check_allows_move_after_reference_assignment_replaces_old_target() {
    let source = r#"
struct Box { value: int }

fn consume(box: Box) {
    print(box.value)
}

fn main() {
    let box: Box = Box { value: 1 }
    let other: Box = Box { value: 2 }
    let mut borrowed: &Box = &box
    borrowed = &other
    consume(box)
    print(borrowed.value)
}
"#;
    let file = write_temp_paco("reference_assignment_replaces_old_target", source);
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
fn check_rejects_move_after_reference_field_assignment_to_target() {
    let source = r#"
struct Box { value: int }
struct Holder { item: &Box }

fn consume(box: Box) {
    print(box.value)
}

fn main() {
    let box: Box = Box { value: 1 }
    let other: Box = Box { value: 2 }
    let mut holder: Holder = Holder { item: &other }
    holder.item = &box
    consume(box)
    print(holder.item.value)
}
"#;
    let file = write_temp_paco("reference_field_assignment_to_target", source);
    let cli = paco_driver::Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("borrow") || error.contains("move"));
    assert!(error.contains("box"));
}

#[test]
fn check_allows_move_after_reference_field_assignment_replaces_old_target() {
    let source = r#"
struct Box { value: int }
struct Holder { item: &Box }

fn consume(box: Box) {
    print(box.value)
}

fn main() {
    let box: Box = Box { value: 1 }
    let other: Box = Box { value: 2 }
    let mut holder: Holder = Holder { item: &box }
    holder.item = &other
    consume(box)
    print(holder.item.value)
}
"#;
    let file = write_temp_paco("reference_field_assignment_replaces_old_target", source);
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

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_borrowing_{}_{}_{}.paco",
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
