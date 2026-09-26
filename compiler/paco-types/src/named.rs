//! Named dimensions: rigid names for run-time extents (witnesses, opened
//! `Dyn` positions and opened existentials), the shape intrinsics every
//! `Shaped` type gets, and the diagnostics and fixes about them.

use super::*;
use paco_diag::Edit;

/// Where the run-time value of a dimension name comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AtomSource {
    /// The value of this local (`let n = x.dim(0);`, `dims::witness`).
    Value(LocalId),
    /// `local(.field).extent(axis)`, read once when `local` is bound; `ty`
    /// is the type of the place read.
    Extent { local: LocalId, field: Option<String>, axis: usize, ty: Type },
    /// The `dim` parameter of this name of the enclosing item.
    Param(String),
    Unknown,
}

#[derive(Clone, Debug)]
pub struct AtomInfo {
    pub origin: Span,
    pub source: AtomSource,
    pub binding: Option<String>,
    /// `file:line` of `origin`, filled in by the driver.
    pub origin_text: String,
}

/// Inside an item's body, each `dim` parameter is a rigid name, so it never
/// unifies with anything but itself even where a callee's parameter has the
/// same spelling.
pub(crate) fn open_dim_params<'p>(
    params: impl IntoIterator<Item = &'p ast::GenericParam>,
    program: &Program,
    context: &mut FunctionContext<'_>,
) -> HashMap<String, Type> {
    let mut map = HashMap::new();
    for param in params.into_iter().filter(|param| param.is_dim()) {
        let atom = new_atom(&param.name, param.span, AtomSource::Param(param.name.clone()), None, program, context);
        context.generics.insert(param.name.clone(), Type::Generic(atom.clone()));
        map.insert(param.name.clone(), Type::Generic(atom));
    }
    map
}

/// A `let` whose type has dimension arguments, for `paco shapes`.
#[derive(Clone, Debug)]
pub struct ShapeRow {
    pub span: Span,
    pub name: String,
    pub ty: Type,
}

#[derive(Default)]
pub(crate) struct DimState {
    hints: HashMap<*const Expr, Type>,
    depth: HashMap<String, usize>,
    names: Vec<(usize, String, Option<Type>)>,
    unclaimed: HashMap<String, String>,
    fields: HashMap<LocalId, HashMap<String, Type>>,
    pub(crate) statement: Option<Span>,
    pub(crate) return_ty: Option<Ty>,
}

thread_local! {
    static RIGID: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static ORIGINS: RefCell<HashMap<String, Span>> = RefCell::new(HashMap::new());
    static ANONYMOUS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static PARAMS: RefCell<HashMap<String, Span>> = RefCell::new(HashMap::new());
    static RECORD_SHAPES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes type checking on this thread record the shape of every `let`
/// (`paco shapes`); off by default, since nothing else reads them.
pub fn record_shapes(on: bool) {
    RECORD_SHAPES.with(|flag| flag.set(on));
}

/// The dimension parameters of the item being checked, for notes naming
/// where a dimension in a diagnostic comes from.
pub(crate) fn set_params<'p>(params: impl IntoIterator<Item = &'p ast::GenericParam>) {
    let params = params.into_iter().filter(|param| param.is_const() || param.is_dim()).map(|param| (param.name.clone(), param.span)).collect();
    PARAMS.with(|slot| *slot.borrow_mut() = params);
}

/// An opened `Dyn` nobody named: it has an identity, but may still decay to
/// `Dyn` wherever a `Dyn` is expected.
pub(crate) fn is_anonymous(name: &str) -> bool {
    ANONYMOUS.with(|anonymous| anonymous.borrow().contains(name))
}

fn set_anonymous(name: &str, anonymous: bool) {
    ANONYMOUS.with(|set| {
        if anonymous {
            set.borrow_mut().insert(name.to_string());
        } else {
            set.borrow_mut().remove(name);
        }
    });
}

/// A dimension the program named: a witness, a `dim` parameter or an
/// existential. Converting it to `Dyn` or to a `const` parameter loses it.
pub(crate) fn is_named(ty: &Type) -> bool {
    let named = |name: &str| (is_rigid(name) && !is_anonymous(name)) || dims::is_existential(name);
    match ty {
        Type::Generic(name) => named(name),
        Type::Dim(Dim::Const(expr)) => expr.any_name(named),
        _ => false,
    }
}

/// `ty` with every anonymous name decayed back to `Dyn`.
pub(crate) fn erase_anonymous(ty: &Type) -> Type {
    match ty {
        Type::Generic(name) if is_anonymous(name) => Type::Dim(Dim::Dyn),
        Type::Dim(Dim::Const(expr)) if expr.any_name(is_anonymous) => Type::Dim(Dim::Dyn),
        Type::Struct(name, items) => Type::Struct(name.clone(), items.iter().map(erase_anonymous).collect()),
        Type::Enum(name, items) => Type::Enum(name.clone(), items.iter().map(erase_anonymous).collect()),
        Type::Tuple(items) => Type::Tuple(items.iter().map(erase_anonymous).collect()),
        Type::Pack(items) => Type::Pack(items.iter().map(erase_anonymous).collect()),
        Type::Borrow { mutable, ty } => Type::Borrow { mutable: *mutable, ty: Box::new(erase_anonymous(ty)) },
        Type::Slice(ty) => Type::Slice(Box::new(erase_anonymous(ty))),
        other => other.clone(),
    }
}

/// `compatible`, letting anonymous extents decay to `Dyn`.
pub(crate) fn decays_to(actual: &Type, expected: &Type) -> bool {
    compatible(actual, expected) || compatible(&erase_anonymous(actual), expected)
}

/// While a signature is resolved, its `dim` parameters count as run-time
/// dimensions (for `PACO-E0347`); a body names them by atoms instead.
pub(crate) fn set_rigid(names: HashSet<String>) -> HashSet<String> {
    RIGID.with(|rigid| std::mem::replace(&mut *rigid.borrow_mut(), names))
}

pub(crate) fn dim_param_names<'p>(params: impl IntoIterator<Item = &'p ast::GenericParam>) -> HashSet<String> {
    params.into_iter().filter(|param| param.is_dim()).map(|param| param.name.clone()).collect()
}

pub(crate) fn is_rigid(name: &str) -> bool {
    dims::is_atom(name) || RIGID.with(|rigid| rigid.borrow().contains(name))
}

pub(crate) fn origin_of(name: &str) -> Option<Span> {
    ORIGINS.with(|origins| origins.borrow().get(name).copied())
}

