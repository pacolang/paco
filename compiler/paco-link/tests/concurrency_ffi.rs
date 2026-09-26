//! Confirms `paco-link` locates and links against the pre-built
//! `paco-runtime-ffi` static library, by hand-building an object file that
//! calls `paco_rt_spawn`/`paco_rt_join` directly — no `paco-mir`/codegen
//! involved yet, matching `link.rs`'s existing hand-built-object pattern.

use std::path::PathBuf;
use std::process::Command;

use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, UserFuncName, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_link_{name}_{}_{}.{extension}",
        std::process::id(),
        monotonic_suffix()
    ));
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[test]
fn links_a_hand_built_object_calling_paco_rt_spawn_and_join() {
    let isa = paco_codegen_cranelift::host_isa().expect("host isa");
    let pointer_type = isa.pointer_type();
    let object_builder = ObjectBuilder::new(isa, "spawn_ffi", default_libcall_names()).unwrap();
    let mut module = ObjectModule::new(object_builder);

    let spawn_sig = {
        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(pointer_type)); // thunk fn ptr
        sig.params.push(AbiParam::new(pointer_type)); // captures ptr
        sig.params.push(AbiParam::new(pointer_type)); // captures_len
        sig.params.push(AbiParam::new(pointer_type)); // result_len
        sig.returns.push(AbiParam::new(pointer_type)); // handle
        sig
    };
    let spawn_func = module.declare_function("paco_rt_spawn", Linkage::Import, &spawn_sig).unwrap();

    let join_sig = {
        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(pointer_type)); // handle
        sig.params.push(AbiParam::new(pointer_type)); // result_out ptr
        sig.params.push(AbiParam::new(pointer_type)); // result_len
        sig.returns.push(AbiParam::new(types::I32)); // status
        sig
    };
    let join_func = module.declare_function("paco_rt_join", Linkage::Import, &join_sig).unwrap();

    // The spawned thunk: fn(captures_ptr, result_out) — writes a known
    // pattern (42) into `result_out` as an `i64`, ignoring captures.
    let thunk_sig = {
        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(pointer_type));
        sig.params.push(AbiParam::new(pointer_type));
        sig
    };
    let thunk_func = module.declare_function("thunk", Linkage::Export, &thunk_sig).unwrap();
    {
        let mut ctx = module.make_context();
        ctx.func.signature = thunk_sig;
        ctx.func.name = UserFuncName::user(0, thunk_func.as_u32());
        let mut fb_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
        let block = builder.create_block();
        builder.append_block_params_for_function_params(block);
        builder.switch_to_block(block);
        let result_out = builder.block_params(block)[1];
        let value = builder.ins().iconst(types::I64, 42);
        builder.ins().store(MemFlagsData::trusted(), value, result_out, 0);
        builder.ins().return_(&[]);
        builder.seal_all_blocks();
        builder.finalize(module.target_config());
        module.define_function(thunk_func, &mut ctx).unwrap();
        module.clear_context(&mut ctx);
    }

    // __paco_entry: spawn(thunk), join it, return the joined value as the
    // process exit code — the runtime's `paco_rt_main` calls `paco_rt_init`
    // before this function runs.
    let entry_sig = {
        let mut sig = module.make_signature();
        sig.returns.push(AbiParam::new(types::I64));
        sig
    };
    let entry_func = module.declare_function(paco_mir::ENTRY_SYMBOL, Linkage::Export, &entry_sig).unwrap();
    {
        let mut ctx = module.make_context();
        ctx.func.signature = entry_sig;
        ctx.func.name = UserFuncName::user(0, entry_func.as_u32());
        let mut fb_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);

        let spawn_ref = module.declare_func_in_func(spawn_func, builder.func);
        let join_ref = module.declare_func_in_func(join_func, builder.func);
        let thunk_ref = module.declare_func_in_func(thunk_func, builder.func);

        let block = builder.create_block();
        builder.switch_to_block(block);

        let result_slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let result_ptr = builder.ins().stack_addr(pointer_type, result_slot, 0);

        let thunk_addr = builder.ins().func_addr(pointer_type, thunk_ref);
        let null = builder.ins().iconst(pointer_type, 0);
        let zero_len = builder.ins().iconst(pointer_type, 0);
        let eight = builder.ins().iconst(pointer_type, 8);

        let spawn_call = builder.ins().call(spawn_ref, &[thunk_addr, null, zero_len, eight]);
        let handle = builder.inst_results(spawn_call)[0];

        builder.ins().call(join_ref, &[handle, result_ptr, eight]);

        let value = builder.ins().load(types::I64, MemFlagsData::trusted(), result_ptr, 0);
        builder.ins().return_(&[value]);
        builder.seal_all_blocks();
        builder.finalize(module.target_config());
        module.define_function(entry_func, &mut ctx).unwrap();
        module.clear_context(&mut ctx);
    }

    let flag = module.declare_data(paco_mir::ENTRY_RETURNS_VALUE_SYMBOL, Linkage::Export, false, false).unwrap();
    let mut description = cranelift_module::DataDescription::new();
    description.define(Box::new([1]));
    module.define_data(flag, &description).unwrap();

    let product = module.finish();
    let bytes = product.emit().unwrap();
    let object_path = temp_path("spawn_ffi", "o");
    std::fs::write(&object_path, bytes).unwrap();

    let exe_path = temp_path("spawn_ffi", "exe");
    paco_link::link_program(&paco_link::LinkRequest {
        objects: &[object_path],
        output: &exe_path,
        mode: paco_link::LinkMode::Static,
        extra_libs: &[],
        target: &crate::link::native_target(),
        sysroot: None,
        debug: false,
    }).expect("linking should succeed");

    let status = Command::new(&exe_path).status().expect("compiled binary should run");
    assert_eq!(status.code().expect("process should exit normally"), 42);
}
