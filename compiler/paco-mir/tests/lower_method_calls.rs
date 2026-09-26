use paco_diag::Reporter;
use paco_mir::{CallTarget, Profile, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

#[test]
fn method_calls_resolve_to_an_explicit_call_target() {
    let source = r#"
        struct Counter {
            value: i64,

            fn get(&self) -> i64 { self.value }
        }

        fn main(c: Counter) -> i64 {
            c.get()
        }
        "#;
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops = paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = TypeRegistry::from_module(&module);
    let function = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .unwrap();

    let (body, _outlined) = paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug);

    let call_target = body.blocks.iter().find_map(|block| match &block.terminator {
        Terminator::Call { target, .. } => Some(target.clone()),
        _ => None,
    });

    // No vtable slot, no method-name-only marker: the concrete function
    // `Counter::get` is named directly, exactly as an ordinary call would
    // name its target.
    assert_eq!(call_target, Some(CallTarget("Counter::get".to_string())));
}
