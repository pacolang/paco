//! Reverse-mode automatic differentiation over MIR.
//!
//! Every `grad(f, inputs)` call site (lowered to a call to [`GRAD_PREFIX`]
//! followed by `f`'s symbol) is replaced by a call to `f`'s augmented
//! primal, which runs `f` while recording on a tape the branches it takes
//! and the values it overwrites, followed by a call to `f`'s pullback, which
//! replays the tape backwards accumulating adjoints. Both are ordinary MIR,
//! generated per function and per set of differentiated parameters, so every
//! backend runs them and a pullback can itself be differentiated.

pub mod activity;
mod builder;
mod transform;
pub mod types;

use std::collections::{HashMap, HashSet};

use paco_diag::Diagnostic;
use paco_span::Span;
use paco_types::{FloatWidth, IntWidth, Type};

use crate::body::{Body, CallTarget, Local, Operand, Place, Rvalue, Statement, Terminator};
use crate::lower::GRAD_PREFIX;
use crate::TypeLayouts;

/// What the transform needs from the driver: bodies it did not lower
/// because nothing called them yet.
pub trait Host {
    /// The symbol of `owner`'s method `method`, lowering it if needed.
    fn method(&mut self, owner: &Type, method: &str) -> Option<String>;
    /// The symbol of the `#[derivative]` registered for the function whose
    /// symbol is `function`, lowering it if needed.
    fn derivative(&mut self, function: &str) -> Option<String>;
    /// Bodies lowered by `method` and `derivative` since the last call.
    fn take_bodies(&mut self) -> Vec<(String, Body)>;
}

fn ptr() -> Type {
    Type::Borrow { mutable: true, ty: Box::new(Type::Int(IntWidth::I64)) }
}

/// The tape runtime (`paco-runtime-ffi/src/autodiff.rs`), as extern
/// signatures for the backends.
pub fn runtime_externs() -> Vec<(String, Vec<Type>, Type)> {
    let i64 = Type::Int(IntWidth::I64);
    let f64 = Type::Float(FloatWidth::F64);
    let f32 = Type::Float(FloatWidth::F32);
    let unit = Type::Unit;
    [
        ("paco_ad_tape_new", vec![], i64.clone()),
        ("paco_ad_tape_free", vec![i64.clone()], unit.clone()),
        ("paco_ad_tape_retire", vec![i64.clone(), i64.clone()], unit.clone()),
        ("paco_ad_push_f64", vec![i64.clone(), f64.clone()], unit.clone()),
        ("paco_ad_pop_f64", vec![i64.clone()], f64.clone()),
        ("paco_ad_push_f32", vec![i64.clone(), f32.clone()], unit.clone()),
        ("paco_ad_pop_f32", vec![i64.clone()], f32.clone()),
        ("paco_ad_push_i64", vec![i64.clone(), i64.clone()], unit.clone()),
        ("paco_ad_pop_i64", vec![i64.clone()], i64.clone()),
        ("paco_ad_push_bytes", vec![i64.clone(), ptr(), i64.clone()], unit.clone()),
        ("paco_ad_pop_bytes", vec![i64.clone(), ptr(), i64.clone()], unit.clone()),
        ("paco_ad_adj_push_f64", vec![i64.clone(), f64.clone()], unit.clone()),
        ("paco_ad_adj_pop_f64", vec![i64.clone()], f64.clone()),
        ("paco_ad_adj_push_f32", vec![i64.clone(), f32.clone()], unit.clone()),
        ("paco_ad_adj_pop_f32", vec![i64.clone()], f32.clone()),
        ("paco_ad_shadow", vec![i64.clone(), ptr(), i64.clone(), ptr()], unit.clone()),
        ("paco_ad_copy", vec![ptr(), ptr(), i64.clone()], unit.clone()),
        ("paco_ad_hold", vec![i64.clone()], ptr()),
        ("paco_ad_unhold", vec![ptr()], unit),
    ]
    .into_iter()
    .map(|(name, params, ret)| (name.to_string(), params, ret))
    .collect()
}

/// An error found while differentiating, with the calls that led to it.
#[derive(Clone, Debug)]
pub struct AdError {
    pub code: &'static str,
    pub span: Span,
    pub message: String,
    pub chain: Vec<(String, Span)>,
}

impl AdError {
    pub fn into_diagnostic(self) -> Diagnostic {
        let mut diagnostic = Diagnostic::error(self.code, self.span, self.message);
        for (function, span) in &self.chain {
            let function = if function == crate::ENTRY_SYMBOL { "main" } else { function };
            diagnostic = diagnostic.with_secondary(*span, format!("reached through this call in `{function}`"));
        }
        diagnostic
    }
}

