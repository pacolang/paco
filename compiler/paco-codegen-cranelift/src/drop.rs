//! Per-type drop and clone glue: one `(ptr) -> ()` function per owning type.
//! Drop releases what the value at `ptr` owns; clone fixes up a fresh
//! bitwise copy at `ptr` so it owns its own heap storage.

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{AbiParam, AtomicRmwOp, InstBuilder, MemFlagsData, Signature, UserFuncName, Value, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module};

use paco_mir::glue::{CELL_VALUE_OFFSET, CLOSURE_DROP_FN, CLOSURE_HEADER, glue_fields, handle_fns, is_cell, owning_types};
use paco_mir::{Body, TypeLayouts};
use paco_types::Type;

#[derive(Default)]
pub struct Glue {
    fns: HashMap<Type, (FuncId, FuncId)>,
}

impl Glue {
    pub(crate) fn needs_drop(&self, ty: &Type) -> bool {
        self.fns.contains_key(ty)
    }

    pub(crate) fn all(&self) -> impl Iterator<Item = (&Type, &(FuncId, FuncId))> {
        self.fns.iter()
    }
}

fn glue_signature<M: Module>(module: &M) -> Signature {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig
}

/// Declares and defines drop/clone glue for every owning type reachable
/// from `bodies`. `user_drops` maps a type to its `fn drop(&mut self)`.
pub fn build_glue<M: Module>(
    module: &mut M,
    bodies: &[(String, Body)],
    layouts: &TypeLayouts<'_>,
    user_drops: &HashMap<Type, FuncId>,
) -> Result<Glue, String> {
    let owning = owning_types(bodies, layouts, user_drops);

    let sig = glue_signature(module);
    let mut glue = Glue::default();
    for (index, ty) in owning.iter().enumerate() {
        let drop_id = module
            .declare_function(&format!("__paco_drop_{index}"), Linkage::Local, &sig)
            .map_err(|error| error.to_string())?;
        let clone_id = module
            .declare_function(&format!("__paco_clone_{index}"), Linkage::Local, &sig)
            .map_err(|error| error.to_string())?;
        glue.fns.insert(ty.clone(), (drop_id, clone_id));
    }

    let mut libc_sig = module.make_signature();
    libc_sig.params.push(AbiParam::new(types::I64));
    let free = module.declare_function("paco_free", Linkage::Import, &libc_sig).map_err(|error| error.to_string())?;
    libc_sig.returns.push(AbiParam::new(types::I64));
    let malloc = module.declare_function("paco_alloc", Linkage::Import, &libc_sig).map_err(|error| error.to_string())?;

    let mut handles = HashMap::new();
    for ty in &owning {
        if let Some((retain, release)) = handle_fns(ty, layouts) {
            let mut declare = |name: &str| {
                module.declare_function(name, Linkage::Import, &sig).map_err(|error| error.to_string())
            };
            handles.insert(ty.clone(), (declare(retain)?, declare(release)?));
        }
    }

    let mut ctx = module.make_context();
    let mut fb_ctx = FunctionBuilderContext::new();
    for ty in &owning {
        let (drop_id, clone_id) = glue.fns[ty];
        for (func_id, is_drop) in [(drop_id, true), (clone_id, false)] {
            ctx.func.signature = sig.clone();
            ctx.func.name = UserFuncName::user(0, func_id.as_u32());
            {
                let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
                let mut emitter = Emitter { module, glue: &glue, layouts, free, malloc, handles: &handles };
                let entry = builder.create_block();
                builder.append_block_params_for_function_params(entry);
                builder.switch_to_block(entry);
                let addr = builder.block_params(entry)[0];
                if is_drop {
                    if let Some(user) = user_drops.get(ty) {
                        match layouts.live_offset(ty) {
                            Some(offset) => {
                                let live = builder.ins().load(types::I8, MemFlagsData::trusted(), addr, offset as i32);
                                let call = builder.create_block();
                                let contents = builder.create_block();
                                builder.ins().brif(live, call, &[], contents, &[]);
                                builder.switch_to_block(call);
                                emitter.call(&mut builder, *user, &[addr]);
                                builder.ins().jump(contents, &[]);
                                builder.switch_to_block(contents);
                            }
                            None => {
                                emitter.call(&mut builder, *user, &[addr]);
                            }
                        }
                    }
                    emitter.drop_contents(&mut builder, ty, addr);
                } else {
                    emitter.clone_contents(&mut builder, ty, addr);
                }
                builder.ins().return_(&[]);
                builder.seal_all_blocks();
                builder.finalize(module.target_config());
            }
            module.define_function(func_id, &mut ctx).map_err(|error| format!("failed to define glue: {error}"))?;
            module.clear_context(&mut ctx);
        }
    }
    Ok(glue)
}

