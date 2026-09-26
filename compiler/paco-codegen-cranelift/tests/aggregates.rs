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
fn compiles_and_runs_struct_field_construction_and_access() {
    let result = compile_and_run_main(
        "struct Point { x: i64, y: i64 }
        fn main() -> i64 {
            let p = Point { x: 3, y: 4 };
            p.x + p.y
        }",
    );
    assert_eq!(result, 7);
}

#[test]
fn compiles_and_runs_a_nested_struct_field_chain() {
    let result = compile_and_run_main(
        "struct Inner { v: i64 }
        struct Outer { inner: Inner }
        fn main() -> i64 {
            let o = Outer { inner: Inner { v: 7 } };
            o.inner.v
        }",
    );
    assert_eq!(result, 7);
}

#[test]
fn compiles_and_runs_enum_construction_and_match() {
    let result = compile_and_run_main(
        "enum Shape { Circle(i64), Square(i64) }
        fn main() -> i64 {
            match Shape::Circle(7) {
                Shape::Circle(r) => r,
                Shape::Square(side) => side,
            }
        }",
    );
    assert_eq!(result, 7);
}

#[test]
fn compiles_and_runs_a_function_taking_a_struct_by_value() {
    let result = compile_and_run_main(
        "struct Point { x: i64, y: i64 }
        fn sum(p: Point) -> i64 { p.x + p.y }
        fn main() -> i64 { sum(Point { x: 3, y: 4 }) }",
    );
    assert_eq!(result, 7);
}
