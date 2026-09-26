//! Confirms `paco-codegen-cranelift` compiles `iter fn`/`yield`/`.next()`
//! (task 6's reuse of thunk outlining) into working calls against the
//! real `paco-runtime-ffi` generator functions — JIT-linked directly,
//! matching `channel_ffi.rs`/`spawn_ffi.rs`/`select_ffi.rs`'s pattern.

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
    builder.symbol("paco_rt_spawn", paco_runtime_ffi::paco_rt_spawn as *const u8);
    builder.symbol(
        "paco_rt_receiver_is_ready",
        paco_runtime_ffi::paco_rt_receiver_is_ready as *const u8,
    );
    builder.symbol("paco_rt_generator_new", paco_runtime_ffi::paco_rt_generator_new as *const u8);
    builder.symbol(
        "paco_rt_generator_yield",
        paco_runtime_ffi::paco_rt_generator_yield as *const u8,
    );
    builder.symbol(
        "paco_rt_generator_next",
        paco_runtime_ffi::paco_rt_generator_next as *const u8,
    );
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

    let bodies: Vec<(String, Body)> = module
        .items
        .iter()
        .flat_map(|item| match item {
            Item::Fn(function) => {
                let (body, outlined) = if function.is_iter {
                    paco_mir::lower_iter_fn(function, &typed, &registry, &drops, Profile::Debug)
                } else {
                    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug)
                };
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
fn a_generator_yields_the_expected_sequence_across_bounded_pulls() {
    // The same bounded-pull pattern `phase-8-concurrency`'s own eval tests
    // use (`.take()`/`for`-over-iterator remain out of scope): pull a fixed
    // number of times and sum, rather than draining until exhaustion.
    let output = compile_and_run_main(
        r#"
        enum Option { Some(i64), None }

        iter fn counts_up(start: i64) -> i64 {
            yield start;
            yield start + 1;
            yield start + 2
        }

        fn main() -> i64 {
            let g = counts_up(10);
            let mut sum = 0;
            let a = g.next();
            match a {
                Option::Some(value) => { sum = sum + value }
                Option::None => {}
            }
            let b = g.next();
            match b {
                Option::Some(value) => { sum = sum + value }
                Option::None => {}
            }
            let c = g.next();
            match c {
                Option::Some(value) => { sum = sum + value }
                Option::None => {}
            }
            sum
        }
        "#,
    );
    // 10 + 11 + 12
    assert_eq!(output, 33);
}

#[test]
fn a_generator_reports_exhaustion_as_none() {
    let output = compile_and_run_main(
        r#"
        enum Option { Some(i64), None }

        iter fn one_value() -> i64 {
            yield 42
        }

        fn main() -> i64 {
            let g = one_value();
            let first = g.next();
            let second = g.next();
            match second {
                Option::Some(value) => value,
                Option::None => -1,
            }
        }
        "#,
    );
    assert_eq!(output, -1);
}
