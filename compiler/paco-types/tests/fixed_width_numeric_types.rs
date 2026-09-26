use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::ast::Item;
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

fn parse_source(source: &str) -> paco_syntax::ast::Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    assert!(!reporter.has_errors());
    module
}

#[test]
fn u8_and_byte_resolve_to_the_same_type() {
    let module = parse_source("fn f(a: u8, b: byte) -> bool { a == b }");
    let mut reporter = Reporter::new();
    let typed = infer_module(&module, &mut reporter).expect("module should type-check");

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected fn item");
    };
    let a_ty = typed.type_of_param(&function.params[0]);
    let b_ty = typed.type_of_param(&function.params[1]);
    assert_eq!(a_ty, Some(&Type::Int(IntWidth::U8)));
    assert_eq!(b_ty, Some(&Type::Int(IntWidth::U8)));
}

#[test]
fn int_is_rejected_with_a_dedicated_diagnostic() {
    let error = check_source("fn main() { let x: int = 5; }").expect("expected an error");
    assert!(error.contains("PACO-E0328"));
    assert!(error.contains("int"));
    assert!(error.contains("i64"));
}

#[test]
fn uint_is_rejected_with_a_dedicated_diagnostic() {
    let error = check_source("fn main() { let x: uint = 5; }").expect("expected an error");
    assert!(error.contains("PACO-E0328"));
    assert!(error.contains("uint"));
    assert!(error.contains("u64"));
}

#[test]
fn every_fixed_width_integer_type_checks_as_a_function_parameter() {
    for name in ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"] {
        let source = format!("fn f(x: {name}) -> {name} {{ x }}");
        let error = check_source(&source);
        assert!(error.is_none(), "`{name}` should type-check cleanly: {error:?}");
    }
}

#[test]
fn a_literal_within_range_is_accepted_for_a_narrow_target() {
    let error = check_source("fn main() { let x: u8 = 200; }");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn an_out_of_range_literal_is_rejected_naming_value_and_range() {
    let error = check_source("fn main() { let x: u8 = 300; }").expect("expected an error");
    assert!(error.contains("PACO-E0329"));
    assert!(error.contains("300"));
    assert!(error.contains("u8"));
}
