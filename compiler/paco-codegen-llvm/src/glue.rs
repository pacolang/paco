//! Per-type drop and clone glue: one `(ptr) -> void` function per owning
//! type. Drop releases what the value at `ptr` owns; clone fixes up a fresh
//! bitwise copy at `ptr` so it owns its own heap storage.

use std::collections::HashMap;

use inkwell::IntPredicate;
use inkwell::module::Linkage;
use inkwell::values::{FunctionValue, IntValue, PointerValue};

use paco_mir::Body;
use paco_mir::glue::{
    CELL_VALUE_OFFSET, CLOSURE_DROP_FN, CLOSURE_HEADER, element_size, glue_fields, handle_fns, is_cell, owning_types,
    user_drop_fns,
};
use paco_types::Type;

use crate::codegen::Generator;

pub(crate) fn build(generator: &mut Generator<'_, '_, '_>, bodies: &[(String, Body)], imports: &[(String, Body)]) {
    let all_bodies: Vec<(String, Body)> = bodies.iter().chain(imports).cloned().collect();
    let user_drops = user_drop_fns(&all_bodies);
    let owning = owning_types(bodies, generator.layouts, &user_drops);
    let glue_type = generator.context.void_type().fn_type(&[generator.ptr().into()], false);
    for (index, ty) in owning.iter().enumerate() {
        let drop = generator.module.add_function(&format!("__paco_drop_{index}"), glue_type, Some(Linkage::Internal));
        let clone = generator.module.add_function(&format!("__paco_clone_{index}"), glue_type, Some(Linkage::Internal));
        generator.glue.insert(ty.clone(), (drop, clone));
    }
    let user_drops: HashMap<Type, FunctionValue<'_>> = user_drops
        .into_iter()
        .map(|(ty, name)| (ty, generator.module.get_function(&name).expect("user drop is declared")))
        .collect();
    for ty in &owning {
        let (drop, clone) = generator.glue[ty];
        for (function, is_drop) in [(drop, true), (clone, false)] {
            let entry = generator.block(function);
            generator.builder.position_at_end(entry);
            let address = function.get_first_param().expect("glue parameter").into_pointer_value();
            let emitter = Emitter { generator, function, entry };
            if is_drop {
                if let Some(user) = user_drops.get(ty) {
                    match generator.layouts.live_offset(ty) {
                        Some(offset) => {
                            let live = generator.load(generator.i8().into(), address, offset as i64).into_int_value();
                            let call = generator.block(function);
                            let contents = generator.block(function);
                            generator.branch_if(live, call, contents);
                            generator.builder.position_at_end(call);
                            generator.call(*user, &[address.into()]);
                            generator.builder.build_unconditional_branch(contents).expect("br");
                            generator.builder.position_at_end(contents);
                        }
                        None => {
                            generator.call(*user, &[address.into()]);
                        }
                    }
                }
                emitter.drop_contents(ty, address);
            } else {
                emitter.clone_contents(ty, address);
            }
            generator.builder.build_return(None).expect("ret");
        }
    }
}

struct Emitter<'g, 'ctx, 'm, 'a> {
    generator: &'g Generator<'ctx, 'm, 'a>,
    function: FunctionValue<'ctx>,
    entry: inkwell::basic_block::BasicBlock<'ctx>,
}