/// Every dimension position of a struct type (through one borrow), in order.
pub(crate) fn slots(ty: &Type, program: &Program) -> Option<Vec<Type>> {
    let Type::Struct(name, args) = strip(ty) else { return None };
    let info = program.structs.get(name)?;
    let mut out = Vec::new();
    for (arg, kind) in args.iter().zip(&info.kinds) {
        match (kind, arg) {
            (ParamKind::Const | ParamKind::Dim, _) => out.push(arg.clone()),
            (ParamKind::Pack, Type::Pack(items)) => {
                out.extend(items.iter().filter(|item| !matches!(item, Type::Spread(_))).cloned())
            }
            _ => {}
        }
    }
    Some(out)
}

/// Rebuilds a struct type (through one borrow), replacing the dimension
/// positions `f` returns a type for.
pub(crate) fn map_slots(ty: &Type, program: &Program, f: &mut dyn FnMut(usize, &Type) -> Option<Type>) -> Type {
    match ty {
        Type::Borrow { mutable, ty: inner } => {
            Type::Borrow { mutable: *mutable, ty: Box::new(map_slots(inner, program, f)) }
        }
        Type::Struct(name, args) => {
            let Some(info) = program.structs.get(name) else { return ty.clone() };
            let mut axis = 0;
            let mut out = Vec::with_capacity(args.len());
            for (index, arg) in args.iter().enumerate() {
                match (info.kinds.get(index), arg) {
                    (Some(ParamKind::Const | ParamKind::Dim), _) => {
                        out.push(f(axis, arg).unwrap_or_else(|| arg.clone()));
                        axis += 1;
                    }
                    (Some(ParamKind::Pack), Type::Pack(items)) => {
                        let items = items
                            .iter()
                            .map(|item| {
                                if matches!(item, Type::Spread(_)) {
                                    return item.clone();
                                }
                                let mapped = f(axis, item).unwrap_or_else(|| item.clone());
                                axis += 1;
                                mapped
                            })
                            .collect();
                        out.push(Type::Pack(items));
                    }
                    _ => out.push(arg.clone()),
                }
            }
            Type::Struct(name.clone(), out)
        }
        other => other.clone(),
    }
}

fn strip(ty: &Type) -> &Type {
    match ty {
        Type::Borrow { ty, .. } => ty,
        other => other,
    }
}

fn is_static(ty: &Type) -> bool {
    matches!(ty, Type::Dim(Dim::Const(expr)) if expr.is_ground())
}

/// Whether a dimension has an identity known only at run time.
pub(crate) fn is_symbolic(ty: &Type) -> bool {
    match ty {
        Type::Generic(name) => is_rigid(name) || dims::is_existential(name),
        Type::Dim(Dim::Const(expr)) => expr.any_name(|name| is_rigid(name) || dims::is_existential(name)),
        _ => false,
    }
}

/// Every atom a type mentions.
pub(crate) fn atoms_in(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Generic(name) if dims::is_atom(name) && !out.contains(name) => out.push(name.clone()),
        Type::Generic(_) => {}
        Type::Dim(Dim::Const(expr)) if expr.any_name(dims::is_atom) => {
            for name in expr.names() {
                if dims::is_atom(name) && !out.iter().any(|known| known == name) {
                    out.push(name.to_string());
                }
            }
        }
        Type::Struct(_, items) | Type::Enum(_, items) | Type::Tuple(items) | Type::Pack(items) => {
            items.iter().for_each(|item| atoms_in(item, out))
        }
        Type::Borrow { ty, .. } | Type::RawPointer { ty, .. } | Type::Slice(ty) => atoms_in(ty, out),
        Type::Fn(params, ret) => {
            params.iter().for_each(|param| atoms_in(param, out));
            atoms_in(ret, out);
        }
        _ => {}
    }
}

/// Replaces every name `map` covers, anywhere in `ty`.
pub(crate) fn rename(ty: &Type, map: &HashMap<String, Type>) -> Type {
    if map.is_empty() {
        return ty.clone();
    }
    substitute_generics(ty, map)
}

pub(crate) fn new_atom(
    display: &str,
    origin: Span,
    source: AtomSource,
    binding: Option<String>,
    program: &Program,
    context: &mut FunctionContext<'_>,
) -> String {
    let name = dims::fresh_atom(display);
    set_anonymous(&name, false);
    program.atoms.borrow_mut().insert(name.clone(), AtomInfo { origin, source, binding, origin_text: String::new() });
    ORIGINS.with(|origins| origins.borrow_mut().insert(name.clone(), origin));
    context.dims.depth.insert(name.clone(), context.scopes.len());
    name
}

fn claim(atom: &str, display: String, origin: Span, source: AtomSource, binding: &str, program: &Program, context: &mut FunctionContext<'_>) {
    dims::rename_atom(atom, &display);
    if let Some(info) = program.atoms.borrow_mut().get_mut(atom) {
        info.origin = origin;
        info.source = source;
        info.binding = Some(binding.to_string());
    }
    ORIGINS.with(|origins| origins.borrow_mut().insert(atom.to_string(), origin));
    context.dims.depth.insert(atom.to_string(), context.scopes.len());
}

/// Gives `?b` in a producer's type a fresh name for this use; a `let` that
/// binds the value then names it after the binding.
pub(crate) fn open_existentials(ty: Type, producer: &str, span: Span, program: &Program, context: &mut FunctionContext<'_>) -> Type {
    fn collect(ty: &Type, out: &mut Vec<String>) {
        match ty {
            Type::Generic(name) if dims::is_existential(name) && !out.contains(name) => out.push(name.clone()),
            Type::Struct(_, items) | Type::Enum(_, items) | Type::Tuple(items) | Type::Pack(items) => {
                items.iter().for_each(|item| collect(item, out))
            }
            Type::Borrow { ty, .. } | Type::Slice(ty) => collect(ty, out),
            _ => {}
        }
    }
    let mut names = Vec::new();
    collect(&ty, &mut names);
    if names.is_empty() {
        return ty;
    }
    let producer = producer.rsplit("::").next().unwrap_or(producer);
    let mut map = HashMap::new();
    for name in names {
        let short = name.trim_start_matches('?').to_string();
        let atom = new_atom(&format!("{producer}.{short}"), span, AtomSource::Unknown, None, program, context);
        context.dims.unclaimed.insert(atom.clone(), short);
        map.insert(name, Type::Generic(atom));
    }
    rename(&ty, &map)
}

fn has_existential_fields(fields: &HashMap<String, Type>) -> Vec<(String, String, usize)> {
    let mut found: Vec<(String, String, usize)> = Vec::new();
    let mut names: Vec<&String> = fields.keys().collect();
    names.sort();
    for field in names {
        let Type::Struct(..) = strip(&fields[field]) else { continue };
        let mut axis = 0;
        let mut visit = |ty: &Type| {
            if let Type::Generic(name) = ty
                && dims::is_existential(name)
                && !found.iter().any(|(known, ..)| known == name)
            {
                found.push((name.clone(), field.clone(), axis));
            }
            axis += 1;
        };
        if let Type::Struct(_, args) = strip(&fields[field]) {
            for arg in args {
                match arg {
                    Type::Pack(items) => items.iter().for_each(&mut visit),
                    Type::Generic(_) | Type::Dim(_) => visit(arg),
                    _ => {}
                }
            }
        }
    }
    found
}

