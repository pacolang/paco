use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::ast::{Expr, Item};
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{IntWidth, Type, infer_module};

fn parse_source(source: &str) -> paco_syntax::ast::Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());
    module
}

#[test]
fn infer_module_attaches_a_type_to_a_sample_expression() {
    let module = parse_source("fn main() -> i64 { 1 + 2 }");
    let mut reporter = Reporter::new();

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let tail = function
        .body
        .tail
        .as_deref()
        .expect("expected tail expression");
    assert!(matches!(tail, Expr::Binary { .. }));
    assert_eq!(typed.type_of(tail), Some(&Type::Int(IntWidth::I64)));
}

#[test]
fn infer_module_returns_the_same_diagnostics_as_check_module() {
    let module = parse_source("fn main() -> i64 { true }");
    let mut infer_reporter = Reporter::new();
    let mut check_reporter = Reporter::new();

    let infer_result = infer_module(&module, &mut infer_reporter);
    let check_result = paco_types::check_module(&module, &mut check_reporter);

    assert!(infer_result.is_err());
    assert!(check_result.is_err());
    assert_eq!(
        infer_reporter.diagnostics()[0].code(),
        check_reporter.diagnostics()[0].code()
    );
}
