//! Generation of augmented primals and pullbacks.
//!
//! The pullback restores, step by step backwards, the state the primal had
//! before each statement: the primal pushes the final value of every local
//! the pullback reads ("needed") when it returns, and the old value of such a
//! local before overwriting it; the pullback pops them in reverse. Derivative
//! rules therefore read operands exactly as the primal saw them. Borrows are
//! followed to the place they point at, so the pullback keeps its own copy
//! of the value behind each borrowed parameter.

use std::collections::{HashMap, HashSet};

use paco_span::Span;
use paco_types::{FloatWidth, IntWidth, Type};

use super::activity::{self, Activity, Callees, Summary, TAPE_LOCAL, base_local, call_arg_roots, is_tape_call, operand_place, ref_targets, root, rvalue_operands};
use super::builder::{Builder, copy, float, int};
use super::types::{Leaf, LeafKind, Path, Proj, Types, project, strip_borrow};
use super::{AdError, Externs, Host, map_operand, map_place, param_types};
use crate::TypeLayouts;
use crate::body::{
    BasicBlockId, BinOp, Body, Constant, Local, LocalDecl, MathOp, Operand, Place, Rvalue, Statement, Terminator, UnOp,
};
use crate::lower::PANIC_SYMBOL;

fn mask_suffix(mask: &[bool]) -> String {
    mask.iter().map(|active| if *active { '1' } else { '0' }).collect()
}

fn primal_name(name: &str, mask: &[bool]) -> String {
    format!("{name}__primal_{}", mask_suffix(mask))
}

fn pullback_name(name: &str, mask: &[bool]) -> String {
    format!("{name}__pullback_{}", mask_suffix(mask))
}

type Chain = Vec<(String, Span)>;

pub(super) struct Program<'l, 'a, 'h> {
    bodies: Vec<(String, Body)>,
    index: HashMap<String, usize>,
    types: Types<'l, 'a>,
    externs: Externs,
    user_externs: HashSet<String>,
    host: &'h mut dyn Host,
    expanded: HashSet<String>,
    expanding: Vec<String>,
    jobs: HashSet<(String, Vec<bool>)>,
    summaries: HashMap<String, Summary>,
    errors: Vec<AdError>,
}

struct Known<'p> {
    externs: &'p Externs,
    bodies: &'p [(String, Body)],
    index: &'p HashMap<String, usize>,
    summaries: &'p HashMap<String, Summary>,
}

impl Callees for Known<'_> {
    fn params(&self, target: &str) -> Option<Vec<Type>> {
        match self.index.get(target) {
            Some(&index) => Some(param_types(&self.bodies[index].1)),
            None => self.externs.get(target).cloned(),
        }
    }
    fn summary(&self, target: &str) -> Option<Summary> {
        self.summaries.get(target).cloned()
    }
}

impl<'l, 'a, 'h> Program<'l, 'a, 'h> {
    pub(super) fn new(bodies: Vec<(String, Body)>, layouts: &'l TypeLayouts<'a>, externs: &[(String, Vec<Type>, Type)], host: &'h mut dyn Host) -> Self {
        let index = bodies.iter().enumerate().map(|(index, (name, _))| (name.clone(), index)).collect();
        let mut all_externs: Externs = externs.iter().map(|(name, params, _)| (name.clone(), params.clone())).collect();
        for (name, params, _) in super::runtime_externs() {
            all_externs.insert(name, params);
        }
        Self {
            bodies,
            index,
            types: Types { layouts },
            externs: all_externs,
            user_externs: externs.iter().map(|(name, ..)| name.clone()).collect(),
            host,
            expanded: HashSet::new(),
            expanding: Vec::new(),
            jobs: HashSet::new(),
            summaries: HashMap::new(),
            errors: Vec::new(),
        }
    }

    pub(super) fn names_with_sites(&self) -> Vec<String> {
        self.bodies.iter().filter(|(_, body)| super::grad_sites(body).next().is_some()).map(|(name, _)| name.clone()).collect()
    }

    pub(super) fn errors(&self) -> Vec<AdError> {
        self.errors.clone()
    }

    pub(super) fn into_bodies(self) -> Vec<(String, Body)> {
        self.bodies
    }

    fn body(&self, name: &str) -> Option<&Body> {
        self.index.get(name).map(|&index| &self.bodies[index].1)
    }

    fn add_body(&mut self, name: String, body: Body) {
        match self.index.get(&name) {
            Some(&index) => self.bodies[index].1 = body,
            None => {
                self.index.insert(name.clone(), self.bodies.len());
                self.bodies.push((name, body));
            }
        }
    }

    fn absorb_host_bodies(&mut self) {
        for (name, body) in self.host.take_bodies() {
            if !self.index.contains_key(&name) {
                self.add_body(name, body);
            }
        }
    }

    /// Lowers, ahead of `Job::custom`'s own lookup, the `#[derivative]` (and
    /// its residual's `pullback`) of every function `body` calls directly —
    /// a generic instantiation is lowered lazily by the host, too late for
    /// `custom`'s own frozen snapshot of `self.bodies` to see it otherwise.
    fn prime_derivatives(&mut self, body: &Body) {
        for target in super::call_targets(body) {
            let Some(derivative) = self.host.derivative(target) else { continue };
            self.absorb_host_bodies();
            let Some(&index) = self.index.get(&derivative) else { continue };
            let Type::Tuple(items) = self.bodies[index].1.return_ty.clone() else { continue };
            if self.host.method(&items[1], "pullback").is_some() {
                self.absorb_host_bodies();
            }
        }
    }

    /// Replaces every `grad` site in `name`'s body, differentiating the
    /// functions they name (whose own sites are expanded first).
    pub(super) fn expand(&mut self, name: &str, chain: &mut Chain) {
        if self.expanded.contains(name) {
            return;
        }
        let Some(body) = self.body(name) else { return };
        if self.expanding.iter().any(|open| open == name) {
            self.errors.push(AdError {
                code: "PACO-E0812",
                span: body.span,
                message: format!("`{name}` takes the gradient of a function that calls `{name}` itself; a gradient cannot contain itself"),
                chain: chain.clone(),
            });
            return;
        }
        let sites: Vec<(usize, String)> = super::grad_sites(body).map(|(block, target)| (block, target.to_string())).collect();
        self.expanding.push(name.to_string());
        for (block, target) in sites {
            let Some(span) = self.body(name).map(|body| body.terminator_span(block)) else { continue };
            chain.push((name.to_string(), span));
            self.expand_reachable(&target, chain);
            let mask = match self.body(&target) {
                Some(body) => body.locals[..body.param_count].iter().map(|local| self.types.can_be_active(&local.ty)).collect(),
                None => Vec::new(),
            };
            let ok = self.ensure(&target, &mask, chain);
            chain.pop();
            if ok {
                self.rewrite_site(name, block, &target, &mask);
            }
        }
        self.expanding.pop();
        self.expanded.insert(name.to_string());
    }

    fn expand_reachable(&mut self, name: &str, chain: &mut Chain) {
        let mut seen = HashSet::new();
        let mut pending = vec![name.to_string()];
        while let Some(next) = pending.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            self.expand(&next, chain);
            if let Some(body) = self.body(&next) {
                pending.extend(super::call_targets(body).into_iter().map(str::to_string));
            }
        }
    }

    fn summary_for(&mut self, name: &str) {
        let pending: Vec<String> = {
            let mut reachable = Vec::new();
            let mut stack = vec![name.to_string()];
            while let Some(next) = stack.pop() {
                if self.summaries.contains_key(&next) || reachable.contains(&next) {
                    continue;
                }
                let Some(body) = self.body(&next) else { continue };
                stack.extend(super::call_targets(body).into_iter().map(str::to_string));
                reachable.push(next);
            }
            reachable
        };
        if pending.is_empty() {
            return;
        }
        let bodies: HashMap<String, &Body> = self.bodies.iter().map(|(name, body)| (name.clone(), body)).collect();
        let known = &self.summaries;
        let externs = &self.externs;
        let index = &self.index;
        let all = &self.bodies;
        let params = |target: &str| -> Option<Vec<Type>> {
            match index.get(target) {
                Some(&at) => Some(param_types(&all[at].1)),
                None => externs.get(target).cloned(),
            }
        };
        let mut computed = activity::summaries(&pending, &bodies, &self.types, &params);
        for (name, summary) in known {
            computed.entry(name.clone()).or_insert_with(|| summary.clone());
        }
        self.summaries = computed;
    }

    /// Generates `name`'s primal and pullback for `mask`, and those of
    /// everything they call with active arguments. False when an error
    /// was reported.
    fn ensure(&mut self, name: &str, mask: &[bool], chain: &Chain) -> bool {
        let before = self.errors.len();
        let mut pending: Vec<(String, Vec<bool>, Chain)> = vec![(name.to_string(), mask.to_vec(), chain.clone())];
        while let Some((name, mask, chain)) = pending.pop() {
            if !self.jobs.insert((name.clone(), mask.clone())) {
                continue;
            }
            let mut open = chain.clone();
            self.expand_reachable(&name, &mut open);
            self.summary_for(&name);
            let Some(body) = self.body(&name).cloned() else { continue };
            self.prime_derivatives(&body);
            let known = Known { externs: &self.externs, bodies: &self.bodies, index: &self.index, summaries: &self.summaries };
            let mut job = Job::new(&name, &body, &mask, &chain, &self.types, &known, &self.user_externs, &mut *self.host);
            job.analyze();
            let output = job.generate();
            self.errors.extend(output.errors);
            pending.extend(output.requests);
            if let Some((primal, pullback)) = output.bodies {
                self.add_body(primal_name(&name, &mask), primal);
                self.add_body(pullback_name(&name, &mask), pullback);
            }
            self.absorb_host_bodies();
        }
        self.errors.len() == before
    }

    /// Replaces the `grad` call in `block` of `name` with primal, pullback
    /// and the construction of `(output, gradients)`.
    fn rewrite_site(&mut self, name: &str, block: usize, target: &str, mask: &[bool]) {
        let Some(callee) = self.body(target).cloned() else { return };
        let Some(body) = self.body(name).cloned() else { return };
        let Terminator::Call { args, destination, resume, .. } = body.blocks[block].terminator.clone() else { return };
        let span = body.terminator_span(block);
        let mut builder = Builder::new(body);
        builder.span = span;
        builder.reopen(BasicBlockId(block as u32));
        let i64 = Type::Int(IntWidth::I64);
        let tape = builder.call_value("paco_ad_tape_new", Vec::new(), i64.clone());
        builder.body.locals[tape.0 as usize].name = Some(TAPE_LOCAL.to_string());
        let mut primal_args = args.clone();
        primal_args.push(copy(tape));
        let unit = callee.return_ty == Type::Unit;
        let output_ty = match &destination {
            Some(place) => match place_type(&builder.body, place, &self.types) {
                Type::Tuple(items) => items[0].clone(),
                _ => Type::Unknown,
            },
            None => Type::Unknown,
        };
        let output = builder.local(output_ty.clone());
        if unit {
            builder.call(&primal_name(target, mask), primal_args, None);
        } else {
            builder.call(&primal_name(target, mask), primal_args, Some(Place::Local(output)));
        }
        let param_tys = param_types(&callee);
        let mut adjoints: Vec<Vec<(Leaf, Local, Option<Local>)>> = Vec::new();
        let mut pointers = Vec::new();
        let mut saved_heap = Vec::new();
        for (index, (arg, param_ty)) in args.iter().zip(&param_tys).enumerate() {
            let mut slots = Vec::new();
            if !mask.get(index).copied().unwrap_or(false) {
                adjoints.push(slots);
                continue;
            }
            let pointee = strip_borrow(param_ty).clone();
            for leaf in self.types.leaves(&pointee) {
                match &leaf.kind {
                    LeafKind::Float(width) => {
                        let slot = builder.temp(Type::Float(*width), Rvalue::Use(float(0.0, *width)));
                        pointers.push(reference(&mut builder, Place::Local(slot), Type::Float(*width)));
                        slots.push((leaf.clone(), slot, None));
                    }
                    LeafKind::Opaque { tangent, .. } => {
                        let value = builder.local(tangent.clone());
                        let set = builder.temp(Type::Bool, Rvalue::Use(Operand::Constant(Constant::Bool(false))));
                        pointers.push(reference(&mut builder, Place::Local(value), tangent.clone()));
                        pointers.push(reference(&mut builder, Place::Local(set), Type::Bool));
                        slots.push((leaf.clone(), value, Some(set)));
                    }
                }
            }
            if unit && matches!(param_ty, Type::Borrow { mutable: true, .. }) {
                for (leaf, slot, _) in &slots {
                    if let LeafKind::Float(width) = leaf.kind {
                        builder.assign(Place::Local(*slot), Rvalue::Use(float(1.0, width)));
                    }
                }
                let value = pointee_operand(arg, &builder.body);
                builder.assign(Place::Local(output), Rvalue::Use(value));
            }
            if matches!(param_ty, Type::Borrow { mutable: true, .. })
                && self.types.has_heap(&pointee)
                && let Some(value) = operand_as_place(&pointee_operand(arg, &builder.body))
            {
                let saved = builder.temp(pointee.clone(), Rvalue::Use(Operand::Copy(value.clone())));
                saved_heap.push((value, saved));
            }
            adjoints.push(slots);
        }
        let mut pullback_args = vec![copy(tape)];
        if !unit {
            for leaf in self.types.leaves(&callee.return_ty) {
                if let LeafKind::Float(width) = leaf.kind {
                    pullback_args.push(float(1.0, width));
                }
            }
        }
        pullback_args.extend(pointers);
        builder.call(&pullback_name(target, mask), pullback_args, None);
        let mut gradients = Vec::new();
        for (index, (arg, param_ty)) in args.iter().zip(&param_tys).enumerate() {
            if !mask.get(index).copied().unwrap_or(false) {
                continue;
            }
            let pointee = strip_borrow(param_ty).clone();
            let primal = pointee_operand(arg, &builder.body);
            gradients.push(self.gradient(&mut builder, tape, primal, &pointee, &adjoints[index]));
        }
        for (place, saved) in saved_heap {
            builder.assign(place, Rvalue::Use(Operand::Move(Place::Local(saved))));
        }
        if let Some(destination) = destination {
            let gradient = match gradients.as_slice() {
                [single] => Operand::Move(Place::Local(*single)),
                many => {
                    let tys: Vec<Type> = many.iter().map(|local| builder.ty(*local).clone()).collect();
                    let tuple = builder.temp(Type::Tuple(tys.clone()), Rvalue::Aggregate {
                        ty: Type::Tuple(tys),
                        variant: None,
                        fields: many.iter().map(|local| Operand::Move(Place::Local(*local))).collect(),
                    });
                    Operand::Move(Place::Local(tuple))
                }
            };
            let ty = place_type(&builder.body, &destination, &self.types);
            builder.assign(destination, Rvalue::Aggregate { ty, variant: None, fields: vec![Operand::Move(Place::Local(output)), gradient] });
        }
        builder.call("paco_ad_tape_free", vec![copy(tape)], None);
        builder.goto(resume);
        let body = builder.into_body();
        self.add_body(name.to_string(), body);
        self.absorb_host_bodies();
    }

    /// The gradient of the input whose value is `primal`: its leaves'
    /// adjoints written into a copy of it, heap elements copied from their
    /// shadows, or its opaque adjoint.
    fn gradient(&mut self, builder: &mut Builder, tape: Local, primal: Operand, ty: &Type, slots: &[(Leaf, Local, Option<Local>)]) -> Local {
        if let [(Leaf { path, kind: LeafKind::Float(_) }, slot, _)] = slots
            && path.is_empty()
        {
            return *slot;
        }
        if let [(Leaf { path, kind: LeafKind::Opaque { primal: primal_ty, tangent } }, value, Some(set))] = slots
            && path.is_empty()
        {
            let result = builder.local(tangent.clone());
            let zero = self.host.method(primal_ty, "zero_tangent");
            let (value, set) = (*value, *set);
            builder.if_else(
                copy(set),
                |builder| builder.assign(Place::Local(result), Rvalue::Use(Operand::Move(Place::Local(value)))),
                |builder| {
                    let value = builder.temp(primal_ty.clone(), Rvalue::Use(primal.clone()));
                    let pointer = reference(builder, Place::Local(value), primal_ty.clone());
                    match zero {
                        Some(zero) => builder.call(&zero, vec![pointer], Some(Place::Local(result))),
                        None => builder.finish(Terminator::Unreachable),
                    }
                },
            );
            return result;
        }
        let result = builder.temp(ty.clone(), Rvalue::Use(primal.clone()));
        for (leaf, slot, _) in slots {
            if let LeafKind::Float(_) = leaf.kind {
                builder.assign(project(Place::Local(result), &leaf.path), Rvalue::Use(copy(*slot)));
            }
        }
        for (path, elem) in self.types.heap_paths(ty) {
            let Some(primal) = operand_as_place(&primal) else { continue };
            let shadow = shadow_of(builder, tape, project(primal, &path), &elem, &self.types);
            let target = project(Place::Local(result), &path);
            let len = builder.temp(Type::Int(IntWidth::I64), Rvalue::SliceLen(Place::Local(shadow)));
            builder.for_each(copy(len), |builder, index| {
                builder.assign(
                    Place::Index { base: Box::new(target.clone()), index: Box::new(copy(index)) },
                    Rvalue::Use(Operand::Copy(Place::Index { base: Box::new(Place::Local(shadow)), index: Box::new(copy(index)) })),
                );
            });
        }
        result
    }
}