/// Opens the anonymous extents of a value bound to an immutable name: each
/// `Dyn` position becomes the rigid name `x.dim<axis>`, and existentials the
/// value came with are named after `x`.
/// Whether a written field type mentions `?b`.
pub(crate) fn ty_has_existential(ty: &Ty) -> bool {
    match ty {
        Ty::Existential(..) => true,
        Ty::Generic { args, .. } | Ty::Tuple(args, _) => args.iter().any(ty_has_existential),
        Ty::Borrow { ty, .. } | Ty::Slice(ty, _) => ty_has_existential(ty),
        _ => false,
    }
}

fn has_open_positions(ty: &Type, unclaimed: &HashMap<String, String>) -> bool {
    let open = |arg: &Type| match arg {
        Type::Dim(Dim::Dyn) => true,
        Type::Generic(name) => unclaimed.contains_key(name),
        _ => false,
    };
    match ty {
        Type::Borrow { ty, .. } => has_open_positions(ty, unclaimed),
        Type::Tuple(items) => items.iter().any(|item| has_open_positions(item, unclaimed)),
        Type::Struct(_, args) => args.iter().any(|arg| match arg {
            Type::Pack(items) => items.iter().any(open),
            other => open(other),
        }),
        _ => false,
    }
}

pub(crate) fn open_binding(ty: Type, name: &str, id: LocalId, span: Span, program: &Program, context: &mut FunctionContext<'_>) -> Type {
    let existential_fields = matches!(strip(&ty), Type::Struct(struct_name, _) if program.existential_structs.contains(struct_name));
    if !existential_fields && !has_open_positions(&ty, &context.dims.unclaimed) {
        return ty;
    }
    let opened = match &ty {
        Type::Tuple(items) => Type::Tuple(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| open_slots(item, name, &format!("{name}.{index}"), Some(index.to_string()), id, span, program, context))
                .collect(),
        ),
        _ => open_slots(&ty, name, name, None, id, span, program, context),
    };
    if existential_fields
        && let Some(fields) = instantiated_struct_fields(strip(&opened), program, &mut Reporter::new())
    {
        let existentials = has_existential_fields(&fields);
        if !existentials.is_empty() {
            let mut map = HashMap::new();
            for (existential, field, axis) in existentials {
                let short = existential.trim_start_matches('?');
                let source = AtomSource::Extent { local: id, ty: fields[&field].clone(), field: Some(field), axis };
                let atom = new_atom(&format!("{name}.{short}"), span, source, Some(name.to_string()), program, context);
                map.insert(existential, Type::Generic(atom));
            }
            context.dims.fields.insert(id, map);
        }
    }
    opened
}

#[allow(clippy::too_many_arguments)]
fn open_slots(
    ty: &Type,
    binding: &str,
    display: &str,
    field: Option<String>,
    id: LocalId,
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
) -> Type {
    map_slots(ty, program, &mut |axis, slot| {
        let source = AtomSource::Extent { local: id, field: field.clone(), axis, ty: ty.clone() };
        match slot {
            Type::Dim(Dim::Dyn) => {
                let atom = new_atom(&format!("{display}.dim{axis}"), span, source, Some(binding.to_string()), program, context);
                set_anonymous(&atom, true);
                Some(Type::Generic(atom))
            }
            Type::Generic(atom) if context.dims.unclaimed.contains_key(atom) => {
                let short = context.dims.unclaimed.remove(atom).unwrap_or_default();
                claim(atom, format!("{display}.{short}"), span, source, binding, program, context);
                None
            }
            _ => None,
        }
    })
}

/// The type of `base.field`, with the existentials of `base`'s struct opened
/// once per binding.
pub(crate) fn field_type(base: &Expr, ty: Type, field: &str, span: Span, program: &Program, context: &mut FunctionContext<'_>) -> Type {
    let mut names = Vec::new();
    fn existentials(ty: &Type, out: &mut Vec<String>) {
        match ty {
            Type::Generic(name) if dims::is_existential(name) => out.push(name.clone()),
            Type::Struct(_, items) | Type::Pack(items) | Type::Tuple(items) => items.iter().for_each(|item| existentials(item, out)),
            Type::Borrow { ty, .. } => existentials(ty, out),
            _ => {}
        }
    }
    existentials(&ty, &mut names);
    if names.is_empty() {
        return ty;
    }
    if let Some(id) = program.locals.expr(base)
        && let Some(map) = context.dims.fields.get(&id)
    {
        return rename(&ty, map);
    }
    open_existentials(ty, field, span, program, context)
}

/// Destructuring a struct with existential fields opens each `?b` once,
/// shared by every field that mentions it.
pub(crate) fn open_pattern_fields(
    fields: HashMap<String, Type>,
    patterns: &[(String, Pat)],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
) -> HashMap<String, Type> {
    let existentials = has_existential_fields(&fields);
    if existentials.is_empty() {
        return fields;
    }
    let mut map = HashMap::new();
    for (existential, field, axis) in existentials {
        let short = existential.trim_start_matches('?');
        let binding = patterns.iter().find(|(name, _)| name == &field).map(|(_, pattern)| pattern);
        let (display, source, name) = match binding {
            Some(pattern @ Pat::Ident(name, _)) => match program.locals.pat(pattern) {
                Some(id) => (
                    format!("{name}.{short}"),
                    AtomSource::Extent { local: id, field: None, axis, ty: fields[&field].clone() },
                    Some(name.clone()),
                ),
                None => (format!("{field}.{short}"), AtomSource::Unknown, None),
            },
            _ => (format!("{field}.{short}"), AtomSource::Unknown, None),
        };
        let atom = new_atom(&display, span, source, name, program, context);
        map.insert(existential, Type::Generic(atom));
    }
    fields.into_iter().map(|(name, ty)| (name, rename(&ty, &map))).collect()
}

/// `n` names `dimension` in types for the rest of the enclosing block.
fn bind_name(name: &str, dimension: Type, context: &mut FunctionContext<'_>) {
    let previous = context.generics.insert(name.to_string(), dimension);
    context.dims.names.push((context.scopes.len(), name.to_string(), previous));
}

