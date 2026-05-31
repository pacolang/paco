//! Minimal primitive type checking for the Phase 1 executable subset.

use std::collections::HashMap;

use paco_diag::{Diagnostic, Reporter};
use paco_span::Span;
use paco_syntax::ast::{
    BinaryOp, Block, Expr, FnDecl, Item, LetStmt, Literal, Module, Pat, Stmt, Ty, UnaryOp,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Never,
    Unknown,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeError;

pub fn check_module(module: &Module, reporter: &mut Reporter) -> Result<(), TypeError> {
    let mut functions = HashMap::new();
    for item in &module.items {
        if let Item::Fn(function) = item {
            functions.insert(
                function.name.clone(),
                FunctionSig::from_decl(function, reporter),
            );
        }
    }

    for item in &module.items {
        if let Item::Fn(function) = item {
            check_function(function, &functions, reporter);
        }
    }

    if reporter.has_errors() {
        Err(TypeError)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FunctionSig {
    params: Vec<Type>,
    return_ty: Type,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Binding {
    ty: Type,
    mutable: bool,
}

struct FunctionContext<'a> {
    scopes: Vec<HashMap<String, Binding>>,
    expected_return: &'a Type,
    loop_depth: usize,
}

impl FunctionSig {
    fn from_decl(function: &FnDecl, reporter: &mut Reporter) -> Self {
        Self {
            params: function
                .params
                .iter()
                .map(|param| ty_from_ast(&param.ty, reporter))
                .collect(),
            return_ty: function
                .return_ty
                .as_ref()
                .map_or(Type::Unit, |ty| ty_from_ast(ty, reporter)),
        }
    }
}

fn check_function(
    function: &FnDecl,
    functions: &HashMap<String, FunctionSig>,
    reporter: &mut Reporter,
) {
    let signature = functions
        .get(&function.name)
        .expect("function signature is collected before checking");
    let mut context = FunctionContext {
        scopes: vec![HashMap::new()],
        expected_return: &signature.return_ty,
        loop_depth: 0,
    };
    for (param, param_ty) in function.params.iter().zip(&signature.params) {
        if let Pat::Ident(name, _) = &param.pattern {
            context.scopes.last_mut().unwrap().insert(
                name.clone(),
                Binding {
                    ty: param_ty.clone(),
                    mutable: false,
                },
            );
        }
    }

    let actual = infer_block(&function.body, functions, &mut context, reporter);
    if !compatible(&actual, &signature.return_ty) && actual != Type::Never {
        reporter.push(Diagnostic::error(
            "PACO-E0302",
            function.span,
            format!(
                "return type mismatch: expected {}, found {}",
                signature.return_ty.name(),
                actual.name()
            ),
        ));
    }
}

fn infer_block(
    block: &Block,
    functions: &HashMap<String, FunctionSig>,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    context.scopes.push(HashMap::new());
    for statement in &block.stmts {
        match statement {
            Stmt::Let(statement) => {
                check_let(statement, functions, context, reporter);
            }
            Stmt::Expr(expr) => {
                if infer_expr(expr, functions, context, reporter) == Type::Never {
                    context.scopes.pop();
                    return Type::Never;
                }
            }
            Stmt::Item(_) => {}
        }
    }
    let result = block.tail.as_ref().map_or(Type::Unit, |expr| {
        infer_expr(expr, functions, context, reporter)
    });
    context.scopes.pop();
    result
}

fn check_let(
    statement: &LetStmt,
    functions: &HashMap<String, FunctionSig>,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    let value_ty = statement.value.as_ref().map_or(Type::Unknown, |value| {
        infer_expr(value, functions, context, reporter)
    });
    let declared_ty = statement.ty.as_ref().map(|ty| ty_from_ast(ty, reporter));
    let binding_ty = declared_ty.clone().unwrap_or_else(|| value_ty.clone());
    if let Some(declared_ty) = declared_ty
        && !compatible(&value_ty, &declared_ty)
    {
        reporter.push(Diagnostic::error(
            "PACO-E0302",
            statement.span,
            format!(
                "type mismatch: expected {}, found {}",
                declared_ty.name(),
                value_ty.name()
            ),
        ));
    }
    if let Pat::Ident(name, _) = &statement.pattern {
        context.scopes.last_mut().unwrap().insert(
            name.clone(),
            Binding {
                ty: binding_ty,
                mutable: statement.mutable,
            },
        );
    }
}

fn infer_expr(
    expr: &Expr,
    functions: &HashMap<String, FunctionSig>,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    match expr {
        Expr::Literal(Literal::Int(_), _) => Type::Int,
        Expr::Literal(Literal::Float(_), _) => Type::Float,
        Expr::Literal(Literal::Bool(_), _) => Type::Bool,
        Expr::Literal(Literal::String(_), _) => Type::String,
        Expr::Literal(Literal::Char(_), span) => unsupported_expr("char literals", *span, reporter),
        Expr::Ident(name, _) => lookup(&context.scopes, name)
            .map(|binding| binding.ty)
            .unwrap_or(Type::Unknown),
        Expr::Block(block) => infer_block(block, functions, context, reporter),
        Expr::If {
            condition,
            then_branch,
            else_branch,
            span,
        } => {
            let condition_ty = infer_expr(condition, functions, context, reporter);
            if !compatible(&condition_ty, &Type::Bool) {
                reporter.push(Diagnostic::error(
                    "PACO-E0303",
                    *span,
                    format!("if condition must be bool, found {}", condition_ty.name()),
                ));
            }
            let then_ty = infer_block(then_branch, functions, context, reporter);
            let else_ty = else_branch.as_ref().map_or(Type::Unit, |else_branch| {
                infer_expr(else_branch, functions, context, reporter)
            });
            join_branch_types(&then_ty, &else_ty).unwrap_or_else(|| {
                reporter.push(Diagnostic::error(
                    "PACO-E0304",
                    *span,
                    format!(
                        "if branches have incompatible types: {} and {}",
                        then_ty.name(),
                        else_ty.name()
                    ),
                ));
                Type::Error
            })
        }
        Expr::Loop { body, .. } => {
            context.loop_depth += 1;
            infer_block(body, functions, context, reporter);
            context.loop_depth -= 1;
            Type::Unit
        }
        Expr::While {
            condition,
            body,
            span,
            ..
        } => {
            let condition_ty = infer_expr(condition, functions, context, reporter);
            if !compatible(&condition_ty, &Type::Bool) {
                reporter.push(Diagnostic::error(
                    "PACO-E0303",
                    *span,
                    format!(
                        "while condition must be bool, found {}",
                        condition_ty.name()
                    ),
                ));
            }
            context.loop_depth += 1;
            infer_block(body, functions, context, reporter);
            context.loop_depth -= 1;
            Type::Unit
        }
        Expr::Call { callee, args, span } => {
            infer_call(callee, args, *span, functions, context, reporter)
        }
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => infer_binary(*op, left, right, *span, functions, context, reporter),
        Expr::Unary { op, expr, span } => {
            let ty = infer_expr(expr, functions, context, reporter);
            match op {
                UnaryOp::Not if compatible(&ty, &Type::Bool) => Type::Bool,
                UnaryOp::Neg if compatible(&ty, &Type::Int) => Type::Int,
                UnaryOp::Not => {
                    reporter.push(Diagnostic::error(
                        "PACO-E0301",
                        *span,
                        format!("type mismatch: expected bool, found {}", ty.name()),
                    ));
                    Type::Error
                }
                UnaryOp::Neg => {
                    reporter.push(Diagnostic::error(
                        "PACO-E0301",
                        *span,
                        format!("type mismatch: expected numeric, found {}", ty.name()),
                    ));
                    Type::Error
                }
            }
        }
        Expr::Assign {
            target,
            value,
            span,
        } => infer_assign(target, value, *span, functions, context, reporter),
        Expr::Return(value, span) => {
            let actual = value.as_ref().map_or(Type::Unit, |value| {
                infer_expr(value, functions, context, reporter)
            });
            if !compatible(&actual, context.expected_return) {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!(
                        "return type mismatch: expected {}, found {}",
                        context.expected_return.name(),
                        actual.name()
                    ),
                ));
            }
            Type::Never
        }
        Expr::Break(value, span) => {
            if context.loop_depth == 0 {
                reporter.push(Diagnostic::error(
                    "PACO-E0308",
                    *span,
                    "break cannot be used outside of a loop",
                ));
            }
            if let Some(value) = value {
                infer_expr(value, functions, context, reporter);
            }
            Type::Never
        }
        Expr::Continue(span) => {
            if context.loop_depth == 0 {
                reporter.push(Diagnostic::error(
                    "PACO-E0308",
                    *span,
                    "continue cannot be used outside of a loop",
                ));
            }
            Type::Never
        }
        Expr::Match { span, .. } => unsupported_expr("match expressions", *span, reporter),
        Expr::MethodCall { span, .. } => unsupported_expr("method calls", *span, reporter),
        Expr::AssociatedCall { span, .. } => {
            unsupported_expr("associated function calls", *span, reporter)
        }
        Expr::Field { span, .. } => unsupported_expr("field access", *span, reporter),
        Expr::Index { span, .. } => unsupported_expr("index expressions", *span, reporter),
        Expr::Spawn { span, .. } => unsupported_expr("spawn expressions", *span, reporter),
        Expr::Select { span, .. } => unsupported_expr("select expressions", *span, reporter),
        Expr::Comptime { span, .. } => unsupported_expr("comptime expressions", *span, reporter),
        Expr::Yield(_, span) => unsupported_expr("yield expressions", *span, reporter),
        Expr::StructLiteral { span, .. } => unsupported_expr("struct literals", *span, reporter),
        Expr::Borrow { span, .. } => unsupported_expr("borrow expressions", *span, reporter),
    }
}

fn infer_assign(
    target: &Expr,
    value: &Expr,
    span: Span,
    functions: &HashMap<String, FunctionSig>,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let value_ty = infer_expr(value, functions, context, reporter);
    let target_ty = match target {
        Expr::Ident(name, _) => {
            let binding = lookup(&context.scopes, name);
            if let Some(binding) = &binding
                && !binding.mutable
            {
                reporter.push(Diagnostic::error(
                    "PACO-E0307",
                    span,
                    format!("cannot assign to immutable binding `{name}`"),
                ));
            }
            binding.map(|binding| binding.ty).unwrap_or(Type::Unknown)
        }
        _ => unsupported_expr("assignment targets other than identifiers", span, reporter),
    };
    if !compatible(&value_ty, &target_ty) {
        reporter.push(Diagnostic::error(
            "PACO-E0302",
            span,
            format!(
                "type mismatch: expected {}, found {}",
                target_ty.name(),
                value_ty.name()
            ),
        ));
    }
    Type::Unit
}

fn infer_call(
    callee: &Expr,
    args: &[Expr],
    span: Span,
    functions: &HashMap<String, FunctionSig>,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let Expr::Ident(name, _) = callee else {
        return unsupported_expr("indirect calls", span, reporter);
    };

    if name == "print" {
        if args.len() != 1 {
            reporter.push(Diagnostic::error(
                "PACO-E0305",
                span,
                format!("print expects 1 argument, found {}", args.len()),
            ));
        }
        for arg in args {
            infer_expr(arg, functions, context, reporter);
        }
        return Type::Unit;
    }

    let Some(signature) = functions.get(name).cloned() else {
        return Type::Unknown;
    };

    if signature.params.len() != args.len() {
        reporter.push(Diagnostic::error(
            "PACO-E0305",
            span,
            format!(
                "function `{name}` expects {} arguments, found {}",
                signature.params.len(),
                args.len()
            ),
        ));
        return Type::Error;
    }

    for (arg, expected) in args.iter().zip(&signature.params) {
        let actual = infer_expr(arg, functions, context, reporter);
        if !compatible(&actual, expected) {
            reporter.push(Diagnostic::error(
                "PACO-E0302",
                span,
                format!(
                    "type mismatch: expected {}, found {}",
                    expected.name(),
                    actual.name()
                ),
            ));
        }
    }
    signature.return_ty
}

fn infer_binary(
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    span: Span,
    functions: &HashMap<String, FunctionSig>,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let left_ty = infer_expr(left, functions, context, reporter);
    let right_ty = infer_expr(right, functions, context, reporter);
    match op {
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
            if compatible(&left_ty, &Type::Int) && compatible(&right_ty, &Type::Int) {
                Type::Int
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected int and int, found {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
        BinaryOp::Eq | BinaryOp::Ne => {
            if compatible(&left_ty, &right_ty) {
                Type::Bool
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: cannot compare {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            if compatible(&left_ty, &Type::Int) && compatible(&right_ty, &Type::Int) {
                Type::Bool
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected int and int, found {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
        BinaryOp::And | BinaryOp::Or => {
            if compatible(&left_ty, &Type::Bool) && compatible(&right_ty, &Type::Bool) {
                Type::Bool
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected bool and bool, found {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
    }
}

fn ty_from_ast(ty: &Ty, reporter: &mut Reporter) -> Type {
    match ty {
        Ty::Path(path, _) if path.as_slice() == ["int"] => Type::Int,
        Ty::Path(path, _) if path.as_slice() == ["float"] => Type::Float,
        Ty::Path(path, _) if path.as_slice() == ["bool"] => Type::Bool,
        Ty::Path(path, _) if path.as_slice() == ["string"] => Type::String,
        _ => {
            reporter.push(Diagnostic::error(
                "PACO-E0306",
                ty_span(ty),
                "type is not supported in Phase 1",
            ));
            Type::Error
        }
    }
}

fn ty_span(ty: &Ty) -> Span {
    match ty {
        Ty::Path(_, span)
        | Ty::Tuple(_, span)
        | Ty::Slice(_, span)
        | Ty::Dyn { span, .. }
        | Ty::Fn { span, .. }
        | Ty::Infer(span)
        | Ty::Never(span)
        | Ty::Borrow { span, .. } => *span,
    }
}

fn unsupported_expr(feature: &str, span: Span, reporter: &mut Reporter) -> Type {
    reporter.push(Diagnostic::error(
        "PACO-E0306",
        span,
        format!("expression is not supported in Phase 1: {feature}"),
    ));
    Type::Error
}

fn lookup(scopes: &[HashMap<String, Binding>], name: &str) -> Option<Binding> {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
}

fn compatible(actual: &Type, expected: &Type) -> bool {
    actual == expected
        || matches!(actual, Type::Unknown | Type::Error)
        || matches!(expected, Type::Unknown | Type::Error)
}

fn join_branch_types(left: &Type, right: &Type) -> Option<Type> {
    if compatible(left, right) {
        Some(left.clone())
    } else if *left == Type::Never {
        Some(right.clone())
    } else if *right == Type::Never {
        Some(left.clone())
    } else {
        None
    }
}

impl Type {
    fn name(&self) -> &'static str {
        match self {
            Type::Int => "int",
            Type::Float => "float",
            Type::Bool => "bool",
            Type::String => "string",
            Type::Unit => "unit",
            Type::Never => "never",
            Type::Unknown => "unknown",
            Type::Error => "error",
        }
    }
}
