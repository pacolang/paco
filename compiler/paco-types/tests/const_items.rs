use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{check_module, infer_module};

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
fn module_level_const_type_checks() {
    let error = check_source("const TILE: i64 = 64;");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn associated_const_inside_struct_type_checks() {
    let error = check_source("struct Tensor<T> { data: T, const RANK: i64 = 2; }");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn duplicate_module_level_const_is_rejected() {
    let error = check_source("const TILE: i64 = 64; const TILE: i64 = 32;").expect("expected error");
    assert!(error.contains("PACO-E0317"));
}

#[test]
fn duplicate_associated_const_is_rejected() {
    let error = check_source(
        "struct Tensor<T> { data: T, const RANK: i64 = 2;, const RANK: i64 = 3; }",
    )
    .expect("expected error");
    assert!(error.contains("PACO-E0317"));
}

#[test]
fn const_initializer_type_mismatch_is_rejected() {
    let error = check_source("const TILE: i64 = true;").expect("expected error");
    assert!(error.contains("PACO-E0302"));
}

#[test]
fn literal_initializer_is_evaluable() {
    let error = check_source("const EPS: float = 0.000001;");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn operation_over_a_prior_const_is_evaluable() {
    let error = check_source("const TILE: i64 = 64; const DOUBLE_TILE: i64 = TILE * 2;");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn function_call_initializer_is_rejected_as_non_evaluable() {
    let error =
        check_source("fn compute_size() -> i64 { 1 } const SIZE: i64 = compute_size();")
            .expect("expected error");
    assert!(error.contains("PACO-E0318"));
}

#[test]
fn local_binding_initializer_is_rejected_as_non_evaluable() {
    let error =
        check_source("fn f(n: i64) -> i64 { n } const SIZE: i64 = f(1);").expect("expected error");
    assert!(error.contains("PACO-E0318"));
}

#[test]
fn module_level_const_resolves_by_name_in_a_function_body() {
    let error = check_source("const TILE: i64 = 64; fn main() -> i64 { TILE + 1 }");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn local_binding_shadows_a_same_named_const() {
    let error =
        check_source("const TILE: i64 = 64; fn main() -> bool { let TILE = true; TILE }");
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn unresolved_identifier_is_reported() {
    let error = check_source("fn main() -> i64 { totally_undeclared_name }").expect("expected error");
    assert!(error.contains("PACO-E0319"));
    assert!(error.contains("totally_undeclared_name"));
}

#[test]
fn unresolved_identifier_does_not_cascade_into_a_second_diagnostic() {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", "fn main() -> i64 { totally_undeclared_name + 1 }");
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let result = check_module(&module, &mut reporter);

    assert!(result.is_err());
    assert_eq!(reporter.diagnostics().len(), 1);
    assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0319");
}

#[test]
fn end_to_end_module_type_checks_with_no_diagnostics() {
    let source = r#"
        const TILE: i64 = 64;

        struct Tensor<T> {
            data: T,
            const RANK: i64 = 2;,
        }

        fn area() -> i64 {
            TILE * TILE
        }
        "#;
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let typed = infer_module(&module, &mut reporter);

    assert!(typed.is_ok(), "{}", reporter.emit_to_string(&sources));
    assert!(!reporter.has_errors());
}