/// Leaving a block: names bound in it go out of scope, and a result that
/// mentions them is given fresh names (an existential), so no name outlives
/// its scope.
pub(crate) fn end_scope(result: Type, program: &Program, context: &mut FunctionContext<'_>) -> Type {
    let depth = context.scopes.len();
    while let Some((at, _, _)) = context.dims.names.last() {
        if *at <= depth {
            break;
        }
        let (_, name, previous) = context.dims.names.pop().expect("checked");
        match previous {
            Some(previous) => context.generics.insert(name, previous),
            None => context.generics.remove(&name),
        };
    }
    let mut atoms = Vec::new();
    atoms_in(&result, &mut atoms);
    let escaping: Vec<String> =
        atoms.into_iter().filter(|atom| context.dims.depth.get(atom).is_some_and(|at| *at > depth)).collect();
    if escaping.is_empty() {
        return result;
    }
    let mut map = HashMap::new();
    for atom in escaping {
        let display = dims::display_name(&atom);
        let origin = origin_of(&atom).unwrap_or_else(|| Span::new_root(0, 0));
        let fresh = new_atom(&display, origin, AtomSource::Unknown, None, program, context);
        set_anonymous(&fresh, is_anonymous(&atom));
        context.dims.unclaimed.insert(fresh.clone(), display.rsplit('.').next().unwrap_or(&display).to_string());
        map.insert(atom, Type::Generic(fresh));
    }
    rename(&result, &map)
}

/// The type the context expects of `expr`, for intrinsics and parameters
/// that only the result type determines.
pub(crate) fn set_hint(expr: &Expr, ty: Type, context: &mut FunctionContext<'_>) {
    let consumes = matches!(
        expr,
        Expr::MethodCall { .. } | Expr::Call { .. } | Expr::AssociatedCall { .. } | Expr::Try { .. } | Expr::Block(_) | Expr::Unsafe(..)
    );
    if consumes && !matches!(ty, Type::Unknown | Type::Error) {
        context.dims.hints.insert(expr as *const Expr, ty);
    }
}

pub(crate) fn take_hint(expr: &Expr, context: &mut FunctionContext<'_>) -> Option<Type> {
    context.dims.hints.remove(&(expr as *const Expr))
}

/// Passes the target type on to the expression that produces the value.
pub(crate) fn forward_hint(from: &Expr, to: &Expr, context: &mut FunctionContext<'_>) {
    if let Some(ty) = take_hint(from, context) {
        set_hint(to, ty, context);
    }
}

fn shaped_name(ty: &Type, program: &Program) -> Option<String> {
    let Type::Struct(name, _) = strip(ty) else { return None };
    let signature = program.methods.get(&(name.clone(), "extent".to_string()))?;
    (signature.params == [Type::Int(IntWidth::I64)] && signature.return_ty == Type::Int(IntWidth::I64)).then(|| name.clone())
}

pub(crate) fn dim_error_type(program: &Program) -> Option<Type> {
    let mut keys: Vec<&String> =
        program.structs.keys().filter(|key| key.as_str() == "DimError" || key.ends_with("dims::DimError")).collect();
    keys.sort_by_key(|key| (!key.contains("::"), key.len()));
    keys.first().map(|key| Type::Struct((*key).clone(), Vec::new()))
}

fn not_shaped(ty: &Type, method: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        "PACO-E0345",
        span,
        format!("`{}` does not satisfy `stdlib::dims::Shaped`, so it has no `{method}()`", strip(ty).name()),
    )
    .with_note("`Shaped` needs `fn extent(&self, axis: i64) -> i64`")
}

/// `dim`, `with_dims`, `as_dims`, `assume_dims` and `erase_dims`: available
/// on every `Shaped` type that does not declare a method of the same name.
#[allow(clippy::too_many_arguments)]
pub(crate) fn infer_intrinsic(
    call: &Expr,
    receiver_ty: &Type,
    method: &str,
    args: &[Expr],
    span: Span,
    program: &Program,
    context: &mut FunctionContext<'_>,
    reporter: &mut Reporter,
) -> Option<Type> {
    if !matches!(method, "dim" | "with_dims" | "as_dims" | "assume_dims" | "erase_dims") {
        return None;
    }
    for arg in args {
        infer_expr(arg, program, context, reporter);
    }
    let Some(slots) = slots(receiver_ty, program).filter(|_| shaped_name(receiver_ty, program).is_some()) else {
        reporter.push(not_shaped(receiver_ty, method, span));
        return Some(Type::Error);
    };
    match method {
        "dim" => {
            let axis = match args {
                [Expr::Literal(Literal::Int(axis), _)] => *axis,
                _ => {
                    reporter.push(Diagnostic::error(
                        "PACO-E0345",
                        span,
                        "the axis of `dim` must be an integer literal; use `extent(i)` for a computed axis",
                    ));
                    return Some(Type::Error);
                }
            };
            if axis < 0 || axis as usize >= slots.len() {
                reporter.push(Diagnostic::error(
                    "PACO-E0345",
                    span,
                    format!("axis {axis} is out of range for `{}`, which has {} dimensions", strip(receiver_ty).name(), slots.len()),
                ));
                return Some(Type::Error);
            }
            Some(Type::Int(IntWidth::I64))
        }
        "erase_dims" => Some(map_slots(strip(receiver_ty), program, &mut |_, slot| {
            (!is_static(slot)).then_some(Type::Dim(Dim::Dyn))
        })),
        _ => {
            if method == "assume_dims" {
                require_unsafe(reporter, span, context.in_unsafe, "`assume_dims`");
            }
            let hint = take_hint(call, context);
            let target = hint.as_ref().map(|hint| match (method, hint) {
                (_, Type::Enum(name, args)) if name == "Result" && args.len() == 2 => strip(&args[0]).clone(),
                (_, other) => strip(other).clone(),
            });
            let Some(target) = target else {
                reporter.push(Diagnostic::error(
                    "PACO-E0348",
                    span,
                    format!("`{method}` needs a target type: annotate the binding, for example `let x: Grid<f32, n> = x.{method}()?;`"),
                ));
                return Some(Type::Error);
            };
            if !check_refinement(strip(receiver_ty), &target, method, span, program, reporter) {
                return Some(Type::Error);
            }
            if method == "assume_dims" {
                return Some(target);
            }
            let Some(error) = dim_error_type(program) else {
                reporter.push(Diagnostic::error(
                    "PACO-E1001",
                    span,
                    format!("`{method}` returns `dims::DimError`; add `use stdlib::dims;`"),
                ));
                return Some(Type::Error);
            };
            Some(match method {
                "with_dims" => Type::Enum("Result".to_string(), vec![target, error]),
                "as_dims" => Type::Enum(
                    "Result".to_string(),
                    vec![Type::Borrow { mutable: false, ty: Box::new(target) }, error],
                ),
                _ => target,
            })
        }
    }
}

