//! Lowering of the per-type builtins behind the collections: `hash_of`,
//! a 64-bit hash, and `slice_sort_native`, which sorts primitive slices in
//! the runtime.

use super::*;

const U64: IntWidth = IntWidth::U64;

fn word(value: u64) -> Operand {
    Operand::Constant(Constant::Int(value as i64, U64))
}

impl Lowerer<'_> {
    pub(super) fn lower_hash_of(&mut self, arg: &Expr) -> Operand {
        let arg_ty = self.type_of(arg);
        let value_ty = strip_borrow(&arg_ty).clone();
        let borrowed = self.lower_operand(arg);
        match &value_ty {
            Type::String => self.emit_call(CallTarget("string_hash".to_string()), vec![borrowed], Type::Int(U64)),
            Type::Struct(..) | Type::Enum(..) => {
                let Some(type_name) = type_name_of(&value_ty) else { unreachable!("nominal types have a name") };
                let base_target = self.registry.symbol(&format!("{type_name}::hash"));
                let callee = self.registry.find_method_decl(&type_name, "hash").map(|(decl, _)| decl);
                let symbolic = nominal_type_args(&self.symbolic(&value_ty)).to_vec();
                let (key, _) = self.instance_of(Some(&type_name), callee, &symbolic);
                let target = CallTarget(self.instantiations.record(&base_target, &key));
                self.emit_call(target, vec![borrowed], Type::Int(U64))
            }
            _ => {
                let value = Operand::Copy(Place::Deref { address: Box::new(borrowed), ty: value_ty.clone() });
                let bits = self.declare_local(None, Type::Int(U64), false);
                self.push(Statement::Assign(Place::Local(bits), Rvalue::Cast { operand: value, target: Type::Int(U64) }));
                self.mix(Operand::Copy(Place::Local(bits)))
            }
        }
    }

    /// The 64-bit finalizer of MurmurHash3: every input bit affects every
    /// output bit, so masking the result to a table size stays uniform.
    fn mix(&mut self, value: Operand) -> Operand {
        let ty = Type::Int(U64);
        let mut h = value;
        for multiplier in [0xff51_afd7_ed55_8ccd_u64, 0xc4ce_b9fe_1a85_ec53] {
            let shifted = self.binary(BinOp::Shr, h.clone(), word(33), ty.clone());
            let folded = self.binary(BinOp::BitXor, h, shifted, ty.clone());
            h = self.binary(BinOp::WrappingMul, folded, word(multiplier), ty.clone());
        }
        let shifted = self.binary(BinOp::Shr, h.clone(), word(33), ty.clone());
        self.binary(BinOp::BitXor, h, shifted, ty)
    }

    /// Sorts in the runtime and yields `true` when the element type is a
    /// primitive the runtime sorts; yields `false` otherwise.
    pub(super) fn lower_slice_sort_native(&mut self, xs: &Expr, len: &Expr) -> Operand {
        let elem = match strip_borrow(&self.type_of(xs)) {
            Type::Slice(elem) => elem.as_ref().clone(),
            other => unreachable!("`slice_sort_native` takes a slice, found {other:?}"),
        };
        let kind = match elem {
            Type::Int(IntWidth::I8) => 0,
            Type::Int(IntWidth::I16) => 1,
            Type::Int(IntWidth::I32) => 2,
            Type::Int(IntWidth::I64) => 3,
            Type::Int(IntWidth::U8) | Type::Bool => 4,
            Type::Int(IntWidth::U16) => 5,
            Type::Int(IntWidth::U32) | Type::Char => 6,
            Type::Int(IntWidth::U64) => 7,
            Type::Float(FloatWidth::F32) => 8,
            Type::Float(FloatWidth::F64) => 9,
            Type::Float(FloatWidth::F16) => 10,
            Type::Float(FloatWidth::BF16) => 11,
            _ => return Operand::Constant(Constant::Bool(false)),
        };
        let slice = self.lower_operand(xs);
        let len = self.lower_operand(len);
        let kind = Operand::Constant(Constant::Int(kind, IntWidth::I32));
        self.emit_call(CallTarget("slice_sort".to_string()), vec![slice, len, kind], Type::Unit);
        Operand::Constant(Constant::Bool(true))
    }
}
