use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{Type, infer_module};

#[test]
fn slice_type_resolves_to_type_slice() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn f(x: []float) -> []float { x }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let tail = function.body.tail.as_deref().unwrap();
    assert_eq!(typed.type_of(tail), Some(&Type::Slice(Box::new(Type::Float(paco_types::FloatWidth::F64)))));
}
