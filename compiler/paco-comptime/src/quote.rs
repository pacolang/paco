use std::collections::{HashMap, HashSet};

use paco_span::Span;
use paco_syntax::ast::{self, Block, Expr, FnDecl, Item, Literal, MutVisit, QuoteBody, Stmt, Ty};
use paco_syntax::parse::expr_span;
use paco_types::{FloatWidth, IntWidth, Type};

/// A value a `quote` splice evaluated to.
#[derive(Clone, Debug)]
pub(crate) enum Splice {
    Type(Type),
    Code(Box<QuoteBody>),
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
}

/// `template` with each splice point replaced by `values[its key span]`
/// (see `paco_mir::splice_sites` for the keys).
pub(crate) fn substitute(template: &QuoteBody, values: &HashMap<Span, Splice>) -> Result<QuoteBody, String> {
    struct Splicer<'v> {
        values: &'v HashMap<Span, Splice>,
        error: Option<String>,
    }
    impl Splicer<'_> {
        fn value(&mut self, span: Span) -> Option<Splice> {
            let value = self.values.get(&span).cloned();
            if value.is_none() && self.error.is_none() {
                self.error = Some("a `quote` splice was not evaluated".to_string());
            }
            value
        }

        fn ty(&mut self, ty: &mut Ty) {
            match ty {
                Ty::Splice(_, span) => {
                    let span = *span;
                    match self.value(span) {
                        Some(Splice::Type(value)) => *ty = type_to_ty(&value, span),
                        Some(_) => self.error = Some("a type-position splice must evaluate to a type value".to_string()),
                        None => {}
                    }
                }
                Ty::Generic { args, .. } | Ty::Tuple(args, _) => args.iter_mut().for_each(|arg| self.ty(arg)),
                Ty::Slice(inner, _) | Ty::Borrow { ty: inner, .. } | Ty::RawPointer { ty: inner, .. } => self.ty(inner),
                Ty::Fn { params, return_ty, .. } => {
                    params.iter_mut().for_each(|param| self.ty(param));
                    if let Some(return_ty) = return_ty {
                        self.ty(return_ty);
                    }
                }
                Ty::Path(..) | Ty::Dyn { .. } | Ty::Infer(_) | Ty::Never(_) | Ty::Const(..) | Ty::DynDim(_) | Ty::Expand(..) | Ty::Existential(..) => {}
            }
        }

        fn function(&mut self, function: &mut FnDecl) {
            if let Some(name) = function.name_splice.take() {
                match self.value(expr_span(&name)) {
                    Some(Splice::String(name)) => function.name = name,
                    Some(_) => self.error = Some("a function-name splice must evaluate to a string".to_string()),
                    None => {}
                }
            }
            function.params.iter_mut().for_each(|param| self.ty(&mut param.ty));
            if let Some(return_ty) = &mut function.return_ty {
                self.ty(return_ty);
            }
            self.block(&mut function.body);
        }

        fn block(&mut self, block: &mut Block) {
            for statement in &mut block.stmts {
                match statement {
                    Stmt::Let(let_stmt) => {
                        if let Some(ty) = &mut let_stmt.ty {
                            self.ty(ty);
                        }
                        if let Some(value) = &mut let_stmt.value {
                            self.visit_expr_mut(value);
                        }
                    }
                    Stmt::Expr(expr) => self.visit_expr_mut(expr),
                    Stmt::Item(item) => self.visit_item_mut(item),
                }
            }
            if let Some(tail) = &mut block.tail {
                self.visit_expr_mut(tail);
            }
        }
    }
    impl MutVisit for Splicer<'_> {
        fn visit_item_mut(&mut self, item: &mut Item) {
            match item {
                Item::Methods(block) => {
                    self.ty(&mut block.target);
                    block.methods.iter_mut().for_each(|method| self.function(method));
                }
                Item::Fn(function) => self.function(function),
                Item::Struct(decl) => decl.methods.iter_mut().for_each(|method| self.function(method)),
                Item::Enum(decl) => decl.methods.iter_mut().for_each(|method| self.function(method)),
                _ => {}
            }
        }

        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if self.error.is_some() {
                return;
            }
            match expr {
                Expr::Quote(..) => return,
                Expr::Splice(_, span) => {
                    let span = *span;
                    match self.value(span).map(|value| splice_expr(value, span)) {
                        Some(Ok(replacement)) => *expr = replacement,
                        Some(Err(error)) => self.error = Some(error),
                        None => {}
                    }
                    return;
                }
                Expr::Call { callee, args, span, .. }
                    if matches!(callee.as_ref(), Expr::Ident(name, _) if name == "splice_field") && args.len() == 2 =>
                {
                    let span = *span;
                    let mut base = args[0].clone();
                    self.visit_expr_mut(&mut base);
                    match self.value(span) {
                        Some(Splice::String(field)) => *expr = Expr::Field { base: Box::new(base), field, span },
                        Some(_) => self.error = Some("an identifier-position splice must evaluate to a string".to_string()),
                        None => {}
                    }
                    return;
                }
                Expr::AssociatedCall { ty, .. } | Expr::StructLiteral { ty, .. } | Expr::Cast { ty, .. } => self.ty(ty),
                Expr::Call { type_args, .. } => type_args.iter_mut().for_each(|ty| self.ty(ty)),
                Expr::Closure { params, .. } => {
                    params.iter_mut().filter_map(|param| param.ty.as_mut()).for_each(|ty| self.ty(ty));
                }
                _ => {}
            }
            match expr {
                Expr::Block(block) | Expr::Unsafe(block, _) => self.block(block),
                Expr::Loop { body, .. } => self.block(body),
                Expr::While { condition, body, .. } => {
                    self.visit_expr_mut(condition);
                    self.block(body);
                }
                Expr::If { condition, then_branch, else_branch, .. } => {
                    self.visit_expr_mut(condition);
                    self.block(then_branch);
                    if let Some(else_branch) = else_branch {
                        self.visit_expr_mut(else_branch);
                    }
                }
                Expr::Select { arms, default, .. } => {
                    for arm in arms {
                        self.visit_expr_mut(&mut arm.operation);
                        self.block(&mut arm.body);
                    }
                    if let Some(default) = default {
                        self.block(default);
                    }
                }
                _ => ast::walk_expr_mut(self, expr),
            }
        }
    }

    let mut body = template.clone();
    let mut splicer = Splicer { values, error: None };
    match &mut body {
        QuoteBody::Item(item) => splicer.visit_item_mut(item),
        QuoteBody::Expr(expr) => splicer.visit_expr_mut(expr),
    }
    match splicer.error {
        Some(error) => Err(error),
        None => Ok(body),
    }
}

