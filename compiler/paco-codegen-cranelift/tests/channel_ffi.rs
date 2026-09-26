//! Confirms `paco-codegen-cranelift` actually compiles the new channel MIR
//! shapes (task 3.1/3.2's lowering) into working calls against the real
//! `paco-runtime-ffi` functions — JIT-linked directly against this test
//! binary's own copy of them (no `paco-link`/static-library step involved;
//! that path is already covered by `paco-link/tests/concurrency_ffi.rs`).

use std::mem;

use cranelift_codegen::settings::Configurable;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::default_libcall_names;
use paco_diag::Reporter;
use paco_mir::{Body, Profile, TypeLayouts, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::{Item, Module};
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

use crate::ensure_runtime_init;

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
    builder.symbol("paco_rt_channel", paco_runtime_ffi::paco_rt_channel as *const u8);
    builder.symbol("paco_rt_send", paco_runtime_ffi::paco_rt_send as *const u8);
    builder.symbol("paco_rt_recv", paco_runtime_ffi::paco_rt_recv as *const u8);
    builder.symbol("paco_rt_sender_close", paco_runtime_ffi::paco_rt_sender_close as *const u8);
    builder.symbol("paco_rt_receiver_close", paco_runtime_ffi::paco_rt_receiver_close as *const u8);
    builder.symbol("paco_rt_join", paco_runtime_ffi::paco_rt_join as *const u8);
    builder.symbol("paco_rt_sender_retain", paco_runtime_ffi::paco_rt_sender_retain as *const u8);
    builder.symbol("paco_rt_sender_release", paco_runtime_ffi::paco_rt_sender_release as *const u8);
    builder.symbol("paco_rt_receiver_retain", paco_runtime_ffi::paco_rt_receiver_retain as *const u8);
    builder.symbol("paco_rt_receiver_release", paco_runtime_ffi::paco_rt_receiver_release as *const u8);
    builder.symbol("paco_rt_join_handle_retain", paco_runtime_ffi::paco_rt_join_handle_retain as *const u8);
    builder.symbol("paco_rt_join_handle_release", paco_runtime_ffi::paco_rt_join_handle_release as *const u8);
    builder.symbol("paco_rt_generator_retain", paco_runtime_ffi::paco_rt_generator_retain as *const u8);
    builder.symbol("paco_rt_generator_release", paco_runtime_ffi::paco_rt_generator_release as *const u8);
    JITModule::new(builder)
}

fn lower_all_functions(source: &str) -> (Vec<(String, Body)>, Module) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops =
        paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = TypeRegistry::from_module(&module);

    let bodies = module
        .items
        .iter()
        .flat_map(|item| match item {
            Item::Fn(function) => {
                let (body, outlined) =
                    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug);
                let mut result = vec![(function.name.clone(), body)];
                result.extend(outlined);
                result
            }
            _ => Vec::new(),
        })
        .collect();
    (bodies, module)
}

fn compile_and_run_main(source: &str) -> i64 {
    ensure_runtime_init();
    let (bodies, parsed_module) = lower_all_functions(source);
    let layouts = TypeLayouts::from_module(&parsed_module);
    let mut module = jit_module();
    let func_ids = paco_codegen_cranelift::declare_and_define(&mut module, &bodies, &[], &layouts).unwrap();
    module.finalize_definitions().unwrap();

    let main_id = func_ids["main"];
    let code = module.get_finalized_function(main_id);
    let main_fn = unsafe { mem::transmute::<*const u8, extern "C" fn() -> i64>(code) };
    main_fn()
}

#[test]
fn a_value_sent_on_a_channel_is_received_intact() {
    let output = compile_and_run_main(
        r#"
        enum Result { Ok(i64), Err(RecvError) }

        fn main() -> i64 {
            let (tx, rx) = channel<i64>(capacity: 4);
            let sent = tx.send(42);
            match rx.recv() {
                Result::Ok(value) => value,
                Result::Err(e) => -1,
            }
        }
        "#,
    );
    assert_eq!(output, 42);
}

#[test]
fn closing_a_sender_makes_recv_report_err() {
    let output = compile_and_run_main(
        r#"
        enum Result { Ok(i64), Err(RecvError) }

        fn main() -> i64 {
            let (tx, rx) = channel<i64>(capacity: 4);
            tx.close();
            match rx.recv() {
                Result::Ok(value) => value,
                Result::Err(e) => -1,
            }
        }
        "#,
    );
    assert_eq!(output, -1);
}
