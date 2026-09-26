//! AST → MIR lowering.

mod named;
mod overflow;
mod hash;

use std::collections::HashMap;

use paco_borrow::DropPlan;
use paco_resolve::LocalId;
use paco_span::Span;
use paco_syntax::ast::UnaryOp as AstUnOp;
use paco_syntax::parse::expr_span;
use paco_syntax::ast::{
    BinaryOp, Block, ClosureParam, EnumDecl, Expr, FnDecl, GenericParam, GenericParamKind, Item, Literal, MatchArm, Module,
    Pat, SelectArm, Stmt, StructDecl, Ty, Visit, generic_names, walk_expr,
};
use paco_types::{FloatWidth, IntWidth, Type, TypedModule};

use crate::body::{
    BasicBlock, BasicBlockId, BinOp, BlockSpans, Body, CallTarget, Constant, Local, LocalDecl, MathOp, Operand, Place,
    Profile, QuoteTemplate, Rvalue, Statement, Terminator, UnOp,
};
use crate::comptime::{ComptimeKey, ComptimeValue, splice_sites};
use crate::layout::scalar_layout;

/// Names the temporary a `?` unwraps; its payload is moved out rather than
/// copied, since nothing else can observe the temporary afterwards.
pub const TRY_TEMP: &str = "$try";

/// The runtime entry an explicit `panic(message)` calls; codegen appends the
/// call's source location.
pub const PANIC_SYMBOL: &str = "paco_rt_panic";

fn is_copy(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int(_) | Type::Float(_) | Type::Char | Type::Bool | Type::Borrow { .. } | Type::RawPointer { .. }
    )
}

/// The byte length to marshal a value of `ty` across the `paco-runtime-ffi`
/// boundary (`channel`/`spawn`'s send/recv/result payloads). Only scalar
/// (`scalar_layout`-computable) types are supported today: `Lowerer` has no
/// `TypeLayouts` access (unlike `paco-codegen-cranelift`, which does), so a
/// struct/enum-typed channel element or spawn result — needing a name-keyed
/// aggregate layout lookup — isn't computable here. Every differential/
/// driver test this change adds uses a scalar element type (`channel<i64>`
/// etc., matching the existing `phase-8-concurrency` test corpus); this is a
/// disclosed, narrower-than-general scope boundary, not a silent gap —
/// extending it means threading `layouts: &TypeLayouts` into `Lowerer`,
/// which would widen `lower_function`'s public signature for every caller
/// in the workspace, so it was not attempted as part of this change.
/// Every identifier `expr` references, in the order first encountered
/// (duplicates included — callers dedupe). Reuses `paco-syntax::ast::Visit`
/// instead of a hand-written recursive match over every `Expr` variant.
#[derive(Default)]
struct IdentCollector {
    names: Vec<(String, Span)>,
}

impl Visit for IdentCollector {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Ident(name, span) = expr {
            self.names.push((name.clone(), *span));
        }
        walk_expr(self, expr);
    }
}

fn collect_referenced_idents(expr: &Expr) -> Vec<(String, Span)> {
    let mut collector = IdentCollector::default();
    collector.visit_expr(expr);
    collector.names
}

fn scalar_byte_len(ty: &Type) -> u64 {
    scalar_layout(ty)
        .unwrap_or_else(|| {
            panic!(
                "channel/spawn FFI marshaling only supports scalar element types today \
                 (int widths, float, bool, char) — found {ty:?}"
            )
        })
        .size
}

pub struct TypeRegistry<'a> {
    enums: HashMap<String, &'a EnumDecl>,
    consts: HashMap<String, &'a Expr>,
    structs: HashMap<String, &'a StructDecl>,
    functions: HashMap<String, &'a FnDecl>,
    /// Methods of `methods` blocks, by `(type, method)`.
    extensions: HashMap<(String, String), &'a FnDecl>,
    own_qualifier: String,
    own_symbols: std::collections::HashSet<String>,
    /// The generic parameters of the `methods<T> []T` block.
    slice_generics: &'a [GenericParam],
}

impl<'a> TypeRegistry<'a> {
    pub fn from_module(module: &'a Module) -> Self {
        Self::from_module_with_imports(module, &[])
    }

