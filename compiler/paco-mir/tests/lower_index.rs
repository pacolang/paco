//! `arrays-and-slices` task 4.1: `Expr::Index` lowering shape. `[]T`'s
//! built-in case becomes `Place::Index` (bounds-checked by codegen); a
//! structurally-dispatched user `index` method becomes a `Terminator::Call`
//! plus `Place::Deref` of its returned pointer.

use paco_diag::Reporter;
use paco_mir::{CallTarget, Operand, Place, Profile, Statement, Terminator, TypeRegistry};
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
    let main = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .unwrap();

    paco_mir::lower_function(main, &typed, &registry, &drops, Profile::Debug).0
}

fn assignments(body: &paco_mir::Body) -> Vec<&Statement> {
    body.blocks.iter().flat_map(|block| &block.statements).collect()
}

#[test]
fn slice_indexing_lowers_to_place_index() {
    let body = lower_main(
        r#"
        fn main() {
            let buf: []i64 = slice_of_zeros<i64>(4);
            let x: i64 = buf[0];
        }
        "#,
    );

    let found_place_index = assignments(&body).iter().any(|statement| {
        matches!(
            statement,
            Statement::Assign(_, paco_mir::Rvalue::Use(Operand::Copy(Place::Index { .. }) | Operand::Move(Place::Index { .. })))
        )
    });
    assert!(found_place_index, "expected a `Place::Index` read in the lowered body: {body:#?}");
}

#[test]
fn slice_index_assignment_stores_through_place_index() {
    let body = lower_main(
        r#"
        fn main() {
            let mut buf: []i64 = slice_of_zeros<i64>(4);
            buf[0] = 9
        }
        "#,
    );

    let found_store = assignments(&body)
        .iter()
        .any(|statement| matches!(statement, Statement::Assign(Place::Index { .. }, _)));
    assert!(found_store, "expected a `Place::Index` assignment target in the lowered body: {body:#?}");
}

#[test]
fn a_structurally_dispatched_index_method_lowers_to_a_call_and_a_deref_place() {
    let body = lower_main(
        r#"
        struct Bag {
            value: i64,

            fn index(&self, i: i64) -> &i64 {
                &self.value
            }
        }

        fn main() {
            let b = Bag { value: 7 };
            let x: i64 = b[0];
        }
        "#,
    );

    let call_count = body
        .blocks
        .iter()
        .filter(|block| matches!(&block.terminator, Terminator::Call { target, .. } if target == &CallTarget("Bag::index".to_string())))
        .count();
    assert_eq!(call_count, 1, "expected exactly one call to `Bag::index`: {body:#?}");

    let found_deref = assignments(&body).iter().any(|statement| {
        matches!(
            statement,
            Statement::Assign(_, paco_mir::Rvalue::Use(Operand::Copy(Place::Deref { .. }) | Operand::Move(Place::Deref { .. })))
        )
    });
    assert!(found_deref, "expected a `Place::Deref` read in the lowered body: {body:#?}");
}
