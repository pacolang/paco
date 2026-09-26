use paco_codegen_llvm::LlvmBackend;
use paco_diag::Reporter;
use paco_mir::{Backend, Body, Profile, Target, TypeLayouts};
use paco_span::SourceMap;
use paco_syntax::ast::{Item, Module};
use paco_syntax::{lex::lex, parse::parse_module};

fn conformance_source(path: &str) -> String {
    std::fs::read_to_string(format!("{}/../../tests/conformance/{path}/input.paco", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn parse(source: &str) -> Module {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));
    module
}

fn lower(module: &Module, profile: Profile) -> Vec<(String, Body)> {
    let mut reporter = Reporter::new();
    let typed = paco_types::infer_module(module, &mut reporter).expect("module should type-check");
    let drops = paco_borrow::analyze_module(module, &mut reporter).expect("module should borrow-check");
    let registry = paco_mir::TypeRegistry::from_module(module);
    let mut bodies = Vec::new();
    for item in &module.items {
        if let Item::Fn(function) = item {
            let (body, outlined) = paco_mir::lower_function(function, &typed, &registry, &drops, profile);
            bodies.push((function.name.clone(), body));
            bodies.extend(outlined);
        }
    }
    bodies
}

fn backend<'a>(bodies: &[(String, Body)], layouts: &'a TypeLayouts<'a>) -> LlvmBackend<'a> {
    let mut backend = LlvmBackend::new(Vec::new(), layouts);
    for (name, body) in bodies {
        backend.lower_body(name, body).unwrap();
    }
    backend
}

fn instruction_count(ir: &str) -> usize {
    ir.lines().filter(|line| line.starts_with("  ") && !line.trim_start().starts_with(';')).count()
}

#[test]
fn factorial_lowers_to_a_module_llvm_verifies() {
    let module = parse(&conformance_source("core/factorial"));
    let layouts = TypeLayouts::from_module(&module);
    let bodies = lower(&module, Profile::Debug);
    let ir = backend(&bodies, &layouts).ir(&Target { triple: None, profile: Profile::Debug }).unwrap();
    assert!(ir.contains("define i64 @fact(i64"), "{ir}");
}

#[test]
fn aggregate_copies_use_the_memcpy_intrinsic() {
    let module = parse(
        "struct Point { x: i64, y: i64 }\nfn shift(p: Point) -> Point { Point { x: p.x + 1, y: p.y } }\nfn main() { let p = shift(Point { x: 1, y: 2 }); print(p.x); }",
    );
    let layouts = TypeLayouts::from_module(&module);
    let bodies = lower(&module, Profile::Debug);
    let ir = backend(&bodies, &layouts).ir(&Target { triple: None, profile: Profile::Debug }).unwrap();
    assert!(ir.contains("call void @llvm.memcpy"), "{ir}");
}

#[test]
fn release_pipeline_shrinks_a_compute_heavy_program() {
    let module = parse(&conformance_source("core/collatz_steps"));
    let layouts = TypeLayouts::from_module(&module);
    let bodies = lower(&module, Profile::Release);
    let backend = backend(&bodies, &layouts);
    let unoptimized = backend.ir(&Target { triple: None, profile: Profile::Debug }).unwrap();
    let optimized = backend.ir(&Target { triple: None, profile: Profile::Release }).unwrap();
    assert!(!optimized.contains("alloca"), "mem2reg should promote every local:\n{optimized}");
    assert!(
        instruction_count(&optimized) * 2 < instruction_count(&unoptimized),
        "optimized: {}, unoptimized: {}",
        instruction_count(&optimized),
        instruction_count(&unoptimized)
    );
}

#[test]
fn release_arithmetic_wraps_instead_of_trapping() {
    let module = parse("fn add(a: i32, b: i32) -> i32 { a + b }\nfn main() { let a: i32 = 2147483647; let b: i32 = 1; print(add(a, b)); }");
    let layouts = TypeLayouts::from_module(&module);
    let ir = backend(&lower(&module, Profile::Release), &layouts).ir(&Target { triple: None, profile: Profile::Release }).unwrap();
    assert!(!ir.contains("with.overflow") && !ir.contains("llvm.trap"), "{ir}");
    let ir = backend(&lower(&module, Profile::Debug), &layouts).ir(&Target { triple: None, profile: Profile::Debug }).unwrap();
    assert!(ir.contains("llvm.sadd.with.overflow.i32"), "{ir}");
}

#[test]
fn cross_target_emits_an_object_for_that_architecture() {
    let module = parse(&conformance_source("core/factorial"));
    let layouts = TypeLayouts::from_module(&module);
    let bodies = lower(&module, Profile::Release);
    let target = Target { triple: Some("aarch64-unknown-linux-gnu".to_string()), profile: Profile::Release };
    let object = backend(&bodies, &layouts).finish(&target).unwrap();
    const EM_AARCH64: u16 = 183;
    assert_eq!(&object[..4], b"\x7fELF");
    assert_eq!(u16::from_le_bytes([object[18], object[19]]), EM_AARCH64);
}

#[test]
fn an_unknown_target_triple_is_a_diagnostic() {
    let module = parse(&conformance_source("core/factorial"));
    let layouts = TypeLayouts::from_module(&module);
    let bodies = lower(&module, Profile::Release);
    let target = Target { triple: Some("bogus-unknown-nowhere".to_string()), profile: Profile::Release };
    let error = backend(&bodies, &layouts).finish(&target).unwrap_err();
    assert!(error.starts_with("unsupported target triple `bogus-unknown-nowhere`"), "{error}");
}

#[test]
fn allocations_go_through_the_runtime_allocator_not_libc() {
    {
        let case = "slices, strings and closures";
        let module = parse("fn main() {\n    let mut cells: []i64 = slice_of_zeros<i64>(4);\n    cells[1] = 2;\n    let text = string_concat(&\"a\", &\"b\");\n    let offset = cells[1];\n    let add = |n: i64| n + offset;\n    print(add(1));\n    print(text)\n}\n");
        let layouts = TypeLayouts::from_module(&module);
        let bodies = lower(&module, Profile::Debug);
        let ir = backend(&bodies, &layouts).ir(&Target { triple: None, profile: Profile::Debug }).unwrap();
        for libc in ["@malloc(", "@calloc(", "@realloc(", "@free("] {
            assert!(!ir.contains(libc), "{case} references {libc}");
        }
        assert!(ir.contains("@paco_free("), "{case}");
    }
}