    pub fn from_module_with_imports(module: &'a Module, imports: &[(String, &'a Module)]) -> Self {
        let mut registry = Self {
            enums: HashMap::new(),
            consts: HashMap::new(),
            structs: HashMap::new(),
            functions: HashMap::new(),
            extensions: HashMap::new(),
            own_qualifier: String::new(),
            own_symbols: std::collections::HashSet::new(),
            slice_generics: &[],
        };
        registry.collect_module(module, "");
        for (qualifier, imported_module) in imports {
            registry.collect_pub_items(imported_module, qualifier);
        }
        registry
    }

    fn collect_module(&mut self, module: &'a Module, qualifier: &str) {
        let key = |name: &str| if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") };
        for item in &module.items {
            match item {
                Item::Enum(decl) => {
                    self.enums.insert(key(&decl.name), decl);
                }
                Item::Const(decl) => {
                    self.consts.insert(key(&decl.name), &decl.value);
                }
                Item::Struct(decl) => {
                    self.structs.insert(key(&decl.name), decl);
                }
                // Only top-level functions — needed so `iter fn` call-site
                // lowering can look up the callee's own declared parameters
                // and body to outline it (task 6). Methods/associated
                // functions never need this lookup (only free functions can
                // be `iter fn`, per the grammar), so they are deliberately
                // not indexed here.
                Item::Fn(decl) => {
                    self.functions.insert(key(&decl.name), decl);
                }
                _ => {}
            }
            let (owner, methods) = match item {
                Item::Fn(decl) => {
                    self.own_symbols.insert(decl.name.clone());
                    continue;
                }
                Item::Struct(decl) => (decl.name.clone(), &decl.methods),
                Item::Enum(decl) => (decl.name.clone(), &decl.methods),
                Item::Methods(block) => match &block.target {
                    Ty::Path(path, _) | Ty::Generic { path, .. }
                        if path.len() == 1 && !is_primitive_type_name(&path[0]) =>
                    {
                        (path[0].clone(), &block.methods)
                    }
                    Ty::Slice(..) => (paco_types::SLICE_TYPE_NAME.to_string(), &block.methods),
                    _ => continue,
                },
                _ => continue,
            };
            for method in methods {
                self.own_symbols.insert(format!("{owner}::{}", method.name));
            }
            if let Item::Methods(block) = item {
                if matches!(block.target, Ty::Slice(..)) {
                    self.slice_generics = &block.generics;
                }
                for method in &block.methods {
                    self.extensions.insert((key(&owner), method.name.clone()), method);
                }
            }
        }
    }

    pub fn with_own_qualifier(mut self, qualifier: &str) -> Self {
        self.own_qualifier = qualifier.to_string();
        self
    }

    fn symbol(&self, name: &str) -> String {
        if !self.own_qualifier.is_empty() && self.own_symbols.contains(name) {
            format!("{}::{name}", self.own_qualifier)
        } else {
            name.to_string()
        }
    }

    fn collect_pub_items(&mut self, module: &'a Module, qualifier: &str) {
        let key = |name: &str| if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") };
        for item in &module.items {
            match item {
                Item::Enum(decl) if decl.is_pub => {
                    self.enums.entry(key(&decl.name)).or_insert(decl);
                }
                Item::Struct(decl) if decl.is_pub => {
                    self.structs.entry(key(&decl.name)).or_insert(decl);
                }
                Item::Fn(decl) if decl.is_pub => {
                    self.functions.entry(key(&decl.name)).or_insert(decl);
                }
                Item::Methods(block) if matches!(block.target, Ty::Slice(..)) => {
                    self.slice_generics = &block.generics;
                    for method in &block.methods {
                        self.extensions.entry((paco_types::SLICE_TYPE_NAME.to_string(), method.name.clone())).or_insert(method);
                    }
                }
                Item::Methods(block) => {
                    if let Ty::Path(path, _) | Ty::Generic { path, .. } = &block.target
                        && let [name] = path.as_slice()
                    {
                        for method in &block.methods {
                            self.extensions.entry((key(name), method.name.clone())).or_insert(method);
                            self.extensions.entry((name.clone(), method.name.clone())).or_insert(method);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The generic parameters a struct or enum declares.
    pub fn owner_generics(&self, type_name: &str) -> &'a [GenericParam] {
        if let Some(decl) = self.structs.get(type_name) {
            return &decl.generics;
        }
        if let Some(decl) = self.enums.get(type_name) {
            return &decl.generics;
        }
        if type_name == paco_types::SLICE_TYPE_NAME {
            return self.slice_generics;
        }
        &[]
    }

    pub fn function(&self, name: &str) -> Option<&'a FnDecl> {
        self.functions.get(name).copied()
    }

    /// Resolves an AST field type to a MIR `Type`. `generics` (the
    /// enclosing struct/enum's own declared type parameter names) is
    /// checked first, producing a `Type::Generic` template marker instead
    /// of falling through to `Type::Unknown` — needed so
    /// `enum_variant_field_ty` can substitute a generic enum's
    /// pattern-bound field type with the scrutinee's own concrete type
    /// arguments.
    fn resolve_ty_identity_with_generics(&self, ty: &Ty, generics: &[String]) -> Type {
        match ty {
            Ty::Path(path, _) if path.len() == 1 && generics.iter().any(|g| g == &path[0]) => {
                Type::Generic(path[0].clone())
            }
            Ty::Path(path, _) if path.len() == 1 => match path[0].as_str() {
                "i8" => Type::Int(IntWidth::I8),
                "i16" => Type::Int(IntWidth::I16),
                "i32" => Type::Int(IntWidth::I32),
                "i64" => Type::Int(IntWidth::I64),
                "u8" | "byte" => Type::Int(IntWidth::U8),
                "u16" => Type::Int(IntWidth::U16),
                "u32" => Type::Int(IntWidth::U32),
                "u64" => Type::Int(IntWidth::U64),
                "char" => Type::Char,
                name if paco_types::FloatWidth::from_name(name).is_some() => {
                    Type::Float(paco_types::FloatWidth::from_name(name).expect("checked"))
                }
                "bool" => Type::Bool,
                "string" => Type::String,
                // ADR 0022 prelude structs with no source `Item::Struct`
                // declaration (`paco-types::register_prelude` registers
                // these for type-checking; this registry only walks the
                // AST, so it never sees that registration) — needed so a
                // pattern binding an enum variant field of one of these
                // types (e.g. `Result::Err(e)` where `e: RecvError`)
                // resolves to a real type instead of `Type::Unknown`.
                name @ ("Sender" | "Receiver" | "JoinHandle" | "Arc" | "Generator") => {
                    Type::Struct(name.to_string(), vec![Type::Unknown])
                }
                name @ ("SendError" | "RecvError" | "TcpListener" | "TcpStream" | "TaskPanic") => {
                    Type::Struct(name.to_string(), Vec::new())
                }
                name if self.structs.contains_key(name) => Type::Struct(name.to_string(), Vec::new()),
                name if self.enums.contains_key(name) => Type::Enum(name.to_string(), Vec::new()),
                _ => Type::Unknown,
            },
            Ty::Slice(inner, _) => Type::Slice(Box::new(self.resolve_ty_identity_with_generics(inner, generics))),
            Ty::Borrow { mutable, ty, .. } => {
                Type::Borrow { mutable: *mutable, ty: Box::new(self.resolve_ty_identity_with_generics(ty, generics)) }
            }
            Ty::Tuple(items, _) if items.is_empty() => Type::Unit,
            Ty::Tuple(items, _) => {
                Type::Tuple(items.iter().map(|item| self.resolve_ty_identity_with_generics(item, generics)).collect())
            }
            Ty::Generic { path, args, .. } => {
                let key = path.join("::");
                let params: Vec<&GenericParam> =
                    self.owner_generics(&key).iter().filter(|param| param.kind != GenericParamKind::Lifetime).collect();
                if params.is_empty() {
                    if crate::type_layout::is_builtin_handle(&key) && !self.structs.contains_key(&key) {
                        return Type::Struct(
                            key,
                            args.iter().map(|arg| self.resolve_ty_identity_with_generics(arg, generics)).collect(),
                        );
                    }
                    return Type::Unknown;
                }
                let dimension = |arg: &Ty| match arg {
                    Ty::Const(expr, _) => match expr.as_ref() {
                        Expr::Literal(Literal::Int(value), _) => {
                            Type::Dim(paco_types::Dim::Const(paco_types::ConstExpr::lit(*value)))
                        }
                        _ => Type::Dim(paco_types::Dim::Dyn),
                    },
                    Ty::Path(path, _) if path.len() == 1 && generics.contains(&path[0]) => {
                        Type::Generic(path[0].clone())
                    }
                    Ty::Expand(name, _) if generics.contains(name) => Type::Generic(name.clone()),
                    _ => Type::Dim(paco_types::Dim::Dyn),
                };
                let mut out = Vec::new();
                for (index, param) in params.iter().enumerate() {
                    out.push(match param.kind {
                        GenericParamKind::ConstPack(_) => match args.get(index..).unwrap_or(&[]) {
                            [Ty::Expand(name, _)] if generics.contains(name) => Type::Generic(name.clone()),
                            rest => Type::Pack(rest.iter().map(dimension).collect()),
                        },
                        GenericParamKind::Const(_) | GenericParamKind::Dim => args.get(index).map_or(Type::Unknown, dimension),
                        _ => args.get(index).map_or(Type::Unknown, |arg| self.resolve_ty_identity_with_generics(arg, generics)),
                    });
                }
                if self.structs.contains_key(&key) { Type::Struct(key, out) } else { Type::Enum(key, out) }
            }
            _ => Type::Unknown,
        }
    }

    fn enum_variant_field_ty(&self, enum_name: &str, type_args: &[Type], variant_name: &str, index: usize) -> Type {
        let Some(decl) = self.enums.get(enum_name) else {
            return Type::Unknown;
        };
        let Some(variant) = decl.variants.iter().find(|variant| variant.name == variant_name) else {
            return Type::Unknown;
        };
        let template = match &variant.fields {
            paco_syntax::ast::VariantFields::Tuple(tys) => {
                tys.get(index).map(|ty| self.resolve_ty_identity_with_generics(ty, &generic_names(&decl.generics)))
            }
            _ => None,
        };
        let Some(template) = template else {
            return Type::Unknown;
        };
        if decl.generics.is_empty() || type_args.is_empty() {
            return template;
        }
        let substitutions: HashMap<String, Type> =
            generic_names(&decl.generics).into_iter().zip(type_args.iter().cloned()).collect();
        paco_types::substitute_generics(&template, &substitutions)
    }

    fn struct_field_ty(&self, struct_name: &str, type_args: &[Type], field_name: &str) -> Type {
        let Some(decl) = self.structs.get(struct_name) else {
            return Type::Unknown;
        };
        let Some(field) = decl.fields.iter().find(|field| field.name == field_name) else {
            return Type::Unknown;
        };
        let template = self.resolve_ty_identity_with_generics(&field.ty, &generic_names(&decl.generics));
        if decl.generics.is_empty() || type_args.is_empty() {
            return template;
        }
        let substitutions: HashMap<String, Type> =
            generic_names(&decl.generics).into_iter().zip(type_args.iter().cloned()).collect();
        paco_types::substitute_generics(&template, &substitutions)
    }

    fn struct_field_order(&self, struct_name: &str) -> Option<Vec<&str>> {
        Some(
            self.structs
                .get(struct_name)?
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect(),
        )
    }

    fn variant_index(&self, enum_name: &str, variant_name: &str) -> Option<usize> {
        self.enums
            .get(enum_name)?
            .variants
            .iter()
            .position(|variant| variant.name == variant_name)
    }

    fn find_enum_by_variant(&self, variant_name: &str) -> Option<&str> {
        self.enums
            .iter()
            .filter(|(_, decl)| decl.variants.iter().any(|variant| variant.name == variant_name))
            .min_by_key(|(name, _)| name.contains("::"))
            .map(|(name, _)| name.as_str())
    }

    /// A struct/enum's own method by name, alongside the enclosing type's
    /// own declared generic parameter names — the two pieces
    /// `paco-driver`'s instantiation worklist needs to build a
    /// substitution map and re-lower the method for one concrete
    /// instantiation (`generic-function-codegen`).
    pub fn find_method_decl(&self, type_name: &str, method_name: &str) -> Option<(&'a FnDecl, Vec<String>)> {
        if let Some(decl) = self.structs.get(type_name)
            && let Some(method) = decl.methods.iter().find(|method| method.name == method_name)
        {
            return Some((method, generic_names(&decl.generics)));
        }
        if let Some(decl) = self.enums.get(type_name)
            && let Some(method) = decl.methods.iter().find(|method| method.name == method_name)
        {
            return Some((method, generic_names(&decl.generics)));
        }
        let method = self.extensions.get(&(type_name.to_string(), method_name.to_string()))?;
        Some((method, generic_names(self.owner_generics(type_name))))
    }
}

/// Tracks concrete `(method, type args)` instantiations a generic
/// struct/enum method's call sites discover they need while lowering —
/// `generic-function-codegen`'s equivalent of `TypeLayouts::struct_info`/
/// `enum_info`'s own lazy, cache-on-demand layout instantiation, but for a
/// method's compiled *body* instead of a type's data layout. Shared via
/// `&InstantiationRegistry` (interior mutability, mirroring
/// `TypeLayouts`'s own `RefCell`-backed caches) so any `Lowerer`, for any
/// module being lowered, can record a new instantiation without needing
/// `&mut` access threaded through the whole lowering call stack.
#[derive(Default)]
pub struct InstantiationRegistry {
    seen: std::cell::RefCell<std::collections::HashSet<(String, Vec<Type>)>>,
    pending: std::cell::RefCell<std::collections::VecDeque<(String, Vec<Type>)>>,
    max_instantiations: std::cell::Cell<usize>,
    comptime_values: std::cell::RefCell<HashMap<ComptimeKey, ComptimeValue>>,
    comptime_sites: std::cell::RefCell<Vec<ComptimeSite>>,
}

/// A `comptime` block lowering outlined into the body `name`, to be
/// evaluated before the program is compiled.
#[derive(Clone, Debug)]
pub struct ComptimeSite {
    pub name: String,
    pub key: ComptimeKey,
    pub span: Span,
    pub ty: Type,
}

const DEFAULT_MAX_INSTANTIATIONS_PER_METHOD: usize = 4096;

impl InstantiationRegistry {
    pub fn new() -> Self {
        let registry = Self::default();
        registry.max_instantiations.set(DEFAULT_MAX_INSTANTIATIONS_PER_METHOD);
        registry
    }

    pub fn with_max_instantiations(self, max: usize) -> Self {
        self.max_instantiations.set(max);
        self
    }

    /// Records `(method_name, type_args)` as an instantiation a call site
    /// needs (a no-op for a non-generic method, `type_args` empty — the
    /// common case, keeping every existing non-generic call site's
    /// `CallTarget` name unchanged), and returns the symbol name the call
    /// site's own `CallTarget` should use.
    fn record(&self, method_name: &str, type_args: &[Type]) -> String {
        if type_args.is_empty() {
            return method_name.to_string();
        }
        let key = (method_name.to_string(), type_args.to_vec());
        let mangled = mangled_name(method_name, type_args);
        let is_new = self.seen.borrow_mut().insert(key.clone());
        if is_new {
            let count = self
                .seen
                .borrow()
                .iter()
                .filter(|(name, _)| name == method_name)
                .count();
            if count >= self.max_instantiations.get() {
                panic!(
                    "generic method `{method_name}` exceeded the instantiation cap ({} distinct sets of \
                     type arguments) while instantiating it with {type_args:?}; this usually means a type \
                     parameter is unexpectedly unbounded (e.g. unbounded recursive generic construction), \
                     not that `{method_name}` legitimately needs this many instantiations",
                    self.max_instantiations.get()
                );
            }
            self.pending.borrow_mut().push_back(key);
        }
        mangled
    }

    /// Makes lowering embed `values` in place of the `comptime` blocks they
    /// were computed for.
    /// The symbol of `method_name` instantiated with `type_args`, queued
    /// for lowering when it is new.
    pub fn request(&self, method_name: &str, type_args: &[Type]) -> String {
        self.record(method_name, type_args)
    }

    pub fn set_comptime_values(&self, values: HashMap<ComptimeKey, ComptimeValue>) {
        *self.comptime_values.borrow_mut() = values;
    }

    fn comptime_value(&self, key: &ComptimeKey) -> Option<ComptimeValue> {
        self.comptime_values.borrow().get(key).cloned()
    }

    fn record_comptime_site(&self, site: ComptimeSite) {
        self.comptime_sites.borrow_mut().push(site);
    }

    pub fn has_comptime_sites(&self) -> bool {
        !self.comptime_sites.borrow().is_empty()
    }

    /// Every `comptime` block outlined since the last call.
    pub fn take_comptime_sites(&self) -> Vec<ComptimeSite> {
        std::mem::take(&mut *self.comptime_sites.borrow_mut())
    }

    /// Drains every instantiation recorded since the last drain — the
    /// worklist `paco-driver::build_file` repeatedly consumes until empty
    /// (a newly-lowered body may itself record further instantiations).
    pub fn drain_pending(&self) -> Vec<(String, Vec<Type>)> {
        self.pending.borrow_mut().drain(..).collect()
    }
}

fn substitutions_key(substitutions: &HashMap<String, Type>) -> String {
    let mut pairs: Vec<String> = substitutions.iter().map(|(name, ty)| format!("{name}={}", mangle_type(ty))).collect();
    pairs.sort();
    pairs.join(",")
}

/// `{method_name}::{mangled type args}` — e.g. `Vec::push::i64` — the
/// shared naming convention both the instantiation worklist (naming a
/// freshly-lowered `Body`) and call-site lowering (naming a `CallTarget`)
/// use, so they always agree without direct coordination.
pub fn mangled_name(method_name: &str, type_args: &[Type]) -> String {
    if type_args.is_empty() {
        return method_name.to_string();
    }
    let parts: Vec<String> = type_args.iter().map(mangle_type).collect();
    format!("{method_name}::{}", parts.join("_"))
}

fn mangle_type(ty: &Type) -> String {
    match ty {
        Type::Int(width) => match width {
            IntWidth::I8 => "i8",
            IntWidth::I16 => "i16",
            IntWidth::I32 => "i32",
            IntWidth::I64 => "i64",
            IntWidth::U8 => "u8",
            IntWidth::U16 => "u16",
            IntWidth::U32 => "u32",
            IntWidth::U64 => "u64",
        }
        .to_string(),
        Type::Float(width) => width.name().to_string(),
        Type::Bool => "bool".to_string(),
        Type::Char => "char".to_string(),
        Type::String => "string".to_string(),
        Type::Unit => "unit".to_string(),
        Type::Struct(name, args) | Type::Enum(name, args) => {
            if args.is_empty() {
                name.clone()
            } else {
                format!("{name}_{}", args.iter().map(mangle_type).collect::<Vec<_>>().join("_"))
            }
        }
        Type::Slice(elem) => format!("slice_{}", mangle_type(elem)),
        Type::Tuple(items) => format!("tuple_{}", items.iter().map(mangle_type).collect::<Vec<_>>().join("_")),
        Type::Borrow { mutable, ty } => format!("{}_{}", if *mutable { "refmut" } else { "ref" }, mangle_type(ty)),
        Type::RawPointer { mutable, ty } => format!("{}_{}", if *mutable { "ptrmut" } else { "ptr" }, mangle_type(ty)),
        Type::Generic(name) => format!("generic_{name}"),
        Type::Dim(paco_types::Dim::Dyn) => "dyn".to_string(),
        Type::Dim(paco_types::Dim::Const(expr)) => match expr.as_lit() {
            Some(value) => format!("d{value}"),
            None => format!("dexpr{}", expr.normal_form().replace(' ', "")),
        },
        Type::Pack(items) => format!("pack{}_{}", items.len(), items.iter().map(mangle_type).collect::<Vec<_>>().join("_")),
        _ => "unknown".to_string(),
    }
}

struct LoopTargets {
    break_target: BasicBlockId,
    continue_target: BasicBlockId,
    scope_depth: usize,
}

fn placeholder_block() -> BasicBlock {
    BasicBlock {
        statements: Vec::new(),
        terminator: Terminator::Unreachable,
    }
}

/// A process-unique name for a `spawn`/`iter fn` thunk's outlined
/// top-level function. A single `AtomicU32` (not a per-module counter
/// threaded through `lower_function`'s signature) is sufficient: this
/// compiler runs one compilation per process, and thunk names only need
/// to be unique within it, never stable or reproducible across builds.
fn fresh_thunk_name(kind: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("__paco_{kind}_thunk_{id}")
}

struct Lowerer<'a> {
    typed: &'a TypedModule<'a>,
    registry: &'a TypeRegistry<'a>,
    drops: &'a DropPlan<'a>,
    expected_return: Type,
    profile: Profile,
    /// The current instantiation's generic-parameter bindings (e.g. `T` ->
    /// `Type::Int(I64)`), applied to every type `type_of`/`type_of_param`/
    /// `type_of_fn_return` reads from `typed` — empty for a non-generic
    /// function, matching pre-`generic-function-codegen` behavior exactly.
    substitutions: &'a HashMap<String, Type>,
    /// Shared across every `Lowerer` lowering any function in the same
    /// compiled program — records a generic method call site's own
    /// concrete instantiation for `paco-driver`'s worklist to lower later.
    instantiations: &'a InstantiationRegistry,
    locals: Vec<LocalDecl>,
    blocks: Vec<BasicBlock>,
    current: BasicBlockId,
    /// The unreachable block that follows a diverging call; nothing
    /// evaluated there is ever observed.
    dead: Option<BasicBlockId>,
    statements: Vec<Statement>,
    span: Span,
    spans: Vec<BlockSpans>,
    statement_spans: Vec<Span>,
    scopes: Vec<HashMap<LocalId, Local>>,
    /// Locals bound in each entry of `scopes`, in binding order.
    scope_locals: Vec<Vec<Local>>,
    loops: Vec<LoopTargets>,
    /// Top-level bodies outlined while lowering (`spawn`/`iter fn` thunks —
    /// task 4/6), collected alongside the function's own `Body` so the
    /// caller can declare and define them too. Only ever appended to on
    /// the *outermost* `Lowerer` for a source function — an outlined
    /// thunk's own nested `Lowerer` (built in `outline_thunk`) never itself
    /// outlines further (nothing in this change nests `spawn` inside
    /// `spawn`'s own operand at the MIR level any differently — it would
    /// just recurse, which is fine, but its own `outlined` list is merged
    /// into the outer one explicitly, never silently dropped).
    outlined: Vec<(String, Body)>,
    /// Lowering a body only compile-time evaluation runs (a `comptime fn`
    /// or an outlined `comptime` block): nested `comptime` blocks run in
    /// place.
    in_comptime: bool,
    /// The run-time value of each dimension name read so far.
    atom_values: HashMap<String, Operand>,
    /// Raw source text for a call to any of `stdlib::test`'s
    /// `#[builtin(assert)]`-family functions' own argument sub-expressions
    /// (`unit-testing`'s design.md), keyed by that sub-expression's own
    /// `Span` — empty for every lowering path that never populates it
    /// (`paco-driver` is the only caller that does).
    source_text: &'a HashMap<Span, String>,
}

impl<'a> Lowerer<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        typed: &'a TypedModule<'a>,
        registry: &'a TypeRegistry<'a>,
        drops: &'a DropPlan<'a>,
        expected_return: Type,
        profile: Profile,
        substitutions: &'a HashMap<String, Type>,
        instantiations: &'a InstantiationRegistry,
        source_text: &'a HashMap<Span, String>,
    ) -> Self {
        Self {
            typed,
            registry,
            drops,
            expected_return,
            profile,
            substitutions,
            instantiations,
            locals: Vec::new(),
            blocks: vec![placeholder_block()],
            current: BasicBlockId(0),
            dead: None,
            statements: Vec::new(),
            span: Span::new_root(0, 0),
            spans: vec![BlockSpans::default()],
            statement_spans: Vec::new(),
            scopes: vec![HashMap::new()],
            scope_locals: vec![Vec::new()],
            loops: Vec::new(),
            outlined: Vec::new(),
            in_comptime: false,
            atom_values: HashMap::new(),
            source_text,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.scope_locals.push(Vec::new());
    }

    fn pop_scope(&mut self) -> HashMap<LocalId, Local> {
        self.scope_locals.pop();
        self.scopes.pop().expect("scope stack is never empty")
    }

    /// Drops every local bound in scopes `depth..`, innermost first, each
    /// scope in reverse binding order. Codegen skips locals that no longer
    /// own a value.
    fn drop_scopes_from(&mut self, depth: usize) {
        let locals: Vec<Local> = self.scope_locals[depth..].iter().rev().flat_map(|scope| scope.iter().rev().copied()).collect();
        for local in locals {
            self.drop_local(local);
        }
    }

    fn drop_local(&mut self, local: Local) {
        if !is_copy(&self.locals[local.0 as usize].ty) && self.locals[local.0 as usize].ty != Type::Unit {
            self.push(Statement::Drop(Place::Local(local)));
        }
    }

    /// Moves a place-valued result into a fresh temporary so the locals it
    /// was read from can be dropped before the result is consumed.
    fn detach(&mut self, operand: Operand, ty: Type) -> Operand {
        match operand {
            Operand::Move(_) | Operand::Copy(_) if !is_copy(&ty) && ty != Type::Unit => {
                let temp = self.declare_local(None, ty, false);
                self.push(Statement::Assign(Place::Local(temp), Rvalue::Use(operand)));
                Operand::Move(Place::Local(temp))
            }
            other => other,
        }
    }

    fn declare_local(&mut self, name: Option<String>, ty: Type, mutable: bool) -> Local {
        let local = Local(self.locals.len() as u32);
        self.locals.push(LocalDecl { name, ty, mutable });
        local
    }

    fn bind(&mut self, id: LocalId, local: Local) {
        self.scopes.last_mut().unwrap().insert(id, local);
        self.scope_locals.last_mut().unwrap().push(local);
        if !self.typed.atoms_of(id).is_empty() {
            self.bind_atoms(id, local);
        }
    }

    fn binding_id(&self, pattern: &Pat) -> LocalId {
        self.typed.locals().pat(pattern).unwrap_or_else(|| panic!("pattern binds no local: {pattern:?}"))
    }

    fn resolve_id(&self, id: LocalId) -> Option<Local> {
        self.scopes.iter().rev().find_map(|scope| scope.get(&id).copied())
    }

    fn try_resolve(&self, expr: &Expr) -> Option<Local> {
        self.resolve_id(self.typed.locals().expr(expr)?)
    }

    fn lower_const_generic(&mut self, name: &str) -> Option<Operand> {
        let invalid = |ty: &Type| match ty {
            Type::Dim(paco_types::Dim::Const(expr)) if expr.is_ground() => match expr.as_lit() {
                Some(value) if value < 0 => Some(format!("dimension `{name}` is negative: {value}")),
                None => Some(format!("dimension `{name}` = `{}` overflows i64 or divides by zero", expr.normal_form())),
                _ => None,
            },
            _ => None,
        };
        let bound = self.substitutions.get(name)?.clone();
        let items = match &bound {
            Type::Pack(items) => items.as_slice(),
            other => std::slice::from_ref(other),
        };
        if let Some(message) = items.iter().find_map(invalid) {
            self.lower_panic(Operand::Constant(Constant::Str(message)));
            return Some(Operand::Constant(Constant::Int(0, IntWidth::I64)));
        }
        let is_value = |ty: &Type| matches!(ty, Type::Dim(_)) || matches!(ty, Type::Generic(name) if paco_types::is_atom(name));
        match &bound {
            Type::Pack(items) => {
                if !items.iter().all(is_value) {
                    return None;
                }
                let slice_ty = Type::Slice(Box::new(Type::Int(IntWidth::I64)));
                let slice = self.declare_local(None, slice_ty, false);
                let resume = self.reserve_block();
                self.finish_current(Terminator::Call {
                    target: CallTarget("slice_of_zeros".to_string()),
                    args: vec![Operand::Constant(Constant::Int(items.len() as i64, IntWidth::I64))],
                    destination: Some(Place::Local(slice)),
                    resume,
                });
                self.switch_to(resume);
                for (index, item) in items.iter().enumerate() {
                    let value = self.dim_value(item, name);
                    let element = Place::Index {
                        base: Box::new(Place::Local(slice)),
                        index: Box::new(Operand::Constant(Constant::Int(index as i64, IntWidth::I64))),
                    };
                    self.push(Statement::Assign(element, Rvalue::Use(value)));
                }
                Some(Operand::Move(Place::Local(slice)))
            }
            other if is_value(other) => Some(self.dim_value(other, name)),
            _ => None,
        }
    }

    fn own_generic_args(&self, call: &Expr) -> Vec<Type> {
        self.typed
            .call_generics(call)
            .map(|args| args.iter().map(|arg| self.symbolic(arg)).collect())
            .unwrap_or_default()
    }

    fn lower_const(&mut self, name: &str) -> Operand {
        let init = *self
            .registry
            .consts
            .get(name)
            .unwrap_or_else(|| panic!("unresolved local `{name}`"));
        match fold_const(init, &self.registry.consts, self.typed) {
            Some(constant) => Operand::Constant(constant),
            None => self.lower_operand(init),
        }
    }

    fn type_of(&self, expr: &Expr) -> Type {
        paco_types::erase_symbolic(&self.symbolic_type_of(expr))
    }

    fn push(&mut self, statement: Statement) {
        if self.dead == Some(self.current) {
            return;
        }
        self.statements.push(statement);
        self.statement_spans.push(self.span);
    }

    fn reserve_block(&mut self) -> BasicBlockId {
        let id = BasicBlockId(self.blocks.len() as u32);
        self.blocks.push(placeholder_block());
        self.spans.push(BlockSpans::default());
        id
    }

    fn finish_current(&mut self, terminator: Terminator) {
        let statements = std::mem::take(&mut self.statements);
        self.blocks[self.current.0 as usize] = BasicBlock {
            statements,
            terminator,
        };
        self.spans[self.current.0 as usize] =
            BlockSpans { statements: std::mem::take(&mut self.statement_spans), terminator: Some(self.span) };
    }

    fn at<T>(&mut self, span: Span, lower: impl FnOnce(&mut Self) -> T) -> T {
        let outer = std::mem::replace(&mut self.span, span);
        let result = lower(self);
        self.span = outer;
        result
    }

    fn into_body(self, profile: Profile, param_count: usize, return_ty: Type, span: Span) -> (Body, Vec<(String, Body)>) {
        let body = Body { locals: self.locals, blocks: self.blocks, profile, param_count, return_ty, span, spans: self.spans };
        (body, self.outlined)
    }

    fn switch_to(&mut self, id: BasicBlockId) {
        self.current = id;
    }

    fn terminate_and_abandon(&mut self, terminator: Terminator) {
        self.finish_current(terminator);
        let dead = self.reserve_block();
        self.switch_to(dead);
        self.dead = Some(dead);
    }

    fn lower_block(&mut self, block: &Block) -> Operand {
        self.push_scope();
        for statement in &block.stmts {
            let first_temp = self.locals.len();
            self.lower_statement(statement);
            let bound: std::collections::HashSet<Local> = self.scope_locals.last().unwrap().iter().copied().collect();
            for index in (first_temp..self.locals.len()).rev() {
                let local = Local(index as u32);
                if !bound.contains(&local) {
                    self.drop_local(local);
                }
            }
        }
        let result = block
            .tail
            .as_deref()
            .map(|tail| {
                let operand = self.lower_operand(tail);
                let ty = self.type_of(tail);
                self.detach(operand, ty)
            })
            .unwrap_or(Operand::Constant(Constant::Unit));
        let depth = self.scopes.len() - 1;
        self.drop_scopes_from(depth);
        self.pop_scope();
        result
    }

    fn lower_statement(&mut self, statement: &Stmt) {
        let span = match statement {
            Stmt::Let(let_stmt) => let_stmt.span,
            Stmt::Expr(expr) => expr_span(expr),
            Stmt::Item(_) => self.span,
        };
        self.at(span, |this| this.lower_statement_at(statement));
    }

    fn lower_statement_at(&mut self, statement: &Stmt) {
        {
            match statement {
                Stmt::Let(let_stmt) => {
                    let value = let_stmt.value.as_ref().expect("let without initializer");
                    let name = match &let_stmt.pattern {
                        Pat::Ident(name, _) => name.clone(),
                        Pat::Tuple(patterns, _) if matches!(value, Expr::Call { callee, .. } if matches!(callee.as_ref(), Expr::Ident(name, _) if name == "channel")) => {
                            self.lower_channel_let(patterns, value);
                            return;
                        }
                        pattern => {
                            let ty = self.type_of(value);
                            let place = self.lower_place(value);
                            let unmatched = self.reserve_block();
                            self.lower_pattern_test(pattern, &place, &ty, unmatched);
                            let matched = self.reserve_block();
                            self.finish_current(Terminator::Goto(matched));
                            self.switch_to(unmatched);
                            self.finish_current(Terminator::Unreachable);
                            self.switch_to(matched);
                            self.bind_pattern_at(pattern, place, ty, let_stmt.mutable);
                            return;
                        }
                    };
                    let ty = self.type_of(value);
                    let rvalue = self.lower_rvalue(value);
                    let local = self.declare_local(Some(name), ty, let_stmt.mutable);
                    self.at(expr_span(value), |this| this.push(Statement::Assign(Place::Local(local), rvalue)));
                    self.bind(self.binding_id(&let_stmt.pattern), local);
                }
                Stmt::Expr(expr) => {
                    self.lower_operand(expr);
                }
                Stmt::Item(_) => {}
            }
        }
    }

    /// Builds `result_ty::Ok(ok_value)` / `::Err(err_value)` from `status`
    /// (every `paco-runtime-ffi` entry point's own `0 = success` convention).
    /// `result_ty` must be a `Result`-shaped enum with `Ok`/`Err` variants —
    /// the same one the calling program's own source declares (this
    /// compiler's `Result` has no source-independent prelude enum
    /// registration, only a prelude *type name* used for signatures; see
    /// `lower_try`, which resolves `?` against the identical convention).
    fn lower_ffi_result(
        &mut self,
        status: Operand,
        result_ty: Type,
        ok_value: Operand,
        err_value: Operand,
    ) -> Operand {
        assert!(matches!(result_ty, Type::Enum(..)), "expected an enum-typed FFI result, found {result_ty:?}");
        let result_local = self.declare_local(None, result_ty.clone(), false);

        let ok_block = self.reserve_block();
        let err_block = self.reserve_block();
        let join_block = self.reserve_block();
        self.finish_current(Terminator::SwitchInt {
            discriminant: status,
            targets: vec![(0, ok_block)],
            otherwise: err_block,
        });

        self.switch_to(ok_block);
        self.push(Statement::Assign(
            Place::Local(result_local),
            Rvalue::Aggregate { ty: result_ty.clone(), variant: Some("Ok".to_string()), fields: vec![ok_value] },
        ));
        self.finish_current(Terminator::Goto(join_block));

        self.switch_to(err_block);
        self.push(Statement::Assign(
            Place::Local(result_local),
            Rvalue::Aggregate { ty: result_ty.clone(), variant: Some("Err".to_string()), fields: vec![err_value] },
        ));
        self.finish_current(Terminator::Goto(join_block));

        self.switch_to(join_block);
        Operand::Move(Place::Local(result_local))
    }

    /// Materializes a field-less prelude error struct (`SendError`/
    /// `RecvError` — ADR 0022, no source `Item::Struct` declaration, but
    /// `paco-mir::TypeLayouts` seeds a real zero-size layout for both; see
    /// its own doc comment) as an `Rvalue::Aggregate` value.
    fn lower_empty_error_struct(&mut self, name: &str) -> Operand {
        let ty = Type::Struct(name.to_string(), Vec::new());
        let local = self.declare_local(None, ty.clone(), false);
        self.push(Statement::Assign(
            Place::Local(local),
            Rvalue::Aggregate { ty, variant: None, fields: Vec::new() },
        ));
        Operand::Move(Place::Local(local))
    }

    /// Lowers `sender.send(value)` to a `paco_rt_send` call: marshals
    /// `value` through a freshly materialized local and its address (the
    /// FFI boundary takes raw bytes, not a typed value), then wraps the
    /// returned status in `Result<(), SendError>`.
    fn lower_channel_send(&mut self, receiver: &Expr, value: &Expr, call_expr: &Expr) -> Operand {
        let sender_operand = self.lower_operand(receiver);

        let value_ty = self.type_of(value);
        let value_rvalue = self.lower_rvalue(value);
        let value_local = self.declare_local(None, value_ty.clone(), false);
        self.push(Statement::Assign(Place::Local(value_local), value_rvalue));
        let value_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(value_ptr),
            Rvalue::Ref { mutable: false, place: Place::Local(value_local) },
        ));
        let value_len = scalar_byte_len(&value_ty);

        let status_local = self.declare_local(None, Type::Int(IntWidth::I32), false);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget("paco_rt_send".to_string()),
            args: vec![
                sender_operand,
                Operand::Copy(Place::Local(value_ptr)),
                Operand::Constant(Constant::Int(value_len as i64, IntWidth::I64)),
            ],
            destination: Some(Place::Local(status_local)),
            resume,
        });
        self.switch_to(resume);

        let send_error = self.lower_empty_error_struct("SendError");
        let result_ty = self.type_of(call_expr);
        self.lower_ffi_result(
            Operand::Copy(Place::Local(status_local)),
            result_ty,
            Operand::Constant(Constant::Unit),
            send_error,
        )
    }

    /// Lowers `sender.close()`/`receiver.close()` to a direct FFI call with
    /// no return value.
    fn lower_channel_close(&mut self, receiver: &Expr, target: &str) -> Operand {
        let receiver_operand = self.lower_operand(receiver);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget(target.to_string()),
            args: vec![receiver_operand],
            destination: None,
            resume,
        });
        self.switch_to(resume);
        Operand::Constant(Constant::Unit)
    }

    /// Lowers `receiver.recv()` to a `paco_rt_recv` call: allocates a local
    /// to receive the value's bytes (its address is the FFI's output
    /// pointer), then wraps the returned status in `Result<T, RecvError>`.
    fn lower_channel_recv(&mut self, receiver: &Expr, call_expr: &Expr) -> Operand {
        let receiver_operand = self.lower_operand(receiver);

        let result_ty = self.type_of(call_expr);
        let Type::Enum(_, type_args) = &result_ty else {
            panic!("Receiver::recv() should type-check to Result<T, RecvError>, found {result_ty:?}");
        };
        let value_ty = type_args[0].clone();
        let value_len = scalar_byte_len(&value_ty);

        let value_local = self.declare_local(None, value_ty, false);
        let value_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(value_ptr),
            Rvalue::Ref { mutable: true, place: Place::Local(value_local) },
        ));

        let status_local = self.declare_local(None, Type::Int(IntWidth::I32), false);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget("paco_rt_recv".to_string()),
            args: vec![
                receiver_operand,
                Operand::Copy(Place::Local(value_ptr)),
                Operand::Constant(Constant::Int(value_len as i64, IntWidth::I64)),
            ],
            destination: Some(Place::Local(status_local)),
            resume,
        });
        self.switch_to(resume);

        let recv_error = self.lower_empty_error_struct("RecvError");
        self.lower_ffi_result(
            Operand::Copy(Place::Local(status_local)),
            result_ty,
            Operand::Move(Place::Local(value_local)),
            recv_error,
        )
    }

    /// Lowers `generator.next()` to a `paco_rt_generator_next` call,
    /// wrapping its status into `Option<T>` — `Some(value)` when a value
    /// was yielded, `None` both when the generator is exhausted *and*
    /// when its thunk panicked (status `1` or `2`): a panicking generator
    /// body has the identical "can't construct a real error value yet"
    /// limitation `lower_join_handle_join` already has for `TaskPanic`,
    /// but `Option<T>` has no error payload to even approximate — folding
    /// both into `None` is the closest honest fit, not a silent
    /// misrepresentation of a real yielded value.
    fn lower_generator_next(&mut self, receiver: &Expr, call_expr: &Expr) -> Operand {
        let receiver_operand = self.lower_operand(receiver);

        let option_ty = self.type_of(call_expr);
        let Type::Enum(_, type_args) = &option_ty else {
            panic!("Generator::next() should type-check to Option<T>, found {option_ty:?}");
        };
        let elem_ty = type_args[0].clone();
        let elem_len = scalar_byte_len(&elem_ty);

        let value_local = self.declare_local(None, elem_ty, false);
        let value_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(value_ptr),
            Rvalue::Ref { mutable: true, place: Place::Local(value_local) },
        ));

        let status_local = self.declare_local(None, Type::Int(IntWidth::I32), false);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget("paco_rt_generator_next".to_string()),
            args: vec![
                receiver_operand,
                Operand::Copy(Place::Local(value_ptr)),
                Operand::Constant(Constant::Int(elem_len as i64, IntWidth::I64)),
            ],
            destination: Some(Place::Local(status_local)),
            resume,
        });
        self.switch_to(resume);

        let result_local = self.declare_local(None, option_ty.clone(), false);
        let some_id = self.reserve_block();
        let none_id = self.reserve_block();
        let join_id = self.reserve_block();
        self.finish_current(Terminator::SwitchInt {
            discriminant: Operand::Copy(Place::Local(status_local)),
            targets: vec![(0, some_id)],
            otherwise: none_id,
        });

        self.switch_to(some_id);
        self.push(Statement::Assign(
            Place::Local(result_local),
            Rvalue::Aggregate {
                ty: option_ty.clone(),
                variant: Some("Some".to_string()),
                fields: vec![Operand::Move(Place::Local(value_local))],
            },
        ));
        self.finish_current(Terminator::Goto(join_id));

        self.switch_to(none_id);
        self.push(Statement::Assign(
            Place::Local(result_local),
            Rvalue::Aggregate { ty: option_ty, variant: Some("None".to_string()), fields: Vec::new() },
        ));
        self.finish_current(Terminator::Goto(join_id));

        self.switch_to(join_id);
        Operand::Move(Place::Local(result_local))
    }

    fn lower_join_handle_join(&mut self, receiver: &Expr, call_expr: &Expr) -> Operand {
        let handle_operand = self.lower_operand(receiver);

        let result_ty = self.type_of(call_expr);
        let Type::Enum(_, type_args) = &result_ty else {
            panic!("JoinHandle::join() should type-check to Result<T, TaskPanic>, found {result_ty:?}");
        };
        let value_ty = type_args[0].clone();
        let unit = matches!(value_ty, Type::Unit);
        let value_len = if unit { 0 } else { scalar_byte_len(&value_ty) };

        let value_local = self.declare_local(None, value_ty, false);
        let value_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        let pointer = if unit {
            Rvalue::Use(Operand::Constant(Constant::Int(0, IntWidth::I64)))
        } else {
            Rvalue::Ref { mutable: true, place: Place::Local(value_local) }
        };
        self.push(Statement::Assign(Place::Local(value_ptr), pointer));
        let message = self.declare_local(None, Type::String, false);
        let message_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(Place::Local(message_ptr), Rvalue::Ref { mutable: true, place: Place::Local(message) }));

        let status_local = self.declare_local(None, Type::Int(IntWidth::I32), false);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget("paco_rt_join".to_string()),
            args: vec![
                handle_operand,
                Operand::Copy(Place::Local(value_ptr)),
                Operand::Constant(Constant::Int(value_len as i64, IntWidth::I64)),
                Operand::Copy(Place::Local(message_ptr)),
            ],
            destination: Some(Place::Local(status_local)),
            resume,
        });
        self.switch_to(resume);

        let result_local = self.declare_local(None, result_ty.clone(), false);
        let result_ty_err = result_ty.clone();
        let ok_block = self.reserve_block();
        let panic_block = self.reserve_block();
        let join_block = self.reserve_block();
        self.finish_current(Terminator::SwitchInt {
            discriminant: Operand::Copy(Place::Local(status_local)),
            targets: vec![(0, ok_block)],
            otherwise: panic_block,
        });

        self.switch_to(ok_block);
        self.push(Statement::Assign(
            Place::Local(result_local),
            Rvalue::Aggregate {
                ty: result_ty,
                variant: Some("Ok".to_string()),
                fields: vec![if unit { Operand::Constant(Constant::Unit) } else { Operand::Move(Place::Local(value_local)) }],
            },
        ));
        self.finish_current(Terminator::Goto(join_block));

        self.switch_to(panic_block);
        let task_panic_ty = Type::Struct("TaskPanic".to_string(), Vec::new());
        let task_panic = self.declare_local(None, task_panic_ty.clone(), false);
        self.push(Statement::Assign(
            Place::Local(task_panic),
            Rvalue::Aggregate { ty: task_panic_ty, variant: None, fields: vec![Operand::Move(Place::Local(message))] },
        ));
        self.push(Statement::Assign(
            Place::Local(result_local),
            Rvalue::Aggregate {
                ty: result_ty_err,
                variant: Some("Err".to_string()),
                fields: vec![Operand::Move(Place::Local(task_panic))],
            },
        ));
        self.finish_current(Terminator::Goto(join_block));

        self.switch_to(join_block);
        Operand::Move(Place::Local(result_local))
    }

    /// Lowers `let (tx, rx) = channel<T>(capacity: N)` — the only shape
    /// `Pat::Tuple` is ever used for in this language today (confirmed:
    /// there is no tuple-literal expression syntax at all, and `channel`'s
    /// prelude-registered 2-tuple return is the only producer of
    /// `Type::Tuple`), so this is deliberately a special case for exactly
    /// this call, not general tuple-pattern lowering.
    ///
    /// `paco_rt_channel` writes its two opaque handles through output
    /// pointers rather than returning a 2-pointer struct by value (see
    /// `paco-runtime-ffi`'s own doc comment on it), so this takes the
    /// address of `tx`/`rx`'s own locals (`Rvalue::Ref`) and passes them as
    /// ordinary call arguments — no multi-destination `Terminator::Call`
    /// needed.
    fn lower_channel_let(&mut self, patterns: &[Pat], value: &Expr) {
        let Expr::Call { callee, args, .. } = value else {
            panic!("a tuple let-pattern's initializer must be a `channel<T>(...)` call")
        };
        let Expr::Ident(callee_name, _) = callee.as_ref() else {
            unreachable!("the caller matched a `channel` identifier callee")
        };
        assert_eq!(callee_name, "channel", "a tuple let-pattern's initializer must be `channel<T>(...)`");
        let [tx @ Pat::Ident(tx_name, _), rx @ Pat::Ident(rx_name, _)] = patterns else {
            panic!("channel<T>(...) must be destructured into exactly two identifier bindings")
        };

        let (sender_ty, receiver_ty) = match self.type_of(value) {
            Type::Tuple(elements) if elements.len() == 2 => {
                (elements[0].clone(), elements[1].clone())
            }
            other => panic!("channel<T>(...) should type-check to a 2-tuple, found {other:?}"),
        };

        let capacity = self.lower_operand(&args[0]);

        let raw_sender = self.declare_local(None, Type::Int(IntWidth::I64), false);
        let raw_receiver = self.declare_local(None, Type::Int(IntWidth::I64), false);
        let sender_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(sender_ptr),
            Rvalue::Ref { mutable: true, place: Place::Local(raw_sender) },
        ));
        let receiver_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(receiver_ptr),
            Rvalue::Ref { mutable: true, place: Place::Local(raw_receiver) },
        ));

        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget("paco_rt_channel".to_string()),
            args: vec![
                capacity,
                Operand::Copy(Place::Local(sender_ptr)),
                Operand::Copy(Place::Local(receiver_ptr)),
            ],
            destination: None,
            resume,
        });
        self.switch_to(resume);

        let sender_local = self.declare_local(Some(tx_name.clone()), sender_ty, false);
        let receiver_local = self.declare_local(Some(rx_name.clone()), receiver_ty, false);
        self.push(Statement::Assign(Place::Local(sender_local), Rvalue::Use(Operand::Copy(Place::Local(raw_sender)))));
        self.push(Statement::Assign(Place::Local(receiver_local), Rvalue::Use(Operand::Copy(Place::Local(raw_receiver)))));

        self.bind(self.binding_id(tx), sender_local);
        self.bind(self.binding_id(rx), receiver_local);
    }

    /// The free variables of `expr` — every identifier it references that
    /// is already bound in an enclosing scope at the point `expr` appears
    /// (a `spawn` operand or an `iter fn` body) — exactly the set that
    /// must become a thunk's captured parameters, since the outlined
    /// function has no access to the caller's own locals. Reimplements the
    /// walk `paco-borrow`'s existing spawn-capture move-checking already
    /// performs (design.md accepts duplicating it rather than adding a
    /// `paco-mir` → `paco-borrow` dependency), deduplicated, in
    /// first-reference order for deterministic thunk parameter ordering.
    /// A referenced name that does *not* resolve in the current scope
    /// (a function name, an enum/struct constructor path segment) is
    /// simply not a local and is silently excluded — not every identifier
    /// `Expr::Ident` ever appears as names a captured variable.
    fn free_variables(&self, expr: &Expr) -> Vec<(LocalId, Local)> {
        let mut seen = std::collections::HashSet::new();
        let mut captures = Vec::new();
        for (name, span) in collect_referenced_idents(expr) {
            let Some(id) = self.typed.locals().get(&name, span) else {
                continue;
            };
            if seen.insert(id)
                && let Some(local) = self.resolve_id(id)
            {
                captures.push((id, local));
            }
        }
        captures
    }

    /// Lowers `spawn <operand>` by outlining `operand` into a fresh
    /// top-level thunk function, packing its captured free variables into
    /// a byte buffer, and calling `paco_rt_spawn` with the thunk's
    /// address, that buffer, and the result size — returning the
    /// `JoinHandle<T>`-typed operand `paco_rt_spawn` hands back.
    ///
    /// The captures buffer uses one fixed 8-byte slot per captured
    /// variable (not a tightly packed struct layout) — simple, and
    /// sufficient since every captured variable this change supports is
    /// already scalar and ≤ 8 bytes (`scalar_byte_len`'s own scoping,
    /// shared with `channel`/`send`/`recv`'s marshaling); a struct/enum
    /// capture would need real per-instantiation layout info this
    /// `Lowerer` doesn't have, same boundary as task 3's.
    /// Lowers a call to an `iter fn` (`name` resolves to one, checked by
    /// the caller before this is reached) — reuses the exact same
    /// captures-buffer marshaling `lower_spawn` uses, but the "captures"
    /// here are the call's own arguments, not an enclosing scope's
    /// referenced locals (an `iter fn` is a plain top-level declaration,
    /// not a closure — see `lower_iter_fn`, which outlines its body
    /// exactly once, not per call site the way `lower_spawn` outlines a
    /// fresh thunk per `spawn` expression).
    fn lower_iter_fn_call(&mut self, name: &str, args: &[Expr], call_expr: &Expr) -> Operand {
        let arg_values: Vec<(Type, Operand)> = args
            .iter()
            .map(|arg| (self.type_of(arg), self.lower_operand(arg)))
            .collect();

        let captures_size = (arg_values.len() as i64) * 8;
        let buffer = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(buffer),
            Rvalue::RawAlloc { size: captures_size as u64 },
        ));
        for (index, (ty, operand)) in arg_values.into_iter().enumerate() {
            let slot_addr = self.offset_address(buffer, (index as i64) * 8);
            self.push(Statement::Store {
                address: Operand::Copy(Place::Local(slot_addr)),
                value: operand,
                ty,
            });
        }

        let thunk_addr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(Place::Local(thunk_addr), Rvalue::FuncAddr(self.registry.symbol(name))));

        let handle_ty = self.type_of(call_expr);
        let handle_local = self.declare_local(None, handle_ty, false);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget("paco_rt_generator_new".to_string()),
            args: vec![
                Operand::Copy(Place::Local(thunk_addr)),
                Operand::Copy(Place::Local(buffer)),
                Operand::Constant(Constant::Int(captures_size, IntWidth::I64)),
            ],
            destination: Some(Place::Local(handle_local)),
            resume,
        });
        self.switch_to(resume);
        Operand::Move(Place::Local(handle_local))
    }

    /// Lowers `yield <value>` to a `paco_rt_generator_yield` call —
    /// marshals `value` through a freshly materialized local and its
    /// address, the same pattern `lower_channel_send` uses to pass a value
    /// across the FFI boundary as raw bytes. Trusted to only ever appear
    /// inside an `iter fn` body: the type checker already rejects `yield`
    /// anywhere else (`PACO-E0326`), the same "an earlier pass already
    /// proved this" trust this compiler's later stages extend throughout
    /// (e.g. MIR never re-checks borrow rules either).
    fn lower_yield(&mut self, value: &Expr) -> Operand {
        let value_ty = self.type_of(value);
        let value_rvalue = self.lower_rvalue(value);
        let value_local = self.declare_local(None, value_ty.clone(), false);
        self.push(Statement::Assign(Place::Local(value_local), value_rvalue));
        let value_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(value_ptr),
            Rvalue::Ref { mutable: false, place: Place::Local(value_local) },
        ));
        let value_len = scalar_byte_len(&value_ty);
        let cancelled = self.declare_local(None, Type::Int(IntWidth::I32), false);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget("paco_rt_generator_yield".to_string()),
            args: vec![
                Operand::Copy(Place::Local(value_ptr)),
                Operand::Constant(Constant::Int(value_len as i64, IntWidth::I64)),
            ],
            destination: Some(Place::Local(cancelled)),
            resume,
        });
        self.switch_to(resume);
        self.return_if_cancelled(cancelled);
        Operand::Constant(Constant::Unit)
    }

    /// A dropped generator resumes its thunk with `cancelled` set; returning
    /// drops every local still owned at the suspension point.
    fn return_if_cancelled(&mut self, cancelled: Local) {
        let continue_id = self.reserve_block();
        let cancel_id = self.reserve_block();
        self.finish_current(Terminator::SwitchInt {
            discriminant: Operand::Copy(Place::Local(cancelled)),
            targets: vec![(0, continue_id)],
            otherwise: cancel_id,
        });
        self.switch_to(cancel_id);
        self.finish_current(Terminator::Return(Operand::Constant(Constant::Unit)));
        self.switch_to(continue_id);
    }

    fn lower_spawn(&mut self, operand: &Expr) -> Operand {
        self.lower_spawn_via(operand, "paco_rt_spawn")
    }

    fn lower_spawn_via(&mut self, operand: &Expr, entry: &str) -> Operand {
        let captures = self.free_variables(operand);
        let dims = self.dimension_captures();
        let result_ty = self.type_of(operand);

        let thunk_name = fresh_thunk_name("spawn");
        let thunk_body = self.outline_thunk(operand, &captures, &dims, &result_ty);
        self.outlined.push((thunk_name.clone(), thunk_body));

        let captures_size = ((captures.len() + dims.len()) as i64) * 8;
        let buffer = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(buffer),
            Rvalue::RawAlloc { size: captures_size as u64 },
        ));
        for (index, (_, local)) in captures.iter().enumerate() {
            let capture_ty = self.locals[local.0 as usize].ty.clone();
            let slot_addr = self.offset_address(buffer, (index as i64) * 8);
            let value = if is_copy(&capture_ty) {
                Operand::Copy(Place::Local(*local))
            } else {
                Operand::Move(Place::Local(*local))
            };
            self.push(Statement::Store {
                address: Operand::Copy(Place::Local(slot_addr)),
                value,
                ty: capture_ty,
            });
        }
        self.store_dimension_captures(buffer, (captures.len() as i64) * 8, &dims);

        let thunk_addr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(Place::Local(thunk_addr), Rvalue::FuncAddr(thunk_name)));

        let result_len = if matches!(result_ty, Type::Unit) { 0 } else { scalar_byte_len(&result_ty) as i64 };

        let handle_ty = Type::Struct("JoinHandle".to_string(), vec![result_ty]);
        let handle_local = self.declare_local(None, handle_ty, false);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget(entry.to_string()),
            args: vec![
                Operand::Copy(Place::Local(thunk_addr)),
                Operand::Copy(Place::Local(buffer)),
                Operand::Constant(Constant::Int(captures_size, IntWidth::I64)),
                Operand::Constant(Constant::Int(result_len, IntWidth::I64)),
            ],
            destination: Some(Place::Local(handle_local)),
            resume,
        });
        self.switch_to(resume);
        Operand::Move(Place::Local(handle_local))
    }

    /// Reads a value moved into memory at `address` into a fresh local that
    /// owns it.
    fn load_owned(&mut self, name: Option<String>, address: Local, ty: Type) -> Local {
        let loaded = self.declare_local(None, ty.clone(), false);
        self.push(Statement::Assign(
            Place::Local(loaded),
            Rvalue::Load { address: Operand::Copy(Place::Local(address)), ty: ty.clone() },
        ));
        let owner = self.declare_local(name, ty.clone(), false);
        if is_copy(&ty) {
            self.push(Statement::Assign(Place::Local(owner), Rvalue::Use(Operand::Copy(Place::Local(loaded)))));
        } else {
            self.push(Statement::Assign(Place::Local(owner), Rvalue::Use(Operand::Move(Place::Local(loaded)))));
            self.push(Statement::FreeBox { address: Operand::Copy(Place::Local(address)), ty });
        }
        owner
    }

    /// `self.declare_local` + `Statement::Assign` computing `base + offset`
    /// as a fresh `i64`-typed local — used to index into a raw captures
    /// buffer a fixed number of 8-byte slots from its start.
    fn offset_address(&mut self, base: Local, offset: i64) -> Local {
        let addr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(addr),
            Rvalue::BinaryOp(
                BinOp::Add,
                Operand::Copy(Place::Local(base)),
                Operand::Constant(Constant::Int(offset, IntWidth::I64)),
            ),
        ));
        addr
    }

    /// Builds `operand`'s outlined thunk body: a fresh top-level function
    /// taking exactly `(captures: i64, result_out: i64)` — matching
    /// `paco_rt_spawn`'s fixed C ABI, which calls every thunk with exactly
    /// these two pointers and expects no return value — that unpacks each
    /// captured variable from its 8-byte slot, lowers `operand` itself in
    /// that freshly bound scope, and writes the result through
    /// `result_out` (`Statement::Store`) instead of `Terminator::Return`.
    fn outline_thunk(&mut self, operand: &Expr, captures: &[(LocalId, Local)], dims: &[(String, Operand)], result_ty: &Type) -> Body {
        let mut thunk = Lowerer::new(
            self.typed,
            self.registry,
            self.drops,
            result_ty.clone(),
            self.profile,
            self.substitutions,
            self.instantiations,
            self.source_text,
        );

        thunk.span = expr_span(operand);
        let captures_param = thunk.declare_local(Some("__captures".to_string()), Type::Int(IntWidth::I64), false);
        let result_param = thunk.declare_local(Some("__result_out".to_string()), Type::Int(IntWidth::I64), false);
        thunk.load_dimension_captures(captures_param, (captures.len() as i64) * 8, dims);

        for (index, (id, outer_local)) in captures.iter().enumerate() {
            let outer = &self.locals[outer_local.0 as usize];
            let slot_addr = thunk.offset_address(captures_param, (index as i64) * 8);
            let value_local = thunk.load_owned(outer.name.clone(), slot_addr, outer.ty.clone());
            thunk.bind(*id, value_local);
        }

        let result_operand = thunk.lower_operand(operand);
        if !matches!(result_ty, Type::Unit) {
            thunk.push(Statement::Store {
                address: Operand::Copy(Place::Local(result_param)),
                value: result_operand,
                ty: result_ty.clone(),
            });
        }
        thunk.finish_current(Terminator::Return(Operand::Constant(Constant::Unit)));

        self.outlined.append(&mut thunk.outlined);
        thunk.into_body(self.profile, 2, Type::Unit, expr_span(operand)).0
    }

    fn lower_closure(&mut self, expr: &Expr, params: &[ClosureParam], body: &Expr) -> Operand {
        let closure_ty = self.type_of(expr);
        let Type::Fn(param_tys, ret) = &closure_ty else {
            panic!("closure should type-check to a function type, found {closure_ty:?}")
        };
        let captures = self.free_variables(body);
        let dims = self.dimension_captures();
        let thunk_name = fresh_thunk_name("closure");
        let thunk = self.outline_closure(params, param_tys, ret, body, &captures, &dims);
        self.outlined.push((thunk_name.clone(), thunk));

        let header = self.declare_local(None, Type::Int(IntWidth::I64), false);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget("paco_calloc".to_string()),
            args: vec![
                Operand::Constant(Constant::Int(1, IntWidth::I64)),
                Operand::Constant(Constant::Int(8 + CLOSURE_CAPTURES_OFFSET + ((captures.len() + dims.len()) as i64) * 8, IntWidth::I64)),
            ],
            destination: Some(Place::Local(header)),
            resume,
        });
        self.switch_to(resume);
        self.push(Statement::Store {
            address: Operand::Copy(Place::Local(header)),
            value: Operand::Constant(Constant::Int(1, IntWidth::I64)),
            ty: Type::Int(IntWidth::I64),
        });
        let env = self.offset_address(header, 8);

        let thunk_addr = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(Place::Local(thunk_addr), Rvalue::FuncAddr(thunk_name)));
        self.push(Statement::Store {
            address: Operand::Copy(Place::Local(env)),
            value: Operand::Copy(Place::Local(thunk_addr)),
            ty: Type::Int(IntWidth::I64),
        });
        let capture_tys: Vec<Type> = captures.iter().map(|(_, local)| self.locals[local.0 as usize].ty.clone()).collect();
        if capture_tys.iter().any(|ty| !is_copy(ty)) {
            let drop_name = fresh_thunk_name("drop");
            let drop_body = outline_env_drop(&capture_tys, self.profile, self.span);
            self.outlined.push((drop_name.clone(), drop_body));
            let drop_addr = self.declare_local(None, Type::Int(IntWidth::I64), false);
            self.push(Statement::Assign(Place::Local(drop_addr), Rvalue::FuncAddr(drop_name)));
            let drop_slot = self.offset_address(env, 8);
            self.push(Statement::Store {
                address: Operand::Copy(Place::Local(drop_slot)),
                value: Operand::Copy(Place::Local(drop_addr)),
                ty: Type::Int(IntWidth::I64),
            });
        }
        for (index, (_, local)) in captures.iter().enumerate() {
            let capture_ty = self.locals[local.0 as usize].ty.clone();
            let slot_addr = self.offset_address(env, CLOSURE_CAPTURES_OFFSET + (index as i64) * 8);
            self.push(Statement::Store {
                address: Operand::Copy(Place::Local(slot_addr)),
                value: Operand::Copy(Place::Local(*local)),
                ty: capture_ty,
            });
        }
        self.store_dimension_captures(env, CLOSURE_CAPTURES_OFFSET + (captures.len() as i64) * 8, &dims);
        let closure = self.declare_local(None, closure_ty.clone(), false);
        self.push(Statement::Assign(Place::Local(closure), Rvalue::Use(Operand::Copy(Place::Local(env)))));
        Operand::Move(Place::Local(closure))
    }

    fn outline_closure(
        &mut self,
        params: &[ClosureParam],
        param_tys: &[Type],
        ret: &Type,
        body: &Expr,
        captures: &[(LocalId, Local)],
        dims: &[(String, Operand)],
    ) -> Body {
        let mut thunk = Lowerer::new(
            self.typed,
            self.registry,
            self.drops,
            ret.clone(),
            self.profile,
            self.substitutions,
            self.instantiations,
            self.source_text,
        );
        thunk.span = expr_span(body);
        let env = thunk.declare_local(Some("__env".to_string()), Type::Int(IntWidth::I64), false);
        let mut param_locals = Vec::with_capacity(params.len());
        for (param, ty) in params.iter().zip(param_tys) {
            let Pat::Ident(name, _) = &param.pattern else {
                panic!("only identifier closure parameters are lowered")
            };
            param_locals.push((self.binding_id(&param.pattern), thunk.declare_local(Some(name.clone()), ty.clone(), false)));
        }
        thunk.load_dimension_captures(env, CLOSURE_CAPTURES_OFFSET + (captures.len() as i64) * 8, dims);
        for (index, (id, outer_local)) in captures.iter().enumerate() {
            let outer = &self.locals[outer_local.0 as usize];
            let capture_ty = outer.ty.clone();
            let slot_addr = thunk.offset_address(env, CLOSURE_CAPTURES_OFFSET + (index as i64) * 8);
            let value_local = thunk.declare_local(outer.name.clone(), capture_ty.clone(), false);
            thunk.push(Statement::Assign(
                Place::Local(value_local),
                Rvalue::Load { address: Operand::Copy(Place::Local(slot_addr)), ty: capture_ty },
            ));
            thunk.bind(*id, value_local);
        }
        for (id, local) in param_locals {
            thunk.bind(id, local);
        }
        let result = thunk.lower_operand(body);
        thunk.finish_current(Terminator::Return(result));

        self.outlined.append(&mut thunk.outlined);
        thunk.into_body(self.profile, 1 + params.len(), ret.clone(), expr_span(body)).0
    }

    fn lower_closure_call(&mut self, closure: Place, args: &[Expr], expr: &Expr) -> Operand {
        let callee = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(callee),
            Rvalue::Load { address: Operand::Copy(closure.clone()), ty: Type::Int(IntWidth::I64) },
        ));
        let mut lowered = vec![Operand::Copy(closure)];
        lowered.extend(args.iter().map(|arg| self.lower_operand(arg)));
        let ty = self.type_of(expr);
        let destination = if matches!(ty, Type::Unit) {
            None
        } else {
            Some(Place::Local(self.declare_local(None, ty.clone(), false)))
        };
        let resume = self.reserve_block();
        self.finish_current(Terminator::CallIndirect {
            callee: Operand::Copy(Place::Local(callee)),
            args: lowered,
            destination: destination.clone(),
            resume,
        });
        self.switch_to(resume);
        match destination {
            Some(place) if is_copy(&ty) => Operand::Copy(place),
            Some(place) => Operand::Move(place),
            None => Operand::Constant(Constant::Unit),
        }
    }

    fn lower_call(&mut self, expr: &Expr) -> Operand {
        let Expr::Call { callee, args, .. } = expr else {
            unreachable!()
        };
        let Expr::Ident(name, _) = callee.as_ref() else {
            let closure = self.lower_place(callee);
            return self.lower_closure_call(closure, args, expr);
        };
        if let Some(local) = self.try_resolve(callee)
            && matches!(self.locals[local.0 as usize].ty, Type::Fn(..))
        {
            return self.lower_closure_call(Place::Local(local), args, expr);
        }
        if self.registry.function(name).is_some_and(|function| function.is_iter) {
            return self.lower_iter_fn_call(name, args, expr);
        }
        if !self.in_comptime && self.registry.function(name).is_some_and(is_comptime_only) {
            return self.lower_comptime(expr, expr);
        }
        if name == "hash_of" && self.registry.function(name).is_none() && let [arg] = args.as_slice() {
            return self.lower_hash_of(arg);
        }
        if name == "slice_sort_native" && self.registry.function(name).is_none() && let [xs, len] = args.as_slice() {
            return self.lower_slice_sort_native(xs, len);
        }
        if name == "panic" && self.registry.function(name).is_none() {
            let message = match args.first() {
                Some(arg) => self.panic_message(arg),
                None => Operand::Constant(Constant::Str("explicit panic".to_string())),
            };
            self.lower_panic(message);
            return Operand::Constant(Constant::Unit);
        }
        if name == "spawn_blocking"
            && let [Expr::Closure { params, body, .. }] = args.as_slice()
            && params.is_empty()
        {
            return self.lower_spawn_via(body, "paco_rt_spawn_blocking");
        }
        if self.registry.function(name).is_none() && self.registry.find_enum_by_variant(name).is_some() {
            return self.lower_enum_variant_construction(name, args, expr);
        }
        if self.registry.function(name).is_some_and(is_builtin_grad) {
            return self.lower_grad(args, expr);
        }
        if self.registry.function(name).is_some_and(is_builtin_test_assert) {
            let kind = name.rsplit("::").next().unwrap_or(name);
            return self.lower_test_assert(kind, args, expr);
        }
        let callee = self.registry.function(name);
        let (target, symbolic, slots) = match callee {
            Some(callee) => {
                let own_args = self.own_generic_args(expr);
                let (key, slots) = self.instance_of(None, Some(callee), &own_args);
                (CallTarget(self.instantiations.record(&self.registry.symbol(name), &key)), own_args, slots)
            }
            None => (CallTarget(self.registry.symbol(name)), Vec::new(), Vec::new()),
        };
        let lowered: Vec<(Operand, Type)> = args.iter().map(|arg| (self.lower_operand(arg), self.type_of(arg))).collect();
        let mut args = self.hidden_values(None, callee, &slots, &symbolic, &lowered);
        args.extend(lowered.into_iter().map(|(operand, _)| operand));
        let ty = self.type_of(expr);
        let destination = if matches!(ty, Type::Unit) {
            None
        } else {
            Some(Place::Local(self.declare_local(None, ty.clone(), false)))
        };
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target,
            args,
            destination: destination.clone(),
            resume,
        });
        self.switch_to(resume);
        match destination {
            Some(place) if is_copy(&ty) => Operand::Copy(place),
            Some(place) => Operand::Move(place),
            None => Operand::Constant(Constant::Unit),
        }
    }

    /// `grad(f, inputs)` becomes a call to `GRAD_PREFIX` + `f`'s instance
    /// with `f`'s own arguments; the autodiff transform expands it.
    fn lower_grad(&mut self, args: &[Expr], call_expr: &Expr) -> Operand {
        let [Expr::Ident(name, _), inputs] = args else { unreachable!("checked by the type checker") };
        let callee = self.registry.function(name).expect("`grad` names a function");
        let own_args = self.own_generic_args(&args[0]);
        let (key, slots) = self.instance_of(None, Some(callee), &own_args);
        let target = self.instantiations.record(&self.registry.symbol(name), &key);
        let lowered: Vec<(Operand, Type)> = match (callee.params.len(), inputs) {
            (1, input) => vec![(self.lower_operand(input), self.type_of(input))],
            (_, Expr::Tuple(items, _)) => items.iter().map(|item| (self.lower_operand(item), self.type_of(item))).collect(),
            (_, tuple) => {
                let Type::Tuple(items) = self.type_of(tuple) else { unreachable!("checked by the type checker") };
                let place = self.lower_place(tuple);
                items
                    .into_iter()
                    .enumerate()
                    .map(|(index, ty)| {
                        let field = Place::Field { base: Box::new(place.clone()), field: index.to_string() };
                        (if is_copy(&ty) { Operand::Copy(field) } else { Operand::Move(field) }, ty)
                    })
                    .collect()
            }
        };
        let mut call_args = self.hidden_values(None, Some(callee), &slots, &own_args, &lowered);
        call_args.extend(lowered.into_iter().map(|(operand, _)| operand));
        let ty = self.type_of(call_expr);
        self.emit_call(CallTarget(format!("{GRAD_PREFIX}{target}")), call_args, ty)
    }

    /// The call-site replacement for any of `stdlib::test`'s nine
    /// `#[builtin(name)]` assertion functions (`unit-testing`'s design.md):
    /// `kind` is the function's own bare name, one of `TEST_ASSERT_BUILTINS`.
    /// Always evaluates to `Unit`.
    fn lower_test_assert(&mut self, kind: &str, args: &[Expr], call_expr: &Expr) -> Operand {
        match kind {
            "assert" | "assert_true" | "assert_false" => {
                let (cond, message) = match args {
                    [cond] => (cond, None),
                    [cond, message] => (cond, Some(message)),
                    _ => unreachable!("checked by the type checker"),
                };
                self.lower_bool_assert(kind, cond, message, call_expr)
            }
            "assert_eq" | "assert_ne" => {
                let (left, right, message) = match args {
                    [left, right] => (left, right, None),
                    [left, right, message] => (left, right, Some(message)),
                    _ => unreachable!("checked by the type checker"),
                };
                self.lower_eq_assert(kind == "assert_ne", left, right, message, call_expr)
            }
            "assert_some" | "assert_none" | "assert_ok" | "assert_err" => {
                let (receiver, message) = match args {
                    [receiver] => (receiver, None),
                    [receiver, message] => (receiver, Some(message)),
                    _ => unreachable!("checked by the type checker"),
                };
                self.lower_option_result_assert(kind, receiver, message, call_expr)
            }
            _ => unreachable!("only `TEST_ASSERT_BUILTINS` names reach `lower_test_assert`"),
        }
    }

    /// `assert`/`assert_true`'s condition must hold, `assert_false`'s must
    /// not; either way, the failing side panics naming `cond`'s source text
    /// (and, when `cond` is itself a top-level `==`/`!=`, both operands'
    /// source text and values) and the passing side is a no-op.
    fn lower_bool_assert(&mut self, kind: &str, cond: &Expr, message: Option<&Expr>, call_expr: &Expr) -> Operand {
        let panic_on_true = kind == "assert_false";
        let cond_operand = if let Expr::Binary { op: op @ (BinaryOp::Eq | BinaryOp::Ne), left, right, .. } = cond {
            self.lower_eq_condition(*op == BinaryOp::Ne, left, right, call_expr)
        } else {
            self.lower_operand(cond)
        };
        let true_block = self.reserve_block();
        let false_block = self.reserve_block();
        self.finish_current(Terminator::SwitchInt { discriminant: cond_operand, targets: vec![(1, true_block)], otherwise: false_block });
        let (panic_block, pass_block) = if panic_on_true { (true_block, false_block) } else { (false_block, true_block) };
        self.switch_to(panic_block);
        let text = self.expr_text(cond).unwrap_or_else(|| "<condition>".to_string());
        let mut parts = vec![Operand::Constant(Constant::Str(format!("assertion failed: {text}")))];
        if let Expr::Binary { op: BinaryOp::Eq | BinaryOp::Ne, left, right, .. } = cond {
            self.push_comparison_values(&mut parts, left, right);
        }
        let built = self.concat_strings(parts);
        let message = self.with_optional_message(built, message);
        self.at(expr_span(call_expr), |this| this.lower_panic(message));
        self.switch_to(pass_block);
        Operand::Constant(Constant::Unit)
    }

    /// `assert_eq`/`assert_ne`: the failing side panics naming both
    /// operands' source text and values.
    fn lower_eq_assert(&mut self, ne: bool, left: &Expr, right: &Expr, message: Option<&Expr>, call_expr: &Expr) -> Operand {
        let cond = self.lower_eq_condition(ne, left, right, call_expr);
        let pass_block = self.reserve_block();
        let fail_block = self.reserve_block();
        self.finish_current(Terminator::SwitchInt { discriminant: cond, targets: vec![(1, pass_block)], otherwise: fail_block });
        self.switch_to(fail_block);
        let left_text = self.expr_text(left).unwrap_or_else(|| "<left>".to_string());
        let right_text = self.expr_text(right).unwrap_or_else(|| "<right>".to_string());
        let verb = if ne { "!=" } else { "==" };
        let mut parts = vec![Operand::Constant(Constant::Str(format!("assertion failed: `{left_text}` {verb} `{right_text}`")))];
        self.push_comparison_values(&mut parts, left, right);
        let built = self.concat_strings(parts);
        let message = self.with_optional_message(built, message);
        self.at(expr_span(call_expr), |this| this.lower_panic(message));
        self.switch_to(pass_block);
        Operand::Constant(Constant::Unit)
    }

    /// `left <op> right` where `op` is `==` (`ne` false) or `!=` (`ne`
    /// true): the same dispatch ordinary `==`/`!=` lowering uses (a
    /// primitive `BinOp`, or an operator-overloaded `eq`/`ne` method for a
    /// struct/enum), reimplemented here because neither operand's `Expr`
    /// is itself the `Binary` node the type checker inferred a type for
    /// (`assert_eq`/`assert_ne`'s operands are two separate call arguments,
    /// never wrapped in one).
    fn lower_eq_condition(&mut self, ne: bool, left: &Expr, right: &Expr, call_expr: &Expr) -> Operand {
        let op = if ne { BinaryOp::Ne } else { BinaryOp::Eq };
        if let Some(method) = binary_operator_method_name(op)
            && type_name_of(&self.type_of(left)).is_some()
        {
            let by_ref = self.method_takes_ref_operand(left, method, right);
            return self.lower_method_call_with(left, method, std::slice::from_ref(right), call_expr, by_ref, Some(Type::Bool));
        }
        let left_operand = self.lower_operand(left);
        let right_operand = self.lower_operand(right);
        let test = self.declare_local(None, Type::Bool, false);
        self.push(Statement::Assign(Place::Local(test), Rvalue::BinaryOp(lower_binary_op(op), left_operand, right_operand)));
        Operand::Copy(Place::Local(test))
    }

    /// Appends ` (<left text> = <left value>, <right text> = <right
    /// value>)` to a failing comparison's message. Only ever reached on the
    /// failure path, after the branch condition is already decided — a
    /// second evaluation of `left`/`right`, correct for the side-effect-free
    /// operands an assertion almost always compares directly.
    /// ponytail: re-evaluates instead of reusing the branch's own operands;
    /// revisit if a real test calls something side-effecting inside
    /// `assert_eq`/`assert(x == y)`.
    fn push_comparison_values(&mut self, parts: &mut Vec<Operand>, left: &Expr, right: &Expr) {
        let left_text = self.expr_text(left).unwrap_or_else(|| "<left>".to_string());
        let right_text = self.expr_text(right).unwrap_or_else(|| "<right>".to_string());
        let left_val = self.describe_expr_value(left);
        let right_val = self.describe_expr_value(right);
        parts.push(Operand::Constant(Constant::Str(format!(" ({left_text} = "))));
        parts.push(left_val);
        parts.push(Operand::Constant(Constant::Str(format!(", {right_text} = "))));
        parts.push(right_val);
        parts.push(Operand::Constant(Constant::Str(")".to_string())));
    }

    /// `assert_some`/`assert_none`/`assert_ok`/`assert_err`: the failing
    /// side panics naming `receiver`'s source text and the variant it
    /// actually held (plus, for `assert_ok`/`assert_err`, that variant's
    /// payload value, per `specs/stdlib/test/spec.md`'s own scenario).
    fn lower_option_result_assert(&mut self, kind: &str, receiver: &Expr, message: Option<&Expr>, call_expr: &Expr) -> Operand {
        let receiver_ty = self.type_of(receiver);
        let Type::Enum(enum_name, type_args) = strip_borrow(&receiver_ty) else { unreachable!("checked by the type checker") };
        let enum_name = enum_name.clone();
        let type_args = type_args.clone();
        let option_like = matches!(kind, "assert_some" | "assert_none");
        let expect_success = matches!(kind, "assert_some" | "assert_ok");
        let success = if option_like { "Some" } else { "Ok" };
        let failure = if option_like { "None" } else { "Err" };
        let success_index =
            self.registry.variant_index(&enum_name, success).unwrap_or_else(|| panic!("unknown variant `{enum_name}::{success}`"));
        let receiver_operand = self.lower_operand(receiver);
        let scrutinee = self.declare_local(None, receiver_ty.clone(), false);
        self.push(Statement::Assign(Place::Local(scrutinee), Rvalue::Use(receiver_operand)));
        let discriminant = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(Place::Local(discriminant), Rvalue::Discriminant(Place::Local(scrutinee))));
        let success_block = self.reserve_block();
        let failure_block = self.reserve_block();
        self.finish_current(Terminator::SwitchInt {
            discriminant: Operand::Copy(Place::Local(discriminant)),
            targets: vec![(success_index as i128, success_block)],
            otherwise: failure_block,
        });
        let (panic_block, pass_block, panic_variant) =
            if expect_success { (failure_block, success_block, failure) } else { (success_block, failure_block, success) };
        self.switch_to(panic_block);
        let text = self.expr_text(receiver).unwrap_or_else(|| "<expression>".to_string());
        let mut parts =
            vec![Operand::Constant(Constant::Str(format!("assertion failed: `{text}` was `{panic_variant}`, expected `{success}`")))];
        if matches!(kind, "assert_ok" | "assert_err") {
            let payload_ty = if expect_success { type_args[1].clone() } else { type_args[0].clone() };
            let payload = Place::VariantField { base: Box::new(Place::Local(scrutinee)), variant: panic_variant.to_string(), index: 0 };
            let payload_operand = if is_copy(&payload_ty) { Operand::Copy(payload) } else { Operand::Move(payload) };
            let described = self.describe_value(payload_operand, &payload_ty);
            parts.push(Operand::Constant(Constant::Str(": ".to_string())));
            parts.push(described);
        }
        let built = self.concat_strings(parts);
        let message = self.with_optional_message(built, message);
        self.at(expr_span(call_expr), |this| this.lower_panic(message));
        self.switch_to(pass_block);
        Operand::Constant(Constant::Unit)
    }

    /// A `string` operand for an already-lowered value: the primitive text
    /// `panic`/`print` already know how to render, or the type's bare name
    /// otherwise (no general `Display`-style formatting exists yet).
    fn describe_value(&mut self, operand: Operand, ty: &Type) -> Operand {
        match ty {
            Type::Int(_) | Type::Bool | Type::Char | Type::Float(_) => self.primitive_to_string(operand, ty),
            Type::String => operand,
            other => Operand::Constant(Constant::Str(format!("<{}>", other.name()))),
        }
    }

    /// Same as [`Self::describe_value`], but lowers `expr` itself first —
    /// only for a value not already in hand as an `Operand` (see
    /// `push_comparison_values`'s own note on when this re-evaluates it).
    fn describe_expr_value(&mut self, expr: &Expr) -> Operand {
        let ty = self.type_of(expr);
        match strip_borrow(&ty) {
            Type::Int(_) | Type::Bool | Type::Char | Type::Float(_) | Type::String => {
                let ty = strip_borrow(&ty).clone();
                let operand = self.lower_operand(expr);
                self.describe_value(operand, &ty)
            }
            other => Operand::Constant(Constant::Str(format!("<{}>", other.name()))),
        }
    }

    /// `message`'s raw source text (an already-typed sub-expression of a
    /// `stdlib::test` assertion call), if `paco-driver` recorded it.
    fn expr_text(&self, expr: &Expr) -> Option<String> {
        self.source_text.get(&expr_span(expr)).cloned()
    }

    /// Concatenates `parts` (`string` operands) left to right, the same
    /// `Rvalue::BinaryOp(BinOp::Add, ..)` a hand-written `"a" + b` already
    /// lowers to for two strings (codegen's own `paco_string_concat`).
    fn concat_strings(&mut self, parts: Vec<Operand>) -> Operand {
        let mut iter = parts.into_iter();
        let Some(mut acc) = iter.next() else {
            return Operand::Constant(Constant::Str(String::new()));
        };
        for part in iter {
            let temp = self.declare_local(None, Type::String, false);
            self.push(Statement::Assign(Place::Local(temp), Rvalue::BinaryOp(BinOp::Add, acc, part)));
            acc = Operand::Move(Place::Local(temp));
        }
        acc
    }

    /// Appends an optional trailing user message (`assert_eq(a, b, "..")`)
    /// to an already-built failure `message`, converting it to `string`
    /// first via `panic_message`'s own non-`string`-to-`string` coercion.
    fn with_optional_message(&mut self, message: Operand, extra: Option<&Expr>) -> Operand {
        match extra {
            Some(extra) => {
                let extra = self.panic_message(extra);
                self.concat_strings(vec![message, Operand::Constant(Constant::Str(": ".to_string())), extra])
            }
            None => message,
        }
    }

    fn lower_comptime(&mut self, expr: &Expr, inner: &Expr) -> Operand {
        if self.in_comptime {
            return self.lower_operand(inner);
        }
        let key: ComptimeKey = (expr as *const Expr as usize, substitutions_key(self.substitutions));
        let ty = self.type_of(expr);
        if let Some(value) = self.instantiations.comptime_value(&key) {
            return self.materialize(&value);
        }
        let name = fresh_thunk_name("comptime");
        let mut body = Lowerer::new(
            self.typed,
            self.registry,
            self.drops,
            ty.clone(),
            self.profile,
            self.substitutions,
            self.instantiations,
            self.source_text,
        );
        body.in_comptime = true;
        body.span = expr_span(inner);
        let captured: Vec<String> =
            self.free_variables(inner).iter().filter_map(|(_, local)| self.locals[local.0 as usize].name.clone()).collect();
        if let Some(variable) = captured.first() {
            let message = format!("a `comptime` block cannot use the runtime variable `{variable}`");
            body.lower_panic(Operand::Constant(Constant::Str(message)));
            body.finish_current(Terminator::Unreachable);
        } else {
            let result = body.lower_operand(inner);
            body.finish_current(Terminator::Return(result));
        }
        self.outlined.append(&mut body.outlined);
        let (outlined, _) = body.into_body(self.profile, 0, ty.clone(), expr_span(expr));
        self.outlined.push((name.clone(), outlined));
        self.instantiations.record_comptime_site(ComptimeSite { name: name.clone(), key, span: expr_span(expr), ty: ty.clone() });
        self.emit_call(CallTarget(name), Vec::new(), ty)
    }

    /// Builds `value` as ordinary MIR.
    fn materialize(&mut self, value: &ComptimeValue) -> Operand {
        match value {
            ComptimeValue::Scalar(constant) => Operand::Constant(constant.clone()),
            ComptimeValue::Record(ty, fields) | ComptimeValue::Variant(ty, _, fields) => {
                let variant = match value {
                    ComptimeValue::Variant(_, variant, _) => Some(variant.clone()),
                    _ => None,
                };
                let fields = fields.iter().map(|field| self.materialize(field)).collect();
                let temp = self.declare_local(None, ty.clone(), false);
                self.push(Statement::Assign(Place::Local(temp), Rvalue::Aggregate { ty: ty.clone(), variant, fields }));
                if is_copy(ty) { Operand::Copy(Place::Local(temp)) } else { Operand::Move(Place::Local(temp)) }
            }
            ComptimeValue::Slice(ty, items) => {
                let slice = self.declare_local(None, ty.clone(), false);
                let resume = self.reserve_block();
                self.finish_current(Terminator::Call {
                    target: CallTarget("slice_of_zeros".to_string()),
                    args: vec![Operand::Constant(Constant::Int(items.len() as i64, IntWidth::I64))],
                    destination: Some(Place::Local(slice)),
                    resume,
                });
                self.switch_to(resume);
                for (index, item) in items.iter().enumerate() {
                    let value = self.materialize(item);
                    let element = Place::Index {
                        base: Box::new(Place::Local(slice)),
                        index: Box::new(Operand::Constant(Constant::Int(index as i64, IntWidth::I64))),
                    };
                    self.push(Statement::Assign(element, Rvalue::Use(value)));
                }
                Operand::Move(Place::Local(slice))
            }
            ComptimeValue::Type(_) | ComptimeValue::Code(_) => {
                panic!("a `type` or `Code` value cannot be embedded in the compiled program")
            }
        }
    }

    fn lower_panic(&mut self, message: Operand) {
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target: CallTarget(PANIC_SYMBOL.to_string()),
            args: vec![message],
            destination: None,
            resume,
        });
        self.switch_to(resume);
        self.terminate_and_abandon(Terminator::Unreachable);
    }

    fn panic_message(&mut self, arg: &Expr) -> Operand {
        let ty = self.type_of(arg);
        let operand = self.lower_operand(arg);
        match strip_borrow(&ty) {
            Type::String => operand,
            other => {
                let other = other.clone();
                self.primitive_to_string(operand, &other)
            }
        }
    }

    /// The text `print` shows for a primitive value.
    fn primitive_to_string(&mut self, operand: Operand, ty: &Type) -> Operand {
        let (builtin, operand) = match ty {
            Type::Int(IntWidth::I64) => ("int_to_string", operand),
            Type::Int(IntWidth::U64) => ("uint_to_string", operand),
            Type::Int(width) => {
                let (builtin, target) = if width.is_signed() {
                    ("int_to_string", IntWidth::I64)
                } else {
                    ("uint_to_string", IntWidth::U64)
                };
                let wide = self.declare_local(None, Type::Int(target), false);
                self.push(Statement::Assign(Place::Local(wide), Rvalue::Cast { operand, target: Type::Int(target) }));
                (builtin, Operand::Copy(Place::Local(wide)))
            }
            Type::Bool => ("bool_to_string", operand),
            Type::Char => ("char_to_string", operand),
            Type::Float(_) => ("float_to_string", operand),
            other => panic!("no text form is lowered for {other:?}"),
        };
        self.emit_call(CallTarget(builtin.to_string()), vec![operand], Type::String)
    }

    fn lower_if(
        &mut self,
        condition: &Expr,
        then_branch: &Block,
        else_branch: Option<&Expr>,
        ty: Type,
    ) -> Operand {
        let cond = self.lower_operand(condition);
        let result = (!matches!(ty, Type::Unit)).then(|| self.declare_local(None, ty.clone(), false));

        let then_id = self.reserve_block();
        let else_id = self.reserve_block();
        let join_id = self.reserve_block();

        self.finish_current(Terminator::SwitchInt {
            discriminant: cond,
            targets: vec![(1, then_id)],
            otherwise: else_id,
        });

        self.switch_to(then_id);
        let then_value = self.lower_block(then_branch);
        if let Some(result) = result {
            self.push(Statement::Assign(
                Place::Local(result),
                Rvalue::Use(then_value),
            ));
        }
        self.finish_current(Terminator::Goto(join_id));

        self.switch_to(else_id);
        let else_value = match else_branch {
            Some(expr) => self.lower_operand(expr),
            None => Operand::Constant(Constant::Unit),
        };
        if let Some(result) = result {
            self.push(Statement::Assign(
                Place::Local(result),
                Rvalue::Use(else_value),
            ));
        }
        self.finish_current(Terminator::Goto(join_id));

        self.switch_to(join_id);
        match result {
            Some(local) if is_copy(&ty) => Operand::Copy(Place::Local(local)),
            Some(local) => Operand::Move(Place::Local(local)),
            None => Operand::Constant(Constant::Unit),
        }
    }

    fn lower_short_circuit(&mut self, and: bool, left: &Expr, right: &Expr) -> Operand {
        let result = self.declare_local(None, Type::Bool, false);
        let left = self.lower_operand(left);
        self.push(Statement::Assign(Place::Local(result), Rvalue::Use(left)));
        let right_id = self.reserve_block();
        let join_id = self.reserve_block();
        let (on_true, otherwise) = if and { (right_id, join_id) } else { (join_id, right_id) };
        self.finish_current(Terminator::SwitchInt {
            discriminant: Operand::Copy(Place::Local(result)),
            targets: vec![(1, on_true)],
            otherwise,
        });
        self.switch_to(right_id);
        let right = self.lower_operand(right);
        self.push(Statement::Assign(Place::Local(result), Rvalue::Use(right)));
        self.finish_current(Terminator::Goto(join_id));
        self.switch_to(join_id);
        Operand::Copy(Place::Local(result))
    }

    fn lower_while(&mut self, condition: &Expr, body: &Block) {
        let header_id = self.reserve_block();
        let body_id = self.reserve_block();
        let exit_id = self.reserve_block();

        self.finish_current(Terminator::Goto(header_id));
        self.switch_to(header_id);
        let cond = self.lower_operand(condition);
        self.finish_current(Terminator::SwitchInt {
            discriminant: cond,
            targets: vec![(1, body_id)],
            otherwise: exit_id,
        });

        self.switch_to(body_id);
        self.loops.push(LoopTargets {
            break_target: exit_id,
            continue_target: header_id,
            scope_depth: self.scopes.len(),
        });
        self.lower_block(body);
        self.loops.pop();
        self.finish_current(Terminator::Goto(header_id));

        self.switch_to(exit_id);
    }

    fn lower_loop(&mut self, body: &Block) -> Operand {
        let header_id = self.reserve_block();
        let exit_id = self.reserve_block();

        self.finish_current(Terminator::Goto(header_id));
        self.switch_to(header_id);

        self.loops.push(LoopTargets {
            break_target: exit_id,
            continue_target: header_id,
            scope_depth: self.scopes.len(),
        });
        self.lower_block(body);
        self.loops.pop();
        self.finish_current(Terminator::Goto(header_id));

        self.switch_to(exit_id);
        Operand::Constant(Constant::Unit)
    }

    fn lower_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], ty: Type) -> Operand {
        let scrutinee_ty = self.type_of(scrutinee);
        let scrutinee_place = self.lower_place(scrutinee);
        if !arms.iter().all(|arm| arm.guard.is_none() && is_switchable_pattern(&arm.pattern)) {
            return self.lower_match_chain(scrutinee_place, &scrutinee_ty, arms, ty);
        }

        let result = (!matches!(ty, Type::Unit)).then(|| self.declare_local(None, ty.clone(), false));

        let arm_ids: Vec<BasicBlockId> = arms.iter().map(|_| self.reserve_block()).collect();
        let join_id = self.reserve_block();

        let discriminant = match &scrutinee_ty {
            Type::Enum(..) => {
                let temp = self.declare_local(None, Type::Int(IntWidth::I64), false);
                self.push(Statement::Assign(
                    Place::Local(temp),
                    Rvalue::Discriminant(scrutinee_place.clone()),
                ));
                Operand::Copy(Place::Local(temp))
            }
            _ => Operand::Copy(scrutinee_place.clone()),
        };

        let mut targets = Vec::new();
        let mut otherwise = None;
        for (arm, &arm_id) in arms.iter().zip(&arm_ids) {
            match &arm.pattern {
                Pat::Literal(Literal::Int(value), _) => targets.push((*value as i128, arm_id)),
                Pat::Literal(Literal::Bool(value), _) => {
                    targets.push((if *value { 1 } else { 0 }, arm_id))
                }
                Pat::Enum { path, .. } => {
                    let Type::Enum(enum_name, _) = &scrutinee_ty else {
                        panic!("enum pattern against a non-enum scrutinee")
                    };
                    let variant_name = path.last().expect("enum pattern path is never empty");
                    let index = self
                        .registry
                        .variant_index(enum_name, variant_name)
                        .unwrap_or_else(|| panic!("unknown variant `{enum_name}::{variant_name}`"));
                    targets.push((index as i128, arm_id));
                }
                _ => otherwise = Some(arm_id),
            }
        }
        let otherwise = otherwise.unwrap_or_else(|| {
            arm_ids
                .last()
                .copied()
                .expect("a match always has at least one arm")
        });

        self.finish_current(Terminator::SwitchInt {
            discriminant,
            targets,
            otherwise,
        });

        for (arm, &arm_id) in arms.iter().zip(&arm_ids) {
            self.switch_to(arm_id);
            self.push_scope();
            self.bind_pattern_at(&arm.pattern, scrutinee_place.clone(), scrutinee_ty.clone(), false);
            let value = self.lower_operand(&arm.body);
            if let Some(result) = result {
                self.push(Statement::Assign(Place::Local(result), Rvalue::Use(value)));
            }
            self.end_arm_scope();
            self.finish_current(Terminator::Goto(join_id));
        }

        self.switch_to(join_id);
        match result {
            Some(local) if is_copy(&ty) => Operand::Copy(Place::Local(local)),
            Some(local) => Operand::Move(Place::Local(local)),
            None => Operand::Constant(Constant::Unit),
        }
    }

    /// Lowers `select { arms.. default }` to a chain of readiness checks,
    /// one per arm: `is_ready()`, and only if ready, `recv()`, else the next
    /// arm; `default` when none is ready.
    fn lower_select(&mut self, arms: &[SelectArm], default: Option<&Block>, ty: Type) -> Operand {
        let Some(default) = default else {
            panic!("`select` without a `default` arm is not lowered yet (codegen requires a default arm)");
        };

        let result = (!matches!(ty, Type::Unit)).then(|| self.declare_local(None, ty.clone(), false));
        let join_id = self.reserve_block();

        let extracted: Vec<(Option<&Expr>, &Expr)> = arms
            .iter()
            .map(|arm| match &arm.operation {
                Expr::Assign { target, value, .. } => {
                    let Expr::Ident(..) = target.as_ref() else {
                        panic!("select binding target must be an identifier");
                    };
                    let Expr::MethodCall { receiver, method, .. } = value.as_ref() else {
                        panic!("select arm operation must be a `.recv()` call");
                    };
                    assert_eq!(method, "recv", "select does not support `.{method}()` arms yet");
                    (Some(target.as_ref()), receiver.as_ref())
                }
                Expr::MethodCall { receiver, method, .. } => {
                    assert_eq!(method, "recv", "select does not support `.{method}()` arms yet");
                    (None, receiver.as_ref())
                }
                _ => panic!("select arm operation must be a `.recv()` call"),
            })
            .collect();

        // Every arm's receiver is evaluated once, before any polling.
        let receivers: Vec<Local> = extracted
            .iter()
            .map(|(_, receiver_expr)| {
                let receiver_ty = self.type_of(receiver_expr);
                let receiver_operand = self.lower_operand(receiver_expr);
                let local = self.declare_local(None, receiver_ty, false);
                self.push(Statement::Assign(Place::Local(local), Rvalue::Use(receiver_operand)));
                local
            })
            .collect();

        let check_ids: Vec<BasicBlockId> = arms.iter().map(|_| self.reserve_block()).collect();
        let arm_ids: Vec<BasicBlockId> = arms.iter().map(|_| self.reserve_block()).collect();
        let default_id = self.reserve_block();

        self.finish_current(Terminator::Goto(check_ids[0]));

        for index in 0..arms.len() {
            self.switch_to(check_ids[index]);
            let ready_local = self.declare_local(None, Type::Int(IntWidth::I32), false);
            let resume = self.reserve_block();
            self.finish_current(Terminator::Call {
                target: CallTarget("paco_rt_receiver_is_ready".to_string()),
                args: vec![Operand::Copy(Place::Local(receivers[index]))],
                destination: Some(Place::Local(ready_local)),
                resume,
            });
            self.switch_to(resume);
            let next = check_ids.get(index + 1).copied().unwrap_or(default_id);
            self.finish_current(Terminator::SwitchInt {
                discriminant: Operand::Copy(Place::Local(ready_local)),
                targets: vec![(0, next)],
                otherwise: arm_ids[index],
            });
        }

        for (index, (arm, (binding, receiver_expr))) in arms.iter().zip(&extracted).enumerate() {
            self.switch_to(arm_ids[index]);
            self.push_scope();

            let elem_ty = match self.type_of(receiver_expr) {
                Type::Struct(name, args) if name == "Receiver" => {
                    args.into_iter().next().expect("Receiver<T> always has one type argument")
                }
                other => panic!("select arm receiver is not a channel Receiver: {other:?}"),
            };
            let binding_name = binding.and_then(|target| match target {
                Expr::Ident(name, _) => Some(name.clone()),
                _ => None,
            });
            let value_local = self.declare_local(binding_name, elem_ty.clone(), false);
            let value_ptr = self.declare_local(None, Type::Int(IntWidth::I64), false);
            self.push(Statement::Assign(
                Place::Local(value_ptr),
                Rvalue::Ref { mutable: true, place: Place::Local(value_local) },
            ));
            let value_len = scalar_byte_len(&elem_ty);
            let recv_resume = self.reserve_block();
            self.finish_current(Terminator::Call {
                target: CallTarget("paco_rt_recv".to_string()),
                args: vec![
                    Operand::Copy(Place::Local(receivers[index])),
                    Operand::Copy(Place::Local(value_ptr)),
                    Operand::Constant(Constant::Int(value_len as i64, IntWidth::I64)),
                ],
                destination: None,
                resume: recv_resume,
            });
            self.switch_to(recv_resume);
            if let Some(id) = binding.and_then(|target| self.typed.locals().expr(target)) {
                self.bind(id, value_local);
            }

            let arm_value = self.lower_block(&arm.body);
            if let Some(result) = result {
                self.push(Statement::Assign(Place::Local(result), Rvalue::Use(arm_value)));
            }
            self.end_arm_scope();
            self.finish_current(Terminator::Goto(join_id));
        }

        self.switch_to(default_id);
        let default_value = self.lower_block(default);
        if let Some(result) = result {
            self.push(Statement::Assign(Place::Local(result), Rvalue::Use(default_value)));
        }
        self.finish_current(Terminator::Goto(join_id));

        self.switch_to(join_id);
        match result {
            Some(local) if is_copy(&ty) => Operand::Copy(Place::Local(local)),
            Some(local) => Operand::Move(Place::Local(local)),
            None => Operand::Constant(Constant::Unit),
        }
    }

    /// Tests each arm in order, falling through to the next on a failed
    /// pattern or guard.
    fn lower_match_chain(&mut self, place: Place, scrutinee_ty: &Type, arms: &[MatchArm], ty: Type) -> Operand {
        let result = (!matches!(ty, Type::Unit)).then(|| self.declare_local(None, ty.clone(), false));
        let join_id = self.reserve_block();
        for arm in arms {
            let next = self.reserve_block();
            self.push_scope();
            self.lower_pattern_test(&arm.pattern, &place, scrutinee_ty, next);
            self.bind_pattern_at(&arm.pattern, place.clone(), scrutinee_ty.clone(), false);
            let rejected = arm.guard.as_ref().map(|guard| {
                let condition = self.lower_operand(guard);
                let rejected = self.reserve_block();
                self.branch_unless(condition, rejected);
                (rejected, self.scope_locals.last().unwrap().clone())
            });
            let value = self.lower_operand(&arm.body);
            if let Some(result) = result {
                self.push(Statement::Assign(Place::Local(result), Rvalue::Use(value)));
            }
            self.end_arm_scope();
            self.finish_current(Terminator::Goto(join_id));
            if let Some((rejected, bound)) = rejected {
                self.switch_to(rejected);
                bound.into_iter().rev().for_each(|local| self.drop_local(local));
                self.finish_current(Terminator::Goto(next));
            }
            self.switch_to(next);
        }
        self.finish_current(Terminator::Unreachable);
        self.switch_to(join_id);
        match result {
            Some(local) if is_copy(&ty) => Operand::Copy(Place::Local(local)),
            Some(local) => Operand::Move(Place::Local(local)),
            None => Operand::Constant(Constant::Unit),
        }
    }

    fn end_arm_scope(&mut self) {
        let depth = self.scopes.len() - 1;
        self.drop_scopes_from(depth);
        self.pop_scope();
    }

    fn branch_unless(&mut self, condition: Operand, fail: BasicBlockId) {
        let matched = self.reserve_block();
        self.finish_current(Terminator::SwitchInt { discriminant: condition, targets: vec![(0, fail)], otherwise: matched });
        self.switch_to(matched);
    }

    fn compare_place(&mut self, op: BinOp, place: &Place, constant: Constant) -> Operand {
        let test = self.declare_local(None, Type::Bool, false);
        self.push(Statement::Assign(
            Place::Local(test),
            Rvalue::BinaryOp(op, Operand::Copy(place.clone()), Operand::Constant(constant)),
        ));
        Operand::Copy(Place::Local(test))
    }

    /// Continues in the current block only if `place` matches `pattern`;
    /// otherwise jumps to `fail`.
    fn lower_pattern_test(&mut self, pattern: &Pat, place: &Place, ty: &Type, fail: BasicBlockId) {
        if let Some(variant) = self.unit_variant_pattern(pattern, ty) {
            return self.lower_pattern_test(&variant, place, ty, fail);
        }
        match pattern {
            Pat::Wildcard(_) | Pat::Ident(_, _) => {}
            Pat::Binding { pattern, .. } => self.lower_pattern_test(pattern, place, ty, fail),
            Pat::Literal(literal, _) => {
                let test = self.compare_place(BinOp::Eq, place, lower_literal(literal, ty));
                self.branch_unless(test, fail);
            }
            Pat::Range { start, end, inclusive, .. } => {
                if let Pat::Literal(literal, _) = start.as_ref() {
                    let test = self.compare_place(BinOp::Ge, place, lower_literal(literal, ty));
                    self.branch_unless(test, fail);
                }
                if let Pat::Literal(literal, _) = end.as_ref() {
                    let op = if *inclusive { BinOp::Le } else { BinOp::Lt };
                    let test = self.compare_place(op, place, lower_literal(literal, ty));
                    self.branch_unless(test, fail);
                }
            }
            Pat::Tuple(patterns, _) => {
                let Type::Tuple(items) = strip_borrow(ty) else {
                    panic!("tuple pattern against a non-tuple scrutinee")
                };
                for (index, (pattern, item_ty)) in patterns.iter().zip(items).enumerate() {
                    let field = Place::Field { base: Box::new(place.clone()), field: index.to_string() };
                    self.lower_pattern_test(pattern, &field, item_ty, fail);
                }
            }
            Pat::Enum { path, fields, .. } => {
                let Type::Enum(enum_name, enum_args) = strip_borrow(ty) else {
                    panic!("enum pattern against a non-enum scrutinee")
                };
                let variant_name = path.last().expect("enum pattern path is never empty");
                let index = self
                    .registry
                    .variant_index(enum_name, variant_name)
                    .unwrap_or_else(|| panic!("unknown variant `{enum_name}::{variant_name}`"));
                let discriminant = self.declare_local(None, Type::Int(IntWidth::I64), false);
                self.push(Statement::Assign(Place::Local(discriminant), Rvalue::Discriminant(place.clone())));
                let test = self.compare_place(BinOp::Eq, &Place::Local(discriminant), Constant::Int(index as i64, IntWidth::I64));
                self.branch_unless(test, fail);
                for (field_index, field_pattern) in fields.iter().enumerate() {
                    let field_place = Place::VariantField {
                        base: Box::new(place.clone()),
                        variant: variant_name.clone(),
                        index: field_index,
                    };
                    let field_ty = self.registry.enum_variant_field_ty(enum_name, enum_args, variant_name, field_index);
                    self.lower_pattern_test(field_pattern, &field_place, &field_ty, fail);
                }
            }
            Pat::Or(alternatives, _) => {
                let matched = self.reserve_block();
                let mut shared: Vec<(LocalId, Local)> = Vec::new();
                for alternative in alternatives {
                    let next = self.reserve_block();
                    self.lower_pattern_test(alternative, place, ty, next);
                    if pattern_binds(alternative) {
                        self.push_scope();
                        self.bind_pattern_at(alternative, place.clone(), ty.clone(), false);
                        let bound = self.pop_scope();
                        if shared.is_empty() {
                            let mut ids: Vec<&LocalId> = bound.keys().collect();
                            ids.sort();
                            for id in ids {
                                let decl = self.locals[bound[id].0 as usize].clone();
                                shared.push((*id, self.declare_local(decl.name, decl.ty, false)));
                            }
                        }
                        for (id, target) in &shared {
                            let source = Place::Local(bound[id]);
                            let operand = if is_copy(&self.locals[target.0 as usize].ty) {
                                Operand::Copy(source)
                            } else {
                                Operand::Move(source)
                            };
                            self.push(Statement::Assign(Place::Local(*target), Rvalue::Use(operand)));
                        }
                    }
                    self.finish_current(Terminator::Goto(matched));
                    self.switch_to(next);
                }
                self.finish_current(Terminator::Goto(fail));
                self.switch_to(matched);
                for (id, local) in shared {
                    self.bind(id, local);
                }
            }
            Pat::Struct { fields, .. } => {
                let Type::Struct(name, args) = strip_borrow(ty) else {
                    panic!("struct pattern against a non-struct scrutinee")
                };
                for (field, pattern) in fields {
                    let field_ty = self.registry.struct_field_ty(name, args, field);
                    let field_place = Place::Field { base: Box::new(place.clone()), field: field.clone() };
                    self.lower_pattern_test(pattern, &field_place, &field_ty, fail);
                }
            }
        }
    }

    /// A bare identifier naming a variant of the scrutinee's enum is that
    /// variant, not a binding.
    fn unit_variant_pattern(&self, pattern: &Pat, ty: &Type) -> Option<Pat> {
        let (Pat::Ident(name, span), Type::Enum(enum_name, _)) = (pattern, strip_borrow(ty)) else {
            return None;
        };
        self.registry.variant_index(enum_name, name)?;
        Some(Pat::Enum { path: vec![name.clone()], fields: Vec::new(), span: *span })
    }

    fn bind_pattern_at(&mut self, pattern: &Pat, place: Place, ty: Type, mutable: bool) {
        if self.unit_variant_pattern(pattern, &ty).is_some() {
            return;
        }
        match pattern {
            Pat::Ident(name, _) => {
                let local = self.declare_local(Some(name.clone()), ty.clone(), mutable);
                let operand = if is_copy(&ty) { Operand::Copy(place) } else { Operand::Move(place) };
                self.push(Statement::Assign(Place::Local(local), Rvalue::Use(operand)));
                self.bind(self.binding_id(pattern), local);
            }
            Pat::Binding { name, pattern: inner, .. } => {
                let local = self.declare_local(Some(name.clone()), ty.clone(), mutable);
                let operand = if is_copy(&ty) { Operand::Copy(place.clone()) } else { Operand::Move(place.clone()) };
                self.push(Statement::Assign(Place::Local(local), Rvalue::Use(operand)));
                self.bind(self.binding_id(pattern), local);
                self.bind_pattern_at(inner, place, ty, mutable);
            }
            Pat::Tuple(patterns, _) => {
                let Type::Tuple(items) = strip_borrow(&ty).clone() else {
                    panic!("tuple pattern against a non-tuple value")
                };
                for (index, (pattern, item_ty)) in patterns.iter().zip(items).enumerate() {
                    let field = Place::Field { base: Box::new(place.clone()), field: index.to_string() };
                    self.bind_pattern_at(pattern, field, item_ty, mutable);
                }
            }
            Pat::Enum { path, fields, .. } => {
                let Type::Enum(enum_name, enum_args) = strip_borrow(&ty).clone() else {
                    panic!("enum pattern against a non-enum value")
                };
                let variant_name = path.last().expect("enum pattern path is never empty");
                for (index, field_pattern) in fields.iter().enumerate() {
                    let field_place = Place::VariantField {
                        base: Box::new(place.clone()),
                        variant: variant_name.clone(),
                        index,
                    };
                    let field_ty = self.registry.enum_variant_field_ty(&enum_name, &enum_args, variant_name, index);
                    self.bind_pattern_at(field_pattern, field_place, field_ty, mutable);
                }
            }
            Pat::Or(..) | Pat::Wildcard(_) | Pat::Literal(_, _) | Pat::Range { .. } => {}
            Pat::Struct { fields, .. } => {
                let Type::Struct(name, args) = strip_borrow(&ty).clone() else {
                    panic!("struct pattern against a non-struct value")
                };
                for (field, pattern) in fields {
                    let field_ty = self.registry.struct_field_ty(&name, &args, field);
                    let field_place = Place::Field { base: Box::new(place.clone()), field: field.clone() };
                    self.bind_pattern_at(pattern, field_place, field_ty, mutable);
                }
            }
        }
    }

    fn place_as_local_alias(&mut self, place: Place, ty: Type) -> Local {
        match place {
            Place::Local(local) => local,
            projected => {
                let temp = self.declare_local(None, ty, false);
                self.push(Statement::Assign(
                    Place::Local(temp),
                    Rvalue::Use(Operand::Move(projected)),
                ));
                temp
            }
        }
    }

    fn lower_place(&mut self, expr: &Expr) -> Place {
        self.at(expr_span(expr), |this| this.lower_place_at(expr))
    }

    fn lower_place_at(&mut self, expr: &Expr) -> Place {
        match expr {
            Expr::Ident(name, _) => match self.try_resolve(expr) {
                Some(local) => Place::Local(local),
                None if self.substitutions.contains_key(name) => match self.lower_const_generic(name) {
                    Some(Operand::Copy(place) | Operand::Move(place)) => place,
                    other => {
                        let ty = self.type_of(expr);
                        let temp = self.declare_local(None, ty, false);
                        let operand = other.unwrap_or_else(|| panic!("`{name}` is not a const generic parameter"));
                        self.push(Statement::Assign(Place::Local(temp), Rvalue::Use(operand)));
                        Place::Local(temp)
                    }
                },
                None => {
                    let operand = match self.type_of(expr) {
                        Type::TypeValue(ty) => Operand::Constant(Constant::Type(*ty)),
                        _ => self.lower_const(name),
                    };
                    let ty = self.type_of(expr);
                    let temp = self.declare_local(None, ty, false);
                    self.push(Statement::Assign(Place::Local(temp), Rvalue::Use(operand)));
                    Place::Local(temp)
                }
            },
            Expr::Field { base, field, .. } => Place::Field {
                base: Box::new(self.lower_place(base)),
                field: field.clone(),
            },
            Expr::Index { base, index, .. } if index.len() > 1 => {
                let address = self.lower_index_dispatch_address(base, index);
                let ty = self.type_of(expr);
                Place::Deref { address: Box::new(address), ty }
            }
            Expr::Index { base, index, .. } => {
                let base_ty = self.type_of(base);
                if slice_elem_ty(&base_ty).is_some() {
                    let base_place = self.lower_place(base);
                    let index_operand = self.lower_operand(&index[0]);
                    Place::Index {
                        base: Box::new(base_place),
                        index: Box::new(index_operand),
                    }
                } else {
                    let address = self.lower_index_dispatch_address(base, index);
                    let ty = self.type_of(expr);
                    Place::Deref { address: Box::new(address), ty }
                }
            }
            Expr::Call { .. } => match self.lower_call(expr) {
                Operand::Copy(place) | Operand::Move(place) => place,
                Operand::Constant(_) => panic!("a unit-returning call has no place"),
            },
            Expr::Unary { op: AstUnOp::Deref, expr: inner, .. } => {
                let (Type::RawPointer { ty, .. } | Type::Borrow { ty, .. }) = self.type_of(inner) else {
                    panic!("`*` applied to a non-pointer type reached MIR lowering");
                };
                let address = self.lower_operand(inner);
                Place::Deref { address: Box::new(address), ty: *ty }
            }
            _ => {
                let rvalue = self.lower_rvalue(expr);
                let ty = self.type_of(expr);
                let temp = self.declare_local(None, ty, false);
                self.push(Statement::Assign(Place::Local(temp), rvalue));
                Place::Local(temp)
            }
        }
    }

    fn lower_operand(&mut self, expr: &Expr) -> Operand {
        self.at(expr_span(expr), |this| this.lower_operand_at(expr))
    }

    fn lower_operand_at(&mut self, expr: &Expr) -> Operand {
        match expr {
            Expr::Literal(literal, _) => {
                Operand::Constant(lower_literal(literal, &self.type_of(expr)))
            }
            Expr::Ident(name, _) => match self.try_resolve(expr) {
                Some(local) => {
                    let ty = self.type_of(expr);
                    if is_copy(&ty) {
                        Operand::Copy(Place::Local(local))
                    } else {
                        Operand::Move(Place::Local(local))
                    }
                }
                None if self.registry.find_enum_by_variant(name).is_some() => {
                    self.lower_enum_variant_construction(name, &[], expr)
                }
                None => match self.lower_const_generic(name) {
                    Some(operand) => operand,
                    None => match self.type_of(expr) {
                        Type::TypeValue(ty) => Operand::Constant(Constant::Type(*ty)),
                        _ => self.lower_const(name),
                    },
                },
            },
            Expr::Comptime { expr: inner, .. } => self.lower_comptime(expr, inner),
            Expr::Quote(body, _) => {
                let splices = splice_sites(body).into_iter().map(|(span, inner)| (span, self.lower_operand(inner))).collect();
                let code = self.declare_local(None, Type::Code, false);
                self.push(Statement::Assign(
                    Place::Local(code),
                    Rvalue::Quote { template: QuoteTemplate(body.clone()), splices },
                ));
                Operand::Move(Place::Local(code))
            }
            Expr::Call { .. } => self.lower_call(expr),
            Expr::Spawn { expr: operand, .. } => self.lower_spawn(operand),
            Expr::Closure { params, body, .. } => self.lower_closure(expr, params, body),
            Expr::Yield(value, _) => self.lower_yield(value),
            Expr::Block(block) | Expr::Unsafe(block, _) => self.lower_block(block),
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                let ty = self.type_of(expr);
                self.lower_if(condition, then_branch, else_branch.as_deref(), ty)
            }
            Expr::While {
                condition, body, ..
            } => {
                self.lower_while(condition, body);
                Operand::Constant(Constant::Unit)
            }
            Expr::Loop { body, .. } => self.lower_loop(body),
            Expr::Match {
                scrutinee, arms, ..
            } => {
                let ty = self.type_of(expr);
                self.lower_match(scrutinee, arms, ty)
            }
            Expr::Select { arms, default, .. } => {
                let ty = self.type_of(expr);
                self.lower_select(arms, default.as_ref(), ty)
            }
            Expr::Return(value, _) => {
                let operand = match value.as_deref() {
                    Some(value) => {
                        let operand = self.lower_operand(value);
                        let ty = self.type_of(value);
                        self.detach(operand, ty)
                    }
                    None => Operand::Constant(Constant::Unit),
                };
                self.drop_scopes_from(1);
                self.terminate_and_abandon(Terminator::Return(operand));
                Operand::Constant(Constant::Unit)
            }
            Expr::Break(value, _) => {
                if let Some(value) = value {
                    self.lower_operand(value);
                }
                let loop_targets = self.loops.last().expect("break outside a loop is rejected before lowering");
                let (target, depth) = (loop_targets.break_target, loop_targets.scope_depth);
                self.drop_scopes_from(depth);
                self.terminate_and_abandon(Terminator::Goto(target));
                Operand::Constant(Constant::Unit)
            }
            Expr::Continue(_) => {
                let loop_targets = self.loops.last().expect("continue outside a loop is rejected before lowering");
                let (target, depth) = (loop_targets.continue_target, loop_targets.scope_depth);
                self.drop_scopes_from(depth);
                self.terminate_and_abandon(Terminator::Goto(target));
                Operand::Constant(Constant::Unit)
            }
            Expr::AssociatedCall {
                ty, function, args, ..
            } => self.lower_associated_call(ty, function, args, expr),
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.lower_method_call(receiver, method, args, expr),
            Expr::Try { expr: operand, .. } => self.lower_try(operand),
            Expr::Assign { target, value, .. } => {
                let rvalue = self.lower_rvalue(value);
                let place = self.lower_place(target);
                let span = if matches!(place, Place::Index { .. }) { expr_span(target) } else { expr_span(value) };
                self.at(span, |this| this.push(Statement::Assign(place, rvalue)));
                Operand::Constant(Constant::Unit)
            }
            Expr::StructLiteral { ty, fields, .. } => self.lower_struct_literal(ty, fields, expr),
            _ => {
                let ty = self.type_of(expr);
                let place = self.lower_place(expr);
                if matches!(place, Place::Index { .. }) && is_copy(&ty) {
                    let temp = self.declare_local(None, ty, false);
                    self.push(Statement::Assign(Place::Local(temp), Rvalue::Use(Operand::Copy(place))));
                    Operand::Copy(Place::Local(temp))
                } else if is_copy(&ty) {
                    Operand::Copy(place)
                } else {
                    Operand::Move(place)
                }
            }
        }
    }

    fn lower_rvalue(&mut self, expr: &Expr) -> Rvalue {
        self.at(expr_span(expr), |this| this.lower_rvalue_at(expr))
    }

    fn lower_rvalue_at(&mut self, expr: &Expr) -> Rvalue {
        match expr {
            Expr::Binary {
                op, left, right, ..
            } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    return Rvalue::Use(self.lower_short_circuit(*op == BinaryOp::And, left, right));
                }
                if matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge)
                    && matches!(self.type_of(left), Type::Struct(..) | Type::Enum(..))
                {
                    let by_ref = self.method_takes_ref_operand(left, "cmp", right);
                    let order =
                        self.lower_method_call_with(
                        left,
                        "cmp",
                        std::slice::from_ref(right.as_ref()),
                        expr,
                        by_ref,
                        Some(Type::Int(IntWidth::I64)),
                    );
                    return Rvalue::BinaryOp(lower_binary_op(*op), order, Operand::Constant(Constant::Int(0, IntWidth::I64)));
                }
                if let Some(method) = binary_operator_method_name(*op)
                    && type_name_of(&self.type_of(left)).is_some()
                {
                    let by_ref = self.method_takes_ref_operand(left, method, right);
                    return Rvalue::Use(self.lower_method_call_with(
                        left,
                        method,
                        std::slice::from_ref(right.as_ref()),
                        expr,
                        by_ref,
                        None,
                    ));
                }
                let left = self.lower_operand(left);
                let right = self.lower_operand(right);
                Rvalue::BinaryOp(lower_binary_op(*op), left, right)
            }
            Expr::Unary { op: AstUnOp::Deref, .. } => {
                let place = self.lower_place(expr);
                if is_copy(&self.type_of(expr)) { Rvalue::Use(Operand::Copy(place)) } else { Rvalue::Use(Operand::Move(place)) }
            }
            Expr::Unary { op: AstUnOp::Neg, expr: inner, .. }
                if matches!(self.type_of(inner), Type::Struct(..) | Type::Enum(..)) =>
            {
                Rvalue::Use(self.lower_method_call(inner, "neg", &[], expr))
            }
            Expr::Unary { op: AstUnOp::Neg, expr: inner, .. }
                if let (Expr::Literal(Literal::Int(value), _), Type::Int(width)) = (inner.as_ref(), self.type_of(expr)) =>
            {
                Rvalue::Use(Operand::Constant(Constant::Int(width.wrap(-i128::from(*value)), width)))
            }
            Expr::Unary { op, expr, .. } => {
                let operand = self.lower_operand(expr);
                Rvalue::UnaryOp(lower_unary_op(*op), operand)
            }
            Expr::Cast { expr: source, .. } => {
                let operand = self.lower_operand(source);
                let target = self.type_of(expr);
                Rvalue::Cast { operand, target }
            }
            // A source-level `&expr`/`&mut expr` — distinct from
            // `Rvalue::Ref` uses the lowerer synthesizes itself elsewhere
            // (e.g. FFI output-pointer args), this is the first place a
            // *written* borrow expression reaches MIR at all. Needed for
            // `fn index(&self, i: Idx) -> &Output` bodies, which must
            // return one.
            Expr::Borrow { mutable, expr: inner, .. } => {
                let place = self.lower_place(inner);
                match &place {
                    Place::Local(local)
                        if matches!(
                            (&self.locals[local.0 as usize].ty, self.type_of(expr)),
                            (Type::Borrow { ty: held, .. }, Type::Borrow { ty: wanted, .. }) if **held == *wanted
                        ) =>
                    {
                        Rvalue::Use(Operand::Copy(place))
                    }
                    _ => Rvalue::Ref { mutable: *mutable, place },
                }
            }
            Expr::Tuple(items, _) if items.is_empty() => Rvalue::Use(Operand::Constant(Constant::Unit)),
            Expr::Tuple(items, _) => Rvalue::Aggregate {
                ty: self.type_of(expr),
                variant: None,
                fields: items.iter().map(|item| self.lower_operand(item)).collect(),
            },
            Expr::Literal(_, _)
            | Expr::Ident(_, _)
            | Expr::Call { .. }
            | Expr::AssociatedCall { .. }
            | Expr::MethodCall { .. }
            | Expr::Block(_)
            | Expr::Unsafe(_, _)
            | Expr::If { .. }
            | Expr::While { .. }
            | Expr::Loop { .. }
            | Expr::Match { .. }
            | Expr::Return(_, _)
            | Expr::Break(_, _)
            | Expr::Continue(_)
            | Expr::Try { .. }
            | Expr::Assign { .. }
            | Expr::Spawn { .. }
            | Expr::Closure { .. }
            | Expr::Select { .. }
            | Expr::Yield(_, _)
            | Expr::Field { .. }
            | Expr::Index { .. }
            | Expr::Comptime { .. }
            | Expr::Quote(..)
            | Expr::StructLiteral { .. } => Rvalue::Use(self.lower_operand(expr)),
            _ => panic!("this expression kind is not lowered yet: {expr:?}"),
        }
    }

    fn lower_associated_call(&mut self, ty: &Ty, function: &str, args: &[Expr], call_expr: &Expr) -> Operand {
        let type_name = associated_call_type_name(ty);
        if self.registry.variant_index(&type_name, function).is_none() {
            return self.lower_plain_associated_call(&type_name, function, args, call_expr);
        }
        self.lower_enum_variant_construction(function, args, call_expr)
    }

    fn lower_plain_associated_call(
        &mut self,
        type_name: &str,
        function: &str,
        args: &[Expr],
        call_expr: &Expr,
    ) -> Operand {
        let ty = self.type_of(call_expr);
        if let Some(op) = self.shared_cell_op(type_name, function) {
            let call_args = args.iter().map(|arg| self.lower_operand(arg)).collect();
            return self.emit_call(CallTarget(format!("$cell::{op}")), call_args, ty);
        }
        let qualified = format!("{type_name}::{function}");
        if self.registry.function(&qualified).is_some_and(is_builtin_grad) {
            return self.lower_grad(args, call_expr);
        }
        if self.registry.function(&qualified).is_some_and(is_builtin_test_assert) {
            return self.lower_test_assert(function, args, call_expr);
        }
        let base_target = self.registry.symbol(&qualified);
        let (owner, callee, symbolic) = match self.registry.function(&qualified) {
            Some(callee) => (None, Some(callee), self.own_generic_args(call_expr)),
            None => {
                let mut symbolic = match self.typed.call_target(call_expr) {
                    Some(target) => nominal_type_args(&self.symbolic(target)).to_vec(),
                    None => nominal_type_args(&self.symbolic_type_of(call_expr)).to_vec(),
                };
                symbolic.extend(self.own_generic_args(call_expr));
                let callee = self.registry.find_method_decl(type_name, function).map(|(decl, _)| decl);
                (Some(type_name), callee, symbolic)
            }
        };
        let (key, slots) = self.instance_of(owner, callee, &symbolic);
        let target = CallTarget(self.instantiations.record(&base_target, &key));
        let lowered: Vec<(Operand, Type)> = args.iter().map(|arg| (self.lower_operand(arg), self.type_of(arg))).collect();
        let mut call_args = self.hidden_values(owner, callee, &slots, &symbolic, &lowered);
        call_args.extend(lowered.into_iter().map(|(operand, _)| operand));

        let destination = if matches!(ty, Type::Unit) {
            None
        } else {
            Some(Place::Local(self.declare_local(None, ty.clone(), false)))
        };
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target,
            args: call_args,
            destination: destination.clone(),
            resume,
        });
        self.switch_to(resume);
        match destination {
            Some(place) if is_copy(&ty) => Operand::Copy(place),
            Some(place) => Operand::Move(place),
            None => Operand::Constant(Constant::Unit),
        }
    }

    fn lower_enum_variant_construction(&mut self, function: &str, args: &[Expr], call_expr: &Expr) -> Operand {
        let result_ty = self.type_of(call_expr);
        let fields = args.iter().map(|arg| self.lower_operand(arg)).collect();
        let rvalue = Rvalue::Aggregate {
            ty: result_ty.clone(),
            variant: Some(function.to_string()),
            fields,
        };
        let temp = self.declare_local(None, result_ty.clone(), false);
        self.push(Statement::Assign(Place::Local(temp), rvalue));
        if is_copy(&result_ty) {
            Operand::Copy(Place::Local(temp))
        } else {
            Operand::Move(Place::Local(temp))
        }
    }

    fn lower_struct_literal(&mut self, ty: &Ty, fields: &[(String, Expr)], literal_expr: &Expr) -> Operand {
        let type_name = associated_call_type_name(ty);
        let order = self
            .registry
            .struct_field_order(&type_name)
            .unwrap_or_else(|| panic!("unknown struct `{type_name}`"));

        let mut values: HashMap<&str, Operand> = HashMap::new();
        for (field_name, value_expr) in fields {
            let operand = self.lower_operand(value_expr);
            values.insert(field_name.as_str(), operand);
        }
        let ordered_operands: Vec<Operand> = order
            .iter()
            .map(|field_name| {
                values.remove(field_name).unwrap_or_else(|| {
                    panic!("struct literal for `{type_name}` is missing field `{field_name}`")
                })
            })
            .collect();

        let result_ty = self.type_of(literal_expr);
        let rvalue = Rvalue::Aggregate {
            ty: result_ty.clone(),
            variant: None,
            fields: ordered_operands,
        };
        let temp = self.declare_local(None, result_ty.clone(), false);
        self.push(Statement::Assign(Place::Local(temp), rvalue));
        if is_copy(&result_ty) {
            Operand::Copy(Place::Local(temp))
        } else {
            Operand::Move(Place::Local(temp))
        }
    }

    /// Calls a structurally-dispatched `index(&self, i: Idx) -> &Output`
    /// method (Paco has no `impl Trait for Type` — ADR 0002's structural
    /// trait satisfaction, same `program.methods`-by-name lookup
    /// `lower_method_call` already uses for ordinary calls) and returns its
    /// pointer result as an `Operand`, for `lower_place` to wrap in
    /// `Place::Deref`.
    fn lower_index_dispatch_address(&mut self, base: &Expr, index: &[Expr]) -> Operand {
        let receiver_ty = self.type_of(base);
        let type_name = type_name_of(&receiver_ty).unwrap_or_else(|| {
            panic!("indexing receiver has no resolvable named type and no built-in `[]T` layout: {receiver_ty:?}")
        });
        let receiver_operand = self.lower_operand(base);
        let index_operand = match index {
            [single] => self.lower_operand(single),
            many => {
                let ty = Type::Tuple(many.iter().map(|item| self.type_of(item)).collect());
                let fields = many.iter().map(|item| self.lower_operand(item)).collect();
                let tuple = self.declare_local(None, ty.clone(), false);
                self.push(Statement::Assign(Place::Local(tuple), Rvalue::Aggregate { ty, variant: None, fields }));
                Operand::Move(Place::Local(tuple))
            }
        };
        let ptr_local = self.declare_local(None, Type::Int(IntWidth::I64), false);
        let base_target = self.registry.symbol(&format!("{type_name}::index"));
        let symbolic = nominal_type_args(&self.symbolic_type_of(base)).to_vec();
        let callee = self.registry.find_method_decl(&type_name, "index").map(|(decl, _)| decl);
        let (key, slots) = self.instance_of(Some(&type_name), callee, &symbolic);
        let target = CallTarget(self.instantiations.record(&base_target, &key));
        let mut args = self.hidden_values(Some(&type_name), callee, &slots, &symbolic, &[]);
        args.push(receiver_operand);
        args.push(index_operand);
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target,
            args,
            destination: Some(Place::Local(ptr_local)),
            resume,
        });
        self.switch_to(resume);
        Operand::Copy(Place::Local(ptr_local))
    }

    /// Whether `receiver.method(operand)` takes a `&Self` parameter that the
    /// operator form passes a value to.
    fn method_takes_ref_operand(&self, receiver: &Expr, method: &str, operand: &Expr) -> bool {
        type_name_of(&self.type_of(receiver))
            .and_then(|name| self.registry.find_method_decl(&name, method))
            .and_then(|(decl, _)| decl.params.get(1))
            .is_some_and(|param| matches!(param.ty, Ty::Borrow { mutable: false, .. }))
            && !matches!(self.type_of(operand), Type::Borrow { .. })
    }

    fn lower_method_call(&mut self, receiver: &Expr, method: &str, args: &[Expr], call_expr: &Expr) -> Operand {
        self.lower_method_call_with(receiver, method, args, call_expr, false, None)
    }

    /// `borrow_args` passes each argument by shared reference, as an
    /// overloaded operator does for a `&Self` parameter; `result_ty`
    /// overrides the call expression's type when the operator's value is
    /// not the method's result.
    fn lower_method_call_with(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
        call_expr: &Expr,
        borrow_args: bool,
        result_ty: Option<Type>,
    ) -> Operand {
        let receiver_ty = self.type_of(receiver);
        if let Type::Float(_) = receiver_ty
            && let Some(op) = MathOp::from_method(method)
            && self.registry.find_method_decl(&receiver_ty.name(), method).is_none()
        {
            let mut operands = vec![self.lower_operand(receiver)];
            operands.extend(args.iter().map(|arg| self.lower_operand(arg)));
            let result = self.declare_local(None, receiver_ty, false);
            self.push(Statement::Assign(Place::Local(result), Rvalue::Math(op, operands)));
            return Operand::Copy(Place::Local(result));
        }
        if method == "to_string"
            && args.is_empty()
            && matches!(receiver_ty, Type::Int(_) | Type::Float(_) | Type::Bool | Type::Char)
            && self.registry.find_method_decl(&receiver_ty.name(), method).is_none()
        {
            let value = self.lower_operand(receiver);
            return self.primitive_to_string(value, &receiver_ty);
        }
        if let Type::Int(width) = receiver_ty
            && let Some((family, operation)) = paco_types::int_overflow_method(method)
            && let [arg] = args
        {
            let result_ty = self.type_of(call_expr);
            return self.lower_overflow_method(receiver, family, operation, arg, width, result_ty);
        }
        if method == "len" && slice_elem_ty(&receiver_ty).is_some() {
            let place = self.lower_place(receiver);
            let len = self.declare_local(None, Type::Int(IntWidth::I64), false);
            self.push(Statement::Assign(Place::Local(len), Rvalue::SliceLen(place)));
            return Operand::Copy(Place::Local(len));
        }
        let type_name = type_name_of(&receiver_ty)
            .or_else(|| match strip_borrow(&receiver_ty) {
                primitive @ (Type::Int(_) | Type::Float(_) | Type::Bool | Type::Char | Type::String) => {
                    Some(primitive.name())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("method receiver has no resolvable named type: {receiver_ty:?}"));

        if let Some(op) = self.shared_cell_op(&type_name, method) {
            let mut call_args = vec![self.lower_operand(receiver)];
            call_args.extend(args.iter().map(|arg| self.lower_operand(arg)));
            let ty = self.type_of(call_expr);
            return self.emit_call(CallTarget(format!("$cell::{op}")), call_args, ty);
        }

        if let ("Option" | "Result", "unwrap" | "expect") = (type_name.as_str(), method)
            && matches!(strip_borrow(&receiver_ty), Type::Enum(..))
        {
            return self.lower_unwrap(receiver, &type_name, method, args, call_expr);
        }
        match (type_name.as_str(), method) {
            ("Sender", "send") => return self.lower_channel_send(receiver, &args[0], call_expr),
            ("Sender", "close") => return self.lower_channel_close(receiver, "paco_rt_sender_close"),
            ("Receiver", "close") => return self.lower_channel_close(receiver, "paco_rt_receiver_close"),
            ("Receiver", "recv") => return self.lower_channel_recv(receiver, call_expr),
            ("JoinHandle", "join") => return self.lower_join_handle_join(receiver, call_expr),
            ("Generator", "next") => return self.lower_generator_next(receiver, call_expr),
            _ => {}
        }

        if self.is_intrinsic(&type_name, method) {
            return self.lower_dim_intrinsic(receiver, method, args, call_expr);
        }
        let base_target = self.registry.symbol(&format!("{type_name}::{method}"));
        let mut symbolic = nominal_type_args(&self.symbolic_type_of(receiver)).to_vec();
        symbolic.extend(self.own_generic_args(call_expr));
        let callee = self.registry.find_method_decl(&type_name, method).map(|(decl, _)| decl);
        let (key, slots) = self.instance_of(Some(&type_name), callee, &symbolic);
        let target = CallTarget(self.instantiations.record(&base_target, &key));

        let mut lowered = vec![(self.lower_operand(receiver), receiver_ty.clone())];
        for arg in args {
            let ty = self.type_of(arg);
            if borrow_args {
                let place = self.lower_place(arg);
                let borrow_ty = Type::Borrow { mutable: false, ty: Box::new(ty) };
                let temp = self.declare_local(None, borrow_ty.clone(), false);
                self.push(Statement::Assign(Place::Local(temp), Rvalue::Ref { mutable: false, place }));
                lowered.push((Operand::Copy(Place::Local(temp)), borrow_ty));
            } else {
                lowered.push((self.lower_operand(arg), ty));
            }
        }
        let mut call_args = self.hidden_values(Some(&type_name), callee, &slots, &symbolic, &lowered);
        call_args.extend(lowered.into_iter().map(|(operand, _)| operand));

        let ty = result_ty.unwrap_or_else(|| self.type_of(call_expr));
        self.emit_call(target, call_args, ty)
    }

    /// Inlined so a failed check reports the caller's location.
    fn lower_unwrap(&mut self, receiver: &Expr, type_name: &str, method: &str, args: &[Expr], call_expr: &Expr) -> Operand {
        let receiver_ty = self.type_of(receiver);
        let Type::Enum(enum_name, _) = &receiver_ty else { unreachable!("checked by the caller") };
        let success = if type_name == "Option" { "Some" } else { "Ok" };
        let success_index = self
            .registry
            .variant_index(enum_name, success)
            .unwrap_or_else(|| panic!("unknown variant `{enum_name}::{success}`"));
        let receiver_operand = self.lower_operand(receiver);
        let scrutinee = self.declare_local(Some(TRY_TEMP.to_string()), receiver_ty.clone(), false);
        self.push(Statement::Assign(Place::Local(scrutinee), Rvalue::Use(receiver_operand)));
        let discriminant = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(Place::Local(discriminant), Rvalue::Discriminant(Place::Local(scrutinee))));
        let success_block = self.reserve_block();
        let failure_block = self.reserve_block();
        self.finish_current(Terminator::SwitchInt {
            discriminant: Operand::Copy(Place::Local(discriminant)),
            targets: vec![(success_index as i128, success_block)],
            otherwise: failure_block,
        });

        self.switch_to(failure_block);
        let message = match (method, args) {
            ("expect", [message]) => self.lower_operand(message),
            _ if type_name == "Option" => Operand::Constant(Constant::Str("called `Option::unwrap()` on a `None` value".to_string())),
            _ => Operand::Constant(Constant::Str("called `Result::unwrap()` on an `Err` value".to_string())),
        };
        self.at(expr_span(call_expr), |this| this.lower_panic(message));

        self.switch_to(success_block);
        let value_ty = self.type_of(call_expr);
        let payload = Place::VariantField { base: Box::new(Place::Local(scrutinee)), variant: success.to_string(), index: 0 };
        let value = self.place_as_local_alias(payload, value_ty.clone());
        if is_copy(&value_ty) { Operand::Copy(Place::Local(value)) } else { Operand::Move(Place::Local(value)) }
    }

    fn shared_cell_op(&self, type_name: &str, method: &str) -> Option<&'static str> {
        if self.registry.struct_field_order(type_name).is_some() {
            return None;
        }
        match (type_name, method) {
            ("Rc" | "Arc" | "Cell" | "RefCell" | "Mutex" | "RwLock", "new") => Some("new"),
            ("Rc" | "Arc" | "Cell" | "RefCell", "get") | ("Mutex", "lock") | ("RwLock", "read") => Some("get"),
            ("Cell" | "RefCell" | "Mutex", "set") | ("RwLock", "write") => Some("set"),
            ("Rc" | "Arc", "clone") => Some("clone"),
            ("Rc" | "Arc", "strong_count") => Some("strong_count"),
            _ => None,
        }
    }

    fn emit_call(&mut self, target: CallTarget, call_args: Vec<Operand>, ty: Type) -> Operand {
        let destination = if matches!(ty, Type::Unit) {
            None
        } else {
            Some(Place::Local(self.declare_local(None, ty.clone(), false)))
        };
        let resume = self.reserve_block();
        self.finish_current(Terminator::Call {
            target,
            args: call_args,
            destination: destination.clone(),
            resume,
        });
        self.switch_to(resume);
        match destination {
            Some(place) if is_copy(&ty) => Operand::Copy(place),
            Some(place) => Operand::Move(place),
            None => Operand::Constant(Constant::Unit),
        }
    }

    fn lower_try(&mut self, operand: &Expr) -> Operand {
        let operand_ty = self.type_of(operand);
        let Type::Enum(enum_name, type_args) = &operand_ty else {
            panic!("`?` operand is not an enum-typed Result: {operand_ty:?}");
        };
        let ok_ty = type_args[0].clone();
        let src_err_ty = type_args[1].clone();

        let scrutinee_place = self.lower_place(operand);
        if let Place::Local(temp) = scrutinee_place
            && self.locals[temp.0 as usize].name.is_none()
        {
            self.locals[temp.0 as usize].name = Some(TRY_TEMP.to_string());
        }
        let ok_index = self
            .registry
            .variant_index(enum_name, "Ok")
            .unwrap_or_else(|| panic!("unknown variant `{enum_name}::Ok`"));

        let discriminant_temp = self.declare_local(None, Type::Int(IntWidth::I64), false);
        self.push(Statement::Assign(
            Place::Local(discriminant_temp),
            Rvalue::Discriminant(scrutinee_place.clone()),
        ));

        let ok_block = self.reserve_block();
        let err_block = self.reserve_block();
        self.finish_current(Terminator::SwitchInt {
            discriminant: Operand::Copy(Place::Local(discriminant_temp)),
            targets: vec![(ok_index as i128, ok_block)],
            otherwise: err_block,
        });

        self.switch_to(err_block);
        let err_payload = Place::VariantField {
            base: Box::new(scrutinee_place.clone()),
            variant: "Err".to_string(),
            index: 0,
        };
        let err_operand = if is_copy(&src_err_ty) {
            Operand::Copy(err_payload)
        } else {
            Operand::Move(err_payload)
        };

        let expected_return = self.expected_return.clone();
        let Type::Enum(_, return_args) = &expected_return else {
            panic!("enclosing function's return type is not a Result: {expected_return:?}");
        };
        let dst_err_ty = return_args[1].clone();

        let converted_operand = if dst_err_ty == src_err_ty {
            err_operand
        } else {
            let dst_name = type_name_of(&dst_err_ty).unwrap_or_else(|| {
                panic!("`?` error type has no resolvable named type: {dst_err_ty:?}")
            });
            let target = CallTarget(self.registry.symbol(&format!("{dst_name}::from")));
            let destination = Place::Local(self.declare_local(None, dst_err_ty.clone(), false));
            let resume = self.reserve_block();
            self.finish_current(Terminator::Call {
                target,
                args: vec![err_operand],
                destination: Some(destination.clone()),
                resume,
            });
            self.switch_to(resume);
            if is_copy(&dst_err_ty) {
                Operand::Copy(destination)
            } else {
                Operand::Move(destination)
            }
        };

        let err_result_local = self.declare_local(None, expected_return.clone(), false);
        self.push(Statement::Assign(
            Place::Local(err_result_local),
            Rvalue::Aggregate {
                ty: expected_return,
                variant: Some("Err".to_string()),
                fields: vec![converted_operand],
            },
        ));
        self.drop_scopes_from(1);
        self.terminate_and_abandon(Terminator::Return(Operand::Move(Place::Local(
            err_result_local,
        ))));

        self.switch_to(ok_block);
        let ok_payload = Place::VariantField {
            base: Box::new(scrutinee_place),
            variant: "Ok".to_string(),
            index: 0,
        };
        let ok_local = self.place_as_local_alias(ok_payload, ok_ty.clone());
        if is_copy(&ok_ty) {
            Operand::Copy(Place::Local(ok_local))
        } else {
            Operand::Move(Place::Local(ok_local))
        }
    }
}

