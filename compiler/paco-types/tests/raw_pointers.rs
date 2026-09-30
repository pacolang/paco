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
