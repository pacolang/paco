//! Type checking for executable frontend features.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque, hash_map::Entry};
use std::marker::PhantomData;

mod dims;
mod floats;
mod named;

pub use dims::{ConstExpr, Dim, DimOp, Factor, Verdict, dim_type, display_name, fresh_atom, is_atom, is_existential};
pub use named::{AtomInfo, AtomSource, ShapeRow, record_shapes};
use paco_syntax::parse::expr_span;

use paco_diag::{Diagnostic, Reporter};
use paco_match::{ConstructorSet, analyze_match};
use paco_span::Span;
use paco_resolve::LocalId;
use paco_syntax::ast::{
    self, BinaryOp, Block, ConstDecl, EnumDecl, Expr, ExternBlock, FnDecl, Item, LetStmt, Literal,
    MatchArm, MethodsBlock, Module, Param, Pat, QuoteBody, Stmt, StructDecl, Ty, UnaryOp,
    VariantFields, Visit, walk_expr,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum IntWidth {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
}

impl IntWidth {
    fn name(self) -> &'static str {
        match self {
            IntWidth::I8 => "i8",
            IntWidth::I16 => "i16",
            IntWidth::I32 => "i32",
            IntWidth::I64 => "i64",
            IntWidth::U8 => "u8",
            IntWidth::U16 => "u16",
            IntWidth::U32 => "u32",
            IntWidth::U64 => "u64",
        }
    }

    pub fn is_signed(self) -> bool {
        matches!(self, IntWidth::I8 | IntWidth::I16 | IntWidth::I32 | IntWidth::I64)
    }

    /// Byte width, used by `paco-mir`/`paco-codegen-cranelift` for layout
    /// and native-type selection.
    pub fn bytes(self) -> u64 {
        match self {
            IntWidth::I8 | IntWidth::U8 => 1,
            IntWidth::I16 | IntWidth::U16 => 2,
            IntWidth::I32 | IntWidth::U32 => 4,
            IntWidth::I64 | IntWidth::U64 => 8,
        }
    }

    /// `(min, max)` as `i128`, wide enough to hold `u64::MAX` without
    /// wrapping — used for literal range checking (task 2.3) and by
    /// `paco-mir`'s constant folder for width-correct overflow checking.
    pub fn range(self) -> (i128, i128) {
        match self {
            IntWidth::I8 => (i8::MIN as i128, i8::MAX as i128),
            IntWidth::I16 => (i16::MIN as i128, i16::MAX as i128),
            IntWidth::I32 => (i32::MIN as i128, i32::MAX as i128),
            IntWidth::I64 => (i64::MIN as i128, i64::MAX as i128),
            IntWidth::U8 => (0, u8::MAX as i128),
            IntWidth::U16 => (0, u16::MAX as i128),
            IntWidth::U32 => (0, u32::MAX as i128),
            IntWidth::U64 => (0, u64::MAX as i128),
        }
    }

    /// Reinterprets a payload stored as a raw bit pattern in one `i64`
    /// (`paco-mir`'s `Constant::Int`) as this width's true numeric value. Every width but
    /// `U64` already fits its true value inside `i64` directly (even
    /// `u32::MAX` is comfortably positive as `i64`); `U64` is the only width
    /// whose payload can look negative as `i64` while meaning a large
    /// positive value, so it alone needs reinterpreting through `u64` before
    /// widening.
    pub fn to_i128(self, bits: i64) -> i128 {
        if matches!(self, IntWidth::U64) {
            i128::from(bits as u64)
        } else {
            i128::from(bits)
        }
    }

    /// Truncates `value` to this width with two's-complement wraparound and
    /// returns the `i64` bit-pattern payload `to_i128` reads.
    pub fn wrap(self, value: i128) -> i64 {
        let bits = self.bytes() * 8;
        let low = (value as u128) & (u128::MAX >> (128 - bits));
        if self.is_signed() && (low >> (bits - 1)) & 1 == 1 {
            (low as i128 - (1i128 << bits)) as i64
        } else {
            low as u64 as i64
        }
    }

    /// Inverse of `to_i128`: narrows an already width-checked value back to
    /// the `i64` bit-pattern payload.
    pub fn from_i128(self, value: i128) -> i64 {
        if matches!(self, IntWidth::U64) {
            value as u64 as i64
        } else {
            value as i64
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FloatWidth {
    F64,
    F32,
    F16,
    BF16,
    F8E4M3,
    F8E5M2,
}

impl FloatWidth {
    pub fn name(self) -> &'static str {
        match self {
            FloatWidth::F64 => "float",
            FloatWidth::F32 => "f32",
            FloatWidth::F16 => "f16",
            FloatWidth::BF16 => "bf16",
            FloatWidth::F8E4M3 => "f8e4m3",
            FloatWidth::F8E5M2 => "f8e5m2",
        }
    }

    pub fn from_name(name: &str) -> Option<FloatWidth> {
        Some(match name {
            "float" | "f64" => FloatWidth::F64,
            "f32" => FloatWidth::F32,
            "f16" => FloatWidth::F16,
            "bf16" => FloatWidth::BF16,
            "f8e4m3" => FloatWidth::F8E4M3,
            "f8e5m2" => FloatWidth::F8E5M2,
            _ => return None,
        })
    }

    pub fn bytes(self) -> u64 {
        match self {
            FloatWidth::F64 => 8,
            FloatWidth::F32 => 4,
            FloatWidth::F16 | FloatWidth::BF16 => 2,
            FloatWidth::F8E4M3 | FloatWidth::F8E5M2 => 1,
        }
    }

    pub fn has_arithmetic(self) -> bool {
        !matches!(self, FloatWidth::F8E4M3 | FloatWidth::F8E5M2)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Type {
    Int(IntWidth),
    Float(FloatWidth),
    Bool,
    String,
    Char,
    Unit,
    Never,
    Struct(String, Vec<Type>),
    Enum(String, Vec<Type>),
    Borrow { mutable: bool, ty: Box<Type> },
    RawPointer { mutable: bool, ty: Box<Type> },
    /// `[]T` (ADR 0020): an owned, fixed-length buffer. Source-level syntax
    /// for it is not parsed yet (deferred to Phase 2b per ADR 0020's own
    /// consequences); this variant exists so `paco-mir`'s `Layout` engine
    /// can represent it once a `Ty::Slice` AST node exists.
    Slice(Box<Type>),
    Tuple(Vec<Type>),
    Fn(Vec<Type>, Box<Type>),
    Generic(String),
    /// `type` (`phase-9-comptime` Decision 5): a comptime-only value that
    /// *is* a captured `Type`, not a value of that type. Only meaningful
    /// as a `comptime fn` parameter/return annotation — never has a
    /// runtime representation once compilation finishes. The boxed
    /// `Type` is `Type::Unknown` at the bare declaration site (`t: type`)
    /// and becomes concrete once a call site resolves a real argument
    /// against it (`ty_from_ast`'s own site, plus the call-argument
    /// resolution this variant specifically enables).
    TypeValue(Box<Type>),
    /// The result of evaluating a `quote { .. }` expression
    /// (`phase-9-comptime` Decision 7): a handle over one spliced,
    /// not-yet-lowered AST item or expression. Comptime-only, like
    /// `TypeValue` — never has a runtime representation once compilation
    /// finishes.
    Code,
    Dim(Dim),
    Pack(Vec<Type>),
    /// `R...` as the last element of a pack: the rest of the dimensions,
    /// bound as a whole to the pack parameter `R`.
    Spread(String),
    Unknown,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeError;

/// The result of [`infer_module`]: every `Expr` node checked, with its
/// inferred [`Type`] attached. Nodes are identified by address into the
/// `Module` that produced this `TypedModule`, which is why the lifetime is
/// tied to that borrow — callers look types up with a `&Expr` borrowed from
/// the same `Module`.
pub struct TypedModule<'a> {
    types: HashMap<*const Expr, Type>,
    param_types: HashMap<*const Param, Type>,
    return_types: HashMap<*const FnDecl, Type>,
    call_generics: HashMap<*const Expr, Vec<Type>>,
    call_targets: HashMap<*const Expr, Type>,
    self_types: HashMap<*const FnDecl, Type>,
    locals: paco_resolve::Locals,
    atoms: HashMap<String, AtomInfo>,
    atoms_by_local: HashMap<LocalId, Vec<String>>,
    shapes: Vec<ShapeRow>,
    _module: PhantomData<&'a Module>,
}

impl<'a> TypedModule<'a> {
    pub fn type_of(&self, expr: &Expr) -> Option<&Type> {
        self.types.get(&(expr as *const Expr))
    }

    pub fn expr_types(&self) -> &HashMap<*const Expr, Type> {
        &self.types
    }

    /// A function parameter's resolved type (its receiver's `Self` type for
    /// `self`/`&self`/`&mut self`, or its declared type otherwise) — needed
    /// because `Param` carries only the unresolved syntactic `Ty`.
    pub fn type_of_param(&self, param: &Param) -> Option<&Type> {
        self.param_types.get(&(param as *const Param))
    }

    /// A function's resolved return type — needed because `FnDecl` carries
    /// only the unresolved syntactic `Ty`.
    pub fn type_of_fn_return(&self, function: &FnDecl) -> Option<&Type> {
        self.return_types.get(&(function as *const FnDecl))
    }

    /// The arguments a generic call bound to the callee's own generic
    /// parameters (not its receiver's), in declaration order.
    pub fn call_generics(&self, call: &Expr) -> Option<&[Type]> {
        self.call_generics.get(&(call as *const Expr)).map(Vec::as_slice)
    }

    /// The type an associated call written with explicit generic arguments
    /// (`Tensor<f32, 2, 3>::zeros()`) was made on.
    pub fn call_target(&self, call: &Expr) -> Option<&Type> {
        self.call_targets.get(&(call as *const Expr))
    }

    /// The `Self` type a struct/enum/`methods` function was declared on,
    /// with the owner's generic parameters left as `Type::Generic`.
    pub fn self_type_of(&self, function: &FnDecl) -> Option<&Type> {
        self.self_types.get(&(function as *const FnDecl))
    }

    pub fn locals(&self) -> &paco_resolve::Locals {
        &self.locals
    }

    /// A dimension name (`x.dim0`, a witness, an opened existential).
    pub fn atom(&self, name: &str) -> Option<&AtomInfo> {
        self.atoms.get(name)
    }

    /// The names whose value is read from the local `id` when it is bound.
    pub fn atoms_of(&self, id: LocalId) -> &[String] {
        self.atoms_by_local.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Every `let` whose type names dimensions, in source order.
    pub fn shapes(&self) -> &[ShapeRow] {
        &self.shapes
    }

    /// Fills each name's `origin_text` (`file:line`), which run-time errors print.
    pub fn label_origins(&mut self, label: impl Fn(Span) -> String) {
        for info in self.atoms.values_mut() {
            info.origin_text = label(info.origin);
        }
    }
}

/// `ty` as code generation sees it: every dimension known only at run time
/// (a name, an existential, an expression over names) is `Dyn`, so all of
/// them share one layout and one instance.
pub fn erase_symbolic(ty: &Type) -> Type {
    let symbolic = |name: &str| is_atom(name) || is_existential(name);
    match ty {
        Type::Generic(name) if symbolic(name) => Type::Dim(Dim::Dyn),
        Type::Dim(Dim::Const(expr)) if expr.any_name(symbolic) => Type::Dim(Dim::Dyn),
        Type::Struct(name, items) => Type::Struct(name.clone(), items.iter().map(erase_symbolic).collect()),
        Type::Enum(name, items) => Type::Enum(name.clone(), items.iter().map(erase_symbolic).collect()),
        Type::Tuple(items) => Type::Tuple(items.iter().map(erase_symbolic).collect()),
        Type::Pack(items) => Type::Pack(items.iter().map(erase_symbolic).collect()),
        Type::Borrow { mutable, ty } => Type::Borrow { mutable: *mutable, ty: Box::new(erase_symbolic(ty)) },
        Type::RawPointer { mutable, ty } => Type::RawPointer { mutable: *mutable, ty: Box::new(erase_symbolic(ty)) },
        Type::Slice(ty) => Type::Slice(Box::new(erase_symbolic(ty))),
        Type::Fn(params, ret) => Type::Fn(params.iter().map(erase_symbolic).collect(), Box::new(erase_symbolic(ret))),
        other => other.clone(),
    }
}

/// `ty` with every *anonymous* dimension name — one nobody has named yet,
/// via a witness or a claimed `x.dim(axis)` — decayed back to `Dyn`. Unlike
/// [`erase_symbolic`], a name the program went on to claim (so it now has a
/// real, deliberately-read value) is left alone: only names that are still
/// implicitly opened decay, matching the `named` module's own rule for when
/// a name counts as "not yet claimed by anything the program wrote".
pub fn erase_anonymous(ty: &Type) -> Type {
    named::erase_anonymous(ty)
}

pub fn bind_generics(template: &Type, concrete: &Type) -> HashMap<String, Type> {
    let mut substitutions = HashMap::new();
    unify_type(template, concrete, &mut substitutions);
    substitutions
}

/// Type-checks `module`, returning the same diagnostics as [`check_module`]
/// but also the inferred [`Type`] of every expression, via [`TypedModule`].
pub fn infer_module<'a>(
    module: &'a Module,
    reporter: &mut Reporter,
) -> Result<TypedModule<'a>, TypeError> {
    infer_module_with_imports(module, &[], reporter)
}

pub fn check_module(module: &Module, reporter: &mut Reporter) -> Result<(), TypeError> {
    infer_module(module, reporter).map(|_| ())
}

/// Same as [`infer_module`], but `imports` (`(qualifier, imported_module)`
/// pairs) seeds another file's `pub` items into this module's
/// environment — qualified for an explicit `use`, bare for an empty
/// qualifier.
pub fn infer_module_with_imports<'a>(
    module: &'a Module,
    imports: &[(String, &Module)],
    reporter: &mut Reporter,
) -> Result<TypedModule<'a>, TypeError> {
    let program = run_checks_with_imports(module, imports, reporter);

    if reporter.has_errors() {
        Err(TypeError)
    } else {
        let atoms = program.atoms.into_inner();
        let mut atoms_by_local: HashMap<LocalId, Vec<String>> = HashMap::new();
        let mut names: Vec<&String> = atoms.keys().collect();
        names.sort();
        for name in names {
            match &atoms[name].source {
                AtomSource::Value(id) | AtomSource::Extent { local: id, .. } => {
                    atoms_by_local.entry(*id).or_default().push(name.clone())
                }
                AtomSource::Param(_) | AtomSource::Unknown => {}
            }
        }
        let mut shapes = program.shapes.into_inner();
        shapes.sort_by_key(|row| (row.span.file_id(), row.span.start()));
        Ok(TypedModule {
            atoms,
            atoms_by_local,
            shapes,
            types: program.types.into_inner(),
            param_types: program.param_types.into_inner(),
            return_types: program.return_types.into_inner(),
            call_generics: program.call_generics.into_inner(),
            call_targets: program.call_targets.into_inner(),
            self_types: program.self_types.into_inner(),
            locals: program.locals,
            _module: PhantomData,
        })
    }
}

pub fn check_module_with_imports(
    module: &Module,
    imports: &[(String, &Module)],
    reporter: &mut Reporter,
) -> Result<(), TypeError> {
    infer_module_with_imports(module, imports, reporter).map(|_| ())
}

fn run_checks_with_imports(module: &Module, imports: &[(String, &Module)], reporter: &mut Reporter) -> Program {
    let program = Program::from_module_with_imports(module, imports, reporter);

    for item in &module.items {
        match item {
            Item::Fn(function) => {
                let Some(signature) = program.functions.get(&function.name) else {
                    continue;
                };
                check_function(function, &function.name, signature, &[], &program, reporter);
            }
            Item::Struct(decl) => {
                let self_ty = nominal_self_type(&decl.name, &decl.generics, matches!(item, Item::Enum(_)));
                for method in &decl.methods {
                    check_attached_function(method, &decl.name, &self_ty, &decl.generics, &program, reporter);
                }
            }
            Item::Enum(decl) => {
                let self_ty = nominal_self_type(&decl.name, &decl.generics, matches!(item, Item::Enum(_)));
                for method in &decl.methods {
                    check_attached_function(method, &decl.name, &self_ty, &decl.generics, &program, reporter);
                }
            }
            Item::Methods(block) => check_methods_block(block, &program, reporter),
            Item::Trait(_) | Item::Use(_) | Item::Const(_) | Item::Extern(_) => {}
        }
    }
    check_derivatives(module, &program, reporter);
    check_instantiation_limits(&program, reporter);
    for ty in program.types.borrow_mut().values_mut() {
        if has_unbound_variant_param(ty) {
            *ty = strip_variant_param_names(ty);
        }
    }

    program
}

fn strip_variant_param_names(ty: &Type) -> Type {
    match ty {
        Type::Generic(name) => Type::Generic(name.split('#').next().unwrap_or(name).to_string()),
        Type::Struct(name, args) => Type::Struct(name.clone(), args.iter().map(strip_variant_param_names).collect()),
        Type::Enum(name, args) => Type::Enum(name.clone(), args.iter().map(strip_variant_param_names).collect()),
        Type::Tuple(items) => Type::Tuple(items.iter().map(strip_variant_param_names).collect()),
        Type::Pack(items) => Type::Pack(items.iter().map(strip_variant_param_names).collect()),
        Type::Borrow { mutable, ty } => Type::Borrow { mutable: *mutable, ty: Box::new(strip_variant_param_names(ty)) },
        Type::RawPointer { mutable, ty } => {
            Type::RawPointer { mutable: *mutable, ty: Box::new(strip_variant_param_names(ty)) }
        }
        Type::Slice(ty) => Type::Slice(Box::new(strip_variant_param_names(ty))),
        other => other.clone(),
    }
}

fn pack_type(items: Vec<Type>) -> Type {
    match items.as_slice() {
        [Type::Spread(name)] => Type::Generic(name.clone()),
        _ => Type::Pack(items),
    }
}

fn is_concrete_const(ty: &Type) -> bool {
    match ty {
        Type::Generic(_) | Type::Spread(_) => false,
        Type::Dim(Dim::Const(expr)) => expr.as_lit().is_some(),
        Type::Dim(Dim::Dyn) => true,
        Type::Pack(items) => items.iter().all(is_concrete_const),
        _ => true,
    }
}

fn format_const_binding(consts: &BTreeMap<String, Type>) -> String {
    let parts: Vec<String> = consts
        .iter()
        .map(|(name, value)| match value {
            Type::Pack(_) => format!("{name} = [{}]", value.name()),
            _ => format!("{name} = {}", value.name()),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

fn check_instantiation_limits(program: &Program, reporter: &mut Reporter) {
    let edges = program.instantiations.borrow();
    let mut instances: HashMap<String, Vec<BTreeMap<String, Type>>> = HashMap::new();
    let mut exceeded: BTreeMap<String, Span> = BTreeMap::new();
    let mut worklist = VecDeque::new();
    let mut add = |callee: &str, consts: BTreeMap<String, Type>, span: Span, worklist: &mut VecDeque<_>| {
        let limit = program.instantiation_limits.get(callee).copied().unwrap_or(DEFAULT_INSTANTIATION_LIMIT);
        let seen = instances.entry(callee.to_string()).or_default();
        if seen.contains(&consts) || seen.len() > limit {
            return;
        }
        seen.push(consts.clone());
        if seen.len() > limit {
            exceeded.entry(callee.to_string()).or_insert(span);
            return;
        }
        worklist.push_back((callee.to_string(), consts));
    };
    for edge in edges.iter() {
        if edge.consts.values().all(is_concrete_const) {
            add(&edge.callee, edge.consts.clone(), edge.span, &mut worklist);
        }
    }
    while let Some((item, binding)) = worklist.pop_front() {
        let substitutions: HashMap<String, Type> = binding.into_iter().collect();
        for edge in edges.iter().filter(|edge| edge.caller == item) {
            let consts: BTreeMap<String, Type> = edge
                .consts
                .iter()
                .map(|(name, value)| (name.clone(), substitute_generics(value, &substitutions)))
                .collect();
            if consts.values().all(is_concrete_const) {
                add(&edge.callee, consts, edge.span, &mut worklist);
            }
        }
    }
    for (item, span) in exceeded {
        let bindings = &instances[&item];
        let limit = bindings.len() - 1;
        let listed = bindings.iter().map(format_const_binding).collect::<Vec<_>>().join(", ");
        reporter.push(
            Diagnostic::error(
                "PACO-E0337",
                span,
                format!(
                    "`{item}` exceeds its instantiation limit of {limit} distinct const arguments; \
                     instantiated with {listed}"
                ),
            )
            .with_note("raise the limit with `#[instantiation_limit(N)]` on the item, or use `Dyn` for dimensions that vary"),
        );
    }
}

fn record_call_generics(call: &Expr, generics: &[String], substitutions: &HashMap<String, Type>, program: &Program) {
    if generics.is_empty() {
        return;
    }
    let bound = generics
        .iter()
        .map(|name| substitutions.get(name).cloned().unwrap_or(Type::Generic(name.clone())))
        .collect();
    program.call_generics.borrow_mut().insert(call as *const Expr, bound);
}

fn record_instantiation(
    callee: &str,
    substitutions: &HashMap<String, Type>,
    span: Span,
    program: &Program,
    context: &FunctionContext<'_>,
) {
    let owner = callee.rsplit_once("::").and_then(|(owner, _)| program.structs.get(owner));
    let is_dim = |name: &str| {
        owner.is_some_and(|info| info.generics.iter().zip(&info.kinds).any(|(generic, kind)| generic == name && *kind == ParamKind::Dim))
            || program.own_generics.get(callee).is_some_and(|params| params.iter().any(|param| param.name == name && param.is_dim()))
    };
    let consts: BTreeMap<String, Type> = substitutions
        .iter()
        .filter(|(name, value)| {
            name.as_str() != "Self"
                && !is_dim(name)
                && match value {
                    Type::Dim(_) | Type::Pack(_) => true,
                    Type::Generic(param) => param != *name && context.const_values.contains_key(param),
                    _ => false,
                }
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    if !consts.is_empty() {
        program.instantiations.borrow_mut().push(InstantiationEdge {
            caller: context.item.clone(),
            callee: callee.to_string(),
            consts,
            span,
        });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Binding {
    ty: Type,
    mutable: bool,
    closure: Option<*const Expr>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParamKind {
    Type,
    Const,
    Pack,
    Dim,
}

fn param_kinds(params: &[ast::GenericParam]) -> Vec<ParamKind> {
    params
        .iter()
        .filter_map(|param| match param.kind {
            ast::GenericParamKind::Lifetime => None,
            ast::GenericParamKind::Type => Some(ParamKind::Type),
            ast::GenericParamKind::Const(_) => Some(ParamKind::Const),
            ast::GenericParamKind::ConstPack(_) => Some(ParamKind::Pack),
            ast::GenericParamKind::Dim => Some(ParamKind::Dim),
        })
        .collect()
}

#[derive(Clone, Debug)]
struct StructInfo {
    generics: Vec<String>,
    kinds: Vec<ParamKind>,
    fields: Vec<(String, Ty, Span)>,
    assoc: Vec<(String, Ty)>,
}

fn struct_assoc(decl: &ast::StructDecl) -> Vec<(String, Ty)> {
    decl.assoc_types.iter().filter_map(|assoc| Some((assoc.name.clone(), assoc.default.clone()?))).collect()
}

#[derive(Clone, Debug)]
struct EnumInfo {
    generics: Vec<String>,
    kinds: Vec<ParamKind>,
    variants: Vec<VariantInfo>,
}

#[derive(Clone, Debug)]
struct VariantInfo {
    name: String,
    fields: VariantFields,
    span: Span,
}

#[derive(Clone, Debug)]
struct FunctionSig {
    generics: Vec<String>,
    params: Vec<Type>,
    body_params: Vec<Type>,
    return_ty: Type,
    receiver: Option<Receiver>,
    requires_unsafe: bool,
    /// `phase-9-comptime` Decision 5: only callable while
    /// `FunctionContext::in_comptime` is set — checked the same way
    /// `requires_unsafe` gates a call needing `unsafe { .. }`.
    requires_comptime: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Receiver {
    mutable: bool,
}

#[derive(Clone, Debug)]
struct ConstInfo {
    ty: Type,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct TraitInfo {
    generics: Vec<String>,
    methods: HashSet<String>,
    assoc_types: HashSet<String>,
}

#[derive(Default)]
struct Program {
    /// User types marked `#[derive(Copy)]`, by plain and qualified name.
    copy_types: HashSet<String>,
    structs: HashMap<String, StructInfo>,
    enums: HashMap<String, EnumInfo>,
    traits: HashMap<String, TraitInfo>,
    functions: HashMap<String, FunctionSig>,
    extern_functions: HashSet<String>,
    iter_functions: HashSet<String>,
    methods: HashMap<(String, String), FunctionSig>,
    associated: HashMap<(String, String), FunctionSig>,
    consts: HashMap<String, ConstInfo>,
    assoc_consts: HashMap<(String, String), ConstInfo>,
    types: RefCell<HashMap<*const Expr, Type>>,
    param_types: RefCell<HashMap<*const Param, Type>>,
    return_types: RefCell<HashMap<*const FnDecl, Type>>,
    call_generics: RefCell<HashMap<*const Expr, Vec<Type>>>,
    call_targets: RefCell<HashMap<*const Expr, Type>>,
    self_types: RefCell<HashMap<*const FnDecl, Type>>,
    private_imports: HashSet<String>,
    instantiations: RefCell<Vec<InstantiationEdge>>,
    instantiation_limits: HashMap<String, usize>,
    bounds: HashMap<String, Vec<(String, Vec<String>)>>,
    struct_bounds: HashMap<String, Vec<(String, Vec<String>)>>,
    /// While an imported module's signatures are resolved, its own bare
    /// type names mean its qualified types.
    import_qualifier: RefCell<Option<String>>,
    differentiable: HashSet<String>,
    builtin_grad: HashSet<String>,
    /// Functions marked `#[builtin(assert)]`/`#[builtin(assert_eq)]`/etc.
    /// (`stdlib::test`'s nine assertion functions); the assert kind is the
    /// function's own bare name, so no separate enum is needed.
    builtin_test_asserts: HashSet<String>,
    locals: paco_resolve::Locals,
    atoms: RefCell<HashMap<String, AtomInfo>>,
    shapes: RefCell<Vec<ShapeRow>>,
    /// Each item's own generic parameters, by the key calls record it under.
    own_generics: HashMap<String, Vec<ast::GenericParam>>,
    /// `#[broadcasts(D, T)]`: the source and target packs of a method.
    broadcasts: HashMap<String, (String, String)>,
    /// Structs with a `?b` in a field type.
    existential_structs: HashSet<String>,
}

fn param_bounds<'p>(params: impl IntoIterator<Item = &'p ast::GenericParam>) -> Vec<(String, Vec<String>)> {
    params
        .into_iter()
        .filter(|param| !param.bounds.is_empty())
        .map(|param| {
            let traits = param
                .bounds
                .iter()
                .filter_map(|bound| match bound {
                    Ty::Path(path, _) | Ty::Generic { path, .. } => path.last().cloned(),
                    _ => None,
                })
                .collect();
            (param.name.clone(), traits)
        })
        .collect()
}

/// Whether `ty` satisfies `trait_name`. Primitives satisfy the operator
/// traits natively (FP8 has none); other types satisfy a declared trait
/// structurally, by having all of its methods.
/// Whether values of `ty` are duplicated rather than moved. `generic_copy`
/// says which type parameters in scope are bounded by `Copy`.
fn is_copy_type(ty: &Type, copy_types: &HashSet<String>, generic_copy: &dyn Fn(&str) -> bool) -> bool {
    match ty {
        Type::Int(_)
        | Type::Float(_)
        | Type::Bool
        | Type::Char
        | Type::Unit
        | Type::Never
        | Type::Borrow { mutable: false, .. }
        | Type::RawPointer { .. }
        | Type::Dim(_)
        | Type::Pack(_)
        | Type::Spread(_)
        | Type::Error
        | Type::Unknown => true,
        Type::Tuple(items) => items.iter().all(|item| is_copy_type(item, copy_types, generic_copy)),
        Type::Struct(name, args) | Type::Enum(name, args) => {
            copy_types.contains(name) && args.iter().all(|arg| is_copy_type(arg, copy_types, generic_copy))
        }
        Type::Generic(name) => generic_copy(name),
        _ => false,
    }
}

fn satisfies(ty: &Type, trait_name: &str, program: &Program, context: Option<&FunctionContext<'_>>) -> bool {
    if trait_name == "Copy" {
        let bounded = |name: &str| {
            context.and_then(|context| context.bounds.get(name)).is_some_and(|traits| traits.iter().any(|t| t == "Copy"))
        };
        return is_copy_type(ty, &program.copy_types, &bounded);
    }
    let arithmetic = |ty: &Type| match ty {
        Type::Int(_) => true,
        Type::Float(width) => width.has_arithmetic(),
        _ => false,
    };
    match ty {
        Type::Generic(name) => context
            .and_then(|context| context.bounds.get(name))
            .is_some_and(|traits| traits.iter().any(|t| t == trait_name)),
        Type::Error | Type::Unknown => true,
        Type::Int(_) | Type::Float(_) | Type::Bool | Type::Char | Type::String => match trait_name {
            "Numeric" => matches!(ty, Type::Int(_) | Type::Float(_)),
            "Float" | "Differentiable" => matches!(ty, Type::Float(width) if width.has_arithmetic()),
            "Add" | "Sub" | "Mul" | "Div" | "Rem" => arithmetic(ty),
            "Neg" => matches!(ty, Type::Int(width) if width.is_signed()) || matches!(ty, Type::Float(w) if w.has_arithmetic()),
            "Ord" => arithmetic(ty) || matches!(ty, Type::Char),
            "Hash" => !matches!(ty, Type::Float(_)),
            "Copy" => !matches!(ty, Type::String),
            _ => true,
        },
        Type::Struct(..) | Type::Enum(..) => {
            let Some(type_name) = target_type_name(ty) else { return true };
            match trait_name {
                "Numeric" | "Float" => false,
                _ => program.traits.get(trait_name).is_none_or(|info| {
                    let methods = info.methods.iter().all(|method| {
                        let key = (type_name.clone(), method.clone());
                        program.methods.contains_key(&key) || program.associated.contains_key(&key)
                    });
                    let declared = |assoc: &String| {
                        program.structs.get(&type_name).is_none_or(|decl| decl.assoc.iter().any(|(name, _)| name == assoc))
                    };
                    methods && (!matches!(trait_name, "Differentiable" | "Pullback") || info.assoc_types.iter().all(declared))
                }),
            }
        }
        _ => true,
    }
}

fn mutable_witness(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        "PACO-E0345",
        span,
        format!("`{name}` is bound by `let mut`, so it cannot name a dimension: its value could change"),
    )
    .with_note(format!("bind it immutably: `let {name} = ...;`"))
}

fn check_bounds(
    bounds: &[(String, Vec<String>)],
    substitutions: &HashMap<String, Type>,
    what: &str,
    span: Span,
    program: &Program,
    context: Option<&FunctionContext<'_>>,
    reporter: &mut Reporter,
) {
    for (param, traits) in bounds {
        let Some(ty) = substitutions.get(param) else { continue };
        if matches!(ty, Type::Generic(name) if name == param) || (context.is_none() && matches!(ty, Type::Generic(_))) {
            continue;
        }
        if let Some(trait_name) = traits.iter().find(|trait_name| !satisfies(ty, trait_name, program, context)) {
            reporter.push(Diagnostic::error(
                "PACO-E0340",
                span,
                format!("`{}` does not satisfy `{trait_name}`, required by `{param}` in `{what}`", ty.name()),
            ));
        }
    }
}

#[derive(Clone, Debug)]
struct InstantiationEdge {
    caller: String,
    callee: String,
    consts: BTreeMap<String, Type>,
    span: Span,
}

pub const DEFAULT_INSTANTIATION_LIMIT: usize = 256;

/// `stdlib::test`'s nine `#[builtin(name)]`-tagged assertion functions; each
/// name doubles as its own "assert kind" at a call site.
pub const TEST_ASSERT_BUILTINS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "assert_true",
    "assert_false",
    "assert_some",
    "assert_none",
    "assert_ok",
    "assert_err",
];

fn instantiation_limit(function: &FnDecl) -> Option<usize> {
    function.attrs.iter().find(|attr| attr.name == "instantiation_limit").and_then(|attr| match attr.args.first() {
        Some(ast::AttributeArg::Literal(Literal::Int(limit), _)) => usize::try_from(*limit).ok(),
        _ => None,
    })
}

impl Program {
    fn from_module_with_imports(module: &Module, imports: &[(String, &Module)], reporter: &mut Reporter) -> Self {
        let mut program = Self {
            locals: paco_resolve::resolve_locals(std::iter::once(module).chain(imports.iter().map(|(_, imported)| *imported))),
            ..Self::default()
        };
        program.register_prelude();
        for (qualifier, declared) in imports.iter().map(|(qualifier, imported)| (qualifier.as_str(), *imported)).chain([("", module)]) {
            for item in &declared.items {
                let (name, attrs) = match item {
                    Item::Struct(decl) => (&decl.name, &decl.attrs),
                    Item::Enum(decl) => (&decl.name, &decl.attrs),
                    _ => continue,
                };
                if ast::has_derive(attrs, "Copy") {
                    program.copy_types.insert(name.clone());
                    program.copy_types.insert(if qualifier.is_empty() { name.clone() } else { format!("{qualifier}::{name}") });
                }
            }
        }
        let mut imported_shapes = HashSet::new();
        for (qualifier, imported_module) in imports {
            program.import_type_shapes(imported_module, qualifier, &mut imported_shapes);
        }
        for (qualifier, imported_module) in imports {
            *program.import_qualifier.borrow_mut() = (!qualifier.is_empty()).then(|| qualifier.clone());
            program.import_functions_and_methods(imported_module, qualifier, &imported_shapes, &mut Reporter::new());
        }
        *program.import_qualifier.borrow_mut() = None;
        program.collect_types(module, &imported_shapes, reporter);
        program.collect_traits(module, reporter);
        for (qualifier, imported_module) in imports {
            program.import_traits(imported_module, qualifier);
        }
        program.validate_declared_types(module, reporter);
        program.collect_functions(module, reporter);
        program.collect_consts(module, reporter);
        program.check_recursive_value_layout(reporter);
        program
    }

    fn import_type_shapes(&mut self, module: &Module, qualifier: &str, imported_shapes: &mut HashSet<String>) {
        let key = |name: &str| if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") };
        for item in &module.items {
            match item {
                Item::Struct(decl) if decl.is_pub => {
                    let qualified = key(&decl.name);
                    if self.structs.contains_key(&qualified) || self.enums.contains_key(&qualified) {
                        continue;
                    }
                    let fields: Vec<_> = decl
                        .fields
                        .iter()
                        .map(|field| (field.name.clone(), field.ty.clone(), field.span))
                        .collect();
                    self.structs
                        .entry(decl.name.clone())
                        .or_insert_with(|| StructInfo { generics: ast::generic_names(&decl.generics), kinds: param_kinds(&decl.generics), fields: fields.clone(), assoc: struct_assoc(decl) });
                    if decl.fields.iter().any(|field| named::ty_has_existential(&field.ty)) {
                        self.existential_structs.insert(qualified.clone());
                        self.existential_structs.insert(decl.name.clone());
                    }
                    self.structs.insert(qualified.clone(), StructInfo { generics: ast::generic_names(&decl.generics), kinds: param_kinds(&decl.generics), fields, assoc: struct_assoc(decl) });
                    self.struct_bounds.insert(qualified.clone(), param_bounds(&decl.generics));
                    self.struct_bounds.entry(decl.name.clone()).or_insert_with(|| param_bounds(&decl.generics));
                    imported_shapes.insert(qualified);
                }
                Item::Enum(decl) if decl.is_pub => {
                    let qualified = key(&decl.name);
                    if self.structs.contains_key(&qualified) || self.enums.contains_key(&qualified) {
                        continue;
                    }
                    let variants: Vec<_> = decl
                        .variants
                        .iter()
                        .map(|variant| VariantInfo {
                            name: variant.name.clone(),
                            fields: variant.fields.clone(),
                            span: variant.span,
                        })
                        .collect();
                    self.enums
                        .entry(decl.name.clone())
                        .or_insert_with(|| EnumInfo { generics: ast::generic_names(&decl.generics), kinds: param_kinds(&decl.generics), variants: variants.clone() });
                    self.enums.insert(qualified.clone(), EnumInfo { generics: ast::generic_names(&decl.generics), kinds: param_kinds(&decl.generics), variants });
                    imported_shapes.insert(qualified);
                }
                Item::Struct(decl) if !decl.is_pub => {
                    self.private_imports.insert(key(&decl.name));
                }
                Item::Enum(decl) if !decl.is_pub => {
                    self.private_imports.insert(key(&decl.name));
                }
                Item::Fn(function) if !function.is_pub => {
                    self.private_imports.insert(key(&function.name));
                }
                _ => {}
            }
        }
    }

    fn import_functions_and_methods(
        &mut self,
        module: &Module,
        qualifier: &str,
        imported_shapes: &HashSet<String>,
        reporter: &mut Reporter,
    ) {
        let key = |name: &str| if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") };
        for item in &module.items {
            match item {
                Item::Struct(decl) if decl.is_pub && imported_shapes.contains(&key(&decl.name)) => {
                    let qualified = key(&decl.name);
                    let generics: Vec<Type> = ast::generic_names(&decl.generics).into_iter().map(Type::Generic).collect();
                    let self_ty = Type::Struct(qualified.clone(), generics.clone());
                    self.import_attached_functions(&qualified, self_ty, &decl.methods, &decl.generics, reporter);
                    if qualified != decl.name {
                        let bare_self_ty = Type::Struct(decl.name.clone(), generics);
                        self.import_attached_functions(&decl.name, bare_self_ty, &decl.methods, &decl.generics, reporter);
                    }
                }
                Item::Enum(decl) if decl.is_pub && imported_shapes.contains(&key(&decl.name)) => {
                    let qualified = key(&decl.name);
                    let generics: Vec<Type> = ast::generic_names(&decl.generics).into_iter().map(Type::Generic).collect();
                    let self_ty = Type::Enum(qualified.clone(), generics.clone());
                    self.import_attached_functions(&qualified, self_ty, &decl.methods, &decl.generics, reporter);
                    if qualified != decl.name {
                        let bare_self_ty = Type::Enum(decl.name.clone(), generics);
                        self.import_attached_functions(&decl.name, bare_self_ty, &decl.methods, &decl.generics, reporter);
                    }
                }
                Item::Fn(function) if function.is_pub => {
                    let qualified = key(&function.name);
                    if self.functions.contains_key(&qualified) {
                        continue;
                    }
                    let signature = self.function_sig(function, None, reporter);
                    if let Some(limit) = instantiation_limit(function) {
                        self.instantiation_limits.insert(qualified.clone(), limit);
                    }
                    self.record_bounds(qualified.clone(), &[], function);
                    self.register_function_attrs(qualified.clone(), function);
                    self.functions.insert(qualified, signature);
                }
                Item::Methods(block) => {
                    let target_ty = self.ty_from_ast(&block.target, &generic_substitutions(&ast::generic_names(&block.generics)), reporter);
                    if let Some(type_name) = target_type_name(&target_ty) {
                        let qualified = key(&type_name);
                        let qualified_ty = match &target_ty {
                            Type::Struct(_, args) if qualified != type_name => Some(Type::Struct(qualified.clone(), args.clone())),
                            Type::Enum(_, args) if qualified != type_name => Some(Type::Enum(qualified.clone(), args.clone())),
                            _ => None,
                        };
                        if let Some(qualified_ty) = qualified_ty {
                            self.import_attached_functions(&qualified, qualified_ty, &block.methods, &block.generics, reporter);
                        }
                        self.import_attached_functions(&type_name, target_ty, &block.methods, &block.generics, reporter);
                    }
                }
                _ => {}
            }
        }
    }

    fn import_attached_functions(
        &mut self,
        type_name: &str,
        self_ty: Type,
        methods: &[FnDecl],
        owner: &[ast::GenericParam],
        reporter: &mut Reporter,
    ) {
        for method in methods {
            self.record_bounds(format!("{type_name}::{}", method.name), owner, method);
            let signature = self.function_sig(method, Some(self_ty.clone()), reporter);
            if let Some(limit) = instantiation_limit(method) {
                self.instantiation_limits.insert(format!("{type_name}::{}", method.name), limit);
            }
            let key = (type_name.to_string(), method.name.clone());
            let target = if signature.receiver.is_some() {
                &mut self.methods
            } else {
                &mut self.associated
            };
            target.entry(key).or_insert(signature);
        }
    }

    fn is_private_import(&self, qualified_name: &str) -> bool {
        self.private_imports.contains(qualified_name)
    }

    /// ADR 0022: prelude types/functions with no source declaration.
    fn register_prelude(&mut self) {
        let root = Span::new_root(0, 0);
        for name in [
            "Sender", "Receiver", "JoinHandle", "Arc", "Generator", "Rc", "Cell", "RefCell", "Mutex", "RwLock",
        ] {
            self.structs.insert(
                name.to_string(),
                StructInfo {
                    generics: vec!["T".to_string()],
                    kinds: vec![ParamKind::Type],
                    fields: Vec::new(),
                    assoc: Vec::new(),
                },
            );
        }
        for name in ["SendError", "RecvError", "TcpListener", "TcpStream"] {
            self.structs.insert(
                name.to_string(),
                StructInfo { generics: Vec::new(), kinds: Vec::new(), fields: Vec::new(), assoc: Vec::new() },
            );
        }
        self.structs.insert(
            "TaskPanic".to_string(),
            StructInfo {
                generics: Vec::new(),
                kinds: Vec::new(),
                fields: vec![("message".to_string(), Ty::Path(vec!["string".to_string()], root), root)],
                assoc: Vec::new(),
            },
        );
        self.functions.insert(
            "channel".to_string(),
            FunctionSig {
                generics: vec!["T".to_string()],
                params: vec![Type::Int(IntWidth::I64)],
                body_params: vec![Type::Int(IntWidth::I64)],
                return_ty: Type::Tuple(vec![
                    Type::Struct("Sender".to_string(), vec![Type::Generic("T".to_string())]),
                    Type::Struct("Receiver".to_string(), vec![Type::Generic("T".to_string())]),
                ]),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        // Placeholder only: `[]T` has no literal-construction syntax yet
        // (design.md's Non-Goals — array/slice literal syntax is left to
        // whichever change adds it, most likely `phase-12-standard-
        // library`). This exists solely so `arrays-and-slices`' own
        // verification (type-checking, borrow-checking, codegen tests) can
        // construct a `[]T` value to index into at all. Not a considered
        // general construction API — delete or supersede it, don't build
        // on it as if it were designed.
        self.functions.insert(
            "hash_of".to_string(),
            FunctionSig {
                generics: vec!["T".to_string()],
                params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::Generic("T".to_string())) }],
                body_params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::Generic("T".to_string())) }],
                return_ty: Type::Int(IntWidth::U64),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.bounds.insert("hash_of".to_string(), vec![("T".to_string(), vec!["Hash".to_string()])]);
        let sorted = vec![
            Type::Borrow { mutable: true, ty: Box::new(Type::Slice(Box::new(Type::Generic("T".to_string())))) },
            Type::Int(IntWidth::I64),
        ];
        self.functions.insert(
            "slice_sort_native".to_string(),
            FunctionSig {
                generics: vec!["T".to_string()],
                params: sorted.clone(),
                body_params: sorted,
                return_ty: Type::Bool,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "slice_of_zeros".to_string(),
            FunctionSig {
                generics: vec!["T".to_string()],
                params: vec![Type::Int(IntWidth::I64)],
                body_params: vec![Type::Int(IntWidth::I64)],
                return_ty: Type::Slice(Box::new(Type::Generic("T".to_string()))),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        // `phase-9-comptime` Decision 6: type introspection. `FieldInfo`
        // is a real prelude struct (`stdlib/core/comptime.paco`, injected the
        // same way `Vec`/`Map`/etc. are, ADR 0022) — these two entries
        // only register the *builtins*' own signatures; `FieldInfo`'s own
        // shape is declared there, not here.
        self.functions.insert(
            "fields_of".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::TypeValue(Box::new(Type::Unknown))],
                body_params: vec![Type::TypeValue(Box::new(Type::Unknown))],
                return_ty: Type::Struct("FieldIter".to_string(), Vec::new()),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: true,
            },
        );
        self.functions.insert(
            "type_name".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::TypeValue(Box::new(Type::Unknown))],
                body_params: vec![Type::TypeValue(Box::new(Type::Unknown))],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: true,
            },
        );
        // `phase-9-comptime` Decision 7: renders a `Code`'s spliced content
        // back to Paco source text — needed to observe/compare `quote { .. }`
        // output (tasks 5.3/5.4's own tests) and, later, for diagnostics
        // that reference generated code.
        self.functions.insert(
            "code_to_string".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Code],
                body_params: vec![Type::Code],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: true,
            },
        );
        self.functions.insert(
            "string_concat".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![
                    Type::Borrow { mutable: false, ty: Box::new(Type::String) },
                    Type::Borrow { mutable: false, ty: Box::new(Type::String) },
                ],
                body_params: vec![
                    Type::Borrow { mutable: false, ty: Box::new(Type::String) },
                    Type::Borrow { mutable: false, ty: Box::new(Type::String) },
                ],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "uint_to_string".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Int(IntWidth::U64)],
                body_params: vec![Type::Int(IntWidth::U64)],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "int_to_string".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Int(IntWidth::I64)],
                body_params: vec![Type::Int(IntWidth::I64)],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "arg_count".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: Vec::new(),
                body_params: Vec::new(),
                return_ty: Type::Int(IntWidth::I64),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "arg_at".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Int(IntWidth::I64)],
                body_params: vec![Type::Int(IntWidth::I64)],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "bool_to_string".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Bool],
                body_params: vec![Type::Bool],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "float_to_string".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Float(FloatWidth::F64)],
                body_params: vec![Type::Float(FloatWidth::F64)],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "char_to_string".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Char],
                body_params: vec![Type::Char],
                return_ty: Type::String,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "string_len_bytes".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }],
                body_params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }],
                return_ty: Type::Int(IntWidth::I64),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "string_char_at".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }, Type::Int(IntWidth::I64)],
                body_params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }, Type::Int(IntWidth::I64)],
                return_ty: Type::Enum("Option".to_string(), vec![Type::Char]),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "string_next_char_boundary".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }, Type::Int(IntWidth::I64)],
                body_params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }, Type::Int(IntWidth::I64)],
                return_ty: Type::Int(IntWidth::I64),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "string_byte_at".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }, Type::Int(IntWidth::I64)],
                body_params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }, Type::Int(IntWidth::I64)],
                return_ty: Type::Enum("Option".to_string(), vec![Type::Int(IntWidth::I64)]),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        let string_ref = Type::Borrow { mutable: false, ty: Box::new(Type::String) };
        let bytes = Type::Slice(Box::new(Type::Int(IntWidth::U8)));
        let builtins = [
            ("string_to_bytes", vec![string_ref.clone()], bytes.clone()),
            (
                "string_from_bytes",
                vec![Type::Borrow { mutable: false, ty: Box::new(bytes.clone()) }, Type::Int(IntWidth::I64), Type::Int(IntWidth::I64)],
                Type::Enum("Option".to_string(), vec![Type::String]),
            ),
            (
                "bytes_write_string",
                vec![Type::Borrow { mutable: true, ty: Box::new(bytes) }, Type::Int(IntWidth::I64), string_ref.clone()],
                Type::Bool,
            ),
            ("string_hash", vec![string_ref], Type::Int(IntWidth::U64)),
        ];
        for (name, params, return_ty) in builtins {
            self.functions.insert(
                name.to_string(),
                FunctionSig {
                    generics: Vec::new(),
                    params: params.clone(),
                    body_params: params,
                    return_ty,
                    receiver: None,
                    requires_unsafe: false,
                    requires_comptime: false,
                },
            );
        }
        self.functions.insert(
            "string_slice_utf8".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::String, Type::Int(IntWidth::I64), Type::Int(IntWidth::I64)],
                body_params: vec![Type::String, Type::Int(IntWidth::I64), Type::Int(IntWidth::I64)],
                return_ty: Type::Enum("Option".to_string(), vec![Type::String]),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "stderr_write".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::String],
                body_params: vec![Type::String],
                return_ty: Type::Unit,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "fs_read_to_string".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }],
                body_params: vec![Type::Borrow { mutable: false, ty: Box::new(Type::String) }],
                return_ty: Type::Enum("Option".to_string(), vec![Type::String]),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "slice_as_ptr".to_string(),
            FunctionSig {
                generics: vec!["T".to_string()],
                params: vec![Type::Borrow {
                    mutable: false,
                    ty: Box::new(Type::Slice(Box::new(Type::Generic("T".to_string())))),
                }],
                body_params: vec![Type::Borrow {
                    mutable: false,
                    ty: Box::new(Type::Slice(Box::new(Type::Generic("T".to_string())))),
                }],
                return_ty: Type::RawPointer { mutable: false, ty: Box::new(Type::Generic("T".to_string())) },
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "slice_as_mut_ptr".to_string(),
            FunctionSig {
                generics: vec!["T".to_string()],
                params: vec![Type::Borrow {
                    mutable: true,
                    ty: Box::new(Type::Slice(Box::new(Type::Generic("T".to_string())))),
                }],
                body_params: vec![Type::Borrow {
                    mutable: true,
                    ty: Box::new(Type::Slice(Box::new(Type::Generic("T".to_string())))),
                }],
                return_ty: Type::RawPointer { mutable: true, ty: Box::new(Type::Generic("T".to_string())) },
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.functions.insert(
            "tcp_listen".to_string(),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Int(IntWidth::I64)],
                body_params: vec![Type::Int(IntWidth::I64)],
                return_ty: Type::Struct("TcpListener".to_string(), Vec::new()),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.methods.insert(
            ("TcpListener".to_string(), "accept".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: Vec::new(),
                body_params: vec![Type::Struct("TcpListener".to_string(), Vec::new())],
                return_ty: Type::Struct("TcpStream".to_string(), Vec::new()),
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.methods.insert(
            ("TcpStream".to_string(), "read".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Int(IntWidth::I64)],
                body_params: vec![Type::Struct("TcpStream".to_string(), Vec::new()), Type::Int(IntWidth::I64)],
                return_ty: Type::String,
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.methods.insert(
            ("TcpStream".to_string(), "write".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::String],
                body_params: vec![Type::Struct("TcpStream".to_string(), Vec::new()), Type::String],
                return_ty: Type::Unit,
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );

        let sender_self = Type::Struct("Sender".to_string(), vec![Type::Generic("T".to_string())]);
        let receiver_self =
            Type::Struct("Receiver".to_string(), vec![Type::Generic("T".to_string())]);
        self.methods.insert(
            ("Sender".to_string(), "send".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Generic("T".to_string())],
                body_params: vec![sender_self.clone(), Type::Generic("T".to_string())],
                return_ty: Type::Enum(
                    "Result".to_string(),
                    vec![Type::Unit, Type::Struct("SendError".to_string(), Vec::new())],
                ),
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.methods.insert(
            ("Sender".to_string(), "close".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: Vec::new(),
                body_params: vec![sender_self],
                return_ty: Type::Unit,
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.methods.insert(
            ("Receiver".to_string(), "recv".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: Vec::new(),
                body_params: vec![receiver_self],
                return_ty: Type::Enum(
                    "Result".to_string(),
                    vec![
                        Type::Generic("T".to_string()),
                        Type::Struct("RecvError".to_string(), Vec::new()),
                    ],
                ),
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.methods.insert(
            ("JoinHandle".to_string(), "join".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: Vec::new(),
                body_params: vec![Type::Struct(
                    "JoinHandle".to_string(),
                    vec![Type::Generic("T".to_string())],
                )],
                return_ty: Type::Enum(
                    "Result".to_string(),
                    vec![
                        Type::Generic("T".to_string()),
                        Type::Struct("TaskPanic".to_string(), Vec::new()),
                    ],
                ),
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.methods.insert(
            ("Generator".to_string(), "next".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: Vec::new(),
                body_params: vec![Type::Struct(
                    "Generator".to_string(),
                    vec![Type::Generic("T".to_string())],
                )],
                return_ty: Type::Enum(
                    "Option".to_string(),
                    vec![Type::Generic("T".to_string())],
                ),
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.register_shared_cell("Rc", "get", None, true);
        self.register_shared_cell("Arc", "get", None, true);
        self.register_shared_cell("Cell", "get", Some("set"), false);
        self.register_shared_cell("RefCell", "get", Some("set"), false);
        self.register_shared_cell("Mutex", "lock", Some("set"), false);
        self.register_shared_cell("RwLock", "read", Some("write"), false);
        // `phase-9-comptime` Decision 7: concatenates a dynamically-built
        // list of expression-shaped `Code` fragments, joined by `sep` as
        // raw code between each pair, into one `Code`.
        self.associated.insert(
            ("Code".to_string(), "join".to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: vec![Type::Struct("Vec".to_string(), vec![Type::Code]), Type::String],
                body_params: vec![Type::Struct("Vec".to_string(), vec![Type::Code]), Type::String],
                return_ty: Type::Code,
                receiver: None,
                requires_unsafe: false,
                requires_comptime: true,
            },
        );
    }

    fn register_shared_cell(
        &mut self,
        name: &str,
        read_method: &str,
        write_method: Option<&str>,
        has_clone_and_count: bool,
    ) {
        let self_ty = Type::Struct(name.to_string(), vec![Type::Generic("T".to_string())]);
        self.associated.insert(
            (name.to_string(), "new".to_string()),
            FunctionSig {
                generics: vec!["T".to_string()],
                params: vec![Type::Generic("T".to_string())],
                body_params: vec![Type::Generic("T".to_string())],
                return_ty: self_ty.clone(),
                receiver: None,
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        self.methods.insert(
            (name.to_string(), read_method.to_string()),
            FunctionSig {
                generics: Vec::new(),
                params: Vec::new(),
                body_params: vec![self_ty.clone()],
                return_ty: Type::Generic("T".to_string()),
                receiver: Some(Receiver { mutable: false }),
                requires_unsafe: false,
                requires_comptime: false,
            },
        );
        if let Some(write_method) = write_method {
            self.methods.insert(
                (name.to_string(), write_method.to_string()),
                FunctionSig {
                    generics: Vec::new(),
                    params: vec![Type::Generic("T".to_string())],
                    body_params: vec![self_ty.clone(), Type::Generic("T".to_string())],
                    return_ty: Type::Unit,
                    receiver: Some(Receiver { mutable: false }),
                    requires_unsafe: false,
                    requires_comptime: false,
                },
            );
        }
        if has_clone_and_count {
            self.methods.insert(
                (name.to_string(), "clone".to_string()),
                FunctionSig {
                    generics: Vec::new(),
                    params: Vec::new(),
                    body_params: vec![self_ty.clone()],
                    return_ty: self_ty.clone(),
                    receiver: Some(Receiver { mutable: false }),
                    requires_unsafe: false,
                    requires_comptime: false,
                },
            );
            self.methods.insert(
                (name.to_string(), "strong_count".to_string()),
                FunctionSig {
                    generics: Vec::new(),
                    params: Vec::new(),
                    body_params: vec![self_ty],
                    return_ty: Type::Int(IntWidth::I64),
                    receiver: Some(Receiver { mutable: false }),
                    requires_unsafe: false,
                    requires_comptime: false,
                },
            );
        }
    }

    fn collect_consts(&mut self, module: &Module, reporter: &mut Reporter) {
        for item in &module.items {
            match item {
                Item::Const(decl) => self.collect_one_const(decl, None, reporter),
                Item::Struct(decl) => {
                    for constant in &decl.consts {
                        self.collect_one_const(constant, Some(&decl.name), reporter);
                    }
                }
                Item::Enum(decl) => {
                    for constant in &decl.consts {
                        self.collect_one_const(constant, Some(&decl.name), reporter);
                    }
                }
                Item::Methods(block) => {
                    let target_ty = self.ty_from_ast(&block.target, &generic_substitutions(&ast::generic_names(&block.generics)), reporter);
                    if let Some(type_name) = target_type_name(&target_ty) {
                        for constant in &block.consts {
                            self.collect_one_const(constant, Some(&type_name), reporter);
                        }
                    }
                }
                Item::Fn(_) | Item::Trait(_) | Item::Use(_) | Item::Extern(_) => {}
            }
        }
    }

    fn collect_one_const(&mut self, decl: &ConstDecl, owner: Option<&str>, reporter: &mut Reporter) {
        let key = owner.map(|owner| (owner.to_string(), decl.name.clone()));
        let is_duplicate = match &key {
            None => self.consts.contains_key(&decl.name),
            Some(key) => self.assoc_consts.contains_key(key),
        };
        if is_duplicate {
            reporter.push(Diagnostic::error(
                "PACO-E0317",
                decl.span,
                format!("duplicate constant `{}`", decl.name),
            ));
            return;
        }

        let declared_ty = self.ty_from_ast(&decl.ty, &HashMap::new(), reporter);

        let mut context = FunctionContext {
            scopes: Vec::new(),
            expected_return: &Type::Unit,
            loop_depth: 0,
            in_unsafe: false,
            in_iter: false,
            in_comptime: false,
            generics: HashMap::new(),
            const_values: HashMap::new(),
            item: decl.name.clone(),
            bounds: HashMap::new(),
            closure_params: HashMap::new(),
            unresolved_closure_params: Vec::new(),
            dims: Default::default(),
        };
        let actual_ty =
            infer_literal_against_expected(&decl.value, Some(&declared_ty), self, &mut context, reporter);
        if !compatible(&actual_ty, &declared_ty) {
            reporter.push(mismatch_diagnostic(decl.span, "", &declared_ty, &actual_ty));
        }
        if !self.is_evaluable_at_compile_time(&decl.value) {
            reporter.push(Diagnostic::error(
                "PACO-E0318",
                decl.span,
                "const initializer is not evaluable at compile time",
            ));
        }

        let info = ConstInfo { ty: declared_ty };
        match key {
            None => {
                self.consts.insert(decl.name.clone(), info);
            }
            Some(key) => {
                self.assoc_consts.insert(key, info);
            }
        }
    }

    fn is_evaluable_at_compile_time(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Literal(_, _) => true,
            Expr::Unary { expr, .. } => self.is_evaluable_at_compile_time(expr),
            Expr::Binary { left, right, .. } => {
                self.is_evaluable_at_compile_time(left) && self.is_evaluable_at_compile_time(right)
            }
            Expr::Ident(name, _) => self.consts.contains_key(name),
            Expr::Comptime { .. } => true,
            _ => false,
        }
    }

    fn collect_types(&mut self, module: &Module, imported_shapes: &HashSet<String>, reporter: &mut Reporter) {
        for item in &module.items {
            match item {
                Item::Struct(decl) => {
                    if (self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name))
                        && !imported_shapes.contains(&decl.name)
                    {
                        reporter.push(Diagnostic::error(
                            "PACO-E0310",
                            decl.span,
                            format!("duplicate type `{}`", decl.name),
                        ));
                        continue;
                    }
                    let mut seen = HashSet::new();
                    let mut fields = Vec::new();
                    for field in &decl.fields {
                        if !seen.insert(field.name.clone()) {
                            reporter.push(Diagnostic::error(
                                "PACO-E0311",
                                field.span,
                                format!("duplicate field `{}`", field.name),
                            ));
                        }
                        fields.push((field.name.clone(), field.ty.clone(), field.span));
                    }
                    if decl.fields.iter().any(|field| named::ty_has_existential(&field.ty)) {
                        self.existential_structs.insert(decl.name.clone());
                    }
                    self.structs.insert(
                        decl.name.clone(),
                        StructInfo {
                            generics: ast::generic_names(&decl.generics),
                            kinds: param_kinds(&decl.generics),
                            fields,
                            assoc: struct_assoc(decl),
                        },
                    );
                    self.struct_bounds.insert(decl.name.clone(), param_bounds(&decl.generics));
                }
                Item::Enum(decl) => {
                    if (self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name))
                        && !imported_shapes.contains(&decl.name)
                    {
                        reporter.push(Diagnostic::error(
                            "PACO-E0310",
                            decl.span,
                            format!("duplicate type `{}`", decl.name),
                        ));
                        continue;
                    }
                    let mut seen = HashSet::new();
                    let mut variants = Vec::new();
                    for variant in &decl.variants {
                        if !seen.insert(variant.name.clone()) {
                            reporter.push(Diagnostic::error(
                                "PACO-E0313",
                                variant.span,
                                format!("duplicate enum variant `{}`", variant.name),
                            ));
                        }
                        variants.push(VariantInfo {
                            name: variant.name.clone(),
                            fields: variant.fields.clone(),
                            span: variant.span,
                        });
                    }
                    self.enums.insert(
                        decl.name.clone(),
                        EnumInfo {
                            generics: ast::generic_names(&decl.generics),
                            kinds: param_kinds(&decl.generics),
                            variants,
                        },
                    );
                }
                _ => {}
            }
        }
    }

    fn collect_traits(&mut self, module: &Module, reporter: &mut Reporter) {
        for item in &module.items {
            let Item::Trait(decl) = item else { continue };
            if self.structs.contains_key(&decl.name)
                || self.enums.contains_key(&decl.name)
                || self.traits.contains_key(&decl.name)
            {
                reporter.push(Diagnostic::error(
                    "PACO-E0310",
                    decl.span,
                    format!("duplicate type `{}`", decl.name),
                ));
                continue;
            }

            let mut methods = HashSet::new();
            for method in &decl.methods {
                if !methods.insert(method.name.clone()) {
                    reporter.push(Diagnostic::error(
                        "PACO-E0320",
                        method.span,
                        format!("duplicate method `{}` in trait `{}`", method.name, decl.name),
                    ));
                }
            }

            let mut assoc_types = HashSet::new();
            for assoc_type in &decl.assoc_types {
                if !assoc_types.insert(assoc_type.name.clone()) {
                    reporter.push(Diagnostic::error(
                        "PACO-E0321",
                        assoc_type.span,
                        format!(
                            "duplicate associated type `{}` in trait `{}`",
                            assoc_type.name, decl.name
                        ),
                    ));
                }
            }

            self.traits.insert(
                decl.name.clone(),
                TraitInfo {
                    generics: ast::generic_names(&decl.generics),
                    methods,
                    assoc_types,
                },
            );
        }
    }

    /// An imported module's public traits, so bounds naming them are checked;
    /// a trait the importing module declares itself shadows them.
    fn import_traits(&mut self, module: &Module, qualifier: &str) {
        for item in &module.items {
            let Item::Trait(decl) = item else { continue };
            if !decl.is_pub {
                continue;
            }
            let info = TraitInfo {
                generics: ast::generic_names(&decl.generics),
                methods: decl.methods.iter().map(|method| method.name.clone()).collect(),
                assoc_types: decl.assoc_types.iter().map(|assoc| assoc.name.clone()).collect(),
            };
            if !qualifier.is_empty() {
                self.traits.entry(format!("{qualifier}::{}", decl.name)).or_insert_with(|| info.clone());
            }
            self.traits.entry(decl.name.clone()).or_insert(info);
        }
    }

    fn collect_functions(&mut self, module: &Module, reporter: &mut Reporter) {
        for item in &module.items {
            match item {
                Item::Fn(function) => {
                    if function.is_iter {
                        self.iter_functions.insert(function.name.clone());
                    }
                    let signature = self.function_sig(function, None, reporter);
                    self.functions.insert(function.name.clone(), signature);
                    self.record_bounds(function.name.clone(), &[], function);
                    self.register_function_attrs(function.name.clone(), function);
                    if let Some(limit) = instantiation_limit(function) {
                        self.instantiation_limits.insert(function.name.clone(), limit);
                    }
                }
                Item::Struct(decl) => self.collect_attached_functions(decl, reporter),
                Item::Enum(decl) => self.collect_enum_functions(decl, reporter),
                Item::Methods(block) => self.collect_extension_methods(block, reporter),
                Item::Extern(block) => self.collect_extern_functions(block, reporter),
                Item::Trait(_) | Item::Use(_) | Item::Const(_) => {}
            }
        }
    }

    fn collect_extern_functions(&mut self, block: &ExternBlock, reporter: &mut Reporter) {
        for function in &block.functions {
            self.extern_functions.insert(function.name.clone());
            let params = function
                .params
                .iter()
                .map(|param| self.ty_from_ast(&param.ty, &HashMap::new(), reporter))
                .collect::<Vec<_>>();
            let return_ty = function
                .return_ty
                .as_ref()
                .map_or(Type::Unit, |ty| self.ty_from_ast(ty, &HashMap::new(), reporter));
            self.functions.insert(
                function.name.clone(),
                FunctionSig {
                    generics: ast::generic_names(&function.generics),
                    body_params: params.clone(),
                    params,
                    return_ty,
                    receiver: None,
                    requires_unsafe: true,
                    requires_comptime: false,
                },
            );
        }
    }

    fn check_derived_copy(&self, attrs: &[ast::Attribute], generics: &[ast::GenericParam], fields: &[(String, Type, Span)], reporter: &mut Reporter) {
        if !ast::has_derive(attrs, "Copy") {
            return;
        }
        let params: HashSet<String> = ast::generic_names(generics).into_iter().collect();
        if let Some((name, ty, span)) = fields.iter().find(|(_, ty, _)| !is_copy_type(ty, &self.copy_types, &|name| params.contains(name))) {
            reporter.push(
                Diagnostic::error(
                    "PACO-E0354",
                    *span,
                    format!("cannot derive `Copy`: field `{name}` has type `{}`, which is not `Copy`", ty.name()),
                )
                .with_note("a `Copy` type is duplicated bit for bit, so every field must be `Copy` too"),
            );
        }
    }

    fn validate_declared_types(&self, module: &Module, reporter: &mut Reporter) {
        for item in &module.items {
            match item {
                Item::Struct(decl) => {
                    let env = generic_substitutions(&ast::generic_names(&decl.generics));
                    let fields: Vec<(String, Type, Span)> =
                        decl.fields.iter().map(|field| (field.name.clone(), self.ty_from_ast(&field.ty, &env, reporter), field.span)).collect();
                    self.check_derived_copy(&decl.attrs, &decl.generics, &fields, reporter);
                }
                Item::Enum(decl) => {
                    let env = generic_substitutions(&ast::generic_names(&decl.generics));
                    let mut payloads = Vec::new();
                    for variant in &decl.variants {
                        match &variant.fields {
                            VariantFields::Unit => {}
                            VariantFields::Tuple(types) => {
                                for (index, ty) in types.iter().enumerate() {
                                    let resolved = self.ty_from_ast(ty, &env, reporter);
                                    payloads.push((format!("{}.{index}", variant.name), resolved, ty_span(ty)));
                                }
                            }
                            VariantFields::Struct(fields) => {
                                for field in fields {
                                    let resolved = self.ty_from_ast(&field.ty, &env, reporter);
                                    payloads.push((format!("{}.{}", variant.name, field.name), resolved, field.span));
                                }
                            }
                        }
                    }
                    self.check_derived_copy(&decl.attrs, &decl.generics, &payloads, reporter);
                }
                Item::Fn(_) | Item::Methods(_) | Item::Trait(_) | Item::Use(_) | Item::Const(_) | Item::Extern(_) => {}
            }
        }
    }

    fn collect_attached_functions(&mut self, decl: &StructDecl, reporter: &mut Reporter) {
        let self_ty = Type::Struct(
            decl.name.clone(),
            ast::generic_names(&decl.generics)
                .into_iter()
                .map(Type::Generic)
                .collect(),
        );
        for method in &decl.methods {
            self.record_bounds(format!("{}::{}", decl.name, method.name), &decl.generics, method);
            let signature = self.function_sig(method, Some(self_ty.clone()), reporter);
            self.insert_attached_signature(&decl.name, method, signature, reporter);
        }
    }

    fn register_function_attrs(&mut self, key: String, function: &FnDecl) {
        if is_differentiable(function) {
            self.differentiable.insert(key.clone());
        }
        let builtin_grad = function.attrs.iter().any(|attr| {
            attr.name == "builtin" && matches!(attr.args.first(), Some(ast::AttributeArg::Path(path, _)) if path == &["grad"])
        });
        if builtin_grad {
            self.builtin_grad.insert(key.clone());
        }
        let builtin_test_assert = function.attrs.iter().any(|attr| {
            attr.name == "builtin"
                && matches!(attr.args.first(), Some(ast::AttributeArg::Path(path, _)) if path.len() == 1 && TEST_ASSERT_BUILTINS.contains(&path[0].as_str()))
        });
        if builtin_test_assert {
            self.builtin_test_asserts.insert(key);
        }
    }

    fn record_bounds(&mut self, key: String, owner: &[ast::GenericParam], function: &FnDecl) {
        self.own_generics.insert(key.clone(), function.generics.clone());
        if let Some(attr) = function.attrs.iter().find(|attr| attr.name == "broadcasts")
            && let [ast::AttributeArg::Path(source, _), ast::AttributeArg::Path(target, _)] = attr.args.as_slice()
            && let ([source], [target]) = (source.as_slice(), target.as_slice())
        {
            self.broadcasts.insert(key.clone(), (source.clone(), target.clone()));
        }
        let bounds = param_bounds(owner.iter().chain(&function.generics));
        if !bounds.is_empty() {
            self.bounds.insert(key, bounds);
        }
    }

    fn collect_enum_functions(&mut self, decl: &EnumDecl, reporter: &mut Reporter) {
        let self_ty = Type::Enum(
            decl.name.clone(),
            ast::generic_names(&decl.generics)
                .into_iter()
                .map(Type::Generic)
                .collect(),
        );
        for method in &decl.methods {
            self.record_bounds(format!("{}::{}", decl.name, method.name), &decl.generics, method);
            let signature = self.function_sig(method, Some(self_ty.clone()), reporter);
            self.insert_attached_signature(&decl.name, method, signature, reporter);
        }
    }

    fn collect_extension_methods(&mut self, block: &MethodsBlock, reporter: &mut Reporter) {
        let target_ty = self.ty_from_ast(&block.target, &generic_substitutions(&ast::generic_names(&block.generics)), reporter);
        let Some(target_name) = target_type_name(&target_ty) else {
            reporter.push(Diagnostic::error(
                "PACO-E0310",
                ty_span(&block.target),
                "methods block target must be a known nominal type",
            ));
            return;
        };
        for method in &block.methods {
            self.record_bounds(format!("{target_name}::{}", method.name), &block.generics, method);
            let signature = self.function_sig(method, Some(target_ty.clone()), reporter);
            self.insert_attached_signature(&target_name, method, signature, reporter);
        }
    }

    fn insert_attached_signature(
        &mut self,
        type_name: &str,
        function: &FnDecl,
        signature: FunctionSig,
        reporter: &mut Reporter,
    ) {
        if let Some(limit) = instantiation_limit(function) {
            self.instantiation_limits.insert(format!("{type_name}::{}", function.name), limit);
        }
        let key = (type_name.to_string(), function.name.clone());
        let target = if signature.receiver.is_some() {
            &mut self.methods
        } else {
            &mut self.associated
        };
        match target.entry(key) {
            Entry::Occupied(_) => {
                reporter.push(Diagnostic::error(
                    "PACO-E0314",
                    function.span,
                    format!("duplicate method `{}` for `{type_name}`", function.name),
                ));
            }
            Entry::Vacant(entry) => {
                entry.insert(signature);
            }
        }
    }

    fn function_sig(
        &self,
        function: &FnDecl,
        self_ty: Option<Type>,
        reporter: &mut Reporter,
    ) -> FunctionSig {
        let mut env = HashMap::new();
        for generic in ast::generic_names(&function.generics) {
            env.insert(generic.clone(), Type::Generic(generic));
        }
        if let Some(self_ty) = &self_ty {
            env.insert("Self".to_string(), self_ty.clone());
            for arg in nominal_generic_args(self_ty) {
                let items = match arg {
                    Type::Pack(items) => items.as_slice(),
                    other => std::slice::from_ref(other),
                };
                for item in items {
                    if let Type::Generic(name) | Type::Spread(name) = item {
                        env.insert(name.clone(), Type::Generic(name.clone()));
                    }
                }
            }
        }

        let mut rigid = named::dim_param_names(&function.generics);
        if let Some(Type::Struct(name, args)) = &self_ty
            && let Some(info) = self.structs.get(name)
        {
            for (arg, kind) in args.iter().zip(&info.kinds) {
                if let (Type::Generic(param), ParamKind::Dim) = (arg, kind) {
                    rigid.insert(param.clone());
                }
            }
        }
        let previous_rigid = named::set_rigid(rigid);
        let receiver = function.params.first().and_then(receiver_from_param);
        let body_params = function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                if index == 0 && receiver.is_some() {
                    self_ty.clone().unwrap_or(Type::Error)
                } else {
                    self.ty_from_ast(&param.ty, &env, reporter)
                }
            })
            .collect::<Vec<_>>();
        let params = if receiver.is_some() {
            body_params.iter().skip(1).cloned().collect()
        } else {
            body_params.clone()
        };
        let return_ty = function
            .return_ty
            .as_ref()
            .map_or(Type::Unit, |ty| self.ty_from_ast(ty, &env, reporter));
        named::set_rigid(previous_rigid);

        FunctionSig {
            params,
            body_params,
            return_ty,
            receiver,
            generics: ast::generic_names(&function.generics),
            requires_unsafe: function.is_unsafe,
            requires_comptime: function.is_comptime,
        }
    }

    fn ty_from_ast(
        &self,
        ty: &Ty,
        generics: &HashMap<String, Type>,
        reporter: &mut Reporter,
    ) -> Type {
        self.ty_from_ast_omitted(ty, generics, false, reporter)
    }

    fn ty_from_ast_omitted(
        &self,
        ty: &Ty,
        generics: &HashMap<String, Type>,
        allow_omitted: bool,
        reporter: &mut Reporter,
    ) -> Type {
        match ty {
            Ty::Path(path, span) if path.as_slice() == ["int"] => {
                reporter.push(Diagnostic::error(
                    "PACO-E0328",
                    *span,
                    "`int` is not a Paco type; use `i64`",
                ));
                Type::Error
            }
            Ty::Path(path, span) if path.as_slice() == ["uint"] => {
                reporter.push(Diagnostic::error(
                    "PACO-E0328",
                    *span,
                    "`uint` is not a Paco type; use `u64`",
                ));
                Type::Error
            }
            Ty::Path(path, _) if path.as_slice() == ["i8"] => Type::Int(IntWidth::I8),
            Ty::Path(path, _) if path.as_slice() == ["i16"] => Type::Int(IntWidth::I16),
            Ty::Path(path, _) if path.as_slice() == ["i32"] => Type::Int(IntWidth::I32),
            Ty::Path(path, _) if path.as_slice() == ["i64"] => Type::Int(IntWidth::I64),
            Ty::Path(path, _) if path.as_slice() == ["u8"] || path.as_slice() == ["byte"] => {
                Type::Int(IntWidth::U8)
            }
            Ty::Path(path, _) if path.as_slice() == ["u16"] => Type::Int(IntWidth::U16),
            Ty::Path(path, _) if path.as_slice() == ["u32"] => Type::Int(IntWidth::U32),
            Ty::Path(path, _) if path.as_slice() == ["u64"] => Type::Int(IntWidth::U64),
            Ty::Path(path, _) if path.as_slice() == ["char"] => Type::Char,
            Ty::Path(path, _) if path.len() == 1 && FloatWidth::from_name(&path[0]).is_some() => {
                Type::Float(FloatWidth::from_name(&path[0]).expect("checked"))
            }
            Ty::Path(path, _) if path.as_slice() == ["bool"] => Type::Bool,
            Ty::Path(path, _) if path.as_slice() == ["string"] => Type::String,
            Ty::Path(path, _) if path.as_slice() == ["type"] => {
                Type::TypeValue(Box::new(Type::Unknown))
            }
            // `Code` (`phase-9-comptime` Decision 7) is otherwise only
            // ever an *inferred* type (a `quote { .. }` expression's own
            // result) — this one spelling, `Code::join`'s receiver
            // position, is the sole place it's written as an annotation.
            Ty::Path(path, _) if path.as_slice() == ["Code"] => Type::Code,
            Ty::Borrow { mutable, ty, .. } => Type::Borrow {
                mutable: *mutable,
                ty: Box::new(self.ty_from_ast_omitted(ty, generics, allow_omitted, reporter)),
            },
            Ty::Slice(ty, _) => Type::Slice(Box::new(
                self.ty_from_ast_omitted(ty, generics, allow_omitted, reporter),
            )),
            Ty::Tuple(items, _) if items.is_empty() => Type::Unit,
            Ty::Tuple(items, _) => Type::Tuple(
                items
                    .iter()
                    .map(|item| self.ty_from_ast_omitted(item, generics, allow_omitted, reporter))
                    .collect(),
            ),
            Ty::RawPointer { mutable, ty, .. } => Type::RawPointer {
                mutable: *mutable,
                ty: Box::new(self.ty_from_ast_omitted(ty, generics, allow_omitted, reporter)),
            },
            Ty::Fn { params, return_ty, .. } => Type::Fn(
                params.iter().map(|param| self.ty_from_ast_omitted(param, generics, allow_omitted, reporter)).collect(),
                Box::new(match return_ty {
                    Some(ret) => self.ty_from_ast_omitted(ret, generics, allow_omitted, reporter),
                    None => Type::Unit,
                }),
            ),
            Ty::Path(path, span) => {
                let key = self.resolve_type_key(path);
                if path.len() == 1
                    && let Some(ty) = generics.get(&path[0])
                {
                    ty.clone()
                } else if let Some(info) = self.structs.get(&key) {
                    if !info.generics.is_empty() && !allow_omitted {
                        self.check_generic_arity(&key, info.generics.len(), 0, *span, reporter);
                    }
                    let args = info.generics.iter().map(|g| Type::Generic(g.clone())).collect();
                    Type::Struct(key, args)
                } else if let Some(info) = self.enums.get(&key) {
                    if !info.generics.is_empty() && !allow_omitted {
                        self.check_generic_arity(&key, info.generics.len(), 0, *span, reporter);
                    }
                    let args = info.generics.iter().map(|g| Type::Generic(g.clone())).collect();
                    Type::Enum(key, args)
                } else {
                    reporter.push(Diagnostic::error(
                        "PACO-E0306",
                        ty_span(ty),
                        format!("type is not supported yet: {}", path.join("::")),
                    ));
                    Type::Error
                }
            }
            Ty::Generic { path, args, .. } => {
                let key = self.resolve_type_key(path);
                if let Some(info) = self.structs.get(&key) {
                    let args = self.generic_args_from_ast(&key, &info.kinds, args, generics, allow_omitted, ty_span(ty), reporter);
                    Type::Struct(key, args)
                } else if let Some(info) = self.enums.get(&key) {
                    let args = self.generic_args_from_ast(&key, &info.kinds, args, generics, allow_omitted, ty_span(ty), reporter);
                    Type::Enum(key, args)
                } else {
                    reporter.push(Diagnostic::error(
                        "PACO-E0306",
                        ty_span(ty),
                        format!("type is not supported yet: {}", path.join("::")),
                    ));
                    Type::Error
                }
            }
            _ => {
                reporter.push(Diagnostic::error(
                    "PACO-E0306",
                    ty_span(ty),
                    "type is not supported yet",
                ));
                Type::Error
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generic_args_from_ast(
        &self,
        key: &str,
        kinds: &[ParamKind],
        args: &[Ty],
        generics: &HashMap<String, Type>,
        allow_omitted: bool,
        span: Span,
        reporter: &mut Reporter,
    ) -> Vec<Type> {
        let has_pack = kinds.last() == Some(&ParamKind::Pack);
        let fixed = if has_pack { kinds.len() - 1 } else { kinds.len() };
        if has_pack {
            if args.len() < fixed {
                reporter.push(Diagnostic::error(
                    "PACO-E0316",
                    span,
                    format!("generic arity mismatch for `{key}`: expected at least {fixed}, found {}", args.len()),
                ));
            }
        } else {
            self.check_generic_arity(key, kinds.len(), args.len(), span, reporter);
        }
        let mut out = Vec::with_capacity(kinds.len());
        for (arg, kind) in args.iter().zip(kinds).take(fixed) {
            out.push(match kind {
                ParamKind::Const => {
                    let dimension = self.const_arg_from_ast(arg, generics, reporter);
                    if named::is_named(&dimension) {
                        reporter.push(Diagnostic::error(
                            "PACO-E0347",
                            ty_span(arg),
                            format!(
                                "a `const` parameter of `{key}` cannot take the run-time dimension `{}`; declare the parameter `dim`",
                                dimension.name()
                            ),
                        ));
                    }
                    dimension
                }
                ParamKind::Dim => self.const_arg_from_ast(arg, generics, reporter),
                _ => self.type_arg_from_ast(arg, generics, allow_omitted, reporter),
            });
        }
        if let (Some(bounds), Some(info)) = (self.struct_bounds.get(key), self.structs.get(key)) {
            let substitutions: HashMap<String, Type> = info.generics.iter().cloned().zip(out.iter().cloned()).collect();
            check_bounds(bounds, &substitutions, key, span, self, None, reporter);
        }
        if has_pack {
            let rest = args.get(fixed..).unwrap_or(&[]);
            let mut pack = Vec::with_capacity(rest.len());
            for (index, arg) in rest.iter().enumerate() {
                match arg {
                    Ty::Expand(name, span) if index + 1 == rest.len() => match generics.get(name) {
                        Some(Type::Generic(param)) => pack.push(Type::Spread(param.clone())),
                        Some(Type::Pack(items)) => pack.extend(items.iter().cloned()),
                        _ => {
                            reporter.push(Diagnostic::error(
                                "PACO-E0338",
                                *span,
                                format!("`{name}...` does not name a const parameter pack in scope"),
                            ));
                            pack.push(Type::Error);
                        }
                    },
                    other => pack.push(self.const_arg_from_ast(other, generics, reporter)),
                }
            }
            out.push(pack_type(pack));
        }
        out
    }

    fn type_arg_from_ast(
        &self,
        arg: &Ty,
        generics: &HashMap<String, Type>,
        allow_omitted: bool,
        reporter: &mut Reporter,
    ) -> Type {
        if matches!(arg, Ty::Const(..) | Ty::DynDim(_) | Ty::Expand(..)) {
            reporter.push(Diagnostic::error(
                "PACO-E0338",
                ty_span(arg),
                "expected a type argument, found a constant dimension",
            ));
            return Type::Error;
        }
        self.ty_from_ast_omitted(arg, generics, allow_omitted, reporter)
    }

    fn const_arg_from_ast(&self, arg: &Ty, generics: &HashMap<String, Type>, reporter: &mut Reporter) -> Type {
        match arg {
            Ty::DynDim(_) => Type::Dim(Dim::Dyn),
            Ty::Const(expr, _) => match self.const_expr_from_ast(expr, generics, reporter) {
                Some(Dim::Const(expr)) => dim_type(Dim::Const(expr)),
                Some(Dim::Dyn) => Type::Dim(Dim::Dyn),
                None => Type::Error,
            },
            Ty::Path(path, span) if path.len() == 1 => match generics.get(&path[0]) {
                Some(bound @ (Type::Generic(_) | Type::Dim(_))) => bound.clone(),
                Some(Type::Error) => Type::Error,
                Some(Type::Unknown) => {
                    reporter.push(mutable_witness(&path[0], *span));
                    Type::Error
                }
                _ => {
                    reporter.push(Diagnostic::error(
                        "PACO-E0338",
                        *span,
                        format!("expected a constant dimension or `Dyn`, found `{}`", path[0]),
                    ));
                    Type::Error
                }
            },
            Ty::Existential(name, _) => Type::Generic(format!("?{name}")),
            Ty::Expand(name, span) => {
                reporter.push(Diagnostic::error(
                    "PACO-E0338",
                    *span,
                    format!("`{name}...` may only appear last, in a const parameter pack position"),
                ));
                Type::Error
            }
            other => {
                reporter.push(Diagnostic::error(
                    "PACO-E0338",
                    ty_span(other),
                    "expected a constant dimension or `Dyn`, found a type",
                ));
                Type::Error
            }
        }
    }

    fn const_expr_from_ast(&self, expr: &Expr, generics: &HashMap<String, Type>, reporter: &mut Reporter) -> Option<Dim> {
        let binary = |op: BinaryOp, left: ConstExpr, right: ConstExpr| -> Option<ConstExpr> {
            let op = match op {
                BinaryOp::Add => DimOp::Add,
                BinaryOp::Mul => DimOp::Mul,
                BinaryOp::Sub => DimOp::Sub,
                BinaryOp::Div => DimOp::Div,
                BinaryOp::Rem => DimOp::Rem,
                _ => return None,
            };
            Some(ConstExpr::binary(op, &left, &right))
        };
        let unsupported = |reporter: &mut Reporter, span: Span| {
            reporter.push(Diagnostic::error(
                "PACO-E0338",
                span,
                "a const generic argument must be an integer expression over literals and const parameters",
            ));
        };
        match expr {
            Expr::Literal(Literal::Int(value), _) => Some(Dim::Const(ConstExpr::lit(*value))),
            Expr::Ident(name, span) => match generics.get(name) {
                Some(Type::Generic(param)) => Some(Dim::Const(ConstExpr::param(param))),
                Some(Type::Dim(dim)) => Some(dim.clone()),
                Some(Type::Unknown) => {
                    reporter.push(mutable_witness(name, *span));
                    None
                }
                _ => {
                    reporter.push(Diagnostic::error(
                        "PACO-E0338",
                        *span,
                        format!("`{name}` is not a const generic parameter in scope"),
                    ));
                    None
                }
            },
            Expr::Unary { op: UnaryOp::Neg, expr: inner, span } => match self.const_expr_from_ast(inner, generics, reporter)? {
                Dim::Const(inner) => Some(Dim::Const(ConstExpr::neg(&inner))),
                Dim::Dyn => {
                    unsupported(reporter, *span);
                    None
                }
            },
            Expr::Binary { op, left, right, span } => {
                let left = self.const_expr_from_ast(left, generics, reporter)?;
                let right = self.const_expr_from_ast(right, generics, reporter)?;
                match (left, right) {
                    (Dim::Const(left), Dim::Const(right)) => match binary(*op, left, right) {
                        Some(expr) => Some(Dim::Const(expr)),
                        None => {
                            unsupported(reporter, *span);
                            None
                        }
                    },
                    _ => Some(Dim::Dyn),
                }
            }
            other => {
                unsupported(reporter, expr_span(other));
                None
            }
        }
    }

    fn check_generic_arity(
        &self,
        name: &str,
        expected: usize,
        actual: usize,
        span: Span,
        reporter: &mut Reporter,
    ) {
        if expected != actual {
            reporter.push(Diagnostic::error(
                "PACO-E0316",
                span,
                format!("generic arity mismatch for `{name}`: expected {expected}, found {actual}"),
            ));
        }
    }

    fn check_recursive_value_layout(&self, reporter: &mut Reporter) {
        for name in self.structs.keys() {
            let mut visiting = HashSet::new();
            let current = Type::Struct(name.clone(), Vec::new());
            if let Some(span) = self.find_recursive_value_field(name, &current, &mut visiting) {
                reporter.push(Diagnostic::error(
                    "PACO-E0315",
                    span,
                    format!("recursive by-value field in `{name}`"),
                ));
            }
        }
    }

    fn find_recursive_value_field(
        &self,
        root: &str,
        current: &Type,
        visiting: &mut HashSet<String>,
    ) -> Option<Span> {
        let Type::Struct(current_name, _) = current else {
            return None;
        };
        let key = current.name();
        if !visiting.insert(key.clone()) {
            return None;
        }
        let info = self.structs.get(current_name)?;
        let generics = self.layout_env(current);
        for (_, field_ty, span) in &info.fields {
            let field_ty = self.ty_from_ast_for_layout(field_ty, &generics);
            let Type::Struct(field_name, _) = &field_ty else {
                continue;
            };
            if field_name == root {
                visiting.remove(&key);
                return Some(*span);
            }
            if self.structs.contains_key(field_name)
                && let Some(span) = self.find_recursive_value_field(root, &field_ty, visiting)
            {
                visiting.remove(&key);
                return Some(span);
            }
        }
        visiting.remove(&key);
        None
    }

    fn layout_env(&self, ty: &Type) -> HashMap<String, Type> {
        let Type::Struct(name, args) = ty else {
            return HashMap::new();
        };
        let Some(info) = self.structs.get(name) else {
            return HashMap::new();
        };
        info.generics
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect()
    }

    /// `stdlib::core` is the only source of the prelude (`paco-driver` always
    /// loads it from exactly one place), so a `core::Name` path and its
    /// bare, prelude-injected `Name` always name the same declaration.
    /// Preferring the bare key here — when it actually resolves — is what
    /// makes `use stdlib::core; core::Option<i64>` and unqualified
    /// `Option<i64>` the same nominal type instead of two separate ones.
    fn resolve_type_key(&self, path: &[String]) -> String {
        if let [name] = path
            && let Some(qualifier) = self.import_qualifier.borrow().as_deref()
        {
            let qualified = format!("{qualifier}::{name}");
            if self.structs.contains_key(&qualified) || self.enums.contains_key(&qualified) {
                return qualified;
            }
        }
        if path.first().is_some_and(|segment| segment == "core") {
            let bare = path[1..].join("::");
            if self.structs.contains_key(&bare) || self.enums.contains_key(&bare) {
                return bare;
            }
        }
        path.join("::")
    }

    fn ty_from_ast_for_layout(&self, ty: &Ty, generics: &HashMap<String, Type>) -> Type {
        match ty {
            Ty::Path(path, _) => {
                let key = self.resolve_type_key(path);
                if path.len() == 1
                    && let Some(ty) = generics.get(&path[0])
                {
                    ty.clone()
                } else if self.structs.contains_key(&key) {
                    Type::Struct(key, Vec::new())
                } else if self.enums.contains_key(&key) {
                    Type::Enum(key, Vec::new())
                } else {
                    Type::Unknown
                }
            }
            Ty::Generic { path, args, .. } => {
                let key = self.resolve_type_key(path);
                let args = args
                    .iter()
                    .map(|arg| self.ty_from_ast_for_layout(arg, generics))
                    .collect::<Vec<_>>();
                if self.structs.contains_key(&key) {
                    Type::Struct(key, args)
                } else if self.enums.contains_key(&key) {
                    Type::Enum(key, args)
                } else {
                    Type::Unknown
                }
            }
            _ => Type::Unknown,
        }
    }
}

struct FunctionContext<'a> {
    scopes: Vec<HashMap<LocalId, Binding>>,
    expected_return: &'a Type,
    loop_depth: usize,
    in_unsafe: bool,
    in_iter: bool,
    /// Whether type-checking is currently inside a `comptime { .. }`
    /// block or a `comptime fn`'s own body (`phase-9-comptime` Decision
    /// 5) — gates whether a `comptime fn` call is allowed
    /// (`require_comptime`).
    in_comptime: bool,
    generics: HashMap<String, Type>,
    const_values: HashMap<String, Type>,
    item: String,
    bounds: HashMap<String, Vec<String>>,
    closure_params: HashMap<*const Expr, Vec<Type>>,
    unresolved_closure_params: Vec<Span>,
    dims: named::DimState,
}

fn check_attached_function(
    function: &FnDecl,
    type_name: &str,
    self_ty: &Type,
    owner_params: &[ast::GenericParam],
    program: &Program,
    reporter: &mut Reporter,
) {
    program.self_types.borrow_mut().insert(function as *const FnDecl, self_ty.clone());
    let key = (type_name.to_string(), function.name.clone());
    let signature = program
        .methods
        .get(&key)
        .or_else(|| program.associated.get(&key));
    if let Some(signature) = signature {
        check_function(function, &format!("{type_name}::{}", function.name), signature, owner_params, program, reporter);
    }
}

fn is_differentiable(function: &FnDecl) -> bool {
    function.attrs.iter().any(|attr| attr.name == "differentiable")
}

/// The `Differentiable` items `ty` lacks: `type Tangent`, `zero_tangent`,
/// `move_by`. Float types with arithmetic have them natively.
fn missing_differentiable_items(ty: &Type, program: &Program) -> Vec<&'static str> {
    let Type::Struct(name, _) = ty else { return vec!["Tangent", "zero_tangent", "move_by"] };
    let mut missing = Vec::new();
    if !program.structs.get(name).is_some_and(|info| info.assoc.iter().any(|(assoc, _)| assoc == "Tangent")) {
        missing.push("Tangent");
    }
    for method in ["zero_tangent", "move_by"] {
        if !program.methods.contains_key(&(name.clone(), method.to_string())) {
            missing.push(method);
        }
    }
    missing
}

/// `dims` names the dimension parameters in scope: they are never
/// differentiable inputs, only positions of a differentiable value's type.
fn is_differentiable_type(ty: &Type, float_params: &HashSet<String>, program: &Program) -> bool {
    match ty {
        Type::Float(width) => width.has_arithmetic(),
        Type::Generic(name) => float_params.contains(name),
        Type::Borrow { ty, .. } => is_differentiable_type(ty, float_params, program),
        Type::Tuple(items) => !items.is_empty() && items.iter().all(|item| is_differentiable_type(item, float_params, program)),
        Type::Struct(..) => missing_differentiable_items(ty, program).is_empty(),
        _ => false,
    }
}

fn differentiable_type_note(ty: &Type, program: &Program) -> String {
    let inner = match ty {
        Type::Borrow { ty, .. } => ty,
        other => other,
    };
    match inner {
        Type::Struct(..) => format!(
            "; `{}` does not satisfy `stdlib::autodiff::Differentiable`: it has no {}",
            inner.name(),
            missing_differentiable_items(inner, program).iter().map(|item| format!("`{item}`")).collect::<Vec<_>>().join(", ")
        ),
        _ => String::new(),
    }
}

fn check_differentiable_signature(
    function: &FnDecl,
    signature: &FunctionSig,
    owner_params: &[ast::GenericParam],
    program: &Program,
    reporter: &mut Reporter,
) {
    let float_params: HashSet<String> = owner_params
        .iter()
        .chain(&function.generics)
        .filter(|param| {
            param.bounds.iter().any(|bound| {
                matches!(bound, Ty::Path(path, _) if path.last().is_some_and(|name| name == "Float" || name == "Differentiable"))
            })
        })
        .map(|param| param.name.clone())
        .collect();
    let mut in_out = Vec::new();
    for (param, ty) in function.params.iter().zip(&signature.body_params) {
        let name = match &param.pattern {
            Pat::Ident(name, _) => name.clone(),
            _ => "_".to_string(),
        };
        if !is_differentiable_type(ty, &float_params, program) {
            reporter.push(Diagnostic::error(
                "PACO-E0341",
                param.span,
                format!(
                    "`#[differentiable]` function `{}`: parameter `{name}` has type `{}`; only floating-point scalars and `Differentiable` types (or borrows and tuples of them) can be differentiated{}",
                    function.name,
                    ty.name(),
                    differentiable_type_note(ty, program)
                ),
            ));
        } else if matches!(ty, Type::Borrow { mutable: true, .. }) {
            in_out.push(name);
        }
    }
    if signature.return_ty == Type::Unit {
        if in_out.len() != 1 {
            let found = if in_out.is_empty() {
                "it has none".to_string()
            } else {
                format!("it has {}", in_out.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(" and "))
            };
            reporter.push(Diagnostic::error(
                "PACO-E0813",
                function.span,
                format!(
                    "`#[differentiable]` function `{}` returns `()`, so its result is the final value of its one differentiable `&mut` parameter, but {found}",
                    function.name
                ),
            ));
        }
    } else if !is_differentiable_type(&signature.return_ty, &float_params, program) {
        reporter.push(Diagnostic::error(
            "PACO-E0341",
            function.span,
            format!(
                "`#[differentiable]` function `{}` returns `{}`; it must return a floating-point scalar or a `Differentiable` type (or a tuple of them){}",
                function.name,
                signature.return_ty.name(),
                differentiable_type_note(&signature.return_ty, program)
            ),
        ));
    }
}

/// `f`'s path in `#[derivative(of = f)]`, with the attribute's span.
pub fn derivative_of(function: &FnDecl) -> Option<(Vec<String>, Span)> {
    let attr = function.attrs.iter().find(|attr| attr.name == "derivative")?;
    match attr.args.as_slice() {
        [ast::AttributeArg::Assign(key, path, _)] if key == "of" => Some((path.clone(), attr.span)),
        _ => Some((Vec::new(), attr.span)),
    }
}

fn signature_text(params: &[Type], result: &Type) -> String {
    format!("({}) -> {}", params.iter().map(Type::name).collect::<Vec<_>>().join(", "), result.name())
}

fn struct_assoc_type(ty: &Type, name: &str, program: &Program) -> Option<Type> {
    let Type::Struct(struct_name, _) = ty else { return None };
    let (_, assoc) = program.structs.get(struct_name)?.assoc.iter().find(|(assoc, _)| assoc == name)?;
    Some(instantiate_ty(assoc, ty, program, &mut Reporter::new()))
}

/// `#[derivative(of = f)] fn d(<f's params>) -> (<f's result>, P)`, with
/// `P: Pullback<Seed = <f's result tangent>, Gradients = <f's parameter tangents>>`.
fn check_derivatives(module: &Module, program: &Program, reporter: &mut Reporter) {
    let mut registered: HashMap<String, Span> = HashMap::new();
    for item in &module.items {
        let Item::Fn(function) = item else { continue };
        let Some((path, span)) = derivative_of(function) else { continue };
        let mut fail = |message: String| reporter.push(Diagnostic::error("PACO-E0814", span, message));
        if path.is_empty() {
            fail("`#[derivative]` needs the function it differentiates: `#[derivative(of = f)]`".to_string());
            continue;
        }
        let key = path.join("::");
        // `of` names either a free function (`program.functions`) or, when
        // `path`'s last segment is a method and the rest a type path, a
        // method (`program.methods`) — its receiver, if any, counts as its
        // first parameter via `body_params`.
        let of = program.functions.get(&key).or_else(|| {
            let (method_name, type_path) = path.split_last()?;
            if type_path.is_empty() {
                return None;
            }
            let type_key = program.resolve_type_key(type_path);
            program.methods.get(&(type_key, method_name.clone()))
        });
        let (Some(of), Some(own)) = (of, program.functions.get(&function.name)) else {
            fail(format!("`#[derivative(of = {key})]`: no function or method named `{key}` is in scope"));
            continue;
        };
        let of_params: Vec<Type> = if of.receiver.is_some() { of.body_params.clone() } else { of.params.clone() };
        if registered.insert(key.clone(), span).is_some() {
            fail(format!("`{key}` already has a `#[derivative]`; a function has at most one"));
            continue;
        }
        if own.params != of_params {
            fail(format!(
                "`#[derivative(of = {key})]` function `{}` has signature `{}`, but `{key}` has `{}`; the parameters must be identical",
                function.name,
                signature_text(&own.params, &own.return_ty),
                signature_text(&of_params, &of.return_ty)
            ));
            continue;
        }
        let pullback = match &own.return_ty {
            Type::Tuple(items) if items.len() == 2 && items[0] == of.return_ty => items[1].clone(),
            other => {
                fail(format!(
                    "`#[derivative(of = {key})]` function `{}` returns `{}`; it must return `({}, P)` where `P` satisfies `stdlib::autodiff::Pullback`",
                    function.name,
                    other.name(),
                    of.return_ty.name()
                ));
                continue;
            }
        };
        let has_method = matches!(&pullback, Type::Struct(name, _) if program.methods.contains_key(&(name.clone(), "pullback".to_string())));
        let (Some(seed), Some(gradients), true) = (
            struct_assoc_type(&pullback, "Seed", program),
            struct_assoc_type(&pullback, "Gradients", program),
            has_method,
        ) else {
            fail(format!(
                "`{}` does not satisfy `stdlib::autodiff::Pullback`: it needs `type Seed`, `type Gradients` and `fn pullback(self, seed: Self::Seed) -> Self::Gradients`",
                pullback.name()
            ));
            continue;
        };
        let expected_seed = gradient_type(&of.return_ty, program);
        let tangents: Vec<Type> = of_params
            .iter()
            .filter(|param| is_differentiable_type(param, &HashSet::new(), program))
            .map(|param| gradient_type(param, program))
            .collect();
        let expected_gradients = match tangents.as_slice() {
            [single] => single.clone(),
            _ => Type::Tuple(tangents),
        };
        if seed != expected_seed || gradients != expected_gradients {
            fail(format!(
                "`{}`'s `Seed = {}` and `Gradients = {}` must be `Seed = {}` (the tangent of `{key}`'s result) and `Gradients = {}` (the tangents of its differentiable parameters)",
                pullback.name(),
                seed.name(),
                gradients.name(),
                expected_seed.name(),
                expected_gradients.name()
            ));
        }
    }
}

/// `ty`'s `Differentiable::Tangent`: itself for a float, the declared
/// `type Tangent` for a struct.
fn gradient_type(ty: &Type, program: &Program) -> Type {
    match ty {
        Type::Borrow { ty, .. } => gradient_type(ty, program),
        Type::Tuple(items) => Type::Tuple(items.iter().map(|item| gradient_type(item, program)).collect()),
        Type::Struct(name, _) => program
            .structs
            .get(name)
            .and_then(|info| info.assoc.iter().find(|(assoc, _)| assoc == "Tangent"))
            .map(|(_, tangent)| instantiate_ty(tangent, ty, program, &mut Reporter::new()))
            .unwrap_or_else(|| ty.clone()),
        other => other.clone(),
    }
}

/// `grad(f, inputs) -> (output, gradients)`: `inputs` is `f`'s argument or a
/// tuple of its arguments; each gradient has its input's type.
fn infer_grad_call(
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let [function, inputs] = args else {
        reporter.push(Diagnostic::error(
            "PACO-E0305",
            span,
            format!("`grad` expects 2 arguments (a function and its inputs), found {}", args.len()),
        ));
        return Type::Error;
    };
    let target = match function {
        Expr::Ident(name, _) => program.functions.get(name).map(|signature| (name, signature)),
        _ => None,
    };
    let Some((name, signature)) = target else {
        reporter.push(Diagnostic::error("PACO-E0341", expr_span(function), "`grad` expects the name of a `#[differentiable]` function"));
        return Type::Error;
    };
    if !program.differentiable.contains(name) {
        reporter.push(Diagnostic::error(
            "PACO-E0341",
            expr_span(function),
            format!("`{name}` is not `#[differentiable]`; mark it to take its gradient"),
        ));
        return Type::Error;
    }
    let function_ty = Type::Fn(signature.params.clone(), Box::new(signature.return_ty.clone()));
    program.types.borrow_mut().insert(function as *const Expr, function_ty);
    let expected_inputs = match signature.params.as_slice() {
        [single] => single.clone(),
        many => Type::Tuple(many.to_vec()),
    };
    let actual_inputs = infer_expr(inputs, program, context, reporter);
    let mut substitutions = generic_substitutions(&signature.generics);
    if !unify_type(&expected_inputs, &actual_inputs, &mut substitutions) {
        reporter.push(mismatch_diagnostic(span, "", &expected_inputs, &actual_inputs));
        return Type::Error;
    }
    record_call_generics(function, &signature.generics, &substitutions, program);
    let output = substitute_generics(&signature.return_ty, &substitutions);
    let gradients = gradient_type(&substitute_generics(&expected_inputs, &substitutions), program);
    let output = if output == Type::Unit {
        signature
            .params
            .iter()
            .find_map(|param| match param {
                Type::Borrow { mutable: true, ty } => Some(substitute_generics(ty, &substitutions)),
                _ => None,
            })
            .unwrap_or(Type::Unit)
    } else {
        output
    };
    if !matches!(&output, Type::Float(_) | Type::Generic(_) | Type::Error) {
        reporter.push(Diagnostic::error(
            "PACO-E0813",
            expr_span(function),
            format!(
                "`grad` needs a floating-point scalar result, but the result of `{name}` is `{}`; return a scalar computed from it",
                output.name()
            ),
        ));
        return Type::Tuple(vec![Type::Error, gradients]);
    }
    Type::Tuple(vec![output, gradients])
}

/// The call-site replacement for any of `stdlib::test`'s nine
/// `#[builtin(name)]` assertion functions: `stdlib/test.paco`'s written
/// signature declares a required `message: string` (ordinary Paco has no
/// optional-parameter syntax), but a real call may omit it, exactly as
/// `grad`'s call sites already bypass its own written signature. The
/// function's own bare name is its "assert kind". Always returns `Unit`.
fn infer_test_assert_call(
    name: &str,
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let kind = name.rsplit("::").next().unwrap_or(name);
    let arity = if matches!(kind, "assert_eq" | "assert_ne") { 2 } else { 1 };
    if args.len() != arity && args.len() != arity + 1 {
        reporter.push(Diagnostic::error(
            "PACO-E0305",
            span,
            format!("`{kind}` expects {arity} or {} arguments, found {}", arity + 1, args.len()),
        ));
        return Type::Unit;
    }
    let (main_args, message) = args.split_at(arity);
    if let [message] = message {
        let actual = infer_expr(message, program, context, reporter);
        if actual != Type::String {
            reporter.push(mismatch_diagnostic(expr_span(message), "", &Type::String, &actual));
        }
    }
    match kind {
        "assert" | "assert_true" | "assert_false" => {
            let actual = infer_expr(&main_args[0], program, context, reporter);
            if actual != Type::Bool {
                reporter.push(mismatch_diagnostic(expr_span(&main_args[0]), "", &Type::Bool, &actual));
            }
        }
        "assert_eq" | "assert_ne" => {
            let left = infer_expr(&main_args[0], program, context, reporter);
            let right = infer_expr(&main_args[1], program, context, reporter);
            let mut substitutions = HashMap::new();
            if !unify_type(&left, &right, &mut substitutions) {
                reporter.push(mismatch_diagnostic(span, "", &left, &right));
            }
        }
        "assert_some" | "assert_none" => {
            let actual = infer_expr(&main_args[0], program, context, reporter);
            if !matches!(&actual, Type::Enum(enum_name, _) if enum_name == "Option") {
                let expected = Type::Enum("Option".to_string(), vec![Type::Generic("T".to_string())]);
                reporter.push(mismatch_diagnostic(expr_span(&main_args[0]), "", &expected, &actual));
            }
        }
        "assert_ok" | "assert_err" => {
            let actual = infer_expr(&main_args[0], program, context, reporter);
            if !matches!(&actual, Type::Enum(enum_name, _) if enum_name == "Result") {
                let expected = Type::Enum("Result".to_string(), vec![Type::Generic("T".to_string()), Type::Generic("E".to_string())]);
                reporter.push(mismatch_diagnostic(expr_span(&main_args[0]), "", &expected, &actual));
            }
        }
        _ => unreachable!("only the nine known builtin-test names reach `infer_test_assert_call`"),
    }
    Type::Unit
}

fn nominal_self_type(name: &str, params: &[ast::GenericParam], is_enum: bool) -> Type {
    let args = ast::generic_names(params).into_iter().map(Type::Generic).collect();
    if is_enum { Type::Enum(name.to_string(), args) } else { Type::Struct(name.to_string(), args) }
}

fn const_value_types(params: &[ast::GenericParam]) -> impl Iterator<Item = (String, Type)> + '_ {
    params.iter().filter_map(|param| match param.kind {
        ast::GenericParamKind::Const(_) | ast::GenericParamKind::Dim => Some((param.name.clone(), Type::Int(IntWidth::I64))),
        ast::GenericParamKind::ConstPack(_) => {
            Some((param.name.clone(), Type::Slice(Box::new(Type::Int(IntWidth::I64)))))
        }
        _ => None,
    })
}

fn check_methods_block(block: &MethodsBlock, program: &Program, reporter: &mut Reporter) {
    let target_ty = program.ty_from_ast(&block.target, &generic_substitutions(&ast::generic_names(&block.generics)), reporter);
    let Some(type_name) = target_type_name(&target_ty) else {
        return;
    };
    let mut owner_params = block.generics.clone();
    for arg in nominal_generic_args(&target_ty) {
        if let Type::Generic(name) = arg
            && !owner_params.iter().any(|param| &param.name == name)
        {
            owner_params.push(ast::GenericParam {
                name: name.clone(),
                kind: ast::GenericParamKind::Type,
                bounds: Vec::new(),
                span: block.span,
            });
        }
    }
    for method in &block.methods {
        check_attached_function(method, &type_name, &target_ty, &owner_params, program, reporter);
    }
}

fn check_function(
    function: &FnDecl,
    item: &str,
    signature: &FunctionSig,
    owner_params: &[ast::GenericParam],
    program: &Program,
    reporter: &mut Reporter,
) {
    program
        .return_types
        .borrow_mut()
        .insert(function as *const FnDecl, signature.return_ty.clone());
    if is_differentiable(function) {
        check_differentiable_signature(function, signature, owner_params, program, reporter);
    }
    let mut generics = generic_substitutions(&signature.generics);
    for name in ast::generic_names(owner_params) {
        generics.insert(name.clone(), Type::Generic(name));
    }
    let previous_rigid = named::set_rigid(HashSet::new());
    named::set_params(owner_params.iter().chain(&function.generics));
    let const_values: HashMap<String, Type> =
        const_value_types(owner_params).chain(const_value_types(&function.generics)).collect();
    let mut first_pass = Reporter::new();
    let recorded = program.instantiations.borrow().len();
    let closure_params =
        check_function_body(function, owner_params, item, signature, &generics, &const_values, HashMap::new(), program, &mut first_pass);
    if closure_params.is_empty() {
        for diagnostic in first_pass.diagnostics() {
            reporter.push(diagnostic.clone());
        }
    } else {
        program.instantiations.borrow_mut().truncate(recorded);
        check_function_body(function, owner_params, item, signature, &generics, &const_values, closure_params, program, reporter);
    }
    named::set_rigid(previous_rigid);
}

#[allow(clippy::too_many_arguments)]
fn check_function_body(
    function: &FnDecl,
    owner_params: &[ast::GenericParam],
    item: &str,
    signature: &FunctionSig,
    generics: &HashMap<String, Type>,
    const_values: &HashMap<String, Type>,
    closure_params: HashMap<*const Expr, Vec<Type>>,
    program: &Program,
    reporter: &mut Reporter,
) -> HashMap<*const Expr, Vec<Type>> {
    let placeholder = Type::Unit;
    let mut context = FunctionContext {
        scopes: vec![HashMap::new()],
        expected_return: &placeholder,
        loop_depth: 0,
        in_unsafe: false,
        in_iter: function.is_iter,
        in_comptime: function.is_comptime
            || signature.body_params.iter().chain([&signature.return_ty]).any(|ty| matches!(ty, Type::TypeValue(_) | Type::Code)),
        generics: generics.clone(),
        const_values: const_values.clone(),
        item: item.to_string(),
        bounds: param_bounds(owner_params.iter().chain(&function.generics)).into_iter().collect(),
        closure_params,
        unresolved_closure_params: Vec::new(),
        dims: Default::default(),
    };
    let dim_params = named::open_dim_params(owner_params.iter().chain(&function.generics), program, &mut context);
    let return_ty = named::rename(&signature.return_ty, &dim_params);
    let mut context = FunctionContext { expected_return: &return_ty, ..context };
    for (param, param_ty) in function.params.iter().zip(&signature.body_params) {
        let param_ty = &named::rename(param_ty, &dim_params);
        program
            .param_types
            .borrow_mut()
            .insert(param as *const Param, param_ty.clone());
        if let Some(id) = program.locals.pat(&param.pattern) {
            let ty = match &param.pattern {
                Pat::Ident(name, span) if receiver_from_param(param).is_none() => {
                    named::open_binding(param_ty.clone(), name, id, *span, program, &mut context)
                }
                _ => param_ty.clone(),
            };
            context.scopes.last_mut().unwrap().insert(
                id,
                Binding {
                    ty,
                    mutable: receiver_from_param(param).is_some_and(|receiver| receiver.mutable),
                    closure: None,
                },
            );
        }
    }
    context.dims.return_ty = function.return_ty.clone();

    if let Some(tail) = &function.body.tail {
        seed_closure_params(tail, Some(&return_ty), &mut context);
        named::set_hint(tail, return_ty.clone(), &mut context);
    }
    let actual = infer_block(&function.body, program, &mut context, reporter);
    // `iter fn`'s declared return type names the item `yield` produces, not
    // what the body evaluates to when it falls off the end (which just
    // means the generator is exhausted) — so it is checked per-`yield`
    // above, not against the body's own trailing type here.
    if function.is_iter {
        report_unresolved_closure_params(&context, reporter);
        return context.closure_params;
    }
    let actual = resolve_against_expected(&return_ty, actual, &context);
    if let Some(tail) = &function.body.tail {
        recache(program, tail, &actual);
    }
    named::check_return(function.span, function.body.tail.as_deref(), &return_ty, &actual, program, &context, reporter);
    report_unresolved_closure_params(&context, reporter);
    context.closure_params
}

fn report_unresolved_closure_params(context: &FunctionContext<'_>, reporter: &mut Reporter) {
    for span in &context.unresolved_closure_params {
        reporter.push(Diagnostic::error(
            "PACO-E0335",
            *span,
            "cannot infer the type of this closure parameter; add a type annotation",
        ));
    }
}

fn infer_block(
    block: &Block,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    context.scopes.push(HashMap::new());
    for statement in &block.stmts {
        match statement {
            Stmt::Let(statement) => {
                context.dims.statement = Some(statement.span);
                check_let(statement, program, context, reporter)
            }
            Stmt::Expr(expr) => {
                context.dims.statement = Some(expr_span(expr));
                if infer_expr(expr, program, context, reporter) == Type::Never {
                    context.scopes.pop();
                    named::end_scope(Type::Never, program, context);
                    return Type::Never;
                }
            }
            Stmt::Item(_) => {}
        }
    }
    let result = block.tail.as_ref().map_or(Type::Unit, |expr| {
        context.dims.statement = Some(expr_span(expr));
        infer_expr(expr, program, context, reporter)
    });
    context.scopes.pop();
    named::end_scope(result, program, context)
}

/// A bare integer literal has no width of its own (`infer_expr` defaults it
/// to `i64`) — when the caller already knows the expected type, this checks
/// the literal's value against that width's range and reports it typed as
/// that width instead, rather than defaulting to `i64` and then reporting
/// an opaque width mismatch. Scoped deliberately narrow (design.md): only a
/// direct `let` initializer that is itself a bare integer literal goes
/// through this path — not a general bidirectional-inference pass over
/// every expression position.
fn infer_literal_against_expected(
    value: &Expr,
    expected: Option<&Type>,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    if let (Expr::Literal(Literal::Int(n), span), Some(Type::Int(width))) = (value, expected) {
        check_int_literal_range(*n, *width, *span, reporter);
        let ty = Type::Int(*width);
        // `infer_expr` caches every expression it types (by pointer, for
        // `paco-mir`'s `TypedModule::type_of` to consult later); this early
        // return bypasses `infer_expr`, so it must cache the same way or a
        // narrow-width `let` initializer's literal is left untyped and MIR
        // lowering panics on it.
        program.types.borrow_mut().insert(value as *const Expr, ty.clone());
        return ty;
    }
    if let (Expr::Unary { op: UnaryOp::Neg, expr: inner, span }, Some(Type::Int(width))) = (value, expected)
        && width.is_signed()
        && let Expr::Literal(Literal::Int(n), _) = inner.as_ref()
    {
        check_int_literal_range(n.wrapping_neg(), *width, *span, reporter);
        let ty = Type::Int(*width);
        program.types.borrow_mut().insert(inner.as_ref() as *const Expr, ty.clone());
        program.types.borrow_mut().insert(value as *const Expr, ty.clone());
        return ty;
    }
    if let (Expr::Literal(Literal::Float(_), _), Some(Type::Float(width))) = (value, expected) {
        let ty = Type::Float(*width);
        program.types.borrow_mut().insert(value as *const Expr, ty.clone());
        return ty;
    }
    if let (Expr::Tuple(items, _), Some(Type::Tuple(expected_items))) = (value, expected)
        && items.len() == expected_items.len()
    {
        let ty = Type::Tuple(
            items
                .iter()
                .zip(expected_items)
                .map(|(item, expected)| infer_literal_against_expected(item, Some(expected), program, context, reporter))
                .collect(),
        );
        program.types.borrow_mut().insert(value as *const Expr, ty.clone());
        return ty;
    }
    infer_expr(value, program, context, reporter)
}

fn check_int_literal_range(value: i64, width: IntWidth, span: Span, reporter: &mut Reporter) {
    let (min, max) = width.range();
    let value = i128::from(value);
    if value < min || value > max {
        reporter.push(Diagnostic::error(
            "PACO-E0329",
            span,
            format!(
                "integer literal `{value}` does not fit in `{}` (range {min}..={max})",
                width.name()
            ),
        ));
    }
}

fn is_numeric_primitive(ty: &Type) -> bool {
    matches!(ty, Type::Int(_) | Type::Float(_) | Type::Char)
}

fn check_let(
    statement: &LetStmt,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    let declared_ty = statement
        .ty
        .as_ref()
        .map(|ty| program.ty_from_ast(ty, &context.generics, reporter));
    let mut value_ty = statement.value.as_ref().map_or(Type::Unknown, |value| {
        seed_closure_params(value, declared_ty.as_ref(), context);
        if let Some(declared_ty) = &declared_ty {
            named::set_hint(value, declared_ty.clone(), context);
        }
        infer_literal_against_expected(value, declared_ty.as_ref(), program, context, reporter)
    });

    if let Some(ref declared_ty) = declared_ty {
        value_ty = resolve_against_expected(declared_ty, value_ty, context);
        if let Some(value) = &statement.value {
            recache(program, value, &value_ty);
        }
    }

    let binding_ty = declared_ty.clone().unwrap_or_else(|| value_ty.clone());
    if let Some(declared_ty) = declared_ty
        && !named::decays_to(&value_ty, &declared_ty)
    {
        if let Some((axis, name)) = named::dyn_escape(&declared_ty, &value_ty, program) {
            let diagnostic = named::escape_diagnostic(statement.span, axis, &name, "annotated as `Dyn`");
            reporter.push(named::escape_fixes(diagnostic, statement.value.as_ref(), axis, &name, None, program));
        } else {
            reporter.push(mismatch_diagnostic(statement.span, "", &declared_ty, &value_ty));
        }
    }
    bind_let_pattern(
        &statement.pattern,
        &binding_ty,
        statement.mutable,
        program,
        context,
        reporter,
    );
    named::bind_witness(statement, program, context);
    if let Some(id) = program.locals.pat(&statement.pattern)
        && let Some(binding) = context.scopes.last().and_then(|scope| scope.get(&id))
    {
        named::record_shape(&statement.pattern, &binding.ty, program);
    }
    if let (Pat::Ident(..), Some(value @ Expr::Closure { .. })) = (&statement.pattern, &statement.value)
        && let Some(id) = program.locals.pat(&statement.pattern)
        && let Some(binding) = context.scopes.last_mut().unwrap().get_mut(&id)
    {
        binding.closure = Some(value as *const Expr);
    }
}

/// Whether a `break` in `body` exits this loop (not a nested one).
fn loop_breaks(body: &Block) -> bool {
    struct Breaks(bool);
    impl ast::Visit for Breaks {
        fn visit_expr(&mut self, expr: &Expr) {
            match expr {
                Expr::Break(..) => self.0 = true,
                Expr::Loop { .. } | Expr::While { .. } | Expr::Closure { .. } => {}
                _ => ast::walk_expr(self, expr),
            }
        }
    }
    let mut breaks = Breaks(false);
    ast::walk_block(&mut breaks, body);
    breaks.0
}

fn seed_closure_params(value: &Expr, expected: Option<&Type>, context: &mut FunctionContext<'_>) {
    if let (Expr::Closure { params, .. }, Some(Type::Fn(expected_params, _))) = (value, expected)
        && params.len() == expected_params.len()
    {
        context.closure_params.entry(value as *const Expr).or_insert_with(|| expected_params.clone());
    }
}

fn infer_expr(
    expr: &Expr,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let ty = infer_expr_uncached(expr, program, context, reporter);
    program
        .types
        .borrow_mut()
        .insert(expr as *const Expr, ty.clone());
    ty
}

fn infer_expr_uncached(
    expr: &Expr,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    match expr {
        Expr::Tuple(items, _) if items.is_empty() => Type::Unit,
        Expr::Tuple(items, _) => Type::Tuple(items.iter().map(|item| infer_expr(item, program, context, reporter)).collect()),
        Expr::Literal(Literal::Int(_), _) => Type::Int(IntWidth::I64),
        Expr::Literal(Literal::Float(_), _) => Type::Float(FloatWidth::F64),
        Expr::Literal(Literal::Bool(_), _) => Type::Bool,
        Expr::Literal(Literal::String(_), _) => Type::String,
        Expr::Literal(Literal::Char(_), _) => Type::Char,
        Expr::Ident(name, span) => lookup(program, &context.scopes, expr)
            .map(|binding| binding.ty)
            .or_else(|| program.consts.get(name).map(|info| info.ty.clone()))
            .or_else(|| context.const_values.get(name).cloned())
            .or_else(|| find_zero_field_variant_enum(name, program))
            .unwrap_or_else(|| {
                reporter.push(Diagnostic::error(
                    "PACO-E0319",
                    *span,
                    format!("unresolved identifier `{name}`"),
                ));
                Type::Error
            }),
        Expr::Block(block) => {
            if let Some(tail) = &block.tail {
                named::forward_hint(expr, tail, context);
            }
            infer_block(block, program, context, reporter)
        }
        Expr::Unsafe(block, _) => {
            if let Some(tail) = &block.tail {
                named::forward_hint(expr, tail, context);
            }
            let was_unsafe = context.in_unsafe;
            context.in_unsafe = true;
            let ty = infer_block(block, program, context, reporter);
            context.in_unsafe = was_unsafe;
            ty
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
            span,
        } => infer_if(
            condition,
            then_branch,
            else_branch.as_deref(),
            *span,
            program,
            context,
            reporter,
        ),
        Expr::Loop { body, .. } => {
            context.loop_depth += 1;
            infer_block(body, program, context, reporter);
            context.loop_depth -= 1;
            if loop_breaks(body) { Type::Unit } else { Type::Never }
        }
        Expr::While {
            condition,
            body,
            span,
            ..
        } => {
            let condition_ty = infer_expr(condition, program, context, reporter);
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
            infer_block(body, program, context, reporter);
            context.loop_depth -= 1;
            Type::Unit
        }
        Expr::Call { callee, type_args, args, span } => {
            infer_call(expr, callee, type_args, args, *span, program, context, reporter)
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } => infer_method_call(expr, receiver, method, args, *span, program, context, reporter),
        Expr::AssociatedCall {
            ty,
            function,
            args,
            span,
        } => infer_associated_call(expr, ty, function, args, *span, program, context, reporter),
        Expr::StructLiteral { ty, fields, span } => {
            infer_struct_literal(ty, fields, *span, program, context, reporter)
        }
        Expr::Field { base, field, span } => {
            infer_field(base, field, *span, program, context, reporter)
        }
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => infer_binary(expr, *op, left, right, *span, program, context, reporter),
        Expr::Unary { op, expr: operand, span } => infer_unary(expr, *op, operand, *span, program, context, reporter),
        Expr::Assign {
            target,
            value,
            span,
        } => infer_assign(target, value, *span, program, context, reporter),
        Expr::Return(value, span) => {
            let actual = value.as_ref().map_or(Type::Unit, |value| {
                let expected_return = context.expected_return.clone();
                seed_closure_params(value, Some(&expected_return), context);
                named::set_hint(value, expected_return, context);
                infer_expr(value, program, context, reporter)
            });
            let actual = resolve_against_expected(context.expected_return, actual, context);
            if let Some(value) = value {
                recache(program, value, &actual);
            }
            named::check_return(*span, value.as_deref(), context.expected_return, &actual, program, context, reporter);
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
                infer_expr(value, program, context, reporter);
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
        Expr::Match {
            scrutinee,
            arms,
            span,
        } => infer_match(scrutinee, arms, *span, program, context, reporter),
        Expr::Try { expr: inner, span } => {
            named::forward_hint(expr, inner, context);
            infer_try(inner, *span, program, context, reporter)
        }
        Expr::Cast { expr, ty, span } => {
            let source_ty = infer_expr(expr, program, context, reporter);
            let target_ty = program.ty_from_ast(ty, &context.generics, reporter);
            let castable = |ty: &Type| {
                is_numeric_primitive(ty) || (matches!(ty, Type::Generic(_)) && satisfies(ty, "Numeric", program, Some(context)))
            };
            if !castable(&source_ty) || !castable(&target_ty) {
                reporter.push(Diagnostic::error(
                    "PACO-E0330",
                    *span,
                    format!(
                        "cannot cast {} to {}: `as` only converts between numeric primitive types",
                        source_ty.name(),
                        target_ty.name()
                    ),
                ));
                return Type::Error;
            }
            target_ty
        }
        Expr::Index { base, index, span } => infer_index(base, index, *span, program, context, reporter),
        Expr::Closure { params, body, .. } => infer_closure(expr, params, body, program, context, reporter),
        Expr::Spawn { expr, .. } => {
            warn_blocking_calls_on_worker(expr, program, reporter);
            let inner = infer_expr(expr, program, context, reporter);
            Type::Struct("JoinHandle".to_string(), vec![inner])
        }
        Expr::Select { arms, default, span } => {
            infer_select(arms, default.as_ref(), *span, program, context, reporter)
        }
        Expr::Comptime { expr, .. } => {
            let was_comptime = context.in_comptime;
            context.in_comptime = true;
            let ty = infer_expr(expr, program, context, reporter);
            context.in_comptime = was_comptime;
            ty
        }
        Expr::Splice(expr, _) => infer_expr(expr, program, context, reporter),
        Expr::Quote(body, span) => {
            require_comptime(reporter, *span, context.in_comptime, "`quote`");
            infer_quote_splices(body, program, context, reporter);
            Type::Code
        }
        Expr::Yield(expr, span) => {
            if !context.in_iter {
                reporter.push(Diagnostic::error(
                    "PACO-E0326",
                    *span,
                    "`yield` is only allowed inside an `iter fn` body",
                ));
                return Type::Error;
            }
            let yielded_ty = infer_expr(expr, program, context, reporter);
            if !compatible(&yielded_ty, context.expected_return) {
                reporter.push(mismatch_diagnostic(*span, "", context.expected_return, &yielded_ty));
            }
            Type::Unit
        }
        Expr::Borrow {
            mutable,
            expr,
            span,
        } => {
            if *mutable && !is_mutable_place(expr, program, context) {
                reporter.push(Diagnostic::error(
                    "PACO-E0307",
                    *span,
                    format!(
                        "cannot take a mutable borrow of immutable `{}`",
                        place_name(expr).unwrap_or("place")
                    ),
                ));
            }
            Type::Borrow {
                mutable: *mutable,
                ty: Box::new(infer_expr(expr, program, context, reporter)),
            }
        }
    }
}

fn infer_try(
    inner: &Expr,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let operand_ty = infer_expr(inner, program, context, reporter);
    if operand_ty == Type::Error {
        return Type::Error;
    }
    let Some((ok_ty, src_error)) = result_type_args(&operand_ty) else {
        reporter.push(Diagnostic::error(
            "PACO-E0322",
            span,
            format!(
                "`?` requires a `Result<T, E>` operand, found {}",
                operand_ty.name()
            ),
        ));
        return Type::Error;
    };

    let Some((_, dst_error)) = result_type_args(context.expected_return) else {
        reporter.push(Diagnostic::error(
            "PACO-E0323",
            span,
            "`?` can only be used in a function returning `Result<U, E>`",
        ));
        return Type::Error;
    };

    if src_error == dst_error {
        return ok_ty;
    }

    let conversion_exists = target_type_name(&dst_error).is_some_and(|dst_name| {
        program
            .associated
            .get(&(dst_name, "from".to_string()))
            .is_some_and(|signature| {
                let mut substitutions = generic_substitutions(&signature.generics);
                substitutions.insert("Self".to_string(), dst_error.clone());
                signature.params.len() == 1
                    && unify_type(&signature.params[0], &src_error, &mut substitutions)
            })
    });
    if !conversion_exists {
        reporter.push(Diagnostic::error(
            "PACO-E0324",
            span,
            format!(
                "no conversion from {} to {} exists",
                src_error.name(),
                dst_error.name()
            ),
        ));
        return Type::Error;
    }

    ok_ty
}

fn result_type_args(ty: &Type) -> Option<(Type, Type)> {
    let Type::Enum(name, args) = ty else {
        return None;
    };
    if name == "Result" && args.len() == 2 {
        Some((args[0].clone(), args[1].clone()))
    } else {
        None
    }
}

fn infer_if(
    condition: &Expr,
    then_branch: &Block,
    else_branch: Option<&Expr>,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let condition_ty = infer_expr(condition, program, context, reporter);
    if !compatible(&condition_ty, &Type::Bool) {
        reporter.push(Diagnostic::error(
            "PACO-E0303",
            span,
            format!("if condition must be bool, found {}", condition_ty.name()),
        ));
    }
    let then_ty = infer_block(then_branch, program, context, reporter);
    let else_ty = else_branch.map_or(Type::Unit, |else_branch| {
        infer_expr(else_branch, program, context, reporter)
    });
    if let Some(joined) = join_branch_types(&then_ty, &else_ty) {
        if let Some(tail) = &then_branch.tail {
            refine_branch(program, tail, &joined);
        }
        if let Some(else_branch) = else_branch {
            refine_branch(program, else_branch, &joined);
        }
    }
    join_branch_types(&then_ty, &else_ty).unwrap_or_else(|| {
        reporter.push(Diagnostic::error(
            "PACO-E0304",
            span,
            format!(
                "if branches have incompatible types: {} and {}",
                then_ty.name(),
                else_ty.name()
            ),
        ));
        Type::Error
    })
}

fn infer_match(
    scrutinee: &Expr,
    arms: &[MatchArm],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let scrutinee_ty = infer_expr(scrutinee, program, context, reporter);
    check_match_coverage(&scrutinee_ty, arms, span, program, reporter);

    let mut result_ty = None;
    for arm in arms {
        context.scopes.push(HashMap::new());
        bind_pattern_types(&arm.pattern, &scrutinee_ty, program, context, reporter);
        if let Some(guard) = &arm.guard {
            let guard_ty = infer_expr(guard, program, context, reporter);
            if !compatible(&guard_ty, &Type::Bool) {
                reporter.push(Diagnostic::error(
                    "PACO-E0303",
                    arm.span,
                    format!("match guard must be bool, found {}", guard_ty.name()),
                ));
            }
        }
        let arm_ty = infer_expr(&arm.body, program, context, reporter);
        context.scopes.pop();
        result_ty = Some(match result_ty {
            None => arm_ty,
            Some(previous) => join_branch_types(&previous, &arm_ty).unwrap_or_else(|| {
                reporter.push(Diagnostic::error(
                    "PACO-E0304",
                    arm.span,
                    format!(
                        "match arms have incompatible types: {} and {}",
                        previous.name(),
                        arm_ty.name()
                    ),
                ));
                Type::Error
            }),
        });
    }

    if let Some(joined) = &result_ty {
        for arm in arms {
            refine_branch(program, &arm.body, joined);
        }
    }
    result_ty.unwrap_or_else(|| {
        reporter.push(Diagnostic::error(
            "PACO-E0401",
            span,
            "non-exhaustive match: no arms were provided",
        ));
        Type::Error
    })
}

fn find_zero_field_variant_enum(name: &str, program: &Program) -> Option<Type> {
    let candidates = program.enums.iter().filter(|(_, info)| {
        info.variants.iter().any(|variant| {
            variant.name == name
                && (matches!(&variant.fields, VariantFields::Unit)
                    || matches!(&variant.fields, VariantFields::Tuple(tys) if tys.is_empty()))
        })
    });
    let mut candidates: Vec<_> = candidates.collect();
    candidates.sort_by_key(|(enum_name, _)| enum_name.contains("::"));
    candidates.first().map(|(enum_name, info)| {
        let args = info.generics.iter().cloned().map(Type::Generic).collect();
        fresh_enum_params(Type::Enum((*enum_name).clone(), args))
    })
}

fn is_zero_field_variant(expected: &Type, name: &str, program: &Program) -> bool {
    let Type::Enum(enum_name, _) = expected else {
        return false;
    };
    program.enums.get(enum_name).is_some_and(|info| {
        info.variants.iter().any(|variant| {
            variant.name == name
                && (matches!(&variant.fields, VariantFields::Unit)
                    || matches!(&variant.fields, VariantFields::Tuple(tys) if tys.is_empty()))
        })
    })
}

fn bind_pattern_types(
    pattern: &Pat,
    expected: &Type,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    match pattern {
        Pat::Ident(name, _) if is_zero_field_variant(expected, name, program) => {}
        Pat::Ident(..) => {
            bind_local(pattern, expected.clone(), false, program, context);
        }
        Pat::Binding { pattern: inner, .. } => {
            bind_local(pattern, expected.clone(), false, program, context);
            bind_pattern_types(inner, expected, program, context, reporter);
        }
        Pat::Wildcard(_) => {}
        Pat::Literal(literal, span) => {
            let actual = literal_type(literal);
            if !compatible(&actual, expected) {
                reporter.push(mismatch_diagnostic(*span, "pattern ", expected, &actual));
            }
        }
        Pat::Range {
            start, end, span, ..
        } => {
            if !matches!(expected, Type::Int(_)) {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!(
                        "range pattern requires an integer type, found {}",
                        expected.name()
                    ),
                ));
            }
            if !is_int_literal_pattern(start) || !is_int_literal_pattern(end) {
                reporter.push(Diagnostic::error(
                    "PACO-E0306",
                    *span,
                    "range pattern bounds must be integer literals",
                ));
                return;
            }
            bind_pattern_types(start, expected, program, context, reporter);
            bind_pattern_types(end, expected, program, context, reporter);
        }
        Pat::Enum { path, fields, span } => {
            let Type::Enum(enum_name, _) = expected else {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!(
                        "enum pattern requires enum value, found {}",
                        expected.name()
                    ),
                ));
                return;
            };
            if path.len() >= 2 && path.first() != Some(enum_name) {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!("enum pattern does not match `{enum_name}`"),
                ));
                return;
            }
            let variant_name = path.last().unwrap();
            let Some(variant) = program.enums.get(enum_name).and_then(|info| {
                info.variants
                    .iter()
                    .find(|variant| &variant.name == variant_name)
            }) else {
                reporter.push(Diagnostic::error(
                    "PACO-E0314",
                    *span,
                    format!("variant `{variant_name}` not found for `{enum_name}`"),
                ));
                return;
            };
            let expected_fields = variant_field_types(variant, expected, program, reporter);
            if fields.len() != expected_fields.len() {
                reporter.push(Diagnostic::error(
                    "PACO-E0305",
                    *span,
                    format!(
                        "variant `{variant_name}` expects {} fields, found {}",
                        expected_fields.len(),
                        fields.len()
                    ),
                ));
            }
            for (field, field_ty) in fields.iter().zip(expected_fields) {
                bind_pattern_types(field, &field_ty, program, context, reporter);
            }
        }
        Pat::Or(patterns, span) => {
            bind_or_pattern_types(patterns, *span, expected, program, context, reporter);
        }
        Pat::Tuple(elements, span) => {
            bind_tuple_pattern(elements, *span, expected, false, program, context, reporter);
        }
        Pat::Struct { path, fields, rest, span } => {
            let struct_name = path.join("::");
            let expected_fields = match expected {
                Type::Struct(name, _) if name.rsplit("::").next() == path.last().map(String::as_str) => {
                    instantiated_struct_fields(expected, program, reporter)
                }
                _ => None,
            };
            let Some(expected_fields) = expected_fields else {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    *span,
                    format!("struct pattern `{struct_name}` does not match {}", expected.name()),
                ));
                return;
            };
            let expected_fields = named::open_pattern_fields(expected_fields, fields, *span, program, context);
            for (field, pattern) in fields {
                let Some(field_ty) = expected_fields.get(field) else {
                    reporter.push(Diagnostic::error(
                        "PACO-E0311",
                        pattern_span(pattern),
                        format!("unknown field `{field}`"),
                    ));
                    continue;
                };
                bind_pattern_types(pattern, field_ty, program, context, reporter);
            }
            if !rest {
                let mut missing: Vec<&String> = expected_fields
                    .keys()
                    .filter(|name| !fields.iter().any(|(field, _)| field == *name))
                    .collect();
                missing.sort();
                if let Some(first) = missing.first() {
                    reporter.push(Diagnostic::error(
                        "PACO-E0305",
                        *span,
                        format!("struct pattern `{struct_name}` does not mention field `{first}`; list it or add `..`"),
                    ));
                }
            }
        }
    }
}

fn bind_tuple_pattern(
    elements: &[Pat],
    span: Span,
    expected: &Type,
    mutable: bool,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    let Type::Tuple(expected_items) = expected else {
        reporter.push(Diagnostic::error(
            "PACO-E0302",
            span,
            format!("tuple pattern requires a tuple value, found {}", expected.name()),
        ));
        return;
    };
    if elements.len() != expected_items.len() {
        reporter.push(Diagnostic::error(
            "PACO-E0305",
            span,
            format!(
                "tuple pattern expects {} elements, found {}",
                expected_items.len(),
                elements.len()
            ),
        ));
        return;
    }
    for (element, element_ty) in elements.iter().zip(expected_items) {
        bind_let_pattern(element, element_ty, mutable, program, context, reporter);
    }
}

fn bind_let_pattern(
    pattern: &Pat,
    expected: &Type,
    mutable: bool,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    match pattern {
        Pat::Ident(..) => bind_local(pattern, expected.clone(), mutable, program, context),
        Pat::Tuple(elements, span) => {
            bind_tuple_pattern(elements, *span, expected, mutable, program, context, reporter);
        }
        _ => bind_pattern_types(pattern, expected, program, context, reporter),
    }
}

fn bind_or_pattern_types(
    patterns: &[Pat],
    span: Span,
    expected: &Type,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    let mut alternatives = Vec::new();
    for pattern in patterns {
        context.scopes.push(HashMap::new());
        bind_pattern_types(pattern, expected, program, context, reporter);
        alternatives.push(context.scopes.pop().unwrap_or_default());
    }
    let Some(first) = alternatives.first() else {
        return;
    };
    let same_bindings = alternatives.iter().skip(1).all(|alternative| {
        alternative.len() == first.len()
            && first.iter().all(|(id, binding)| {
                alternative
                    .get(id)
                    .is_some_and(|other| compatible(&binding.ty, &other.ty))
            })
    });
    if !same_bindings {
        reporter.push(Diagnostic::error(
            "PACO-E0306",
            span,
            "or-pattern alternatives must bind the same names with compatible types",
        ));
        return;
    }
    context.scopes.last_mut().unwrap().extend(
        first
            .iter()
            .map(|(id, binding)| (*id, binding.clone())),
    );
}

fn check_match_coverage(
    scrutinee_ty: &Type,
    arms: &[MatchArm],
    span: Span,
    program: &Program,
    reporter: &mut Reporter,
) {
    let Some(constructors) = constructors_for_type(scrutinee_ty, program) else {
        return;
    };
    let qualified_arms;
    let arms: &[MatchArm] = match scrutinee_ty {
        Type::Enum(name, _) => {
            qualified_arms = qualify_bare_variant_patterns(arms, name);
            &qualified_arms
        }
        _ => arms,
    };
    let report = analyze_match(arms, constructors);
    for unreachable in report.unreachable_arms {
        let message = unreachable.witness.map_or_else(
            || "unreachable match arm".to_string(),
            |witness| format!("unreachable match arm: `{witness}` is already covered"),
        );
        reporter.push(Diagnostic::error(
            "PACO-E0402",
            pattern_span(&arms[unreachable.index].pattern),
            message,
        ));
    }
    if let Some(witness) = report.missing_witness {
        reporter.push(Diagnostic::error(
            "PACO-E0401",
            span,
            format!("non-exhaustive match: missing `{witness}`"),
        ));
    }
}

fn qualify_bare_variant_patterns(arms: &[MatchArm], enum_name: &str) -> Vec<MatchArm> {
    arms.iter()
        .map(|arm| MatchArm {
            pattern: qualify_bare_variant_pattern(arm.pattern.clone(), enum_name),
            guard: arm.guard.clone(),
            body: arm.body.clone(),
            span: arm.span,
        })
        .collect()
}

fn qualify_bare_variant_pattern(pattern: Pat, enum_name: &str) -> Pat {
    match pattern {
        Pat::Enum { mut path, fields, span } => {
            if path.len() == 1 {
                path.insert(0, enum_name.to_string());
            }
            Pat::Enum { path, fields, span }
        }
        Pat::Binding { name, pattern, span } => Pat::Binding {
            name,
            pattern: Box::new(qualify_bare_variant_pattern(*pattern, enum_name)),
            span,
        },
        Pat::Or(patterns, span) => Pat::Or(
            patterns
                .into_iter()
                .map(|pattern| qualify_bare_variant_pattern(pattern, enum_name))
                .collect(),
            span,
        ),
        other => other,
    }
}

fn constructors_for_type(scrutinee_ty: &Type, program: &Program) -> Option<ConstructorSet> {
    match scrutinee_ty {
        Type::Bool => Some(ConstructorSet::closed(["true", "false"])),
        Type::Enum(name, _) => program.enums.get(name).map(|info| {
            ConstructorSet::closed(
                info.variants
                    .iter()
                    .map(|variant| format!("{name}::{}", variant.name)),
            )
        }),
        Type::Int(_) | Type::Float(_) | Type::String | Type::Char => {
            Some(ConstructorSet::open("_"))
        }
        Type::Unknown | Type::Error => None,
        _ => None,
    }
}

fn is_int_literal_pattern(pattern: &Pat) -> bool {
    matches!(pattern, Pat::Literal(Literal::Int(_), _))
}

fn variant_field_types(
    variant: &VariantInfo,
    enum_ty: &Type,
    program: &Program,
    reporter: &mut Reporter,
) -> Vec<Type> {
    match &variant.fields {
        VariantFields::Unit => Vec::new(),
        VariantFields::Tuple(types) => types
            .iter()
            .map(|ty| instantiate_ty(ty, enum_ty, program, reporter))
            .collect(),
        VariantFields::Struct(_) => {
            reporter.push(Diagnostic::error(
                "PACO-E0306",
                variant.span,
                "named enum variant patterns are not supported yet",
            ));
            Vec::new()
        }
    }
}

fn require_unsafe(reporter: &mut Reporter, span: Span, in_unsafe: bool, what: &str) {
    if !in_unsafe {
        reporter.push(Diagnostic::error(
            "PACO-E0325",
            span,
            format!("{what} requires an `unsafe` block"),
        ));
    }
}

/// `phase-9-comptime` Decision 5: a `comptime fn` has no runtime
/// representation once compilation finishes (it may use `Value::Type`/
/// `Code`, neither of which exist outside comptime evaluation), so
/// calling one is only valid while already inside a comptime context —
/// mirrors `require_unsafe`.
fn require_comptime(reporter: &mut Reporter, span: Span, in_comptime: bool, what: &str) {
    if !in_comptime {
        reporter.push(Diagnostic::error(
            "PACO-E0352",
            span,
            format!("{what} is a `comptime fn` and can only be called from inside a comptime context"),
        ));
    }
}

/// The binding form (`name = expr.recv() => body`) unwraps the channel's
/// `Result<T, RecvError>` to a plain `T` inside `body` — by the time an arm
/// runs, `select` has already established that channel had a value ready,
/// so the `RecvError` case cannot occur there.
fn infer_select(
    arms: &[ast::SelectArm],
    default: Option<&Block>,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let mut branch_types = Vec::new();
    for arm in arms {
        context.scopes.push(HashMap::new());
        match &arm.operation {
            Expr::Assign { target, value, .. } => {
                let Expr::Ident(..) = target.as_ref() else {
                    reporter.push(Diagnostic::error(
                        "PACO-E0327",
                        arm.span,
                        "select binding target must be an identifier",
                    ));
                    context.scopes.pop();
                    continue;
                };
                let value_ty = infer_expr(value, program, context, reporter);
                let item_ty = match &value_ty {
                    Type::Enum(name, args) if name == "Result" && !args.is_empty() => {
                        args[0].clone()
                    }
                    other => other.clone(),
                };
                if let Some(id) = program.locals.expr(target) {
                    context.scopes.last_mut().unwrap().insert(id, Binding { ty: item_ty, mutable: false, closure: None });
                }
            }
            other => {
                infer_expr(other, program, context, reporter);
            }
        }
        branch_types.push(infer_block(&arm.body, program, context, reporter));
        context.scopes.pop();
    }
    if let Some(default) = default {
        branch_types.push(infer_block(default, program, context, reporter));
    }
    if branch_types.is_empty() {
        reporter.push(Diagnostic::error(
            "PACO-E0327",
            span,
            "select must have at least one arm",
        ));
        return Type::Error;
    }
    branch_types
        .into_iter()
        .reduce(|a, b| join_branch_types(&a, &b).unwrap_or(Type::Unit))
        .unwrap_or(Type::Unit)
}

/// Type-checks a `quote { .. }` template's splice points only
/// (`phase-9-comptime` Decision 7) — never the template's own literal
/// structure (method/field names, `self`, the struct being derived
/// for's own not-yet-existing fields, ...), which belongs to the
/// *generated* method's own future scope and is checked for real once
/// re-entry (task 5.5) splices it in and it passes through the ordinary
/// pipeline — checking it here, before that, would reject valid
/// templates for referencing things that only exist after splicing.
fn infer_quote_splices(
    body: &QuoteBody,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    struct SpliceChecker<'p, 'c, 'f, 'r> {
        program: &'p Program,
        context: &'c mut FunctionContext<'f>,
        reporter: &'r mut Reporter,
    }
    impl Visit for SpliceChecker<'_, '_, '_, '_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if let Expr::Splice(inner, _) = expr {
                infer_expr(inner, self.program, self.context, self.reporter);
                return;
            }
            if let Expr::Call { callee, args, .. } = expr
                && matches!(callee.as_ref(), Expr::Ident(name, _) if name == "splice_field")
                && let [base, name] = args.as_slice()
            {
                self.visit_expr(base);
                infer_expr(name, self.program, self.context, self.reporter);
                return;
            }
            walk_expr(self, expr);
        }

        fn visit_item(&mut self, item: &Item) {
            if let Item::Methods(block) = item
                && let Ty::Splice(inner, _) = &block.target
            {
                infer_expr(inner, self.program, self.context, self.reporter);
            }
            ast::walk_item(self, item);
        }
    }
    let mut checker = SpliceChecker { program, context, reporter };
    match body {
        QuoteBody::Item(item) => checker.visit_item(item),
        QuoteBody::Expr(expr) => checker.visit_expr(expr),
    }
    ast::visit_template_ty_splices(body, &mut |splice| {
        infer_expr(splice, program, context, reporter);
    });
}