/// The value an argument passes: behind it when it is a borrow.
fn pointee_operand(arg: &Operand, body: &Body) -> Operand {
    match arg {
        Operand::Copy(place) | Operand::Move(place) => match &body.locals.get(base_local(place).map_or(usize::MAX, |local| local.0 as usize)) {
            Some(decl) if matches!(place, Place::Local(_)) && matches!(decl.ty, Type::Borrow { .. }) => {
                let Type::Borrow { ty, .. } = &decl.ty else { unreachable!() };
                Operand::Copy(Place::Deref { address: Box::new(Operand::Copy(place.clone())), ty: (**ty).clone() })
            }
            _ => Operand::Copy(place.clone()),
        },
        constant => constant.clone(),
    }
}

fn operand_as_place(operand: &Operand) -> Option<Place> {
    operand_place(operand).cloned()
}

fn reference(builder: &mut Builder, place: Place, ty: Type) -> Operand {
    let local = builder.temp(Type::Borrow { mutable: true, ty: Box::new(ty) }, Rvalue::Ref { mutable: true, place });
    copy(local)
}

/// The adjoint slice of the slice at `place`.
fn shadow_of(builder: &mut Builder, tape: Local, place: Place, elem: &Type, types: &Types<'_, '_>) -> Local {
    let slice_ty = Type::Slice(Box::new(elem.clone()));
    let shadow = builder.local(slice_ty.clone());
    let primal = reference(builder, place, slice_ty.clone());
    let out = reference(builder, Place::Local(shadow), slice_ty);
    let size = types.layouts.size_of(elem) as i64;
    builder.call("paco_ad_shadow", vec![copy(tape), primal, int(size), out], None);
    shadow
}

pub(super) fn place_type(body: &Body, place: &Place, types: &Types<'_, '_>) -> Type {
    match place {
        Place::Local(local) => body.locals.get(local.0 as usize).map(|decl| decl.ty.clone()).unwrap_or(Type::Unknown),
        Place::Field { base, field } => types.field_type(&place_type(body, base, types), &Proj::Field(field.clone())),
        Place::VariantField { base, variant, index } => types.field_type(&place_type(body, base, types), &Proj::Variant(variant.clone(), *index)),
        Place::Index { base, .. } => match strip_borrow(&place_type(body, base, types)) {
            Type::Slice(elem) => (**elem).clone(),
            _ => Type::Unknown,
        },
        Place::Deref { ty, .. } => ty.clone(),
    }
}

/// How a scalar is saved on the tape.
#[derive(Clone, Debug)]
enum Scalar {
    F64,
    F32,
    /// `f16`/`bf16`, widened to `f32` exactly.
    Narrow(FloatWidth),
    Int(IntWidth),
    Bytes(u64),
}

/// How the pullback continues after reversing a block.
enum Next {
    Exit,
    Block(usize),
    Popped(Vec<(i128, Option<usize>)>),
}

const START: i128 = -1;

#[derive(Clone, Debug)]
enum CallKind {
    Plain,
    Nested { target: String, mask: Vec<bool> },
    Custom { derivative: String, pullback: String, result: Type, residual: Type, gradients: Type, params: Vec<Type> },
    PushFloat(FloatWidth),
    PopFloat(FloatWidth),
    TapeFree,
}

struct Output {
    bodies: Option<(Body, Body)>,
    errors: Vec<AdError>,
    requests: Vec<(String, Vec<bool>, Chain)>,
}

/// An adjoint slot: a float, or an opaque tangent and whether it is set.
#[derive(Clone, Copy, Debug)]
enum Slot {
    Float(Local, FloatWidth),
    Opaque(Local, Local),
}

struct Job<'j, 'l, 'a> {
    name: &'j str,
    body: &'j Body,
    mask: &'j [bool],
    chain: &'j Chain,
    types: &'j Types<'l, 'a>,
    callees: &'j Known<'j>,
    user_externs: &'j HashSet<String>,
    host: &'j mut dyn Host,
    targets: HashMap<Local, Place>,
    activity: Activity,
    needed: HashSet<Local>,
    calls: HashMap<usize, CallKind>,
    preds: Vec<Vec<usize>>,
    reachable: Vec<bool>,
    returns: Vec<usize>,
    errors: Vec<AdError>,
    requests: Vec<(String, Vec<bool>, Chain)>,
}