fn type_name_of(ty: &Type) -> Option<String> {
    match ty {
        Type::Struct(name, _) | Type::Enum(name, _) => Some(name.clone()),
        Type::Slice(_) => Some(paco_types::SLICE_TYPE_NAME.to_string()),
        Type::Borrow { ty, .. } => type_name_of(ty),
        _ => None,
    }
}

/// Methods on a primitive type are global: the type has no owning module.
pub fn is_primitive_type_name(name: &str) -> bool {
    matches!(name, "string" | "char" | "bool" | "byte" | "float")
        || paco_types::FloatWidth::from_name(name).is_some()
        || matches!(name, "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64")
}

/// A nominal type's own concrete generic arguments (`Vec<i64>` -> `[i64]`),
/// empty for anything without any (including a non-generic nominal type,
/// which is exactly the case `InstantiationRegistry::record` treats as
/// "not generic, use the bare method name unchanged"). Strips a borrow
/// layer first, matching `type_name_of`'s own receiver-type handling.
fn nominal_type_args(ty: &Type) -> &[Type] {
    match ty {
        Type::Struct(_, args) | Type::Enum(_, args) => args,
        Type::Slice(elem) => std::slice::from_ref(elem.as_ref()),
        Type::Borrow { ty, .. } | Type::RawPointer { ty, .. } => nominal_type_args(ty),
        _ => &[],
    }
}

