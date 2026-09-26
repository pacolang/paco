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
fn channel_of_i64_type_checks_to_a_sender_receiver_pair() {
    let module = parse_source("fn main() { let pair = channel<i64>(capacity: 8); }");
    let mut reporter = Reporter::new();

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let stmt = &function.body.stmts[0];
    let paco_syntax::ast::Stmt::Let(let_stmt) = stmt else {
        panic!("expected a let statement");
    };
    let value = let_stmt.value.as_ref().expect("expected an initializer");
    assert!(matches!(value, Expr::Call { .. }));
    assert_eq!(
        typed.type_of(value),
        Some(&Type::Tuple(vec![
            Type::Struct("Sender".to_string(), vec![Type::Int(IntWidth::I64)]),
            Type::Struct("Receiver".to_string(), vec![Type::Int(IntWidth::I64)]),
        ]))
    );
}

#[test]
fn channel_destructures_into_sender_and_receiver_bindings() {
    let source = "fn main() { let (tx, rx) = channel<i64>(capacity: 8); }";
    let module = parse_source(source);
    let mut reporter = Reporter::new();

    infer_module(&module, &mut reporter).expect("module should type-check");
}