impl<'j, 'l, 'a> Job<'j, 'l, 'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: &'j str,
        body: &'j Body,
        mask: &'j [bool],
        chain: &'j Chain,
        types: &'j Types<'l, 'a>,
        callees: &'j Known<'j>,
        user_externs: &'j HashSet<String>,
        host: &'j mut dyn Host,
    ) -> Self {
        let targets = ref_targets(body);
        let activity = activity::activity(body, mask, types, callees);
        Job {
            name,
            body,
            mask,
            chain,
            types,
            callees,
            user_externs,
            host,
            targets,
            activity,
            needed: HashSet::new(),
            calls: HashMap::new(),
            preds: Vec::new(),
            reachable: Vec::new(),
            returns: Vec::new(),
            errors: Vec::new(),
            requests: Vec::new(),
        }
    }

    fn error(&mut self, code: &'static str, span: Span, message: String) {
        self.errors.push(AdError { code, span, message, chain: self.chain.clone() });
    }

    fn root(&self, place: &Place) -> Local {
        root(place, &self.targets)
    }

    fn active_place(&self, place: &Place) -> bool {
        self.activity.is_active(self.root(place))
    }

    fn active_operand(&self, operand: &Operand) -> bool {
        operand_place(operand).is_some_and(|place| self.active_place(place))
    }

    fn ty(&self, place: &Place) -> Type {
        place_type(self.body, place, self.types)
    }

    fn is_pointer_param(&self, local: Local) -> bool {
        (local.0 as usize) < self.body.param_count && matches!(self.body.locals[local.0 as usize].ty, Type::Borrow { .. })
    }

    fn analyze(&mut self) {
        let count = self.body.blocks.len();
        self.preds = vec![Vec::new(); count];
        self.reachable = vec![false; count];
        let mut stack = vec![0usize];
        while let Some(block) = stack.pop() {
            if block >= count || std::mem::replace(&mut self.reachable[block], true) {
                continue;
            }
            for next in successors(&self.body.blocks[block].terminator) {
                stack.push(next);
            }
        }
        for block in 0..count {
            if !self.reachable[block] {
                continue;
            }
            for next in successors(&self.body.blocks[block].terminator) {
                if !self.preds[next].contains(&block) {
                    self.preds[next].push(block);
                }
            }
            if matches!(self.body.blocks[block].terminator, Terminator::Return(_)) {
                self.returns.push(block);
            }
        }
        for block in 0..count {
            if self.reachable[block] {
                self.check_block(block);
            }
        }
        self.compute_needed();
    }

    fn check_block(&mut self, block: usize) {
        let body = self.body;
        for (index, statement) in body.blocks[block].statements.iter().enumerate() {
            let span = body.statement_span(block, index);
            match statement {
                Statement::Assign(place, rvalue) => {
                    let active = self.active_place(place);
                    if active && let Some(message) = self.unknown_pointer(place) {
                        self.error("PACO-E0811", span, message);
                    }
                    if !active {
                        continue;
                    }
                    self.check_projections(place, span);
                    for operand in rvalue_operands(rvalue) {
                        if let Some(source) = operand_place(operand) {
                            if let Some(message) = self.unknown_pointer(source) && self.active_place(source) {
                                self.error("PACO-E0811", span, message);
                            }
                            if self.active_place(source) {
                                self.check_projections(source, span);
                            }
                        }
                    }
                    if let Rvalue::Load { .. } | Rvalue::RawAlloc { .. } = rvalue {
                        self.error("PACO-E0811", span, "a value that carries a derivative is read through a raw address".to_string());
                    }
                    if let Rvalue::Cast { operand, target } = rvalue
                        && let Some(source) = operand_place(operand)
                        && matches!(self.ty(source), Type::RawPointer { .. } | Type::Int(_))
                        && matches!(target, Type::RawPointer { .. })
                    {
                        self.error("PACO-E0811", span, "a value that carries a derivative is reinterpreted as a raw pointer".to_string());
                    }
                }
                Statement::Store { value, .. } if operand_place(value).is_some_and(|place| self.activity.is_varied(self.root(place))) => {
                    self.error(
                        "PACO-E0811",
                        span,
                        "a value that carries a derivative is sent to another task or written through a raw address".to_string(),
                    );
                }
                _ => {}
            }
        }
        let span = body.terminator_span(block);
        match &body.blocks[block].terminator {
            Terminator::Call { target, args, destination, .. } => {
                let kind = self.classify_call(&target.0, args, destination.as_ref(), span);
                self.calls.insert(block, kind);
            }
            Terminator::CallIndirect { args, destination, .. } => {
                let active = args.iter().any(|arg| self.active_operand(arg)) || destination.as_ref().is_some_and(|place| self.active_place(place));
                if active {
                    self.error(
                        "PACO-E0812",
                        span,
                        "a value that carries a derivative is passed to a call through a function value, whose target is not known at compile time; call the function by name".to_string(),
                    );
                }
            }
            _ => {}
        }
    }

    /// A message when `place` reads through a borrow whose target is not
    /// known at compile time.
    fn unknown_pointer(&self, place: &Place) -> Option<String> {
        let local = base_local(place)?;
        let ty = self.body.locals.get(local.0 as usize)?.ty.clone();
        let through = matches!(place, Place::Deref { .. }) || !matches!(place, Place::Local(_)) && matches!(ty, Type::Borrow { .. });
        if !through {
            return None;
        }
        if matches!(ty, Type::RawPointer { .. }) {
            return Some("a value that carries a derivative is read or written through a raw pointer".to_string());
        }
        if self.is_pointer_param(local) || self.targets.contains_key(&local) || !matches!(ty, Type::Borrow { .. }) {
            return None;
        }
        Some("a value that carries a derivative is reached through a borrow whose target is not known at compile time (returned by a call or chosen at run time); borrow the value directly".to_string())
    }

    /// `PACO-E0815` for a field read of a value whose `Tangent` is not itself.
    fn check_projections(&mut self, place: &Place, span: Span) {
        let mut current = place;
        let mut projected = Vec::new();
        while let Place::Field { base, .. } | Place::VariantField { base, .. } | Place::Index { base, .. } = current {
            projected.push(base.as_ref());
            current = base;
        }
        for base in projected {
            let ty = self.ty(base);
            if let Some(tangent) = self.types.opaque_tangent(strip_borrow(&ty)) {
                self.error(
                    "PACO-E0815",
                    span,
                    format!(
                        "`{}` reads a field of a `{}` that carries a derivative, but its `Tangent` is `{}`, not itself, so the compiler cannot route the field's gradient; give `{}` a `#[derivative]`",
                        self.name,
                        strip_borrow(&ty).name(),
                        tangent.name(),
                        self.name
                    ),
                );
                return;
            }
        }
    }

    fn classify_call(&mut self, target: &str, args: &[Operand], destination: Option<&Place>, span: Span) -> CallKind {
        let params = self.callees.params(target);
        let roots = call_arg_roots(args, params.as_deref(), self.body, &self.targets);
        let dest_active = destination.is_some_and(|place| self.active_place(place));
        let arg_active: Vec<bool> = roots.iter().map(|root| root.is_some_and(|(local, _)| self.activity.is_active(local))).collect();
        let active = dest_active || arg_active.iter().any(|active| *active);
        let task_boundary = target.starts_with("paco_rt_spawn") || target == "paco_rt_send" || target == "paco_rt_channel";
        if task_boundary && roots.iter().flatten().any(|(local, _)| self.activity.is_varied(*local)) {
            self.error("PACO-E0811", span, "a value that carries a derivative is sent to another task or channel".to_string());
            return CallKind::Plain;
        }
        match target {
            "paco_ad_push_f64" | "paco_ad_push_f32" if arg_active.get(1).copied().unwrap_or(false) => {
                return CallKind::PushFloat(if target.ends_with("f64") { FloatWidth::F64 } else { FloatWidth::F32 });
            }
            "paco_ad_pop_f64" | "paco_ad_pop_f32" if dest_active => {
                return CallKind::PopFloat(if target.ends_with("f64") { FloatWidth::F64 } else { FloatWidth::F32 });
            }
            "paco_ad_tape_free" => return CallKind::TapeFree,
            _ => {}
        }
        if !active {
            return CallKind::Plain;
        }
        if matches!(target, "print" | "slice_of_zeros" | "paco_ad_tape_new" | "paco_ad_tape_retire" | "paco_ad_shadow") || target == PANIC_SYMBOL {
            return CallKind::Plain;
        }
        if is_tape_call(target) {
            self.error(
                "PACO-E0811",
                span,
                "this gradient differentiates a pullback that saves a structured value as raw bytes; second-order gradients support float scalars and structs of them".to_string(),
            );
            return CallKind::Plain;
        }
        if let Some(derivative) = self.host.derivative(target) {
            return self.custom(target, derivative, span);
        }
        if let Some(params) = params.filter(|_| self.callees.index.contains_key(target)) {
            let callee = &self.callees.bodies[self.callees.index[target]].1;
            let mask: Vec<bool> = params
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    arg_active.get(index).copied().unwrap_or(false)
                        && (self.types.can_be_active(param) || activity::is_tape_local(callee, Local(index as u32)))
                })
                .collect();
            if self.types.leaves(&callee.return_ty).iter().any(|leaf| matches!(leaf.kind, LeafKind::Opaque { .. })) {
                self.error(
                    "PACO-E0815",
                    span,
                    format!(
                        "`{target}` returns a `{}` that carries a derivative and whose `Tangent` is not itself; give `{target}` a `#[derivative]`",
                        callee.return_ty.name()
                    ),
                );
                return CallKind::Plain;
            }
            if strip_borrow(&callee.return_ty) != &callee.return_ty && dest_active {
                self.error(
                    "PACO-E0811",
                    span,
                    format!("`{target}` returns a borrow of a value that carries a derivative; return the value instead"),
                );
                return CallKind::Plain;
            }
            let mut chain = self.chain.clone();
            chain.push((self.name.to_string(), span));
            self.requests.push((target.to_string(), mask.clone(), chain));
            return CallKind::Nested { target: target.to_string(), mask };
        }
        let what = if self.user_externs.contains(target) {
            format!("the `extern` function `{target}`")
        } else {
            format!("`{}`, which the compiler cannot differentiate", target.trim_start_matches('$'))
        };
        let code = if target.starts_with("paco_rt_spawn") || target.starts_with("paco_rt_send") || target == "paco_rt_channel" || target == "slice_as_ptr" || target == "slice_as_mut_ptr" {
            "PACO-E0811"
        } else {
            "PACO-E0810"
        };
        let hint = if code == "PACO-E0810" { format!("; give it a derivative with `#[derivative(of = {target})]`") } else { String::new() };
        self.error(code, span, format!("a value that carries a derivative is passed to or returned from {what}{hint}"));
        CallKind::Plain
    }

    fn custom(&mut self, target: &str, derivative: String, span: Span) -> CallKind {
        let Some(body) = self.callees.index.get(&derivative).map(|&index| &self.callees.bodies[index].1) else {
            self.error("PACO-E0814", span, format!("the `#[derivative]` of `{target}` has no body"));
            return CallKind::Plain;
        };
        let Type::Tuple(items) = &body.return_ty else { return CallKind::Plain };
        let (result, residual) = (items[0].clone(), items[1].clone());
        let Some(pullback) = self.host.method(&residual, "pullback") else {
            self.error("PACO-E0814", span, format!("`{}` has no `pullback` method", residual.name()));
            return CallKind::Plain;
        };
        let gradients = match self.callees.index.get(&pullback).map(|&index| &self.callees.bodies[index].1) {
            Some(body) => body.return_ty.clone(),
            None => {
                self.error("PACO-E0814", span, format!("`{}::pullback` has no body", residual.name()));
                return CallKind::Plain;
            }
        };
        CallKind::Custom { derivative, pullback, result, residual, gradients, params: param_types(body) }
    }

    /// Locals the pullback reads, which the primal therefore saves.
    fn compute_needed(&mut self) {
        let mut needed = HashSet::new();
        let body = self.body;
        let index_roots = |place: &Place, needed: &mut HashSet<Local>, targets: &HashMap<Local, Place>| {
            let mut current = place;
            loop {
                match current {
                    Place::Index { base, index } => {
                        if let Some(index) = operand_place(index) {
                            needed.insert(root(index, targets));
                        }
                        needed.insert(root(base, targets));
                        current = base;
                    }
                    Place::Field { base, .. } | Place::VariantField { base, .. } => current = base,
                    Place::Deref { .. } | Place::Local(_) => break,
                }
            }
        };
        for (block_index, block) in body.blocks.iter().enumerate() {
            if !self.reachable[block_index] {
                continue;
            }
            for statement in &block.statements {
                let Statement::Assign(place, rvalue) = statement else { continue };
                if !self.active_place(place) {
                    continue;
                }
                index_roots(place, &mut needed, &self.targets);
                for operand in rvalue_operands(rvalue) {
                    if let Some(source) = operand_place(operand) {
                        index_roots(source, &mut needed, &self.targets);
                    }
                }
                let read = match rvalue {
                    Rvalue::BinaryOp(BinOp::Mul | BinOp::Div, left, right) => vec![left, right],
                    Rvalue::Math(_, args) => args.iter().collect(),
                    _ => Vec::new(),
                };
                for operand in read {
                    if let Some(source) = operand_place(operand) {
                        needed.insert(self.root(source));
                    }
                }
            }
            if let Terminator::Call { args, .. } = &block.terminator
                && matches!(self.calls.get(&block_index), Some(CallKind::PushFloat(_) | CallKind::PopFloat(_)))
                && let Some(tape) = args.first().and_then(operand_place)
            {
                needed.insert(self.root(tape));
            }
            if let Terminator::Call { destination, args, .. } = &block.terminator
                && let Some(CallKind::Custom { result, .. }) = self.calls.get(&block_index)
            {
                if let Some(place) = destination
                    && !matches!(result, Type::Float(_))
                {
                    needed.insert(self.root(place));
                }
                for place in args.iter().filter_map(operand_place) {
                    let root = self.root(place);
                    if self.types.has_heap(strip_borrow(&body.locals[root.0 as usize].ty)) {
                        needed.insert(root);
                    }
                }
            }
            if let Terminator::Call { args, destination, .. } = &block.terminator
                && !matches!(self.calls.get(&block_index), Some(CallKind::Plain) | None)
            {
                for arg in args {
                    if let Some(place) = operand_place(arg) {
                        index_roots(place, &mut needed, &self.targets);
                    }
                }
                if let Some(place) = destination {
                    index_roots(place, &mut needed, &self.targets);
                }
            }
        }
        loop {
            let before = needed.len();
            for block in &body.blocks {
                for statement in &block.statements {
                    if let Statement::Assign(place, _) = statement
                        && needed.contains(&self.root(place))
                    {
                        index_roots(place, &mut needed, &self.targets);
                    }
                }
            }
            if needed.len() == before {
                break;
            }
        }
        needed.retain(|local| (local.0 as usize) < body.locals.len());
        self.needed = needed;
    }

    fn generate(mut self) -> Output {
        if !self.errors.is_empty() {
            return Output { bodies: None, errors: self.errors, requests: Vec::new() };
        }
        let primal = self.primal();
        let pullback = self.pullback();
        let errors = std::mem::take(&mut self.errors);
        let requests = std::mem::take(&mut self.requests);
        match (primal, pullback, errors.is_empty()) {
            (primal, pullback, true) => Output { bodies: Some((primal, pullback)), errors, requests },
            _ => Output { bodies: None, errors, requests: Vec::new() },
        }
    }

    // ---- saving and restoring values -------------------------------------------------------

    /// The scalars `place` (of type `ty`) is saved as, in push order.
    fn flatten(&self, place: Place, ty: &Type, out: &mut Vec<(Place, Scalar)>) {
        match ty {
            Type::Float(FloatWidth::F64) => out.push((place, Scalar::F64)),
            Type::Float(FloatWidth::F32) => out.push((place, Scalar::F32)),
            Type::Float(width @ (FloatWidth::F16 | FloatWidth::BF16)) => out.push((place, Scalar::Narrow(*width))),
            Type::Int(width) => out.push((place, Scalar::Int(*width))),
            Type::Unit | Type::Never | Type::Borrow { .. } | Type::Fn(..) => {}
            Type::Struct(name, args) if self.types.layouts.has_struct(name) => {
                let base = match place {
                    Place::Deref { address, .. } => match *address {
                        Operand::Copy(pointer) | Operand::Move(pointer) => pointer,
                        Operand::Constant(_) => return,
                    },
                    other => other,
                };
                for (field, field_ty) in self.types.layouts.struct_field_list(name, args) {
                    self.flatten(Place::Field { base: Box::new(base.clone()), field }, &field_ty, out);
                }
            }
            Type::Tuple(items) => {
                let base = match place {
                    Place::Deref { address, .. } => match *address {
                        Operand::Copy(pointer) | Operand::Move(pointer) => pointer,
                        Operand::Constant(_) => return,
                    },
                    other => other,
                };
                for (index, item) in items.iter().enumerate() {
                    self.flatten(Place::Field { base: Box::new(base.clone()), field: index.to_string() }, item, out);
                }
            }
            other => {
                let size = self.types.layouts.size_of(other);
                if size > 0 {
                    out.push((place, Scalar::Bytes(size)));
                }
            }
        }
    }

    /// The places the primal saves before `statement` (in `block`).
    fn saves_for_assign(&self, place: &Place) -> Vec<Place> {
        if self.needed.contains(&self.root(place)) { vec![place.clone()] } else { Vec::new() }
    }

    /// Before a call: its destination and every value it may write through
    /// a `&mut` argument, when the pullback reads them.
    fn saves_for_call(&self, target: &str, args: &[Operand], destination: Option<&Place>) -> Vec<Place> {
        let mut saves = Vec::new();
        if let Some(place) = destination
            && self.needed.contains(&self.root(place))
        {
            saves.push(place.clone());
        }
        let params = self.callees.params(target).unwrap_or_default();
        for (index, arg) in args.iter().enumerate() {
            let Some(place) = operand_place(arg) else { continue };
            let arg_ty = self.ty(place);
            let pointee = match (params.get(index), &arg_ty) {
                (_, Type::Borrow { mutable: true, ty }) => Place::Deref { address: Box::new(Operand::Copy(place.clone())), ty: (**ty).clone() },
                (Some(Type::Borrow { mutable: true, .. }), _) => place.clone(),
                _ => continue,
            };
            if self.needed.contains(&self.root(&pointee)) {
                saves.push(pointee);
            }
        }
        saves
    }

    fn save_list(&self, places: &[Place]) -> Vec<(Place, Scalar)> {
        let mut out = Vec::new();
        for place in places {
            let ty = self.ty(place);
            self.flatten(place.clone(), &ty, &mut out);
        }
        out
    }

    /// Final values of every needed local, pushed when the primal returns.
    fn snapshot(&self) -> Vec<(Place, Scalar)> {
        let mut needed: Vec<Local> = self.needed.iter().copied().collect();
        needed.sort_by_key(|local| local.0);
        let mut out = Vec::new();
        for local in needed {
            let decl = &self.body.locals[local.0 as usize];
            if self.is_pointer_param(local) {
                let Type::Borrow { ty, .. } = &decl.ty else { continue };
                self.flatten(Place::Deref { address: Box::new(copy(local)), ty: (**ty).clone() }, ty, &mut out);
            } else {
                self.flatten(Place::Local(local), &decl.ty, &mut out);
            }
        }
        out
    }

    // ---- augmented primal ------------------------------------------------------------------

    fn primal(&mut self) -> Body {
        let body = self.body;
        let param_count = body.param_count;
        let shift = |local: Local| if (local.0 as usize) >= param_count { Local(local.0 + 1) } else { local };
        let mut locals: Vec<LocalDecl> = body.locals.clone();
        locals.insert(param_count, LocalDecl { name: Some(TAPE_LOCAL.to_string()), ty: Type::Int(IntWidth::I64), mutable: false });
        let tape = Local(param_count as u32);
        let mut builder = Builder::new(Body {
            locals,
            blocks: Vec::new(),
            profile: body.profile,
            param_count: param_count + 1,
            return_ty: body.return_ty.clone(),
            span: body.span,
            spans: Vec::new(),
        });
        let entry = builder.block();
        let map: Vec<BasicBlockId> = (0..body.blocks.len()).map(|_| builder.block()).collect();
        let mut edges: HashMap<(usize, usize), BasicBlockId> = HashMap::new();
        builder.switch_to(entry);
        if !self.preds[0].is_empty() {
            builder.call("paco_ad_push_i64", vec![copy(tape), int(START as i64)], None);
        }
        builder.goto(map[0]);
        let mut target_of = |from: usize, to: usize, builder: &mut Builder| -> BasicBlockId {
            if self.preds[to].len() + usize::from(to == 0) <= 1 {
                return map[to];
            }
            *edges.entry((from, to)).or_insert_with(|| builder.block())
        };
        let mut trampolines: Vec<(BasicBlockId, usize, usize)> = Vec::new();
        for (block, &mapped) in map.iter().enumerate() {
            if !self.reachable[block] {
                continue;
            }
            builder.switch_to(mapped);
            for (index, statement) in body.blocks[block].statements.iter().enumerate() {
                builder.span = body.statement_span(block, index);
                if let Statement::Assign(place, _) = statement {
                    let saves = self.save_list(&self.saves_for_assign(place));
                    emit_pushes(&mut builder, tape, &saves, &shift);
                }
                let mut statement = statement.clone();
                super::map_statement(&mut statement, &shift);
                builder.push(statement);
            }
            builder.span = body.terminator_span(block);
            let mut terminator = body.blocks[block].terminator.clone();
            match &mut terminator {
                Terminator::Return(_) => {
                    let snapshot = self.snapshot();
                    emit_pushes(&mut builder, tape, &snapshot, &shift);
                    if self.returns.len() > 1 {
                        builder.call("paco_ad_push_i64", vec![copy(tape), int(block as i64)], None);
                    }
                    super::map_terminator(&mut terminator, &shift);
                    builder.finish(terminator);
                    continue;
                }
                Terminator::Call { target, args, destination, resume } => {
                    let kind = self.calls.get(&block).cloned().unwrap_or(CallKind::Plain);
                    let original_target = target.0.clone();
                    let saves = self.save_list(&self.saves_for_call(&original_target, args, destination.as_ref()));
                    emit_pushes(&mut builder, tape, &saves, &shift);
                    let next = target_of(block, resume.0 as usize, &mut builder);
                    if self.preds[resume.0 as usize].len() > 1 {
                        trampolines.push((next, block, resume.0 as usize));
                    }
                    let mut args = args.clone();
                    args.iter_mut().for_each(|arg| map_operand(arg, &shift));
                    let mut destination = destination.clone();
                    if let Some(place) = &mut destination {
                        map_place(place, &shift);
                    }
                    match kind {
                        CallKind::Nested { target: callee, mask } => {
                            args.push(copy(tape));
                            finish_call(&mut builder, &primal_name(&callee, &mask), args, destination, next);
                        }
                        CallKind::Custom { derivative, result, residual, .. } => {
                            let pair_ty = Type::Tuple(vec![result.clone(), residual.clone()]);
                            let held = builder.call_value("paco_ad_hold", vec![int(self.types.layouts.size_of(&pair_ty) as i64)], ptr_ty());
                            let pair = Place::Deref { address: Box::new(copy(held)), ty: pair_ty };
                            builder.call(&derivative, args, Some(pair.clone()));
                            let first = take_field(&mut builder, pair.clone(), "0", &result, self.types.layouts.size_of(&result));
                            if let Some(place) = destination {
                                builder.assign(place, Rvalue::Use(Operand::Move(Place::Local(first))));
                            }
                            let second = Place::Field { base: Box::new(pair), field: "1".to_string() };
                            let size = self.types.layouts.size_of(&residual) as i64;
                            let pointer = reference(&mut builder, second, residual);
                            builder.call("paco_ad_push_bytes", vec![copy(tape), pointer, int(size)], None);
                            builder.call("paco_ad_unhold", vec![copy(held)], None);
                            builder.goto(next);
                        }
                        CallKind::TapeFree => {
                            let mut retire = vec![copy(tape)];
                            retire.extend(args);
                            finish_call(&mut builder, "paco_ad_tape_retire", retire, None, next);
                        }
                        _ => finish_call(&mut builder, &original_target, args, destination, next),
                    }
                    continue;
                }
                Terminator::Goto(target) => {
                    let to = target.0 as usize;
                    let next = target_of(block, to, &mut builder);
                    if self.preds[to].len() + usize::from(to == 0) > 1 {
                        trampolines.push((next, block, to));
                    }
                    builder.finish(Terminator::Goto(next));
                    continue;
                }
                Terminator::SwitchInt { discriminant, targets, otherwise } => {
                    let mut discriminant = discriminant.clone();
                    map_operand(&mut discriminant, &shift);
                    let mut mapped = Vec::new();
                    for (value, target) in targets.iter() {
                        let to = target.0 as usize;
                        let next = target_of(block, to, &mut builder);
                        if self.preds[to].len() + usize::from(to == 0) > 1 {
                            trampolines.push((next, block, to));
                        }
                        mapped.push((*value, next));
                    }
                    let to = otherwise.0 as usize;
                    let next = target_of(block, to, &mut builder);
                    if self.preds[to].len() + usize::from(to == 0) > 1 {
                        trampolines.push((next, block, to));
                    }
                    builder.finish(Terminator::SwitchInt { discriminant, targets: mapped, otherwise: next });
                    continue;
                }
                Terminator::CallIndirect { callee, args, destination, resume } => {
                    let mut callee = callee.clone();
                    map_operand(&mut callee, &shift);
                    let mut args = args.clone();
                    args.iter_mut().for_each(|arg| map_operand(arg, &shift));
                    let mut destination = destination.clone();
                    if let Some(place) = &mut destination {
                        map_place(place, &shift);
                    }
                    let to = resume.0 as usize;
                    let next = target_of(block, to, &mut builder);
                    if self.preds[to].len() + usize::from(to == 0) > 1 {
                        trampolines.push((next, block, to));
                    }
                    builder.finish(Terminator::CallIndirect { callee, args, destination, resume: next });
                    continue;
                }
                Terminator::Unreachable => {
                    builder.finish(Terminator::Unreachable);
                    continue;
                }
            }
        }
        trampolines.sort_by_key(|(block, ..)| block.0);
        trampolines.dedup_by_key(|(block, ..)| block.0);
        for (trampoline, from, to) in trampolines {
            builder.switch_to(trampoline);
            builder.call("paco_ad_push_i64", vec![copy(tape), int(from as i64)], None);
            builder.goto(map[to]);
        }
        for (block, &mapped) in map.iter().enumerate() {
            if !self.reachable[block] {
                builder.switch_to(mapped);
                builder.finish(Terminator::Unreachable);
            }
        }
        builder.into_body()
    }

    // ---- pullback --------------------------------------------------------------------------

    fn pullback(&mut self) -> Body {
        let mut pullback = Pullback::new(self);
        pullback.build(self);
        pullback.builder.into_body()
    }
}