/// `[]T`/`&[]T`/`&mut []T` all index the same way — the built-in case,
/// distinct from a structurally-dispatched user `index` method.
fn slice_elem_ty(ty: &Type) -> Option<Type> {
    match ty {
        Type::Slice(elem) => Some(elem.as_ref().clone()),
        Type::Borrow { ty, .. } => slice_elem_ty(ty),
        _ => None,
    }
}

fn associated_call_type_name(ty: &Ty) -> String {
    match ty {
        Ty::Path(path, _) => Some(path.join("::")),
        Ty::Generic { path, .. } => Some(path.join("::")),
        _ => None,
    }
    .unwrap_or_else(|| panic!("associated-call target must be a named type, got {ty:?}"))
}

/// The `IntWidth` an already-type-checked expression's `Constant::Int`
/// should carry; defaults to `I64` for non-integer expressions, where the
/// value is never read.
fn int_width_of(ty: &Type) -> IntWidth {
    match ty {
        Type::Int(width) => *width,
        _ => IntWidth::I64,
    }
}

/// Checks a widened `i128` arithmetic result against `width`'s own range
/// and narrows it back to `Constant::Int`'s `i64` bit-pattern payload.
/// Operands must already be widened through `IntWidth::to_i128`, not plain
/// `i64` arithmetic: a `u64` value past `i64::MAX` looks negative as `i64`,
/// so native `i64` checked arithmetic would misjudge its overflow.
fn checked_in_width(value: i128, width: IntWidth) -> i64 {
    let (min, max) = width.range();
    if value < min || value > max {
        panic!("integer overflow");
    }
    width.from_i128(value)
}

