use paco_diag::Reporter;
use paco_mir::{BinOp, Profile, Rvalue, Statement, Terminator};
use paco_span::{SourceMap, Span};
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

const SOURCE: &str = "fn main() {\n    let a = 1 + 2;\n    let b = a * 3;\n    print(b);\n}\n";

fn text(span: Span) -> &'static str {
    &SOURCE[span.start()..span.end()]
}

#[test]
fn statements_terminators_and_bodies_carry_their_source_spans() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", SOURCE);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    let typed = infer_module(&module, &mut reporter).unwrap();
    let drops = paco_borrow::analyze_module(&module, &mut reporter).unwrap();
    let registry = paco_mir::TypeRegistry::from_module(&module);
    let Some(Item::Fn(main)) = module.items.first() else { panic!("expected `main`") };
    let body = paco_mir::lower_function(main, &typed, &registry, &drops, Profile::Debug).0;

    assert!(text(body.span).starts_with("fn main()"), "{:?}", text(body.span));
    let mut seen = Vec::new();
    for (block_index, block) in body.blocks.iter().enumerate() {
        for (index, statement) in block.statements.iter().enumerate() {
            if let Statement::Assign(_, Rvalue::BinaryOp(op, ..)) = statement {
                seen.push((format!("{op:?}"), text(body.statement_span(block_index, index))));
            }
        }
        if let Terminator::Call { target, .. } = &block.terminator {
            seen.push((target.0.clone(), text(body.terminator_span(block_index))));
        }
    }
    assert_eq!(
        seen,
        [(format!("{:?}", BinOp::Add), "1 + 2"), (format!("{:?}", BinOp::Mul), "a * 3"), ("print".to_string(), "print(b)")]
    );
}

#[test]
fn the_source_locator_resolves_lines_and_columns() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", SOURCE);
    let locator = paco_mir::SourceLocator::new(&sources);
    let start = SOURCE.find("a * 3").unwrap();
    assert_eq!(locator.locate(Span::new(file, start, start + 5)), Some(("main.paco", 3, 13)));
    assert_eq!(locator.locate(Span::new(file, 0, 2)), Some(("main.paco", 1, 1)));
}
