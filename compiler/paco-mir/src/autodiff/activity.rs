//! Activity analysis: a local is *varied* when it depends on a
//! differentiated input, *useful* when a result depends on it, and *active*
//! when both. Only active values get adjoints, saved values or diagnostics.
//! The analysis is flow-insensitive over the locals of one body; calls use
//! per-callee summaries of which parameters reach which outputs, computed
//! bottom-up to a fixed point so recursion converges.

use std::collections::{HashMap, HashSet};

use paco_types::Type;

use crate::body::{Body, Local, Operand, Place, Rvalue, Statement, Terminator};

use super::types::Types;

/// Runtime entry points that move values through a tape: every value pushed
/// may come back from any pop of the same kind.
pub fn is_tape_call(target: &str) -> bool {
    target.starts_with("paco_ad_") && !matches!(target, "paco_ad_tape_new" | "paco_ad_tape_free" | "paco_ad_tape_retire")
}

/// Which parameters flow into a body's outputs: its result and the final
/// value behind each `&mut` parameter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Summary {
    pub result: HashSet<usize>,
    pub params: HashMap<usize, HashSet<usize>>,
}

/// The place a borrow-typed local points at, when every assignment to it
/// names the same place.
pub fn ref_targets(body: &Body) -> HashMap<Local, Place> {
    let mut targets: HashMap<Local, Option<Place>> = HashMap::new();
    for block in &body.blocks {
        for statement in &block.statements {
            let Statement::Assign(Place::Local(local), rvalue) = statement else { continue };
            if (local.0 as usize) < body.param_count || !matches!(body.locals[local.0 as usize].ty, Type::Borrow { .. }) {
                continue;
            }
            let target = match rvalue {
                Rvalue::Ref { place, .. } if place_is_static(place) => Some(place.clone()),
                Rvalue::Use(Operand::Copy(Place::Local(other)) | Operand::Move(Place::Local(other)))
                    if matches!(body.locals[other.0 as usize].ty, Type::Borrow { .. }) =>
                {
                    Some(Place::Local(*other))
                }
                _ => None,
            };
            let entry = targets.entry(*local).or_insert_with(|| target.clone());
            if *entry != target {
                *entry = None;
            }
        }
        if let Terminator::Call { destination: Some(Place::Local(local)), .. } | Terminator::CallIndirect { destination: Some(Place::Local(local)), .. } =
            &block.terminator
            && matches!(body.locals[local.0 as usize].ty, Type::Borrow { .. })
        {
            targets.insert(*local, None);
        }
    }
    targets.into_iter().filter_map(|(local, target)| Some((local, target?))).collect()
}

/// A place whose address cannot change between the borrow and its uses:
/// no index operand but constants and locals assigned once.
fn place_is_static(place: &Place) -> bool {
    match place {
        Place::Local(_) => true,
        Place::Field { base, .. } | Place::VariantField { base, .. } => place_is_static(base),
        Place::Index { base, index } => place_is_static(base) && matches!(index.as_ref(), Operand::Constant(_)),
        Place::Deref { .. } => false,
    }
}

/// The local a place's value lives in, following borrows to their targets.
pub fn root(place: &Place, targets: &HashMap<Local, Place>) -> Local {
    let mut place = place.clone();
    for _ in 0..64 {
        let local = base_local(&place);
        match local.and_then(|local| targets.get(&local)) {
            Some(target) => place = target.clone(),
            None => return local.unwrap_or(Local(u32::MAX)),
        }
    }
    base_local(&place).unwrap_or(Local(u32::MAX))
}

pub fn base_local(place: &Place) -> Option<Local> {
    match place {
        Place::Local(local) => Some(*local),
        Place::Field { base, .. } | Place::VariantField { base, .. } | Place::Index { base, .. } => base_local(base),
        Place::Deref { address, .. } => match address.as_ref() {
            Operand::Copy(place) | Operand::Move(place) => base_local(place),
            Operand::Constant(_) => None,
        },
    }
}

