use paco_diag::Reporter;
use paco_mir::{Body, Operand, Profile, Rvalue, Statement, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn lower_main(source: &str) -> Body {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    let layouts = paco_mir::TypeLayouts::from_module(&module);
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops = paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = TypeRegistry::from_module(&module);
    let function = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .unwrap();

    paco_mir::lower_function(function, &typed, &registry, &drops, &layouts, Profile::Debug).0
}

/// Storing an `extern "C" fn` item's name in a variable materializes its
/// address (`Rvalue::FuncAddr`) rather than panicking as an unresolved
/// local — the bug `lower_const` hit before a bare function-pointer-typed
/// identifier had anywhere else to go.
#[test]
fn storing_an_extern_c_fn_item_materializes_its_address() {
    let body = lower_main(
        r#"
        extern "C" fn add_ints(a: i64, b: i64) -> i64 {
            a + b
        }

        fn main() {
            let p: extern "C" fn(i64, i64) -> i64 = add_ints;
        }
        "#,
    );

    let has_func_addr = body
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .any(|statement| matches!(statement, Statement::Assign(_, Rvalue::FuncAddr(name)) if name == "add_ints"));
    assert!(has_func_addr, "expected `Rvalue::FuncAddr(\"add_ints\")`, found: {:?}", body.blocks);
}

/// Calling a C function pointer value dispatches to a plain indirect call
/// with exactly the declared arguments — unlike a closure call, it must
/// not load an environment pointer or prepend one as a hidden first
/// argument, since the value crossing to/from C is the bare code address.
#[test]
fn calling_a_stored_c_fn_pointer_emits_a_bare_indirect_call() {
    let body = lower_main(
        r#"
        extern "C" fn add_ints(a: i64, b: i64) -> i64 {
            a + b
        }

        fn main() {
            let p: extern "C" fn(i64, i64) -> i64 = add_ints;
            let result = unsafe { p(3, 4) };
        }
        "#,
    );

    let call = body.blocks.iter().find_map(|block| match &block.terminator {
        Terminator::CallIndirect { callee, args, .. } => Some((callee, args)),
        _ => None,
    });
    let (callee, args) = call.expect("expected a `Terminator::CallIndirect`");
    assert_eq!(args.len(), 2, "expected exactly the two declared arguments, found: {args:?}");
    assert!(
        matches!(callee, Operand::Copy(_)),
        "expected the callee operand to be the pointer value itself, found: {callee:?}"
    );
}