fn check_refinement(source: &Type, target: &Type, method: &str, span: Span, program: &Program, reporter: &mut Reporter) -> bool {
    let (Type::Struct(source_name, source_args), Type::Struct(target_name, target_args)) = (source, target) else {
        reporter.push(Diagnostic::error(
            "PACO-E0348",
            span,
            format!("`{method}` cannot turn `{}` into `{}`", source.name(), target.name()),
        ));
        return false;
    };
    let same_shape = |args: &[Type]| -> Vec<Type> {
        args.iter().map(|arg| if matches!(arg, Type::Pack(_) | Type::Dim(_)) || is_symbolic(arg) { Type::Unit } else { arg.clone() }).collect()
    };
    let (source_slots, target_slots) = (slots(source, program).unwrap_or_default(), slots(target, program).unwrap_or_default());
    if source_name != target_name || same_shape(source_args) != same_shape(target_args) || source_slots.len() != target_slots.len() {
        reporter.push(Diagnostic::error(
            "PACO-E0348",
            span,
            format!(
                "`{method}` only renames dimensions; it cannot turn `{}` into `{}`",
                source.name(),
                target.name()
            ),
        ));
        return false;
    }
    for (axis, (from, to)) in source_slots.iter().zip(&target_slots).enumerate() {
        match (is_static(from), is_static(to)) {
            (true, true) if from != to => {
                reporter.push(Diagnostic::error(
                    "PACO-E0336",
                    span,
                    format!("shape mismatch: dimension {axis} expected `{}`, found `{}`", to.name(), from.name()),
                ));
                return false;
            }
            (true, false) | (false, true) => {
                reporter.push(Diagnostic::error(
                    "PACO-E0348",
                    span,
                    format!(
                        "`{method}` cannot change dimension {axis} from `{}` to `{}`: static and run-time extents are stored differently",
                        from.name(),
                        to.name()
                    ),
                ).with_note("convert with a copying constructor of the type instead"));
                return false;
            }
            _ => {}
        }
    }
    true
}

/// `let n = x.dim(0);` and `let t = dims::witness(len)?;` name an extent.
pub(crate) fn bind_witness(statement: &LetStmt, program: &Program, context: &mut FunctionContext<'_>) {
    let (Pat::Ident(name, _), Some(value)) = (&statement.pattern, &statement.value) else { return };
    let dimension = match value {
        Expr::MethodCall { receiver, method, args, .. } if method == "dim" => {
            let receiver_ty = program.types.borrow().get(&(receiver.as_ref() as *const Expr)).cloned();
            let Some(receiver_ty) = receiver_ty else { return };
            let Some(type_name) = shaped_name(&receiver_ty, program) else { return };
            if program.methods.contains_key(&(type_name, "dim".to_string())) {
                return;
            }
            let (Some(slots), [Expr::Literal(Literal::Int(axis), _)]) = (slots(&receiver_ty, program), args.as_slice()) else {
                return;
            };
            let Some(slot) = slots.get(*axis as usize) else { return };
            Some(slot.clone())
        }
        Expr::Try { expr: inner, .. } => match inner.as_ref() {
            Expr::AssociatedCall { ty: Ty::Path(path, _), function, .. }
                if function == "witness" && program.functions.contains_key(&format!("{}::witness", path.join("::"))) =>
            {
                None
            }
            _ => return,
        },
        _ => return,
    };
    if statement.mutable {
        bind_name(name, Type::Unknown, context);
        return;
    }
    let id = program.locals.pat(&statement.pattern);
    let dimension = match dimension {
        Some(Type::Generic(atom)) if dims::is_atom(&atom) => {
            if is_anonymous(&atom) {
                set_anonymous(&atom, false);
                dims::rename_atom(&atom, name);
                if let Some(info) = program.atoms.borrow_mut().get_mut(&atom) {
                    info.origin = statement.span;
                }
                ORIGINS.with(|origins| origins.borrow_mut().insert(atom.clone(), statement.span));
            }
            Type::Generic(atom)
        }
        Some(slot @ (Type::Generic(_) | Type::Dim(Dim::Const(_)))) => slot,
        _ => {
            let Some(id) = id else { return };
            Type::Generic(new_atom(name, statement.span, AtomSource::Value(id), Some(name.clone()), program, context))
        }
    };
    bind_name(name, dimension, context);
}

/// A named dimension passed where a parameter is declared `Dyn` forgets its
/// name; that is the one place a name converts to `Dyn` implicitly.
fn mentions_dyn(ty: &Type) -> bool {
    match ty {
        Type::Dim(Dim::Dyn) => true,
        Type::Struct(_, items) | Type::Enum(_, items) | Type::Tuple(items) | Type::Pack(items) => items.iter().any(mentions_dyn),
        Type::Borrow { ty, .. } => mentions_dyn(ty),
        _ => false,
    }
}

pub(crate) fn subsume_dyn(expected: &Type, actual: Type, program: &Program) -> Type {
    if !mentions_dyn(expected) {
        return actual;
    }
    subsume(expected, &actual, program)
}

fn subsume(expected: &Type, actual: &Type, program: &Program) -> Type {
    match (expected, actual) {
        (Type::Borrow { ty: expected, .. }, Type::Borrow { mutable, ty }) => {
            Type::Borrow { mutable: *mutable, ty: Box::new(subsume(expected, ty, program)) }
        }
        (Type::Tuple(expected_items), Type::Tuple(items)) if expected_items.len() == items.len() => {
            Type::Tuple(expected_items.iter().zip(items).map(|(expected, item)| subsume(expected, item, program)).collect())
        }
        (Type::Enum(expected_name, expected_args), Type::Enum(name, args)) if expected_name == name && expected_args.len() == args.len() => {
            Type::Enum(name.clone(), expected_args.iter().zip(args).map(|(expected, arg)| subsume(expected, arg, program)).collect())
        }
        (Type::Struct(expected_name, expected_args), Type::Struct(name, args)) if expected_name == name && expected_args.len() == args.len() => {
            let Some(expected_slots) = slots(expected, program) else { return actual.clone() };
            let erased = map_slots(actual, program, &mut |axis, slot| {
                (matches!(expected_slots.get(axis), Some(Type::Dim(Dim::Dyn))) && is_symbolic(slot)).then_some(Type::Dim(Dim::Dyn))
            });
            let kinds = program.structs.get(name).map(|info| info.kinds.clone()).unwrap_or_default();
            match erased {
                Type::Struct(name, args) => Type::Struct(
                    name,
                    args.iter()
                        .zip(expected_args)
                        .enumerate()
                        .map(|(index, (arg, expected))| match kinds.get(index) {
                            Some(ParamKind::Type) => subsume(expected, arg, program),
                            _ => arg.clone(),
                        })
                        .collect(),
                ),
                other => other,
            }
        }
        _ => actual.clone(),
    }
}

