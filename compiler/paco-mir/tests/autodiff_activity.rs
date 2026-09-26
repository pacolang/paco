use std::collections::HashMap;

use paco_mir::autodiff::activity::{self, Callees, Summary};
use paco_mir::autodiff::types::Types;
use paco_mir::{
    BasicBlock, BasicBlockId, BinOp, Body, CallTarget, Constant, Local, LocalDecl, Operand, Place, Profile, Rvalue, Statement,
    Terminator, TypeLayouts,
};
use paco_span::Span;
use paco_syntax::ast::Module;
use paco_types::{FloatWidth, IntWidth, Type};

const F64: Type = Type::Float(FloatWidth::F64);
const I64: Type = Type::Int(IntWidth::I64);

fn body(params: &[Type], locals: &[Type], blocks: Vec<BasicBlock>, return_ty: Type) -> Body {
    Body {
        locals: params.iter().chain(locals).map(|ty| LocalDecl { name: None, ty: ty.clone(), mutable: true }).collect(),
        blocks,
        profile: Profile::Debug,
        param_count: params.len(),
        return_ty,
        span: Span::new_root(0, 0),
        spans: Vec::new(),
    }
}

fn copy(local: u32) -> Operand {
    Operand::Copy(Place::Local(Local(local)))
}

fn assign(local: u32, rvalue: Rvalue) -> Statement {
    Statement::Assign(Place::Local(Local(local)), rvalue)
}

fn block(statements: Vec<Statement>, terminator: Terminator) -> BasicBlock {
    BasicBlock { statements, terminator }
}

fn float(value: f64) -> Operand {
    Operand::Constant(Constant::Float(value.to_bits(), FloatWidth::F64))
}

fn int(value: i64) -> Operand {
    Operand::Constant(Constant::Int(value, IntWidth::I64))
}

fn empty_module() -> Module {
    Module { name: None, items: Vec::new(), span: Span::new_root(0, 0) }
}

struct Known(HashMap<String, (Vec<Type>, Summary)>);

impl Callees for Known {
    fn params(&self, target: &str) -> Option<Vec<Type>> {
        self.0.get(target).map(|(params, _)| params.clone())
    }
    fn summary(&self, target: &str) -> Option<Summary> {
        self.0.get(target).map(|(_, summary)| summary.clone())
    }
}

/// `fn f(x: f64) -> f64 { let mut acc = 1.0; let mut i = 0; while i < 3 {
/// acc = acc * x; i = i + 1 } let seen = x > 2.0; acc }`, plus an FP8 copy
/// of `x` that goes nowhere.
fn looped() -> Body {
    body(
        &[F64],
        &[F64, I64, Type::Bool, Type::Bool, Type::Float(FloatWidth::F8E4M3), F64],
        vec![
            block(
                vec![
                    assign(1, Rvalue::Use(float(1.0))),
                    assign(2, Rvalue::Use(int(0))),
                    assign(4, Rvalue::BinaryOp(BinOp::Gt, copy(0), float(2.0))),
                    assign(5, Rvalue::Cast { operand: copy(0), target: Type::Float(FloatWidth::F8E4M3) }),
                    assign(6, Rvalue::Cast { operand: copy(5), target: F64 }),
                ],
                Terminator::Goto(BasicBlockId(1)),
            ),
            block(
                vec![assign(3, Rvalue::BinaryOp(BinOp::Lt, copy(2), int(3)))],
                Terminator::SwitchInt { discriminant: copy(3), targets: vec![(1, BasicBlockId(2))], otherwise: BasicBlockId(3) },
            ),
            block(
                vec![
                    assign(1, Rvalue::BinaryOp(BinOp::Mul, copy(1), copy(0))),
                    assign(2, Rvalue::BinaryOp(BinOp::Add, copy(2), int(1))),
                ],
                Terminator::Goto(BasicBlockId(1)),
            ),
            block(Vec::new(), Terminator::Return(copy(1))),
        ],
        F64,
    )
}

#[test]
fn an_integer_counter_in_a_loop_is_inactive_and_the_accumulator_is_active() {
    let module = empty_module();
    let layouts = TypeLayouts::from_module(&module);
    let types = Types { layouts: &layouts };
    let body = looped();
    let activity = activity::activity(&body, &[true], &types, &Known(HashMap::new()));
    assert!(activity.is_active(Local(0)), "the input feeds the result");
    assert!(activity.is_active(Local(1)), "the accumulator feeds the result");
    assert!(!activity.is_active(Local(2)), "an integer counter never carries a derivative");
    assert!(!activity.is_active(Local(3)), "a loop condition never carries a derivative");
}

