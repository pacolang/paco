//! Values computed at compile time, and where `quote { .. }` templates take
//! their splices.

use paco_span::Span;
use paco_syntax::ast::{Block, Expr, FnDecl, Item, QuoteBody, Stmt, Ty};
use paco_syntax::parse::expr_span;
use paco_types::Type;

use crate::Constant;

/// A value a `comptime` evaluation produced, in a form lowering can embed
/// in the program.
#[derive(Clone, Debug, PartialEq)]
pub enum ComptimeValue {
    Scalar(Constant),
    /// A struct or tuple of `ty`, fields in declaration order.
    Record(Type, Vec<ComptimeValue>),
    Variant(Type, String, Vec<ComptimeValue>),
    /// A `[]T` of `ty`.
    Slice(Type, Vec<ComptimeValue>),
    Type(Type),
    Code(Box<QuoteBody>),
}

/// Identifies one `comptime { .. }` expression in one instantiation of the
/// function containing it.
pub type ComptimeKey = (usize, String);

/// Every splice point of a template that belongs to it rather than to a
/// nested `quote`, as `(key span, expression computing the spliced value)`.
/// Key spans: the `#(..)` for an expression or type splice, the call for
/// `base.#(name)`, and the name expression for `fn #(name)`.
pub fn splice_sites(body: &QuoteBody) -> Vec<(Span, &Expr)> {
    struct Sites<'a> {
        found: Vec<(Span, &'a Expr)>,
    }
    impl<'a> Sites<'a> {
        fn ty(&mut self, ty: &'a Ty) {
            match ty {
                Ty::Splice(inner, span) => self.found.push((*span, inner)),
                Ty::Generic { args, .. } | Ty::Tuple(args, _) => args.iter().for_each(|arg| self.ty(arg)),
                Ty::Slice(inner, _) | Ty::Borrow { ty: inner, .. } | Ty::RawPointer { ty: inner, .. } => self.ty(inner),
                Ty::Fn { params, return_ty, .. } => {
                    params.iter().for_each(|param| self.ty(param));
                    if let Some(return_ty) = return_ty {
                        self.ty(return_ty);
                    }
                }
                Ty::Path(..) | Ty::Dyn { .. } | Ty::Infer(_) | Ty::Never(_) | Ty::Const(..) | Ty::DynDim(_) | Ty::Expand(..) | Ty::Existential(..) => {}
            }
        }

        fn item(&mut self, item: &'a Item) {
            if let Item::Methods(block) = item {
                self.ty(&block.target);
            }
            self.walk_item(item);
        }

        fn walk_item(&mut self, item: &'a Item) {
            match item {
                Item::Fn(function) => self.function(function),
                Item::Methods(block) => block.methods.iter().for_each(|method| self.function(method)),
                Item::Struct(decl) => decl.methods.iter().for_each(|method| self.function(method)),
                Item::Enum(decl) => decl.methods.iter().for_each(|method| self.function(method)),
                _ => {}
            }
        }

        fn function(&mut self, function: &'a FnDecl) {
            if let Some(name) = &function.name_splice {
                self.found.push((expr_span(name), name));
            }
            for param in &function.params {
                self.ty(&param.ty);
            }
            if let Some(return_ty) = &function.return_ty {
                self.ty(return_ty);
            }
            self.block_stmts(&function.body.stmts);
            if let Some(tail) = &function.body.tail {
                self.expr(tail);
            }
        }

        fn block_stmts(&mut self, stmts: &'a [Stmt]) {
            for statement in stmts {
                match statement {
                    Stmt::Let(let_stmt) => {
                        if let Some(ty) = &let_stmt.ty {
                            self.ty(ty);
                        }
                        if let Some(value) = &let_stmt.value {
                            self.expr(value);
                        }
                    }
                    Stmt::Expr(expr) => self.expr(expr),
                    Stmt::Item(item) => self.item(item),
                }
            }
        }

        fn expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Quote(..) => return,
                Expr::Splice(inner, span) => {
                    self.found.push((*span, inner));
                    return;
                }
                Expr::Call { callee, args, span, type_args }
                    if matches!(callee.as_ref(), Expr::Ident(name, _) if name == "splice_field") && args.len() == 2 =>
                {
                    type_args.iter().for_each(|ty| self.ty(ty));
                    self.expr(&args[0]);
                    self.found.push((*span, &args[1]));
                    return;
                }
                Expr::AssociatedCall { ty, .. } | Expr::StructLiteral { ty, .. } | Expr::Cast { ty, .. } => self.ty(ty),
                Expr::Call { type_args, .. } => type_args.iter().for_each(|ty| self.ty(ty)),
                Expr::Closure { params, .. } => params.iter().filter_map(|param| param.ty.as_ref()).for_each(|ty| self.ty(ty)),
                _ => {}
            }
            let (exprs, blocks) = children(expr);
            for child in exprs {
                self.expr(child);
            }
            for block in blocks {
                self.block_stmts(&block.stmts);
                if let Some(tail) = &block.tail {
                    self.expr(tail);
                }
            }
        }
    }

    let mut sites = Sites { found: Vec::new() };
    match body {
        QuoteBody::Item(item) => sites.item(item),
        QuoteBody::Expr(expr) => sites.expr(expr),
    }
    sites.found
}