/// The first position where a named dimension would silently become `Dyn`.
pub(crate) fn dyn_escape(expected: &Type, actual: &Type, program: &Program) -> Option<(usize, Type)> {
    match (expected, actual) {
        (Type::Borrow { ty: expected, .. }, Type::Borrow { ty: actual, .. }) => dyn_escape(expected, actual, program),
        (Type::Enum(a, expected_args), Type::Enum(b, actual_args)) if a == b && expected_args.len() == actual_args.len() => {
            expected_args.iter().zip(actual_args).find_map(|(expected, actual)| dyn_escape(expected, actual, program))
        }
        (Type::Tuple(expected_args), Type::Tuple(actual_args)) if expected_args.len() == actual_args.len() => {
            expected_args.iter().zip(actual_args).find_map(|(expected, actual)| dyn_escape(expected, actual, program))
        }
        (Type::Struct(a, _), Type::Struct(b, _)) if a == b => {
            let (expected_slots, actual_slots) = (slots(expected, program)?, slots(actual, program)?);
            expected_slots
                .iter()
                .zip(&actual_slots)
                .enumerate()
                .find(|(_, (expected, actual))| matches!(expected, Type::Dim(Dim::Dyn)) && is_named(actual))
                .map(|(axis, (_, actual))| (axis, actual.clone()))
        }
        _ => None,
    }
}

/// `PACO-E0343` for a named dimension converted to `Dyn`.
pub(crate) fn escape_diagnostic(span: Span, axis: usize, name: &Type, what: &str) -> Diagnostic {
    let display = name.name();
    let mut diagnostic = Diagnostic::error(
        "PACO-E0343",
        span,
        format!("the dimension `{display}` would become `Dyn` implicitly ({what}, dimension {axis})"),
    );
    if let Type::Generic(atom) = name
        && let Some(origin) = origin_of(atom)
    {
        diagnostic = diagnostic.with_secondary(origin, format!("`{display}` is bound here"));
    }
    diagnostic
}

/// Checks a value returned from the function against its declared type.
pub(crate) fn check_return(
    span: Span,
    value: Option<&Expr>,
    expected: &Type,
    actual: &Type,
    program: &Program,
    context: &FunctionContext<'_>,
    reporter: &mut Reporter,
) {
    if *actual == Type::Never || decays_to(actual, expected) || fits_existential(expected, actual) {
        return;
    }
    if let Some((axis, name)) = dyn_escape(expected, actual, program) {
        let diagnostic = escape_diagnostic(span, axis, &name, "returned where the signature says `Dyn`");
        reporter.push(escape_fixes(diagnostic, value, axis, &name, context.dims.return_ty.as_ref(), program));
        return;
    }
    reporter.push(mismatch_diagnostic(span, "return ", expected, actual));
}

/// Whether `actual` fits a declared type whose existentials (`?n`) take one
/// consistent name each; a static extent is not an existential.
pub(crate) fn fits_existential(expected: &Type, actual: &Type) -> bool {
    let mut names = Vec::new();
    fn collect(ty: &Type, out: &mut Vec<String>) {
        match ty {
            Type::Generic(name) if dims::is_existential(name) => out.push(name.clone()),
            Type::Struct(_, items) | Type::Enum(_, items) | Type::Tuple(items) | Type::Pack(items) => {
                items.iter().for_each(|item| collect(item, out))
            }
            Type::Borrow { ty, .. } | Type::Slice(ty) => collect(ty, out),
            _ => {}
        }
    }
    collect(expected, &mut names);
    if names.is_empty() {
        return false;
    }
    let mut substitutions: HashMap<String, Type> = names.iter().map(|name| (name.clone(), Type::Generic(name.clone()))).collect();
    take_unproved();
    if !unify_type(expected, actual, &mut substitutions) {
        take_unproved();
        return false;
    }
    names.iter().all(|name| !is_static(&substitutions[name])) && compatible(actual, &substitute_generics(expected, &substitutions))
}

/// The fix for a `Dyn` return type that would drop a name: `?n` in the
/// signature (when the position can be found), then `.erase_dims()`.
pub(crate) fn escape_fixes(mut diagnostic: Diagnostic, value: Option<&Expr>, axis: usize, name: &Type, signature: Option<&Ty>, program: &Program) -> Diagnostic {
    let display = match name {
        Type::Generic(atom) => dims::display_name(atom).rsplit('.').next().unwrap_or("n").to_string(),
        _ => "n".to_string(),
    };
    let display = if display.starts_with("dim") { "n".to_string() } else { display };
    if let Some(return_ty) = signature
        && let Some(span) = dyn_arg_span(return_ty, axis, program)
    {
        diagnostic = diagnostic.with_fix(
            format!("name the extent in the signature: `?{display}`"),
            vec![Edit::new(span, format!("?{display}"))],
        );
    }
    if let Some(value) = value {
        let end = expr_span(value).end();
        let span = expr_span(value);
        diagnostic = diagnostic.with_fix(
            "forget the name explicitly with `.erase_dims()`",
            vec![Edit::new(Span::new(span.file_id(), end, end), ".erase_dims()")],
        );
    }
    diagnostic
}

