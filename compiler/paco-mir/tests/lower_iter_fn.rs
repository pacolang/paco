use paco_diag::Reporter;
use paco_mir::{Body, CallTarget, Profile, Statement, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn lower_module(source: &str) -> (paco_mir::Body, Vec<(String, Body)>, paco_mir::Body) {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops = paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = TypeRegistry::from_module(&module);

    let iter_fn = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.is_iter => Some(function),
            _ => None,
        })
        .unwrap();
    let main = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .unwrap();

    let (thunk_body, thunk_outlined) =
        paco_mir::lower_iter_fn(iter_fn, &typed, &registry, &drops, Profile::Debug);
    let (main_body, main_outlined) =
        paco_mir::lower_function(main, &typed, &registry, &drops, Profile::Debug);

    let mut outlined = thunk_outlined;
    outlined.extend(main_outlined);
    (main_body, outlined, thunk_body)
}

#[test]
fn iter_fn_body_outlines_into_a_captures_and_cancel_param_thunk() {
    let (_, _, thunk_body) = lower_module(
        r#"
        iter fn counts_up(start: i64) -> i64 {
            yield start;
            yield start + 1
        }

        fn main() {
            let g = counts_up(0);
        }
        "#,
    );

    // `(captures, cancelled)`, matching `paco_rt_generator_new`'s thunk ABI.
    assert_eq!(thunk_body.param_count, 2);
    assert_eq!(thunk_body.return_ty, paco_types::Type::Unit);

    // `start` (the iter fn's own declared parameter) was unpacked from the
    // captures buffer into a local bound under its own name.
    assert!(
        thunk_body.locals.iter().any(|local| local.name.as_deref() == Some("start")),
        "expected a local named `start` unpacked from the captures buffer, found: {:?}",
        thunk_body.locals
    );

    // Each `yield` lowers to a `paco_rt_generator_yield` call.
    let yield_calls = thunk_body
        .blocks
        .iter()
        .filter(|block| matches!(&block.terminator, Terminator::Call { target, .. } if target == &CallTarget("paco_rt_generator_yield".to_string())))
        .count();
    assert_eq!(yield_calls, 2, "expected one `paco_rt_generator_yield` call per `yield`");
}

#[test]
fn calling_an_iter_fn_lowers_to_a_paco_rt_generator_new_call() {
    let (main_body, _, _) = lower_module(
        r#"
        iter fn counts_up(start: i64) -> i64 {
            yield start
        }

        fn main() {
            let g = counts_up(5);
        }
        "#,
    );

    let new_calls: Vec<_> = main_body
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Call { target, args, destination, .. }
                if target == &CallTarget("paco_rt_generator_new".to_string()) =>
            {
                Some((args.len(), destination.is_some()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(new_calls, vec![(3, true)]);

    // The single call argument (5) was stored into the captures buffer.
    let store_count = main_body
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter(|statement| matches!(statement, Statement::Store { .. }))
        .count();
    assert_eq!(store_count, 1, "expected the argument to be stored into the captures buffer");
}

#[test]
fn generator_next_lowers_to_a_paco_rt_generator_next_call() {
    let (main_body, _, _) = lower_module(
        r#"
        enum Option { Some(i64), None }

        iter fn counts_up(start: i64) -> i64 {
            yield start
        }

        fn main() {
            let g = counts_up(5);
            let first = g.next();
        }
        "#,
    );

    let next_calls = main_body
        .blocks
        .iter()
        .filter(|block| matches!(&block.terminator, Terminator::Call { target, .. } if target == &CallTarget("paco_rt_generator_next".to_string())))
        .count();
    assert_eq!(next_calls, 1);
}
