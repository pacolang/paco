use paco_diag::Reporter;
use paco_mir::{Body, Operand, Rvalue, Statement, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn lower_source(source: &str) -> Body {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops =
        paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = TypeRegistry::from_module(&module);

    let function = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .expect("expected a `main` function");
    paco_mir::lower_function(function, &typed, &registry, &drops, paco_mir::Profile::Debug).0
}

#[test]
fn struct_literal_fields_are_stored_in_declaration_order_regardless_of_source_order() {
    let body = lower_source("struct Point { x: i64, y: i64 } fn main() -> Point { Point { y: 2, x: 1 } }");

    let aggregate = body
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find_map(|statement| match statement {
            Statement::Assign(_, rvalue @ Rvalue::Aggregate { variant: None, .. }) => Some(rvalue),
            _ => None,
        })
        .expect("expected a struct-literal aggregate assignment");

    let Rvalue::Aggregate { fields, .. } = aggregate else {
        unreachable!()
    };
    assert_eq!(
        fields,
        &[
            Operand::Constant(paco_mir::Constant::Int(1, paco_types::IntWidth::I64)),
            Operand::Constant(paco_mir::Constant::Int(2, paco_types::IntWidth::I64)),
        ]
    );
}