pub fn operand_place(operand: &Operand) -> Option<&Place> {
    match operand {
        Operand::Copy(place) | Operand::Move(place) => Some(place),
        Operand::Constant(_) => None,
    }
}

pub fn rvalue_operands(rvalue: &Rvalue) -> Vec<&Operand> {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::UnaryOp(_, operand) | Rvalue::Cast { operand, .. } => vec![operand],
        Rvalue::BinaryOp(_, left, right) => vec![left, right],
        Rvalue::Aggregate { fields, .. } | Rvalue::Math(_, fields) => fields.iter().collect(),
        Rvalue::Load { address, .. } => vec![address],
        _ => Vec::new(),
    }
}

/// Name of the locals holding a tape handle: values pushed through one of
/// them flow to whatever is popped from it.
pub const TAPE_LOCAL: &str = "$tape";

pub fn is_tape_local(body: &Body, local: Local) -> bool {
    body.locals.get(local.0 as usize).is_some_and(|decl| decl.name.as_deref() == Some(TAPE_LOCAL))
}

/// The flow graph of one body: an edge `a -> b` when a derivative can flow
/// from local `a` into local `b`.
pub struct Graph {
    pub edges: Vec<(usize, usize)>,
    pub sinks: Vec<usize>,
    pub locals: usize,
}

/// Parameter types of a call target, and its summary when it has a body.
pub trait Callees {
    fn params(&self, target: &str) -> Option<Vec<Type>>;
    fn summary(&self, target: &str) -> Option<Summary>;
}

/// For each argument of a call, the local its value (or, for a borrow
/// parameter, the value behind it) lives in, and whether the callee may
/// write through it.
pub fn call_arg_roots(args: &[Operand], params: Option<&[Type]>, body: &Body, targets: &HashMap<Local, Place>) -> Vec<Option<(Local, bool)>> {
    args.iter()
        .enumerate()
        .map(|(index, arg)| {
            let place = operand_place(arg)?;
            let param = params.and_then(|params| params.get(index));
            let arg_ty = place_type_hint(place, body);
            let mutable = match (param, &arg_ty) {
                (Some(Type::Borrow { mutable, .. }), _) => *mutable,
                (None, Some(Type::Borrow { mutable, .. })) => *mutable,
                _ => false,
            };
            Some((root(place, targets), mutable))
        })
        .collect()
}

fn place_type_hint(place: &Place, body: &Body) -> Option<Type> {
    match place {
        Place::Local(local) => body.locals.get(local.0 as usize).map(|decl| decl.ty.clone()),
        _ => None,
    }
}

