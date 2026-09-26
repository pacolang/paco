//! Resolves concrete struct/enum layouts from a `Module`'s declarations, for
//! backends that need to compute field/variant offsets and field types
//! (task 4.4). Generic structs/enums are monomorphized lazily, one `Layout`
//! per distinct `(name, type_args)` instantiation, cached in `RefCell`-backed
//! maps since codegen only ever borrows `TypeLayouts` immutably.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use paco_syntax::ast::{EnumDecl, GenericParam, GenericParamKind, Item, Module, StructDecl, Ty, VariantFields, generic_names};
use paco_types::{ConstExpr, Dim, IntWidth, Type, substitute_generics};

use crate::layout::{Layout, Repr, align_up, scalar_layout, struct_layout};

const DISCRIMINANT_SIZE: u64 = 8;

fn qualify(qualifier: &str, name: &str) -> String {
    if qualifier.is_empty() { name.to_string() } else { format!("{qualifier}::{name}") }
}

fn collect_decls<'a>(
    module: &'a Module,
    qualifier: &str,
    struct_decls: &mut HashMap<String, &'a StructDecl>,
    enum_decls: &mut HashMap<String, &'a EnumDecl>,
) {
    for item in &module.items {
        match item {
            Item::Struct(decl) => {
                struct_decls.insert(qualify(qualifier, &decl.name), decl);
            }
            Item::Enum(decl) => {
                enum_decls.insert(qualify(qualifier, &decl.name), decl);
            }
            _ => {}
        }
    }
}

/// Unlike `TypeRegistry`/`paco-types`' own import handling, this registers
/// *every* struct/enum an imported module declares, `pub` or not: a
/// private type can still be structurally referenced by a public one's
/// field (e.g. `stdlib::core`'s private `MapEntry<K, V>` inside `Map<K, V>`'s
/// public `entries: Vec<MapEntry<K, V>>`), and layout computation — unlike
/// name resolution — has no visibility boundary to enforce; it only
/// resolves types the type-checker already validated.
fn collect_imported_decls<'a>(
    module: &'a Module,
    qualifier: &str,
    struct_decls: &mut HashMap<String, &'a StructDecl>,
    enum_decls: &mut HashMap<String, &'a EnumDecl>,
) {
    for item in &module.items {
        match item {
            Item::Struct(decl) => {
                struct_decls.entry(qualify(qualifier, &decl.name)).or_insert(decl);
                struct_decls.entry(decl.name.clone()).or_insert(decl);
            }
            Item::Enum(decl) => {
                enum_decls.entry(qualify(qualifier, &decl.name)).or_insert(decl);
                enum_decls.entry(decl.name.clone()).or_insert(decl);
            }
            _ => {}
        }
    }
}

/// No ADR specifies a concrete number (ADR 0023's own "configurable cap"
/// precedent doesn't either) — this is a deliberately generous default that
/// only exists to turn runaway recursive-generic construction into a clear
/// error instead of unbounded memory growth. `with_max_instantiations`
/// overrides it per `TypeLayouts` instance.
const DEFAULT_MAX_INSTANTIATIONS_PER_ITEM: usize = 4096;

type InstantiationKey = (String, Vec<Type>);

#[derive(Clone)]
struct StructInfo {
    layout: Layout,
    fields: HashMap<String, (Type, u64)>,
    live_offset: Option<u64>,
}

#[derive(Clone)]
struct EnumInfo {
    layout: Layout,
    variants: HashMap<String, Vec<(Type, u64)>>,
    variant_index: HashMap<String, u64>,
    live_offset: Option<u64>,
}

/// A type with a user `drop` carries a hidden byte that construction sets,
/// so zero-filled storage that never held a value is never dropped.
const LIVE_FIELD: &str = "#live";

fn declares_drop(methods: &[paco_syntax::ast::FnDecl]) -> bool {
    methods.iter().any(|method| method.name == "drop")
}

pub struct TypeLayouts<'a> {
    struct_decls: HashMap<String, &'a StructDecl>,
    enum_decls: HashMap<String, &'a EnumDecl>,
    extra_drops: HashSet<String>,
    struct_cache: RefCell<HashMap<InstantiationKey, StructInfo>>,
    enum_cache: RefCell<HashMap<InstantiationKey, EnumInfo>>,
    struct_in_progress: RefCell<HashSet<InstantiationKey>>,
    enum_in_progress: RefCell<HashSet<InstantiationKey>>,
    max_instantiations: usize,
}