fn dyn_arg_span(ty: &Ty, axis: usize, program: &Program) -> Option<Span> {
    match ty {
        Ty::Borrow { ty, .. } => dyn_arg_span(ty, axis, program),
        Ty::Generic { path, args, .. } => {
            let info = program.structs.get(&program.resolve_type_key(path))?;
            let has_pack = info.kinds.last() == Some(&ParamKind::Pack);
            let dims: Vec<&Ty> = args
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    let kind = if has_pack && *index + 1 >= info.kinds.len() { ParamKind::Pack } else { info.kinds[*index] };
                    kind != ParamKind::Type
                })
                .map(|(_, arg)| arg)
                .collect();
            match dims.get(axis) {
                Some(Ty::DynDim(span)) => Some(*span),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `PACO-E0347`: a `const` parameter is monomorphised, so it cannot take an
/// extent known only at run time.
pub(crate) fn check_const_params(key: &str, substitutions: &HashMap<String, Type>, span: Span, program: &Program, reporter: &mut Reporter) {
    let Some(params) = program.own_generics.get(key) else { return };
    for param in params {
        if !matches!(param.kind, ast::GenericParamKind::Const(_)) {
            continue;
        }
        let Some(bound) = substitutions.get(&param.name) else { continue };
        if !is_named(bound) {
            continue;
        }
        reporter.push(
            Diagnostic::error(
                "PACO-E0347",
                span,
                format!(
                    "`const {}` cannot take the run-time dimension `{}`; a `const` parameter is static",
                    param.name,
                    bound.name()
                ),
            )
            .with_secondary(param.span, format!("`{}` is declared `const` here", param.name))
            .with_fix(format!("declare it `dim {}`", param.name), vec![Edit::new(param.span, format!("dim {}", param.name))]),
        );
    }
}

/// `#[broadcasts(D, T)]`: each source axis, aligned from the right, is proved
/// equal to its target or is the literal `1`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_broadcast(
    key: &str,
    receiver: &Expr,
    substitutions: &HashMap<String, Type>,
    span: Span,
    method: &str,
    program: &Program,
    reporter: &mut Reporter,
) {
    let Some((source, target)) = program.broadcasts.get(key) else { return };
    let items = |name: &str| match substitutions.get(name) {
        Some(Type::Pack(items)) => Some(items.clone()),
        Some(other @ (Type::Dim(_) | Type::Generic(_))) if other != &Type::Generic(name.to_string()) => Some(vec![other.clone()]),
        _ => None,
    };
    let (Some(source_dims), Some(target_dims)) = (items(source), items(target)) else {
        reporter.push(Diagnostic::error(
            "PACO-E0346",
            span,
            format!("cannot tell the target shape of `{method}`; annotate the binding"),
        ));
        return;
    };
    let one = Type::Dim(Dim::Const(ConstExpr::lit(1)));
    let problem = if source_dims.len() > target_dims.len() {
        Some(format!("{} source dimensions do not fit {} target dimensions", source_dims.len(), target_dims.len()))
    } else {
        source_dims.iter().rev().zip(target_dims.iter().rev()).enumerate().find_map(|(from_right, (from, to))| {
            let axis = source_dims.len() - 1 - from_right;
            if *from == one || from == to {
                return None;
            }
            let equal = match (from, to) {
                (Type::Dim(Dim::Const(a)), Type::Dim(Dim::Const(b))) => dims::verdict(b, a) == Verdict::Equal,
                _ => false,
            };
            (!equal).then(|| {
                let why = if matches!(from, Type::Dim(Dim::Dyn)) || is_symbolic(from) {
                    "a run-time extent is never treated as `1`"
                } else {
                    "it is neither equal nor the literal `1`"
                };
                format!("dimension {axis} `{}` does not broadcast to `{}`: {why}", from.name(), to.name())
            })
        })
    };
    if let Some(problem) = problem {
        let receiver_end = expr_span(receiver).end();
        let edit = Edit::new(Span::new(span.file_id(), receiver_end, span.end()), format!(".checked_{method}()"));
        reporter.push(
            Diagnostic::error("PACO-E0346", span, format!("cannot prove this broadcast: {problem}"))
                .with_fix(format!("compare at run time with `.checked_{method}()`, which returns `Result`"), vec![edit]),
        );
    }
}

/// Unbound own dimension parameters of a call are solved from the type the
/// context expects of its result.
pub(crate) fn solve_from_hint(call: &Expr, key: &str, return_ty: &Type, substitutions: &mut HashMap<String, Type>, program: &Program, context: &mut FunctionContext<'_>) {
    let hint = take_hint(call, context);
    let Some(params) = program.own_generics.get(key) else { return };
    let unbound = params.iter().any(|param| {
        param.is_const() || param.is_dim()
    } && matches!(substitutions.get(&param.name), Some(Type::Generic(name)) if name == &param.name));
    if !unbound {
        return;
    }
    if let Some(hint) = hint {
        let hint = match &hint {
            Type::Enum(name, args) if name == "Result" && !matches!(return_ty, Type::Enum(name, _) if name == "Result") => args[0].clone(),
            _ => hint,
        };
        unify_type(return_ty, &hint, substitutions);
    }
}

/// A fresh witness name for a fix, unused in the current scope.
fn fresh_witness(context: &FunctionContext<'_>, taken: &[String]) -> String {
    ["n", "m", "k", "b", "t"]
        .iter()
        .map(|name| name.to_string())
        .chain((1..).map(|index| format!("n{index}")))
        .find(|name| !context.generics.contains_key(name) && !taken.contains(name))
        .expect("an unbounded supply of names")
}

/// A `with_dims` fix needs a `Shaped` value and `stdlib::dims` in scope.
fn refinable(ty: &Type, program: &Program) -> bool {
    shaped_name(ty, program).is_some() && dim_error_type(program).is_some()
}

fn try_suffix(program: &Program, context: &FunctionContext<'_>) -> &'static str {
    match (result_type_args(context.expected_return), dim_error_type(program)) {
        (Some((_, error)), Some(dim_error)) if error == dim_error => "?",
        _ => ".unwrap()",
    }
}

fn plain_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Borrow { expr, mutable: false, .. } => plain_name(expr),
        _ => None,
    }
}

/// The source text of a type whose dimension positions are replaced by
/// `names` (a witness or a literal per axis).
fn type_text(ty: &Type, names: &[String], program: &Program) -> String {
    map_slots(strip(ty), program, &mut |axis, _| names.get(axis).map(|name| Type::Generic(name.clone()))).name()
}

/// Fixes for `a + b` whose dimensions are not proved equal: refining both
/// operands once, then the type's `checked_*` form.
#[allow(clippy::too_many_arguments)]
pub(crate) fn operator_fixes(
    mut diagnostic: Diagnostic,
    left: &Expr,
    right: &Expr,
    left_ty: &Type,
    method: &str,
    span: Span,
    program: &Program,
    context: &FunctionContext<'_>,
) -> Diagnostic {
    let (Some(left_name), Some(right_name)) = (plain_name(left), plain_name(right)) else { return diagnostic };
    let (Some(left_slots), Some(statement)) = (slots(left_ty, program), context.dims.statement) else { return diagnostic };
    if !refinable(left_ty, program) {
        return checked_fix(diagnostic, left_name, right_name, left_ty, method, span, program);
    }
    let mut prelude = String::new();
    let mut names = Vec::new();
    for (axis, slot) in left_slots.iter().enumerate() {
        if is_static(slot) {
            names.push(slot.name());
        } else {
            let witness = fresh_witness(context, &names);
            prelude.push_str(&format!("let {witness} = {left_name}.dim({axis}); "));
            names.push(witness);
        }
    }
    let suffix = try_suffix(program, context);
    let target = type_text(left_ty, &names, program);
    let text = format!(
        "{prelude}let {left_name}: {target} = {left_name}.with_dims(){suffix}; let {right_name}: {target} = {right_name}.with_dims(){suffix}; "
    );
    let at = Span::new(statement.file_id(), statement.start(), statement.start());
    diagnostic = diagnostic.with_fix(format!("refine both operands once with `with_dims`: `{}`", text.trim_end()), vec![Edit::new(at, text)]);
    checked_fix(diagnostic, left_name, right_name, left_ty, method, span, program)
}

