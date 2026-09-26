//! Backend-independent facts about owning types: which types need drop and
//! clone glue, and the runtime layouts that glue manipulates.

use std::collections::{HashMap, HashSet};

use paco_types::Type;

use crate::{Body, LocalDecl, Operand, Place, Rvalue, Statement, TypeLayouts, mangled_name, scalar_layout};

pub const CELL_COUNT_OFFSET: i32 = 0;
pub const CELL_LOCK_OFFSET: i32 = 8;
pub const CELL_VALUE_OFFSET: i32 = 16;
/// A closure value points just past its environment's reference count, at
/// `[thunk, drop_fn, captures...]`.
pub const CLOSURE_HEADER: i64 = 8;
pub const CLOSURE_DROP_FN: i32 = 8;
pub const SLICE_DATA_OFFSET: i32 = 0;
pub const SLICE_LEN_OFFSET: i32 = 8;

pub fn is_cell(ty: &Type, layouts: &TypeLayouts<'_>) -> bool {
    matches!(ty, Type::Struct(name, _)
        if matches!(name.as_str(), "Rc" | "Arc" | "Cell" | "RefCell" | "Mutex" | "RwLock") && !layouts.has_struct(name))
}

/// The runtime's `(retain, release)` pair for an opaque concurrency handle.
pub fn handle_fns(ty: &Type, layouts: &TypeLayouts<'_>) -> Option<(&'static str, &'static str)> {
    let Type::Struct(name, _) = ty else { return None };
    if layouts.has_struct(name) {
        return None;
    }
    match name.as_str() {
        "Sender" => Some(("paco_rt_sender_retain", "paco_rt_sender_release")),
        "Receiver" => Some(("paco_rt_receiver_retain", "paco_rt_receiver_release")),
        "JoinHandle" => Some(("paco_rt_join_handle_retain", "paco_rt_join_handle_release")),
        "Generator" => Some(("paco_rt_generator_retain", "paco_rt_generator_release")),
        "TcpListener" => Some(("paco_rt_tcp_listener_retain", "paco_rt_tcp_listener_release")),
        "TcpStream" => Some(("paco_rt_tcp_stream_retain", "paco_rt_tcp_stream_release")),
        _ => None,
    }
}

/// The byte size of one `[]T` element (or boxed value) of type `elem`.
pub fn element_size(elem: &Type, layouts: &TypeLayouts<'_>) -> u64 {
    match elem {
        Type::Struct(name, args) if layouts.has_struct(name) => layouts.struct_layout(name, args).size,
        Type::Struct(..) => 8,
        Type::Enum(name, args) => layouts.enum_layout(name, args).size,
        Type::Tuple(items) => layouts.tuple_layout(items).size,
        _ => scalar_layout(elem).map_or(8, |layout| layout.size),
    }
}

/// Maps each type with a user `fn drop(&mut self)` to that function's symbol.
pub fn user_drop_fns(bodies: &[(String, Body)]) -> HashMap<Type, String> {
    bodies
        .iter()
        .filter_map(|(name, body)| {
            let [LocalDecl { ty: Type::Borrow { mutable: true, ty }, .. }] = &body.locals[..body.param_count] else {
                return None;
            };
            let (Type::Struct(owner, args) | Type::Enum(owner, args)) = ty.as_ref() else { return None };
            (*name == mangled_name(&format!("{owner}::drop"), args) && body.return_ty == Type::Unit)
                .then(|| (ty.as_ref().clone(), name.clone()))
        })
        .collect()
}

struct Analysis<'a, 'l, V> {
    layouts: &'a TypeLayouts<'l>,
    user_drops: &'a HashMap<Type, V>,
    memo: HashMap<Type, bool>,
    visiting: HashSet<Type>,
}

impl<V> Analysis<'_, '_, V> {
    fn needs_drop(&mut self, ty: &Type) -> bool {
        if let Some(known) = self.memo.get(ty) {
            return *known;
        }
        if !self.visiting.insert(ty.clone()) {
            return false;
        }
        let result = match ty {
            Type::String | Type::Slice(_) | Type::Fn(..) => true,
            _ if is_cell(ty, self.layouts) || handle_fns(ty, self.layouts).is_some() => true,
            _ if unresolved(ty) => false,
            Type::Struct(name, _) if !self.layouts.has_struct(name) => false,
            Type::Struct(..) | Type::Enum(..) | Type::Tuple(_) => {
                self.user_drops.contains_key(ty) || children(ty, self.layouts).iter().any(|child| self.needs_drop(child))
            }
            _ => false,
        };
        self.visiting.remove(ty);
        self.memo.insert(ty.clone(), result);
        result
    }
}

