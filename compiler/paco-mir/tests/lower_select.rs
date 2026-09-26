use paco_diag::Reporter;
use paco_mir::{CallTarget, Profile, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn lower_main(source: &str) -> paco_mir::Body {
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

    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug).0
}

fn call_count(body: &paco_mir::Body, name: &str) -> usize {
    body.blocks
        .iter()
        .filter(|block| matches!(&block.terminator, Terminator::Call { target, .. } if target == &CallTarget(name.to_string())))
        .count()
}

#[test]
fn select_with_default_lowers_to_a_readiness_check_and_recv_call() {
    let body = lower_main(
        r#"
        fn main() {
            let (tx, rx) = channel<i64>(capacity: 1);
            let value = select {
                v = rx.recv() => v,
                default => 0,
            };
        }
        "#,
    );

    // One readiness check per arm (here: one arm), and one `recv` call
    // reached only from that arm's block (the `default` block has none).
    assert_eq!(call_count(&body, "paco_rt_receiver_is_ready"), 1);
    assert_eq!(call_count(&body, "paco_rt_recv"), 1);
}

#[test]
fn select_without_default_is_a_named_not_yet_gap() {
    let result = std::panic::catch_unwind(|| {
        lower_main(
            r#"
            fn main() {
                let (tx, rx) = channel<i64>(capacity: 1);
                let value = select {
                    v = rx.recv() => v,
                };
            }
            "#,
        )
    });
    let error = result.expect_err("select without a default should not lower yet");
    let message = error.downcast_ref::<&str>().copied().unwrap_or_default();
    assert!(
        message.contains("default"),
        "expected a message naming the missing `default` arm, got: {message}"
    );
}
