use paco_diag::Reporter;
use paco_mir::{Body, CallTarget, Profile, Statement, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn lower_main(source: &str) -> (Body, Vec<(String, Body)>) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
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

    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug)
}

#[test]
fn spawn_captures_a_referenced_outer_variable_into_an_outlined_thunk() {
    let (body, outlined) = lower_main(
        r#"
        fn main() {
            let x = 5;
            let handle = spawn { x + 1 };
        }
        "#,
    );

    // Exactly one thunk was outlined.
    assert_eq!(outlined.len(), 1, "expected exactly one outlined spawn thunk");
    let (thunk_name, thunk_body) = &outlined[0];
    assert!(thunk_name.starts_with("__paco_spawn_thunk_"));

    // The thunk's own signature is fixed at 2 params (captures, result_out)
    // — matching `paco_rt_spawn`'s C ABI, which calls every thunk with
    // exactly these two pointers.
    assert_eq!(thunk_body.param_count, 2);
    assert_eq!(thunk_body.return_ty, paco_types::Type::Unit);

    // `x` was unpacked from the captures buffer into a local of its own,
    // bound under its original name.
    assert!(
        thunk_body.locals.iter().any(|local| local.name.as_deref() == Some("x")),
        "expected a local named `x` unpacked from the captures buffer, found: {:?}",
        thunk_body.locals
    );

    // The thunk writes its result through `result_out` (`Statement::
    // Store`), not `Terminator::Return` with a real value.
    let has_store = thunk_body
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .any(|statement| matches!(statement, Statement::Store { .. }));
    assert!(has_store, "expected the thunk to store its result through `result_out`");
    for block in &thunk_body.blocks {
        if let Terminator::Return(operand) = &block.terminator {
            assert_eq!(*operand, paco_mir::Operand::Constant(paco_mir::Constant::Unit));
        }
    }

    // The call site (in `main`'s own body) calls `paco_rt_spawn` with 4
    // arguments (thunk address, captures buffer, captures length, result
    // length) and a destination (the `JoinHandle` it hands back).
    let spawn_calls: Vec<_> = body
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Call { target, args, destination, .. }
                if target == &CallTarget("paco_rt_spawn".to_string()) =>
            {
                Some((args.len(), destination.is_some()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(spawn_calls, vec![(4, true)]);
}

#[test]
fn spawn_with_no_captures_outlines_a_thunk_with_an_empty_captures_buffer() {
    let (body, outlined) = lower_main(
        r#"
        fn main() {
            let handle = spawn { 42 };
        }
        "#,
    );
    assert_eq!(outlined.len(), 1);
    let (_, thunk_body) = &outlined[0];
    assert_eq!(thunk_body.param_count, 2);
    // No captured-variable locals beyond the 2 fixed params.
    assert_eq!(thunk_body.locals[0].name.as_deref(), Some("__captures"));
    assert_eq!(thunk_body.locals[1].name.as_deref(), Some("__result_out"));

    let spawn_calls = body
        .blocks
        .iter()
        .filter(|block| matches!(&block.terminator, Terminator::Call { target, .. } if target == &CallTarget("paco_rt_spawn".to_string())))
        .count();
    assert_eq!(spawn_calls, 1);
}