fn infer_closure(
    expr: &Expr,
    params: &[ast::ClosureParam],
    body: &Expr,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let expected_return = Type::Unknown;
    let mut inner = FunctionContext {
        scopes: context.scopes.clone(),
        expected_return: &expected_return,
        loop_depth: 0,
        in_unsafe: context.in_unsafe,
        in_iter: false,
        in_comptime: context.in_comptime,
        generics: context.generics.clone(),
        const_values: context.const_values.clone(),
        item: context.item.clone(),
        bounds: context.bounds.clone(),
        closure_params: std::mem::take(&mut context.closure_params),
        unresolved_closure_params: std::mem::take(&mut context.unresolved_closure_params),
        dims: Default::default(),
    };
    inner.scopes.push(HashMap::new());
    let inferred = inner.closure_params.get(&(expr as *const Expr)).cloned().unwrap_or_default();
    let param_types: Vec<Type> = params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let ty = match &param.ty {
                Some(ty) => program.ty_from_ast(ty, &inner.generics, reporter),
                None => match inferred.get(index) {
                    Some(ty) if *ty != Type::Unknown => ty.clone(),
                    _ => {
                        inner.unresolved_closure_params.push(param.span);
                        Type::Unknown
                    }
                },
            };
            bind_pattern_types(&param.pattern, &ty, program, &mut inner, reporter);
            ty
        })
        .collect();
    let ret = infer_expr(body, program, &mut inner, reporter);
    context.closure_params = inner.closure_params;
    context.unresolved_closure_params = inner.unresolved_closure_params;
    Type::Fn(param_types, Box::new(ret))
}