impl<'ctx> Emitter<'_, 'ctx, '_, '_> {
    fn glue_fn(&self, ty: &Type, is_drop: bool) -> Option<FunctionValue<'ctx>> {
        self.generator.glue.get(ty).map(|&(drop, clone)| if is_drop { drop } else { clone })
    }

    fn each_owned_field(&self, fields: Vec<(Type, u64)>, address: PointerValue<'ctx>, is_drop: bool) {
        let g = self.generator;
        for (field_ty, offset) in fields {
            if let Some(function) = self.glue_fn(&field_ty, is_drop) {
                g.call(function, &[g.offset(address, offset as i64).into()]);
            }
        }
    }

    fn each_variant(&self, ty: &Type, address: PointerValue<'ctx>, is_drop: bool) {
        let g = self.generator;
        let Type::Enum(name, args) = ty else { return };
        let tag = g.load(g.i64().into(), address, 0).into_int_value();
        let exit = g.block(self.function);
        for (index, fields) in g.layouts.enum_variants(name, args) {
            if !fields.iter().any(|(field_ty, _)| g.glue.contains_key(field_ty)) {
                continue;
            }
            let matched = g.block(self.function);
            let next = g.block(self.function);
            let is_variant = g.builder.build_int_compare(IntPredicate::EQ, tag, g.int(index as i64), "").expect("icmp");
            g.builder.build_conditional_branch(is_variant, matched, next).expect("br");
            g.builder.position_at_end(matched);
            self.each_owned_field(fields, address, is_drop);
            g.builder.build_unconditional_branch(exit).expect("br");
            g.builder.position_at_end(next);
        }
        g.builder.build_unconditional_branch(exit).expect("br");
        g.builder.position_at_end(exit);
    }

    fn each_element(&self, data: PointerValue<'ctx>, len: IntValue<'ctx>, elem_size: u64, function: FunctionValue<'ctx>) {
        let g = self.generator;
        let counter = g.entry_alloca(self.entry, 8, 8);
        g.store(counter, 0, g.int(0).into());
        let header = g.block(self.function);
        let body = g.block(self.function);
        let exit = g.block(self.function);
        g.builder.build_unconditional_branch(header).expect("br");
        g.builder.position_at_end(header);
        let current = g.load(g.i64().into(), counter, 0).into_int_value();
        let more = g.builder.build_int_compare(IntPredicate::SLT, current, len, "").expect("icmp");
        g.builder.build_conditional_branch(more, body, exit).expect("br");
        g.builder.position_at_end(body);
        let offset = g.builder.build_int_mul(current, g.int(elem_size as i64), "").expect("mul");
        g.call(function, &[g.offset_by(data, offset).into()]);
        let next = g.builder.build_int_add(current, g.int(1), "").expect("add");
        g.store(counter, 0, next.into());
        g.builder.build_unconditional_branch(header).expect("br");
        g.builder.position_at_end(exit);
    }

    /// Adjusts the reference count of the non-null shared block `counted`
    /// points at, calling `release` when a drop brings it to zero.
    fn refcounted(&self, pointer: PointerValue<'ctx>, header_offset: i64, is_drop: bool, release: impl FnOnce(PointerValue<'ctx>)) {
        let g = self.generator;
        let live = g.block(self.function);
        let exit = g.block(self.function);
        let is_live = g.builder.build_is_not_null(pointer, "").expect("icmp");
        g.builder.build_conditional_branch(is_live, live, exit).expect("br");
        g.builder.position_at_end(live);
        let header = g.offset(pointer, header_offset);
        if is_drop {
            let previous = g.atomic_add(header, -1);
            let last = g.builder.build_int_compare(IntPredicate::EQ, previous, g.int(1), "").expect("icmp");
            let release_block = g.block(self.function);
            g.builder.build_conditional_branch(last, release_block, exit).expect("br");
            g.builder.position_at_end(release_block);
            release(header);
        } else {
            g.atomic_add(header, 1);
        }
        g.builder.build_unconditional_branch(exit).expect("br");
        g.builder.position_at_end(exit);
    }

    fn drop_contents(&self, ty: &Type, address: PointerValue<'ctx>) {
        let g = self.generator;
        if let Some((_, release)) = handle_fns(ty, g.layouts) {
            let handle = g.load(g.i64().into(), address, 0);
            g.call_runtime(release, &[handle], None);
            return;
        }
        match ty {
            Type::Fn(..) => {
                let env = g.as_ptr(g.load(g.ptr().into(), address, 0));
                self.refcounted(env, -CLOSURE_HEADER, true, |header| {
                    let drop_fn = g.as_ptr(g.load(g.ptr().into(), env, i64::from(CLOSURE_DROP_FN)));
                    let drop_captures = g.block(self.function);
                    let free_env = g.block(self.function);
                    let has_drop = g.builder.build_is_not_null(drop_fn, "").expect("icmp");
                    g.builder.build_conditional_branch(has_drop, drop_captures, free_env).expect("br");
                    g.builder.position_at_end(drop_captures);
                    let glue_type = g.context.void_type().fn_type(&[g.ptr().into()], false);
                    g.builder.build_indirect_call(glue_type, drop_fn, &[env.into()], "").expect("call");
                    g.builder.build_unconditional_branch(free_env).expect("br");
                    g.builder.position_at_end(free_env);
                    g.call_runtime("paco_free", &[header.into()], None);
                });
            }
            Type::String | Type::Slice(_) => {
                let data = g.as_ptr(g.load(g.ptr().into(), address, 0));
                if let Type::Slice(elem) = ty
                    && let Some(elem_drop) = self.glue_fn(elem, true)
                {
                    let len = g.load(g.i64().into(), address, 8).into_int_value();
                    self.each_element(data, len, element_size(elem, g.layouts), elem_drop);
                }
                g.call_runtime("paco_free", &[data.into()], None);
            }
            _ if is_cell(ty, g.layouts) => {
                let cell = g.as_ptr(g.load(g.ptr().into(), address, 0));
                self.refcounted(cell, 0, true, |cell| {
                    if let Type::Struct(_, args) = ty
                        && let Some(inner_drop) = args.first().and_then(|inner| self.glue_fn(inner, true))
                    {
                        g.call(inner_drop, &[g.offset(cell, i64::from(CELL_VALUE_OFFSET)).into()]);
                    }
                    g.call_runtime("paco_free", &[cell.into()], None);
                });
            }
            Type::Enum(..) => self.each_variant(ty, address, true),
            _ => self.each_owned_field(glue_fields(ty, g.layouts), address, true),
        }
    }

    fn clone_contents(&self, ty: &Type, address: PointerValue<'ctx>) {
        let g = self.generator;
        if let Some((retain, _)) = handle_fns(ty, g.layouts) {
            let handle = g.load(g.i64().into(), address, 0);
            g.call_runtime(retain, &[handle], None);
            return;
        }
        match ty {
            Type::Fn(..) => {
                let env = g.as_ptr(g.load(g.ptr().into(), address, 0));
                self.refcounted(env, -CLOSURE_HEADER, false, |_| {});
            }
            Type::String | Type::Slice(_) => {
                let data = g.as_ptr(g.load(g.ptr().into(), address, 0));
                let len = g.load(g.i64().into(), address, 8).into_int_value();
                let elem_size = match ty {
                    Type::Slice(elem) => element_size(elem, g.layouts),
                    _ => 1,
                };
                let bytes = g.builder.build_int_mul(len, g.int(elem_size as i64), "").expect("mul");
                let copy = g.as_ptr(g.call_runtime("paco_alloc", &[bytes.into()], Some(g.ptr().into())).expect("paco_alloc"));
                g.builder.build_memcpy(copy, 1, data, 1, bytes).expect("memcpy");
                g.store(address, 0, copy.into());
                if let Type::Slice(elem) = ty
                    && let Some(elem_clone) = self.glue_fn(elem, false)
                {
                    self.each_element(copy, len, elem_size, elem_clone);
                }
            }
            _ if is_cell(ty, g.layouts) => {
                let cell = g.as_ptr(g.load(g.ptr().into(), address, 0));
                self.refcounted(cell, 0, false, |_| {});
            }
            Type::Enum(..) => self.each_variant(ty, address, false),
            _ => self.each_owned_field(glue_fields(ty, g.layouts), address, false),
        }
    }
}