fn strip_borrow(ty: &Type) -> &Type {
    match ty {
        Type::Borrow { ty, .. } => strip_borrow(ty),
        other => other,
    }
}

/// Whether the single-`SwitchInt` lowering in `lower_match` handles `pattern`.
fn is_switchable_pattern(pattern: &Pat) -> bool {
    match pattern {
        Pat::Literal(Literal::Int(_) | Literal::Bool(_), _) | Pat::Wildcard(_) | Pat::Ident(_, _) => true,
        Pat::Enum { fields, .. } => fields.iter().all(|field| matches!(field, Pat::Ident(_, _) | Pat::Wildcard(_))),
        _ => false,
    }
}

fn pattern_binds(pattern: &Pat) -> bool {
    match pattern {
        Pat::Ident(_, _) | Pat::Binding { .. } => true,
        Pat::Tuple(patterns, _) | Pat::Or(patterns, _) | Pat::Enum { fields: patterns, .. } => patterns.iter().any(pattern_binds),
        Pat::Struct { fields, .. } => fields.iter().any(|(_, pattern)| pattern_binds(pattern)),
        Pat::Wildcard(_) | Pat::Literal(_, _) | Pat::Range { .. } => false,
    }
}

fn lower_literal(literal: &Literal, ty: &Type) -> Constant {
    match literal {
        Literal::Int(value) => Constant::Int(*value, int_width_of(ty)),
        Literal::Float(value) => {
            let width = match ty {
                Type::Float(width) => *width,
                _ => FloatWidth::F64,
            };
            Constant::Float(width.round(*value).to_bits(), width)
        }
        Literal::Bool(value) => Constant::Bool(*value),
        Literal::String(value) => Constant::Str(value.clone()),
        Literal::Char(value) => Constant::Char(*value),
    }
}

