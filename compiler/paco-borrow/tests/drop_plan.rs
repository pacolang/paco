use paco_borrow::analyze_module;
use paco_diag::Reporter;
use paco_span::SourceMap;
use paco_resolve::LocalId;
use paco_syntax::ast::{Expr, Item, Stmt, Visit, walk_stmt};
use paco_syntax::{lex::lex, parse::parse_module};

fn parse_source(source: &str) -> paco_syntax::ast::Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors());
    module
}

/// The names of the `let`-bound locals `ids` refer to, in order.
fn names(module: &paco_syntax::ast::Module, ids: &[LocalId]) -> Vec<String> {
    struct Lets<'a>(&'a paco_resolve::Locals, Vec<(LocalId, String)>);
    impl Visit for Lets<'_> {
        fn visit_stmt(&mut self, statement: &Stmt) {
            if let Stmt::Let(statement) = statement
                && let paco_syntax::ast::Pat::Ident(name, _) = &statement.pattern
                && let Some(id) = self.0.pat(&statement.pattern)
            {
                self.1.push((id, name.clone()));
            }
            walk_stmt(self, statement);
        }
    }
    let locals = paco_resolve::resolve_locals([module]);
    let mut lets = Lets(&locals, Vec::new());
    lets.visit_module(module);
    ids.iter().map(|id| lets.1.iter().find(|(bound, _)| bound == id).expect("a let-bound local").1.clone()).collect()
}

fn main_fn(module: &paco_syntax::ast::Module) -> &paco_syntax::ast::FnDecl {
    module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .expect("expected a `main` function")
}

#[test]
fn destructor_call_on_normal_scope_exit() {
    let module = parse_source(r#"fn main() { let a = "hi"; }"#);
    let mut reporter = Reporter::new();

    let plan = analyze_module(&module, &mut reporter).expect("module should borrow-check");

    let function = main_fn(&module);
    assert_eq!(names(&module, plan.drops_at_block_end(&function.body)), ["a".to_string()]);
}

#[test]
fn no_destructor_call_after_a_move() {
    let module = parse_source(
        r#"
        fn consume(value: string) { }
        fn main() {
            let a = "hi";
            consume(a);
        }
        "#,
    );
    let mut reporter = Reporter::new();

    let plan = analyze_module(&module, &mut reporter).expect("module should borrow-check");

    let function = main_fn(&module);
    assert!(plan.drops_at_block_end(&function.body).is_empty());
}

#[test]
fn reverse_declaration_order_on_multi_binding_scope_exit() {
    let module = parse_source(r#"fn main() { let a = "one"; let b = "two"; }"#);
    let mut reporter = Reporter::new();

    let plan = analyze_module(&module, &mut reporter).expect("module should borrow-check");

    let function = main_fn(&module);
    assert_eq!(names(&module, plan.drops_at_block_end(&function.body)),
        ["b".to_string(), "a".to_string()]
    );
}

#[test]
fn destructor_call_on_early_return() {
    let module = parse_source(
        r#"
        fn main() {
            let a = "hi";
            return;
        }
        "#,
    );
    let mut reporter = Reporter::new();

    let plan = analyze_module(&module, &mut reporter).expect("module should borrow-check");

    let function = main_fn(&module);
    // The block never falls off the end normally, so no drop is recorded there.
    assert!(plan.drops_at_block_end(&function.body).is_empty());

    let return_expr = function
        .body
        .stmts
        .iter()
        .find_map(|statement| match statement {
            Stmt::Expr(expr @ Expr::Return(_, _)) => Some(expr),
            _ => None,
        })
        .expect("expected a return statement");
    assert_eq!(names(&module, plan.drops_at_control_transfer(return_expr)),
        ["a".to_string()]
    );
}

#[test]
fn destructor_call_on_break_and_continue() {
    let module = parse_source(
        r#"
        fn main() {
            loop {
                let a = "hi";
                break;
            }
        }
        "#,
    );
    let mut reporter = Reporter::new();

    let plan = analyze_module(&module, &mut reporter).expect("module should borrow-check");

    let function = main_fn(&module);
    let loop_body = match function.body.tail.as_deref() {
        Some(Expr::Loop { body, .. }) => body,
        _ => panic!("expected a loop tail expression"),
    };
    let break_expr = loop_body
        .stmts
        .iter()
        .find_map(|statement| match statement {
            Stmt::Expr(expr @ Expr::Break(_, _)) => Some(expr),
            _ => None,
        })
        .expect("expected a break statement");

    assert_eq!(names(&module, plan.drops_at_control_transfer(break_expr)), ["a".to_string()]);
    // The loop body's own block never falls off the end normally either.
    assert!(plan.drops_at_block_end(loop_body).is_empty());
}

#[test]
fn destructor_call_on_question_mark_error_propagation() {
    let module = parse_source(
        r#"
        enum Result<T, E> { Ok(T), Err(E) }
        fn might_fail() -> Result<i64, i64> { Result::Err(1) }
        fn main() -> Result<i64, i64> {
            let a = "hi";
            let n = might_fail()?;
            Result::Ok(n)
        }
        "#,
    );
    let mut reporter = Reporter::new();

    let plan = analyze_module(&module, &mut reporter).expect("module should borrow-check");

    let function = main_fn(&module);
    let try_expr = function
        .body
        .stmts
        .iter()
        .find_map(|statement| match statement {
            Stmt::Let(let_stmt) => match &let_stmt.value {
                Some(expr @ Expr::Try { .. }) => Some(expr),
                _ => None,
            },
            _ => None,
        })
        .expect("expected a let statement with a `?` initializer");

    assert_eq!(names(&module, plan.drops_at_control_transfer(try_expr)), ["a".to_string()]);
}

#[test]
fn shadowed_bindings_in_one_scope_are_each_dropped() {
    let module = parse_source(r#"fn main() { let a = "one"; let a = "two"; }"#);
    let mut reporter = Reporter::new();

    let plan = analyze_module(&module, &mut reporter).expect("module should borrow-check");

    let drops = plan.drops_at_block_end(&main_fn(&module).body);
    assert_eq!(drops.len(), 2);
    assert_ne!(drops[0], drops[1]);
    assert_eq!(names(&module, drops), ["a".to_string(), "a".to_string()]);
}
