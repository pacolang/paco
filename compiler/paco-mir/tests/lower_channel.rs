use paco_diag::Reporter;
use paco_mir::{CallTarget, Profile, Rvalue, Statement, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn lower_main(source: &str) -> paco_mir::Body {
    lower_function(source, "main")
}

fn lower_function(source: &str, name: &str) -> paco_mir::Body {
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
            Item::Fn(function) if function.name == name => Some(function),
            _ => None,
        })
        .unwrap();

    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug).0
}

fn call_targets(body: &paco_mir::Body, name: &str) -> usize {
    body.blocks
        .iter()
        .filter(|block| matches!(&block.terminator, Terminator::Call { target, .. } if target == &CallTarget(name.to_string())))
        .count()
}

#[test]
fn channel_creation_lowers_to_a_paco_rt_channel_call_via_output_pointer_args() {
    let body = lower_main(
        r#"
        fn main() {
            let (tx, rx) = channel<i64>(capacity: 8);
        }
        "#,
    );

    // Exactly one call targets `paco_rt_channel`, with 3 arguments
    // (capacity, &sender-local, &receiver-local) and no destination
    // (results come back through the two pointer arguments, not a
    // returned value).
    let channel_calls: Vec<_> = body
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Call { target, args, destination, .. } if target == &CallTarget("paco_rt_channel".to_string()) => {
                Some((args.len(), destination.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(channel_calls, vec![(3, None)]);

    // The two output-pointer arguments are each produced by taking the
    // address of a local (`Rvalue::Ref`), not an ordinary value use.
    let ref_count = body
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter(|statement| matches!(statement, Statement::Assign(_, Rvalue::Ref { .. })))
        .count();
    assert_eq!(ref_count, 2, "expected one `Rvalue::Ref` per output-pointer argument");
}

#[test]
fn sender_send_lowers_to_a_paco_rt_send_call() {
    let body = lower_main(
        r#"
        enum Result<T, E> { Ok(T), Err(E) }

        fn main() {
            let (tx, rx) = channel<i64>(capacity: 8);
            let sent = tx.send(1);
        }
        "#,
    );
    assert_eq!(call_targets(&body, "paco_rt_send"), 1);
}

#[test]
fn sender_close_lowers_to_a_paco_rt_sender_close_call() {
    let body = lower_main(
        r#"
        fn main() {
            let (tx, rx) = channel<i64>(capacity: 8);
            tx.close()
        }
        "#,
    );
    assert_eq!(call_targets(&body, "paco_rt_sender_close"), 1);
}

#[test]
fn receiver_recv_lowers_to_a_paco_rt_recv_call() {
    let body = lower_main(
        r#"
        enum Result<T, E> { Ok(T), Err(E) }

        fn main() {
            let (tx, rx) = channel<i64>(capacity: 8);
            let received = rx.recv();
        }
        "#,
    );
    assert_eq!(call_targets(&body, "paco_rt_recv"), 1);
}

#[test]
fn join_handle_join_lowers_to_a_paco_rt_join_call() {
    // `JoinHandle<T>` reaches `.join()`'s lowering the same way regardless
    // of how the handle itself was produced — taking it as a plain
    // function parameter sidesteps `spawn`'s own lowering (a separate,
    // not-yet-implemented task at this point in the change), matching
    // design.md's own migration order ("(3) ... lowering for channel/
    // Sender/Receiver/JoinHandle first ... (4) spawn").
    let body = lower_function(
        r#"
        enum Result<T, E> { Ok(T), Err(E) }

        fn main(handle: JoinHandle<i64>) {
            let result = handle.join();
        }
        "#,
        "main",
    );
    assert_eq!(call_targets(&body, "paco_rt_join"), 1);
}
