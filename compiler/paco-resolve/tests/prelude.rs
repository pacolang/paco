use paco_diag::Reporter;
use paco_resolve::resolve_module;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};

fn resolve_source(source: &str) -> bool {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    resolve_module(&module, &mut reporter).is_ok()
}

#[test]
fn print_resolves_without_a_local_declaration() {
    assert!(resolve_source("fn main() { print(1) }"));
}

#[test]
fn bare_prelude_names_resolve_without_a_local_declaration_or_use() {
    for source in [
        "fn main() { Some(1) }",
        "fn main() { None }",
        "fn main() { Ok(1) }",
        "fn main() { Err(1) }",
        "fn main() { panic(\"msg\") }",
    ] {
        assert!(resolve_source(source), "{source} should resolve");
    }
}

#[test]
fn an_unrelated_undeclared_identifier_still_fails_to_resolve() {
    assert!(!resolve_source("fn main() { totally_undeclared_name }"));
}

#[test]
fn extern_block_function_names_resolve_as_calls() {
    assert!(resolve_source(
        "extern \"C\" { fn risky(); } fn main() { unsafe { risky() } }"
    ));
}

#[test]
fn shadowing_bindings_get_distinct_ids_and_uses_resolve_to_the_latest() {
    use paco_syntax::ast::{Expr, Item, Stmt};
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() { let s = 1; let s = s; print(s); }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");
    let locals = paco_resolve::resolve_locals([&module]);
    let Item::Fn(main) = &module.items[0] else { panic!("expected fn") };
    let [Stmt::Let(first), Stmt::Let(second), Stmt::Expr(Expr::Call { args, .. })] = main.body.stmts.as_slice() else {
        panic!("unexpected body shape")
    };
    let (first_id, second_id) = (locals.pat(&first.pattern).unwrap(), locals.pat(&second.pattern).unwrap());
    assert_ne!(first_id, second_id);
    assert_eq!(locals.expr(second.value.as_ref().unwrap()), Some(first_id));
    assert_eq!(locals.expr(&args[0]), Some(second_id));
}

#[test]
fn a_declared_type_name_resolves_as_a_type_value() {
    assert!(resolve_source("struct User { age: i64 } enum Kind { A } fn main() { fields_of(User); type_name(Kind) }"));
}
