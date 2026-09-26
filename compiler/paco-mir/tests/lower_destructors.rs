use paco_diag::Reporter;
use paco_mir::{Body, CallTarget, Local, Operand, Place, Profile, Statement, Terminator, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

/// `enum Shape { Circle(i64), Other }` gives a droppable (non-`Copy`)
/// local without needing string-literal lowering, which is out of this
/// crate's scope so far (see `lower.rs`'s `lower_literal`).
const SHAPE_DECL: &str = "enum Shape { Circle(i64), Other }";

fn lower_source_fn(source: &str, fn_name: &str) -> Body {
    let source = format!("{SHAPE_DECL}\n{source}");
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", &source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops =
        paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
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

fn local_of(body: &Body, name: &str) -> Local {
    let index = body
        .locals
        .iter()
        .position(|local| local.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no local named `{name}` in {body:#?}"));
    Local(index as u32)
}

fn drop_count_for(body: &Body, local: Local) -> usize {
    body.blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter(|statement| matches!(statement, Statement::Drop(Place::Local(l)) if *l == local))
        .count()
}

/// A block whose statements end with `local`'s `Drop`.
fn block_dropping(body: &Body, local: Local) -> &paco_mir::BasicBlock {
    body.blocks
        .iter()
        .find(|block| block.statements.last() == Some(&Statement::Drop(Place::Local(local))))
        .unwrap_or_else(|| panic!("no block ends with local {local:?}'s drop: {body:#?}"))
}

fn call_args(body: &Body, target: &str) -> Vec<Operand> {
    body.blocks
        .iter()
        .find_map(|block| match &block.terminator {
            Terminator::Call { target: CallTarget(name), args, .. } if name == target => Some(args.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no call to `{target}`"))
}

#[test]
fn destructor_call_on_normal_scope_exit() {
    let body = lower_source("fn main() { let s = Shape::Circle(5); }");
    let s = local_of(&body, "s");

    assert_eq!(drop_count_for(&body, s), 1);
    let block = block_dropping(&body, s);
    assert!(matches!(block.terminator, Terminator::Return(_)));
}

#[test]
fn destructor_call_on_early_return() {
    let body = lower_source("fn main() { let s = Shape::Circle(5); return; }");
    let s = local_of(&body, "s");

    let block = block_dropping(&body, s);
    assert!(matches!(block.terminator, Terminator::Return(_)));
}

#[test]
fn destructor_call_on_break() {
    let body = lower_source("fn main() { loop { let s = Shape::Circle(5); break; } }");
    let s = local_of(&body, "s");

    let block = block_dropping(&body, s);
    assert!(matches!(block.terminator, Terminator::Goto(_)));
}

#[test]
fn a_moved_local_is_passed_by_move() {
    let body = lower_source_fn(
        "fn consume(shape: Shape) { } fn main() { let s = Shape::Circle(5); consume(s); }",
        "main",
    );
    let s = local_of(&body, "s");

    assert_eq!(call_args(&body, "consume"), vec![Operand::Move(Place::Local(s))]);
}

#[test]
fn a_statement_temporary_is_dropped_at_the_end_of_its_statement() {
    let body = lower_source_fn(
        "fn make() -> Shape { Shape::Other } fn main() { make(); let t = Shape::Other; }",
        "main",
    );
    let temp = body
        .blocks
        .iter()
        .find_map(|block| match &block.terminator {
            Terminator::Call { destination: Some(Place::Local(temp)), .. } => Some(*temp),
            _ => None,
        })
        .expect("the call's result lands in a temporary");
    assert_eq!(drop_count_for(&body, temp), 1);
}
