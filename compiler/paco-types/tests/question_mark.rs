use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{Type, check_module, infer_module};

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

const RESULT_DECL: &str = "enum Result<T, E> { Ok(T), Err(E) }";

#[test]
fn non_result_operand_is_rejected() {
    let source = format!("{RESULT_DECL} fn f() -> i64 {{ let n: i64 = 5; n? }}");
    let error = check_source(&source).expect("expected an error");
    assert!(error.contains("PACO-E0322"));
}

#[test]
fn using_question_mark_in_a_non_result_returning_function_is_rejected() {
    let source = format!(
        "{RESULT_DECL} fn g() -> Result<i64, i64> {{ Result::Ok(1) }} fn f() -> i64 {{ g()? }}"
    );
    let error = check_source(&source).expect("expected an error");
    assert!(error.contains("PACO-E0323"));
}

#[test]
fn matching_error_types_need_no_conversion() {
    let source = format!(
        "{RESULT_DECL}
        struct Config {{ v: i64 }}
        fn read_bytes() -> Result<Config, i64> {{ Result::Ok(Config {{ v: 1 }}) }}
        fn f() -> Result<Config, i64> {{
            let c = read_bytes()?;
            Result::Ok(c)
        }}"
    );
    let error = check_source(&source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn matching_error_types_yields_the_ok_payload_type() {
    let source = format!(
        "{RESULT_DECL}
        struct Config {{ v: i64 }}
        fn read_bytes() -> Result<Config, i64> {{ Result::Ok(Config {{ v: 1 }}) }}
        fn f() -> Result<Config, i64> {{
            let c: Config = read_bytes()?;
            Result::Ok(c)
        }}"
    );
    let error = check_source(&source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn differing_error_types_with_a_matching_from_type_check() {
    let source = format!(
        "{RESULT_DECL}
        struct Config {{ v: i64 }}
        struct ParseError {{ msg: string }}
        struct AppError {{
            code: i64,

            fn from(e: ParseError) -> Self {{ AppError {{ code: 1 }} }}
        }}
        fn parse() -> Result<Config, ParseError> {{
            Result::Ok(Config {{ v: 1 }})
        }}
        fn f() -> Result<Config, AppError> {{
            let c = parse()?;
            Result::Ok(c)
        }}"
    );
    let error = check_source(&source);
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn differing_error_types_with_no_matching_from_is_rejected() {
    let source = format!(
        "{RESULT_DECL}
        struct Config {{ v: i64 }}
        struct ParseError {{ msg: string }}
        struct AppError {{ code: i64 }}
        fn parse() -> Result<Config, ParseError> {{
            Result::Ok(Config {{ v: 1 }})
        }}
        fn f() -> Result<Config, AppError> {{
            let c = parse()?;
            Result::Ok(c)
        }}"
    );
    let error = check_source(&source).expect("expected an error");
    assert!(error.contains("PACO-E0324"));
}

#[test]
fn a_try_expression_types_to_the_ok_payload() {
    let mut sources = SourceMap::new();
    let source = format!(
        "{RESULT_DECL}
        struct Config {{ v: i64 }}
        fn read_bytes() -> Result<Config, i64> {{ Result::Ok(Config {{ v: 1 }}) }}
        fn f() -> Result<Config, i64> {{
            let c = read_bytes()?;
            Result::Ok(c)
        }}"
    );
    let file = sources.add_file("main.paco", &source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).expect("source should parse");

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");

    let paco_syntax::ast::Item::Fn(f) = module
        .items
        .iter()
        .find(|item| matches!(item, paco_syntax::ast::Item::Fn(f) if f.name == "f"))
        .unwrap()
    else {
        unreachable!()
    };
    let paco_syntax::ast::Stmt::Let(let_stmt) = &f.body.stmts[0] else {
        panic!("expected a let statement");
    };
    let value = let_stmt.value.as_ref().unwrap();
    assert!(matches!(value, paco_syntax::ast::Expr::Try { .. }));
    assert_eq!(
        typed.type_of(value),
        Some(&Type::Struct("Config".to_string(), Vec::new()))
    );
}
