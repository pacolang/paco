use paco_diag::Reporter;
use paco_mir::{Body, CallTarget, Profile, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

const RESULT_DECL: &str = "enum Result<T, E> { Ok(T), Err(E) }";

fn lower_source_fn(source: &str, fn_name: &str) -> Body {
    let source = format!("{RESULT_DECL}\n{source}");
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", &source);
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
            Item::Fn(function) if function.name == fn_name => Some(function),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a function item named `{fn_name}`"));
    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug).0
}

fn call_targets(body: &Body) -> Vec<&CallTarget> {
    body.blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Call { target, .. } => Some(target),
            _ => None,
        })
        .collect()
}

#[test]
fn cross_type_question_mark_propagation_lowers_to_an_explicit_from_call() {
    let body = lower_source_fn(
        "enum ParseError { Bad(i64) }
        enum AppError {
            Wrapped(i64),
            fn from(e: ParseError) -> Self { AppError::Wrapped(1) }
        }
        fn parse() -> Result<i64, ParseError> { Result::Err(ParseError::Bad(1)) }
        fn f() -> Result<i64, AppError> {
            let n = parse()?;
            Result::Ok(n)
        }",
        "f",
    );

    assert!(
        call_targets(&body).contains(&&CallTarget("AppError::from".to_string())),
        "expected an explicit `AppError::from` call in {body:#?}"
    );
}

#[test]
fn matching_error_type_question_mark_propagation_has_no_conversion_call() {
    let body = lower_source_fn(
        "enum AppError { Wrapped(i64) }
        fn parse() -> Result<i64, AppError> { Result::Err(AppError::Wrapped(1)) }
        fn f() -> Result<i64, AppError> {
            let n = parse()?;
            Result::Ok(n)
        }",
        "f",
    );

    assert!(
        call_targets(&body)
            .iter()
            .all(|target| target.0 != "AppError::from"),
        "expected no conversion call when error types already match: {body:#?}"
    );
}
