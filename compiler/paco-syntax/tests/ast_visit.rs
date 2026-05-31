use paco_span::Span;
use paco_syntax::ast::{
    Block, Expr, FnDecl, Item, Literal, MethodsBlock, Module, MutVisit, Param, Pat, Ty, Visit,
    walk_module, walk_module_mut,
};

#[test]
fn visit_walks_function_items_and_nested_expressions() {
    struct Collector {
        functions: Vec<String>,
        literals: Vec<String>,
    }

    impl Visit for Collector {
        fn visit_fn_decl(&mut self, function: &FnDecl) {
            self.functions.push(function.name.clone());
            paco_syntax::ast::walk_fn_decl(self, function);
        }

        fn visit_literal(&mut self, literal: &Literal) {
            if let Literal::String(value) = literal {
                self.literals.push(value.clone());
            }
        }
    }

    let span = Span::new_root(0, 0);
    let module = Module {
        items: vec![Item::Fn(FnDecl {
            name: "main".to_string(),
            generics: Vec::new(),
            params: vec![Param {
                pattern: Pat::Ident("message".to_string(), span),
                ty: Ty::Path(vec!["string".to_string()], span),
                span,
            }],
            return_ty: None,
            body: Block {
                stmts: Vec::new(),
                tail: Some(Box::new(Expr::Literal(
                    Literal::String("hello".to_string()),
                    span,
                ))),
                span,
            },
            span,
        })],
        span,
    };

    let mut collector = Collector {
        functions: Vec::new(),
        literals: Vec::new(),
    };

    walk_module(&mut collector, &module);

    assert_eq!(collector.functions, vec!["main"]);
    assert_eq!(collector.literals, vec!["hello"]);
}

#[test]
fn visit_walks_methods_block_functions() {
    struct Collector {
        functions: Vec<String>,
    }

    impl Visit for Collector {
        fn visit_fn_decl(&mut self, function: &FnDecl) {
            self.functions.push(function.name.clone());
            paco_syntax::ast::walk_fn_decl(self, function);
        }
    }

    let span = Span::new_root(0, 0);
    let module = Module {
        items: vec![Item::Methods(MethodsBlock {
            generics: Vec::new(),
            target: Ty::Path(vec!["Point".to_string()], span),
            methods: vec![FnDecl {
                name: "distance".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_ty: None,
                body: Block {
                    stmts: Vec::new(),
                    tail: None,
                    span,
                },
                span,
            }],
            span,
        })],
        span,
    };

    let mut collector = Collector {
        functions: Vec::new(),
    };

    walk_module(&mut collector, &module);

    assert_eq!(collector.functions, vec!["distance"]);
}

#[test]
fn mut_visit_walks_nested_tail_expressions() {
    struct Rewriter;

    impl MutVisit for Rewriter {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            match expr {
                Expr::Literal(Literal::String(value), _) => {
                    *value = "changed".to_string();
                }
                _ => paco_syntax::ast::walk_expr_mut(self, expr),
            }
        }
    }

    let span = Span::new_root(0, 0);
    let mut module = Module {
        items: vec![Item::Fn(FnDecl {
            name: "main".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_ty: None,
            body: Block {
                stmts: Vec::new(),
                tail: Some(Box::new(Expr::Literal(
                    Literal::String("original".to_string()),
                    span,
                ))),
                span,
            },
            span,
        })],
        span,
    };

    walk_module_mut(&mut Rewriter, &mut module);

    let Item::Fn(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(tail) = &function.body.tail else {
        panic!("expected tail expression");
    };
    let Expr::Literal(Literal::String(value), _) = tail.as_ref() else {
        panic!("expected string literal tail");
    };
    assert_eq!(value, "changed");
}
