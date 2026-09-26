use paco_diag::Reporter;
use paco_span::Span;
use paco_syntax::ast::{Block, Expr, FnDecl, Item, Module, Ty};
use paco_types::check_module;

#[test]
fn type_checker_reports_a_type_error_inside_a_comptime_expression() {
    // `phase-9-comptime` task 2.2: `comptime { .. }` type-checks its inner
    // block as an ordinary block (no longer `unsupported_expr`) — a type
    // error inside it is still reported, through the same ordinary
    // diagnostic path as outside a `comptime` block.
    let span = Span::new_root(0, 1);
    let module = Module {
        name: None,
        items: vec![Item::Fn(FnDecl {
            name: "main".to_string(),
            name_splice: None,
            generics: Vec::new(),
            params: Vec::new(),
            return_ty: None,
            body: Block {
                stmts: Vec::new(),
                tail: Some(Box::new(Expr::Comptime {
                    expr: Box::new(Expr::Ident("value".to_string(), span)),
                    span,
                })),
                span,
            },
            is_pub: false,
            is_unsafe: false,
            is_iter: false,
            extern_abi: None,
            is_comptime: false,
            attrs: Vec::new(),
            span,
        })],
        span,
    };
    let mut reporter = Reporter::new();

    let result = check_module(&module, &mut reporter);

    assert!(result.is_err());
    assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0319");
}

#[test]
fn type_checker_reports_unknown_path_types() {
    let span = Span::new_root(0, 1);
    let module = Module {
        name: None,
        items: vec![Item::Fn(FnDecl {
            name: "main".to_string(),
            name_splice: None,
            generics: Vec::new(),
            params: Vec::new(),
            return_ty: Some(Ty::Path(vec!["User".to_string()], span)),
            body: Block {
                stmts: Vec::new(),
                tail: None,
                span,
            },
            is_pub: false,
            is_unsafe: false,
            is_iter: false,
            extern_abi: None,
            is_comptime: false,
            attrs: Vec::new(),
            span,
        })],
        span,
    };
    let mut reporter = Reporter::new();

    let result = check_module(&module, &mut reporter);

    assert!(result.is_err());
    assert_eq!(reporter.diagnostics()[0].code(), "PACO-E0306");
}