fn finish_call(builder: &mut Builder, target: &str, args: Vec<Operand>, destination: Option<Place>, next: BasicBlockId) {
    builder.finish(Terminator::Call { target: crate::body::CallTarget(target.to_string()), args, destination, resume: next });
}

/// Moves field `field` out of the tuple at `tuple` without a clone: its
/// bits are copied into a fresh local, which the result then owns.
fn take_field(builder: &mut Builder, tuple: Place, field: &str, ty: &Type, size: u64) -> Local {
    let place = Place::Field { base: Box::new(tuple), field: field.to_string() };
    let copied = builder.local(ty.clone());
    raw_copy(builder, Place::Local(copied), place, size);
    builder.temp(ty.clone(), Rvalue::Use(Operand::Move(Place::Local(copied))))
}

/// Gives up ownership of `local` without dropping it: generated code
/// handed its bits to someone else. Moving it into held scratch memory
/// clears its drop flag; releasing the memory drops nothing.
fn forget(builder: &mut Builder, local: Local, ty: &Type, size: u64) {
    let held = builder.call_value("paco_ad_hold", vec![int(size as i64)], ptr_ty());
    builder.assign(Place::Deref { address: Box::new(copy(held)), ty: ty.clone() }, Rvalue::Use(Operand::Move(Place::Local(local))));
    builder.call("paco_ad_unhold", vec![copy(held)], None);
}

/// `target = source` bit for bit, with no clone and no drop.
fn raw_copy(builder: &mut Builder, target: Place, source: Place, size: u64) {
    let to = builder.temp(ptr_ty(), Rvalue::Ref { mutable: true, place: target });
    let from = builder.temp(ptr_ty(), Rvalue::Ref { mutable: false, place: source });
    builder.call("paco_ad_copy", vec![copy(to), copy(from), int(size as i64)], None);
}