pub fn unresolved(ty: &Type) -> bool {
    match ty {
        Type::Generic(_) | Type::Unknown | Type::Error => true,
        Type::Struct(_, args) | Type::Enum(_, args) | Type::Tuple(args) => args.iter().any(unresolved),
        Type::Slice(inner) | Type::Borrow { ty: inner, .. } | Type::RawPointer { ty: inner, .. } => unresolved(inner),
        _ => false,
    }
}

pub fn children(ty: &Type, layouts: &TypeLayouts<'_>) -> Vec<Type> {
    match ty {
        Type::Slice(elem) => vec![elem.as_ref().clone()],
        Type::Struct(_, args) if is_cell(ty, layouts) => args.iter().take(1).cloned().collect(),
        Type::Struct(name, args) if layouts.has_struct(name) => {
            layouts.struct_fields(name, args).into_iter().map(|(ty, _)| ty).collect()
        }
        Type::Enum(name, args) => layouts
            .enum_variants(name, args)
            .into_iter()
            .flat_map(|(_, fields)| fields.into_iter().map(|(ty, _)| ty))
            .collect(),
        Type::Tuple(items) => items.clone(),
        _ => Vec::new(),
    }
}

fn body_types(body: &Body, out: &mut Vec<Type>) {
    out.extend(body.locals.iter().map(|local| local.ty.clone()));
    for block in &body.blocks {
        for statement in &block.statements {
            match statement {
                Statement::Assign(place, rvalue) => {
                    place_types(place, out);
                    match rvalue {
                        Rvalue::Aggregate { ty, .. } | Rvalue::Load { ty, .. } => out.push(ty.clone()),
                        Rvalue::Ref { place, .. } | Rvalue::Discriminant(place) | Rvalue::SliceLen(place) => {
                            place_types(place, out)
                        }
                        Rvalue::Use(Operand::Copy(place) | Operand::Move(place)) => place_types(place, out),
                        _ => {}
                    }
                }
                Statement::Store { ty, .. } => out.push(ty.clone()),
                Statement::FreeBox { .. } | Statement::Drop(_) | Statement::StorageDead(_) => {}
            }
        }
    }
}

fn place_types(place: &Place, out: &mut Vec<Type>) {
    match place {
        Place::Local(_) => {}
        Place::Field { base, .. } | Place::VariantField { base, .. } | Place::Index { base, .. } => place_types(base, out),
        Place::Deref { ty, .. } => out.push(ty.clone()),
    }
}

/// Every type reachable from `bodies` that owns heap storage or runs a user
/// destructor, i.e. every type that needs drop and clone glue.
pub fn owning_types<V>(bodies: &[(String, Body)], layouts: &TypeLayouts<'_>, user_drops: &HashMap<Type, V>) -> Vec<Type> {
    let mut pending = vec![Type::String];
    for (_, body) in bodies {
        body_types(body, &mut pending);
    }
    let mut seen = HashSet::new();
    let mut analysis = Analysis { layouts, user_drops, memo: HashMap::new(), visiting: HashSet::new() };
    let mut owning = Vec::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty.clone()) || unresolved(&ty) {
            continue;
        }
        if analysis.needs_drop(&ty) {
            owning.push(ty.clone());
        }
        match &ty {
            Type::Borrow { ty, .. } | Type::RawPointer { ty, .. } => pending.push(ty.as_ref().clone()),
            _ => pending.extend(children(&ty, layouts)),
        }
    }
    owning
}

/// The fields glue visits for a struct or tuple, with their byte offsets.
pub fn glue_fields(ty: &Type, layouts: &TypeLayouts<'_>) -> Vec<(Type, u64)> {
    match ty {
        Type::Struct(name, args) => layouts.struct_fields(name, args),
        Type::Tuple(items) => (0..items.len()).map(|index| layouts.tuple_field(items, index)).collect(),
        _ => Vec::new(),
    }
}
