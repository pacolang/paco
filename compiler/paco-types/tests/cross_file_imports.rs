//! `cross-file-modules` task 4.1: `paco_types::infer_module_with_imports`
//! seeds another file's `pub` items under a qualified (`"{qualifier}::
//! {name}"`) or bare (prelude, empty qualifier) key, so a cross-file
//! function call and struct construction both type-check.

use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{ast::Module, lex::lex, parse::parse_module};
use paco_types::{check_module_with_imports, infer_module_with_imports};

fn parsed(source: &str) -> Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));
    module
}

#[test]
fn a_qualified_function_call_type_checks() {
    let a = parsed("pub fn double(x: i64) -> i64 { x * 2 }");
    let b = parsed("fn main() -> i64 { a::double(3) }");
    let mut reporter = Reporter::new();

    let result = infer_module_with_imports(&b, &[("a".to_string(), &a)], &mut reporter);
    assert!(result.is_ok(), "{}", reporter.diagnostics().iter().map(|d| d.code()).collect::<Vec<_>>().join(", "));
}

#[test]
fn a_qualified_struct_construction_type_checks() {
    let a = parsed("pub struct Point { x: i64, y: i64 }");
    let b = parsed("fn main() { let p = a::Point { x: 1, y: 2 }; }");
    let mut reporter = Reporter::new();

    let result = check_module_with_imports(&b, &[("a".to_string(), &a)], &mut reporter);
    assert!(result.is_ok(), "{}", reporter.diagnostics().iter().map(|d| d.code()).collect::<Vec<_>>().join(", "));
}

#[test]
fn a_type_mismatch_against_an_imported_signature_is_still_caught() {
    let a = parsed("pub fn double(x: i64) -> i64 { x * 2 }");
    let b = parsed(r#"fn main() -> i64 { a::double("nope") }"#);
    let mut reporter = Reporter::new();

    let result = infer_module_with_imports(&b, &[("a".to_string(), &a)], &mut reporter);
    assert!(result.is_err(), "expected a type mismatch, got Ok");
}

#[test]
fn an_unqualified_prelude_item_resolves_with_an_empty_qualifier() {
    let core = parsed("pub struct Marker { value: i64 }");
    let b = parsed("fn main() { let m = Marker { value: 1 }; }");
    let mut reporter = Reporter::new();

    let result = check_module_with_imports(&b, &[(String::new(), &core)], &mut reporter);
    assert!(result.is_ok(), "{}", reporter.diagnostics().iter().map(|d| d.code()).collect::<Vec<_>>().join(", "));
}

#[test]
fn a_local_declaration_shadows_a_same_named_prelude_item() {
    let core = parsed("pub struct Marker { }");
    let b = parsed("struct Marker { value: i64 } fn main() { let m = Marker { value: 1 }; }");
    let mut reporter = Reporter::new();

    let result = check_module_with_imports(&b, &[(String::new(), &core)], &mut reporter);
    assert!(result.is_ok(), "{}", reporter.diagnostics().iter().map(|d| d.code()).collect::<Vec<_>>().join(", "));
}