pub fn has_grad_sites(bodies: &[(String, Body)]) -> bool {
    bodies.iter().any(|(_, body)| grad_sites(body).next().is_some())
}

fn grad_sites(body: &Body) -> impl Iterator<Item = (usize, &str)> {
    body.blocks.iter().enumerate().filter_map(|(index, block)| match &block.terminator {
        Terminator::Call { target, .. } => target.0.strip_prefix(GRAD_PREFIX).map(|name| (index, name)),
        _ => None,
    })
}

/// Expands every `grad` site in `bodies`, adding the generated primals and
/// pullbacks. Returns the errors found, one per non-differentiable construct.
pub fn differentiate(bodies: &mut Vec<(String, Body)>, layouts: &TypeLayouts<'_>, externs: &[(String, Vec<Type>, Type)], host: &mut dyn Host) -> Vec<Diagnostic> {
    let mut program = transform::Program::new(std::mem::take(bodies), layouts, externs, host);
    let names: Vec<String> = program.names_with_sites();
    for name in names {
        program.expand(&name, &mut Vec::new());
    }
    let mut errors = program.errors();
    let mut seen = HashSet::new();
    errors.retain(|error| seen.insert((error.code, error.span.file_id(), error.span.start(), error.message.clone())));
    *bodies = program.into_bodies();
    errors.into_iter().map(AdError::into_diagnostic).collect()
}


pub(crate) fn map_place(place: &mut Place, map: &dyn Fn(Local) -> Local) {
    match place {
        Place::Local(local) => *local = map(*local),
        Place::Field { base, .. } | Place::VariantField { base, .. } => map_place(base, map),
        Place::Index { base, index } => {
            map_place(base, map);
            map_operand(index, map);
        }
        Place::Deref { address, .. } => map_operand(address, map),
    }
}

pub(crate) fn map_operand(operand: &mut Operand, map: &dyn Fn(Local) -> Local) {
    if let Operand::Copy(place) | Operand::Move(place) = operand {
        map_place(place, map);
    }
}

fn map_rvalue(rvalue: &mut Rvalue, map: &dyn Fn(Local) -> Local) {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::UnaryOp(_, operand) | Rvalue::Cast { operand, .. } | Rvalue::Load { address: operand, .. } => {
            map_operand(operand, map)
        }
        Rvalue::BinaryOp(_, left, right) => {
            map_operand(left, map);
            map_operand(right, map);
        }
        Rvalue::Aggregate { fields, .. } | Rvalue::Math(_, fields) => fields.iter_mut().for_each(|field| map_operand(field, map)),
        Rvalue::Ref { place, .. } | Rvalue::Discriminant(place) | Rvalue::SliceLen(place) => map_place(place, map),
        Rvalue::Quote { splices, .. } => splices.iter_mut().for_each(|(_, operand)| map_operand(operand, map)),
        Rvalue::RawAlloc { .. } | Rvalue::FuncAddr(_) => {}
    }
}

fn map_statement(statement: &mut Statement, map: &dyn Fn(Local) -> Local) {
    match statement {
        Statement::Assign(place, rvalue) => {
            map_place(place, map);
            map_rvalue(rvalue, map);
        }
        Statement::Store { address, value, .. } => {
            map_operand(address, map);
            map_operand(value, map);
        }
        Statement::FreeBox { address, .. } => map_operand(address, map),
        Statement::Drop(place) => map_place(place, map),
        Statement::StorageDead(local) => *local = map(*local),
    }
}

fn map_terminator(terminator: &mut Terminator, map: &dyn Fn(Local) -> Local) {
    match terminator {
        Terminator::SwitchInt { discriminant, .. } | Terminator::Return(discriminant) => map_operand(discriminant, map),
        Terminator::Call { args, destination, .. } => {
            args.iter_mut().for_each(|arg| map_operand(arg, map));
            if let Some(place) = destination {
                map_place(place, map);
            }
        }
        Terminator::CallIndirect { callee, args, destination, .. } => {
            map_operand(callee, map);
            args.iter_mut().for_each(|arg| map_operand(arg, map));
            if let Some(place) = destination {
                map_place(place, map);
            }
        }
        Terminator::Goto(_) | Terminator::Unreachable => {}
    }
}

/// Names of call targets `body` reaches directly.
pub(crate) fn call_targets(body: &Body) -> HashSet<&str> {
    body.blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Call { target: CallTarget(name), .. } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

pub(crate) fn param_types(body: &Body) -> Vec<Type> {
    body.locals[..body.param_count].iter().map(|local| local.ty.clone()).collect()
}

pub(crate) type Externs = HashMap<String, Vec<Type>>;