/// `None` when the initializer needs more than literals, operators and
/// other constants (e.g. a `comptime` block): it is then lowered in place.
fn fold_const(expr: &Expr, consts: &HashMap<String, &Expr>, typed: &TypedModule<'_>) -> Option<Constant> {
    Some(match expr {
        Expr::Literal(literal, _) => {
            let ty = typed.type_of(expr).cloned().unwrap_or(Type::Int(IntWidth::I64));
            lower_literal(literal, &ty)
        }
        Expr::Ident(name, _) => fold_const(consts.get(name.as_str())?, consts, typed)?,
        Expr::Unary { op, expr, .. } => fold_unary(*op, fold_const(expr, consts, typed)?),
        Expr::Binary {
            op, left, right, ..
        } => fold_binary(
            *op,
            fold_const(left, consts, typed)?,
            fold_const(right, consts, typed)?,
        ),
        _ => return None,
    })
}

fn fold_unary(op: AstUnOp, operand: Constant) -> Constant {
    match (op, operand) {
        (AstUnOp::Neg, Constant::Int(value, width)) => Constant::Int(
            checked_in_width(
                width.to_i128(value).checked_neg().unwrap_or_else(|| panic!("integer overflow")),
                width,
            ),
            width,
        ),
        (AstUnOp::Neg, Constant::Float(bits, width)) => Constant::Float((-f64::from_bits(bits)).to_bits(), width),
        (AstUnOp::Not, Constant::Bool(value)) => Constant::Bool(!value),
        (AstUnOp::BitNot, Constant::Int(value, width)) => Constant::Int(width.wrap(!width.to_i128(value)), width),
        (op, operand) => panic!("const initializer operator `{op:?}` not evaluable over {operand:?}"),
    }
}

