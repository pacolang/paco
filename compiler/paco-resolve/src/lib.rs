//! Minimal name resolution for the Phase 1 executable subset.

use std::collections::HashSet;

use paco_diag::{Diagnostic, Reporter};
use paco_syntax::ast::{Block, Expr, FnDecl, Item, LetStmt, Module, Pat, Stmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolveError;

pub fn resolve_module(module: &Module, reporter: &mut Reporter) -> Result<(), ResolveError> {
    let functions = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(function) => Some(function.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    for item in &module.items {
        match item {
            Item::Fn(function) => resolve_function(function, &functions, reporter),
            Item::Struct(decl) => {
                for method in &decl.methods {
                    resolve_function(method, &functions, reporter);
                }
            }
            Item::Enum(decl) => {
                for method in &decl.methods {
                    resolve_function(method, &functions, reporter);
                }
            }
            Item::Methods(block) => {
                for method in &block.methods {
                    resolve_function(method, &functions, reporter);
                }
            }
            Item::Trait(_) | Item::Use(_) => {}
        }
    }

    if reporter.has_errors() {
        Err(ResolveError)
    } else {
        Ok(())
    }
}

fn resolve_function(function: &FnDecl, functions: &HashSet<String>, reporter: &mut Reporter) {
    let mut scopes = vec![HashSet::new()];
    for param in &function.params {
        bind_pattern(&mut scopes, &param.pattern);
    }
    resolve_block(&function.body, functions, &mut scopes, reporter);
}

fn resolve_block(
    block: &Block,
    functions: &HashSet<String>,
    scopes: &mut Vec<HashSet<String>>,
    reporter: &mut Reporter,
) {
    scopes.push(HashSet::new());
    for statement in &block.stmts {
        match statement {
            Stmt::Let(statement) => resolve_let(statement, functions, scopes, reporter),
            Stmt::Expr(expr) => resolve_expr(expr, functions, scopes, reporter),
            Stmt::Item(_) => {}
        }
    }
    if let Some(tail) = &block.tail {
        resolve_expr(tail, functions, scopes, reporter);
    }
    scopes.pop();
}

fn resolve_let(
    statement: &LetStmt,
    functions: &HashSet<String>,
    scopes: &mut Vec<HashSet<String>>,
    reporter: &mut Reporter,
) {
    if let Some(value) = &statement.value {
        resolve_expr(value, functions, scopes, reporter);
    }
    bind_pattern(scopes, &statement.pattern);
}

fn resolve_expr(
    expr: &Expr,
    functions: &HashSet<String>,
    scopes: &mut Vec<HashSet<String>>,
    reporter: &mut Reporter,
) {
    match expr {
        Expr::Literal(_, _) | Expr::Continue(_) => {}
        Expr::Ident(name, span) => {
            if !is_bound(scopes, name) && !functions.contains(name) && name != "print" {
                reporter.push(Diagnostic::error(
                    "PACO-E0201",
                    *span,
                    format!("name not found `{name}`"),
                ));
            }
        }
        Expr::Block(block) => resolve_block(block, functions, scopes, reporter),
        Expr::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            resolve_expr(condition, functions, scopes, reporter);
            resolve_block(then_branch, functions, scopes, reporter);
            if let Some(else_branch) = else_branch {
                resolve_expr(else_branch, functions, scopes, reporter);
            }
        }
        Expr::Loop { body, .. } => resolve_block(body, functions, scopes, reporter),
        Expr::While {
            condition, body, ..
        } => {
            resolve_expr(condition, functions, scopes, reporter);
            resolve_block(body, functions, scopes, reporter);
        }
        Expr::Call { callee, args, .. } => {
            resolve_expr(callee, functions, scopes, reporter);
            for arg in args {
                resolve_expr(arg, functions, scopes, reporter);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            resolve_expr(receiver, functions, scopes, reporter);
            for arg in args {
                resolve_expr(arg, functions, scopes, reporter);
            }
        }
        Expr::AssociatedCall { args, .. } => {
            for arg in args {
                resolve_expr(arg, functions, scopes, reporter);
            }
        }
        Expr::Binary { left, right, .. } => {
            resolve_expr(left, functions, scopes, reporter);
            resolve_expr(right, functions, scopes, reporter);
        }
        Expr::Unary { expr, .. } => resolve_expr(expr, functions, scopes, reporter),
        Expr::Assign { target, value, .. } => {
            resolve_expr(target, functions, scopes, reporter);
            resolve_expr(value, functions, scopes, reporter);
        }
        Expr::Field { base, .. } => resolve_expr(base, functions, scopes, reporter),
        Expr::Index { base, index, .. } => {
            resolve_expr(base, functions, scopes, reporter);
            resolve_expr(index, functions, scopes, reporter);
        }
        Expr::Return(value, _) | Expr::Break(value, _) => {
            if let Some(value) = value {
                resolve_expr(value, functions, scopes, reporter);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            resolve_expr(scrutinee, functions, scopes, reporter);
            for arm in arms {
                scopes.push(HashSet::new());
                bind_pattern(scopes, &arm.pattern);
                if let Some(guard) = &arm.guard {
                    resolve_expr(guard, functions, scopes, reporter);
                }
                resolve_expr(&arm.body, functions, scopes, reporter);
                scopes.pop();
            }
        }
        Expr::Spawn { expr, .. } | Expr::Comptime { expr, .. } | Expr::Borrow { expr, .. } => {
            resolve_expr(expr, functions, scopes, reporter);
        }
        Expr::Select { arms, default, .. } => {
            for arm in arms {
                resolve_expr(&arm.operation, functions, scopes, reporter);
                resolve_block(&arm.body, functions, scopes, reporter);
            }
            if let Some(default) = default {
                resolve_block(default, functions, scopes, reporter);
            }
        }
        Expr::Yield(expr, _) => resolve_expr(expr, functions, scopes, reporter),
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                resolve_expr(value, functions, scopes, reporter);
            }
        }
    }
}

fn bind_pattern(scopes: &mut [HashSet<String>], pattern: &Pat) {
    if let Pat::Ident(name, _) = pattern
        && let Some(scope) = scopes.last_mut()
    {
        scope.insert(name.clone());
    }
}

fn is_bound(scopes: &[HashSet<String>], name: &str) -> bool {
    scopes.iter().rev().any(|scope| scope.contains(name))
}
