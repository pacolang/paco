//! Lowering of symbolic dimensions: hidden dimension arguments, the values
//! of dimension names, and the shape intrinsics.

use super::*;
use crate::dims::{Slot, hidden_slots, instance_args, slot_name, slot_type};
use paco_types::{AtomSource, ConstExpr, Dim, DimOp, Factor};

pub(super) const INTRINSICS: [&str; 5] = ["dim", "with_dims", "as_dims", "assume_dims", "erase_dims"];

const I64: IntWidth = IntWidth::I64;

fn int(value: i64) -> Operand {
    Operand::Constant(Constant::Int(value, I64))
}

/// Every dimension position of a struct type (through one borrow).
fn slots_of(ty: &Type, registry: &TypeRegistry<'_>) -> Vec<Type> {
    let Type::Struct(name, args) = strip_borrow(ty) else { return Vec::new() };
    let params: Vec<&GenericParam> =
        registry.owner_generics(name).iter().filter(|param| param.kind != GenericParamKind::Lifetime).collect();
    let mut out = Vec::new();
    for (param, arg) in params.iter().zip(args) {
        match (&param.kind, arg) {
            (GenericParamKind::Const(_) | GenericParamKind::Dim, _) => out.push(arg.clone()),
            (GenericParamKind::ConstPack(_), Type::Pack(items)) => out.extend(items.iter().cloned()),
            _ => {}
        }
    }
    out
}

fn is_static(ty: &Type) -> Option<i64> {
    match ty {
        Type::Dim(Dim::Const(expr)) => expr.as_lit(),
        _ => None,
    }
}

impl<'a> Lowerer<'a> {
    /// A type with the instance's bindings applied, dimension names kept.
    pub(super) fn symbolic(&self, ty: &Type) -> Type {
        paco_types::substitute_generics(ty, self.substitutions)
    }

    pub(super) fn symbolic_type_of(&self, expr: &Expr) -> Type {
        let ty = self.typed.type_of(expr).unwrap_or_else(|| panic!("expression has no recorded type: {expr:?}"));
        self.symbolic(ty)
    }

