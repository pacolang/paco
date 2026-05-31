//! Abstract syntax tree definitions and visitor infrastructure.

use paco_span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Fn(FnDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Trait(TraitDecl),
    Methods(MethodsBlock),
    Use(UseDecl),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_ty: Option<Ty>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub pattern: Pat,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StructDecl {
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub generics: Vec<String>,
    pub variants: Vec<EnumVariant>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    pub fields: VariantFields,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VariantFields {
    Unit,
    Tuple(Vec<Ty>),
    Struct(Vec<FieldDecl>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraitDecl {
    pub name: String,
    pub generics: Vec<String>,
    pub methods: Vec<FnSignature>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnSignature {
    pub name: String,
    pub params: Vec<Param>,
    pub return_ty: Option<Ty>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodsBlock {
    pub generics: Vec<String>,
    pub target: Ty,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let(LetStmt),
    Expr(Expr),
    Item(Item),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LetStmt {
    pub mutable: bool,
    pub pattern: Pat,
    pub ty: Option<Ty>,
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Literal(Literal, Span),
    Ident(String, Span),
    Block(Box<Block>),
    If {
        condition: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Box<Expr>>,
        span: Span,
    },
    Loop {
        body: Block,
        span: Span,
    },
    While {
        condition: Box<Expr>,
        body: Block,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<Expr>,
        span: Span,
    },
    AssociatedCall {
        ty: Ty,
        function: String,
        args: Vec<Expr>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        field: String,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    Return(Option<Box<Expr>>, Span),
    Break(Option<Box<Expr>>, Span),
    Continue(Span),
    Spawn {
        expr: Box<Expr>,
        span: Span,
    },
    Select {
        arms: Vec<SelectArm>,
        default: Option<Block>,
        span: Span,
    },
    Comptime {
        expr: Box<Expr>,
        span: Span,
    },
    Yield(Box<Expr>, Span),
    StructLiteral {
        ty: Ty,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
    Borrow {
        mutable: bool,
        expr: Box<Expr>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    pub pattern: Pat,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectArm {
    pub operation: Expr,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pat {
    Ident(String, Span),
    Wildcard(Span),
    Literal(Literal, Span),
    Tuple(Vec<Pat>, Span),
    Struct {
        path: Vec<String>,
        fields: Vec<(String, Pat)>,
        span: Span,
    },
    Enum {
        path: Vec<String>,
        fields: Vec<Pat>,
        span: Span,
    },
    Range {
        start: Box<Pat>,
        end: Box<Pat>,
        inclusive: bool,
        span: Span,
    },
    Or(Vec<Pat>, Span),
    Binding {
        name: String,
        pattern: Box<Pat>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Path(Vec<String>, Span),
    Tuple(Vec<Ty>, Span),
    Slice(Box<Ty>, Span),
    Borrow {
        mutable: bool,
        lifetime: Option<String>,
        ty: Box<Ty>,
        span: Span,
    },
    Dyn {
        trait_path: Vec<String>,
        span: Span,
    },
    Fn {
        params: Vec<Ty>,
        return_ty: Option<Box<Ty>>,
        span: Span,
    },
    Infer(Span),
    Never(Span),
}

pub trait Visit {
    fn visit_module(&mut self, module: &Module) {
        walk_module(self, module);
    }

    fn visit_item(&mut self, item: &Item) {
        walk_item(self, item);
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        walk_fn_decl(self, function);
    }

    fn visit_block(&mut self, block: &Block) {
        walk_block(self, block);
    }

    fn visit_stmt(&mut self, statement: &Stmt) {
        walk_stmt(self, statement);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        walk_expr(self, expr);
    }

    fn visit_literal(&mut self, _literal: &Literal) {}
}

pub trait MutVisit {
    fn visit_module_mut(&mut self, module: &mut Module) {
        walk_module_mut(self, module);
    }

    fn visit_item_mut(&mut self, item: &mut Item) {
        walk_item_mut(self, item);
    }

    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        walk_expr_mut(self, expr);
    }
}

pub fn walk_module<V: Visit + ?Sized>(visitor: &mut V, module: &Module) {
    for item in &module.items {
        visitor.visit_item(item);
    }
}

pub fn walk_item<V: Visit + ?Sized>(visitor: &mut V, item: &Item) {
    match item {
        Item::Fn(function) => visitor.visit_fn_decl(function),
        Item::Struct(decl) => {
            for method in &decl.methods {
                visitor.visit_fn_decl(method);
            }
        }
        Item::Enum(decl) => {
            for method in &decl.methods {
                visitor.visit_fn_decl(method);
            }
        }
        Item::Methods(decl) => {
            for method in &decl.methods {
                visitor.visit_fn_decl(method);
            }
        }
        Item::Trait(_) | Item::Use(_) => {}
    }
}

pub fn walk_fn_decl<V: Visit + ?Sized>(visitor: &mut V, function: &FnDecl) {
    visitor.visit_block(&function.body);
}

pub fn walk_block<V: Visit + ?Sized>(visitor: &mut V, block: &Block) {
    for statement in &block.stmts {
        visitor.visit_stmt(statement);
    }
    if let Some(tail) = &block.tail {
        visitor.visit_expr(tail);
    }
}

pub fn walk_stmt<V: Visit + ?Sized>(visitor: &mut V, statement: &Stmt) {
    match statement {
        Stmt::Let(statement) => {
            if let Some(value) = &statement.value {
                visitor.visit_expr(value);
            }
        }
        Stmt::Expr(expr) => visitor.visit_expr(expr),
        Stmt::Item(item) => visitor.visit_item(item),
    }
}

pub fn walk_expr<V: Visit + ?Sized>(visitor: &mut V, expr: &Expr) {
    match expr {
        Expr::Literal(literal, _) => visitor.visit_literal(literal),
        Expr::Ident(_, _) | Expr::Continue(_) => {}
        Expr::Block(block) => visitor.visit_block(block),
        Expr::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            visitor.visit_expr(condition);
            visitor.visit_block(then_branch);
            if let Some(else_branch) = else_branch {
                visitor.visit_expr(else_branch);
            }
        }
        Expr::Loop { body, .. } => visitor.visit_block(body),
        Expr::While {
            condition, body, ..
        } => {
            visitor.visit_expr(condition);
            visitor.visit_block(body);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            visitor.visit_expr(scrutinee);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    visitor.visit_expr(guard);
                }
                visitor.visit_expr(&arm.body);
            }
        }
        Expr::Call { callee, args, .. } => {
            visitor.visit_expr(callee);
            for arg in args {
                visitor.visit_expr(arg);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            visitor.visit_expr(receiver);
            for arg in args {
                visitor.visit_expr(arg);
            }
        }
        Expr::AssociatedCall { args, .. } => {
            for arg in args {
                visitor.visit_expr(arg);
            }
        }
        Expr::Binary { left, right, .. } => {
            visitor.visit_expr(left);
            visitor.visit_expr(right);
        }
        Expr::Unary { expr, .. } => visitor.visit_expr(expr),
        Expr::Assign { target, value, .. } => {
            visitor.visit_expr(target);
            visitor.visit_expr(value);
        }
        Expr::Field { base, .. } => visitor.visit_expr(base),
        Expr::Index { base, index, .. } => {
            visitor.visit_expr(base);
            visitor.visit_expr(index);
        }
        Expr::Return(value, _) | Expr::Break(value, _) => {
            if let Some(value) = value {
                visitor.visit_expr(value);
            }
        }
        Expr::Spawn { expr, .. } | Expr::Comptime { expr, .. } => visitor.visit_expr(expr),
        Expr::Select { arms, default, .. } => {
            for arm in arms {
                visitor.visit_expr(&arm.operation);
                visitor.visit_block(&arm.body);
            }
            if let Some(default) = default {
                visitor.visit_block(default);
            }
        }
        Expr::Yield(expr, _) => visitor.visit_expr(expr),
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                visitor.visit_expr(value);
            }
        }
        Expr::Borrow { expr, .. } => visitor.visit_expr(expr),
    }
}

pub fn walk_module_mut<V: MutVisit + ?Sized>(visitor: &mut V, module: &mut Module) {
    for item in &mut module.items {
        visitor.visit_item_mut(item);
    }
}

pub fn walk_item_mut<V: MutVisit + ?Sized>(visitor: &mut V, item: &mut Item) {
    match item {
        Item::Fn(function) => walk_block_mut(visitor, &mut function.body),
        Item::Struct(decl) => {
            for method in &mut decl.methods {
                walk_block_mut(visitor, &mut method.body);
            }
        }
        Item::Enum(decl) => {
            for method in &mut decl.methods {
                walk_block_mut(visitor, &mut method.body);
            }
        }
        Item::Methods(decl) => {
            for method in &mut decl.methods {
                walk_block_mut(visitor, &mut method.body);
            }
        }
        Item::Trait(_) | Item::Use(_) => {}
    }
}

pub fn walk_block_mut<V: MutVisit + ?Sized>(visitor: &mut V, block: &mut Block) {
    for statement in &mut block.stmts {
        match statement {
            Stmt::Let(statement) => {
                if let Some(value) = &mut statement.value {
                    visitor.visit_expr_mut(value);
                }
            }
            Stmt::Expr(expr) => visitor.visit_expr_mut(expr),
            Stmt::Item(item) => visitor.visit_item_mut(item),
        }
    }
    if let Some(tail) = &mut block.tail {
        visitor.visit_expr_mut(tail);
    }
}

pub fn walk_expr_mut<V: MutVisit + ?Sized>(visitor: &mut V, expr: &mut Expr) {
    match expr {
        Expr::Literal(_, _) | Expr::Ident(_, _) | Expr::Continue(_) => {}
        Expr::Block(block) => walk_block_mut(visitor, block),
        Expr::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            visitor.visit_expr_mut(condition);
            walk_block_mut(visitor, then_branch);
            if let Some(else_branch) = else_branch {
                visitor.visit_expr_mut(else_branch);
            }
        }
        Expr::Loop { body, .. } => walk_block_mut(visitor, body),
        Expr::While {
            condition, body, ..
        } => {
            visitor.visit_expr_mut(condition);
            walk_block_mut(visitor, body);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            visitor.visit_expr_mut(scrutinee);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    visitor.visit_expr_mut(guard);
                }
                visitor.visit_expr_mut(&mut arm.body);
            }
        }
        Expr::Call { callee, args, .. } => {
            visitor.visit_expr_mut(callee);
            for arg in args {
                visitor.visit_expr_mut(arg);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            visitor.visit_expr_mut(receiver);
            for arg in args {
                visitor.visit_expr_mut(arg);
            }
        }
        Expr::AssociatedCall { args, .. } => {
            for arg in args {
                visitor.visit_expr_mut(arg);
            }
        }
        Expr::Binary { left, right, .. } => {
            visitor.visit_expr_mut(left);
            visitor.visit_expr_mut(right);
        }
        Expr::Unary { expr, .. } => visitor.visit_expr_mut(expr),
        Expr::Assign { target, value, .. } => {
            visitor.visit_expr_mut(target);
            visitor.visit_expr_mut(value);
        }
        Expr::Field { base, .. } => visitor.visit_expr_mut(base),
        Expr::Index { base, index, .. } => {
            visitor.visit_expr_mut(base);
            visitor.visit_expr_mut(index);
        }
        Expr::Return(value, _) | Expr::Break(value, _) => {
            if let Some(value) = value {
                visitor.visit_expr_mut(value);
            }
        }
        Expr::Spawn { expr, .. } | Expr::Comptime { expr, .. } => visitor.visit_expr_mut(expr),
        Expr::Select { arms, default, .. } => {
            for arm in arms {
                visitor.visit_expr_mut(&mut arm.operation);
                walk_block_mut(visitor, &mut arm.body);
            }
            if let Some(default) = default {
                walk_block_mut(visitor, default);
            }
        }
        Expr::Yield(expr, _) => visitor.visit_expr_mut(expr),
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                visitor.visit_expr_mut(value);
            }
        }
        Expr::Borrow { expr, .. } => visitor.visit_expr_mut(expr),
    }
}
