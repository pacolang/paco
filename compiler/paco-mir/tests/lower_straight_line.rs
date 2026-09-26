use paco_diag::Reporter;
use paco_mir::{
    BasicBlock, BasicBlockId, BinOp, Body, CallTarget, LocalDecl, Operand, Place, Profile,
    Rvalue, Statement, Terminator,
};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::{IntWidth, Type, infer_module};

fn lower_source(source: &str) -> Body {
    lower_source_fn(source, "main")
}

fn lower_source_fn(source: &str, fn_name: &str) -> Body {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops = paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = paco_mir::TypeRegistry::from_module(&module);

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

#[test]
fn lowers_a_simple_arithmetic_function() {
    let body = lower_source_fn("fn add(a: i64, b: i64) -> i64 { let c = a + b; c }", "add");

    assert_eq!(
        body.locals,
        vec![
            LocalDecl {
                name: Some("a".to_string()),
                ty: Type::Int(IntWidth::I64),
                mutable: false
            },
            LocalDecl {
                name: Some("b".to_string()),
                ty: Type::Int(IntWidth::I64),
                mutable: false
            },
            LocalDecl {
                name: Some("c".to_string()),
                ty: Type::Int(IntWidth::I64),
                mutable: false
            },
        ]
    );

    let a = Place::Local(paco_mir::Local(0));
    let b = Place::Local(paco_mir::Local(1));
    let c = Place::Local(paco_mir::Local(2));
    assert_eq!(
        body.blocks,
        vec![BasicBlock {
            statements: vec![Statement::Assign(
                c.clone(),
                Rvalue::BinaryOp(BinOp::Add, Operand::Copy(a), Operand::Copy(b))
            )],
            terminator: Terminator::Return(Operand::Copy(c)),
        }]
    );
}

#[test]
fn lowers_a_call_into_a_terminator_and_resume_block() {
    let body = lower_source("fn f() -> i64 { 1 } fn main() -> i64 { let x = f(); x }");

    // The call temporary is local 0 (no params); `x` is local 1.
    let call_result = Place::Local(paco_mir::Local(0));
    let x = Place::Local(paco_mir::Local(1));

    assert_eq!(body.blocks.len(), 2, "a call must end its block");
    assert_eq!(
        body.blocks[0],
        BasicBlock {
            statements: Vec::new(),
            terminator: Terminator::Call {
                target: CallTarget("f".to_string()),
                args: Vec::new(),
                destination: Some(call_result.clone()),
                resume: BasicBlockId(1),
            },
        }
    );
    assert_eq!(
        body.blocks[1],
        BasicBlock {
            statements: vec![Statement::Assign(
                x.clone(),
                Rvalue::Use(Operand::Copy(call_result))
            )],
            terminator: Terminator::Return(Operand::Copy(x)),
        }
    );
}
