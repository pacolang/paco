use paco_diag::Reporter;
use paco_span::Span;
use paco_syntax::ast::{Block, Expr, FnDecl, Item, Module, Ty};
use paco_types::check_module;

#[test]
fn type_checker_reports_unsupported_phase_one_expressions() {
    let span = Span::new_root(0, 1);
    let module = Module {
        items: vec![Item::Fn(FnDecl {
            name: "main".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_ty: None,
            body: Block {
                stmts: Vec::new(),
                tail: Some(Box::new(Expr::Borrow {
                    mutable: false,
                    expr: Box::new(Expr::Ident("value".to_string(), span)),
                    span,
                })),
                span,
            },
            span,
        })],
        span,
    };
    let mut reporter = Reporter::new();

    let result = check_module(&module, &mut reporter);

    assert!(result.is_err());
    assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0306");
}

#[test]
fn type_checker_reports_unknown_phase_one_types() {
    let span = Span::new_root(0, 1);
    let module = Module {
        items: vec![Item::Fn(FnDecl {
            name: "main".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_ty: Some(Ty::Path(vec!["User".to_string()], span)),
            body: Block {
                stmts: Vec::new(),
                tail: None,
                span,
            },
            span,
        })],
        span,
    };
    let mut reporter = Reporter::new();

    let result = check_module(&module, &mut reporter);

    assert!(result.is_err());
    assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0306");
}