fn successors(terminator: &Terminator) -> Vec<usize> {
    match terminator {
        Terminator::Goto(target) => vec![target.0 as usize],
        Terminator::SwitchInt { targets, otherwise, .. } => {
            let mut out: Vec<usize> = targets.iter().map(|(_, target)| target.0 as usize).collect();
            out.push(otherwise.0 as usize);
            out
        }
        Terminator::Call { resume, .. } | Terminator::CallIndirect { resume, .. } => vec![resume.0 as usize],
        Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
    }
}

fn emit_pushes(builder: &mut Builder, tape: Local, saves: &[(Place, Scalar)], shift: &dyn Fn(Local) -> Local) {
    for (place, scalar) in saves {
        let mut place = place.clone();
        map_place(&mut place, shift);
        push_scalar(builder, tape, place, scalar);
    }
}

fn push_scalar(builder: &mut Builder, tape: Local, place: Place, scalar: &Scalar) {
    let value = Operand::Copy(place.clone());
    match scalar {
        Scalar::F64 => builder.call("paco_ad_push_f64", vec![copy(tape), value], None),
        Scalar::F32 => builder.call("paco_ad_push_f32", vec![copy(tape), value], None),
        Scalar::Narrow(_) => {
            let wide = builder.temp(Type::Float(FloatWidth::F32), Rvalue::Cast { operand: value, target: Type::Float(FloatWidth::F32) });
            builder.call("paco_ad_push_f32", vec![copy(tape), copy(wide)], None);
        }
        Scalar::Int(width) => {
            let wide = if *width == IntWidth::I64 {
                value
            } else {
                copy(builder.temp(Type::Int(IntWidth::I64), Rvalue::Cast { operand: value, target: Type::Int(IntWidth::I64) }))
            };
            builder.call("paco_ad_push_i64", vec![copy(tape), wide], None);
        }
        Scalar::Bytes(size) => {
            let pointer = builder.temp(ptr_ty(), Rvalue::Ref { mutable: false, place });
            builder.call("paco_ad_push_bytes", vec![copy(tape), copy(pointer), int(*size as i64)], None);
        }
    }
}

fn pop_scalar(builder: &mut Builder, tape: Local, place: Place, scalar: &Scalar) {
    match scalar {
        Scalar::F64 => builder.call("paco_ad_pop_f64", vec![copy(tape)], Some(place)),
        Scalar::F32 => builder.call("paco_ad_pop_f32", vec![copy(tape)], Some(place)),
        Scalar::Narrow(width) => {
            let wide = builder.call_value("paco_ad_pop_f32", vec![copy(tape)], Type::Float(FloatWidth::F32));
            builder.assign(place, Rvalue::Cast { operand: copy(wide), target: Type::Float(*width) });
        }
        Scalar::Int(width) => {
            if *width == IntWidth::I64 {
                builder.call("paco_ad_pop_i64", vec![copy(tape)], Some(place));
            } else {
                let wide = builder.call_value("paco_ad_pop_i64", vec![copy(tape)], Type::Int(IntWidth::I64));
                builder.assign(place, Rvalue::Cast { operand: copy(wide), target: Type::Int(*width) });
            }
        }
        Scalar::Bytes(size) => {
            let pointer = builder.temp(ptr_ty(), Rvalue::Ref { mutable: true, place });
            builder.call("paco_ad_pop_bytes", vec![copy(tape), copy(pointer), int(*size as i64)], None);
        }
    }
}

fn ptr_ty() -> Type {
    Type::Borrow { mutable: true, ty: Box::new(Type::Int(IntWidth::I64)) }
}

/// A leaf of a differentiated parameter and the pointers to the caller's
/// adjoint of it (a second one, to the "is set" flag, for an opaque leaf).
type OutputSlot = (Leaf, Local, Option<Local>);

/// The pullback under construction: `Job`'s body replayed backwards.
struct Pullback {
    builder: Builder,
    tape: Local,
    /// Pullback local of each primal local.
    offset: u32,
    seeds: Vec<(Leaf, Local)>,
    /// Per differentiated parameter: its leaves and the pointers to the
    /// caller's adjoints of them.
    outputs: Vec<(usize, Vec<OutputSlot>)>,
    adjoints: HashMap<(Local, Path), Slot>,
    reversed: Vec<BasicBlockId>,
    exit: BasicBlockId,
}

impl Pullback {
    fn new(job: &Job<'_, '_, '_>) -> Self {
        let body = job.body;
        let i64 = Type::Int(IntWidth::I64);
        let mut params = vec![LocalDecl { name: Some(TAPE_LOCAL.to_string()), ty: i64, mutable: false }];
        let mut seeds = Vec::new();
        let unit = body.return_ty == Type::Unit;
        if !unit {
            for leaf in job.types.leaves(&body.return_ty) {
                if let LeafKind::Float(width) = leaf.kind {
                    params.push(LocalDecl { name: None, ty: Type::Float(width), mutable: true });
                    seeds.push((leaf, Local(params.len() as u32 - 1)));
                }
            }
        }
        let mut outputs = Vec::new();
        for (index, active) in job.mask.iter().enumerate() {
            if !active {
                continue;
            }
            let pointee = strip_borrow(&body.locals[index].ty).clone();
            let mut slots = Vec::new();
            for leaf in job.types.leaves(&pointee) {
                match &leaf.kind {
                    LeafKind::Float(width) => {
                        params.push(LocalDecl { name: None, ty: Type::Borrow { mutable: true, ty: Box::new(Type::Float(*width)) }, mutable: false });
                        slots.push((leaf.clone(), Local(params.len() as u32 - 1), None));
                    }
                    LeafKind::Opaque { tangent, .. } => {
                        params.push(LocalDecl { name: None, ty: Type::Borrow { mutable: true, ty: Box::new(tangent.clone()) }, mutable: false });
                        let value = Local(params.len() as u32 - 1);
                        params.push(LocalDecl { name: None, ty: Type::Borrow { mutable: true, ty: Box::new(Type::Bool) }, mutable: false });
                        slots.push((leaf.clone(), value, Some(Local(params.len() as u32 - 1))));
                    }
                }
            }
            outputs.push((index, slots));
        }
        let param_count = params.len();
        let offset = param_count as u32;
        let mut locals = params;
        for (index, decl) in body.locals.iter().enumerate() {
            let mut decl = decl.clone();
            if index < body.param_count
                && let Type::Borrow { ty, .. } = &decl.ty
            {
                decl.ty = (**ty).clone();
            }
            decl.mutable = true;
            locals.push(decl);
        }
        let mut builder = Builder::new(Body {
            locals,
            blocks: Vec::new(),
            profile: body.profile,
            param_count,
            return_ty: Type::Unit,
            span: body.span,
            spans: Vec::new(),
        });
        let entry = builder.block();
        let reversed = (0..body.blocks.len()).map(|_| builder.block()).collect();
        let exit = builder.block();
        let _ = entry;
        Pullback { builder, tape: Local(0), offset, seeds, outputs, adjoints: HashMap::new(), reversed, exit }
    }

    fn local(&self, local: Local) -> Local {
        Local(local.0 + self.offset)
    }

    /// The pullback place holding what `place` held in the primal, with
    /// borrows replaced by the places they point at.
    fn place(&self, job: &Job<'_, '_, '_>, place: &Place) -> Place {
        match place {
            Place::Local(local) => Place::Local(self.local(*local)),
            Place::Field { base, field } => Place::Field { base: Box::new(self.deref_base(job, base)), field: field.clone() },
            Place::VariantField { base, variant, index } => {
                Place::VariantField { base: Box::new(self.deref_base(job, base)), variant: variant.clone(), index: *index }
            }
            Place::Index { base, index } => Place::Index { base: Box::new(self.deref_base(job, base)), index: Box::new(self.operand(job, index)) },
            Place::Deref { address, .. } => match address.as_ref() {
                Operand::Copy(pointer) | Operand::Move(pointer) => self.pointee(job, pointer),
                Operand::Constant(_) => Place::Local(Local(u32::MAX)),
            },
        }
    }

    /// A projection base: a borrow-typed base is replaced by its target.
    fn deref_base(&self, job: &Job<'_, '_, '_>, base: &Place) -> Place {
        if matches!(job.ty(base), Type::Borrow { .. }) { self.pointee(job, base) } else { self.place(job, base) }
    }

    fn pointee(&self, job: &Job<'_, '_, '_>, pointer: &Place) -> Place {
        if let Place::Local(local) = pointer {
            if job.is_pointer_param(*local) {
                return Place::Local(self.local(*local));
            }
            if let Some(target) = job.targets.get(local) {
                return self.place(job, target);
            }
        }
        if let Type::Borrow { .. } = job.ty(pointer) {
            return self.deref_base(job, pointer);
        }
        self.place(job, pointer)
    }

    fn operand(&self, job: &Job<'_, '_, '_>, operand: &Operand) -> Operand {
        match operand {
            Operand::Copy(place) => Operand::Copy(self.place(job, place)),
            Operand::Move(place) => Operand::Copy(self.place(job, place)),
            Operand::Constant(constant) => Operand::Constant(constant.clone()),
        }
    }

    fn slot(&mut self, root: Local, path: &Path, kind: &LeafKind) -> Slot {
        if let Some(slot) = self.adjoints.get(&(root, path.clone())) {
            return *slot;
        }
        let slot = match kind {
            LeafKind::Float(width) => Slot::Float(self.builder.local(Type::Float(*width)), *width),
            LeafKind::Opaque { tangent, .. } => Slot::Opaque(self.builder.local(tangent.clone()), self.builder.local(Type::Bool)),
        };
        self.adjoints.insert((root, path.clone()), slot);
        slot
    }

    /// The adjoint of every leaf of the pullback place `place`, as places,
    /// with the leaves' paths relative to `place`. Heap elements resolve to
    /// their shadow slices.
    fn adjoint_places(&mut self, job: &Job<'_, '_, '_>, place: &Place) -> Vec<(Path, LeafKind, AdjPlace)> {
        let ty = self.pullback_type(job, place);
        let leaves = job.types.leaves(&ty);
        let (base, path) = self.split_heap(job, place);
        let mut out = Vec::new();
        for leaf in leaves {
            let mut full = path.clone();
            full.extend(leaf.path.iter().cloned());
            let adj = match &base {
                HeapBase::Local(root) => AdjPlace::Slot(self.slot(*root, &full, &leaf.kind)),
                HeapBase::Element(element) => {
                    if let LeafKind::Opaque { .. } = leaf.kind {
                        continue;
                    }
                    AdjPlace::Place(project(element.clone(), &full))
                }
            };
            out.push((leaf.path.clone(), leaf.kind.clone(), adj));
        }
        out
    }

    fn pullback_type(&self, job: &Job<'_, '_, '_>, place: &Place) -> Type {
        place_type(&self.builder.body, place, job.types)
    }

    /// Splits a pullback place at its last slice index: the adjoint lives in
    /// a local (no index) or in an element of the slice's shadow.
    fn split_heap(&mut self, job: &Job<'_, '_, '_>, place: &Place) -> (HeapBase, Path) {
        let mut path = Vec::new();
        let mut current = place.clone();
        loop {
            match current {
                Place::Local(local) => {
                    path.reverse();
                    return (HeapBase::Local(local), path);
                }
                Place::Field { base, field } => {
                    path.push(Proj::Field(field));
                    current = *base;
                }
                Place::VariantField { base, variant, index } => {
                    path.push(Proj::Variant(variant, index));
                    current = *base;
                }
                Place::Index { base, index } => {
                    path.reverse();
                    let slice_ty = self.pullback_type(job, &base);
                    let elem = match strip_borrow(&slice_ty) {
                        Type::Slice(elem) => (**elem).clone(),
                        _ => Type::Unknown,
                    };
                    let shadow = shadow_of(&mut self.builder, self.tape, *base, &elem, job.types);
                    return (HeapBase::Element(Place::Index { base: Box::new(Place::Local(shadow)), index }), path);
                }
                Place::Deref { .. } => {
                    path.reverse();
                    return (HeapBase::Local(Local(u32::MAX)), path);
                }
            }
        }
    }

    fn add(&mut self, target: &AdjPlace, width: FloatWidth, value: Operand) {
        let place = match target {
            AdjPlace::Slot(Slot::Float(local, _)) => Place::Local(*local),
            AdjPlace::Place(place) => place.clone(),
            AdjPlace::Slot(Slot::Opaque(..)) => return,
        };
        let sum = self.builder.binary(BinOp::Add, Operand::Copy(place.clone()), value, Type::Float(width));
        self.builder.assign(place, Rvalue::Use(copy(sum)));
    }