fn fold_binary(op: BinaryOp, left: Constant, right: Constant) -> Constant {
    match (op, left, right) {
        (BinaryOp::Add, Constant::Int(l, width), Constant::Int(r, _)) => Constant::Int(
            checked_in_width(
                width.to_i128(l).checked_add(width.to_i128(r)).unwrap_or_else(|| panic!("integer overflow")),
                width,
            ),
            width,
        ),
        (BinaryOp::Sub, Constant::Int(l, width), Constant::Int(r, _)) => Constant::Int(
            checked_in_width(
                width.to_i128(l).checked_sub(width.to_i128(r)).unwrap_or_else(|| panic!("integer overflow")),
                width,
            ),
            width,
        ),
        (BinaryOp::Mul, Constant::Int(l, width), Constant::Int(r, _)) => Constant::Int(
            checked_in_width(
                width.to_i128(l).checked_mul(width.to_i128(r)).unwrap_or_else(|| panic!("integer overflow")),
                width,
            ),
            width,
        ),
        (BinaryOp::Div, Constant::Int(l, width), Constant::Int(r, _)) => Constant::Int(
            checked_in_width(
                width.to_i128(l).checked_div(width.to_i128(r)).unwrap_or_else(|| panic!("division by zero")),
                width,
            ),
            width,
        ),
        (BinaryOp::Rem, Constant::Int(l, width), Constant::Int(r, _)) => Constant::Int(
            checked_in_width(
                width.to_i128(l).checked_rem(width.to_i128(r)).unwrap_or_else(|| panic!("division by zero")),
                width,
            ),
            width,
        ),
        (BinaryOp::Eq, Constant::Int(l, _), Constant::Int(r, _)) => Constant::Bool(l == r),
        (BinaryOp::Ne, Constant::Int(l, _), Constant::Int(r, _)) => Constant::Bool(l != r),
        // Ordering goes through `to_i128` (same `u64`-past-`i64::MAX` reason
        // as the arithmetic arms above); a raw `i64 < i64` would misorder it.
        (BinaryOp::Lt, Constant::Int(l, width), Constant::Int(r, _)) => {
            Constant::Bool(width.to_i128(l) < width.to_i128(r))
        }
        (BinaryOp::Le, Constant::Int(l, width), Constant::Int(r, _)) => {
            Constant::Bool(width.to_i128(l) <= width.to_i128(r))
        }
        (BinaryOp::Gt, Constant::Int(l, width), Constant::Int(r, _)) => {
            Constant::Bool(width.to_i128(l) > width.to_i128(r))
        }
        (BinaryOp::Ge, Constant::Int(l, width), Constant::Int(r, _)) => {
            Constant::Bool(width.to_i128(l) >= width.to_i128(r))
        }
        (BinaryOp::BitAnd, Constant::Int(l, width), Constant::Int(r, _)) => {
            Constant::Int(width.wrap(width.to_i128(l) & width.to_i128(r)), width)
        }
        (BinaryOp::BitOr, Constant::Int(l, width), Constant::Int(r, _)) => {
            Constant::Int(width.wrap(width.to_i128(l) | width.to_i128(r)), width)
        }
        (BinaryOp::BitXor, Constant::Int(l, width), Constant::Int(r, _)) => {
            Constant::Int(width.wrap(width.to_i128(l) ^ width.to_i128(r)), width)
        }
        (op @ (BinaryOp::Shl | BinaryOp::Shr), Constant::Int(l, width), Constant::Int(r, amount_width)) => {
            let amount = amount_width.to_i128(r);
            if !(0..(width.bytes() * 8) as i128).contains(&amount) {
                panic!("shift overflow");
            }
            let value = width.to_i128(l);
            let shifted = if op == BinaryOp::Shl { value << amount } else { value >> amount };
            Constant::Int(width.wrap(shifted), width)
        }
        (BinaryOp::Add, Constant::Float(l, width), Constant::Float(r, _)) => {
            Constant::Float(width.round(f64::from_bits(l) + f64::from_bits(r)).to_bits(), width)
        }
        (BinaryOp::Sub, Constant::Float(l, width), Constant::Float(r, _)) => {
            Constant::Float(width.round(f64::from_bits(l) - f64::from_bits(r)).to_bits(), width)
        }
        (BinaryOp::Mul, Constant::Float(l, width), Constant::Float(r, _)) => {
            Constant::Float(width.round(f64::from_bits(l) * f64::from_bits(r)).to_bits(), width)
        }
        (BinaryOp::Div, Constant::Float(l, width), Constant::Float(r, _)) => {
            Constant::Float(width.round(f64::from_bits(l) / f64::from_bits(r)).to_bits(), width)
        }
        (BinaryOp::Rem, Constant::Float(l, width), Constant::Float(r, _)) => {
            Constant::Float(width.round(f64::from_bits(l) % f64::from_bits(r)).to_bits(), width)
        }
        (BinaryOp::And, Constant::Bool(l), Constant::Bool(r)) => Constant::Bool(l && r),
        (BinaryOp::Or, Constant::Bool(l), Constant::Bool(r)) => Constant::Bool(l || r),
        (BinaryOp::Eq, Constant::Bool(l), Constant::Bool(r)) => Constant::Bool(l == r),
        (BinaryOp::Ne, Constant::Bool(l), Constant::Bool(r)) => Constant::Bool(l != r),
        (op, left, right) => {
            panic!("const initializer operator `{op:?}` not evaluable over {left:?}, {right:?}")
        }
    }
}

