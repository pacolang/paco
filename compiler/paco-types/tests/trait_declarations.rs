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

#[test]
fn unknown_type_in_a_trait_method_signature_is_reported_at_that_type() {
    let error = check_source("trait T { fn f(&self) -> Missing; }").expect("expected error");
    assert!(error.contains("PACO-E0306"), "{error}");
    assert!(error.contains("Missing"), "{error}");
}

#[test]
fn a_bound_naming_an_undeclared_trait_is_rejected() {
    let error = check_source("fn f<T: Nope>(x: T) {}").expect("expected error");
    assert!(error.contains("PACO-E0349"), "{error}");
}

#[test]
fn a_bound_naming_a_declared_trait_is_accepted() {
    let error = check_source("trait Shape { fn area(&self) -> i64; } fn f<T: Shape>(x: &T) {}");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_bound_naming_a_builtin_trait_is_accepted() {
    let error = check_source("fn f<T: Add + Copy>(x: T) {}");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_method_call_on_a_bounded_generic_resolves_through_the_bound() {
    let error = check_source(
        "
trait Shape {
    fn area(&self) -> i64;
}
fn total<T: Shape>(x: &T) -> i64 { x.area() }
",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn an_operator_on_a_bounded_generic_resolves_through_the_bound() {
    let error = check_source(
        "
trait Add {
    fn add(&self, other: Self) -> Self;
}
fn sum<T: Add + Copy>(a: T, b: T) -> T { a + b }
",
    );
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn a_method_not_provided_by_any_bound_is_rejected() {
    let error = check_source(
        "
trait Shape {
    fn area(&self) -> i64;
}
fn f<T: Shape>(x: &T) -> i64 { x.perimeter() }
",
    )
    .expect("expected error");
    assert!(error.contains("PACO-E0314"), "{error}");
    assert!(error.contains("perimeter") && error.contains("Shape"), "{error}");
}