    fn callee_params(&self, owner: Option<&str>) -> &'a [GenericParam] {
        owner.map_or(&[], |owner| self.registry.owner_generics(owner))
    }

    /// The instance a call with these type arguments reaches, and the hidden
    /// dimension arguments it takes.
    pub(super) fn instance_of(&self, owner: Option<&str>, callee: Option<&FnDecl>, symbolic_args: &[Type]) -> (Vec<Type>, Vec<Slot>) {
        let owner_params = self.callee_params(owner);
        let key = instance_args(owner_params, callee, symbolic_args);
        let slots = hidden_slots(owner_params, callee, &key);
        (key, slots)
    }

    /// The values of a call's hidden dimension arguments. A `dim` parameter
    /// bound to an anonymous extent takes the extent of the argument that
    /// fixed it.
    pub(super) fn hidden_values(
        &mut self,
        owner: Option<&str>,
        callee: Option<&FnDecl>,
        slots: &[Slot],
        symbolic_args: &[Type],
        args: &[(Operand, Type)],
    ) -> Vec<Operand> {
        let owner_params = self.callee_params(owner);
        slots
            .iter()
            .map(|slot| {
                let ty = slot_type(symbolic_args, *slot);
                let name = slot_name(owner_params, callee, *slot);
                // Binding a `Shaped` value whose type has an open `Dyn`
                // position (`self`, a `let`, a match arm) opens that position
                // into an anonymous atom (`paco-types`' `open_binding`) purely
                // so `x.dim(axis)` *can* read its real extent later — nothing
                // has claimed it yet. That atom's symbolic name then keeps
                // flowing through every further generic call this value is
                // passed to, including ones with no idea it now stands for
                // more than `Dyn`. A `const` position that is still
                // fundamentally `Dyn` this way must keep passing the `-1`
                // sentinel such code branches on (`if M < 0 { .. }`), not the
                // atom's real value — unlike a name the program went on to
                // *claim* (a witness, or `let n = x.dim(axis);`), which keeps
                // its real value from here on, so only anonymous names decay.
                // The one exception is the callee itself declaring this slot
                // `dim`-kind, the one place that does mean "give the real
                // extent", handled below via `extent_of_argument`.
                if paco_types::erase_anonymous(&ty) != Type::Dim(Dim::Dyn) {
                    return self.dim_value(&ty, &name);
                }
                if let Some(callee) = callee
                    && callee.generics.iter().any(|param| param.name == name && param.is_dim())
                    && let Some(value) = self.extent_of_argument(callee, &name, args)
                {
                    return value;
                }
                int(-1)
            })
            .collect()
    }

    fn extent_of_argument(&mut self, callee: &FnDecl, name: &str, args: &[(Operand, Type)]) -> Option<Operand> {
        let offset = usize::from(callee.params.first().is_some_and(|param| matches!(&param.pattern, Pat::Ident(name, _) if name == "self")));
        for (index, param) in callee.params.iter().enumerate().skip(offset) {
            let Some(axis) = ty_axis(&param.ty, name, self.registry) else { continue };
            let (operand, ty) = args.get(index)?.clone();
            let receiver = match operand {
                Operand::Copy(place) | Operand::Move(place) => Operand::Copy(place),
                Operand::Constant(_) => return None,
            };
            return self.emit_extent(receiver, &ty, axis);
        }
        None
    }

    /// The run-time value of a dimension: a constant, a name's value, or
    /// checked arithmetic over them. `Dyn` with nothing to read is `-1`.
    pub(super) fn dim_value(&mut self, ty: &Type, what: &str) -> Operand {
        match ty {
            Type::Dim(Dim::Const(expr)) => match expr.as_lit() {
                Some(value) => int(value),
                None => self.eval_dim(expr, what),
            },
            Type::Generic(name) => self.name_value(name),
            _ => int(-1),
        }
    }

    fn name_value(&mut self, name: &str) -> Operand {
        if let Some(value) = self.atom_values.get(name) {
            return value.clone();
        }
        if let Some(bound) = self.substitutions.get(name).cloned()
            && bound != Type::Generic(name.to_string())
        {
            return self.dim_value(&bound, name);
        }
        match self.typed.atom(name).map(|info| info.source.clone()) {
            Some(AtomSource::Value(id)) => {
                if let Some(local) = self.resolve_id(id) {
                    return Operand::Copy(Place::Local(local));
                }
            }
            Some(AtomSource::Param(param)) if param != name => return self.name_value(&param),
            _ => {}
        }
        int(-1)
    }

    fn eval_dim(&mut self, expr: &ConstExpr, what: &str) -> Operand {
        let message = format!("dimension `{what}` = `{expr}` overflows i64 or divides by zero");
        let mut total = int(0);
        for (coefficient, factors) in expr.terms() {
            let mut term = int(coefficient);
            for (factor, exponent) in factors {
                let value = match factor {
                    Factor::Name(name) => self.name_value(name),
                    Factor::Opaque(op, left, right) => {
                        let left = self.eval_dim(&left, what);
                        let right = self.eval_dim(&right, what);
                        self.checked(op, left, right, &message)
                    }
                };
                for _ in 0..exponent {
                    term = self.checked(DimOp::Mul, term, value.clone(), &message);
                }
            }
            total = self.checked(DimOp::Add, total, term, &message);
        }
        let negative = self.compare(BinOp::Lt, total.clone(), int(0));
        self.panic_if(negative, &format!("dimension `{what}` = `{expr}` is negative"));
        total
    }

    pub(super) fn binary(&mut self, op: BinOp, left: Operand, right: Operand, ty: Type) -> Operand {
        let local = self.declare_local(None, ty, false);
        self.push(Statement::Assign(Place::Local(local), Rvalue::BinaryOp(op, left, right)));
        Operand::Copy(Place::Local(local))
    }

    pub(super) fn compare(&mut self, op: BinOp, left: Operand, right: Operand) -> Operand {
        self.binary(op, left, right, Type::Bool)
    }

    fn panic_if(&mut self, condition: Operand, message: &str) {
        let fail = self.reserve_block();
        let next = self.reserve_block();
        self.finish_current(Terminator::SwitchInt { discriminant: condition, targets: vec![(1, fail)], otherwise: next });
        self.switch_to(fail);
        self.lower_panic(Operand::Constant(Constant::Str(message.to_string())));
        self.finish_current(Terminator::Goto(next));
        self.switch_to(next);
    }

    /// `left op right` on `i64`, panicking with `message` instead of
    /// overflowing or dividing by zero, in every profile.
    fn checked(&mut self, op: DimOp, left: Operand, right: Operand, message: &str) -> Operand {
        let min = int(i64::MIN);
        let overflow = match op {
            DimOp::Add | DimOp::Sub => self.add_overflows(op, left.clone(), right.clone()),
            DimOp::Mul => self.mul_overflows(left.clone(), right.clone()),
            DimOp::Div | DimOp::Rem => {
                let zero = self.compare(BinOp::Eq, right.clone(), int(0));
                let is_min = self.compare(BinOp::Eq, left.clone(), min);
                let minus_one = self.compare(BinOp::Eq, right.clone(), int(-1));
                let wraps = self.compare(BinOp::And, is_min, minus_one);
                self.compare(BinOp::Or, zero, wraps)
            }
        };
        self.panic_if(overflow, message);
        let op = match op {
            DimOp::Add => BinOp::Add,
            DimOp::Sub => BinOp::Sub,
            DimOp::Mul => BinOp::Mul,
            DimOp::Div => BinOp::Div,
            DimOp::Rem => BinOp::Rem,
        };
        self.binary(op, left, right, Type::Int(I64))
    }

    /// Whether `left ± right` overflows, decided with limits that cannot.
    fn add_overflows(&mut self, op: DimOp, left: Operand, right: Operand) -> Operand {
        let result = self.declare_local(None, Type::Bool, false);
        let positive = self.compare(BinOp::Gt, right.clone(), int(0));
        let (up, down, join) = (self.reserve_block(), self.reserve_block(), self.reserve_block());
        self.finish_current(Terminator::SwitchInt { discriminant: positive, targets: vec![(1, up)], otherwise: down });
        let cases = match op {
            DimOp::Add => [(up, BinOp::Gt, BinOp::Sub, i64::MAX), (down, BinOp::Lt, BinOp::Sub, i64::MIN)],
            _ => [(up, BinOp::Lt, BinOp::Add, i64::MIN), (down, BinOp::Gt, BinOp::Add, i64::MAX)],
        };
        for (block, test, limit_op, limit) in cases {
            self.switch_to(block);
            let bound = self.binary(limit_op, int(limit), right.clone(), Type::Int(I64));
            let overflow = self.compare(test, left.clone(), bound);
            self.push(Statement::Assign(Place::Local(result), Rvalue::Use(overflow)));
            self.finish_current(Terminator::Goto(join));
        }
        self.switch_to(join);
        Operand::Copy(Place::Local(result))
    }

    /// Whether `left * right` overflows, decided with divisions that cannot.
    fn mul_overflows(&mut self, left: Operand, right: Operand) -> Operand {
        let result = self.declare_local(None, Type::Bool, false);
        let set = |this: &mut Self, value: Operand| this.push(Statement::Assign(Place::Local(result), Rvalue::Use(value)));
        set(self, Operand::Constant(Constant::Bool(false)));
        let join = self.reserve_block();
        let left_zero = self.compare(BinOp::Eq, left.clone(), int(0));
        let right_zero = self.compare(BinOp::Eq, right.clone(), int(0));
        let trivial = self.compare(BinOp::Or, left_zero, right_zero);
        let check = self.reserve_block();
        self.finish_current(Terminator::SwitchInt { discriminant: trivial, targets: vec![(1, join)], otherwise: check });
        self.switch_to(check);
        let left_positive = self.compare(BinOp::Gt, left.clone(), int(0));
        let right_positive = self.compare(BinOp::Gt, right.clone(), int(0));
        let (pp, pn, np, nn) = (self.reserve_block(), self.reserve_block(), self.reserve_block(), self.reserve_block());
        let (left_pos, left_neg) = (self.reserve_block(), self.reserve_block());
        self.finish_current(Terminator::SwitchInt { discriminant: left_positive, targets: vec![(1, left_pos)], otherwise: left_neg });
        self.switch_to(left_pos);
        self.finish_current(Terminator::SwitchInt { discriminant: right_positive.clone(), targets: vec![(1, pp)], otherwise: pn });
        self.switch_to(left_neg);
        self.finish_current(Terminator::SwitchInt { discriminant: right_positive, targets: vec![(1, np)], otherwise: nn });
        let (max, min) = (int(i64::MAX), int(i64::MIN));
        let cases = [
            (pp, left.clone(), BinOp::Gt, max.clone(), right.clone()),
            (pn, right.clone(), BinOp::Lt, min.clone(), left.clone()),
            (np, left.clone(), BinOp::Lt, min, right.clone()),
            (nn, left.clone(), BinOp::Lt, max, right.clone()),
        ];
        for (block, tested, op, limit, divisor) in cases {
            self.switch_to(block);
            let bound = self.binary(BinOp::Div, limit, divisor, Type::Int(I64));
            let overflow = self.compare(op, tested, bound);
            set(self, overflow);
            self.finish_current(Terminator::Goto(join));
        }
        self.switch_to(join);
        Operand::Copy(Place::Local(result))
    }

    /// `receiver.extent(axis)` through the type's `Shaped` method, with
    /// every hidden dimension of the call passed as unknown.
    pub(super) fn emit_extent(&mut self, receiver: Operand, ty: &Type, axis: usize) -> Option<Operand> {
        let Type::Struct(name, args) = strip_borrow(ty).clone() else { return None };
        let (decl, _) = self.registry.find_method_decl(&name, "extent")?;
        let (key, slots) = self.instance_of(Some(&name), Some(decl), &args);
        let target = CallTarget(self.instantiations.record(&self.registry.symbol(&format!("{name}::extent")), &key));
        let mut call_args: Vec<Operand> = slots.iter().map(|_| int(-1)).collect();
        call_args.push(receiver);
        call_args.push(int(axis as i64));
        Some(self.emit_call(target, call_args, Type::Int(I64)))
    }

    /// Reads, once, the extents that names opened on the binding `id` stand
    /// for.
    pub(super) fn bind_atoms(&mut self, id: LocalId, local: Local) {
        let atoms = self.typed.atoms_of(id).to_vec();
        for atom in atoms {
            if self.atom_values.contains_key(&atom) {
                continue;
            }
            let Some(info) = self.typed.atom(&atom) else { continue };
            let AtomSource::Extent { field, axis, ty, .. } = info.source.clone() else { continue };
            let ty = paco_types::erase_symbolic(&self.symbolic(&ty));
            let base_ty = self.locals[local.0 as usize].ty.clone();
            let place = match &field {
                None => Place::Local(local),
                Some(field) => {
                    let base = match &base_ty {
                        Type::Borrow { .. } => Place::Deref { address: Box::new(Operand::Copy(Place::Local(local))), ty: strip_borrow(&base_ty).clone() },
                        _ => Place::Local(local),
                    };
                    Place::Field { base: Box::new(base), field: field.clone() }
                }
            };
            if let Some(value) = self.emit_extent(Operand::Copy(place), &ty, axis) {
                let cached = self.declare_local(None, Type::Int(I64), false);
                self.push(Statement::Assign(Place::Local(cached), Rvalue::Use(value)));
                self.atom_values.insert(atom, Operand::Copy(Place::Local(cached)));
            }
        }
    }

    /// The dimension values known here, which a closure or task body needs
    /// to pass on to the generic code it calls.
    pub(super) fn dimension_captures(&self) -> Vec<(String, Operand)> {
        let mut out: Vec<(String, Operand)> = self
            .atom_values
            .iter()
            .filter(|(_, value)| matches!(value, Operand::Copy(Place::Local(_))))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        for scope in &self.scopes {
            for (id, local) in scope {
                for atom in self.typed.atoms_of(*id) {
                    if matches!(self.typed.atom(atom).map(|info| &info.source), Some(AtomSource::Value(source)) if source == id)
                        && !out.iter().any(|(name, _)| name == atom)
                    {
                        out.push((atom.clone(), Operand::Copy(Place::Local(*local))));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub(super) fn store_dimension_captures(&mut self, base: Local, offset: i64, dims: &[(String, Operand)]) {
        for (index, (_, value)) in dims.iter().enumerate() {
            let slot = self.offset_address(base, offset + (index as i64) * 8);
            self.push(Statement::Store { address: Operand::Copy(Place::Local(slot)), value: value.clone(), ty: Type::Int(I64) });
        }
    }

    pub(super) fn load_dimension_captures(&mut self, base: Local, offset: i64, dims: &[(String, Operand)]) {
        for (index, (name, _)) in dims.iter().enumerate() {
            let slot = self.offset_address(base, offset + (index as i64) * 8);
            let value = self.declare_local(None, Type::Int(I64), false);
            self.push(Statement::Assign(Place::Local(value), Rvalue::Load { address: Operand::Copy(Place::Local(slot)), ty: Type::Int(I64) }));
            self.atom_values.insert(name.clone(), Operand::Copy(Place::Local(value)));
        }
    }

    pub(super) fn is_intrinsic(&self, type_name: &str, method: &str) -> bool {
        INTRINSICS.contains(&method) && self.registry.find_method_decl(type_name, method).is_none()
    }

    /// `dim`, `with_dims`, `as_dims`, `assume_dims` and `erase_dims`.
    pub(super) fn lower_dim_intrinsic(&mut self, receiver: &Expr, method: &str, args: &[Expr], call_expr: &Expr) -> Operand {
        let receiver_symbolic = self.symbolic_type_of(receiver);
        let receiver_ty = self.type_of(receiver);
        let result_ty = self.type_of(call_expr);
        match method {
            "dim" => {
                let axis = match args {
                    [Expr::Literal(Literal::Int(axis), _)] => *axis as usize,
                    _ => panic!("`dim` takes a literal axis, checked by the type checker"),
                };
                let slot = slots_of(&receiver_symbolic, self.registry).get(axis).cloned().unwrap_or(Type::Dim(Dim::Dyn));
                if slot == Type::Dim(Dim::Dyn) {
                    let place = self.lower_place(receiver);
                    return self.emit_extent(Operand::Copy(place), &receiver_ty, axis).unwrap_or(int(-1));
                }
                self.dim_value(&slot, &slot.name())
            }
            "erase_dims" => {
                let value = self.lower_operand(receiver);
                let local = self.declare_local(None, result_ty.clone(), false);
                self.push(Statement::Assign(Place::Local(local), Rvalue::Use(value)));
                Operand::Move(Place::Local(local))
            }
            _ => self.lower_refinement(receiver, &receiver_symbolic, &receiver_ty, method, call_expr),
        }
    }

    fn lower_refinement(&mut self, receiver: &Expr, source: &Type, receiver_ty: &Type, method: &str, call_expr: &Expr) -> Operand {
        let result_ty = self.type_of(call_expr);
        let result_symbolic = self.symbolic_type_of(call_expr);
        let target = match &result_symbolic {
            Type::Enum(name, args) if name == "Result" && method != "assume_dims" => strip_borrow(&args[0]).clone(),
            other => strip_borrow(other).clone(),
        };
        let place = self.lower_place(receiver);
        let checks: Vec<(usize, Type, Type)> = slots_of(source, self.registry)
            .into_iter()
            .zip(slots_of(&target, self.registry))
            .enumerate()
            .filter(|(_, (from, to))| from != to && (is_static(from).is_none() || is_static(to).is_none()))
            .map(|(axis, (from, to))| (axis, from, to))
            .collect();
        let out = self.declare_local(None, result_ty.clone(), false);
        let join = self.reserve_block();
        let checked = method != "assume_dims" || self.profile == Profile::Debug;
        if checked {
            for (axis, from, to) in &checks {
                let observed = match from {
                    Type::Dim(Dim::Dyn) => self.emit_extent(Operand::Copy(place.clone()), receiver_ty, *axis).unwrap_or(int(-1)),
                    _ => self.dim_value(from, &from.name()),
                };
                let expected = self.dim_value(to, &to.name());
                if method == "assume_dims" {
                    let differ = self.compare(BinOp::Ne, observed, expected);
                    self.panic_if(differ, &format!("`assume_dims`: dimension {axis} is not `{}`", to.name()));
                    continue;
                }
                let negative = self.compare(BinOp::Lt, observed.clone(), int(0));
                let differ = self.compare(BinOp::Ne, observed.clone(), expected.clone());
                let bad = self.compare(BinOp::Or, negative, differ);
                let fail = self.reserve_block();
                let next = self.reserve_block();
                self.finish_current(Terminator::SwitchInt { discriminant: bad, targets: vec![(1, fail)], otherwise: next });
                self.switch_to(fail);
                let error = self.dim_error(&result_ty, *axis, to, expected, observed);
                self.push(Statement::Assign(
                    Place::Local(out),
                    Rvalue::Aggregate { ty: result_ty.clone(), variant: Some("Err".to_string()), fields: vec![error] },
                ));
                self.finish_current(Terminator::Goto(join));
                self.switch_to(next);
            }
        }
        let value = match method {
            "as_dims" if matches!(receiver_ty, Type::Borrow { .. }) => Operand::Copy(place),
            "as_dims" => {
                let borrow_ty = Type::Borrow { mutable: false, ty: Box::new(receiver_ty.clone()) };
                let borrow = self.declare_local(None, borrow_ty, false);
                self.push(Statement::Assign(Place::Local(borrow), Rvalue::Ref { mutable: false, place }));
                Operand::Copy(Place::Local(borrow))
            }
            _ if is_copy(receiver_ty) => Operand::Copy(place),
            _ => Operand::Move(place),
        };
        let rvalue = if method == "assume_dims" {
            Rvalue::Use(value)
        } else {
            Rvalue::Aggregate { ty: result_ty.clone(), variant: Some("Ok".to_string()), fields: vec![value] }
        };
        self.push(Statement::Assign(Place::Local(out), rvalue));
        self.finish_current(Terminator::Goto(join));
        self.switch_to(join);
        if is_copy(&result_ty) { Operand::Copy(Place::Local(out)) } else { Operand::Move(Place::Local(out)) }
    }

    fn dim_error(&mut self, result_ty: &Type, axis: usize, target: &Type, bound: Operand, observed: Operand) -> Operand {
        let Type::Enum(_, args) = result_ty else { panic!("a refinement returns `Result`") };
        let error_ty = args[1].clone();
        let Type::Struct(error_name, _) = &error_ty else { panic!("`DimError` is a struct") };
        let origin = match target {
            Type::Generic(name) => self.typed.atom(name).map(|info| info.origin_text.clone()).unwrap_or_default(),
            _ => String::new(),
        };
        let order = self.registry.struct_field_order(error_name).unwrap_or_else(|| panic!("unknown struct `{error_name}`"));
        let fields = order
            .iter()
            .map(|field| match *field {
                "axis" => int(axis as i64),
                "expected" => Operand::Constant(Constant::Str(target.name())),
                "origin" => Operand::Constant(Constant::Str(origin.clone())),
                "bound" => bound.clone(),
                "observed" => observed.clone(),
                other => panic!("`DimError` has an unexpected field `{other}`"),
            })
            .collect();
        let local = self.declare_local(None, error_ty.clone(), false);
        self.push(Statement::Assign(Place::Local(local), Rvalue::Aggregate { ty: error_ty, variant: None, fields }));
        Operand::Move(Place::Local(local))
    }
}

/// The dimension axis at which a parameter type `ty` (through one borrow)
/// mentions the generic `name` alone.
fn ty_axis(ty: &Ty, name: &str, registry: &TypeRegistry<'_>) -> Option<usize> {
    match ty {
        Ty::Borrow { ty, .. } => ty_axis(ty, name, registry),
        Ty::Generic { path, args, .. } => {
            let params: Vec<&GenericParam> =
                registry.owner_generics(&path.join("::")).iter().filter(|param| param.kind != GenericParamKind::Lifetime).collect();
            let has_pack = params.last().is_some_and(|param| param.is_pack());
            let mut axis = 0;
            for (index, arg) in args.iter().enumerate() {
                let in_pack = has_pack && index + 1 >= params.len();
                if !in_pack && params.get(index).is_none_or(|param| param.kind == GenericParamKind::Type) {
                    continue;
                }
                if matches!(arg, Ty::Path(path, _) if path.len() == 1 && path[0] == name) {
                    return Some(axis);
                }
                axis += 1;
            }
            None
        }
        _ => None,
    }
}
