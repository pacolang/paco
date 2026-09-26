//! Abstract syntax tree definitions and visitor infrastructure.

use paco_span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub name: Option<ModuleDecl>,
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleDecl {
    pub name: String,
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
    Const(ConstDecl),
    Extern(ExternBlock),
}

/// The body of a `quote { .. }` expression (`Expr::Quote`,
/// `phase-9-comptime` Decision 7): the template parses as an item when
/// its content starts with an item keyword (e.g. `methods`), and as an
/// expression otherwise.
#[derive(Clone, Debug, PartialEq)]
pub enum QuoteBody {
    Item(Item),
    Expr(Expr),
}

/// `#[derive(Display, Serialize)]` (grammar.ebnf's `OuterAttribute`).
/// `name` is the bare identifier (`derive`); `args` is empty for a
/// bare `#[name]` with no parenthesized argument list at all (`()` with
/// zero args, e.g. `#[test]`, parses to `Some(vec![])` — distinct from no
/// parens — but this AST collapses both to an empty `args`, since no
/// current consumer needs to tell them apart).
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub args: Vec<AttributeArg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AttributeArg {
    Literal(Literal, Span),
    Path(Vec<String>, Span),
    Nested(Attribute),
    /// `key = path`, as in `#[derivative(of = f)]`.
    Assign(String, Vec<String>, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExternBlock {
    pub abi: String,
    pub functions: Vec<FnSignature>,
    pub span: Span,
}

/// `const NAME: Type = Expr` (ADR 0016). `ty` is mandatory — unlike
/// `LetStmt.ty`, a `const`'s type is never inferred.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Ty,
    pub value: Expr,
    pub is_pub: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub name_splice: Option<Box<Expr>>,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_ty: Option<Ty>,
    pub body: Block,
    pub is_pub: bool,
    pub is_unsafe: bool,
    pub is_iter: bool,
    pub is_comptime: bool,
    pub extern_abi: Option<String>,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenericParam {
    pub name: String,
    pub kind: GenericParamKind,
    pub bounds: Vec<Ty>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GenericParamKind {
    Type,
    Lifetime,
    Const(Ty),
    ConstPack(Ty),
    Dim,
}

impl GenericParam {
    pub fn is_const(&self) -> bool {
        matches!(self.kind, GenericParamKind::Const(_) | GenericParamKind::ConstPack(_))
    }

    pub fn is_pack(&self) -> bool {
        matches!(self.kind, GenericParamKind::ConstPack(_))
    }

    pub fn is_dim(&self) -> bool {
        matches!(self.kind, GenericParamKind::Dim)
    }
}

impl PartialEq<String> for GenericParam {
    fn eq(&self, other: &String) -> bool {
        self.kind == GenericParamKind::Type && &self.name == other
    }
}

impl PartialEq<&str> for GenericParam {
    fn eq(&self, other: &&str) -> bool {
        self.kind == GenericParamKind::Type && self.name == *other
    }
}

pub fn has_derive(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().filter(|attr| attr.name == "derive").flat_map(|attr| &attr.args).any(
        |arg| matches!(arg, AttributeArg::Path(path, _) if path.last().is_some_and(|last| last == name)),
    )
}

/// The names of the parameters in `params` bounded by the trait `bound`.
pub fn bounded_by(params: &[GenericParam], bound: &str) -> Vec<String> {
    params
        .iter()
        .filter(|param| {
            param.bounds.iter().any(|ty| matches!(ty, Ty::Path(path, _) | Ty::Generic { path, .. } if path.last().is_some_and(|last| last == bound)))
        })
        .map(|param| param.name.clone())
        .collect()
}

pub fn generic_names(params: &[GenericParam]) -> Vec<String> {
    params
        .iter()
        .filter(|param| param.kind != GenericParamKind::Lifetime)
        .map(|param| param.name.clone())
        .collect()
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
    pub generics: Vec<GenericParam>,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<FnDecl>,
    pub consts: Vec<ConstDecl>,
    /// `type Name = Ty;` members, e.g. `Differentiable`'s `Tangent`.
    pub assoc_types: Vec<AssocTypeDecl>,
    pub is_pub: bool,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: Ty,
    pub is_pub: bool,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub variants: Vec<EnumVariant>,
    pub methods: Vec<FnDecl>,
    pub consts: Vec<ConstDecl>,
    pub is_pub: bool,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    pub fields: VariantFields,
    pub attrs: Vec<Attribute>,
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
    pub generics: Vec<GenericParam>,
    pub methods: Vec<FnSignature>,
    pub consts: Vec<ConstDecl>,
    pub assoc_types: Vec<AssocTypeDecl>,
    pub is_pub: bool,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnSignature {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_ty: Option<Ty>,
    pub body: Option<Block>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssocTypeDecl {
    pub name: String,
    pub default: Option<Ty>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodsBlock {
    pub generics: Vec<GenericParam>,
    pub target: Ty,
    pub methods: Vec<FnDecl>,
    pub consts: Vec<ConstDecl>,
    pub span: Span,
}

/// Which grammar form (ADR 0015) a `UseDecl.path` was parsed from: plain
/// `::`-separated (`use a::b::c`) or `DomainPath` (`use a.b/c`). Both
/// flatten to the same `Vec<String>`, so this is the only way downstream
/// code (e.g. `git-module-fetch`'s dependency resolution) can tell them
/// apart once parsing is done.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsePathKind {
    Plain,
    Domain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub kind: UsePathKind,
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
    Unsafe(Box<Block>, Span),
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
        type_args: Vec<Ty>,
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
        index: Vec<Expr>,
        span: Span,
    },
    Try {
        expr: Box<Expr>,
        span: Span,
    },
    Cast {
        expr: Box<Expr>,
        ty: Ty,
        span: Span,
    },
    Return(Option<Box<Expr>>, Span),
    Break(Option<Box<Expr>>, Span),
    Continue(Span),
    Spawn {
        expr: Box<Expr>,
        span: Span,
    },
    Closure {
        params: Vec<ClosureParam>,
        body: Box<Expr>,
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
    /// `quote { .. }` (`phase-9-comptime` Decision 7): a comptime-only
    /// template, parsed with the ordinary `Item`/`Expr` grammar except
    /// for `#(expr)` splice points (`Expr::Splice`/`Ty::Splice`) — one
    /// item (e.g. a `methods` block) or one expression, decided by
    /// whichever the template's own body parses as. Evaluating it walks
    /// the template, evaluates each splice, and produces a `Code` value.
    Quote(Box<QuoteBody>, Span),
    /// `#(expr)` outside a type position — an expression-position splice
    /// point, only meaningful inside a `quote { .. }` template. `expr` is
    /// evaluated when the enclosing template is spliced, and its value
    /// becomes a literal (or, if it is itself a `Code`, that `Code`'s own
    /// AST is embedded directly).
    Splice(Box<Expr>, Span),
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
    /// `(a, b)`; `()` is the unit value.
    Tuple(Vec<Expr>, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    pub pattern: Pat,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClosureParam {
    pub pattern: Pat,
    pub ty: Option<Ty>,
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
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Not,
    Neg,
    Deref,
    BitNot,
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
        rest: bool,
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
    Generic {
        path: Vec<String>,
        args: Vec<Ty>,
        span: Span,
    },
    Tuple(Vec<Ty>, Span),
    Slice(Box<Ty>, Span),
    Borrow {
        mutable: bool,
        lifetime: Option<String>,
        ty: Box<Ty>,
        span: Span,
    },
    RawPointer {
        mutable: bool,
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
    Const(Box<Expr>, Span),
    DynDim(Span),
    Expand(String, Span),
    /// `?b`: a dimension chosen by the producer, opened by the consumer.
    Existential(String, Span),
    /// `#(expr)` in a type position, only meaningful inside a `quote { ..
    /// }` template (`phase-9-comptime` Decision 7) — `expr` is evaluated
    /// (to a `Value::Type`) when the template is spliced, becoming a
    /// `Ty::Path` in the resulting `Code`.
    Splice(Box<Expr>, Span),
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
            for constant in &decl.consts {
                visitor.visit_expr(&constant.value);
            }
        }
        Item::Enum(decl) => {
            for method in &decl.methods {
                visitor.visit_fn_decl(method);
            }
            for constant in &decl.consts {
                visitor.visit_expr(&constant.value);
            }
        }
        Item::Methods(decl) => {
            for method in &decl.methods {
                visitor.visit_fn_decl(method);
            }
            for constant in &decl.consts {
                visitor.visit_expr(&constant.value);
            }
        }
        Item::Const(constant) => visitor.visit_expr(&constant.value),
        Item::Trait(_) | Item::Use(_) | Item::Extern(_) => {}
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
        Expr::Unsafe(block, _) => visitor.visit_block(block),
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
            for index_expr in index {
                visitor.visit_expr(index_expr);
            }
        }
        Expr::Return(value, _) | Expr::Break(value, _) => {
            if let Some(value) = value {
                visitor.visit_expr(value);
            }
        }
        Expr::Spawn { expr, .. } | Expr::Comptime { expr, .. } | Expr::Splice(expr, _) => {
            visitor.visit_expr(expr)
        }
        Expr::Closure { body, .. } => visitor.visit_expr(body),
        Expr::Quote(body, _) => match body.as_ref() {
            QuoteBody::Item(item) => visitor.visit_item(item),
            QuoteBody::Expr(expr) => visitor.visit_expr(expr),
        },
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
        Expr::Tuple(items, _) => {
            for item in items {
                visitor.visit_expr(item);
            }
        }
        Expr::Try { expr, .. } => visitor.visit_expr(expr),
        Expr::Cast { expr, .. } => visitor.visit_expr(expr),
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
            for constant in &mut decl.consts {
                visitor.visit_expr_mut(&mut constant.value);
            }
        }
        Item::Enum(decl) => {
            for method in &mut decl.methods {
                walk_block_mut(visitor, &mut method.body);
            }
            for constant in &mut decl.consts {
                visitor.visit_expr_mut(&mut constant.value);
            }
        }
        Item::Methods(decl) => {
            for method in &mut decl.methods {
                walk_block_mut(visitor, &mut method.body);
            }
            for constant in &mut decl.consts {
                visitor.visit_expr_mut(&mut constant.value);
            }
        }
        Item::Const(constant) => visitor.visit_expr_mut(&mut constant.value),
        Item::Trait(_) | Item::Use(_) | Item::Extern(_) => {}
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
        Expr::Unsafe(block, _) => walk_block_mut(visitor, block),
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
            for index_expr in index {
                visitor.visit_expr_mut(index_expr);
            }
        }
        Expr::Return(value, _) | Expr::Break(value, _) => {
            if let Some(value) = value {
                visitor.visit_expr_mut(value);
            }
        }
        Expr::Spawn { expr, .. } | Expr::Comptime { expr, .. } | Expr::Splice(expr, _) => {
            visitor.visit_expr_mut(expr)
        }
        Expr::Closure { body, .. } => visitor.visit_expr_mut(body),
        Expr::Quote(body, _) => match body.as_mut() {
            QuoteBody::Item(item) => visitor.visit_item_mut(item),
            QuoteBody::Expr(expr) => visitor.visit_expr_mut(expr),
        },
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
        Expr::Tuple(items, _) => {
            for item in items {
                visitor.visit_expr_mut(item);
            }
        }
        Expr::Try { expr, .. } => visitor.visit_expr_mut(expr),
        Expr::Cast { expr, .. } => visitor.visit_expr_mut(expr),
    }
}

pub fn visit_template_ty_splices(body: &QuoteBody, f: &mut dyn FnMut(&Expr)) {
    struct Finder<'f>(&'f mut dyn FnMut(&Expr));
    impl Finder<'_> {
        fn ty(&mut self, ty: &Ty) {
            match ty {
                Ty::Splice(inner, _) => (self.0)(inner),
                Ty::Generic { args, .. } | Ty::Tuple(args, _) => {
                    for arg in args {
                        self.ty(arg);
                    }
                }
                Ty::Slice(inner, _) | Ty::Borrow { ty: inner, .. } | Ty::RawPointer { ty: inner, .. } => self.ty(inner),
                Ty::Fn { params, return_ty, .. } => {
                    for param in params {
                        self.ty(param);
                    }
                    if let Some(return_ty) = return_ty {
                        self.ty(return_ty);
                    }
                }
                Ty::Path(..) | Ty::Dyn { .. } | Ty::Infer(_) | Ty::Never(_) | Ty::Const(..) | Ty::DynDim(_) | Ty::Expand(..) | Ty::Existential(..) => {}
            }
        }
    }
    impl Visit for Finder<'_> {
        fn visit_item(&mut self, item: &Item) {
            if let Item::Methods(block) = item {
                self.ty(&block.target);
            }
            walk_item(self, item);
        }

        fn visit_fn_decl(&mut self, function: &FnDecl) {
            if let Some(splice) = &function.name_splice {
                (self.0)(splice);
            }
            for param in &function.params {
                self.ty(&param.ty);
            }
            if let Some(return_ty) = &function.return_ty {
                self.ty(return_ty);
            }
            walk_fn_decl(self, function);
        }

        fn visit_stmt(&mut self, statement: &Stmt) {
            if let Stmt::Let(statement) = statement
                && let Some(ty) = &statement.ty
            {
                self.ty(ty);
            }
            walk_stmt(self, statement);
        }

        fn visit_expr(&mut self, expr: &Expr) {
            match expr {
                Expr::Quote(..) | Expr::Splice(..) => return,
                Expr::AssociatedCall { ty, .. } | Expr::StructLiteral { ty, .. } | Expr::Cast { ty, .. } => self.ty(ty),
                Expr::Call { type_args, .. } => {
                    for ty in type_args {
                        self.ty(ty);
                    }
                }
                Expr::Closure { params, .. } => {
                    for ty in params.iter().filter_map(|param| param.ty.as_ref()) {
                        self.ty(ty);
                    }
                }
                _ => {}
            }
            walk_expr(self, expr);
        }
    }
    let mut finder = Finder(f);
    match body {
        QuoteBody::Item(item) => finder.visit_item(item),
        QuoteBody::Expr(expr) => finder.visit_expr(expr),
    }
}
