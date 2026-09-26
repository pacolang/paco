//! `Rvalue::Ref` codegen for a scalar (non-struct/enum) local: previously
//! an unconditional `panic!("borrow codegen is not implemented yet")`,
//! needed as a prerequisite for `concurrency-codegen`'s FFI calls (passing
//! the address of a local as an output-pointer argument). No Paco source
//! syntax exercises this directly yet (`*x` raw-pointer deref is a
//! separate, still-unimplemented gap — `paco-mir::lower`'s own
//! `AstUnOp::Deref` arm still panics), so this hand-builds MIR directly,
//! matching `paco-link/tests/link.rs`'s hand-built-object pattern: store a
//! known value into a local, take its address, return the address itself
//! as the function's result, and dereference it from the Rust test side to
//! confirm it is the local's real, live memory address.

use std::mem;

use cranelift_codegen::settings::Configurable;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::default_libcall_names;
use paco_mir::{
    BasicBlock, Body, Constant, Local, LocalDecl, Operand, Place, Profile, Rvalue, Statement,
    Terminator, TypeLayouts,
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

#[test]
fn taking_the_address_of_a_scalar_local_returns_its_real_memory_address() {
    // fn main() -> i64 {
    //     let x: i64 = 42;      // local 0
    //     let p: i64 = &x;      // local 1, holds x's address as a raw i64
    //     return p;
    // }
    let x = Local(0);
    let p = Local(1);
    let body = Body {
        locals: vec![
            LocalDecl { name: Some("x".to_string()), ty: Type::Int(IntWidth::I64), mutable: false },
            LocalDecl { name: Some("p".to_string()), ty: Type::Int(IntWidth::I64), mutable: false },
        ],
        blocks: vec![BasicBlock {
            statements: vec![
                Statement::Assign(
                    Place::Local(x),
                    Rvalue::Use(Operand::Constant(Constant::Int(42, IntWidth::I64))),
                ),
                Statement::Assign(
                    Place::Local(p),
                    Rvalue::Ref { mutable: false, place: Place::Local(x) },
                ),
            ],
            terminator: Terminator::Return(Operand::Copy(Place::Local(p))),
        }],
        profile: Profile::Debug,
        param_count: 0,
        return_ty: Type::Int(IntWidth::I64),
        span: paco_span::Span::new_root(0, 0),
        spans: Vec::new(),
    };

    let mut module = jit_module();
    let empty_module = paco_syntax::ast::Module {
        name: None,
        items: Vec::new(),
        span: paco_span::Span::new_root(0, 0),
    };
    let layouts = TypeLayouts::from_module(&empty_module);
    let func_ids =
        paco_codegen_cranelift::declare_and_define(&mut module, &[("main".to_string(), body)], &[], &layouts)
            .unwrap();
    module.finalize_definitions().unwrap();

    let main_id = func_ids["main"];
    let code = module.get_finalized_function(main_id);
    let main_fn = unsafe { mem::transmute::<*const u8, extern "C" fn() -> i64>(code) };
    let address = main_fn();

    let value = unsafe { *(address as *const i64) };
    assert_eq!(value, 42, "the returned address should be `x`'s real, live memory address");
}
