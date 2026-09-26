//! Minimal name resolution for the core executable subset.

use std::collections::{HashMap, HashSet};

use paco_span::Span;

use paco_diag::{Diagnostic, Reporter};
use paco_syntax::ast::{Block, Expr, FnDecl, GenericParam, Item, LetStmt, Module, Pat, QuoteBody, Stmt, Visit, walk_expr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolveError;

const PRELUDE_VALUES: &[&str] = &[
    "print",
    "panic",
    "Some",
    "None",
    "Ok",
    "Err",
    "channel",
    "spawn_blocking",
    "tcp_listen",
    "slice_of_zeros",
    "hash_of",
    "slice_sort_native",
    "slice_as_ptr",
    "slice_as_mut_ptr",
    // `phase-9-comptime` builtins: `splice_field` is never written by a
    // user directly — the parser desugars `base.#(name)` to
    // `splice_field(base, name)` (Decision 7) — the others are ordinary
    // comptime-only builtins (Decisions 5-6).
    "splice_field",
    "fields_of",
    "type_name",
    "code_to_string",
    // `phase-9-comptime` task 7.1: `derive_display`'s own generated
    // method body (re-entry-checked as part of the *user's* module, task
    // 5.5) calls these ordinary runtime builtins by bare name — like
    // `string_concat` itself, none of them are declared as an `Item::Fn`
    // anywhere in `.paco` source, so without this they'd report a
    // spurious "name not found" the moment any generated (or hand-
    // written) code called them.
    "string_concat",
    "int_to_string",
    "uint_to_string",
    "bool_to_string",
    "float_to_string",
    "char_to_string",
    "string_len_bytes",
    "string_char_at",
    "string_next_char_boundary",
    "string_byte_at",
    "string_slice_utf8",
    "string_to_bytes",
    "string_from_bytes",
    "bytes_write_string",
    "string_hash",
    "fs_read_to_string",
    "stderr_write",
    "arg_count",
    "arg_at",
];

pub fn resolve_module(module: &Module, reporter: &mut Reporter) -> Result<(), ResolveError> {
    resolve_module_with_imports(module, &HashSet::new(), reporter)
}

/// Same as [`resolve_module`], but `imported_names` (qualified or bare
/// function names) are also treated as known.
pub fn resolve_module_with_imports(
    module: &Module,
    imported_names: &HashSet<String>,
    reporter: &mut Reporter,
) -> Result<(), ResolveError> {
    let mut functions = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(function) => Some(function.name.clone()),
            Item::Const(decl) => Some(decl.name.clone()),
            Item::Struct(decl) => Some(decl.name.clone()),
            Item::Enum(decl) => Some(decl.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    for item in &module.items {
        if let Item::Extern(block) = item {
            functions.extend(block.functions.iter().map(|function| function.name.clone()));
        }
    }
    functions.extend(imported_names.iter().cloned());

    let mut locals = Locals::default();
    Resolver { functions: &functions, reporter: Some(reporter), locals: &mut locals, scopes: Vec::new() }.module(module);

    if reporter.has_errors() {
        Err(ResolveError)
    } else {
        Ok(())
    }
}

/// A binding introduced by a `let`, parameter, pattern, closure parameter or
/// `select` arm. Every occurrence of the binding's name that refers to it
/// maps to the same id; shadowing bindings get distinct ids.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LocalId(pub u32);

/// Maps each identifier occurrence (binding site or use, keyed by name and
/// span so it survives AST clones) to the local binding it denotes.
#[derive(Clone, Debug, Default)]
pub struct Locals {
    occurrences: HashMap<Span, Vec<(String, LocalId)>>,
    count: u32,
}

impl Locals {
    pub fn get(&self, name: &str, span: Span) -> Option<LocalId> {
        self.occurrences.get(&span)?.iter().find(|(bound, _)| bound == name).map(|(_, id)| *id)
    }

    /// The local an `Expr::Ident` refers to, if it names one.
    pub fn expr(&self, expr: &Expr) -> Option<LocalId> {
        match expr {
            Expr::Ident(name, span) => self.get(name, *span),
            _ => None,
        }
    }

    /// The local a `Pat::Ident` or `Pat::Binding` introduces.
    pub fn pat(&self, pat: &Pat) -> Option<LocalId> {
        match pat {
            Pat::Ident(name, span) | Pat::Binding { name, span, .. } => self.get(name, *span),
            _ => None,
        }
    }

    /// Resolves the locals of every function body in `module`.
    pub fn add_module(&mut self, module: &Module) {
        Resolver { functions: &HashSet::new(), reporter: None, locals: self, scopes: Vec::new() }.module(module);
    }

    fn intern(&mut self, name: &str, span: Span) -> LocalId {
        if let Some(id) = self.get(name, span) {
            return id;
        }
        let id = LocalId(self.count);
        self.count += 1;
        self.record(name, span, id);
        id
    }

    fn record(&mut self, name: &str, span: Span, id: LocalId) {
        let entries = self.occurrences.entry(span).or_default();
        match entries.iter_mut().find(|(bound, _)| bound == name) {
            Some(entry) => entry.1 = id,
            None => entries.push((name.to_string(), id)),
        }
    }
}

pub fn resolve_locals<'a>(modules: impl IntoIterator<Item = &'a Module>) -> Locals {
    let mut locals = Locals::default();
    for module in modules {
        locals.add_module(module);
    }
    locals
}

type Scope = HashMap<String, Option<LocalId>>;

struct Resolver<'a, 'r> {
    functions: &'a HashSet<String>,
    reporter: Option<&'r mut Reporter>,
    locals: &'a mut Locals,
    scopes: Vec<Scope>,
}

impl Resolver<'_, '_> {
    fn module(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Fn(function) => self.function(function, &[]),
                Item::Struct(decl) => {
                    for method in &decl.methods {
                        self.function(method, &decl.generics);
                    }
                }
                Item::Enum(decl) => {
                    for method in &decl.methods {
                        self.function(method, &decl.generics);
                    }
                }
                Item::Methods(block) => {
                    for method in &block.methods {
                        self.function(method, &block.generics);
                    }
                }
                Item::Trait(_) | Item::Use(_) | Item::Const(_) | Item::Extern(_) => {}
            }
        }
    }

    fn function(&mut self, function: &FnDecl, owner_params: &[GenericParam]) {
        let const_params = owner_params.iter().chain(&function.generics).filter(|param| param.is_const() || param.is_dim());
        self.scopes = vec![const_params.map(|param| (param.name.clone(), None)).collect()];
        for param in &function.params {
            self.bind_pattern(&param.pattern, None, &mut HashMap::new());
        }
        self.block(&function.body);
        self.scopes.clear();
    }

    fn block(&mut self, block: &Block) {
        self.scopes.push(Scope::new());
        for statement in &block.stmts {
            match statement {
                Stmt::Let(statement) => self.let_stmt(statement),
                Stmt::Expr(expr) => self.expr(expr),
                Stmt::Item(_) => {}
            }
        }
        if let Some(tail) = &block.tail {
            self.expr(tail);
        }
        self.scopes.pop();
    }

    fn let_stmt(&mut self, statement: &LetStmt) {
        if let Some(value) = &statement.value {
            self.expr(value);
        }
        self.bind_pattern(&statement.pattern, None, &mut HashMap::new());
    }

    fn ident(&mut self, name: &str, span: Span) {
        match self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            Some(Some(id)) => self.locals.record(name, span, *id),
            Some(None) => {}
            None => {
                if !self.functions.contains(name)
                    && !PRELUDE_VALUES.contains(&name)
                    && let Some(reporter) = self.reporter.as_deref_mut()
                {
                    reporter.push(Diagnostic::error("PACO-E0201", span, format!("name not found `{name}`")));
                }
            }
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Literal(_, _) | Expr::Continue(_) => {}
            Expr::Ident(name, span) => self.ident(name, *span),
            Expr::Block(block) | Expr::Unsafe(block, _) => self.block(block),
            Expr::If { condition, then_branch, else_branch, .. } => {
                self.expr(condition);
                self.block(then_branch);
                if let Some(else_branch) = else_branch {
                    self.expr(else_branch);
                }
            }
            Expr::Loop { body, .. } => self.block(body),
            Expr::While { condition, body, .. } => {
                self.expr(condition);
                self.block(body);
            }
            Expr::Call { callee, args, .. } => {
                self.expr(callee);
                args.iter().for_each(|arg| self.expr(arg));
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.expr(receiver);
                args.iter().for_each(|arg| self.expr(arg));
            }
            Expr::AssociatedCall { args, .. } | Expr::Tuple(args, _) => args.iter().for_each(|arg| self.expr(arg)),
            Expr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Expr::Field { base, .. } => self.expr(base),
            Expr::Index { base, index, .. } => {
                self.expr(base);
                index.iter().for_each(|index| self.expr(index));
            }
            Expr::Return(value, _) | Expr::Break(value, _) => {
                if let Some(value) = value {
                    self.expr(value);
                }
            }
            Expr::Match { scrutinee, arms, .. } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.scopes.push(Scope::new());
                    self.bind_pattern(&arm.pattern, None, &mut HashMap::new());
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    self.expr(&arm.body);
                    self.scopes.pop();
                }
            }
            Expr::Closure { params, body, .. } => {
                self.scopes.push(Scope::new());
                for param in params {
                    self.bind_pattern(&param.pattern, None, &mut HashMap::new());
                }
                self.expr(body);
                self.scopes.pop();
            }
            Expr::Unary { expr, .. }
            | Expr::Spawn { expr, .. }
            | Expr::Comptime { expr, .. }
            | Expr::Splice(expr, _)
            | Expr::Borrow { expr, .. }
            | Expr::Try { expr, .. }
            | Expr::Cast { expr, .. }
            | Expr::Yield(expr, _) => self.expr(expr),
            // A `quote { .. }` template belongs to the generated code's own
            // future scope; only its splice points reference this one.
            Expr::Quote(body, _) => self.quote_splices(body),
            Expr::Select { arms, default, .. } => {
                for arm in arms {
                    self.scopes.push(Scope::new());
                    match &arm.operation {
                        Expr::Assign { target, value, .. } => {
                            self.expr(value);
                            if let Expr::Ident(name, span) = target.as_ref() {
                                self.bind(name, *span, None, &mut HashMap::new());
                            }
                        }
                        other => self.expr(other),
                    }
                    self.block(&arm.body);
                    self.scopes.pop();
                }
                if let Some(default) = default {
                    self.block(default);
                }
            }
            Expr::StructLiteral { fields, .. } => fields.iter().for_each(|(_, value)| self.expr(value)),
        }
    }

    /// Binds every name `pattern` introduces. Each alternative of an
    /// or-pattern after the first reuses the first alternative's ids.
    fn bind_pattern(&mut self, pattern: &Pat, alias: Option<&HashMap<String, LocalId>>, bound: &mut HashMap<String, LocalId>) {
        match pattern {
            Pat::Ident(name, span) => self.bind(name, *span, alias, bound),
            Pat::Binding { name, pattern, span } => {
                self.bind(name, *span, alias, bound);
                self.bind_pattern(pattern, alias, bound);
            }
            Pat::Or(alternatives, _) => {
                let mut first = HashMap::new();
                for (index, alternative) in alternatives.iter().enumerate() {
                    if index == 0 {
                        self.bind_pattern(alternative, alias, &mut first);
                    } else {
                        self.bind_pattern(alternative, Some(&first), &mut HashMap::new());
                    }
                }
                bound.extend(first);
            }
            Pat::Tuple(fields, _) | Pat::Enum { fields, .. } => {
                fields.iter().for_each(|field| self.bind_pattern(field, alias, bound));
            }
            Pat::Struct { fields, .. } => fields.iter().for_each(|(_, field)| self.bind_pattern(field, alias, bound)),
            Pat::Range { start, end, .. } => {
                self.bind_pattern(start, alias, bound);
                self.bind_pattern(end, alias, bound);
            }
            Pat::Wildcard(_) | Pat::Literal(_, _) => {}
        }
    }

    fn bind(&mut self, name: &str, span: Span, alias: Option<&HashMap<String, LocalId>>, bound: &mut HashMap<String, LocalId>) {
        let id = match alias.and_then(|alias| alias.get(name)) {
            Some(id) => {
                self.locals.record(name, span, *id);
                *id
            }
            None => self.locals.intern(name, span),
        };
        bound.insert(name.to_string(), id);
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), Some(id));
        }
    }

    fn quote_splices(&mut self, body: &QuoteBody) {
        struct SpliceResolver<'s, 'a, 'r> {
            resolver: &'s mut Resolver<'a, 'r>,
        }
        impl Visit for SpliceResolver<'_, '_, '_> {
            fn visit_expr(&mut self, expr: &Expr) {
                if let Expr::Splice(inner, _) = expr {
                    self.resolver.expr(inner);
                    return;
                }
                if let Expr::Call { callee, args, .. } = expr
                    && matches!(callee.as_ref(), Expr::Ident(name, _) if name == "splice_field")
                    && let [base, name] = args.as_slice()
                {
                    self.visit_expr(base);
                    self.resolver.expr(name);
                    return;
                }
                if matches!(expr, Expr::Quote(..)) {
                    return;
                }
                walk_expr(self, expr);
            }
        }
        let mut splices = SpliceResolver { resolver: self };
        match body {
            QuoteBody::Item(item) => splices.visit_item(item),
            QuoteBody::Expr(expr) => splices.visit_expr(expr),
        }
        paco_syntax::ast::visit_template_ty_splices(body, &mut |splice| self.expr(splice));
    }
}