fn infer_closure_call(
    binding: &Binding,
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let Type::Fn(params, ret) = &binding.ty else {
        return Type::Error;
    };
    if let Some(closure) = binding.closure
        && params.contains(&Type::Unknown)
        && args.len() == params.len()
    {
        let arg_types = args.iter().map(|arg| infer_expr(arg, program, context, reporter)).collect();
        context.closure_params.entry(closure).or_insert(arg_types);
        return (**ret).clone();
    }
    if args.len() != params.len() {
        reporter.push(Diagnostic::error(
            "PACO-E0305",
            span,
            format!("closure expects {} arguments, found {}", params.len(), args.len()),
        ));
    }
    for (arg, param) in args.iter().zip(params) {
        let actual = infer_expr(arg, program, context, reporter);
        if !compatible(&actual, param) {
            reporter.push(mismatch_diagnostic(paco_syntax::parse::expr_span(arg), "", param, &actual));
        }
    }
    (**ret).clone()
}

fn infer_spawn_blocking(
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let [closure] = args else {
        reporter.push(Diagnostic::error(
            "PACO-E0305",
            span,
            format!("spawn_blocking expects 1 argument, found {}", args.len()),
        ));
        return Type::Error;
    };
    match infer_expr(closure, program, context, reporter) {
        Type::Fn(params, ret) if params.is_empty() => Type::Struct("JoinHandle".to_string(), vec![*ret]),
        Type::Error => Type::Error,
        other => {
            reporter.push(Diagnostic::error(
                "PACO-E0302",
                paco_syntax::parse::expr_span(closure),
                format!("spawn_blocking expects a closure taking no arguments, found {}", other.name()),
            ));
            Type::Error
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn infer_call(
    call: &Expr,
    callee: &Expr,
    type_args: &[Ty],
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let Expr::Ident(name, _) = callee else {
        return match infer_expr(callee, program, context, reporter) {
            ty @ Type::Fn(..) => {
                let binding = Binding { ty, mutable: false, closure: None };
                infer_closure_call(&binding, args, span, program, context, reporter)
            }
            Type::Error | Type::Unknown => Type::Error,
            other => {
                reporter.push(Diagnostic::error(
                    "PACO-E0302",
                    span,
                    format!("a value of type {} cannot be called", other.name()),
                ));
                Type::Error
            }
        };
    };
    if let Some(binding) = lookup(program, &context.scopes, callee) {
        if matches!(binding.ty, Type::Fn(..)) {
            return infer_closure_call(&binding, args, span, program, context, reporter);
        }
        if !matches!(binding.ty, Type::Unknown | Type::Error) {
            reporter.push(Diagnostic::error(
                "PACO-E0302",
                span,
                format!("`{name}` has type {} and cannot be called", binding.ty.name()),
            ));
            return Type::Error;
        }
    } else if name == "spawn_blocking" {
        return infer_spawn_blocking(args, span, program, context, reporter);
    } else if name == "panic" && !program.functions.contains_key(name) {
        for arg in args {
            infer_expr(arg, program, context, reporter);
        }
        return Type::Never;
    }

    if name == "print" {
        if args.len() != 1 {
            reporter.push(Diagnostic::error(
                "PACO-E0305",
                span,
                format!("print expects 1 argument, found {}", args.len()),
            ));
        }
        for arg in args {
            infer_expr(arg, program, context, reporter);
        }
        return Type::Unit;
    }

    infer_function_signature_call(call, name, type_args, args, span, program, context, reporter)
}

fn infer_bare_variant_construction(
    name: &str,
    args: &[Expr],
    _span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let mut candidates: Vec<_> = program
        .enums
        .iter()
        .filter_map(|(enum_name, info)| {
            info.variants
                .iter()
                .find(|variant| variant.name == name)
                .map(|variant| (enum_name.clone(), info.generics.clone(), variant.clone()))
        })
        .collect();
    candidates.sort_by_key(|(enum_name, ..)| enum_name.contains("::"));
    let Some((enum_name, generics, variant)) = candidates.into_iter().next() else {
        return Type::Unknown;
    };
    let target_ty = fresh_enum_params(Type::Enum(enum_name, generics.into_iter().map(Type::Generic).collect()));
    let mut substitutions = HashMap::new();
    check_variant_args(&variant, args, &target_ty, program, context, reporter, &mut substitutions);
    substitute_generics(&target_ty, &substitutions)
}

#[allow(clippy::too_many_arguments)]
fn infer_function_signature_call(
    call: &Expr,
    name: &str,
    type_args: &[Ty],
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let Some(signature) = program.functions.get(name).cloned() else {
        return infer_bare_variant_construction(name, args, span, program, context, reporter);
    };
    if program.builtin_grad.contains(name) {
        return infer_grad_call(args, span, program, context, reporter);
    }
    if program.builtin_test_asserts.contains(name) {
        return infer_test_assert_call(name, args, span, program, context, reporter);
    }
    if signature.requires_unsafe {
        require_unsafe(
            reporter,
            span,
            context.in_unsafe,
            &format!("calling `{name}`"),
        );
    }
    if signature.requires_comptime {
        require_comptime(reporter, span, context.in_comptime, &format!("`{name}`"));
    }
    let mut substitutions = generic_substitutions(&signature.generics);
    if !type_args.is_empty() {
        if type_args.len() != signature.generics.len() {
            reporter.push(Diagnostic::error(
                "PACO-E0316",
                span,
                format!(
                    "generic arity mismatch for `{name}`: expected {}, found {}",
                    signature.generics.len(),
                    type_args.len()
                ),
            ));
        } else {
            for (generic_name, type_arg) in signature.generics.iter().zip(type_args) {
                let resolved = match type_arg {
                    Ty::Const(..) | Ty::DynDim(_) => program.const_arg_from_ast(type_arg, &context.generics, reporter),
                    _ => program.ty_from_ast(type_arg, &context.generics, reporter),
                };
                substitutions.insert(generic_name.clone(), resolved);
            }
        }
    }
    check_args(
        args,
        &signature.params,
        span,
        program,
        context,
        reporter,
        &mut substitutions,
        Some((name, &signature.generics)),
    );
    named::solve_from_hint(call, name, &signature.return_ty, &mut substitutions, program, context);
    named::check_const_params(name, &substitutions, span, program, reporter);
    record_instantiation(name, &substitutions, span, program, context);
    record_call_generics(call, &signature.generics, &substitutions, program);
    if let Some(bounds) = program.bounds.get(name) {
        check_bounds(bounds, &substitutions, name, span, program, Some(context), reporter);
    }
    let item_ty = substitute_generics(&signature.return_ty, &substitutions);
    let item_ty = named::open_existentials(item_ty, name, span, program, context);
    if program.iter_functions.contains(name) {
        Type::Struct("Generator".to_string(), vec![item_ty])
    } else {
        item_ty
    }
}

#[allow(clippy::too_many_arguments)]
fn infer_method_call(
    call: &Expr,
    receiver: &Expr,
    method: &str,
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    if matches!(method, "unwrap" | "expect") {
        named::forward_hint(call, receiver, context);
    }
    let (receiver_ty, receiver_borrow) = match infer_expr(receiver, program, context, reporter) {
        Type::Borrow { mutable, ty } => (*ty, Some(mutable)),
        other => (other, None),
    };
    if matches!(receiver_ty, Type::Slice(_)) && method == "len" && args.is_empty() {
        return Type::Int(IntWidth::I64);
    }
    if receiver_ty == Type::Error {
        for arg in args {
            infer_expr(arg, program, context, reporter);
        }
        return Type::Error;
    }
    let Some(type_name) = target_type_name(&receiver_ty) else {
        reporter.push(Diagnostic::error(
            "PACO-E0314",
            span,
            format!("method `{method}` not found"),
        ));
        return Type::Error;
    };
    let Some(signature) = program
        .methods
        .get(&(type_name.clone(), method.to_string()))
        .cloned()
    else {
        if let Some(ty) = named::infer_intrinsic(call, &receiver_ty, method, args, span, program, context, reporter) {
            return ty;
        }
        if matches!(receiver_ty, Type::Int(_) | Type::Float(_) | Type::Bool | Type::Char) && method == "to_string" {
            check_args(args, &[], span, program, context, reporter, &mut HashMap::new(), None);
            return Type::String;
        }
        if let Type::Int(_) = receiver_ty
            && let Some((family, _)) = int_overflow_method(method)
        {
            check_args(args, std::slice::from_ref(&receiver_ty), span, program, context, reporter, &mut HashMap::new(), None);
            return match family {
                "checked" => Type::Enum("Option".to_string(), vec![receiver_ty]),
                "overflowing" => Type::Tuple(vec![receiver_ty, Type::Bool]),
                _ => receiver_ty,
            };
        }
        if let Type::Float(width) = receiver_ty
            && let Some(arity) = float_intrinsic_arity(method)
        {
            if !width.has_arithmetic() {
                for arg in args {
                    infer_expr(arg, program, context, reporter);
                }
                reporter.push(fp8_arithmetic_diagnostic(span, width));
                return Type::Error;
            }
            let expected = vec![receiver_ty.clone(); arity];
            check_args(args, &expected, span, program, context, reporter, &mut HashMap::new(), None);
            return receiver_ty;
        }
        reporter.push(Diagnostic::error(
            "PACO-E0314",
            span,
            format!("method `{method}` not found for `{type_name}`"),
        ));
        return Type::Error;
    };
    if signature
        .receiver
        .as_ref()
        .is_some_and(|receiver| receiver.mutable)
        && !receiver_borrow.unwrap_or_else(|| is_mutable_place(receiver, program, context))
    {
        reporter.push(Diagnostic::error(
            "PACO-E0307",
            span,
            "mutable method receiver requires a mutable binding",
        ));
    }
    let mut substitutions = generic_substitutions(&signature.generics);
    substitutions.insert("Self".to_string(), receiver_ty.clone());
    if let Some(expected_receiver) = signature.body_params.first() {
        unify_type(expected_receiver, &named::subsume_dyn(expected_receiver, receiver_ty.clone(), program), &mut substitutions);
    }
    check_args(
        args,
        &signature.params,
        span,
        program,
        context,
        reporter,
        &mut substitutions,
        None,
    );
    let callee = format!("{type_name}::{method}");
    named::solve_from_hint(call, &callee, &signature.return_ty, &mut substitutions, program, context);
    named::check_const_params(&callee, &substitutions, span, program, reporter);
    named::check_broadcast(&callee, receiver, &substitutions, span, method, program, reporter);
    record_instantiation(&callee, &substitutions, span, program, context);
    record_call_generics(call, &signature.generics, &substitutions, program);
    if let Some(bounds) = program.bounds.get(&callee) {
        check_bounds(bounds, &substitutions, &callee, span, program, Some(context), reporter);
    }
    let result = substitute_generics(&signature.return_ty, &substitutions);
    named::open_existentials(result, method, span, program, context)
}

#[allow(clippy::too_many_arguments)]
fn infer_associated_call(
    call: &Expr,
    ty: &Ty,
    function: &str,
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    if let Ty::Path(path, _) = ty
        && path.len() == 1
    {
        let qualified_name = format!("{}::{function}", path[0]);
        if program.functions.contains_key(&qualified_name) {
            return infer_function_signature_call(call, &qualified_name, &[], args, span, program, context, reporter);
        }
        if program.is_private_import(&qualified_name) {
            reporter.push(Diagnostic::error(
                "PACO-E0333",
                span,
                format!("`{function}` is private in `{}`", path[0]),
            ));
            return Type::Error;
        }
    }

    let target_ty = program.ty_from_ast_omitted(ty, &context.generics, true, reporter);
    let Some(type_name) = target_type_name(&target_ty) else {
        return Type::Error;
    };

    if let Some(enum_info) = program.enums.get(&type_name)
        && let Some(variant) = enum_info
            .variants
            .iter()
            .find(|variant| variant.name == function)
    {
        let target_ty = if matches!(ty, Ty::Path(..)) { fresh_enum_params(target_ty) } else { target_ty };
        let mut substitutions = HashMap::new();
        check_variant_args(variant, args, &target_ty, program, context, reporter, &mut substitutions);
            return substitute_generics(&target_ty, &substitutions);
    }

    let Some(signature) = program
        .associated
        .get(&(type_name.clone(), function.to_string()))
        .cloned()
    else {
        reporter.push(Diagnostic::error(
            "PACO-E0314",
            span,
            format!("associated function `{function}` not found for `{type_name}`"),
        ));
        return Type::Error;
    };
    if signature.requires_comptime {
        require_comptime(reporter, span, context.in_comptime, &format!("`{type_name}::{function}`"));
    }
    let mut substitutions = generic_substitutions(&signature.generics);
    substitutions.insert("Self".to_string(), target_ty.clone());
    if let Some(info) = program.structs.get(&type_name) {
        for (name, arg) in info.generics.iter().zip(nominal_generic_args(&target_ty)) {
            if !matches!(arg, Type::Unknown | Type::Generic(_)) {
                substitutions.insert(name.clone(), arg.clone());
            }
        }
    }
    check_args(
        args,
        &signature.params,
        span,
        program,
        context,
        reporter,
        &mut substitutions,
        None,
    );
    let callee = format!("{type_name}::{function}");
    named::solve_from_hint(call, &callee, &signature.return_ty, &mut substitutions, program, context);
    named::check_const_params(&callee, &substitutions, span, program, reporter);
    record_instantiation(&callee, &substitutions, span, program, context);
    record_call_generics(call, &signature.generics, &substitutions, program);
    if matches!(ty, Ty::Generic { .. }) {
        program.call_targets.borrow_mut().insert(call as *const Expr, target_ty.clone());
    }
    if let Some(bounds) = program.bounds.get(&callee) {
        check_bounds(bounds, &substitutions, &callee, span, program, Some(context), reporter);
    }
    let result = substitute_generics(&signature.return_ty, &substitutions);
    named::open_existentials(result, function, span, program, context)
}

fn infer_struct_literal(
    ty: &Ty,
    fields: &[(String, Expr)],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    if let Ty::Path(path, _) = ty
        && path.len() > 1
        && program.is_private_import(&path.join("::"))
    {
        reporter.push(Diagnostic::error(
            "PACO-E0333",
            span,
            format!("`{}` is private in `{}`", path[path.len() - 1], path[..path.len() - 1].join("::")),
        ));
        return Type::Error;
    }
    let struct_ty = program.ty_from_ast_omitted(ty, &context.generics, true, reporter);
    let Type::Struct(name, _) = &struct_ty else {
        reporter.push(Diagnostic::error(
            "PACO-E0311",
            span,
            "struct literal target must be a struct type",
        ));
        return Type::Error;
    };
    let Some(expected_fields) = instantiated_struct_fields(&struct_ty, program, reporter) else {
        return Type::Error;
    };
    let mut substitutions = HashMap::new();
    let mut provided = HashSet::new();
    for (field_name, value) in fields {
        if !provided.insert(field_name.clone()) {
            reporter.push(Diagnostic::error(
                "PACO-E0311",
                span,
                format!("duplicate field `{field_name}`"),
            ));
        }
        let Some(expected_ty) = expected_fields.get(field_name) else {
            reporter.push(Diagnostic::error(
                "PACO-E0311",
                span,
                format!("unknown field `{field_name}` for `{name}`"),
            ));
            infer_expr(value, program, context, reporter);
            continue;
        };
        seed_closure_params(value, Some(expected_ty), context);
        let actual_ty = infer_expr(value, program, context, reporter);
        take_unproved();
        if !unify_type(expected_ty, &actual_ty, &mut substitutions) {
            let unproved = take_unproved();
            let expected_now = substitute_generics(expected_ty, &substitutions);
            reporter.push(unify_mismatch_diagnostic(span, "", &expected_now, &actual_ty, &substitutions, unproved, None));
        }
        // `actual_ty` alone can leave a callee's own generic unresolved
        // (e.g. `items: Vec::new()` against an expected `Vec<Pair<K, V>>`
        // field infers `Vec::new()` as `Vec<T>` in isolation, `T` being
        // `Vec`'s own, unrelated to this struct's `K`/`V`) — re-resolving
        // against `expected_ty` and re-caching, like `check_let` already
        // does for its own value expression, lets the field value's own
        // generic — however deep — pick up `K`/`V` from the field's
        // declared type instead of staying an unrelated bare `Generic`.
        let resolved_ty = resolve_against_expected(expected_ty, actual_ty, context);
        recache(program, value, &resolved_ty);
    }
    for field_name in expected_fields.keys() {
        if !provided.contains(field_name) {
            reporter.push(Diagnostic::error(
                "PACO-E0311",
                span,
                format!("missing field `{field_name}` for `{name}`"),
            ));
        }
    }
    substitute_generics(&struct_ty, &substitutions)
}

fn infer_field(
    base: &Expr,
    field: &str,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let base_ty = match infer_expr(base, program, context, reporter) {
        Type::Borrow { ty, .. } => *ty,
        ty => ty,
    };
    let Some(fields) = instantiated_struct_fields(&base_ty, program, reporter) else {
        reporter.push(Diagnostic::error(
            "PACO-E0311",
            span,
            format!("unknown field `{field}`"),
        ));
        return Type::Error;
    };
    if let Some(ty) = fields.get(field) {
        return named::field_type(base, ty.clone(), field, span, program, context);
    }
    fields.get(field).cloned().unwrap_or_else(|| {
        reporter.push(Diagnostic::error(
            "PACO-E0311",
            span,
            format!("unknown field `{field}`"),
        ));
        Type::Error
    })
}

/// Type-checks `base[index...]`. `[]T`/`&[]T`/`&mut []T` are a compiler-known
/// built-in (per `docs/design/spec.md` §7, "primitive types satisfy the
/// relevant magic traits natively... not through a `methods` block") and
/// bypass method lookup entirely. Any other receiver type dispatches
/// structurally to an `index` method — Paco has no `impl Trait for Type`
/// syntax (ADR 0002: trait satisfaction is structural, "if `File` has the
/// method, it satisfies `Sink`"), so `Index<Idx>` dispatch is exactly the
/// same `program.methods` lookup an ordinary `a.method()` call uses,
/// keyed on the method name `index` instead of a caller-given name.
fn infer_index(
    base: &Expr,
    index: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let base_ty = infer_expr(base, program, context, reporter);
    let receiver_ty = match &base_ty {
        Type::Borrow { ty, .. } => ty.as_ref().clone(),
        other => other.clone(),
    };
    let index_tys: Vec<Type> = index
        .iter()
        .map(|index_expr| infer_expr(index_expr, program, context, reporter))
        .collect();

    if let Type::Slice(elem) = &receiver_ty {
        if index_tys.len() != 1 {
            reporter.push(Diagnostic::error(
                "PACO-E0331",
                span,
                format!("`[]T` indexing takes exactly one index, found {}", index_tys.len()),
            ));
            return Type::Error;
        }
        if !compatible(&index_tys[0], &Type::Int(IntWidth::I64)) {
            reporter.push(Diagnostic::error(
                "PACO-E0331",
                span,
                format!("slice index must be `i64`, found {}", index_tys[0].name()),
            ));
            return Type::Error;
        }
        return elem.as_ref().clone();
    }

    let Some(type_name) = target_type_name(&receiver_ty) else {
        reporter.push(Diagnostic::error(
            "PACO-E0332",
            span,
            format!("type `{}` does not support indexing (no `index` method)", receiver_ty.name()),
        ));
        return Type::Error;
    };
    let Some(signature) = program.methods.get(&(type_name.clone(), "index".to_string())).cloned() else {
        reporter.push(Diagnostic::error(
            "PACO-E0332",
            span,
            format!(
                "`{type_name}` has no `index` method; indexing requires `fn index(&self, i: Idx) -> &Output`"
            ),
        ));
        return Type::Error;
    };

    let idx_ty = if index_tys.len() == 1 {
        index_tys.into_iter().next().expect("checked len == 1")
    } else {
        Type::Tuple(index_tys)
    };

    let mut substitutions = generic_substitutions(&signature.generics);
    substitutions.insert("Self".to_string(), receiver_ty.clone());
    if let Some(expected_receiver) = signature.body_params.first() {
        unify_type(expected_receiver, &receiver_ty, &mut substitutions);
    }
    let matched = signature
        .params
        .first()
        .is_some_and(|expected_idx| unify_type(expected_idx, &idx_ty, &mut substitutions));
    if !matched {
        let expected = signature.params.first().map(Type::name).unwrap_or_default();
        reporter.push(Diagnostic::error(
            "PACO-E0301",
            span,
            format!("type mismatch: `{type_name}::index` expects {expected}, found {}", idx_ty.name()),
        ));
        return Type::Error;
    }

    let return_ty = substitute_generics(&signature.return_ty, &substitutions);
    match return_ty {
        Type::Borrow { ty, .. } => *ty,
        other => other,
    }
}

fn operator_trait_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "Add",
        BinaryOp::Sub => "Sub",
        BinaryOp::Mul => "Mul",
        BinaryOp::Div => "Div",
        BinaryOp::Rem => "Rem",
        _ => "Ord",
    }
}

fn operator_method_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Rem => "rem",
        other => panic!("`{other:?}` has no operator-overload method name"),
    }
}

