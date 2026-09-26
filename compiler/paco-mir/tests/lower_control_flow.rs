use paco_diag::Reporter;
use paco_mir::{
    BasicBlockId, Body, Operand, Place, Profile, Rvalue, Statement, Terminator, TypeRegistry,
};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn lower_source_fn(source: &str, fn_name: &str) -> Body {
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
            Item::Fn(function) if function.name == fn_name => Some(function),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a function item named `{fn_name}`"));
    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug).0
}

fn lower_source(source: &str) -> Body {
    lower_source_fn(source, "main")
}

/// A block's terminator, for asserting shape without pinning every operand.
fn terminators(body: &Body) -> Vec<&Terminator> {
    body.blocks.iter().map(|block| &block.terminator).collect()
}

#[test]
fn lowers_if_to_a_switch_with_a_join_block() {
    let body = lower_source("fn main() -> i64 { if true { 1 } else { 2 } }");

    // cond block, then block, else block, join block.
    assert_eq!(body.blocks.len(), 4);
    assert!(matches!(
        body.blocks[0].terminator,
        Terminator::SwitchInt {
            targets: ref t,
            otherwise: BasicBlockId(2),
            ..
        } if t == &[(1, BasicBlockId(1))]
    ));
    assert_eq!(body.blocks[1].terminator, Terminator::Goto(BasicBlockId(3)));
    assert_eq!(body.blocks[2].terminator, Terminator::Goto(BasicBlockId(3)));
    // The join block reads back whatever the taken branch assigned.
    assert!(matches!(
        body.blocks[3].terminator,
        Terminator::Return(Operand::Copy(_))
    ));
}

#[test]
fn lowers_while_with_a_back_edge_and_exit() {
    let body = lower_source("fn main() { while true { } }");

    let terms = terminators(&body);
    // entry -> header, header -> {body, exit} switch, body -> header, exit -> return.
    assert_eq!(terms.len(), 4);
    assert_eq!(*terms[0], Terminator::Goto(BasicBlockId(1)));
    assert!(matches!(*terms[1], Terminator::SwitchInt { .. }));
    assert_eq!(*terms[2], Terminator::Goto(BasicBlockId(1)), "loop body must jump back to the header");
    assert_eq!(*terms[3], Terminator::Return(Operand::Constant(paco_mir::Constant::Unit)));
}

#[test]
fn lowers_loop_with_break_as_the_only_exit() {
    let body = lower_source("fn main() { loop { break; } }");

    // entry -> header, header (loop body, ends in break) -> exit, dead block after break, exit -> return.
    let terms = terminators(&body);
    assert_eq!(*terms[0], Terminator::Goto(BasicBlockId(1)));
    // header's body is just `break;`, so it jumps straight to the exit block.
    let exit = match terms[1] {
        Terminator::Goto(id) => *id,
        other => panic!("expected break to lower to a Goto, got {other:?}"),
    };
    assert_eq!(*terms[exit.0 as usize], Terminator::Return(Operand::Constant(paco_mir::Constant::Unit)));
}

#[test]
fn continue_jumps_to_the_loop_header_not_the_exit() {
    let body = lower_source("fn main() { loop { continue; } }");

    let terms = terminators(&body);
    // header block (block 1) contains just `continue;`, which must Goto back to itself (the header).
    assert_eq!(*terms[1], Terminator::Goto(BasicBlockId(1)));
}

#[test]
fn lowers_match_over_an_enum_with_a_wildcard_arm() {
    let body = lower_source(
        r#"
        enum Shape {
            Circle(i64),
            Other,
        }
        fn main() -> i64 {
            let s = Shape::Circle(5);
            match s {
                Shape::Circle(r) => r,
                _ => 0,
            }
        }
        "#,
    );

    // Find the SwitchInt over the enum's discriminant.
    let switch = body
        .blocks
        .iter()
        .find_map(|block| match &block.terminator {
            Terminator::SwitchInt {
                targets, otherwise, ..
            } => Some((targets.clone(), *otherwise)),
            _ => None,
        })
        .expect("expected a SwitchInt lowered from the match");
    // `Circle` is variant 0 (declared first); the wildcard arm is `otherwise`,
    // a different block.
    assert_eq!(switch.0.len(), 1);
    assert_eq!(switch.0[0].0, 0);
    assert_ne!(switch.0[0].1, switch.1);

    // Somewhere, the `Circle` arm reads its payload's field 0 into a local
    // and returns it (matching `r`).
    let has_variant_field_read = body.blocks.iter().any(|block| {
        block.statements.iter().any(|statement| matches!(
            statement,
            Statement::Assign(_, Rvalue::Use(Operand::Copy(Place::VariantField { index: 0, .. }) | Operand::Move(Place::VariantField { index: 0, .. })))
        ))
    });
    assert!(
        has_variant_field_read,
        "expected the Circle arm to project its payload field: {body:#?}"
    );

    // And the enum's discriminant is read via Rvalue::Discriminant somewhere.
    let has_discriminant_read = body.blocks.iter().any(|block| {
        block
            .statements
            .iter()
            .any(|statement| matches!(statement, Statement::Assign(_, Rvalue::Discriminant(_))))
    });
    assert!(has_discriminant_read);
}
