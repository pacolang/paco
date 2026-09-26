//! Symbolic dimensions after type checking: every dimension known only at
//! run time is `Dyn` in an instance's key, and its value travels as a hidden
//! leading `i64` argument.

use paco_syntax::ast::{FnDecl, GenericParam, GenericParamKind};
use paco_types::{Dim, Type};

/// A hidden argument: the dimension at `args[arg]`, or at element `item` of
/// the pack there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Slot {
    pub arg: usize,
    pub item: Option<usize>,
}

fn kinds<'p>(owner: &'p [GenericParam], own: Option<&'p FnDecl>) -> Vec<&'p GenericParamKind> {
    owner
        .iter()
        .chain(own.map_or(&[][..], |function| &function.generics[..]))
        .filter(|param| param.kind != GenericParamKind::Lifetime)
        .map(|param| &param.kind)
        .collect()
}

/// The type arguments an instance is keyed by: symbolic dimensions and every
/// `dim` parameter collapse to `Dyn`.
pub fn instance_args(owner: &[GenericParam], own: Option<&FnDecl>, args: &[Type]) -> Vec<Type> {
    let kinds = kinds(owner, own);
    args.iter()
        .enumerate()
        .map(|(index, arg)| match kinds.get(index) {
            Some(GenericParamKind::Dim) => Type::Dim(Dim::Dyn),
            _ => paco_types::erase_symbolic(arg),
        })
        .collect()
}

/// Where an instance keyed by `args` takes hidden dimension arguments: each
/// `dim` parameter, and each `const` position bound to `Dyn`. A `drop`
/// method and an `iter fn` take none, since their callers are generated.
pub fn hidden_slots(owner: &[GenericParam], own: Option<&FnDecl>, args: &[Type]) -> Vec<Slot> {
    if own.is_some_and(|function| function.is_iter || (function.name == "drop" && function.params.len() == 1)) {
        return Vec::new();
    }
    let kinds = kinds(owner, own);
    let mut slots = Vec::new();
    for (arg, (kind, ty)) in kinds.iter().zip(args).enumerate() {
        match (kind, ty) {
            (GenericParamKind::Dim, _) | (GenericParamKind::Const(_), Type::Dim(Dim::Dyn)) => slots.push(Slot { arg, item: None }),
            (GenericParamKind::ConstPack(_), Type::Pack(items)) => {
                for (item, element) in items.iter().enumerate() {
                    if *element == Type::Dim(Dim::Dyn) {
                        slots.push(Slot { arg, item: Some(item) });
                    }
                }
            }
            _ => {}
        }
    }
    slots
}

pub fn slot_type(args: &[Type], slot: Slot) -> Type {
    match (args.get(slot.arg), slot.item) {
        (Some(Type::Pack(items)), Some(item)) => items.get(item).cloned().unwrap_or(Type::Dim(Dim::Dyn)),
        (Some(ty), None) => ty.clone(),
        _ => Type::Dim(Dim::Dyn),
    }
}

pub fn with_slot(args: &mut [Type], slot: Slot, ty: Type) {
    match (args.get_mut(slot.arg), slot.item) {
        (Some(Type::Pack(items)), Some(item)) => items[item] = ty,
        (Some(arg), None) => *arg = ty,
        _ => {}
    }
}

/// The name the parameter behind `slot` has, for messages and hidden names.
pub fn slot_name(owner: &[GenericParam], own: Option<&FnDecl>, slot: Slot) -> String {
    owner
        .iter()
        .chain(own.map_or(&[][..], |function| &function.generics[..]))
        .filter(|param| param.kind != GenericParamKind::Lifetime)
        .nth(slot.arg)
        .map_or_else(|| "D".to_string(), |param| param.name.clone())
}