#[allow(clippy::too_many_arguments)]
fn infer_operator_method(
    call: &Expr,
    method_name: &str,
    receiver_ty: &Type,
    arg_ty: Option<&Type>,
    span: Span,
    program: &Program,
    context: &FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Option<Type> {
    let receiver_ty = match receiver_ty {
        Type::Borrow { ty, .. } => ty.as_ref(),
        other => other,
    };
    let type_name = target_type_name(receiver_ty)?;
    let Some(signature) = program.methods.get(&(type_name.clone(), method_name.to_string())).cloned() else {
        reporter.push(Diagnostic::error(
            "PACO-E0334",
            span,
            format!("`{type_name}` has no `{method_name}` method; this operator requires one"),
        ));
        return Some(Type::Error);
    };

    let mut substitutions = generic_substitutions(&signature.generics);
    substitutions.insert("Self".to_string(), receiver_ty.clone());
    if let Some(expected_receiver) = signature.body_params.first() {
        unify_type(expected_receiver, &named::subsume_dyn(expected_receiver, receiver_ty.clone(), program), &mut substitutions);
    }

    if let Some(arg_ty) = arg_ty {
        take_unproved();
        let matched = signature.params.first().is_some_and(|expected| match expected {
            Type::Borrow { mutable: false, ty } if !matches!(arg_ty, Type::Borrow { .. }) => {
                unify_type(ty, arg_ty, &mut substitutions)
            }
            expected => unify_type(expected, arg_ty, &mut substitutions),
        });
        if let (false, Some(unproved), Some(expected)) = (matched, take_unproved(), signature.params.first()) {
            let expected_now = substitute_generics(expected, &substitutions);
            reporter.push(unify_mismatch_diagnostic(span, "", &expected_now, arg_ty, &substitutions, Some(unproved), None));
            return Some(Type::Error);
        }
        if !matched {
            if let Some(expected) = signature.params.first() {
                let expected_now = substitute_generics(expected, &substitutions);
                let diagnostic = unify_mismatch_diagnostic(span, "", &expected_now, arg_ty, &substitutions, None, None);
                if diagnostic.code() != "PACO-E0302" {
                    reporter.push(diagnostic);
                    return Some(Type::Error);
                }
            }
            let expected = signature
                .params
                .first()
                .map(|expected| substitute_generics(expected, &substitutions).name())
                .unwrap_or_default();
            reporter.push(Diagnostic::error(
                "PACO-E0301",
                span,
                format!("type mismatch: `{type_name}::{method_name}` expects {expected}, found {}", arg_ty.name()),
            ));
            return Some(Type::Error);
        }
    }

    let callee = format!("{type_name}::{method_name}");
    record_instantiation(&callee, &substitutions, span, program, context);
    record_call_generics(call, &signature.generics, &substitutions, program);
    if let Some(bounds) = program.bounds.get(&callee) {
        let mut bound_errors = Reporter::new();
        check_bounds(bounds, &substitutions, &callee, span, program, Some(context), &mut bound_errors);
        if bound_errors.has_errors() {
            for diagnostic in bound_errors.diagnostics() {
                reporter.push(diagnostic.clone());
            }
            return Some(Type::Error);
        }
    }
    Some(substitute_generics(&signature.return_ty, &substitutions))
}

fn infer_assign(
    target: &Expr,
    value: &Expr,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let value_ty = infer_expr(value, program, context, reporter);
    let target_ty = match target {
        Expr::Ident(name, _) => {
            let binding = lookup(program, &context.scopes, target);
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
        Expr::Field { base, field, .. } => {
            if !is_assignable_field_base(base, program, context) {
                reporter.push(Diagnostic::error(
                    "PACO-E0307",
                    span,
                    "cannot assign through immutable binding or shared borrow",
                ));
            }
            infer_field(base, field, span, program, context, reporter)
        }
        Expr::Index { base, index, .. } => {
            if !is_assignable_field_base(base, program, context) {
                reporter.push(Diagnostic::error(
                    "PACO-E0307",
                    span,
                    "cannot assign through immutable binding or shared borrow",
                ));
            }
            infer_index(base, index, span, program, context, reporter)
        }
        Expr::Unary { op: UnaryOp::Deref, expr: pointer, .. } => {
            match infer_expr(pointer, program, context, reporter) {
                Type::Borrow { mutable: true, ty } => *ty,
                Type::RawPointer { mutable: true, ty } => {
                    require_unsafe(reporter, span, context.in_unsafe, "dereferencing a raw pointer");
                    *ty
                }
                Type::Error | Type::Unknown => Type::Error,
                other => {
                    reporter.push(Diagnostic::error(
                        "PACO-E0307",
                        span,
                        format!("cannot assign through {}: it is not a `&mut` borrow or `*mut` pointer", other.name()),
                    ));
                    Type::Error
                }
            }
        }
        _ => unsupported_expr(
            "assignment targets other than identifiers, fields, indexing, or dereferences",
            span,
            reporter,
        ),
    };
    if let Some(diagnostic) = named::outlives(target, &value_ty, span, program, context) {
        reporter.push(diagnostic);
    } else if !named::decays_to(&value_ty, &target_ty) {
        if let Some((axis, name)) = named::dyn_escape(&target_ty, &value_ty, program) {
            let diagnostic = named::escape_diagnostic(span, axis, &name, "assigned to a `Dyn` place");
            reporter.push(named::escape_fixes(diagnostic, Some(value), axis, &name, None, program));
        } else {
            reporter.push(mismatch_diagnostic(span, "", &target_ty, &value_ty));
        }
    }
    Type::Unit
}

#[allow(clippy::too_many_arguments)]
fn infer_binary(
    call: &Expr,
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let left_ty = infer_expr(left, program, context, reporter);
    let right_ty = infer_expr(right, program, context, reporter);
    // An operand that already failed to type-check (e.g. an unresolved
    // identifier) must not cascade into a second, misleading diagnostic here
    // — `compatible` treats `Error`/`Unknown` as a wildcard, but the arms
    // below need an exact `Type::Int(_)` match (to require equal widths),
    // which doesn't tolerate `Error` the way the old single-width check did.
    if matches!(&left_ty, Type::Error | Type::Unknown) || matches!(&right_ty, Type::Error | Type::Unknown)
    {
        return Type::Error;
    }
    match op {
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
            if let (Type::Float(width), Type::Float(_)) = (&left_ty, &right_ty)
                && left_ty == right_ty
                && !width.has_arithmetic()
            {
                reporter.push(fp8_arithmetic_diagnostic(span, *width));
                Type::Error
            } else if (matches!(&left_ty, Type::Int(_) | Type::Float(_)) && compatible(&left_ty, &right_ty))
                || (matches!(&left_ty, Type::Generic(_))
                    && left_ty == right_ty
                    && satisfies(&left_ty, operator_trait_name(op), program, Some(context)))
            {
                left_ty
            } else if matches!(&left_ty, Type::Int(_) | Type::Float(_)) && matches!(&right_ty, Type::Int(_) | Type::Float(_)) {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected compatible numeric types, found {} and {}; convert explicitly with `as`",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            } else if let Some(result) = {
                let before = reporter.diagnostics().len();
                let result =
                    infer_operator_method(call, operator_method_name(op), &left_ty, Some(&right_ty), span, program, context, reporter);
                if let Some(diagnostic) =
                    reporter.diagnostics_mut()[before..].iter_mut().find(|diagnostic| diagnostic.code() == "PACO-E0342")
                {
                    *diagnostic = named::operator_fixes(
                        diagnostic.clone(),
                        left,
                        right,
                        &left_ty,
                        operator_method_name(op),
                        span,
                        program,
                        context,
                    );
                }
                result
            } {
                result
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected compatible numeric types, found {} and {}",
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
        BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
            if matches!(&left_ty, Type::Int(_)) && left_ty == right_ty {
                left_ty
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: `{}` requires two operands of the same integer type, found {} and {}",
                        bitwise_symbol(op),
                        left_ty.name(),
                        right_ty.name()
                    ),
                ));
                Type::Error
            }
        }
        BinaryOp::Shl | BinaryOp::Shr => {
            if matches!(&left_ty, Type::Int(_)) && matches!(&right_ty, Type::Int(_)) {
                left_ty
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: `{}` requires integer operands, found {} and {}",
                        bitwise_symbol(op),
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
            if let Type::Float(width) = &left_ty
                && !width.has_arithmetic()
                && left_ty == right_ty
            {
                reporter.push(fp8_arithmetic_diagnostic(span, *width));
                Type::Error
            } else if (matches!(&left_ty, Type::Int(_) | Type::Char | Type::Float(_)) && compatible(&left_ty, &right_ty))
                || (matches!(&left_ty, Type::Generic(_))
                    && left_ty == right_ty
                    && satisfies(&left_ty, "Ord", program, Some(context)))
            {
                Type::Bool
            } else if matches!(&left_ty, Type::Struct(..) | Type::Enum(..)) {
                match infer_operator_method(call, "cmp", &left_ty, Some(&right_ty), span, program, context, reporter) {
                    Some(Type::Int(IntWidth::I64)) => Type::Bool,
                    Some(Type::Error) | None => Type::Error,
                    Some(other) => {
                        reporter.push(Diagnostic::error(
                            "PACO-E0301",
                            span,
                            format!("type mismatch: `cmp` must return i64 to order values, found {}", other.name()),
                        ));
                        Type::Error
                    }
                }
            } else {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!(
                        "type mismatch: expected compatible numeric types, found {} and {}",
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

fn bitwise_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::Shl => "<<",
        _ => ">>",
    }
}

/// The method-table name of `[]T`, whose methods a `methods<T> []T` block
/// declares.
pub const SLICE_TYPE_NAME: &str = "[]";

/// Splits an explicit overflow method name such as `wrapping_add` into its
/// family (`wrapping`, `saturating`, `checked`, `overflowing`) and operation.
pub fn int_overflow_method(method: &str) -> Option<(&str, &str)> {
    let (family, operation) = method.split_once('_')?;
    (matches!(family, "wrapping" | "saturating" | "checked" | "overflowing") && matches!(operation, "add" | "sub" | "mul"))
        .then_some((family, operation))
}

/// Operand count after the receiver of each float math method.
pub fn float_intrinsic_arity(method: &str) -> Option<usize> {
    match method {
        "sqrt" | "exp" | "ln" | "sin" | "cos" | "tanh" | "abs" => Some(0),
        "powf" | "min" | "max" => Some(1),
        _ => None,
    }
}

fn fp8_arithmetic_diagnostic(span: Span, width: FloatWidth) -> Diagnostic {
    Diagnostic::error(
        "PACO-E0339",
        span,
        format!("`{}` is a storage-only format with no arithmetic or ordering; convert with `as` first", width.name()),
    )
}

fn infer_unary(
    call: &Expr,
    op: UnaryOp,
    expr: &Expr,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    let ty = infer_expr(expr, program, context, reporter);
    match op {
        UnaryOp::Not if compatible(&ty, &Type::Bool) => Type::Bool,
        // Cascade-safe: an operand that already failed to type-check must
        // not trigger a second, misleading diagnostic here (see the same
        // guard in `infer_binary`).
        _ if matches!(&ty, Type::Error | Type::Unknown) => Type::Error,
        UnaryOp::Neg if matches!(&ty, Type::Int(width) if width.is_signed()) => ty,
        UnaryOp::Neg if matches!(&ty, Type::Float(width) if width.has_arithmetic()) => ty,
        UnaryOp::Neg if matches!(&ty, Type::Generic(_)) && satisfies(&ty, "Neg", program, Some(context)) => ty,
        UnaryOp::Neg if matches!(&ty, Type::Struct(..) | Type::Enum(..)) => {
            infer_operator_method(call, "neg", &ty, None, span, program, context, reporter)
                .expect("Struct/Enum receiver always resolves via target_type_name")
        }
        UnaryOp::Not => {
            reporter.push(Diagnostic::error(
                "PACO-E0301",
                span,
                format!("type mismatch: expected bool, found {}", ty.name()),
            ));
            Type::Error
        }
        UnaryOp::Neg => {
            reporter.push(Diagnostic::error(
                "PACO-E0301",
                span,
                format!("type mismatch: expected numeric, found {}", ty.name()),
            ));
            Type::Error
        }
        UnaryOp::BitNot if matches!(&ty, Type::Int(_)) => ty,
        UnaryOp::BitNot => {
            reporter.push(Diagnostic::error(
                "PACO-E0301",
                span,
                format!("type mismatch: `~` requires an integer operand, found {}", ty.name()),
            ));
            Type::Error
        }
        UnaryOp::Deref => match ty {
            Type::RawPointer { ty: pointee, .. } => {
                require_unsafe(reporter, span, context.in_unsafe, "dereferencing a raw pointer");
                *pointee
            }
            Type::Borrow { ty: pointee, .. } => *pointee,
            _ => {
                reporter.push(Diagnostic::error(
                    "PACO-E0301",
                    span,
                    format!("type mismatch: expected a borrow or raw pointer, found {}", ty.name()),
                ));
                Type::Error
            }
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn check_args(
    args: &[Expr],
    expected: &[Type],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
    substitutions: &mut HashMap<String, Type>,
    callee: Option<(&str, &[String])>,
) {
    if args.len() != expected.len() {
        reporter.push(Diagnostic::error(
            "PACO-E0305",
            span,
            format!(
                "expected {} arguments, found {}",
                expected.len(),
                args.len()
            ),
        ));
    }
    let (closures, others): (Vec<_>, Vec<_>) =
        args.iter().zip(expected).partition(|(arg, _)| matches!(arg, Expr::Closure { .. }));
    let mut deferred = Vec::new();
    for (arg, expected) in others.into_iter().chain(closures) {
        let actual = if let (Type::TypeValue(_), Expr::Ident(name, ident_span)) = (expected, arg) {
            infer_type_value_arg(name, *ident_span, arg, program, context, reporter)
        } else {
            let expected_now = substitute_generics(expected, substitutions);
            if !type_has_foreign_generic(&expected_now, &context.generics) {
                seed_closure_params(arg, Some(&expected_now), context);
                named::set_hint(arg, expected_now, context);
            }
            infer_expr(arg, program, context, reporter)
        };
        let actual = named::subsume_dyn(expected, actual, program);
        take_unproved();
        if !unify_type(expected, &actual, substitutions) {
            let unproved = take_unproved();
            let expected_now = substitute_generics(expected, substitutions);
            if unproved.is_none() && has_unbound_dim(&expected_now, substitutions) {
                deferred.push((arg, expected, actual));
            } else {
                let diagnostic = unify_mismatch_diagnostic(span, "", &expected_now, &actual, substitutions, unproved, callee);
                let diagnostic = if diagnostic.code() == "PACO-E0342" {
                    named::argument_fix(diagnostic, arg, &expected_now, &actual, program, context)
                } else {
                    diagnostic
                };
                reporter.push(diagnostic);
            }
            continue;
        }
        recache_resolved_arg(arg, expected, &actual, program, context, substitutions);
    }
    for (arg, expected, actual) in deferred {
        take_unproved();
        if unify_type(expected, &actual, substitutions) {
            recache_resolved_arg(arg, expected, &actual, program, context, substitutions);
        } else {
            let unproved = take_unproved();
            let expected_now = substitute_generics(expected, substitutions);
            reporter.push(unify_mismatch_diagnostic(span, "", &expected_now, &actual, substitutions, unproved, callee));
        }
    }
}

fn recache_resolved_arg(
    arg: &Expr,
    expected: &Type,
    actual: &Type,
    program: &Program,
    context: &FunctionContext<'_>,
    substitutions: &HashMap<String, Type>,
) {
    let expected_now = substitute_generics(expected, substitutions);
    if type_has_foreign_generic(actual, &context.generics) && !type_has_foreign_generic(&expected_now, &context.generics) {
        recache(program, arg, &expected_now);
    }
}

/// Whether a dimension of `ty` still mentions a parameter that
/// `substitutions` leaves unbound.
fn has_unbound_dim(ty: &Type, substitutions: &HashMap<String, Type>) -> bool {
    match ty {
        Type::Dim(Dim::Const(expr)) => !expr.unbound_params(substitutions).is_empty(),
        Type::Struct(_, items) | Type::Enum(_, items) | Type::Pack(items) | Type::Tuple(items) => {
            items.iter().any(|item| has_unbound_dim(item, substitutions))
        }
        Type::Borrow { ty, .. } | Type::Slice(ty) => has_unbound_dim(ty, substitutions),
        _ => false,
    }
}

/// `phase-9-comptime` Decision 5: a bare identifier passed where a `type`
/// parameter is expected resolves against a non-generic struct/enum name
/// instead of a variable binding. Caches the resolved `Type::TypeValue` on
/// `arg` itself (matching `infer_expr`'s own caching convention) so
/// `Lowerer::type_of` sees the same result later. Generic structs/enums
/// are explicitly out of scope (Decision 5) and reported as an error
/// rather than silently passed through with no type arguments.
fn infer_type_value_arg(
    name: &str,
    span: Span,
    arg: &Expr,
    program: &Program,
    context: &FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Type {
    // A bare identifier passed as a `type` argument is a literal
    // struct/enum name *unless* it's already a bound variable holding a
    // `Type::TypeValue` — e.g. a `comptime fn(t: type)`'s own `t`,
    // passed on to another `type`-typed parameter (`fields_of(t)`
    // inside `derive_display`'s own body) rather than naming a struct
    // called `t`. A real struct/enum name is never itself a bound
    // variable, so checking this first is unambiguous.
    if let Some(binding) = lookup(program, &context.scopes, arg)
        && matches!(&binding.ty, Type::TypeValue(_))
    {
        program.types.borrow_mut().insert(arg as *const Expr, binding.ty.clone());
        return binding.ty;
    }
    let resolved = if let Some(info) = program.structs.get(name) {
        if !info.generics.is_empty() {
            reporter.push(Diagnostic::error(
                "PACO-E0351",
                span,
                format!("`{name}` is generic; a `type` argument must name a non-generic struct or enum"),
            ));
            Type::Error
        } else {
            Type::TypeValue(Box::new(Type::Struct(name.to_string(), Vec::new())))
        }
    } else if let Some(info) = program.enums.get(name) {
        if !info.generics.is_empty() {
            reporter.push(Diagnostic::error(
                "PACO-E0351",
                span,
                format!("`{name}` is generic; a `type` argument must name a non-generic struct or enum"),
            ));
            Type::Error
        } else {
            Type::TypeValue(Box::new(Type::Enum(name.to_string(), Vec::new())))
        }
    } else {
        reporter.push(Diagnostic::error(
            "PACO-E0319",
            span,
            format!("unresolved identifier `{name}`"),
        ));
        Type::Error
    };
    program.types.borrow_mut().insert(arg as *const Expr, resolved.clone());
    resolved
}

/// Renames an enum's own still-unbound parameters so binding them to
/// argument types cannot capture a same-named parameter of the caller.
fn fresh_enum_params(ty: Type) -> Type {
    match ty {
        Type::Enum(name, args) => {
            let args = args
                .into_iter()
                .map(|arg| match arg {
                    Type::Generic(param) => Type::Generic(format!("{param}#{name}")),
                    other => other,
                })
                .collect();
            Type::Enum(name, args)
        }
        other => other,
    }
}

fn check_variant_args(
    variant: &VariantInfo,
    args: &[Expr],
    enum_ty: &Type,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
    substitutions: &mut HashMap<String, Type>,
) {
    let expected = match &variant.fields {
        VariantFields::Unit => Vec::new(),
        VariantFields::Tuple(tys) => tys
            .iter()
            .map(|ty| instantiate_ty(ty, enum_ty, program, reporter))
            .collect(),
        VariantFields::Struct(_) => {
            reporter.push(Diagnostic::error(
                "PACO-E0306",
                variant.span,
                "named enum variant construction is not supported yet",
            ));
            Vec::new()
        }
    };
    check_args(
        args,
        &expected,
        variant.span,
        program,
        context,
        reporter,
        substitutions,
        None,
    );
}

fn mismatch_diagnostic(
    span: Span,
    prefix: &str,
    expected: &Type,
    actual: &Type,
) -> Diagnostic {
    unify_mismatch_diagnostic(span, prefix, expected, actual, &HashMap::new(), None, None)
}

/// The diagnostic for a failed `unify_type` of `expected` (already
/// substituted) against `actual`, taking the unproved-dimension note that
/// the failure recorded.
fn unify_mismatch_diagnostic(
    span: Span,
    prefix: &str,
    expected: &Type,
    actual: &Type,
    substitutions: &HashMap<String, Type>,
    unproved: Option<String>,
    callee: Option<(&str, &[String])>,
) -> Diagnostic {
    let both = format!("expected `{}`, found `{}`", expected.name(), actual.name());
    let cannot_prove = |detail: String, note: String| {
        Diagnostic::error("PACO-E0342", span, format!("{prefix}cannot prove these dimensions are equal: {detail} ({both})"))
            .with_note(note)
    };
    let dyn_note = "`Dyn` extents are never proved equal; name the extent or use a `checked_*` operation that returns `Result`";
    if let Some(note) = unproved {
        return cannot_prove(note, dyn_note.to_string());
    }
    match first_dim_mismatch(expected, actual) {
        Some(DimMismatch::Rank(expected_rank, actual_rank)) => Diagnostic::error(
            "PACO-E0336",
            span,
            format!("{prefix}shape mismatch: expected rank {expected_rank}, found rank {actual_rank} ({both})"),
        ),
        Some(DimMismatch::Dim(index, expected_dim, actual_dim)) => {
            let detail = format!("dimension {index} expected `{}`, found `{}`", expected_dim.name(), actual_dim.name());
            let is_dyn = |ty: &Type| matches!(ty, Type::Dim(Dim::Dyn));
            let is_lit = |ty: &Type| matches!(ty, Type::Dim(Dim::Const(expr)) if expr.as_lit().is_some());
            let with_origins = |diagnostic: Diagnostic| named::origin_notes(diagnostic, &[&expected_dim, &actual_dim]);
            if (is_dyn(&expected_dim) || is_dyn(&actual_dim)) && !is_lit(&expected_dim) && !is_lit(&actual_dim) {
                return with_origins(cannot_prove(detail, dyn_note.to_string()));
            }
            let names_differ = named::is_symbolic(&expected_dim) || named::is_symbolic(&actual_dim);
            if names_differ && (is_lit(&expected_dim) || is_lit(&actual_dim)) {
                return with_origins(
                    Diagnostic::error("PACO-E0336", span, format!("{prefix}shape mismatch: {detail} ({both})"))
                        .with_note("a static extent is never equal to a run-time one; refine the value with `with_dims` or use a `Dyn` parameter"),
                );
            }
            let as_expr = |ty: &Type| match ty {
                Type::Dim(Dim::Const(expr)) => Some(expr.clone()),
                Type::Generic(name) => Some(ConstExpr::param(name)),
                _ => None,
            };
            let (Some(expected_expr), Some(actual_expr)) = (as_expr(&expected_dim), as_expr(&actual_dim)) else {
                return Diagnostic::error("PACO-E0336", span, format!("{prefix}shape mismatch: {detail} ({both})"));
            };
            if let Some((param, value)) = dims::uninferred(&expected_expr, &actual_expr, substitutions) {
                let mut diagnostic = Diagnostic::error(
                    "PACO-E0344",
                    span,
                    format!("{prefix}cannot infer `{param}` from `{expected_expr}`: {detail} ({both})"),
                );
                if let Some((name, generics)) = callee {
                    let args = generics
                        .iter()
                        .map(|generic| match (generic == param, value, substitutions.get(generic)) {
                            (true, Some(value), _) => value.to_string(),
                            (_, _, Some(bound)) if !matches!(bound, Type::Generic(_)) => bound.name(),
                            _ => "_".to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let insert = span.start() + name.rsplit("::").next().unwrap_or(name).len();
                    diagnostic = diagnostic.with_fix(
                        format!("pass `{param}` explicitly: `{name}<{args}>(..)`"),
                        vec![paco_diag::Edit::new(Span::new(span.file_id(), insert, insert), format!("<{args}>"))],
                    );
                } else {
                    diagnostic = diagnostic.with_note(format!("pass `{param}` explicitly"));
                }
                return diagnostic;
            }
            let opaque = |expr: &ConstExpr| {
                expr.terms().iter().any(|(_, factors)| factors.iter().any(|(factor, _)| matches!(factor, dims::Factor::Opaque(..))))
            };
            let diagnostic = match dims::verdict(&expected_expr, &actual_expr) {
                Verdict::CannotProve if names_differ && !opaque(&expected_expr) && !opaque(&actual_expr) => cannot_prove(
                    detail,
                    format!(
                        "`{}` and `{}` are run-time extents with different names; nothing proves them equal",
                        expected_expr.normal_form(),
                        actual_expr.normal_form()
                    ),
                ),
                Verdict::CannotProve => cannot_prove(
                    detail,
                    format!(
                        "normal forms `{}` and `{}`; `/`, `%` and overflowing terms are opaque",
                        expected_expr.normal_form(),
                        actual_expr.normal_form()
                    ),
                ),
                _ => {
                    let mut diagnostic = Diagnostic::error("PACO-E0336", span, format!("{prefix}shape mismatch: {detail} ({both})"));
                    if expected_expr.to_string() != expected_expr.normal_form() || actual_expr.to_string() != actual_expr.normal_form() {
                        diagnostic = diagnostic.with_note(format!(
                            "normal forms `{}` and `{}` differ by a constant",
                            expected_expr.normal_form(),
                            actual_expr.normal_form()
                        ));
                    }
                    diagnostic
                }
            };
            with_origins(diagnostic)
        }
        None => Diagnostic::error(
            "PACO-E0302",
            span,
            format!("{prefix}type mismatch: expected {}, found {}", expected.name(), actual.name()),
        ),
    }
}

enum DimMismatch {
    Rank(usize, usize),
    Dim(usize, Type, Type),
}

fn is_dimlike(ty: &Type) -> bool {
    match ty {
        Type::Dim(_) => true,
        Type::Generic(name) => named::is_rigid(name) || is_existential(name),
        _ => false,
    }
}

fn first_dim_mismatch(expected: &Type, actual: &Type) -> Option<DimMismatch> {
    match (expected, actual) {
        (Type::Borrow { ty: expected, .. }, Type::Borrow { ty: actual, .. })
        | (Type::Slice(expected), Type::Slice(actual)) => first_dim_mismatch(expected, actual),
        (Type::Struct(expected_name, expected_args), Type::Struct(actual_name, actual_args))
        | (Type::Enum(expected_name, expected_args), Type::Enum(actual_name, actual_args))
            if expected_name == actual_name && expected_args.len() == actual_args.len() =>
        {
            let mut position = 0;
            for (expected, actual) in expected_args.iter().zip(actual_args) {
                match (expected, actual) {
                    (Type::Pack(expected_dims), Type::Pack(actual_dims)) => {
                        let spread = matches!(expected_dims.last(), Some(Type::Spread(_)));
                        let expected_dims = if spread { &expected_dims[..expected_dims.len() - 1] } else { &expected_dims[..] };
                        if actual_dims.len() < expected_dims.len() || (!spread && actual_dims.len() != expected_dims.len()) {
                            return Some(DimMismatch::Rank(expected_dims.len(), actual_dims.len()));
                        }
                        for (expected, actual) in expected_dims.iter().zip(actual_dims) {
                            if expected != actual {
                                return Some(DimMismatch::Dim(position, expected.clone(), actual.clone()));
                            }
                            position += 1;
                        }
                    }
                    _ if (is_dimlike(expected) || is_dimlike(actual)) && expected != actual => {
                        return Some(DimMismatch::Dim(position, expected.clone(), actual.clone()));
                    }
                    _ if is_dimlike(expected) => position += 1,
                    _ => {
                        if let Some(inner) = first_dim_mismatch(expected, actual) {
                            return Some(inner);
                        }
                    }
                }
            }
            None
        }
        (Type::Tuple(expected_items), Type::Tuple(actual_items)) if expected_items.len() == actual_items.len() => {
            expected_items.iter().zip(actual_items).find_map(|(expected, actual)| first_dim_mismatch(expected, actual))
        }
        _ => None,
    }
}

fn instantiated_struct_fields(
    ty: &Type,
    program: &Program,
    reporter: &mut Reporter,
) -> Option<HashMap<String, Type>> {
    let Type::Struct(name, args) = ty else {
        return None;
    };
    let info = program.structs.get(name)?;
    if info.generics.len() != args.len() {
        reporter.push(Diagnostic::error(
            "PACO-E0316",
            Span::new_root(0, 0),
            format!(
                "generic arity mismatch for `{name}`: expected {}, found {}",
                info.generics.len(),
                args.len()
            ),
        ));
        return Some(HashMap::new());
    }
    let env = info
        .generics
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect::<HashMap<_, _>>();
    Some(
        info.fields
            .iter()
            .map(|(name, ty, _)| (name.clone(), program.ty_from_ast(ty, &env, reporter)))
            .collect(),
    )
}

fn instantiate_ty(ty: &Ty, owner: &Type, program: &Program, reporter: &mut Reporter) -> Type {
    let (env, owner_name) = match owner {
        Type::Struct(name, args) => (
            program
                .structs
                .get(name)
                .map(|info| {
                    info.generics
                        .iter()
                        .cloned()
                        .zip(args.iter().cloned())
                        .collect()
                })
                .unwrap_or_default(),
            Some(name.as_str()),
        ),
        Type::Enum(name, args) => (
            program
                .enums
                .get(name)
                .map(|info| {
                    info.generics
                        .iter()
                        .cloned()
                        .zip(args.iter().cloned())
                        .collect()
                })
                .unwrap_or_default(),
            Some(name.as_str()),
        ),
        _ => (HashMap::new(), None),
    };
    // `ty` names types the way `owner`'s own module spells them (bare,
    // even for a cross-module struct); resolve bare names against that
    // module's qualifier, as `import_functions_and_methods` does.
    let qualifier = owner_name.and_then(|name| name.rsplit_once("::")).map(|(qualifier, _)| qualifier.to_string());
    let previous = std::mem::replace(&mut *program.import_qualifier.borrow_mut(), qualifier);
    let result = program.ty_from_ast(ty, &env, reporter);
    *program.import_qualifier.borrow_mut() = previous;
    result
}

fn generic_substitutions(generics: &[String]) -> HashMap<String, Type> {
    generics
        .iter()
        .map(|name| (name.clone(), Type::Generic(name.clone())))
        .collect()
}

fn nominal_generic_args(ty: &Type) -> &[Type] {
    match ty {
        Type::Struct(_, args) | Type::Enum(_, args) => args.as_slice(),
        Type::Slice(elem) => std::slice::from_ref(elem.as_ref()),
        _ => &[],
    }
}

fn resolve_against_expected(expected: &Type, actual: Type, context: &FunctionContext<'_>) -> Type {
    let mut substitutions = HashMap::new();
    unify_type(expected, &actual, &mut substitutions);
    substitutions.retain(|name, _| !context.generics.contains_key(name));
    let resolved = substitute_generics(&actual, &substitutions);
    if type_has_foreign_generic(&resolved, &context.generics) {
        expected.clone()
    } else {
        resolved
    }
}

fn type_has_foreign_generic(ty: &Type, known: &HashMap<String, Type>) -> bool {
    match ty {
        Type::Generic(name) => !known.contains_key(name) && !is_atom(name) && !is_existential(name),
        Type::Struct(_, args) | Type::Enum(_, args) => {
            args.iter().any(|arg| type_has_foreign_generic(arg, known))
        }
        Type::Borrow { ty, .. } | Type::RawPointer { ty, .. } | Type::Slice(ty) => {
            type_has_foreign_generic(ty, known)
        }
        Type::Tuple(items) => items.iter().any(|item| type_has_foreign_generic(item, known)),
        _ => false,
    }
}

/// A parameter already bound to a rigid dimension name must agree with it.
fn is_bound(name: &str, existing: &Type) -> bool {
    match existing {
        Type::Generic(other) => other != name && is_atom(other),
        _ => true,
    }
}

/// Unifies dimensions where one side is a rigid name, which binds only the
/// callee's own unbound parameters.
fn unify_rigid(expected: &Type, actual: &Type, substitutions: &mut HashMap<String, Type>) -> bool {
    let as_dim = |ty: &Type| match ty {
        Type::Generic(name) => Some(Dim::Const(ConstExpr::param(name))),
        Type::Dim(dim) => Some(dim.clone()),
        _ => None,
    };
    let (Some(expected_dim), Some(actual_dim)) = (as_dim(expected), as_dim(actual)) else {
        return false;
    };
    if expected_dim == Dim::Dyn {
        return true;
    }
    let agreed = dims::unify(&expected_dim, &actual_dim, substitutions);
    if !agreed
        && let Dim::Const(expr) = &expected_dim
        && expr.substitute(substitutions).is_none()
    {
        record_unproved(format!("`{expr}` depends on a parameter bound to `Dyn`"));
    }
    agreed
}

fn unify_type(expected: &Type, actual: &Type, substitutions: &mut HashMap<String, Type>) -> bool {
    match (expected, actual) {
        (Type::Generic(expected_name), Type::Generic(actual_name))
            if expected_name == actual_name && is_atom(expected_name) && !substitutions.contains_key(expected_name) =>
        {
            true
        }
        (Type::Generic(name), _) if !is_atom(name) || substitutions.contains_key(name) => match substitutions.get(name) {
            Some(existing) if is_bound(name, existing) => {
                let mut decayed = None;
                let agreed = rebinding_agrees_at(name, &existing.clone(), actual, &mut decayed);
                if let Some(decayed) = decayed {
                    substitutions.insert(name.clone(), decayed);
                }
                agreed
            }
            _ => {
                substitutions.insert(name.clone(), actual.clone());
                true
            }
        },
        (_, Type::Generic(name)) if !is_atom(name) => match substitutions.get(name) {
            Some(existing) if is_bound(name, existing) => rebinding_agrees(name, existing, expected),
            _ => {
                substitutions.insert(name.clone(), expected.clone());
                true
            }
        },
        (Type::Generic(_), _) | (_, Type::Generic(_)) => unify_rigid(expected, actual, substitutions),
        (Type::Struct(expected_name, expected_args), Type::Struct(actual_name, actual_args))
        | (Type::Enum(expected_name, expected_args), Type::Enum(actual_name, actual_args)) => {
            expected_name == actual_name
                && expected_args.len() == actual_args.len()
                && expected_args
                    .iter()
                    .zip(actual_args)
                    .all(|(expected, actual)| unify_type(expected, actual, substitutions))
        }
        (
            Type::Borrow {
                mutable: expected_mutable,
                ty: expected,
            },
            Type::Borrow {
                mutable: actual_mutable,
                ty: actual,
            },
        ) => expected_mutable == actual_mutable && unify_type(expected, actual, substitutions),
        (
            Type::RawPointer {
                mutable: expected_mutable,
                ty: expected,
            },
            Type::RawPointer {
                mutable: actual_mutable,
                ty: actual,
            },
        ) => expected_mutable == actual_mutable && unify_type(expected, actual, substitutions),
        (Type::Tuple(expected_items), Type::Tuple(actual_items)) => {
            expected_items.len() == actual_items.len()
                && expected_items
                    .iter()
                    .zip(actual_items)
                    .all(|(expected, actual)| unify_type(expected, actual, substitutions))
        }
        (Type::Slice(expected_elem), Type::Slice(actual_elem)) => {
            unify_type(expected_elem, actual_elem, substitutions)
        }
        (Type::Fn(expected_params, expected_ret), Type::Fn(actual_params, actual_ret)) => {
            expected_params.len() == actual_params.len()
                && expected_params
                    .iter()
                    .zip(actual_params)
                    .all(|(expected, actual)| unify_type(expected, actual, substitutions))
                && unify_type(expected_ret, actual_ret, substitutions)
        }
        (Type::Pack(expected_items), Type::Pack(actual_items)) => {
            if let Some(Type::Spread(rest)) = expected_items.last()
                && actual_items.last() != Some(&Type::Spread(rest.clone()))
            {
                let fixed = expected_items.len() - 1;
                return actual_items.len() >= fixed
                    && expected_items[..fixed]
                        .iter()
                        .zip(actual_items)
                        .all(|(expected, actual)| unify_type(expected, actual, substitutions))
                    && unify_type(
                        &Type::Generic(rest.clone()),
                        &Type::Pack(actual_items[fixed..].to_vec()),
                        substitutions,
                    );
            }
            expected_items.len() == actual_items.len()
                && expected_items
                    .iter()
                    .zip(actual_items)
                    .all(|(expected, actual)| unify_type(expected, actual, substitutions))
        }
        (Type::Dim(expected_dim), Type::Dim(actual_dim)) => {
            let agreed = dims::unify(expected_dim, actual_dim, substitutions);
            if !agreed
                && let Dim::Const(expr) = expected_dim
                && expr.substitute(substitutions).is_none()
            {
                record_unproved(format!("`{expr}` depends on a parameter bound to `Dyn`"));
            }
            agreed
        }
        (Type::Dim(_), _) | (_, Type::Dim(_)) => compatible(actual, expected),
        _ => compatible(actual, expected),
    }
}

thread_local! {
    static UNPROVED_DIM: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn record_unproved(note: String) {
    UNPROVED_DIM.with(|slot| *slot.borrow_mut() = Some(note));
}

fn take_unproved() -> Option<String> {
    UNPROVED_DIM.with(|slot| slot.borrow_mut().take())
}

/// A dimension parameter already bound to `Dyn` never proves a second
/// occurrence; a type parameter holding a `Dyn` type compares as a type.
fn rebinding_agrees(name: &str, existing: &Type, actual: &Type) -> bool {
    rebinding_agrees_at(name, existing, actual, &mut None)
}

/// Like [`rebinding_agrees`]; a type parameter whose two bindings differ
/// only in anonymous extents is rebound to the type with those decayed to
/// `Dyn`, which `decayed` returns.
fn rebinding_agrees_at(name: &str, existing: &Type, actual: &Type, decayed: &mut Option<Type>) -> bool {
    if !matches!(existing, Type::Dim(_) | Type::Pack(_) | Type::Generic(_))
        && !compatible(actual, existing)
        && compatible(&named::erase_anonymous(actual), &named::erase_anonymous(existing))
    {
        *decayed = Some(named::erase_anonymous(existing));
        return true;
    }
    let dyn_pair = |pair: (&Type, &Type)| matches!(pair, (Type::Dim(Dim::Dyn), Type::Dim(Dim::Dyn)));
    let unproved = match (existing, actual) {
        (Type::Pack(existing), Type::Pack(actual)) => existing.iter().zip(actual).any(dyn_pair),
        pair => dyn_pair(pair),
    };
    if unproved {
        record_unproved(format!("`{name}` is bound to `{}` by an earlier argument", existing.name()));
        return false;
    }
    compatible(actual, existing)
}

thread_local! {
    static SUBSTITUTING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn substitute_generics(ty: &Type, substitutions: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Generic(name) => {
            let Some(val) = substitutions.get(name) else {
                return Type::Generic(name.clone());
            };
            // A binding may mention a same-named parameter of another item
            // (`T` ↦ `Tensor<T, ..>`); it is resolved once, never re-entered.
            if val == ty || SUBSTITUTING.with(|stack| stack.borrow().contains(name)) {
                return val.clone();
            }
            SUBSTITUTING.with(|stack| stack.borrow_mut().push(name.clone()));
            let resolved = substitute_generics(val, substitutions);
            SUBSTITUTING.with(|stack| stack.borrow_mut().pop());
            resolved
        }
        Type::Struct(name, args) => Type::Struct(
            name.clone(),
            args.iter()
                .map(|arg| substitute_generics(arg, substitutions))
                .collect(),
        ),
        Type::Enum(name, args) => Type::Enum(
            name.clone(),
            args.iter()
                .map(|arg| substitute_generics(arg, substitutions))
                .collect(),
        ),
        Type::Borrow { mutable, ty } => Type::Borrow {
            mutable: *mutable,
            ty: Box::new(substitute_generics(ty, substitutions)),
        },
        Type::RawPointer { mutable, ty } => Type::RawPointer {
            mutable: *mutable,
            ty: Box::new(substitute_generics(ty, substitutions)),
        },
        Type::Tuple(items) => Type::Tuple(
            items
                .iter()
                .map(|item| substitute_generics(item, substitutions))
                .collect(),
        ),
        Type::Slice(elem) => Type::Slice(Box::new(substitute_generics(elem, substitutions))),
        Type::Fn(params, ret) => Type::Fn(
            params.iter().map(|param| substitute_generics(param, substitutions)).collect(),
            Box::new(substitute_generics(ret, substitutions)),
        ),
        Type::Dim(dim) => dims::substitute_dim(dim, substitutions),
        Type::Pack(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Type::Spread(name) => match substitutions.get(name) {
                        Some(Type::Pack(bound)) => {
                            out.extend(bound.iter().map(|item| substitute_generics(item, substitutions)))
                        }
                        Some(Type::Generic(other)) => out.push(Type::Spread(other.clone())),
                        _ => out.push(item.clone()),
                    },
                    other => out.push(substitute_generics(other, substitutions)),
                }
            }
            pack_type(out)
        }
        other => other.clone(),
    }
}

fn receiver_from_param(param: &Param) -> Option<Receiver> {
    let Pat::Ident(name, _) = &param.pattern else {
        return None;
    };
    if name != "self" {
        return None;
    }
    match &param.ty {
        Ty::Borrow { mutable, .. } => Some(Receiver { mutable: *mutable }),
        Ty::Path(_, _) => Some(Receiver { mutable: false }),
        _ => None,
    }
}

fn target_type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Struct(name, _) | Type::Enum(name, _) => Some(name.clone()),
        Type::Slice(_) => Some(SLICE_TYPE_NAME.to_string()),
        Type::Int(_) | Type::Float(_) | Type::Bool | Type::String | Type::Char | Type::Code => {
            Some(ty.name())
        }
        _ => None,
    }
}

fn is_mutable_place(expr: &Expr, program: &Program, context: &FunctionContext<'_>) -> bool {
    match expr {
        Expr::Ident(..) => {
            lookup(program, &context.scopes, expr).is_some_and(|binding| binding.mutable)
        }
        Expr::Field { base, .. } | Expr::Index { base, .. } => is_assignable_field_base(base, program, context),
        Expr::Unary { op: UnaryOp::Deref, expr: pointer, .. } => match pointer.as_ref() {
            Expr::Ident(..) => lookup(program, &context.scopes, pointer).is_some_and(|binding| {
                matches!(binding.ty, Type::Borrow { mutable: true, .. } | Type::RawPointer { mutable: true, .. })
            }),
            _ => false,
        },
        _ => false,
    }
}

fn is_assignable_field_base(expr: &Expr, program: &Program, context: &FunctionContext<'_>) -> bool {
    match expr {
        Expr::Ident(..) => lookup(program, &context.scopes, expr).is_some_and(|binding| {
            matches!(binding.ty, Type::Borrow { mutable: true, .. })
                || (binding.mutable && !matches!(binding.ty, Type::Borrow { mutable: false, .. }))
        }),
        Expr::Field { base, .. } | Expr::Index { base, .. } => is_assignable_field_base(base, program, context),
        Expr::Unary { op: UnaryOp::Deref, .. } => is_mutable_place(expr, program, context),
        _ => false,
    }
}

fn place_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Field { base, .. } => place_name(base),
        _ => None,
    }
}

fn ty_span(ty: &Ty) -> Span {
    match ty {
        Ty::Path(_, span)
        | Ty::Generic { span, .. }
        | Ty::Tuple(_, span)
        | Ty::Slice(_, span)
        | Ty::Dyn { span, .. }
        | Ty::Fn { span, .. }
        | Ty::Infer(span)
        | Ty::Never(span)
        | Ty::Const(_, span)
        | Ty::DynDim(span)
        | Ty::Expand(_, span)
        | Ty::Existential(_, span)
        | Ty::Borrow { span, .. }
        | Ty::RawPointer { span, .. }
        | Ty::Splice(_, span) => *span,
    }
}

fn pattern_span(pattern: &Pat) -> Span {
    match pattern {
        Pat::Ident(_, span)
        | Pat::Wildcard(span)
        | Pat::Literal(_, span)
        | Pat::Tuple(_, span)
        | Pat::Struct { span, .. }
        | Pat::Enum { span, .. }
        | Pat::Range { span, .. }
        | Pat::Or(_, span)
        | Pat::Binding { span, .. } => *span,
    }
}

fn literal_type(literal: &Literal) -> Type {
    match literal {
        Literal::Int(_) => Type::Int(IntWidth::I64),
        Literal::Float(_) => Type::Float(FloatWidth::F64),
        Literal::Bool(_) => Type::Bool,
        Literal::String(_) => Type::String,
        Literal::Char(_) => Type::Char,
    }
}

/// ADR 0018: an `extern` call is opaque native code with no yield point, so a
/// direct one inside a spawned task can stall the whole worker pool.
/// `spawn_blocking` is the sanctioned fix, but the compiler cannot know
/// whether a given foreign function actually blocks — so this is a warning,
/// not an error (compilation still succeeds).
fn warn_blocking_calls_on_worker(expr: &Expr, program: &Program, reporter: &mut Reporter) {
    let mut finder = ExternCallFinder {
        extern_functions: &program.extern_functions,
        found: Vec::new(),
    };
    finder.visit_expr(expr);
    for (name, span) in finder.found {
        reporter.push(Diagnostic::new(
            "blocking-call-on-worker",
            paco_diag::Severity::Warning,
            span,
            format!(
                "direct call to extern function `{name}` inside a spawned task may stall the worker pool; wrap it in `spawn_blocking`"
            ),
        ));
    }
}

struct ExternCallFinder<'a> {
    extern_functions: &'a HashSet<String>,
    found: Vec<(String, Span)>,
}

impl ast::Visit for ExternCallFinder<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call { callee, .. } = expr
            && matches!(callee.as_ref(), Expr::Ident(name, _) if name == "spawn_blocking")
        {
            return;
        }
        if let Expr::Call { callee, span, .. } = expr
            && let Expr::Ident(name, _) = callee.as_ref()
            && self.extern_functions.contains(name)
        {
            self.found.push((name.clone(), *span));
        }
        ast::walk_expr(self, expr);
    }
}

fn unsupported_expr(feature: &str, span: Span, reporter: &mut Reporter) -> Type {
    reporter.push(Diagnostic::error(
        "PACO-E0306",
        span,
        format!("expression is not supported yet: {feature}"),
    ));
    Type::Error
}

fn lookup(program: &Program, scopes: &[HashMap<LocalId, Binding>], expr: &Expr) -> Option<Binding> {
    let id = program.locals.expr(expr)?;
    scopes.iter().rev().find_map(|scope| scope.get(&id).cloned())
}

fn bind_local(pattern: &Pat, ty: Type, mutable: bool, program: &Program, context: &mut FunctionContext<'_>) {
    if let Some(id) = program.locals.pat(pattern) {
        let ty = match pattern {
            Pat::Ident(name, span) if !mutable => named::open_binding(ty, name, id, *span, program, context),
            _ => ty,
        };
        context.scopes.last_mut().unwrap().insert(id, Binding { ty, mutable, closure: None });
    }
}

fn compatible(actual: &Type, expected: &Type) -> bool {
    actual == expected
        || matches!(actual, Type::Unknown | Type::Error)
        || matches!(expected, Type::Unknown | Type::Error)
        || matches!(
            (actual, expected),
            (Type::TypeValue(_), Type::TypeValue(_))
        )
        || match (actual, expected) {
            (Type::Struct(a, actual_args), Type::Struct(b, expected_args))
            | (Type::Enum(a, actual_args), Type::Enum(b, expected_args)) => {
                a == b && actual_args.len() == expected_args.len() && actual_args.iter().zip(expected_args).all(|(a, e)| compatible(a, e))
            }
            (Type::Tuple(actual_items), Type::Tuple(expected_items)) => {
                actual_items.len() == expected_items.len()
                    && actual_items.iter().zip(expected_items).all(|(a, e)| compatible(a, e))
            }
            (Type::Borrow { mutable: a, ty: actual }, Type::Borrow { mutable: e, ty: expected }) => a == e && compatible(actual, expected),
            _ => false,
        }
}

fn join_branch_types(left: &Type, right: &Type) -> Option<Type> {
    if compatible(left, right) {
        Some(left.clone())
    } else if *left == Type::Never {
        Some(right.clone())
    } else if *right == Type::Never {
        Some(left.clone())
    } else if has_unbound_variant_param(left) || has_unbound_variant_param(right) {
        let mut substitutions = HashMap::new();
        if unify_type(left, right, &mut substitutions) && unify_type(right, left, &mut substitutions) {
            Some(substitute_generics(if has_unbound_variant_param(left) { right } else { left }, &substitutions))
        } else {
            None
        }
    } else {
        None
    }
}

fn recache(program: &Program, expr: &Expr, ty: &Type) {
    program.types.borrow_mut().insert(expr as *const Expr, ty.clone());
    refine_children(program, expr, ty);
}

fn refine_branch(program: &Program, expr: &Expr, target: &Type) {
    let cached = program.types.borrow().get(&(expr as *const Expr)).cloned();
    let Some(cached) = cached else { return };
    if !has_unbound_variant_param(&cached) {
        return;
    }
    let mut substitutions = HashMap::new();
    if unify_type(&cached, target, &mut substitutions) {
        let refined = substitute_generics(&cached, &substitutions);
        program.types.borrow_mut().insert(expr as *const Expr, refined.clone());
        refine_children(program, expr, &refined);
    }
}

fn refine_children(program: &Program, expr: &Expr, target: &Type) {
    match expr {
        Expr::Block(block) | Expr::Unsafe(block, _) => {
            if let Some(tail) = &block.tail {
                refine_branch(program, tail, target);
            }
        }
        Expr::If { then_branch, else_branch, .. } => {
            if let Some(tail) = &then_branch.tail {
                refine_branch(program, tail, target);
            }
            if let Some(else_branch) = else_branch {
                refine_branch(program, else_branch, target);
            }
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                refine_branch(program, &arm.body, target);
            }
        }
        _ => {}
    }
}