pub fn graph(body: &Body, types: &Types<'_, '_>, callees: &dyn Callees) -> Graph {
    let targets = ref_targets(body);
    let mut edges = Vec::new();
    let carries = |local: Local| body.locals.get(local.0 as usize).is_some_and(|decl| types.can_be_active(&decl.ty));
    let heap = |local: Local| body.locals.get(local.0 as usize).is_some_and(|decl| types.has_heap(&decl.ty));
    let link = |from: Local, to: Local, edges: &mut Vec<(usize, usize)>| {
        if from == to || from.0 == u32::MAX || to.0 == u32::MAX {
            return;
        }
        edges.push((from.0 as usize, to.0 as usize));
        if heap(from) || heap(to) {
            edges.push((to.0 as usize, from.0 as usize));
        }
    };
    for block in &body.blocks {
        for statement in &block.statements {
            match statement {
                Statement::Assign(place, rvalue) => {
                    let dest = root(place, &targets);
                    if !carries(dest) && !matches!(rvalue, Rvalue::Ref { .. }) {
                        continue;
                    }
                    if let Rvalue::Ref { place: target, .. } = rvalue {
                        let target = root(target, &targets);
                        link(target, dest, &mut edges);
                        link(dest, target, &mut edges);
                        continue;
                    }
                    for operand in rvalue_operands(rvalue) {
                        if let Some(source) = operand_place(operand) {
                            link(root(source, &targets), dest, &mut edges);
                        }
                    }
                }
                Statement::Store { address, value, .. } => {
                    if let (Some(address), Some(value)) = (operand_place(address), operand_place(value)) {
                        link(root(value, &targets), root(address, &targets), &mut edges);
                    }
                }
                Statement::FreeBox { .. } | Statement::Drop(_) | Statement::StorageDead(_) => {}
            }
        }
        let (target, args, destination) = match &block.terminator {
            Terminator::Call { target, args, destination, .. } => (Some(target.0.as_str()), args, destination),
            Terminator::CallIndirect { args, destination, .. } => (None, args, destination),
            _ => continue,
        };
        let params = target.and_then(|target| callees.params(target));
        let roots = call_arg_roots(args, params.as_deref(), body, &targets);
        let dest = destination.as_ref().map(|place| root(place, &targets));
        if target.is_some_and(is_tape_call)
            && let Some(Some((tape, _))) = roots.first()
        {
            let tape = tape.0 as usize;
            for (arg, mutable) in roots.iter().skip(1).flatten() {
                edges.push((arg.0 as usize, tape));
                if *mutable {
                    edges.push((tape, arg.0 as usize));
                }
            }
            if let Some(dest) = dest {
                edges.push((tape, dest.0 as usize));
            }
            continue;
        }
        match target.and_then(|target| callees.summary(target)) {
            Some(summary) => {
                for &param in &summary.result {
                    if let (Some(Some((arg, _))), Some(dest)) = (roots.get(param), dest) {
                        link(*arg, dest, &mut edges);
                    }
                }
                for (output, inputs) in &summary.params {
                    let Some(Some((out, _))) = roots.get(*output) else { continue };
                    for &param in inputs {
                        if let Some(Some((arg, _))) = roots.get(param) {
                            link(*arg, *out, &mut edges);
                        }
                    }
                }
            }
            None => {
                for (arg, _) in roots.iter().flatten() {
                    if let Some(dest) = dest {
                        link(*arg, dest, &mut edges);
                    }
                    for (out, mutable) in roots.iter().flatten() {
                        if *mutable {
                            link(*arg, *out, &mut edges);
                        }
                    }
                }
            }
        }
    }
    let mut sinks = Vec::new();
    for block in &body.blocks {
        if let Terminator::Return(operand) = &block.terminator
            && let Some(place) = operand_place(operand)
        {
            sinks.push(root(place, &targets).0 as usize);
        }
    }
    for (index, decl) in body.locals[..body.param_count].iter().enumerate() {
        if matches!(decl.ty, Type::Borrow { mutable: true, .. }) || is_tape_local(body, Local(index as u32)) {
            sinks.push(index);
        }
    }
    Graph { edges, sinks, locals: body.locals.len() }
}

fn reach(graph: &Graph, seeds: &[usize], forward: bool) -> HashSet<usize> {
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(from, to) in &graph.edges {
        let (from, to) = if forward { (from, to) } else { (to, from) };
        adjacency.entry(from).or_default().push(to);
    }
    let mut seen: HashSet<usize> = seeds.iter().copied().collect();
    let mut pending: Vec<usize> = seeds.to_vec();
    while let Some(node) = pending.pop() {
        for &next in adjacency.get(&node).into_iter().flatten() {
            if seen.insert(next) {
                pending.push(next);
            }
        }
    }
    seen
}

#[derive(Clone, Debug)]
pub struct Activity {
    pub varied: Vec<bool>,
    pub useful: Vec<bool>,
}

impl Activity {
    pub fn is_varied(&self, local: Local) -> bool {
        self.varied.get(local.0 as usize).copied().unwrap_or(false)
    }