fn splice_expr(value: Splice, span: Span) -> Result<Expr, String> {
    Ok(match value {
        Splice::String(value) => Expr::Literal(Literal::String(value), span),
        Splice::Int(value) => Expr::Literal(Literal::Int(value), span),
        Splice::Float(value) => Expr::Literal(Literal::Float(value), span),
        Splice::Bool(value) => Expr::Literal(Literal::Bool(value), span),
        Splice::Char(value) => Expr::Literal(Literal::Char(value), span),
        Splice::Code(code) if matches!(*code, QuoteBody::Expr(_)) => {
            let QuoteBody::Expr(expr) = *code else { unreachable!() };
            expr
        }
        Splice::Code(_) => {
            return Err("cannot splice an item-shaped `Code` into an expression position".to_string());
        }
        Splice::Type(ty) => return Err(format!("cannot splice the type `{}` into an expression position", ty.name())),
    })
}

pub(crate) fn render(code: &QuoteBody) -> String {
    match code {
        QuoteBody::Item(item) => paco_syntax::fmt::format_item(item),
        QuoteBody::Expr(expr) => paco_syntax::fmt::format_expr(expr),
    }
}

/// `Code::join`: `methods` blocks for one type merge; expressions are
/// rendered, joined with `separator` as raw code and parsed again.
pub(crate) fn join(pieces: Vec<QuoteBody>, separator: &str) -> Result<QuoteBody, String> {
    if let Some(QuoteBody::Item(Item::Methods(first))) = pieces.first() {
        let mut merged = first.clone();
        for piece in &pieces[1..] {
            let QuoteBody::Item(Item::Methods(block)) = piece else {
                return Err("Code::join cannot mix `methods` blocks with other `Code`".to_string());
            };
            if ty_name(&block.target) != ty_name(&merged.target) {
                return Err("Code::join can only merge `methods` blocks for the same type".to_string());
            }
            merged.methods.extend(block.methods.iter().cloned());
            merged.consts.extend(block.consts.iter().cloned());
        }
        return Ok(QuoteBody::Item(Item::Methods(merged)));
    }
    let mut rendered = Vec::with_capacity(pieces.len());
    for piece in &pieces {
        match piece {
            QuoteBody::Expr(expr) => rendered.push(paco_syntax::fmt::format_expr(expr)),
            QuoteBody::Item(_) => return Err("Code::join only supports expression-shaped `Code`".to_string()),
        }
    }
    let joined = rendered.join(separator);
    let wrapped = format!("fn __code_join__() {{ {joined} }}");
    let mut sources = paco_span::SourceMap::new();
    let file = sources.add_file("<Code::join>", &wrapped);
    let mut reporter = paco_diag::Reporter::new();
    let tokens = paco_syntax::lex::lex(sources.source(file).unwrap_or(""), file, &mut reporter);
    let module = paco_syntax::parse::parse_module(&tokens, &mut reporter).ok().filter(|_| !reporter.has_errors());
    let Some(mut module) = module else {
        return Err(format!("Code::join produced invalid Paco syntax: {joined:?}"));
    };
    let Some(Item::Fn(function)) = module.items.pop() else {
        return Err("Code::join failed to parse its joined fragments".to_string());
    };
    let Some(tail) = function.body.tail else {
        return Err("Code::join produced an empty expression".to_string());
    };
    Ok(QuoteBody::Expr(*tail))
}

