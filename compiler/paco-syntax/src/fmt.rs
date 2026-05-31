//! AST-based code formatter for Paco.

use crate::ast::{
    BinaryOp, Block, EnumDecl, Expr, FnDecl, Item, Literal,
    MethodsBlock, Module, Param, Pat, Stmt, StructDecl, TraitDecl, Ty, UnaryOp, UseDecl,
    VariantFields,
};

pub fn format_module(module: &Module, source: Option<&str>) -> String {
    let mut formatter = Formatter::new(source);
    formatter.format_module(module);
    formatter.output
}

struct Formatter<'a> {
    output: String,
    indent_level: usize,
    source: Option<&'a str>,
}

impl<'a> Formatter<'a> {
    fn new(source: Option<&'a str>) -> Self {
        Self {
            output: String::new(),
            indent_level: 0,
            source,
        }
    }

    fn write(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent_level {
            self.output.push_str("    ");
        }
    }

    fn newline(&mut self) {
        self.output.push('\n');
    }

    fn indent(&mut self) {
        self.indent_level += 1;
    }

    fn dedent(&mut self) {
        if self.indent_level > 0 {
            self.indent_level -= 1;
        }
    }

    fn format_module(&mut self, module: &Module) {
        for (i, item) in module.items.iter().enumerate() {
            if i > 0 {
                self.newline();
                self.newline();
            }
            self.format_item(item);
        }
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.newline();
        }
    }

    fn format_item(&mut self, item: &Item) {
        match item {
            Item::Fn(fn_decl) => self.format_fn_decl(fn_decl),
            Item::Struct(struct_decl) => self.format_struct_decl(struct_decl),
            Item::Enum(enum_decl) => self.format_enum_decl(enum_decl),
            Item::Trait(trait_decl) => self.format_trait_decl(trait_decl),
            Item::Methods(methods_block) => self.format_methods_block(methods_block),
            Item::Use(use_decl) => self.format_use_decl(use_decl),
        }
    }

    fn format_use_decl(&mut self, use_decl: &UseDecl) {
        self.write_indent();
        self.write("use ");
        self.write(&use_decl.path.join("::"));
    }

    fn format_generics(&mut self, generics: &[String]) {
        if !generics.is_empty() {
            self.write("<");
            self.write(&generics.join(", "));
            self.write(">");
        }
    }

    fn format_fn_decl(&mut self, fn_decl: &FnDecl) {
        self.write_indent();
        self.write("fn ");
        self.write(&fn_decl.name);
        self.format_generics(&fn_decl.generics);
        self.write("(");
        for (i, param) in fn_decl.params.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.format_param(param);
        }
        self.write(")");
        if let Some(ref ret_ty) = fn_decl.return_ty {
            self.write(" -> ");
            self.format_ty(ret_ty);
        }
        self.write(" ");
        self.format_block(&fn_decl.body);
    }

    #[allow(clippy::collapsible_if, clippy::collapsible_match)]
    fn format_param(&mut self, param: &Param) {
        if let Pat::Ident(ref name, _) = param.pattern {
            if name == "self" {
                match &param.ty {
                    Ty::Borrow { mutable, ty, .. } => {
                        if let Ty::Path(path, _) = &**ty {
                            if path.as_slice() == ["Self"] {
                                self.write("self&");
                                if *mutable {
                                    self.write("mut");
                                }
                                return;
                            }
                        }
                    }
                    Ty::Path(path, _) => {
                        if path.as_slice() == ["Self"] {
                            self.write("self");
                            return;
                        }
                    }
                    _ => {}
                }
            }
        }
        self.format_pat(&param.pattern);
        self.write(": ");
        self.format_ty(&param.ty);
    }

    fn format_struct_decl(&mut self, struct_decl: &StructDecl) {
        self.write_indent();
        self.write("struct ");
        self.write(&struct_decl.name);
        self.format_generics(&struct_decl.generics);
        self.write(" {");
        self.newline();
        self.indent();

        for (i, field) in struct_decl.fields.iter().enumerate() {
            self.write_indent();
            self.write(&field.name);
            self.write(": ");
            self.format_ty(&field.ty);
            if i < struct_decl.fields.len() - 1 || !struct_decl.methods.is_empty() {
                self.write(",");
            }
            self.newline();
        }

        if !struct_decl.methods.is_empty() {
            if !struct_decl.fields.is_empty() {
                self.newline();
            }
            for (i, method) in struct_decl.methods.iter().enumerate() {
                if i > 0 {
                    self.newline();
                }
                self.format_fn_decl(method);
                self.newline();
            }
        }

        self.dedent();
        self.write_indent();
        self.write("}");
    }

    fn format_enum_decl(&mut self, enum_decl: &EnumDecl) {
        self.write_indent();
        self.write("enum ");
        self.write(&enum_decl.name);
        self.format_generics(&enum_decl.generics);
        self.write(" {");
        self.newline();
        self.indent();

        for (i, variant) in enum_decl.variants.iter().enumerate() {
            self.write_indent();
            self.write(&variant.name);
            match &variant.fields {
                VariantFields::Unit => {}
                VariantFields::Tuple(tys) => {
                    self.write("(");
                    for (j, ty) in tys.iter().enumerate() {
                        if j > 0 {
                            self.write(", ");
                        }
                        self.format_ty(ty);
                    }
                    self.write(")");
                }
                VariantFields::Struct(fields) => {
                    self.write(" {");
                    self.newline();
                    self.indent();
                    for (j, field) in fields.iter().enumerate() {
                        self.write_indent();
                        self.write(&field.name);
                        self.write(": ");
                        self.format_ty(&field.ty);
                        if j < fields.len() - 1 {
                            self.write(",");
                        }
                        self.newline();
                    }
                    self.dedent();
                    self.write_indent();
                    self.write("}");
                }
            }
            if i < enum_decl.variants.len() - 1 || !enum_decl.methods.is_empty() {
                self.write(",");
            }
            self.newline();
        }

        if !enum_decl.methods.is_empty() {
            if !enum_decl.variants.is_empty() {
                self.newline();
            }
            for (i, method) in enum_decl.methods.iter().enumerate() {
                if i > 0 {
                    self.newline();
                }
                self.format_fn_decl(method);
                self.newline();
            }
        }

        self.dedent();
        self.write_indent();
        self.write("}");
    }

    fn format_trait_decl(&mut self, trait_decl: &TraitDecl) {
        self.write_indent();
        self.write("trait ");
        self.write(&trait_decl.name);
        self.format_generics(&trait_decl.generics);
        self.write(" {");
        self.newline();
        self.indent();

        for (i, method) in trait_decl.methods.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.write_indent();
            self.write("fn ");
            self.write(&method.name);
            self.write("(");
            for (j, param) in method.params.iter().enumerate() {
                if j > 0 {
                    self.write(", ");
                }
                self.format_param(param);
            }
            self.write(")");
            if let Some(ref ret_ty) = method.return_ty {
                self.write(" -> ");
                self.format_ty(ret_ty);
            }
            self.write(";");
            self.newline();
        }

        self.dedent();
        self.write_indent();
        self.write("}");
    }

    fn format_methods_block(&mut self, methods_block: &MethodsBlock) {
        self.write_indent();
        self.write("methods ");
        self.format_generics(&methods_block.generics);
        if !methods_block.generics.is_empty() {
            self.write(" ");
        }
        self.format_ty(&methods_block.target);
        self.write(" {");
        self.newline();
        self.indent();

        for (i, method) in methods_block.methods.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.format_fn_decl(method);
            self.newline();
        }

        self.dedent();
        self.write_indent();
        self.write("}");
    }

    fn format_block(&mut self, block: &Block) {
        self.write("{");
        if block.stmts.is_empty() && block.tail.is_none() {
            self.write("}");
            return;
        }
        self.newline();
        self.indent();

        for stmt in &block.stmts {
            self.format_stmt(stmt);
        }
        if let Some(ref tail) = block.tail {
            self.write_indent();
            self.format_expr(tail);
            self.newline();
        }

        self.dedent();
        self.write_indent();
        self.write("}");
    }

    fn format_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(let_stmt) => {
                self.write_indent();
                self.write("let ");
                if let_stmt.mutable {
                    self.write("mut ");
                }
                self.format_pat(&let_stmt.pattern);
                if let Some(ref ty) = let_stmt.ty {
                    self.write(": ");
                    self.format_ty(ty);
                }
                if let Some(ref value) = let_stmt.value {
                    self.write(" = ");
                    self.format_expr(value);
                }
                self.newline();
            }
            Stmt::Expr(expr) => {
                self.write_indent();
                self.format_expr(expr);
                self.newline();
            }
            Stmt::Item(item) => {
                self.format_item(item);
                self.newline();
            }
        }
    }

    fn format_literal(&mut self, lit: &Literal) {
        match lit {
            Literal::Int(n) => self.write(&n.to_string()),
            Literal::Float(f) => {
                let s = f.to_string();
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    self.write(&s);
                } else {
                    self.write(&format!("{}.0", s));
                }
            }
            Literal::Bool(b) => self.write(if *b { "true" } else { "false" }),
            Literal::String(s) => {
                self.write("\"");
                for c in s.chars() {
                    match c {
                        '\n' => self.write("\\n"),
                        '\r' => self.write("\\r"),
                        '\t' => self.write("\\t"),
                        '\\' => self.write("\\\\"),
                        '"' => self.write("\\\""),
                        _ => self.output.push(c),
                    }
                }
                self.write("\"");
            }
            Literal::Char(c) => {
                self.write("'");
                match c {
                    '\n' => self.write("\\n"),
                    '\r' => self.write("\\r"),
                    '\t' => self.write("\\t"),
                    '\\' => self.write("\\\\"),
                    '\'' => self.write("\\'"),
                    _ => self.output.push(*c),
                }
                self.write("'");
            }
        }
    }

    fn format_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(name, _) => self.write(name),
            Pat::Wildcard(_) => self.write("_"),
            Pat::Literal(lit, _) => self.format_literal(lit),
            Pat::Tuple(pats, _) => {
                self.write("(");
                for (i, p) in pats.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.format_pat(p);
                }
                self.write(")");
            }
            Pat::Struct { path, fields, .. } => {
                self.write(&path.join("::"));
                self.write(" { ");
                for (i, (name, p)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(name);
                    self.write(": ");
                    self.format_pat(p);
                }
                self.write(" }");
            }
            Pat::Enum { path, fields, .. } => {
                self.write(&path.join("::"));
                if !fields.is_empty() {
                    self.write("(");
                    for (i, p) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.format_pat(p);
                    }
                    self.write(")");
                }
            }
            Pat::Range { start, end, inclusive, .. } => {
                self.format_pat(start);
                if *inclusive {
                    self.write("..=");
                } else {
                    self.write("..");
                }
                self.format_pat(end);
            }
            Pat::Or(pats, _) => {
                for (i, p) in pats.iter().enumerate() {
                    if i > 0 {
                        self.write(" | ");
                    }
                    self.format_pat(p);
                }
            }
            Pat::Binding { name, pattern, .. } => {
                self.write(name);
                self.write("@");
                self.format_pat(pattern);
            }
        }
    }

    fn format_ty(&mut self, ty: &Ty) {
        match ty {
            Ty::Path(path, _) => self.write(&path.join("::")),
            Ty::Generic { path, args, .. } => {
                self.write(&path.join("::"));
                self.write("<");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.format_ty(arg);
                }
                self.write(">");
            }
            Ty::Tuple(tys, _) => {
                self.write("(");
                for (i, t) in tys.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.format_ty(t);
                }
                self.write(")");
            }
            Ty::Slice(inner, _) => {
                self.write("[");
                self.format_ty(inner);
                self.write("]");
            }
            Ty::Borrow { mutable, lifetime, ty: inner, .. } => {
                self.write("&");
                if let Some(lt) = lifetime {
                    self.write("'");
                    self.write(lt);
                    self.write(" ");
                }
                if *mutable {
                    self.write("mut ");
                }
                self.format_ty(inner);
            }
            Ty::Dyn { trait_path, .. } => {
                self.write("dyn ");
                self.write(&trait_path.join("::"));
            }
            Ty::Fn { params, return_ty, .. } => {
                self.write("fn(");
                for (i, param) in params.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.format_ty(param);
                }
                self.write(")");
                if let Some(ret) = return_ty {
                    self.write(" -> ");
                    self.format_ty(ret);
                }
            }
            Ty::Infer(_) => self.write("_"),
            Ty::Never(_) => self.write("!"),
        }
    }

    fn format_expr_with_precedence(&mut self, expr: &Expr, parent_prec: u8) {
        let prec = expr_precedence(expr);
        if prec < parent_prec {
            self.write("(");
            self.format_expr(expr);
            self.write(")");
        } else {
            self.format_expr(expr);
        }
    }

    fn format_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Literal(lit, _) => self.format_literal(lit),
            Expr::Ident(name, _) => self.write(name),
            Expr::Block(block) => self.format_block(block),
            Expr::If { condition, then_branch, else_branch, .. } => {
                self.write("if ");
                self.format_expr(condition);
                self.write(" ");
                self.format_block(then_branch);
                if let Some(else_br) = else_branch {
                    self.write(" else ");
                    self.format_expr(else_br);
                }
            }
            Expr::Loop { body, .. } => {
                self.write("loop ");
                self.format_block(body);
            }
            Expr::While { condition, body, .. } => {
                self.write("while ");
                self.format_expr(condition);
                self.write(" ");
                self.format_block(body);
            }
            Expr::Match { scrutinee, arms, .. } => {
                self.write("match ");
                self.format_expr(scrutinee);
                self.write(" {");
                self.newline();
                self.indent();
                for arm in arms {
                    self.write_indent();
                    self.format_pat(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.write(" if ");
                        self.format_expr(guard);
                    }
                    self.write(" => ");
                    self.format_expr(&arm.body);
                    if !matches!(arm.body, Expr::Block(_)) {
                        self.write(",");
                    }
                    self.newline();
                }
                self.dedent();
                self.write_indent();
                self.write("}");
            }
            Expr::Call { callee, args, .. } => {
                self.format_expr_with_precedence(callee, 8);
                self.write("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.format_expr(arg);
                }
                self.write(")");
            }
            Expr::MethodCall { receiver, method, args, .. } => {
                self.format_expr_with_precedence(receiver, 8);
                self.write(".");
                self.write(method);
                self.write("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.format_expr(arg);
                }
                self.write(")");
            }
            Expr::AssociatedCall { ty, function, args, span } => {
                self.format_ty(ty);
                self.write("::");
                self.write(function);
                if !args.is_empty() {
                    self.write("(");
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.format_expr(arg);
                    }
                    self.write(")");
                } else {
                    let has_parens = if let Some(src) = self.source {
                        let end = span.end();
                        if end > 0 && end <= src.len() {
                            src[..end].trim_end().ends_with(')')
                        } else {
                            false
                        }
                    } else {
                        function.chars().next().is_some_and(|c| c.is_lowercase())
                    };

                    if has_parens {
                        self.write("()");
                    }
                }
            }
            Expr::Binary { op, left, right, .. } => {
                let dummy_expr = Expr::Binary {
                    op: *op,
                    left: Box::new(left.as_ref().clone()),
                    right: Box::new(right.as_ref().clone()),
                    span: paco_span::Span::new(paco_span::FileId::new(0), 0, 0),
                };
                let prec = expr_precedence(&dummy_expr);
                self.format_expr_with_precedence(left, prec);
                self.write(" ");
                self.write(bin_op_str(op));
                self.write(" ");
                self.format_expr_with_precedence(right, prec + 1);
            }
            Expr::Unary { op, expr, .. } => {
                self.write(unary_op_str(op));
                self.format_expr_with_precedence(expr, 7);
            }
            Expr::Assign { target, value, .. } => {
                let prec = 1;
                self.format_expr_with_precedence(target, prec + 1);
                self.write(" = ");
                self.format_expr_with_precedence(value, prec);
            }
            Expr::Field { base, field, .. } => {
                self.format_expr_with_precedence(base, 8);
                self.write(".");
                self.write(field);
            }
            Expr::Index { base, index, .. } => {
                self.format_expr_with_precedence(base, 8);
                self.write("[");
                self.format_expr(index);
                self.write("]");
            }
            Expr::Return(val, _) => {
                self.write("return");
                if let Some(v) = val {
                    self.write(" ");
                    self.format_expr(v);
                }
            }
            Expr::Break(val, _) => {
                self.write("break");
                if let Some(v) = val {
                    self.write(" ");
                    self.format_expr(v);
                }
            }
            Expr::Continue(_) => {
                self.write("continue");
            }
            Expr::Spawn { expr, .. } => {
                self.write("spawn ");
                self.format_expr(expr);
            }
            Expr::Select { arms, default, .. } => {
                self.write("select {");
                self.newline();
                self.indent();
                for arm in arms {
                    self.write_indent();
                    self.format_expr(&arm.operation);
                    self.write(" => ");
                    self.format_block(&arm.body);
                    self.newline();
                }
                if let Some(def) = default {
                    self.write_indent();
                    self.write("default => ");
                    self.format_block(def);
                    self.newline();
                }
                self.dedent();
                self.write_indent();
                self.write("}");
            }
            Expr::Comptime { expr, .. } => {
                self.write("comptime ");
                self.format_expr(expr);
            }
            Expr::Yield(expr, _) => {
                self.write("yield ");
                self.format_expr(expr);
            }
            Expr::StructLiteral { ty, fields, .. } => {
                self.format_ty(ty);
                self.write(" {");
                if fields.is_empty() {
                    self.write("}");
                } else {
                    self.write(" ");
                    for (i, (name, val)) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.write(name);
                        self.write(": ");
                        self.format_expr(val);
                    }
                    self.write(" }");
                }
            }
            Expr::Borrow { mutable, expr, .. } => {
                self.write("&");
                if *mutable {
                    self.write("mut ");
                }
                self.format_expr_with_precedence(expr, 7);
            }
        }
    }
}

fn expr_precedence(expr: &Expr) -> u8 {
    match expr {
        Expr::Assign { .. } => 1,
        Expr::Binary { op, .. } => match op {
            BinaryOp::Or => 2,
            BinaryOp::And => 3,
            BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 4,
            BinaryOp::Add | BinaryOp::Sub => 5,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 6,
        },
        Expr::Unary { .. } | Expr::Borrow { .. } => 7,
        _ => 8,
    }
}

fn bin_op_str(op: &BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

fn unary_op_str(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "!",
        UnaryOp::Neg => "-",
    }
}