fn checked_fix(diagnostic: Diagnostic, left: &str, right: &str, left_ty: &Type, method: &str, span: Span, program: &Program) -> Diagnostic {
    match target_type_name(strip(left_ty)) {
        Some(type_name) if program.methods.contains_key(&(type_name.clone(), format!("checked_{method}"))) => {
            let replacement = format!("{left}.checked_{method}(&{right})");
            diagnostic.with_fix(format!("use `{replacement}`, which returns `Result`"), vec![Edit::new(span, replacement)])
        }
        _ => diagnostic,
    }
}

/// The `with_dims` fix for an argument whose dimension cannot be proved
/// equal to one fixed by an earlier argument.
pub(crate) fn argument_fix(mut diagnostic: Diagnostic, arg: &Expr, expected: &Type, actual: &Type, program: &Program, context: &FunctionContext<'_>) -> Diagnostic {
    let (Some(name), Some(statement)) = (plain_name(arg), context.dims.statement) else { return diagnostic };
    if !refinable(actual, program) {
        return diagnostic;
    }
    let (Some(expected_slots), Some(actual_slots)) = (slots(expected, program), slots(actual, program)) else { return diagnostic };
    if expected_slots.len() != actual_slots.len() {
        return diagnostic;
    }
    let mut prelude = String::new();
    let mut names = Vec::new();
    for (axis, (want, have)) in expected_slots.iter().zip(&actual_slots).enumerate() {
        let text = if is_static(have) {
            have.name()
        } else if want == have {
            let witness = fresh_witness(context, &names);
            prelude.push_str(&format!("let {witness} = {name}.dim({axis}); "));
            witness
        } else {
            match source_text(want, program, context, &names) {
                Some((text, extra)) => {
                    prelude.push_str(&extra);
                    text
                }
                None => return diagnostic,
            }
        };
        names.push(text);
    }
    let suffix = try_suffix(program, context);
    let target = type_text(actual, &names, program);
    let text = format!("{prelude}let {name}: {target} = {name}.with_dims(){suffix}; ");
    let at = Span::new(statement.file_id(), statement.start(), statement.start());
    diagnostic = diagnostic.with_fix(format!("refine `{name}` once with `with_dims`: `{}`", text.trim_end()), vec![Edit::new(at, text)]);
    diagnostic
}

/// How to write the dimension `ty` in source at the current point, with any
/// `let` it needs first.
fn source_text(ty: &Type, program: &Program, context: &FunctionContext<'_>, taken: &[String]) -> Option<(String, String)> {
    if is_static(ty) {
        return Some((ty.name(), String::new()));
    }
    let Type::Generic(name) = ty else { return None };
    if !dims::is_atom(name) {
        return Some((name.clone(), String::new()));
    }
    if let Some((written, _)) = context.generics.iter().find(|(_, bound)| *bound == ty) {
        return Some((written.clone(), String::new()));
    }
    let info = program.atoms.borrow().get(name).cloned()?;
    match info.source {
        AtomSource::Extent { field: None, axis, .. } => {
            let binding = info.binding?;
            let witness = fresh_witness(context, taken);
            Some((witness.clone(), format!("let {witness} = {binding}.dim({axis}); ")))
        }
        _ => None,
    }
}

/// Notes naming where each dimension involved in a mismatch was bound.
pub(crate) fn origin_notes(mut diagnostic: Diagnostic, dims: &[&Type]) -> Diagnostic {
    let mut seen = Vec::new();
    for ty in dims {
        let mut atoms = Vec::new();
        match ty {
            Type::Generic(name) => atoms.push(name.clone()),
            Type::Dim(Dim::Const(expr)) => atoms.extend(expr.names().iter().map(|name| name.to_string())),
            _ => atoms_in(ty, &mut atoms),
        }
        for atom in atoms {
            if !dims::is_atom(&atom) {
                if !seen.contains(&atom)
                    && let Some(span) = PARAMS.with(|params| params.borrow().get(&atom).copied())
                {
                    diagnostic = diagnostic.with_secondary(span, format!("`{atom}` is a dimension parameter of this item"));
                }
                seen.push(atom);
                continue;
            }
            if seen.contains(&atom) {
                continue;
            }
            if let Some(origin) = origin_of(&atom) {
                let display = dims::display_name(&atom);
                let what = match display.split_once(".dim") {
                    Some((binding, axis)) => format!("`{display}` is the extent of `{binding}` along axis {axis}, bound here"),
                    None => format!("`{display}` is bound here"),
                };
                diagnostic = diagnostic.with_secondary(origin, what);
            }
            seen.push(atom);
        }
    }
    diagnostic
}

/// Records a `let` whose type names dimensions, for `paco shapes`.
pub(crate) fn record_shape(pattern: &Pat, ty: &Type, program: &Program) {
    if !RECORD_SHAPES.with(|flag| flag.get()) {
        return;
    }
    let Pat::Ident(name, span) = pattern else { return };
    let Type::Struct(struct_name, _) = strip(ty) else { return };
    if program.structs.get(struct_name).is_some_and(|info| info.kinds.iter().any(|kind| *kind != ParamKind::Type)) {
        program.shapes.borrow_mut().push(ShapeRow { span: *span, name: name.clone(), ty: ty.clone() });
    }
}

/// The scope depth of the binding `expr` names, when it is a local.
pub(crate) fn binding_depth(expr: &Expr, program: &Program, context: &FunctionContext<'_>) -> Option<usize> {
    let id = program.locals.expr(expr)?;
    context.scopes.iter().position(|scope| scope.contains_key(&id)).map(|index| index + 1)
}

/// `PACO-E0343` for an assignment that would carry a name out of the scope
/// that bound it.
pub(crate) fn outlives(target: &Expr, value_ty: &Type, span: Span, program: &Program, context: &FunctionContext<'_>) -> Option<Diagnostic> {
    let depth = binding_depth(target, program, context)?;
    let mut atoms = Vec::new();
    atoms_in(value_ty, &mut atoms);
    let atom = atoms
        .into_iter()
        .find(|atom| !is_anonymous(atom) && context.dims.depth.get(atom).is_some_and(|at| *at > depth))?;
    let display = dims::display_name(&atom);
    let mut diagnostic = Diagnostic::error(
        "PACO-E0343",
        span,
        format!("the dimension `{display}` does not live as long as `{}`", place_name(target).unwrap_or("the target")),
    );
    if let Some(origin) = origin_of(&atom) {
        diagnostic = diagnostic.with_secondary(origin, format!("`{display}` is bound here, in an inner scope"));
    }
    Some(diagnostic.with_note("each iteration or block opens a fresh name; declare the variable inside the scope, or forget the name with `.erase_dims()`"))
}