/// `expr`'s direct sub-expressions and blocks, in evaluation order.
fn children(expr: &Expr) -> (Vec<&Expr>, Vec<&Block>) {
    let mut exprs: Vec<&Expr> = Vec::new();
    let mut blocks: Vec<&Block> = Vec::new();
    match expr {
        Expr::Literal(..) | Expr::Ident(..) | Expr::Continue(_) | Expr::Quote(..) => {}
        Expr::Block(block) | Expr::Unsafe(block, _) => blocks.push(block),
        Expr::If { condition, then_branch, else_branch, .. } => {
            exprs.push(condition);
            blocks.push(then_branch);
            exprs.extend(else_branch.as_deref());
        }
        Expr::Loop { body, .. } => blocks.push(body),
        Expr::While { condition, body, .. } => {
            exprs.push(condition);
            blocks.push(body);
        }
        Expr::Match { scrutinee, arms, .. } => {
            exprs.push(scrutinee);
            for arm in arms {
                exprs.extend(arm.guard.as_ref());
                exprs.push(&arm.body);
            }
        }
        Expr::Call { callee, args, .. } => {
            exprs.push(callee);
            exprs.extend(args);
        }
        Expr::MethodCall { receiver, args, .. } => {
            exprs.push(receiver);
            exprs.extend(args);
        }
        Expr::AssociatedCall { args, .. } => exprs.extend(args),
        Expr::Binary { left, right, .. } => exprs.extend([left.as_ref(), right.as_ref()]),
        Expr::Assign { target, value, .. } => exprs.extend([target.as_ref(), value.as_ref()]),
        Expr::Index { base, index, .. } => {
            exprs.push(base);
            exprs.extend(index);
        }
        Expr::Return(value, _) | Expr::Break(value, _) => exprs.extend(value.as_deref()),
        Expr::Unary { expr, .. }
        | Expr::Field { base: expr, .. }
        | Expr::Spawn { expr, .. }
        | Expr::Comptime { expr, .. }
        | Expr::Splice(expr, _)
        | Expr::Closure { body: expr, .. }
        | Expr::Yield(expr, _)
        | Expr::Borrow { expr, .. }
        | Expr::Try { expr, .. }
        | Expr::Cast { expr, .. } => exprs.push(expr),
        Expr::Select { arms, default, .. } => {
            for arm in arms {
                exprs.push(&arm.operation);
                blocks.push(&arm.body);
            }
            blocks.extend(default.as_ref());
        }
        Expr::StructLiteral { fields, .. } => exprs.extend(fields.iter().map(|(_, value)| value)),
        Expr::Tuple(items, _) => exprs.extend(items),
    }
    (exprs, blocks)
}