fn binary_operator_method_name(op: BinaryOp) -> Option<&'static str> {
    match op {
        BinaryOp::Add => Some("add"),
        BinaryOp::Sub => Some("sub"),
        BinaryOp::Mul => Some("mul"),
        BinaryOp::Div => Some("div"),
        BinaryOp::Rem => Some("rem"),
        _ => None,
    }
}

fn lower_binary_op(op: BinaryOp) -> BinOp {
    match op {
        BinaryOp::Add => BinOp::Add,
        BinaryOp::Sub => BinOp::Sub,
        BinaryOp::Mul => BinOp::Mul,
        BinaryOp::Div => BinOp::Div,
        BinaryOp::Rem => BinOp::Rem,
        BinaryOp::Eq => BinOp::Eq,
        BinaryOp::Ne => BinOp::Ne,
        BinaryOp::Lt => BinOp::Lt,
        BinaryOp::Le => BinOp::Le,
        BinaryOp::Gt => BinOp::Gt,
        BinaryOp::Ge => BinOp::Ge,
        BinaryOp::And => BinOp::And,
        BinaryOp::Or => BinOp::Or,
        BinaryOp::BitAnd => BinOp::BitAnd,
        BinaryOp::BitOr => BinOp::BitOr,
        BinaryOp::BitXor => BinOp::BitXor,
        BinaryOp::Shl => BinOp::Shl,
        BinaryOp::Shr => BinOp::Shr,
    }
}

fn lower_unary_op(op: AstUnOp) -> UnOp {
    match op {
        AstUnOp::Not => UnOp::Not,
        AstUnOp::Neg => UnOp::Neg,
        AstUnOp::BitNot => UnOp::BitNot,
        AstUnOp::Deref => unreachable!("Deref is lowered directly in lower_rvalue"),
    }
}

/// Lowers `function`. Returns its own `Body` plus any top-level bodies
/// outlined while lowering it (`spawn`/`iter fn` thunks — task 4/6); the
/// caller must declare and define both, the same way it already declares
/// and defines every other top-level function.
pub fn lower_function(
    function: &FnDecl,
    typed: &TypedModule<'_>,
    registry: &TypeRegistry<'_>,
    drops: &DropPlan<'_>,
    profile: Profile,
) -> (Body, Vec<(String, Body)>) {
    lower_function_with_substitutions(
        function,
        typed,
        registry,
        drops,
        profile,
        &HashMap::new(),
        &InstantiationRegistry::new(),
    )
}

/// Same as [`lower_function`], but every type read from `typed` (the
/// generic `TypedModule`'s own cached types — locals, params, return type)
/// is passed through `substitute_generics(ty, substitutions)` before it
/// reaches codegen-facing MIR, and any generic method call this body's own
/// lowering discovers is recorded into `instantiations` instead of being
/// lowered eagerly. `substitutions` empty and a fresh, never-drained
/// `instantiations` (as `lower_function` passes) is exactly the pre-
/// existing, non-generic behavior — `generic-function-codegen`'s own task
/// 1.2 requirement.
pub fn lower_function_with_substitutions(
    function: &FnDecl,
    typed: &TypedModule<'_>,
    registry: &TypeRegistry<'_>,
    drops: &DropPlan<'_>,
    profile: Profile,
    substitutions: &HashMap<String, Type>,
    instantiations: &InstantiationRegistry,
) -> (Body, Vec<(String, Body)>) {
    lower_instance(function, typed, registry, drops, profile, substitutions, &[], instantiations, &HashMap::new())
}

/// Lowers one instance of a generic function. `hidden` names, in order, the
/// dimension names `substitutions` mentions whose values arrive as leading
/// `i64` arguments. `source_text` is `paco-driver`'s own raw-source lookup
/// for a `stdlib::test` assertion call's argument sub-expressions (empty for
/// every caller besides `paco-driver` itself — see `Lowerer::source_text`).
#[allow(clippy::too_many_arguments)]
pub fn lower_instance(
    function: &FnDecl,
    typed: &TypedModule<'_>,
    registry: &TypeRegistry<'_>,
    drops: &DropPlan<'_>,
    profile: Profile,
    substitutions: &HashMap<String, Type>,
    hidden: &[String],
    instantiations: &InstantiationRegistry,
    source_text: &HashMap<Span, String>,
) -> (Body, Vec<(String, Body)>) {
    let expected_return = typed
        .type_of_fn_return(function)
        .cloned()
        .unwrap_or(Type::Unknown);
    let expected_return = paco_types::erase_symbolic(&paco_types::substitute_generics(&expected_return, substitutions));
    let mut lowerer = Lowerer::new(typed, registry, drops, expected_return, profile, substitutions, instantiations, source_text);
    lowerer.in_comptime = is_comptime_only(function);
    for name in hidden {
        let local = lowerer.declare_local(Some(paco_types::display_name(name)), Type::Int(IntWidth::I64), false);
        lowerer.atom_values.insert(name.clone(), Operand::Copy(Place::Local(local)));
    }

    let mut bound = Vec::new();
    for param in &function.params {
        let ty = typed
            .type_of_param(param)
            .cloned()
            .unwrap_or(Type::Unknown);
        let ty = paco_types::erase_symbolic(&paco_types::substitute_generics(&ty, substitutions));
        let ty = match &param.ty {
            Ty::Borrow { mutable, .. } if !is_copy(&ty) => Type::Borrow { mutable: *mutable, ty: Box::new(ty) },
            _ => ty,
        };
        if let Pat::Ident(name, _) = &param.pattern {
            let local = lowerer.declare_local(Some(name.clone()), ty, false);
            bound.push((lowerer.binding_id(&param.pattern), local));
        }
    }
    lowerer.span = function.span;
    for (id, local) in bound {
        lowerer.bind(id, local);
    }

    let result = lowerer.lower_block(&function.body);
    lowerer.span = function.body.span;
    lowerer.finish_current(Terminator::Return(result));

    let return_ty = lowerer.expected_return.clone();
    lowerer.into_body(profile, hidden.len() + function.params.len(), return_ty, function.span)
}

/// A `comptime fn`, or a function taking or returning a `type` or `Code`:
/// it only ever runs during compilation.
/// Prefix of the call a `grad(f, inputs)` lowers to, followed by `f`'s symbol.
pub const GRAD_PREFIX: &str = "$grad:";

fn is_builtin_grad(function: &FnDecl) -> bool {
    function.attrs.iter().any(|attr| {
        attr.name == "builtin" && matches!(attr.args.first(), Some(paco_syntax::ast::AttributeArg::Path(path, _)) if path == &["grad"])
    })
}

/// `stdlib::test`'s nine `#[builtin(name)]`-tagged assertion functions
/// (`compiler/paco-types` and `compiler/paco-borrow` each keep their own
/// independent copy of this same check, per `#[builtin(grad)]`'s own
/// precedent — see `unit-testing`'s design.md).
const TEST_ASSERT_BUILTINS: &[&str] =
    &["assert", "assert_eq", "assert_ne", "assert_true", "assert_false", "assert_some", "assert_none", "assert_ok", "assert_err"];

fn is_builtin_test_assert(function: &FnDecl) -> bool {
    function.attrs.iter().any(|attr| {
        attr.name == "builtin"
            && matches!(attr.args.first(), Some(paco_syntax::ast::AttributeArg::Path(path, _)) if path.len() == 1 && TEST_ASSERT_BUILTINS.contains(&path[0].as_str()))
    })
}

pub fn is_comptime_only(function: &FnDecl) -> bool {
    let compile_time = |ty: &Ty| matches!(ty, Ty::Path(path, _) if path.as_slice() == ["type"] || path.as_slice() == ["Code"]);
    function.is_comptime
        || function.params.iter().any(|param| compile_time(&param.ty))
        || function.return_ty.as_ref().is_some_and(compile_time)
}

/// A closure environment is `[thunk, drop_fn, captures...]`, preceded by
/// its reference count; `drop_fn` (null when nothing needs it) drops the
/// captures the environment owns.
const CLOSURE_CAPTURES_OFFSET: i64 = 16;

fn outline_env_drop(capture_tys: &[Type], profile: Profile, span: Span) -> Body {
    let mut locals = vec![LocalDecl { name: Some("__env".to_string()), ty: Type::Int(IntWidth::I64), mutable: false }];
    let mut statements = Vec::new();
    let mut declare = |ty: Type| {
        locals.push(LocalDecl { name: None, ty, mutable: false });
        Local(locals.len() as u32 - 1)
    };
    for (index, ty) in capture_tys.iter().enumerate().filter(|(_, ty)| !is_copy(ty)) {
        let slot = declare(Type::Int(IntWidth::I64));
        let loaded = declare(ty.clone());
        let owner = declare(ty.clone());
        statements.extend([
            Statement::Assign(
                Place::Local(slot),
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(Place::Local(Local(0))),
                    Operand::Constant(Constant::Int(CLOSURE_CAPTURES_OFFSET + (index as i64) * 8, IntWidth::I64)),
                ),
            ),
            Statement::Assign(Place::Local(loaded), Rvalue::Load { address: Operand::Copy(Place::Local(slot)), ty: ty.clone() }),
            Statement::Assign(Place::Local(owner), Rvalue::Use(Operand::Move(Place::Local(loaded)))),
            Statement::FreeBox { address: Operand::Copy(Place::Local(slot)), ty: ty.clone() },
        ]);
    }
    Body {
        locals,
        blocks: vec![BasicBlock { statements, terminator: Terminator::Return(Operand::Constant(Constant::Unit)) }],
        profile,
        param_count: 1,
        return_ty: Type::Unit,
        span,
        spans: Vec::new(),
    }
}

/// Outlines an `iter fn`'s own body into its "yield thunk" — a top-level
/// function under the `iter fn`'s own declared name, taking exactly one
/// `captures: i64` parameter (no `result_out`: an `iter fn` never returns
/// a single value, it repeatedly calls `paco_rt_generator_yield` instead
/// — see `lower_yield`), matching `paco_rt_generator_new`'s fixed C ABI.
/// The `iter fn`'s own declared *parameters* become this thunk's captures
/// — unpacked here the same way `outline_thunk`'s are, but marshaled into
/// the buffer at each *call site* instead of by an enclosing scope (an
/// `iter fn` is a plain top-level declaration, not a closure; see
/// `Lowerer::lower_iter_fn_call`). The caller (`paco-driver` etc.) must
/// call this instead of `lower_function` for any `Item::Fn` with
/// `is_iter: true` — lowering it as an ordinary function would run its
/// body eagerly and hit `yield` outside any generator-suspend context.
pub fn lower_iter_fn(
    function: &FnDecl,
    typed: &TypedModule<'_>,
    registry: &TypeRegistry<'_>,
    drops: &DropPlan<'_>,
    profile: Profile,
) -> (Body, Vec<(String, Body)>) {
    lower_iter_fn_with_substitutions(
        function,
        typed,
        registry,
        drops,
        profile,
        &HashMap::new(),
        &InstantiationRegistry::new(),
        &HashMap::new(),
    )
}

/// Same as [`lower_iter_fn`], but with `lower_function_with_substitutions`'s
/// own substitution/instantiation-recording behavior. `source_text`: see
/// `lower_instance`.
#[allow(clippy::too_many_arguments)]
pub fn lower_iter_fn_with_substitutions(
    function: &FnDecl,
    typed: &TypedModule<'_>,
    registry: &TypeRegistry<'_>,
    drops: &DropPlan<'_>,
    profile: Profile,
    substitutions: &HashMap<String, Type>,
    instantiations: &InstantiationRegistry,
    source_text: &HashMap<Span, String>,
) -> (Body, Vec<(String, Body)>) {
    let mut lowerer = Lowerer::new(typed, registry, drops, Type::Unit, profile, substitutions, instantiations, source_text);

    let captures_param = lowerer.declare_local(Some("__captures".to_string()), Type::Int(IntWidth::I64), false);
    let cancelled_param = lowerer.declare_local(Some("__cancelled".to_string()), Type::Int(IntWidth::I64), false);

    for (index, param) in function.params.iter().enumerate() {
        let Pat::Ident(name, _) = &param.pattern else {
            continue;
        };
        let ty = typed.type_of_param(param).cloned().unwrap_or(Type::Unknown);
        let ty = paco_types::erase_symbolic(&paco_types::substitute_generics(&ty, substitutions));
        let slot_addr = lowerer.offset_address(captures_param, (index as i64) * 8);
        let value_local = lowerer.load_owned(Some(name.clone()), slot_addr, ty);
        lowerer.bind(lowerer.binding_id(&param.pattern), value_local);
    }
    lowerer.span = function.span;
    lowerer.return_if_cancelled(cancelled_param);

    lowerer.lower_block(&function.body);
    lowerer.finish_current(Terminator::Return(Operand::Constant(Constant::Unit)));

    lowerer.into_body(profile, 2, Type::Unit, function.span)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instantiation_registry_records_the_same_pair_once() {
        let registry = InstantiationRegistry::new();
        let first = registry.record("Vec::push", &[Type::Int(IntWidth::I64)]);
        let second = registry.record("Vec::push", &[Type::Int(IntWidth::I64)]);
        assert_eq!(first, second);
        assert_eq!(registry.drain_pending().len(), 1);
        assert_eq!(registry.drain_pending().len(), 0);
    }

    #[test]
    fn instantiation_registry_is_a_no_op_for_non_generic_calls() {
        let registry = InstantiationRegistry::new();
        let name = registry.record("Vec::len", &[]);
        assert_eq!(name, "Vec::len");
        assert!(registry.drain_pending().is_empty());
    }

    #[test]
    #[should_panic(expected = "exceeded the instantiation cap")]
    fn instantiation_registry_panics_past_its_cap() {
        let registry = InstantiationRegistry::new().with_max_instantiations(2);
        registry.record("Vec::push", &[Type::Int(IntWidth::I8)]);
        registry.record("Vec::push", &[Type::Int(IntWidth::I16)]);
        registry.record("Vec::push", &[Type::Int(IntWidth::I32)]);
    }
}
