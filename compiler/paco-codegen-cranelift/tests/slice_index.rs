//! `arrays-and-slices` task 4.3: codegen for `[]T` indexed reads/writes
//! with a bounds check. `slice_of_zeros`'s own codegen (real heap
//! allocation) isn't implemented yet — a separate, deferred decision (see
//! task 5.2's note) — so this hand-builds a slice descriptor directly in
//! MIR, matching `borrow_of_scalar.rs`'s established pattern for exercising
//! codegen that has no real `.paco` source path ready yet.

use std::mem;

use cranelift_codegen::Context;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use paco_mir::{
    BasicBlock, BasicBlockId, Body, CallTarget, Constant, Local, LocalDecl, Operand, Place, Profile, Rvalue,
    Statement, Terminator, TypeLayouts,
};
use paco_types::{IntWidth, Type};

fn jit_module() -> JITModule {
    let mut flag_builder = cranelift_codegen::settings::builder();
    flag_builder.set("is_pic", "false").unwrap();
    let isa_builder = cranelift_native::builder().expect("host isa");
    let isa = isa_builder
        .finish(cranelift_codegen::settings::Flags::new(flag_builder))
        .unwrap();
    let mut builder = crate::with_panic_symbols(JITBuilder::with_isa(isa, default_libcall_names()));
    builder.symbols([
        ("paco_alloc", paco_runtime_ffi::paco_alloc as *const u8),
        ("paco_calloc", paco_runtime_ffi::paco_calloc as *const u8),
        ("paco_realloc", paco_runtime_ffi::paco_realloc as *const u8),
        ("paco_free", paco_runtime_ffi::paco_free as *const u8),
    ]);
    JITModule::new(builder)
}

fn empty_layouts() -> TypeLayouts<'static> {
    let module = Box::leak(Box::new(paco_syntax::ast::Module {
        name: None,
        items: Vec::new(),
        span: paco_span::Span::new_root(0, 0),
    }));
    TypeLayouts::from_module(module)
}

/// `fn main() -> i64` that fills a heap-owned 4-element `[]i64` `buf`
/// (`Local(2)`) with `[10, 20, 30, 40]`, runs `extra_statements`, then
/// returns `x` (`Local(4)`). Locals 0, 1 and 3 are scratch.
fn build_and_run(extra_statements: impl FnOnce(Local, Local, Local, Local, Local) -> Vec<Statement>) -> i64 {
    let data = Local(0);
    let desc = Local(1);
    let buf = Local(2);
    let addr_tmp = Local(3);
    let x = Local(4);

    let elem_ty = Type::Int(IntWidth::I64);
    let slice_ty = Type::Slice(Box::new(elem_ty.clone()));

    let mut statements = Vec::new();
    for (i, value) in [10i64, 20, 30, 40].into_iter().enumerate() {
        statements.push(Statement::Assign(
            Place::Index {
                base: Box::new(Place::Local(buf)),
                index: Box::new(Operand::Constant(Constant::Int(i as i64, IntWidth::I64))),
            },
            Rvalue::Use(Operand::Constant(Constant::Int(value, IntWidth::I64))),
        ));
    }
    statements.extend(extra_statements(data, desc, buf, addr_tmp, x));
    let allocate = BasicBlock {
        statements: Vec::new(),
        terminator: Terminator::Call {
            target: CallTarget("slice_of_zeros".to_string()),
            args: vec![Operand::Constant(Constant::Int(4, IntWidth::I64))],
            destination: Some(Place::Local(buf)),
            resume: BasicBlockId(1),
        },
    };

    let body = Body {
        locals: vec![
            LocalDecl { name: Some("data".to_string()), ty: Type::Int(IntWidth::I64), mutable: false },
            LocalDecl { name: Some("desc".to_string()), ty: Type::Int(IntWidth::I64), mutable: false },
            LocalDecl { name: Some("buf".to_string()), ty: slice_ty, mutable: true },
            LocalDecl { name: Some("addr_tmp".to_string()), ty: Type::Int(IntWidth::I64), mutable: false },
            LocalDecl { name: Some("x".to_string()), ty: Type::Int(IntWidth::I64), mutable: true },
        ],
        blocks: vec![allocate, BasicBlock { statements, terminator: Terminator::Return(Operand::Copy(Place::Local(x))) }],
        profile: Profile::Debug,
        param_count: 0,
        return_ty: Type::Int(IntWidth::I64),
        span: paco_span::Span::new_root(0, 0),
        spans: Vec::new(),
    };

    let mut module = jit_module();
    let layouts = empty_layouts();
    let func_ids =
        paco_codegen_cranelift::declare_and_define(&mut module, &[("main".to_string(), body)], &[], &layouts).unwrap();
    module.finalize_definitions().unwrap();
    let main_id = func_ids["main"];
    let code = module.get_finalized_function(main_id);
    let main_fn = unsafe { mem::transmute::<*const u8, extern "C" fn() -> i64>(code) };
    main_fn()
}

