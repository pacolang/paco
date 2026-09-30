use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{IntWidth, Type, check_module, infer_module};

fn check_source(source: &str) -> Option<String> {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    if check_module(&module, &mut reporter).is_err() {
        Some(reporter.emit_to_string(&sources))
    } else {
        None
    }
}

#[test]
fn extern_function_resolves_declared_param_and_return_types() {
    let source = "extern \"C\" { fn cblas_sgemm(a: i64, n: i64) -> i64; }
        fn f() -> i64 { unsafe { cblas_sgemm(1, 2) } }";
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn extern_function_call_checks_argument_types() {
    let source = "extern \"C\" { fn cblas_sgemm(a: i64, n: i64) -> i64; }
        fn f() -> i64 { unsafe { cblas_sgemm(true, 2) } }";
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0302"));
}

#[test]
fn in_unsafe_flag_resets_after_the_unsafe_block_ends() {
    let source = "extern \"C\" { fn risky(); }
        fn f() { unsafe { risky() } risky() }";
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0325"));
}

#[test]
fn unsafe_block_type_checks_to_its_tail_expression_type() {
    let mut sources = SourceMap::new();
    let source = "fn f() -> i64 { unsafe { 1 + 1 } }";
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");

    let paco_syntax::ast::Item::Fn(f) = &module.items[0] else {
        panic!("expected fn item");
    };
    let tail = f.body.tail.as_ref().unwrap();
    assert!(matches!(tail.as_ref(), paco_syntax::ast::Expr::Unsafe(_, _)));
    assert_eq!(typed.type_of(tail), Some(&Type::Int(IntWidth::I64)));
}

#[test]
fn empty_unsafe_block_has_unit_type() {
    let error = check_source("fn f() { unsafe { } }");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn calling_an_extern_function_outside_unsafe_is_rejected() {
    let source = "extern \"C\" { fn risky(); } fn f() { risky() }";
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0325"));
}

#[test]
fn calling_an_extern_function_inside_unsafe_is_accepted() {
    let source = "extern \"C\" { fn risky(); } fn f() { unsafe { risky() } }";
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn calling_an_unsafe_fn_outside_unsafe_is_rejected() {
    let source = "unsafe fn risky() { } fn f() { risky() }";
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0325"));
}

#[test]
fn calling_a_plain_function_is_unaffected() {
    let error = check_source("fn risky() { } fn f() { risky() }");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn deref_inside_unsafe_yields_the_pointee_type() {
    let mut sources = SourceMap::new();
    let source = "fn f(p: *const i64) -> i64 { unsafe { *p } }";
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");

    let paco_syntax::ast::Item::Fn(f) = &module.items[0] else {
        panic!("expected fn item");
    };
    let paco_syntax::ast::Expr::Unsafe(block, _) = f.body.tail.as_ref().unwrap().as_ref() else {
        panic!("expected unsafe tail expression");
    };
    let deref = block.tail.as_ref().unwrap();
    assert_eq!(typed.type_of(deref), Some(&Type::Int(IntWidth::I64)));
}

#[test]
fn deref_outside_unsafe_is_rejected() {
    let error = check_source("fn f(p: *const i64) -> i64 { *p }");
    let error = error.expect("expected an error");
    assert!(error.contains("PACO-E0325"));
}

#[test]
fn dereferencing_a_non_pointer_is_a_type_error() {
    let error = check_source("fn f(p: i64) -> i64 { unsafe { *p } }");
    let error = error.expect("expected an error");
    assert!(error.contains("PACO-E0301"));
}

#[test]
fn unsafe_fn_body_is_not_an_implicit_unsafe_context_for_calls() {
    let source = "unsafe fn a() { } unsafe fn b() { a() }";
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0325"));
}

#[test]
fn unsafe_fn_body_is_not_an_implicit_unsafe_context_for_deref() {
    let source = "unsafe fn deref_it(p: *const i64) -> i64 { *p }";
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0325"));
}

#[test]
fn unsafe_fn_body_with_its_own_inner_unsafe_block_is_accepted() {
    let source = "unsafe fn a() { } unsafe fn b() { unsafe { a() } }";
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_link_attribute_naming_a_library_is_accepted() {
    let source = r#"#[link(name = "m")] extern "C" { fn cbrt(x: f64) -> f64; }"#;
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_link_attribute_with_name_and_kind_is_accepted() {
    let source = r#"#[link(name = "m", kind = "dylib")] extern "C" { fn cbrt(x: f64) -> f64; }"#;
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn an_extern_block_without_link_is_still_accepted() {
    let source = r#"extern "C" { fn getpid() -> i32; }"#;
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_link_attribute_with_an_unknown_argument_is_rejected() {
    let source = r#"#[link(namee = "m")] extern "C" { fn cbrt(x: f64) -> f64; }"#;
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0356"), "{error}");
}

#[test]
fn a_link_attribute_with_an_invalid_kind_is_rejected() {
    let source = r#"#[link(name = "m", kind = "shared")] extern "C" { fn cbrt(x: f64) -> f64; }"#;
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0357"), "{error}");
}

#[test]
fn declaring_a_c_function_pointer_parameter_type_is_accepted() {
    let source = r#"extern "C" {
        fn qsort(base: *mut i64, n: u64, size: u64, cmp: extern "C" fn(*const u8, *const u8) -> i32);
    }"#;
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn an_extern_c_fn_item_coerces_to_a_matching_c_function_pointer_type() {
    let source = r#"
        extern "C" fn cmp(a: *const u8, b: *const u8) -> i32 { 0 as i32 }
        fn take(f: extern "C" fn(*const u8, *const u8) -> i32) {}
        fn main() { take(cmp) }
    "#;
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_foreign_function_coerces_to_the_unsafe_c_function_pointer_type() {
    let source = r#"
        extern "C" { fn qsort_cmp(a: *const u8, b: *const u8) -> i32; }
        fn take(f: unsafe extern "C" fn(*const u8, *const u8) -> i32) {}
        fn main() { take(qsort_cmp) }
    "#;
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn an_ordinary_fn_item_does_not_coerce_to_a_c_function_pointer_type() {
    let source = r#"
        fn plain(a: i64) -> i64 { a }
        fn take(f: extern "C" fn(i64) -> i64) {}
        fn main() { take(plain) }
    "#;
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("unresolved identifier"), "{error}");
}

#[test]
fn a_closure_is_rejected_as_a_c_function_pointer() {
    let source = r#"
        fn take(f: extern "C" fn(*const u8, *const u8) -> i32) {}
        fn main() { take(|a, b| 0) }
    "#;
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0358"), "{error}");
}

#[test]
fn calling_a_c_function_pointer_outside_unsafe_is_rejected() {
    let source = "fn call_it(f: extern \"C\" fn(i64) -> i64) -> i64 { f(1) }";
    let error = check_source(source).expect("expected an error");
    assert!(error.contains("PACO-E0325"), "{error}");
}

#[test]
fn calling_a_c_function_pointer_inside_unsafe_is_accepted() {
    let source = "fn call_it(f: extern \"C\" fn(i64) -> i64) -> i64 { unsafe { f(1) } }";
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_plain_fn_type_parameter_still_accepts_a_closure() {
    let source = r#"
        fn take(f: fn(i64) -> i64) -> i64 { f(1) }
        fn main() { take(|x| x + 1); }
    "#;
    let error = check_source(source);
    assert!(error.is_none(), "{error:?}");
}
