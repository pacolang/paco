use cranelift_codegen::Context;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Module, default_libcall_names};
use paco_diag::Reporter;
use paco_mir::{Profile, TypeLayouts, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn compiled_ir_text(source: &str, profile: Profile) -> String {
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
    let layouts = TypeLayouts::from_module(&module);

    let function = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .expect("expected a `main` function");
    let (body, _outlined) = paco_mir::lower_function(function, &typed, &registry, &drops, profile);

    let mut flag_builder = cranelift_codegen::settings::builder();
    flag_builder.set("is_pic", "false").unwrap();
    let isa_builder = cranelift_native::builder().expect("host isa");
    let isa = isa_builder
        .finish(cranelift_codegen::settings::Flags::new(flag_builder))
        .unwrap();
    let mut jit_module = JITModule::new(crate::with_panic_symbols(JITBuilder::with_isa(isa, default_libcall_names())));
    let sig = {
        let mut sig = jit_module.make_signature();
        sig.returns.push(cranelift_codegen::ir::AbiParam::new(
            cranelift_codegen::ir::types::I64,
        ));
        sig
    };
    let func_id = jit_module
        .declare_function("main", cranelift_module::Linkage::Export, &sig)
        .unwrap();

    let mut ctx = Context::new();
    ctx.func.signature = sig;
    ctx.func.name = cranelift_codegen::ir::UserFuncName::user(0, func_id.as_u32());
    let mut fb_ctx = FunctionBuilderContext::new();
    let func_ids = std::collections::HashMap::from([("main".to_string(), func_id)]);
    paco_codegen_cranelift::compile_function(&mut jit_module, &mut ctx, &mut fb_ctx, &body, &func_ids, &layouts);

    ctx.func.to_string()
}

#[test]
fn debug_build_emits_a_checked_overflow_add() {
    let ir = compiled_ir_text("fn main() -> i64 { 1 + 2 }", Profile::Debug);
    assert!(ir.contains("sadd_overflow"), "{ir}");
    assert!(ir.contains("cold:") && ir.contains("call fn1"), "{ir}");
}

#[test]
fn release_build_emits_a_plain_wrapping_add() {
    let ir = compiled_ir_text("fn main() -> i64 { 1 + 2 }", Profile::Release);
    assert!(!ir.contains("sadd_overflow"), "{ir}");
    assert!(!ir.contains("cold:"), "{ir}");
    assert!(ir.contains("iadd"), "{ir}");
}

#[test]
fn debug_build_emits_an_unsigned_overflow_check_for_a_u8_addition() {
    let ir = compiled_ir_text(
        "fn main() -> i64 { let a: u8 = 200; let b: u8 = 100; (a + b) as i64 }",
        Profile::Debug,
    );
    assert!(ir.contains("uadd_overflow"), "{ir}");
    assert!(!ir.contains("sadd_overflow"), "{ir}");
    assert!(ir.contains("cold:") && ir.contains("call fn1"), "{ir}");
}

#[test]
fn release_build_wraps_a_u8_addition_without_a_check() {
    let ir = compiled_ir_text(
        "fn main() -> i64 { let a: u8 = 200; let b: u8 = 100; (a + b) as i64 }",
        Profile::Release,
    );
    assert!(!ir.contains("overflow"), "{ir}");
    assert!(!ir.contains("cold:"), "{ir}");
    assert!(ir.contains("iadd"), "{ir}");
}
