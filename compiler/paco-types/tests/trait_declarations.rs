use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::check_module;

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
fn trait_with_an_abstract_method_type_checks() {
    let error = check_source("trait Greet { fn hello(&self) -> string; }");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn trait_name_colliding_with_a_struct_is_rejected() {
    let error = check_source("struct Widget { x: i64 } trait Widget { fn hello(&self); }")
        .expect("expected error");
    assert!(error.contains("PACO-E0310"));
}

#[test]
fn duplicate_method_name_within_a_trait_is_rejected() {
    let error = check_source(
        "trait Shape { fn area(&self) -> float; fn area(&self) -> float; }",
    )
    .expect("expected error");
    assert!(error.contains("PACO-E0320"));
}

#[test]
fn duplicate_assoc_type_name_within_a_trait_is_rejected() {
    let error = check_source("trait Container { type Output; type Output; }")
        .expect("expected error");
    assert!(error.contains("PACO-E0321"));
}

#[test]
fn index_trait_with_self_output_and_unbound_generic_type_checks_with_no_diagnostics() {
    let error = check_source(
        "trait Index<Idx> { type Output; fn index(&self, i: Idx) -> &Self::Output; }",
    );
    assert!(error.is_none(), "{error:?}");
}
