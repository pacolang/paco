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
fn compiles_and_runs_a_simple_arithmetic_function() {
    assert_eq!(compile_and_run_main("fn main() -> i64 { 1 + 2 * 3 }"), 7);
}

#[test]
fn compiles_and_runs_a_function_with_a_let_binding() {
    assert_eq!(
        compile_and_run_main("fn main() -> i64 { let a = 10; let b = 3; a - b }"),
        7
    );
}

#[test]
fn compiles_and_runs_a_call_between_two_functions() {
    assert_eq!(
        compile_and_run_main("fn add(a: i64, b: i64) -> i64 { a + b } fn main() -> i64 { add(3, 4) }"),
        7
    );
}

#[test]
fn compiles_and_runs_an_if_expression() {
    assert_eq!(
        compile_and_run_main("fn main() -> i64 { if 1 == 1 { 7 } else { 0 } }"),
        7
    );
}

#[test]
fn compiles_and_runs_a_while_loop() {
    assert_eq!(
        compile_and_run_main(
            "fn main() -> i64 {
                let mut n = 0;
                while n < 7 {
                    n = n + 1;
                }
                n
            }"
        ),
        7
    );
}