fn has_unbound_variant_param(ty: &Type) -> bool {
    match ty {
        Type::Generic(name) => name.contains('#'),
        Type::Struct(_, args) | Type::Enum(_, args) | Type::Tuple(args) | Type::Pack(args) => {
            args.iter().any(has_unbound_variant_param)
        }
        Type::Borrow { ty, .. } | Type::RawPointer { ty, .. } | Type::Slice(ty) => has_unbound_variant_param(ty),
        _ => false,
    }
}

impl Type {
    /// A human-readable name (`"Vec<i64>"`, `"&mut Foo"`, ...), used in
    /// diagnostics and by `comptime`'s `type_name`.
    pub fn name(&self) -> String {
        match self {
            Type::Int(width) => width.name().to_string(),
            Type::Float(width) => width.name().to_string(),
            Type::Bool => "bool".to_string(),
            Type::String => "string".to_string(),
            Type::Char => "char".to_string(),
            Type::Unit => "unit".to_string(),
            Type::Never => "never".to_string(),
            Type::Struct(name, args) | Type::Enum(name, args) if args.is_empty() => name.clone(),
            Type::Struct(name, args) | Type::Enum(name, args) => {
                let args = args.iter().map(Type::name).collect::<Vec<_>>().join(", ");
                format!("{name}<{args}>")
            }
            Type::Borrow { mutable, ty } if *mutable => format!("&mut {}", ty.name()),
            Type::Borrow { ty, .. } => format!("&{}", ty.name()),
            Type::RawPointer { mutable, ty } if *mutable => format!("*mut {}", ty.name()),
            Type::RawPointer { ty, .. } => format!("*const {}", ty.name()),
            Type::Slice(ty) => format!("[]{}", ty.name()),
            Type::Tuple(items) => {
                format!("({})", items.iter().map(Type::name).collect::<Vec<_>>().join(", "))
            }
            Type::Fn(params, ret) => format!(
                "fn({}) -> {}",
                params.iter().map(Type::name).collect::<Vec<_>>().join(", "),
                ret.name()
            ),
            Type::Generic(name) => display_name(name),
            Type::Dim(dim) => dim.to_string(),
            Type::Pack(items) => items.iter().map(Type::name).collect::<Vec<_>>().join(", "),
            Type::Spread(name) => format!("{name}..."),
            Type::TypeValue(_) => "type".to_string(),
            Type::Code => "Code".to_string(),
            Type::Unknown => "unknown".to_string(),
            Type::Error => "error".to_string(),
        }
    }
}