    pub fn is_active(&self, local: Local) -> bool {
        let index = local.0 as usize;
        self.varied.get(index).copied().unwrap_or(false) && self.useful.get(index).copied().unwrap_or(false)
    }
}

/// Activity of `body`'s locals when the parameters in `mask` are varied.
pub fn activity(body: &Body, mask: &[bool], types: &Types<'_, '_>, callees: &dyn Callees) -> Activity {
    let graph = graph(body, types, callees);
    let seeds: Vec<usize> = mask.iter().enumerate().filter(|(_, varied)| **varied).map(|(index, _)| index).collect();
    let varied = reach(&graph, &seeds, true);
    let useful = reach(&graph, &graph.sinks, false);
    let carries = |index: usize| types.can_be_active(&body.locals[index].ty) || is_tape_local(body, Local(index as u32));
    Activity {
        varied: (0..graph.locals).map(|index| varied.contains(&index) && carries(index)).collect(),
        useful: (0..graph.locals).map(|index| useful.contains(&index) && carries(index)).collect(),
    }
}

/// Which of `body`'s parameters reach its result and its `&mut` parameters.
pub fn summarize(body: &Body, types: &Types<'_, '_>, callees: &dyn Callees) -> Summary {
    let graph = graph(body, types, callees);
    let mut result_roots = HashSet::new();
    let targets = ref_targets(body);
    for block in &body.blocks {
        if let Terminator::Return(operand) = &block.terminator
            && let Some(place) = operand_place(operand)
        {
            result_roots.insert(root(place, &targets).0 as usize);
        }
    }
    let mut summary = Summary::default();
    let output = |local: usize| {
        matches!(body.locals[local].ty, Type::Borrow { mutable: true, .. }) || is_tape_local(body, Local(local as u32))
    };
    for param in 0..body.param_count {
        if !types.can_be_active(&body.locals[param].ty) && !is_tape_local(body, Local(param as u32)) {
            continue;
        }
        let reached = reach(&graph, &[param], true);
        if result_roots.iter().any(|root| reached.contains(root)) {
            summary.result.insert(param);
        }
        for target in 0..body.param_count {
            if output(target) && (target == param || reached.contains(&target)) {
                summary.params.entry(target).or_default().insert(param);
            }
        }
    }
    summary
}

/// Summaries of `names` and everything they call, iterated to a fixed
/// point from "nothing flows", so mutually recursive functions converge.
pub fn summaries(names: &[String], bodies: &HashMap<String, &Body>, types: &Types<'_, '_>, params: &dyn Fn(&str) -> Option<Vec<Type>>) -> HashMap<String, Summary> {
    let mut reachable: Vec<String> = Vec::new();
    let mut pending: Vec<String> = names.to_vec();
    while let Some(name) = pending.pop() {
        let Some(body) = bodies.get(name.as_str()) else { continue };
        if reachable.contains(&name) {
            continue;
        }
        reachable.push(name);
        for block in &body.blocks {
            if let Terminator::Call { target, .. } = &block.terminator {
                pending.push(target.0.clone());
            }
        }
    }
    struct Known<'a> {
        summaries: &'a HashMap<String, Summary>,
        params: &'a dyn Fn(&str) -> Option<Vec<Type>>,
    }
    impl Callees for Known<'_> {
        fn params(&self, target: &str) -> Option<Vec<Type>> {
            (self.params)(target)
        }
        fn summary(&self, target: &str) -> Option<Summary> {
            self.summaries.get(target).cloned()
        }
    }
    let mut current: HashMap<String, Summary> = reachable.iter().map(|name| (name.clone(), Summary::default())).collect();
    loop {
        let mut changed = false;
        for name in &reachable {
            let known = Known { summaries: &current, params };
            let next = summarize(bodies[name.as_str()], types, &known);
            if current[name] != next {
                current.insert(name.clone(), next);
                changed = true;
            }
        }
        if !changed {
            return current;
        }
    }
}
