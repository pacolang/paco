use std::collections::HashMap;

use paco_diag::Reporter;
use paco_mir::{Body, ComptimeValue, Constant, InstantiationRegistry, Operand, Profile, Rvalue, Statement, Terminator};
use paco_span::SourceMap;
use paco_syntax::ast::{FnDecl, Item, Module};
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{IntWidth, infer_module};

const SOURCE: &str = "struct User {\n    age: i64,\n}\n\ncomptime fn describe(t: type) -> string {\n    let code = quote { #(type_name(t)) };\n    code_to_string(code)\n}\n\nfn main() {\n    print(comptime { 40 + 2 });\n    print(comptime { describe(User) });\n}\n";

fn parse() -> Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", SOURCE);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    parse_module(&tokens, &mut reporter).unwrap()
}

fn function<'m>(module: &'m Module, name: &str) -> &'m FnDecl {
    module.items.iter().find_map(|item| match item {
        Item::Fn(function) if function.name == name => Some(function),
        _ => None,
    }).unwrap()
}

fn quotes(body: &Body) -> usize {
    body.blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter(|statement| matches!(statement, Statement::Assign(_, Rvalue::Quote { .. })))
        .count()
}

fn calls(body: &Body) -> Vec<&str> {
    body.blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Call { target, .. } => Some(target.0.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn comptime_blocks_are_outlined_then_replaced_by_their_values() {
    let module = parse();
    let mut reporter = Reporter::new();
    let typed = infer_module(&module, &mut reporter).unwrap();
    let drops = paco_borrow::analyze_module(&module, &mut reporter).unwrap();
    let registry = paco_mir::TypeRegistry::from_module(&module);
    let main = function(&module, "main");
    let empty = HashMap::new();

    let outlining = InstantiationRegistry::new();
    let (body, outlined) =
        paco_mir::lower_function_with_substitutions(main, &typed, &registry, &drops, Profile::Debug, &empty, &outlining);
    let sites = outlining.take_comptime_sites();
    assert_eq!(sites.len(), 2);
    assert!(sites.iter().all(|site| calls(&body).contains(&site.name.as_str())), "{:?}", calls(&body));
    assert!(sites.iter().all(|site| outlined.iter().any(|(name, _)| *name == site.name)));
    assert_eq!(quotes(&body), 0);

    let describe = function(&module, "describe");
    let (describe_body, _) = paco_mir::lower_function(describe, &typed, &registry, &drops, Profile::Debug);
    assert_eq!(quotes(&describe_body), 1);

    let values = HashMap::from([
        (sites[0].key.clone(), ComptimeValue::Scalar(Constant::Int(42, IntWidth::I64))),
        (sites[1].key.clone(), ComptimeValue::Scalar(Constant::Str("User".to_string()))),
    ]);
    let embedding = InstantiationRegistry::new();
    embedding.set_comptime_values(values);
    let (body, outlined) =
        paco_mir::lower_function_with_substitutions(main, &typed, &registry, &drops, Profile::Release, &empty, &embedding);
    assert!(outlined.is_empty());
    assert!(!embedding.has_comptime_sites());
    assert_eq!(quotes(&body), 0);
    assert_eq!(calls(&body), ["print", "print"]);
    let constants: Vec<&Operand> = body
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Call { args, .. } => args.first(),
            _ => None,
        })
        .collect();
    assert_eq!(
        constants,
        [
            &Operand::Constant(Constant::Int(42, IntWidth::I64)),
            &Operand::Constant(Constant::Str("User".to_string()))
        ]
    );
}