fn ty_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Path(path, _) | Ty::Generic { path, .. } => Some(path.join("::")),
        _ => None,
    }
}

pub(crate) fn type_to_ty(ty: &Type, span: Span) -> Ty {
    let path = |name: &str| Ty::Path(vec![name.to_string()], span);
    match ty {
        Type::Struct(name, args) | Type::Enum(name, args) => {
            let path: Vec<String> = name.split("::").map(str::to_string).collect();
            if args.is_empty() {
                Ty::Path(path, span)
            } else {
                Ty::Generic { path, args: args.iter().map(|arg| type_to_ty(arg, span)).collect(), span }
            }
        }
        Type::Borrow { mutable, ty } => Ty::Borrow { mutable: *mutable, lifetime: None, ty: Box::new(type_to_ty(ty, span)), span },
        Type::RawPointer { mutable, ty } => Ty::RawPointer { mutable: *mutable, ty: Box::new(type_to_ty(ty, span)), span },
        Type::Slice(ty) => Ty::Slice(Box::new(type_to_ty(ty, span)), span),
        Type::Tuple(items) => Ty::Tuple(items.iter().map(|item| type_to_ty(item, span)).collect(), span),
        Type::Fn(params, ret) => Ty::Fn {
            params: params.iter().map(|param| type_to_ty(param, span)).collect(),
            return_ty: Some(Box::new(type_to_ty(ret, span))),
            span,
        },
        Type::Unit => Ty::Tuple(Vec::new(), span),
        Type::Never => Ty::Never(span),
        Type::Generic(name) => path(name),
        Type::TypeValue(_) => path("type"),
        Type::Unknown | Type::Error => Ty::Infer(span),
        other => path(&other.name()),
    }
}

/// The type a struct field's declared `ty` names, for `fields_of`.
pub(crate) fn resolve_ty(ty: &Ty, enums: &HashSet<String>) -> Type {
    match ty {
        Ty::Path(path, _) if path.len() == 1 => match path[0].as_str() {
            "i8" => Type::Int(IntWidth::I8),
            "i16" => Type::Int(IntWidth::I16),
            "i32" => Type::Int(IntWidth::I32),
            "i64" => Type::Int(IntWidth::I64),
            "u8" | "byte" => Type::Int(IntWidth::U8),
            "u16" => Type::Int(IntWidth::U16),
            "u32" => Type::Int(IntWidth::U32),
            "u64" => Type::Int(IntWidth::U64),
            name if FloatWidth::from_name(name).is_some() => Type::Float(FloatWidth::from_name(name).expect("checked")),
            "bool" => Type::Bool,
            "char" => Type::Char,
            "string" => Type::String,
            name if enums.contains(name) => Type::Enum(name.to_string(), Vec::new()),
            name => Type::Struct(name.to_string(), Vec::new()),
        },
        Ty::Borrow { mutable, ty, .. } => Type::Borrow { mutable: *mutable, ty: Box::new(resolve_ty(ty, enums)) },
        Ty::RawPointer { mutable, ty, .. } => Type::RawPointer { mutable: *mutable, ty: Box::new(resolve_ty(ty, enums)) },
        Ty::Slice(ty, _) => Type::Slice(Box::new(resolve_ty(ty, enums))),
        _ => Type::Unknown,
    }
}