    /// Takes the adjoints of `place`'s leaves, leaving zeros.
    fn take(&mut self, job: &Job<'_, '_, '_>, place: &Place) -> Vec<(Path, Taken)> {
        let mut out = Vec::new();
        for (path, kind, adj) in self.adjoint_places(job, place) {
            match (kind, adj) {
                (LeafKind::Float(width), AdjPlace::Slot(Slot::Float(local, _))) => {
                    let value = self.builder.temp(Type::Float(width), Rvalue::Use(copy(local)));
                    self.builder.assign(Place::Local(local), Rvalue::Use(float(0.0, width)));
                    out.push((path, Taken::Float(value, width)));
                }
                (LeafKind::Float(width), AdjPlace::Place(place)) => {
                    let value = self.builder.temp(Type::Float(width), Rvalue::Use(Operand::Copy(place.clone())));
                    self.builder.assign(place, Rvalue::Use(float(0.0, width)));
                    out.push((path, Taken::Float(value, width)));
                }
                (LeafKind::Opaque { tangent, .. }, AdjPlace::Slot(Slot::Opaque(value, set))) => {
                    let taken = self.builder.local(tangent.clone());
                    let flag = self.builder.temp(Type::Bool, Rvalue::Use(copy(set)));
                    self.builder.when(copy(set), |builder| {
                        builder.assign(Place::Local(taken), Rvalue::Use(Operand::Move(Place::Local(value))));
                    });
                    self.builder.assign(Place::Local(set), Rvalue::Use(Operand::Constant(Constant::Bool(false))));
                    out.push((path, Taken::Opaque(taken, flag, tangent)));
                }
                _ => {}
            }
        }
        out
    }

    /// Adds a taken opaque adjoint into `target`'s opaque slot.
    fn add_opaque(&mut self, job: &mut Job<'_, '_, '_>, target: Slot, value: Local, flag: Local, tangent: &Type) {
        let Slot::Opaque(slot, set) = target else { return };
        let add = job.host.method(tangent, "add");
        let tangent = tangent.clone();
        self.builder.when(copy(flag), |builder| {
            builder.if_else(
                copy(set),
                |builder| {
                    let pointer = reference(builder, Place::Local(slot), tangent.clone());
                    let sum = builder.local(tangent.clone());
                    match &add {
                        Some(add) => builder.call(add, vec![pointer, Operand::Move(Place::Local(value))], Some(Place::Local(sum))),
                        None => builder.finish(Terminator::Unreachable),
                    }
                    if builder.is_open() {
                        builder.assign(Place::Local(slot), Rvalue::Use(Operand::Move(Place::Local(sum))));
                    } else {
                        let resume = builder.block();
                        builder.switch_to(resume);
                    }
                },
                |builder| {
                    builder.assign(Place::Local(slot), Rvalue::Use(Operand::Move(Place::Local(value))));
                    builder.assign(Place::Local(set), Rvalue::Use(Operand::Constant(Constant::Bool(true))));
                },
            );
        });
    }

    /// `adj(place.path) += value` for every taken leaf.
    fn accumulate(&mut self, job: &mut Job<'_, '_, '_>, place: &Place, taken: &[(Path, Taken)]) {
        let targets = self.adjoint_places(job, place);
        for (path, value) in taken {
            let Some((_, _, target)) = targets.iter().find(|(target_path, ..)| target_path == path) else { continue };
            match value {
                Taken::Float(local, width) => self.add(target, *width, copy(*local)),
                Taken::Opaque(local, flag, tangent) => {
                    if let AdjPlace::Slot(slot) = target {
                        self.add_opaque(job, *slot, *local, *flag, tangent);
                    }
                }
            }
        }
    }

    fn build(&mut self, job: &mut Job<'_, '_, '_>) {
        let body = job.body;
        for block in 0..body.blocks.len() {
            if !job.reachable[block] {
                self.builder.switch_to(self.reversed[block]);
                self.builder.finish(Terminator::Unreachable);
                continue;
            }
            self.builder.switch_to(self.reversed[block]);
            self.builder.span = body.terminator_span(block);
            self.reverse_terminator(job, block);
            for (index, statement) in body.blocks[block].statements.iter().enumerate().rev() {
                self.builder.span = body.statement_span(block, index);
                self.reverse_statement(job, statement);
            }
            self.builder.span = body.span;
            let next = self.next(job, block);
            self.continue_to(next);
        }
        let start = self.entry(job);
        self.exit(job);
        self.init(start);
    }

    fn next(&self, job: &Job<'_, '_, '_>, block: usize) -> Next {
        let mut preds: Vec<Option<usize>> = job.preds[block].iter().map(|pred| Some(*pred)).collect();
        if block == 0 {
            preds.push(None);
        }
        match preds.as_slice() {
            [None] => Next::Exit,
            [Some(pred)] => Next::Block(*pred),
            many => Next::Popped(many.iter().map(|pred| (pred.map_or(START, |pred| pred as i128), *pred)).collect()),
        }
    }

    fn continue_to(&mut self, next: Next) {
        match next {
            Next::Exit => self.builder.goto(self.exit),
            Next::Block(pred) => self.builder.goto(self.reversed[pred]),
            Next::Popped(preds) => {
                let id = self.builder.call_value("paco_ad_pop_i64", vec![copy(self.tape)], Type::Int(IntWidth::I64));
                let targets = preds.iter().map(|(value, pred)| (*value, pred.map_or(self.exit, |pred| self.reversed[pred]))).collect();
                let unreachable = self.builder.block();
                self.builder.finish(Terminator::SwitchInt { discriminant: copy(id), targets, otherwise: unreachable });
                self.builder.switch_to(unreachable);
                self.builder.finish(Terminator::Unreachable);
            }
        }
    }

    /// Restores the scalars the primal pushed for `places`, newest first.
    fn restore(&mut self, job: &Job<'_, '_, '_>, places: &[Place]) {
        let saves = job.save_list(places);
        for (place, scalar) in saves.iter().rev() {
            let target = self.place(job, place);
            pop_scalar(&mut self.builder, self.tape, target, scalar);
        }
    }

    fn reverse_statement(&mut self, job: &mut Job<'_, '_, '_>, statement: &Statement) {
        let Statement::Assign(place, rvalue) = statement else { return };
        let active = job.active_place(place);
        let target = self.place(job, place);
        let taken = if active { self.take(job, &target) } else { Vec::new() };
        let saves = job.saves_for_assign(place);
        self.restore(job, &saves);
        if active {
            self.propagate(job, rvalue, &taken);
        }
    }

    fn propagate(&mut self, job: &mut Job<'_, '_, '_>, rvalue: &Rvalue, taken: &[(Path, Taken)]) {
        match rvalue {
            Rvalue::Use(operand) => {
                if let Some(source) = operand_place(operand)
                    && job.active_place(source)
                {
                    let source = self.place(job, source);
                    self.accumulate(job, &source, taken);
                }
            }
            Rvalue::Aggregate { ty, variant, fields } => {
                for (index, field) in fields.iter().enumerate() {
                    let Some(source) = operand_place(field).filter(|source| job.active_place(source)) else { continue };
                    let head = match (ty, variant) {
                        (_, Some(variant)) => Proj::Variant(variant.clone(), index),
                        (Type::Struct(name, args), None) => {
                            let fields = job.types.layouts.struct_field_list(name, args);
                            Proj::Field(fields[index].0.clone())
                        }
                        _ => Proj::Field(index.to_string()),
                    };
                    let inner: Vec<(Path, Taken)> = taken
                        .iter()
                        .filter(|(path, _)| path.first() == Some(&head))
                        .map(|(path, value)| (path[1..].to_vec(), value.clone()))
                        .collect();
                    let source = self.place(job, source);
                    self.accumulate(job, &source, &inner);
                }
            }
            Rvalue::BinaryOp(op, left, right) => {
                let [(_, Taken::Float(t, width))] = taken else { return };
                let (t, width) = (*t, *width);
                let ty = Type::Float(width);
                let a = self.operand(job, left);
                let b = self.operand(job, right);
                let left_active = job.active_operand(left);
                let right_active = job.active_operand(right);
                match op {
                    BinOp::Add => {
                        if left_active {
                            self.add_to(job, left, width, copy(t));
                        }
                        if right_active {
                            self.add_to(job, right, width, copy(t));
                        }
                    }
                    BinOp::Sub => {
                        if left_active {
                            self.add_to(job, left, width, copy(t));
                        }
                        if right_active {
                            let negative = self.builder.temp(ty.clone(), Rvalue::UnaryOp(UnOp::Neg, copy(t)));
                            self.add_to(job, right, width, copy(negative));
                        }
                    }
                    BinOp::Mul => {
                        if left_active {
                            let product = self.builder.binary(BinOp::Mul, copy(t), b.clone(), ty.clone());
                            self.add_to(job, left, width, copy(product));
                        }
                        if right_active {
                            let product = self.builder.binary(BinOp::Mul, copy(t), a.clone(), ty.clone());
                            self.add_to(job, right, width, copy(product));
                        }
                    }
                    BinOp::Div => self.guarded(t, width, |this| {
                        let quotient = this.builder.binary(BinOp::Div, copy(t), b.clone(), ty.clone());
                        if left_active {
                            this.add_to(job, left, width, copy(quotient));
                        }
                        if right_active {
                            let scaled = this.builder.binary(BinOp::Mul, copy(quotient), a, ty.clone());
                            let over = this.builder.binary(BinOp::Div, copy(scaled), b, ty.clone());
                            let negative = this.builder.temp(ty, Rvalue::UnaryOp(UnOp::Neg, copy(over)));
                            this.add_to(job, right, width, copy(negative));
                        }
                    }),
                    _ => {}
                }
            }
            Rvalue::UnaryOp(UnOp::Neg, operand) => {
                let [(_, Taken::Float(t, width))] = taken else { return };
                if job.active_operand(operand) {
                    let negative = self.builder.temp(Type::Float(*width), Rvalue::UnaryOp(UnOp::Neg, copy(*t)));
                    self.add_to(job, operand, *width, copy(negative));
                }
            }
            Rvalue::Cast { operand, .. } => {
                let [(_, Taken::Float(t, _))] = taken else { return };
                if !job.active_operand(operand) {
                    return;
                }
                let source = job.ty(operand_place(operand).expect("an active operand is a place"));
                if let Type::Float(width) = source {
                    let narrowed = self.builder.temp(Type::Float(width), Rvalue::Cast { operand: copy(*t), target: Type::Float(width) });
                    self.add_to(job, operand, width, copy(narrowed));
                }
            }
            Rvalue::Math(op, args) => {
                let [(_, Taken::Float(t, width))] = taken else { return };
                let (t, width) = (*t, *width);
                if matches!(op, MathOp::Sqrt | MathOp::Ln | MathOp::Powf) {
                    self.guarded(t, width, |this| this.math(job, *op, args, t, width));
                } else {
                    self.math(job, *op, args, t, width);
                }
            }
            _ => {}
        }
    }

    /// Runs `rule` only for a nonzero adjoint `t`: a rule whose partial
    /// derivative is infinite at a point the primal only passed through
    /// (`sqrt(0)` on an unused branch) then contributes zero, not NaN.
    fn guarded(&mut self, t: Local, width: FloatWidth, rule: impl FnOnce(&mut Self)) {
        let nonzero = self.builder.binary(BinOp::Ne, copy(t), float(0.0, width), Type::Bool);
        let yes = self.builder.block();
        let join = self.builder.block();
        self.builder.finish(Terminator::SwitchInt { discriminant: copy(nonzero), targets: vec![(1, yes)], otherwise: join });
        self.builder.switch_to(yes);
        rule(self);
        self.builder.goto(join);
        self.builder.switch_to(join);
    }

    fn add_to(&mut self, job: &mut Job<'_, '_, '_>, operand: &Operand, width: FloatWidth, value: Operand) {
        let Some(place) = operand_place(operand) else { return };
        let place = self.place(job, place);
        let targets = self.adjoint_places(job, &place);
        if let Some((_, _, target)) = targets.iter().find(|(path, ..)| path.is_empty()) {
            self.add(target, width, value);
        }
    }