#[cfg(test)]
mod unify_alloc_tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Counting;

    thread_local! {
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static GLOBAL: Counting = Counting;

    #[test]
    fn unifying_dimension_pairs_allocates_nothing() {
        let m = ConstExpr::param("M");
        let k = ConstExpr::param("K");
        let pairs = [
            (Type::Dim(Dim::Const(ConstExpr::binary(DimOp::Mul, &m, &ConstExpr::lit(2)))), Type::Dim(Dim::Const(ConstExpr::lit(6)))),
            (Type::Dim(Dim::Const(ConstExpr::binary(DimOp::Add, &m, &k))), Type::Dim(Dim::Const(ConstExpr::lit(8)))),
            (Type::Dim(Dim::Const(ConstExpr::binary(DimOp::Add, &m, &m))), Type::Dim(Dim::Const(ConstExpr::binary(DimOp::Mul, &ConstExpr::lit(2), &m)))),
            (Type::Dim(Dim::Dyn), Type::Dim(Dim::Dyn)),
        ];
        let mut substitutions = HashMap::from([
            ("M".to_string(), Type::Dim(Dim::Const(ConstExpr::lit(3)))),
            ("K".to_string(), Type::Dim(Dim::Const(ConstExpr::lit(5)))),
        ]);
        for (expected, actual) in &pairs {
            assert!(unify_type(expected, actual, &mut substitutions));
        }
        let before = ALLOCATIONS.with(Cell::get);
        for index in 0..10_000 {
            let (expected, actual) = &pairs[index % pairs.len()];
            assert!(unify_type(expected, actual, &mut substitutions));
        }
        assert_eq!(ALLOCATIONS.with(Cell::get) - before, 0);
    }
}