impl<'a> TypeLayouts<'a> {
    pub fn from_module(module: &'a Module) -> Self {
        Self::from_module_with_imports(module, &[])
    }

    pub fn from_module_with_imports(module: &'a Module, imports: &[(String, &'a Module)]) -> Self {
        let mut struct_decls = HashMap::new();
        let mut enum_decls = HashMap::new();
        collect_decls(module, "", &mut struct_decls, &mut enum_decls);
        for (qualifier, imported_module) in imports {
            collect_imported_decls(imported_module, qualifier, &mut struct_decls, &mut enum_decls);
        }
        let extra_drops = std::iter::once(module)
            .chain(imports.iter().map(|(_, imported)| *imported))
            .flat_map(|module| &module.items)
            .filter_map(|item| match item {
                Item::Methods(block) if declares_drop(&block.methods) => match &block.target {
                    Ty::Path(path, _) | Ty::Generic { path, .. } => path.last().cloned(),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        Self {
            struct_decls,
            enum_decls,
            extra_drops,
            struct_cache: RefCell::new(HashMap::new()),
            enum_cache: RefCell::new(HashMap::new()),
            struct_in_progress: RefCell::new(HashSet::new()),
            enum_in_progress: RefCell::new(HashSet::new()),
            max_instantiations: DEFAULT_MAX_INSTANTIATIONS_PER_ITEM,
        }
    }

    pub fn with_max_instantiations(mut self, max: usize) -> Self {
        self.max_instantiations = max;
        self
    }

    pub fn has_struct(&self, name: &str) -> bool {
        self.struct_decls.contains_key(name) || prelude_error_ty(name).is_some()
    }

    pub fn struct_layout(&self, name: &str, type_args: &[Type]) -> Layout {
        self.struct_info(name, type_args).layout
    }

    pub fn struct_field(&self, name: &str, type_args: &[Type], field: &str) -> (Type, u64) {
        self.struct_info(name, type_args)
            .fields
            .get(field)
            .unwrap_or_else(|| panic!("unknown field `{name}.{field}`"))
            .clone()
    }

    /// Elements in declaration order, each at its natural alignment.
    pub fn tuple_layout(&self, items: &[Type]) -> Layout {
        let fields: Vec<(String, Layout)> =
            items.iter().enumerate().map(|(index, item)| (index.to_string(), self.layout_of(item))).collect();
        struct_layout(&fields, Repr::C)
    }

    pub fn tuple_field(&self, items: &[Type], index: usize) -> (Type, u64) {
        let offset = self.tuple_layout(items).fields[index].offset;
        (items[index].clone(), offset)
    }

    pub fn enum_layout(&self, name: &str, type_args: &[Type]) -> Layout {
        self.enum_info(name, type_args).layout
    }

    pub fn enum_variant_field(&self, name: &str, type_args: &[Type], variant: &str, index: usize) -> (Type, u64) {
        self.enum_info(name, type_args)
            .variants
            .get(variant)
            .unwrap_or_else(|| panic!("unknown variant `{name}::{variant}`"))[index]
            .clone()
    }

    /// Offset of the hidden liveness byte of a type with a user `drop`.
    pub fn live_offset(&self, ty: &Type) -> Option<u64> {
        match ty {
            Type::Struct(name, args) if self.struct_decls.contains_key(name) => self.struct_info(name, args).live_offset,
            Type::Enum(name, args) => self.enum_info(name, args).live_offset,
            _ => None,
        }
    }

    fn has_drop(&self, name: &str, methods: &[paco_syntax::ast::FnDecl]) -> bool {
        declares_drop(methods) || self.extra_drops.contains(name.rsplit("::").next().unwrap_or(name))
    }

    pub fn enum_variant_index(&self, name: &str, type_args: &[Type], variant: &str) -> u64 {
        *self
            .enum_info(name, type_args)
            .variant_index
            .get(variant)
            .unwrap_or_else(|| panic!("unknown variant `{name}::{variant}`"))
    }

    /// Field types and offsets in declaration order.
    pub fn struct_fields(&self, name: &str, type_args: &[Type]) -> Vec<(Type, u64)> {
        let info = self.struct_info(name, type_args);
        let Some(decl) = self.struct_decls.get(name) else {
            return prelude_error_fields(name).into_iter().map(|(_, ty, offset)| (ty, offset)).collect();
        };
        decl.fields.iter().map(|field| info.fields[&field.name].clone()).collect()
    }

    /// Field names and types in declaration order.
    pub fn struct_field_list(&self, name: &str, type_args: &[Type]) -> Vec<(String, Type)> {
        let info = self.struct_info(name, type_args);
        match self.struct_decls.get(name) {
            Some(decl) => decl.fields.iter().map(|field| (field.name.clone(), info.fields[&field.name].0.clone())).collect(),
            None => prelude_error_fields(name).into_iter().map(|(field, ty, _)| (field.to_string(), ty)).collect(),
        }
    }

    /// The struct's `type <assoc> = ...;`, with its type arguments applied.
    pub fn struct_assoc(&self, name: &str, type_args: &[Type], assoc: &str) -> Option<Type> {
        let decl = self.struct_decls.get(name)?;
        let declared = decl.assoc_types.iter().find(|item| item.name == assoc)?.default.as_ref()?;
        let generics = generic_names(&decl.generics);
        let substitutions: HashMap<String, Type> = generics.iter().cloned().zip(type_args.iter().cloned()).collect();
        Some(substitute_generics(&self.resolve_ty_template(declared, &generics), &substitutions))
    }

    /// Whether two names (one qualified by its module, one not) declare the
    /// same struct.
    pub fn same_struct(&self, left: &str, right: &str) -> bool {
        left == right
            || matches!((self.struct_decls.get(left), self.struct_decls.get(right)), (Some(left), Some(right)) if std::ptr::eq(*left, *right))
    }

    pub fn struct_has_method(&self, name: &str, method: &str) -> bool {
        self.struct_decls.get(name).is_some_and(|decl| decl.methods.iter().any(|function| function.name == method))
    }

    pub fn size_of(&self, ty: &Type) -> u64 {
        self.layout_of(ty).size
    }

    /// Variant names in declaration order.
    pub fn enum_variant_names(&self, name: &str) -> Vec<String> {
        self.enum_decls[name].variants.iter().map(|variant| variant.name.clone()).collect()
    }

    /// Each variant's discriminant and payload fields, in declaration order.
    pub fn enum_variants(&self, name: &str, type_args: &[Type]) -> Vec<(u64, Vec<(Type, u64)>)> {
        let info = self.enum_info(name, type_args);
        let decl = self.enum_decls[name];
        decl.variants
            .iter()
            .map(|variant| (info.variant_index[&variant.name], info.variants[&variant.name].clone()))
            .collect()
    }

    fn struct_info(&self, name: &str, type_args: &[Type]) -> StructInfo {
        let key = (name.to_string(), type_args.to_vec());
        if let Some(info) = self.struct_cache.borrow().get(&key) {
            return info.clone();
        }
        if let Some((_, layout)) = prelude_error_struct(name) {
            let info = StructInfo {
                fields: prelude_error_fields(name).into_iter().map(|(field, ty, offset)| (field.to_string(), (ty, offset))).collect(),
                layout,
                live_offset: None,
            };
            self.struct_cache.borrow_mut().insert(key, info.clone());
            return info;
        }

        if !self.struct_in_progress.borrow_mut().insert(key.clone()) {
            panic!("recursive generic struct layout detected for `{name}` with type arguments {type_args:?}");
        }
        self.check_instantiation_cap(name, type_args, "struct", &self.struct_cache);

        let decl = *self
            .struct_decls
            .get(name)
            .unwrap_or_else(|| panic!("unknown struct `{name}` in layout table"));
        let substitutions: HashMap<String, Type> =
            generic_names(&decl.generics).into_iter().zip(type_args.iter().cloned()).collect();

        let field_data: Vec<(String, Type, Layout)> = decl
            .fields
            .iter()
            .map(|field| {
                let template = self.resolve_ty_template(&field.ty, &generic_names(&decl.generics));
                let concrete = substitute_generics(&template, &substitutions);
                let layout = self.layout_of(&concrete);
                (field.name.clone(), concrete, layout)
            })
            .collect();
        let mut layout_input: Vec<(String, Layout)> =
            field_data.iter().map(|(name, _, layout)| (name.clone(), layout.clone())).collect();
        let has_drop = self.has_drop(name, &decl.methods);
        if has_drop {
            layout_input.push((LIVE_FIELD.to_string(), scalar_layout(&Type::Bool).expect("bool has a layout")));
        }
        let layout = struct_layout(&layout_input, Repr::Default);
        let live_offset = layout.fields.iter().find(|field| field.name == LIVE_FIELD).map(|field| field.offset);
        let fields = layout
            .fields
            .iter()
            .filter(|field_layout| field_layout.name != LIVE_FIELD)
            .map(|field_layout| {
                let (_, ty, _) = field_data
                    .iter()
                    .find(|(name, _, _)| *name == field_layout.name)
                    .expect("struct_layout preserves every input field");
                (field_layout.name.clone(), (ty.clone(), field_layout.offset))
            })
            .collect();

        let info = StructInfo { layout, fields, live_offset };
        self.struct_in_progress.borrow_mut().remove(&key);
        self.struct_cache.borrow_mut().insert(key, info.clone());
        info
    }

    fn enum_info(&self, name: &str, type_args: &[Type]) -> EnumInfo {
        let key = (name.to_string(), type_args.to_vec());
        if let Some(info) = self.enum_cache.borrow().get(&key) {
            return info.clone();
        }

        if !self.enum_in_progress.borrow_mut().insert(key.clone()) {
            panic!("recursive generic enum layout detected for `{name}` with type arguments {type_args:?}");
        }
        self.check_instantiation_cap(name, type_args, "enum", &self.enum_cache);

        let decl = *self
            .enum_decls
            .get(name)
            .unwrap_or_else(|| panic!("unknown enum `{name}` in layout table"));
        let substitutions: HashMap<String, Type> =
            generic_names(&decl.generics).into_iter().zip(type_args.iter().cloned()).collect();

        let mut max_payload_size = 0u64;
        let mut max_payload_align = 1u64;
        let mut variants = HashMap::new();
        let variant_index = decl
            .variants
            .iter()
            .enumerate()
            .map(|(index, variant)| (variant.name.clone(), index as u64))
            .collect();
        for variant in &decl.variants {
            let field_tys: &[Ty] = match &variant.fields {
                VariantFields::Unit => &[],
                VariantFields::Tuple(tys) => tys,
                VariantFields::Struct(_) => {
                    panic!("struct-style enum variant layout is not implemented yet: `{name}::{}`", variant.name)
                }
            };
            let field_data: Vec<(Type, Layout)> = field_tys
                .iter()
                .map(|ty| {
                    let template = self.resolve_ty_template(ty, &generic_names(&decl.generics));
                    let concrete = substitute_generics(&template, &substitutions);
                    let layout = self.layout_of(&concrete);
                    (concrete, layout)
                })
                .collect();
            let named: Vec<(String, Layout)> =
                field_data.iter().enumerate().map(|(index, (_, layout))| (index.to_string(), layout.clone())).collect();
            let payload = struct_layout(&named, Repr::C);
            max_payload_size = max_payload_size.max(payload.size);
            max_payload_align = max_payload_align.max(payload.align);
            let fields = payload
                .fields
                .iter()
                .map(|field_layout| {
                    let index: usize = field_layout.name.parse().expect("positional field name");
                    (field_data[index].0.clone(), field_layout.offset + DISCRIMINANT_SIZE)
                })
                .collect();
            variants.insert(variant.name.clone(), fields);
        }
        let align = max_payload_align.max(DISCRIMINANT_SIZE);
        let live_offset = self.has_drop(name, &decl.methods).then_some(DISCRIMINANT_SIZE + max_payload_size);
        let size = align_up(DISCRIMINANT_SIZE + max_payload_size + u64::from(live_offset.is_some()), align);

        let info = EnumInfo {
            layout: Layout { size, align, fields: Vec::new() },
            variants,
            variant_index,
            live_offset,
        };
        self.enum_in_progress.borrow_mut().remove(&key);
        self.enum_cache.borrow_mut().insert(key, info.clone());
        info
    }

    /// Converts an AST field type into a layout `Type` *template*: the
    /// enclosing struct/enum's own generic parameter names become
    /// `Type::Generic`, everything else resolves to a concrete or
    /// still-generic-carrying type immediately. `struct_info`/`enum_info`
    /// substitute the template's `Type::Generic` markers with the caller's
    /// concrete type arguments afterward.
    fn resolve_ty_template(&self, ty: &Ty, generics: &[String]) -> Type {
        match ty {
            Ty::Path(path, _) if path.len() == 1 => {
                if generics.iter().any(|g| g == &path[0]) {
                    return Type::Generic(path[0].clone());
                }
                if let Some(ty) = builtin_ty(&path[0]) {
                    return ty;
                }
                if let Some(ty) = prelude_error_ty(&path[0]) {
                    return ty;
                }
                if self.struct_decls.contains_key(path[0].as_str()) {
                    return Type::Struct(path[0].clone(), Vec::new());
                }
                if self.enum_decls.contains_key(path[0].as_str()) {
                    return Type::Enum(path[0].clone(), Vec::new());
                }
                if is_builtin_handle(&path[0]) {
                    return Type::Struct(path[0].clone(), Vec::new());
                }
                panic!("unknown type `{}` in aggregate layout", path[0]);
            }
            Ty::Path(path, _) => {
                let key = path.join("::");
                if self.struct_decls.contains_key(key.as_str()) {
                    return Type::Struct(key, Vec::new());
                }
                if self.enum_decls.contains_key(key.as_str()) {
                    return Type::Enum(key, Vec::new());
                }
                panic!("unknown type `{key}` in aggregate layout");
            }
            Ty::Generic { path, args, .. } => {
                let key = path.join("::");
                let params = match (self.struct_decls.get(key.as_str()), self.enum_decls.get(key.as_str())) {
                    (Some(decl), _) => decl.generics.as_slice(),
                    (_, Some(decl)) => decl.generics.as_slice(),
                    _ if is_builtin_handle(&key) => {
                        return Type::Struct(key, args.iter().map(|arg| self.resolve_ty_template(arg, generics)).collect());
                    }
                    _ => panic!("unknown generic type `{key}` in aggregate layout"),
                };
                let arg_templates = self.generic_arg_templates(params, args, generics);
                if self.struct_decls.contains_key(key.as_str()) {
                    return Type::Struct(key, arg_templates);
                }
                Type::Enum(key, arg_templates)
            }
            Ty::Const(..) | Ty::DynDim(_) | Ty::Expand(..) => self.const_arg_template(ty, generics),
            Ty::Slice(elem, _) => Type::Slice(Box::new(self.resolve_ty_template(elem, generics))),
            Ty::Borrow { mutable, ty, .. } => {
                Type::Borrow { mutable: *mutable, ty: Box::new(self.resolve_ty_template(ty, generics)) }
            }
            Ty::RawPointer { mutable, ty, .. } => {
                Type::RawPointer { mutable: *mutable, ty: Box::new(self.resolve_ty_template(ty, generics)) }
            }
            Ty::Tuple(items, _) if items.is_empty() => Type::Unit,
            Ty::Tuple(items, _) => Type::Tuple(items.iter().map(|item| self.resolve_ty_template(item, generics)).collect()),
            Ty::Fn { params, return_ty, .. } => Type::Fn(
                params.iter().map(|param| self.resolve_ty_template(param, generics)).collect(),
                Box::new(return_ty.as_ref().map_or(Type::Unit, |ret| self.resolve_ty_template(ret, generics))),
            ),
            _ => panic!("aggregate layout for this field type is not implemented yet: {ty:?}"),
        }
    }

    fn generic_arg_templates(&self, params: &[GenericParam], args: &[Ty], generics: &[String]) -> Vec<Type> {
        let params: Vec<&GenericParam> =
            params.iter().filter(|param| param.kind != GenericParamKind::Lifetime).collect();
        let mut out = Vec::with_capacity(params.len());
        for (index, param) in params.iter().enumerate() {
            match param.kind {
                GenericParamKind::ConstPack(_) => {
                    let rest = args.get(index..).unwrap_or(&[]);
                    if let [Ty::Expand(name, _)] = rest {
                        out.push(Type::Generic(name.clone()));
                    } else {
                        out.push(Type::Pack(rest.iter().map(|arg| self.const_arg_template(arg, generics)).collect()));
                    }
                }
                GenericParamKind::Const(_) => {
                    out.extend(args.get(index).map(|arg| self.const_arg_template(arg, generics)));
                }
                _ => out.extend(args.get(index).map(|arg| self.resolve_ty_template(arg, generics))),
            }
        }
        out
    }

    /// Dimensions never change a layout; a literal keeps its value so the
    /// field's type stays exact, anything else becomes a parameter or `Dyn`.
    fn const_arg_template(&self, ty: &Ty, generics: &[String]) -> Type {
        match ty {
            Ty::Expand(name, _) if generics.contains(name) => Type::Generic(name.clone()),
            Ty::Path(path, _) if path.len() == 1 && generics.contains(&path[0]) => Type::Generic(path[0].clone()),
            Ty::Const(expr, _) => match expr.as_ref() {
                paco_syntax::ast::Expr::Literal(paco_syntax::ast::Literal::Int(value), _) => {
                    Type::Dim(Dim::Const(ConstExpr::lit(*value)))
                }
                _ => Type::Dim(Dim::Dyn),
            },
            _ => Type::Dim(Dim::Dyn),
        }
    }

    fn layout_of(&self, ty: &Type) -> Layout {
        if let Some(layout) = scalar_layout(ty) {
            return layout;
        }
        match ty {
            Type::Struct(name, _) if !self.has_struct(name) && is_builtin_handle(name) => {
                Layout { size: 8, align: 8, fields: Vec::new() }
            }
            Type::Struct(name, args) => self.struct_layout(name, args),
            Type::Enum(name, args) => self.enum_layout(name, args),
            Type::Tuple(items) => self.tuple_layout(items),
            Type::Generic(name) => {
                panic!("unresolved generic type parameter `{name}` in aggregate layout (substitution bug)")
            }
            _ => panic!("aggregate layout for this field type is not implemented yet: {ty:?}"),
        }
    }

    fn check_instantiation_cap<V>(
        &self,
        name: &str,
        type_args: &[Type],
        kind: &str,
        cache: &RefCell<HashMap<InstantiationKey, V>>,
    ) {
        let count = cache.borrow().keys().filter(|(cached_name, _)| cached_name == name).count();
        if count >= self.max_instantiations {
            panic!(
                "generic {kind} `{name}` exceeded the instantiation cap ({} distinct sets of type \
                 arguments) while instantiating it with {type_args:?}; this usually means a type \
                 parameter is unexpectedly unbounded (e.g. unbounded recursive generic construction), \
                 not that `{name}` legitimately needs this many instantiations",
                self.max_instantiations
            );
        }
    }
}

pub(crate) fn is_builtin_handle(name: &str) -> bool {
    matches!(
        name,
        "Rc" | "Arc" | "Cell" | "RefCell" | "Mutex" | "RwLock" | "Sender" | "Receiver" | "JoinHandle" | "Generator"
            | "TcpListener" | "TcpStream"
    )
}

fn builtin_ty(name: &str) -> Option<Type> {
    Some(match name {
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
        // `type` (`phase-9-comptime` Decision 5) never has a runtime
        // representation — any compiled code path that could actually
        // read/write a `type`-typed field or parameter is already a
        // compile error (`paco-types`' own `requires_comptime` check), so
        // this placeholder layout is only ever computed for the
        // compiler's own static aggregate-layout analysis of a prelude
        // struct/method that happens to mention `type` (e.g. `FieldInfo`),
        // never exercised by an actual running program.
        "type" => Type::TypeValue(Box::new(Type::Unknown)),
        _ => return None,
    })
}

fn prelude_error_ty(name: &str) -> Option<Type> {
    match name {
        "SendError" | "RecvError" | "TaskPanic" => Some(Type::Struct(name.to_string(), Vec::new())),
        _ => None,
    }
}

fn prelude_error_fields(name: &str) -> Vec<(&'static str, Type, u64)> {
    match name {
        "TaskPanic" => vec![("message", Type::String, 0)],
        _ => Vec::new(),
    }
}

fn prelude_error_struct(name: &str) -> Option<(Type, Layout)> {
    let fields = prelude_error_fields(name);
    let size = fields.iter().map(|(_, ty, offset)| offset + crate::scalar_layout(ty).map_or(8, |layout| layout.size)).max().unwrap_or(0);
    let layout = Layout {
        size,
        align: if fields.is_empty() { 1 } else { 8 },
        fields: fields.iter().map(|(name, _, offset)| crate::FieldLayout { name: name.to_string(), offset: *offset }).collect(),
    };
    prelude_error_ty(name).map(|ty| (ty, layout))
}