    fn math(&mut self, job: &mut Job<'_, '_, '_>, op: MathOp, args: &[Operand], t: Local, width: FloatWidth) {
        let ty = Type::Float(width);
        let x = self.operand(job, &args[0]);
        let y = args.get(1).map(|arg| self.operand(job, arg));
        let x_active = job.active_operand(&args[0]);
        let y_active = args.get(1).is_some_and(|arg| job.active_operand(arg));
        let one = float(1.0, width);
        let zero = float(0.0, width);
        let apply = |builder: &mut Builder, op: MathOp, args: Vec<Operand>| builder.temp(ty.clone(), Rvalue::Math(op, args));
        match op {
            MathOp::Sqrt | MathOp::Exp | MathOp::Ln | MathOp::Sin | MathOp::Cos | MathOp::Tanh if x_active => {
                let derivative = match op {
                    MathOp::Sqrt => {
                        let root = apply(&mut self.builder, MathOp::Sqrt, vec![x.clone()]);
                        let twice = self.builder.binary(BinOp::Add, copy(root), copy(root), ty.clone());
                        self.builder.binary(BinOp::Div, one, copy(twice), ty.clone())
                    }
                    MathOp::Exp => apply(&mut self.builder, MathOp::Exp, vec![x.clone()]),
                    MathOp::Ln => self.builder.binary(BinOp::Div, one, x.clone(), ty.clone()),
                    MathOp::Sin => apply(&mut self.builder, MathOp::Cos, vec![x.clone()]),
                    MathOp::Cos => {
                        let sine = apply(&mut self.builder, MathOp::Sin, vec![x.clone()]);
                        self.builder.temp(ty.clone(), Rvalue::UnaryOp(UnOp::Neg, copy(sine)))
                    }
                    _ => {
                        let tanh = apply(&mut self.builder, MathOp::Tanh, vec![x.clone()]);
                        let square = self.builder.binary(BinOp::Mul, copy(tanh), copy(tanh), ty.clone());
                        self.builder.binary(BinOp::Sub, one, copy(square), ty.clone())
                    }
                };
                let product = self.builder.binary(BinOp::Mul, copy(t), copy(derivative), ty.clone());
                self.add_to(job, &args[0], width, copy(product));
            }
            MathOp::Abs if x_active => {
                let target = self.slot_place(job, &args[0]);
                let positive = self.builder.binary(BinOp::Gt, x.clone(), zero.clone(), Type::Bool);
                let negative = self.builder.binary(BinOp::Lt, x, zero, Type::Bool);
                if let Some(target) = target {
                    let add_positive = target.clone();
                    self.builder.when(copy(positive), |builder| {
                        let sum = builder.binary(BinOp::Add, Operand::Copy(add_positive.clone()), copy(t), Type::Float(width));
                        builder.assign(add_positive, Rvalue::Use(copy(sum)));
                    });
                    self.builder.when(copy(negative), |builder| {
                        let sum = builder.binary(BinOp::Sub, Operand::Copy(target.clone()), copy(t), Type::Float(width));
                        builder.assign(target, Rvalue::Use(copy(sum)));
                    });
                }
            }
            MathOp::Min | MathOp::Max => {
                let y = y.expect("min and max take two operands");
                let comparison = if op == MathOp::Min { BinOp::Le } else { BinOp::Ge };
                let first = self.builder.binary(comparison, x, y, Type::Bool);
                let left = if x_active { self.slot_place(job, &args[0]) } else { None };
                let right = if y_active { self.slot_place(job, &args[1]) } else { None };
                self.builder.if_else(
                    copy(first),
                    |builder| {
                        if let Some(left) = left {
                            let sum = builder.binary(BinOp::Add, Operand::Copy(left.clone()), copy(t), Type::Float(width));
                            builder.assign(left, Rvalue::Use(copy(sum)));
                        }
                    },
                    |builder| {
                        if let Some(right) = right {
                            let sum = builder.binary(BinOp::Add, Operand::Copy(right.clone()), copy(t), Type::Float(width));
                            builder.assign(right, Rvalue::Use(copy(sum)));
                        }
                    },
                );
            }
            MathOp::Powf => {
                let y = y.expect("powf takes two operands");
                if x_active {
                    let lowered = self.builder.binary(BinOp::Sub, y.clone(), one.clone(), ty.clone());
                    let power = apply(&mut self.builder, MathOp::Powf, vec![x.clone(), copy(lowered)]);
                    let scaled = self.builder.binary(BinOp::Mul, y.clone(), copy(power), ty.clone());
                    let product = self.builder.binary(BinOp::Mul, copy(t), copy(scaled), ty.clone());
                    self.add_to(job, &args[0], width, copy(product));
                }
                if y_active && let Some(target) = self.slot_place(job, &args[1]) {
                    let positive = self.builder.binary(BinOp::Gt, x.clone(), zero, Type::Bool);
                    self.builder.when(copy(positive), |builder| {
                        let power = builder.temp(Type::Float(width), Rvalue::Math(MathOp::Powf, vec![x.clone(), y.clone()]));
                        let log = builder.temp(Type::Float(width), Rvalue::Math(MathOp::Ln, vec![x.clone()]));
                        let scaled = builder.binary(BinOp::Mul, copy(power), copy(log), Type::Float(width));
                        let product = builder.binary(BinOp::Mul, copy(t), copy(scaled), Type::Float(width));
                        let sum = builder.binary(BinOp::Add, Operand::Copy(target.clone()), copy(product), Type::Float(width));
                        builder.assign(target, Rvalue::Use(copy(sum)));
                    });
                }
            }
            _ => {}
        }
    }

    /// The adjoint place of a scalar operand.
    fn slot_place(&mut self, job: &Job<'_, '_, '_>, operand: &Operand) -> Option<Place> {
        let place = self.place(job, operand_place(operand)?);
        let targets = self.adjoint_places(job, &place);
        match targets.into_iter().find(|(path, ..)| path.is_empty())?.2 {
            AdjPlace::Slot(Slot::Float(local, _)) => Some(Place::Local(local)),
            AdjPlace::Place(place) => Some(place),
            AdjPlace::Slot(Slot::Opaque(..)) => None,
        }
    }

    fn reverse_terminator(&mut self, job: &mut Job<'_, '_, '_>, block: usize) {
        let Terminator::Call { target, args, destination, .. } = &job.body.blocks[block].terminator else { return };
        let kind = job.calls.get(&block).cloned().unwrap_or(CallKind::Plain);
        let dest_target = destination.as_ref().map(|place| self.place(job, place));
        let taken = match (&dest_target, destination) {
            (Some(target), Some(place)) if job.active_place(place) => self.take(job, target),
            _ => Vec::new(),
        };
        let saves = job.saves_for_call(&target.0, args, destination.as_ref());
        match kind {
            CallKind::Plain | CallKind::TapeFree => {}
            CallKind::PushFloat(width) => {
                let popped = self.builder.call_value(
                    if width == FloatWidth::F64 { "paco_ad_adj_pop_f64" } else { "paco_ad_adj_pop_f32" },
                    vec![self.operand(job, &args[0])],
                    Type::Float(width),
                );
                let value_width = match job.ty(operand_place(&args[1]).expect("pushed value is a place")) {
                    Type::Float(value_width) => value_width,
                    _ => width,
                };
                self.add_to(job, &args[1], value_width, copy(popped));
            }
            CallKind::PopFloat(width) => {
                let value = match taken.as_slice() {
                    [(_, Taken::Float(value, _))] => copy(*value),
                    _ => float(0.0, width),
                };
                let tape = self.operand(job, &args[0]);
                self.builder.call(if width == FloatWidth::F64 { "paco_ad_adj_push_f64" } else { "paco_ad_adj_push_f32" }, vec![tape, value], None);
            }
            CallKind::Nested { target: callee, mask } => self.reverse_nested(job, &callee, &mask, args, &taken),
            CallKind::Custom { pullback, residual, gradients, params, result, .. } => {
                self.reverse_custom(job, &pullback, &residual, &gradients, &params, &result, args, dest_target.as_ref(), &taken)
            }
        }
        self.restore(job, &saves);
    }