struct Emitter<'a, 'l, M: Module> {
    module: &'a mut M,
    glue: &'a Glue,
    layouts: &'a TypeLayouts<'l>,
    free: FuncId,
    malloc: FuncId,
    handles: &'a HashMap<Type, (FuncId, FuncId)>,
}

impl<M: Module> Emitter<'_, '_, M> {
    fn call(&mut self, builder: &mut FunctionBuilder, func_id: FuncId, args: &[Value]) -> Option<Value> {
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(func_ref, args);
        builder.inst_results(call).first().copied()
    }

    fn each_owned_field(
        &mut self,
        builder: &mut FunctionBuilder,
        fields: Vec<(Type, u64)>,
        addr: Value,
        is_drop: bool,
    ) {
        for (field_ty, offset) in fields {
            if let Some(&(drop_id, clone_id)) = self.glue.fns.get(&field_ty) {
                let field_addr = builder.ins().iadd_imm_s(addr, offset as i64);
                self.call(builder, if is_drop { drop_id } else { clone_id }, &[field_addr]);
            }
        }
    }

    fn each_variant(&mut self, builder: &mut FunctionBuilder, ty: &Type, addr: Value, is_drop: bool) {
        let Type::Enum(name, args) = ty else { return };
        let variants = self.layouts.enum_variants(name, args);
        let tag = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
        let exit = builder.create_block();
        for (index, fields) in variants {
            if !fields.iter().any(|(field_ty, _)| self.glue.needs_drop(field_ty)) {
                continue;
            }
            let matched = builder.create_block();
            let next = builder.create_block();
            let is_variant = builder.ins().icmp_imm_s(IntCC::Equal, tag, index as i64);
            builder.ins().brif(is_variant, matched, &[], next, &[]);
            builder.switch_to_block(matched);
            self.each_owned_field(builder, fields, addr, is_drop);
            builder.ins().jump(exit, &[]);
            builder.switch_to_block(next);
        }
        builder.ins().jump(exit, &[]);
        builder.switch_to_block(exit);
    }

    fn each_element(
        &mut self,
        builder: &mut FunctionBuilder,
        data: Value,
        len: Value,
        elem_size: u64,
        func_id: FuncId,
    ) {
        let index = builder.declare_var(types::I64);
        let zero = builder.ins().iconst(types::I64, 0);
        builder.def_var(index, zero);
        let header = builder.create_block();
        let body = builder.create_block();
        let exit = builder.create_block();
        builder.ins().jump(header, &[]);
        builder.switch_to_block(header);
        let current = builder.use_var(index);
        let more = builder.ins().icmp(IntCC::SignedLessThan, current, len);
        builder.ins().brif(more, body, &[], exit, &[]);
        builder.switch_to_block(body);
        let offset = builder.ins().imul_imm_s(current, elem_size as i64);
        let elem = builder.ins().iadd(data, offset);
        self.call(builder, func_id, &[elem]);
        let next = builder.ins().iadd_imm_s(current, 1);
        builder.def_var(index, next);
        builder.ins().jump(header, &[]);
        builder.switch_to_block(exit);
    }

    fn element_size(&self, elem: &Type) -> u64 {
        paco_mir::glue::element_size(elem, self.layouts)
    }