#[test]
fn a_float_only_feeding_a_comparison_is_varied_but_not_useful() {
    let module = empty_module();
    let layouts = TypeLayouts::from_module(&module);
    let types = Types { layouts: &layouts };
    let body = body(
        &[F64],
        &[F64, Type::Bool],
        vec![block(
            vec![assign(1, Rvalue::BinaryOp(BinOp::Mul, copy(0), float(2.0))), assign(2, Rvalue::BinaryOp(BinOp::Gt, copy(1), float(1.0)))],
            Terminator::Return(float(0.0)),
        )],
        F64,
    );
    let activity = activity::activity(&body, &[true], &types, &Known(HashMap::new()));
    assert!(activity.is_varied(Local(1)));
    assert!(!activity.is_active(Local(1)), "a value the result does not depend on is not useful");
    assert!(!activity.is_active(Local(2)));
}

#[test]
fn fp8_locals_are_never_active() {
    let module = empty_module();
    let layouts = TypeLayouts::from_module(&module);
    let types = Types { layouts: &layouts };
    let mut body = looped();
    body.blocks[3].terminator = Terminator::Return(copy(6));
    let activity = activity::activity(&body, &[true], &types, &Known(HashMap::new()));
    assert!(!activity.is_active(Local(5)), "an FP8 value carries no derivative");
    assert!(!activity.is_varied(Local(6)), "a value widened from FP8 does not depend on the input");
}

fn call(target: &str, args: Vec<Operand>, destination: u32) -> Terminator {
    Terminator::Call { target: CallTarget(target.to_string()), args, destination: Some(Place::Local(Local(destination))), resume: BasicBlockId(1) }
}

#[test]
fn a_helper_that_returns_a_constant_has_an_inactive_result() {
    let module = empty_module();
    let layouts = TypeLayouts::from_module(&module);
    let types = Types { layouts: &layouts };
    let constant = body(&[F64], &[], vec![block(Vec::new(), Terminator::Return(float(3.0)))], F64);
    let square = body(
        &[F64],
        &[F64],
        vec![block(vec![assign(1, Rvalue::BinaryOp(BinOp::Mul, copy(0), copy(0)))], Terminator::Return(copy(1)))],
        F64,
    );
    let caller = body(
        &[F64],
        &[F64, F64, F64],
        vec![
            block(Vec::new(), call("constant", vec![copy(0)], 1)),
            block(Vec::new(), Terminator::Call { target: CallTarget("square".to_string()), args: vec![copy(0)], destination: Some(Place::Local(Local(2))), resume: BasicBlockId(2) }),
            block(vec![assign(3, Rvalue::BinaryOp(BinOp::Add, copy(1), copy(2)))], Terminator::Return(copy(3))),
        ],
        F64,
    );
    let bodies: HashMap<String, &Body> =
        [("constant".to_string(), &constant), ("square".to_string(), &square), ("caller".to_string(), &caller)].into_iter().collect();
    let params = |name: &str| bodies.get(name).map(|body| body.locals[..body.param_count].iter().map(|local| local.ty.clone()).collect());
    let summaries = activity::summaries(&["caller".to_string()], &bodies, &types, &params);
    assert!(summaries["constant"].result.is_empty(), "a constant does not depend on its parameter");
    assert!(summaries["square"].result.contains(&0));
    let known = Known(summaries.iter().map(|(name, summary)| (name.clone(), (vec![F64], summary.clone()))).collect());
    let activity = activity::activity(&caller, &[true], &types, &known);
    assert!(!activity.is_active(Local(1)), "the constant helper's result is inactive");
    assert!(activity.is_active(Local(2)));
}

#[test]
fn mutually_recursive_functions_reach_a_fixed_point() {
    let module = empty_module();
    let layouts = TypeLayouts::from_module(&module);
    let types = Types { layouts: &layouts };
    let recursive = |callee: &str| {
        body(
            &[F64],
            &[Type::Bool, F64, F64],
            vec![
                block(
                    vec![assign(1, Rvalue::BinaryOp(BinOp::Lt, copy(0), float(1.0)))],
                    Terminator::SwitchInt { discriminant: copy(1), targets: vec![(1, BasicBlockId(1))], otherwise: BasicBlockId(2) },
                ),
                block(Vec::new(), Terminator::Return(copy(0))),
                block(
                    vec![assign(2, Rvalue::BinaryOp(BinOp::Mul, copy(0), float(0.5)))],
                    Terminator::Call { target: CallTarget(callee.to_string()), args: vec![copy(2)], destination: Some(Place::Local(Local(3))), resume: BasicBlockId(3) },
                ),
                block(Vec::new(), Terminator::Return(copy(3))),
            ],
            F64,
        )
    };
    let even = recursive("odd");
    let odd = recursive("even");
    let bodies: HashMap<String, &Body> = [("even".to_string(), &even), ("odd".to_string(), &odd)].into_iter().collect();
    let params = |_: &str| Some(vec![F64]);
    let summaries = activity::summaries(&["even".to_string()], &bodies, &types, &params);
    assert!(summaries["even"].result.contains(&0), "{summaries:?}");
    assert!(summaries["odd"].result.contains(&0), "{summaries:?}");
}