    fn reverse_nested(&mut self, job: &mut Job<'_, '_, '_>, callee: &str, mask: &[bool], args: &[Operand], taken: &[(Path, Taken)]) {
        let Some(&index) = job.callees.index.get(callee) else { return };
        let body = &job.callees.bodies[index].1;
        let mut call_args = vec![copy(self.tape)];
        if body.return_ty != Type::Unit {
            for leaf in job.types.leaves(&body.return_ty) {
                if let LeafKind::Float(width) = leaf.kind {
                    let value = taken
                        .iter()
                        .find_map(|(path, value)| match value {
                            Taken::Float(local, _) if *path == leaf.path => Some(copy(*local)),
                            _ => None,
                        })
                        .unwrap_or_else(|| float(0.0, width));
                    call_args.push(value);
                }
            }
        }
        let params = param_types(body);
        for (index, active) in mask.iter().enumerate() {
            if !active {
                continue;
            }
            let Some(place) = operand_place(&args[index]) else { continue };
            let pointee = match (&params[index], job.ty(place)) {
                (Type::Borrow { ty, .. }, Type::Borrow { .. }) => Place::Deref { address: Box::new(Operand::Copy(place.clone())), ty: (**ty).clone() },
                _ => place.clone(),
            };
            let target = self.place(job, &pointee);
            let pointee_ty = strip_borrow(&params[index]).clone();
            let targets = self.adjoint_places(job, &target);
            for leaf in job.types.leaves(&pointee_ty) {
                let found = targets.iter().find(|(path, ..)| *path == leaf.path).map(|(_, _, adj)| adj.clone());
                match (&leaf.kind, found) {
                    (LeafKind::Float(width), Some(AdjPlace::Slot(Slot::Float(local, _)))) => {
                        call_args.push(reference(&mut self.builder, Place::Local(local), Type::Float(*width)));
                    }
                    (LeafKind::Float(width), Some(AdjPlace::Place(place))) => {
                        call_args.push(reference(&mut self.builder, place, Type::Float(*width)));
                    }
                    (LeafKind::Float(width), _) => {
                        let scratch = self.builder.temp(Type::Float(*width), Rvalue::Use(float(0.0, *width)));
                        call_args.push(reference(&mut self.builder, Place::Local(scratch), Type::Float(*width)));
                    }
                    (LeafKind::Opaque { tangent, .. }, Some(AdjPlace::Slot(Slot::Opaque(value, set)))) => {
                        call_args.push(reference(&mut self.builder, Place::Local(value), tangent.clone()));
                        call_args.push(reference(&mut self.builder, Place::Local(set), Type::Bool));
                    }
                    (LeafKind::Opaque { tangent, .. }, _) => {
                        let value = self.builder.local(tangent.clone());
                        let set = self.builder.temp(Type::Bool, Rvalue::Use(Operand::Constant(Constant::Bool(false))));
                        call_args.push(reference(&mut self.builder, Place::Local(value), tangent.clone()));
                        call_args.push(reference(&mut self.builder, Place::Local(set), Type::Bool));
                    }
                }
            }
        }
        self.builder.call(&pullback_name(callee, mask), call_args, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn reverse_custom(
        &mut self,
        job: &mut Job<'_, '_, '_>,
        pullback: &str,
        residual: &Type,
        gradients: &Type,
        params: &[Type],
        result: &Type,
        args: &[Operand],
        dest: Option<&Place>,
        taken: &[(Path, Taken)],
    ) {
        let saved = self.builder.local(residual.clone());
        let size = job.types.layouts.size_of(residual) as i64;
        let pointer = reference(&mut self.builder, Place::Local(saved), residual.clone());
        self.builder.call("paco_ad_pop_bytes", vec![copy(self.tape), pointer, int(size)], None);
        let seed = match (result, dest) {
            (Type::Float(width), _) => match taken {
                [(_, Taken::Float(value, _))] => copy(*value),
                _ => float(0.0, *width),
            },
            (other, Some(dest)) => Operand::Move(Place::Local(self.tangent_of(job, dest, other, taken))),
            (other, None) => {
                let seed = self.builder.temp(other.clone(), Rvalue::Use(Operand::Constant(Constant::Unit)));
                copy(seed)
            }
        };
        let grads = self.builder.call_value(pullback, vec![Operand::Move(Place::Local(saved)), seed], gradients.clone());
        let differentiable: Vec<usize> = params.iter().enumerate().filter(|(_, ty)| job.types.can_be_active(ty)).map(|(index, _)| index).collect();
        for (position, &index) in differentiable.iter().enumerate() {
            let Some(place) = operand_place(&args[index]).filter(|place| job.active_place(place)) else { continue };
            let gradient = if differentiable.len() == 1 {
                Place::Local(grads)
            } else {
                Place::Field { base: Box::new(Place::Local(grads)), field: position.to_string() }
            };
            let pointee = match (&params[index], job.ty(place)) {
                (Type::Borrow { ty, .. }, Type::Borrow { .. }) => Place::Deref { address: Box::new(Operand::Copy(place.clone())), ty: (**ty).clone() },
                _ => place.clone(),
            };
            let target = self.place(job, &pointee);
            let param_ty = strip_borrow(&params[index]).clone();
            self.scatter(job, &target, gradient, &param_ty);
        }
        self.builder.push(Statement::Drop(Place::Local(grads)));
    }

    /// The adjoint of the value at the pullback place `place` (of type `ty`)
    /// as a `ty::Tangent` value: the taken leaves written into a copy of the
    /// value, with its slices' shadows moved in; or the opaque adjoint, or
    /// `zero_tangent` when there is none.
    fn tangent_of(&mut self, job: &mut Job<'_, '_, '_>, place: &Place, ty: &Type, taken: &[(Path, Taken)]) -> Local {
        if let Some(tangent) = job.types.opaque_tangent(ty) {
            let result = self.builder.local(tangent.clone());
            let zero = job.host.method(ty, "zero_tangent");
            let (value, flag) = match taken {
                [(_, Taken::Opaque(value, flag, _))] => (Some(*value), Some(*flag)),
                _ => (None, None),
            };
            let pointer = reference(&mut self.builder, place.clone(), ty.clone());
            let make_zero = |builder: &mut Builder| match &zero {
                Some(zero) => builder.call(zero, vec![pointer.clone()], Some(Place::Local(result))),
                None => {
                    builder.finish(Terminator::Unreachable);
                    let resume = builder.block();
                    builder.switch_to(resume);
                }
            };
            match (value, flag) {
                (Some(value), Some(flag)) => self.builder.if_else(
                    copy(flag),
                    |builder| builder.assign(Place::Local(result), Rvalue::Use(Operand::Move(Place::Local(value)))),
                    make_zero,
                ),
                _ => make_zero(&mut self.builder),
            }
            return result;
        }
        let result = self.builder.temp(ty.clone(), Rvalue::Use(Operand::Copy(place.clone())));
        for leaf in job.types.leaves(ty) {
            let LeafKind::Float(width) = leaf.kind else { continue };
            let value = taken
                .iter()
                .find_map(|(path, value)| match value {
                    Taken::Float(local, _) if *path == leaf.path => Some(copy(*local)),
                    _ => None,
                })
                .unwrap_or_else(|| float(0.0, width));
            self.builder.assign(project(Place::Local(result), &leaf.path), Rvalue::Use(value));
        }
        for (path, elem) in job.types.heap_paths(ty) {
            let shadow = shadow_of(&mut self.builder, self.tape, project(place.clone(), &path), &elem, job.types);
            let target = project(Place::Local(result), &path);
            let len = self.builder.temp(Type::Int(IntWidth::I64), Rvalue::SliceLen(Place::Local(shadow)));
            let leaves = job.types.leaves(&elem);
            self.builder.for_each(copy(len), |builder, index| {
                let element = Place::Index { base: Box::new(Place::Local(shadow)), index: Box::new(copy(index)) };
                builder.assign(
                    Place::Index { base: Box::new(target.clone()), index: Box::new(copy(index)) },
                    Rvalue::Use(Operand::Copy(element.clone())),
                );
                for leaf in &leaves {
                    if let LeafKind::Float(width) = leaf.kind {
                        builder.assign(project(element.clone(), &leaf.path), Rvalue::Use(float(0.0, width)));
                    }
                }
            });
        }
        result
    }

    /// Adds the gradient value at `gradient` (of `ty`'s tangent) into the
    /// adjoints of the pullback place `target`.
    fn scatter(&mut self, job: &mut Job<'_, '_, '_>, target: &Place, gradient: Place, ty: &Type) {
        let targets = self.adjoint_places(job, target);
        for (path, kind, adj) in targets {
            match kind {
                LeafKind::Float(width) => self.add(&adj, width, Operand::Copy(project(gradient.clone(), &path))),
                LeafKind::Opaque { tangent, .. } => {
                    let value = self.builder.temp(tangent.clone(), Rvalue::Use(Operand::Copy(project(gradient.clone(), &path))));
                    let flag = self.builder.temp(Type::Bool, Rvalue::Use(Operand::Constant(Constant::Bool(true))));
                    if let AdjPlace::Slot(slot) = adj {
                        self.add_opaque(job, slot, value, flag, &tangent);
                    }
                }
            }
        }
        for (path, elem) in job.types.heap_paths(ty) {
            let shadow = shadow_of(&mut self.builder, self.tape, project(target.clone(), &path), &elem, job.types);
            let source = project(gradient.clone(), &path);
            let len = self.builder.temp(Type::Int(IntWidth::I64), Rvalue::SliceLen(Place::Local(shadow)));
            let leaves = job.types.leaves(&elem);
            self.builder.for_each(copy(len), |builder, index| {
                for leaf in &leaves {
                    let LeafKind::Float(width) = leaf.kind else { continue };
                    let into = project(Place::Index { base: Box::new(Place::Local(shadow)), index: Box::new(copy(index)) }, &leaf.path);
                    let from = project(Place::Index { base: Box::new(source.clone()), index: Box::new(copy(index)) }, &leaf.path);
                    let sum = builder.binary(BinOp::Add, Operand::Copy(into.clone()), Operand::Copy(from), Type::Float(width));
                    builder.assign(into, Rvalue::Use(copy(sum)));
                }
            });
        }
    }

    /// The first pullback block after the zeroing of adjoints: pops the
    /// primal's final state and seeds the result's adjoints.
    fn entry(&mut self, job: &mut Job<'_, '_, '_>) -> BasicBlockId {
        let body = job.body;
        let start = self.builder.block();
        self.builder.switch_to(start);
        self.builder.span = body.span;
        let returned = if job.returns.len() > 1 {
            Some(self.builder.call_value("paco_ad_pop_i64", vec![copy(self.tape)], Type::Int(IntWidth::I64)))
        } else {
            None
        };
        for (place, scalar) in job.snapshot().iter().rev() {
            let target = self.place(job, place);
            pop_scalar(&mut self.builder, self.tape, target, scalar);
        }
        for (index, slots) in self.outputs.clone() {
            if !matches!(body.locals[index].ty, Type::Borrow { mutable: true, .. }) {
                continue;
            }
            let place = Place::Local(self.local(Local(index as u32)));
            let targets = self.adjoint_places(job, &place);
            for (leaf, pointer, _) in &slots {
                if let LeafKind::Float(width) = leaf.kind
                    && let Some((_, _, adj)) = targets.iter().find(|(path, ..)| *path == leaf.path)
                {
                    let incoming = Place::Deref { address: Box::new(copy(*pointer)), ty: Type::Float(width) };
                    self.add(adj, width, Operand::Copy(incoming.clone()));
                    self.builder.assign(incoming, Rvalue::Use(float(0.0, width)));
                }
            }
        }
        match job.returns.clone().as_slice() {
            [] => self.builder.finish(Terminator::Unreachable),
            [single] => {
                self.seed_return(job, *single);
                self.builder.goto(self.reversed[*single]);
            }
            many => {
                let targets: Vec<(i128, BasicBlockId)> = many.iter().map(|&block| (block as i128, self.builder.block())).collect();
                let unreachable = self.builder.block();
                self.builder.finish(Terminator::SwitchInt {
                    discriminant: copy(returned.expect("several returns pop their id")),
                    targets: targets.clone(),
                    otherwise: unreachable,
                });
                for (block, seed_block) in targets {
                    self.builder.switch_to(seed_block);
                    self.seed_return(job, block as usize);
                    self.builder.goto(self.reversed[block as usize]);
                }
                self.builder.switch_to(unreachable);
                self.builder.finish(Terminator::Unreachable);
            }
        }
        start
    }

    fn seed_return(&mut self, job: &mut Job<'_, '_, '_>, block: usize) {
        let Terminator::Return(operand) = &job.body.blocks[block].terminator else { return };
        let Some(place) = operand_place(operand) else { return };
        let target = self.place(job, place);
        let targets = self.adjoint_places(job, &target);
        for (leaf, seed) in self.seeds.clone() {
            if let (LeafKind::Float(width), Some((_, _, adj))) = (&leaf.kind, targets.iter().find(|(path, ..)| *path == leaf.path)) {
                self.add(adj, *width, copy(seed));
            }
        }
    }

    /// Block 0: zeroes every adjoint slot, then continues at `start`.
    fn init(&mut self, start: BasicBlockId) {
        self.builder.switch_to(BasicBlockId(0));
        let mut slots: Vec<Slot> = self.adjoints.values().copied().collect();
        slots.sort_by_key(|slot| match slot {
            Slot::Float(local, _) | Slot::Opaque(local, _) => local.0,
        });
        for slot in slots {
            match slot {
                Slot::Float(local, width) => self.builder.assign(Place::Local(local), Rvalue::Use(float(0.0, width))),
                Slot::Opaque(_, set) => self.builder.assign(Place::Local(set), Rvalue::Use(Operand::Constant(Constant::Bool(false)))),
            }
        }
        self.builder.goto(start);
    }

    fn exit(&mut self, job: &mut Job<'_, '_, '_>) {
        self.builder.switch_to(self.exit);
        self.builder.span = job.body.span;
        for (index, slots) in self.outputs.clone() {
            let place = Place::Local(self.local(Local(index as u32)));
            let targets = self.adjoint_places(job, &place);
            for (leaf, pointer, set_pointer) in &slots {
                let Some((_, _, adj)) = targets.iter().find(|(path, ..)| *path == leaf.path).cloned() else { continue };
                match (&leaf.kind, adj, set_pointer) {
                    (LeafKind::Float(width), AdjPlace::Slot(Slot::Float(local, _)), _) => {
                        let outgoing = Place::Deref { address: Box::new(copy(*pointer)), ty: Type::Float(*width) };
                        let sum = self.builder.binary(BinOp::Add, Operand::Copy(outgoing.clone()), copy(local), Type::Float(*width));
                        self.builder.assign(outgoing, Rvalue::Use(copy(sum)));
                    }
                    (LeafKind::Opaque { tangent, .. }, AdjPlace::Slot(Slot::Opaque(value, set)), Some(set_pointer)) => {
                        let add = job.host.method(tangent, "add");
                        let size = job.types.layouts.size_of(tangent);
                        give_opaque(&mut self.builder, (*pointer, *set_pointer), (value, set), tangent, add, size);
                    }
                    _ => {}
                }
            }
        }
        for slot in self.adjoints.values().copied().collect::<Vec<_>>() {
            if let Slot::Opaque(value, set) = slot {
                let ty = self.builder.ty(value).clone();
                self.builder.when(copy(set), |builder| {
                    let owned = builder.temp(ty, Rvalue::Use(Operand::Move(Place::Local(value))));
                    builder.push(Statement::Drop(Place::Local(owned)));
                });
            }
        }
        self.builder.finish(Terminator::Return(Operand::Constant(Constant::Unit)));
    }
}

/// Adds the opaque adjoint `(value, set)` into the caller's slot behind
/// `(pointer, set_pointer)`, handing over ownership of `value`.
/// `caller` and `slot` are `(value, is set)` pairs: pointers into the
/// caller for the first, locals for the second.
fn give_opaque(builder: &mut Builder, caller: (Local, Local), slot: (Local, Local), tangent: &Type, add: Option<String>, size: u64) {
    let (pointer, set_pointer) = caller;
    let (value, set) = slot;
    let caller_value = Place::Deref { address: Box::new(copy(pointer)), ty: tangent.clone() };
    let caller_set = Place::Deref { address: Box::new(copy(set_pointer)), ty: Type::Bool };
    let tangent = tangent.clone();
    builder.when(copy(set), |builder| {
        builder.if_else(
            Operand::Copy(caller_set.clone()),
            |builder| {
                let Some(add) = &add else {
                    builder.finish(Terminator::Unreachable);
                    let resume = builder.block();
                    builder.switch_to(resume);
                    return;
                };
                let sum = builder.local(tangent.clone());
                builder.call(add, vec![copy(pointer), Operand::Move(Place::Local(value))], Some(Place::Local(sum)));
                builder.assign(caller_value.clone(), Rvalue::Use(Operand::Move(Place::Local(sum))));
            },
            |builder| {
                raw_copy(builder, caller_value.clone(), Place::Local(value), size);
                builder.assign(caller_set.clone(), Rvalue::Use(Operand::Constant(Constant::Bool(true))));
                forget(builder, value, &tangent, size);
            },
        );
        builder.assign(Place::Local(set), Rvalue::Use(Operand::Constant(Constant::Bool(false))));
    });
}

#[derive(Clone, Debug)]
enum AdjPlace {
    Slot(Slot),
    Place(Place),
}

enum HeapBase {
    Local(Local),
    Element(Place),
}

#[derive(Clone, Debug)]
enum Taken {
    Float(Local, FloatWidth),
    Opaque(Local, Local, Type),
}