    fn drop_contents(&mut self, builder: &mut FunctionBuilder, ty: &Type, addr: Value) {
        if let Some(&(_, release)) = self.handles.get(ty) {
            let handle = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
            self.call(builder, release, &[handle]);
            return;
        }
        match ty {
            Type::Fn(..) => {
                let env = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
                let live = builder.create_block();
                let release = builder.create_block();
                let exit = builder.create_block();
                builder.ins().brif(env, live, &[], exit, &[]);
                builder.switch_to_block(live);
                let header = builder.ins().iadd_imm_s(env, -CLOSURE_HEADER);
                let one = builder.ins().iconst(types::I64, 1);
                let previous = builder.ins().atomic_rmw(types::I64, MemFlagsData::trusted(), AtomicRmwOp::Sub, header, one);
                let last = builder.ins().icmp_imm_s(IntCC::Equal, previous, 1);
                builder.ins().brif(last, release, &[], exit, &[]);
                builder.switch_to_block(release);
                let drop_fn = builder.ins().load(types::I64, MemFlagsData::trusted(), env, CLOSURE_DROP_FN);
                let drop_captures = builder.create_block();
                let free_env = builder.create_block();
                builder.ins().brif(drop_fn, drop_captures, &[], free_env, &[]);
                builder.switch_to_block(drop_captures);
                let sig = builder.import_signature(glue_signature(self.module));
                builder.ins().call_indirect(sig, drop_fn, &[env]);
                builder.ins().jump(free_env, &[]);
                builder.switch_to_block(free_env);
                self.call(builder, self.free, &[header]);
                builder.ins().jump(exit, &[]);
                builder.switch_to_block(exit);
            }
            Type::String | Type::Slice(_) => {
                let data = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
                if let Type::Slice(elem) = ty
                    && let Some(&(elem_drop, _)) = self.glue.fns.get(elem.as_ref())
                {
                    let len = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 8);
                    let size = self.element_size(elem);
                    self.each_element(builder, data, len, size, elem_drop);
                }
                self.call(builder, self.free, &[data]);
            }
            _ if is_cell(ty, self.layouts) => {
                let cell = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
                let live = builder.create_block();
                let release = builder.create_block();
                let exit = builder.create_block();
                builder.ins().brif(cell, live, &[], exit, &[]);
                builder.switch_to_block(live);
                let one = builder.ins().iconst(types::I64, 1);
                let previous = builder.ins().atomic_rmw(types::I64, MemFlagsData::trusted(), AtomicRmwOp::Sub, cell, one);
                let last = builder.ins().icmp_imm_s(IntCC::Equal, previous, 1);
                builder.ins().brif(last, release, &[], exit, &[]);
                builder.switch_to_block(release);
                if let Type::Struct(_, args) = ty
                    && let Some(&(inner_drop, _)) = args.first().and_then(|inner| self.glue.fns.get(inner))
                {
                    let value = builder.ins().iadd_imm_s(cell, i64::from(CELL_VALUE_OFFSET));
                    self.call(builder, inner_drop, &[value]);
                }
                self.call(builder, self.free, &[cell]);
                builder.ins().jump(exit, &[]);
                builder.switch_to_block(exit);
            }
            Type::Enum(..) => self.each_variant(builder, ty, addr, true),
            _ => {
                let fields = glue_fields(ty, self.layouts);
                self.each_owned_field(builder, fields, addr, true);
            }
        }
    }

    fn clone_contents(&mut self, builder: &mut FunctionBuilder, ty: &Type, addr: Value) {
        if let Some(&(retain, _)) = self.handles.get(ty) {
            let handle = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
            self.call(builder, retain, &[handle]);
            return;
        }
        match ty {
            Type::Fn(..) => {
                let env = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
                let live = builder.create_block();
                let exit = builder.create_block();
                builder.ins().brif(env, live, &[], exit, &[]);
                builder.switch_to_block(live);
                let header = builder.ins().iadd_imm_s(env, -CLOSURE_HEADER);
                let one = builder.ins().iconst(types::I64, 1);
                builder.ins().atomic_rmw(types::I64, MemFlagsData::trusted(), AtomicRmwOp::Add, header, one);
                builder.ins().jump(exit, &[]);
                builder.switch_to_block(exit);
            }
            Type::String | Type::Slice(_) => {
                let data = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
                let len = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 8);
                let elem_size = match ty {
                    Type::Slice(elem) => self.element_size(elem),
                    _ => 1,
                };
                let bytes = builder.ins().imul_imm_s(len, elem_size as i64);
                let copy = self.call(builder, self.malloc, &[bytes]).expect("malloc returns a pointer");
                let config = self.module.target_config();
                builder.call_memcpy(config, copy, data, bytes);
                builder.ins().store(MemFlagsData::trusted(), copy, addr, 0);
                if let Type::Slice(elem) = ty
                    && let Some(&(_, elem_clone)) = self.glue.fns.get(elem.as_ref())
                {
                    self.each_element(builder, copy, len, elem_size, elem_clone);
                }
            }
            _ if is_cell(ty, self.layouts) => {
                let cell = builder.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
                let live = builder.create_block();
                let exit = builder.create_block();
                builder.ins().brif(cell, live, &[], exit, &[]);
                builder.switch_to_block(live);
                let one = builder.ins().iconst(types::I64, 1);
                builder.ins().atomic_rmw(types::I64, MemFlagsData::trusted(), AtomicRmwOp::Add, cell, one);
                builder.ins().jump(exit, &[]);
                builder.switch_to_block(exit);
            }
            Type::Enum(..) => self.each_variant(builder, ty, addr, false),
            _ => {
                let fields = glue_fields(ty, self.layouts);
                self.each_owned_field(builder, fields, addr, false);
            }
        }
    }
}
