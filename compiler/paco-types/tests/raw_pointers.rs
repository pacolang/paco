use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::check_module;

fn check_source(source: &str) -> Option<String> {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    check_module(&module, &mut reporter).is_err().then(|| reporter.emit_to_string(&sources))
}

#[test]
fn a_shared_borrow_casts_to_a_const_pointer_of_the_same_type() {
    let error = check_source(
        "
        fn f(x: &i64) -> *const i64 { x as *const i64 }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_mutable_borrow_casts_to_a_mutable_pointer_of_the_same_type() {
    let error = check_source(
        "
        fn f(x: &mut i64) -> *mut i64 { x as *mut i64 }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_mutable_borrow_casts_to_a_const_pointer() {
    let error = check_source(
        "
        fn f(x: &mut i64) -> *const i64 { x as *const i64 }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_shared_borrow_cannot_cast_to_a_mutable_pointer() {
    let error = check_source(
        "
        fn f(x: &i64) -> *mut i64 { x as *mut i64 }
        fn main() {}
        ",
    )
    .expect("expected a cast error");
    assert!(error.contains("PACO-E0330"), "{error}");
}

#[test]
fn a_borrow_cannot_cast_to_a_pointer_of_a_different_pointee() {
    let error = check_source(
        "
        fn f(x: &i64) -> *const f64 { x as *const f64 }
        fn main() {}
        ",
    )
    .expect("expected a cast error");
    assert!(error.contains("PACO-E0330"), "{error}");
}

#[test]
fn a_pointer_casts_to_a_different_pointee_type() {
    let error = check_source(
        "
        struct Pt { x: i64, y: i64 }
        fn f(p: *const Pt) -> *const u8 { p as *const u8 }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_mutable_pointer_casts_to_a_const_pointer_of_the_same_type() {
    let error = check_source(
        "
        fn f(p: *mut i64) -> *const i64 { p as *const i64 }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_const_pointer_casts_to_a_mutable_pointer() {
    let error = check_source(
        "
        fn f(p: *const i64) -> *mut i64 { p as *mut i64 }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_pointer_casts_to_and_from_u64() {
    let error = check_source(
        "
        fn to_addr(p: *const i64) -> u64 { p as u64 }
        fn from_addr(n: u64) -> *const i64 { n as *const i64 }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_pointer_still_cannot_cast_to_a_non_pointer_non_integer_type() {
    let error = check_source(
        "
        fn f(p: *const i64) -> string { p as string }
        fn main() {}
        ",
    )
    .expect("expected a cast error");
    assert!(error.contains("PACO-E0330"), "{error}");
}

#[test]
fn offset_and_add_return_a_pointer_of_the_same_type() {
    let error = check_source(
        "
        fn f(p: *const i64) -> *const i64 { p.offset(1) }
        fn g(p: *mut i64) -> *mut i64 { p.add(1) }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn is_null_returns_a_bool_without_unsafe() {
    let error = check_source(
        "
        fn f(p: *const i64) -> bool { p.is_null() }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn read_requires_unsafe_and_returns_the_pointee_type() {
    let error = check_source(
        "
        fn f(p: *const i64) -> i64 { p.read() }
        fn main() {}
        ",
    )
    .expect("expected an unsafe-block error");
    assert!(error.contains("PACO-E0325"), "{error}");
}

#[test]
fn read_inside_unsafe_type_checks_with_no_diagnostics() {
    let error = check_source(
        "
        fn f(p: *const i64) -> i64 { unsafe { p.read() } }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn write_requires_unsafe_and_a_mutable_pointer() {
    let error = check_source(
        "
        fn f(p: *const i64) { unsafe { p.write(1) } }
        fn main() {}
        ",
    )
    .expect("expected a mutability error");
    assert!(error.contains("PACO-E0330"), "{error}");
}

#[test]
fn write_through_a_mutable_pointer_inside_unsafe_type_checks_with_no_diagnostics() {
    let error = check_source(
        "
        fn f(p: *mut i64) { unsafe { p.write(1) } }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn ptr_null_and_ptr_null_mut_produce_the_expected_pointer_types() {
    let error = check_source(
        "
        fn f() -> *const i64 { ptr_null<i64>() }
        fn g() -> *mut i64 { ptr_null_mut<i64>() }
        fn main() {}
        ",
    );
    assert!(error.is_none(), "{error:?}");
}