#[test]
fn indexing_an_in_bounds_element_reads_the_stored_value() {
    let result = build_and_run(|_data, _desc, buf, _addr_tmp, x| {
        vec![Statement::Assign(
            Place::Local(x),
            Rvalue::Use(Operand::Copy(Place::Index {
                base: Box::new(Place::Local(buf)),
                index: Box::new(Operand::Constant(Constant::Int(2, IntWidth::I64))),
            })),
        )]
    });
    assert_eq!(result, 30);
}

#[test]
fn writing_through_an_index_place_then_reading_it_back_round_trips() {
    let result = build_and_run(|_data, _desc, buf, _addr_tmp, x| {
        vec![
            Statement::Assign(
                Place::Index {
                    base: Box::new(Place::Local(buf)),
                    index: Box::new(Operand::Constant(Constant::Int(1, IntWidth::I64))),
                },
                Rvalue::Use(Operand::Constant(Constant::Int(99, IntWidth::I64))),
            ),
            Statement::Assign(
                Place::Local(x),
                Rvalue::Use(Operand::Copy(Place::Index {
                    base: Box::new(Place::Local(buf)),
                    index: Box::new(Operand::Constant(Constant::Int(1, IntWidth::I64))),
                })),
            ),
        ]
    });
    assert_eq!(result, 99);
}

#[test]
fn slice_indexing_emits_an_unconditional_bounds_check_panic() {
    let data = Local(0);
    let desc = Local(1);
    let buf = Local(2);
    let addr_tmp = Local(3);
    let x = Local(4);
    let elem_ty = Type::Int(IntWidth::I64);
    let slice_ty = Type::Slice(Box::new(elem_ty.clone()));

    let body = Body {
        locals: vec![
            LocalDecl { name: Some("data".to_string()), ty: Type::Int(IntWidth::I64), mutable: false },
            LocalDecl { name: Some("desc".to_string()), ty: Type::Int(IntWidth::I64), mutable: false },
            LocalDecl { name: Some("buf".to_string()), ty: slice_ty, mutable: false },
            LocalDecl { name: Some("addr_tmp".to_string()), ty: Type::Int(IntWidth::I64), mutable: false },
            LocalDecl { name: Some("x".to_string()), ty: elem_ty.clone(), mutable: false },
        ],
        blocks: vec![BasicBlock {
            statements: vec![
                Statement::Assign(Place::Local(desc), Rvalue::RawAlloc { size: 16 }),
                Statement::Assign(Place::Local(buf), Rvalue::Use(Operand::Copy(Place::Local(desc)))),
                Statement::Assign(
                    Place::Local(x),
                    Rvalue::Use(Operand::Copy(Place::Index {
                        base: Box::new(Place::Local(buf)),
                        index: Box::new(Operand::Copy(Place::Local(addr_tmp))),
                    })),
                ),
            ],
            terminator: Terminator::Return(Operand::Copy(Place::Local(x))),
        }],
        profile: Profile::Debug,
        param_count: 0,
        return_ty: elem_ty,
        span: paco_span::Span::new_root(0, 0),
        spans: Vec::new(),
    };

    let mut jit_module = jit_module();
    let layouts = empty_layouts();
    let sig = {
        let mut sig = jit_module.make_signature();
        sig.returns.push(cranelift_codegen::ir::AbiParam::new(cranelift_codegen::ir::types::I64));
        sig
    };
    let func_id = jit_module.declare_function("main", Linkage::Export, &sig).unwrap();
    let mut ctx = Context::new();
    ctx.func.signature = sig;
    ctx.func.name = cranelift_codegen::ir::UserFuncName::user(0, func_id.as_u32());
    let mut fb_ctx = FunctionBuilderContext::new();
    let func_ids = std::collections::HashMap::from([("main".to_string(), func_id)]);
    paco_codegen_cranelift::compile_function(&mut jit_module, &mut ctx, &mut fb_ctx, &body, &func_ids, &layouts);

    let ir = ctx.func.to_string();
    assert!(ir.contains("cold:") && ir.contains("call fn2"), "{ir}");
    let _ = (data, addr_tmp);
}
