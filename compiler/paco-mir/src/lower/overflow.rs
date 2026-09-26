//! Lowering of the explicit overflow methods on integers (`wrapping_add`,
//! `saturating_sub`, `checked_mul`, `overflowing_add`, ...).

use super::*;

impl Lowerer<'_> {
    pub(super) fn lower_overflow_method(
        &mut self,
        receiver: &Expr,
        family: &str,
        operation: &str,
        arg: &Expr,
        width: IntWidth,
        result_ty: Type,
    ) -> Operand {
        let ty = Type::Int(width);
        let left = self.lower_operand(receiver);
        let right = self.lower_operand(arg);
        let (wrapping, overflows) = match operation {
            "add" => (BinOp::WrappingAdd, BinOp::AddOverflows),
            "sub" => (BinOp::WrappingSub, BinOp::SubOverflows),
            _ => (BinOp::WrappingMul, BinOp::MulOverflows),
        };
        let wrapped = self.binary(wrapping, left.clone(), right.clone(), ty.clone());
        if family == "wrapping" {
            return wrapped;
        }
        let overflowed = self.compare(overflows, left.clone(), right.clone());
        let result = self.declare_local(None, result_ty.clone(), false);
        match family {
            "overflowing" => {
                self.push(Statement::Assign(
                    Place::Local(result),
                    Rvalue::Aggregate { ty: result_ty, variant: None, fields: vec![wrapped, overflowed] },
                ));
            }
            "checked" => {
                let none = Rvalue::Aggregate { ty: result_ty.clone(), variant: Some("None".to_string()), fields: Vec::new() };
                let some = Rvalue::Aggregate { ty: result_ty, variant: Some("Some".to_string()), fields: vec![wrapped] };
                self.assign_by_condition(overflowed, result, none, some);
            }
            _ => {
                let bound = self.saturation_bound(operation, left, right, width);
                self.assign_by_condition(overflowed, result, Rvalue::Use(bound), Rvalue::Use(wrapped));
            }
        }
        Operand::Copy(Place::Local(result))
    }

    /// The value a saturating operation clamps to when it overflows.
    fn saturation_bound(&mut self, operation: &str, left: Operand, right: Operand, width: IntWidth) -> Operand {
        let (min, max) = width.range();
        let min = Operand::Constant(Constant::Int(width.from_i128(min), width));
        let max = Operand::Constant(Constant::Int(width.from_i128(max), width));
        if !width.is_signed() {
            return if operation == "sub" { min } else { max };
        }
        let zero = Operand::Constant(Constant::Int(0, width));
        let right_negative = self.compare(BinOp::Lt, right, zero.clone());
        let toward_min = match operation {
            "add" => right_negative,
            "sub" => self.compare(BinOp::Eq, right_negative, Operand::Constant(Constant::Bool(false))),
            _ => {
                let left_negative = self.compare(BinOp::Lt, left, zero);
                self.compare(BinOp::Ne, left_negative, right_negative)
            }
        };
        let bound = self.declare_local(None, Type::Int(width), false);
        self.assign_by_condition(toward_min, bound, Rvalue::Use(min), Rvalue::Use(max));
        Operand::Copy(Place::Local(bound))
    }

    fn assign_by_condition(&mut self, condition: Operand, target: Local, when_true: Rvalue, when_false: Rvalue) {
        let then_id = self.reserve_block();
        let else_id = self.reserve_block();
        let join_id = self.reserve_block();
        self.finish_current(Terminator::SwitchInt { discriminant: condition, targets: vec![(1, then_id)], otherwise: else_id });
        self.switch_to(then_id);
        self.push(Statement::Assign(Place::Local(target), when_true));
        self.finish_current(Terminator::Goto(join_id));
        self.switch_to(else_id);
        self.push(Statement::Assign(Place::Local(target), when_false));
        self.finish_current(Terminator::Goto(join_id));
        self.switch_to(join_id);
    }
}
